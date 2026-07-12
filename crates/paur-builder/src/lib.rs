//! paur-builder: spawn a build container and stream its output.
//!
//! The crate's only job is the *bridge* between the daemon and the
//! container runtime. It does not decide which package to build, when
//! to build it, or what to do with the result — those concerns live
//! in `paur-daemon`.
//!
//! Stream model:
//! - The container's combined stdout+stderr is read line-by-line off an
//!   async reader.
//! - Each line is handed to a [`LogSink`] for persistence (DB row +
//!   text file) and fan-out to live Web UI clients.
//! - The container's exit status is reported via [`BuildOutcome::exit_code`].

use std::path::{Path, PathBuf};
use std::process::Stdio;

use async_trait::async_trait;
use paur_aur::SrcInfo;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use paur_core::{ContainerRuntime, PackageBuildFlags, PkgName, Variant};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// What to build, where to put artifacts, and how to find the runtime.
#[derive(Debug, Clone)]
pub struct BuildRequest {
    /// Package name (used for log labels and the workdir basename).
    pub pkg: PkgName,
    /// AUR git URL.
    pub aur_url: String,
    /// Per-build work directory; the daemon hands out a fresh one.
    pub work_dir: PathBuf,
    /// ccache dir to bind-mount into the container.
    pub ccache_dir: PathBuf,
    /// Container runtime to invoke.
    pub runtime: ContainerRuntime,
    /// Container image name (default: `paur-builder:latest`).
    pub image: String,
    /// Per-package build tuning flags. Applied as:
    /// - `low_memory`             → `MAKEFLAGS=-j2`
    /// - `rust_codegen_units_1`   → appends `-C codegen-units=1` to `RUSTFLAGS`
    /// - `no_ccache`              → skips the ccache bind mount
    /// Empty flags are a no-op (use the daemon default).
    pub flags: PackageBuildFlags,
    /// Which compiled variant this build is for. The daemon picks
    /// the variant from the package's `PackageVariants` at enqueue
    /// time; the builder turns it into the `PAUR_MARCH` env that
    /// `build.sh`'s `apply_march` reads.
    pub variant: Variant,
}

/// What the build produced.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildOutcome {
    /// Container exit code (0 = success).
    pub exit_code: i32,
    /// Paths (relative to `work_dir/out`) of produced `.pkg.tar.*` files.
    pub pkg_files: Vec<PathBuf>,
    /// Parsed `.SRCINFO` if makepkg got that far. `None` for failed builds.
    pub srcinfo: Option<SrcInfo>,
    /// `true` if the build was cancelled via the cancel token
    /// (i.e. the container was killed by the daemon, not by the
    /// container itself exiting). Callers should record the
    /// build as `Cancelled` rather than `Failed` in this case —
    /// the exit code is whatever `kill` produces and not
    /// meaningful.
    #[serde(default)]
    pub cancelled: bool,
}

/// A pluggable destination for each log line produced by a build.
#[async_trait]
pub trait LogSink: Send + Sync {
    /// Called once for every line the container emits. The `stream`
    /// field is informational; we always send both stdout and stderr
    /// together in combined mode.
    async fn write(&self, line: &str) -> Result<(), paur_core::Error>;
}

/// A no-op sink useful in tests.
pub struct NullSink;

#[async_trait]
impl LogSink for NullSink {
    async fn write(&self, _line: &str) -> Result<(), paur_core::Error> {
        Ok(())
    }
}

/// A sink that accumulates lines into an in-memory buffer. Test helper.
#[derive(Default, Clone)]
pub struct CollectingSink {
    pub lines: std::sync::Arc<tokio::sync::Mutex<Vec<String>>>,
}

#[async_trait]
impl LogSink for CollectingSink {
    async fn write(&self, line: &str) -> Result<(), paur_core::Error> {
        self.lines.lock().await.push(line.to_string());
        Ok(())
    }
}

/// Run a build in a container. Streams logs into `sink`. Returns the
/// outcome (exit code + artifact paths).
///
/// `cancel` is a `CancellationToken` the caller can fire to kill the
/// container mid-build. Firing it sets `BuildOutcome.cancelled = true`
/// and returns the exit code produced by `kill` (typically negative
/// or `137` on SIGKILL). When the container finishes naturally the
/// token is left untouched.
pub async fn run_in_container(
    req: &BuildRequest,
    sink: std::sync::Arc<dyn LogSink>,
    cancel: CancellationToken,
) -> paur_core::Result<BuildOutcome> {
    // Pre-create the expected on-disk layout. The container is bind-
    // mounted to /work and /ccache, both of which must exist.
    // Use 0o777 so the container's `builder` user (whose uid will
    // not match the host's paur user) can still write into the
    // bind-mounted directories.
    std::fs::create_dir_all(&req.work_dir)?;
    std::fs::set_permissions(&req.work_dir, std::fs::Permissions::from_mode(0o777))?;
    std::fs::create_dir_all(req.work_dir.join("out"))?;
    std::fs::set_permissions(
        req.work_dir.join("out"),
        std::fs::Permissions::from_mode(0o777),
    )?;
    // ccache dir is only needed when the build is allowed to use it.
    // For `no_ccache` packages, we still create the dir on the host
    // (so the daemon's directory layout stays predictable) but skip
    // the bind mount + CCACHE_DIR env below.
    if !req.flags.no_ccache {
        std::fs::create_dir_all(&req.ccache_dir)?;
        std::fs::set_permissions(
            &req.ccache_dir,
            std::fs::Permissions::from_mode(0o777),
        )?;
    }

    // Build the docker/podman command line. Keep it short and
    // explicit; do not introduce configuration knobs until a use case
    // demands them.
    let bin = req.runtime.binary();
    let mut cmd = Command::new(bin);
    cmd.args(["run", "--rm"])
        .arg("-v")
        .arg(format!("{}:/work", req.work_dir.display()));
    if !req.flags.no_ccache {
        cmd.arg("-v")
            .arg(format!("{}:/ccache", req.ccache_dir.display()))
            .arg("-e")
            .arg("CCACHE_DIR=/ccache");
    }
    if req.flags.low_memory {
        // Cap parallel make jobs to cut peak RAM. -j2 is conservative
        // enough to avoid OOM on the smallest hosts while still
        // parallelizing the long-running link step.
        cmd.arg("-e").arg("MAKEFLAGS=-j2");
    }
    if req.flags.rust_codegen_units_1 {
        // Append to whatever RUSTFLAGS the PKGBUILD may have set;
        // rustc takes the *last* -C codegen-units=1 win, so the
        // package-level setting still wins. We do this via a shell
        // expansion in build.sh instead of the container env, so
        // the inner makepkg can re-export RUSTFLAGS as needed.
        cmd.arg("-e").arg("PAUR_RUST_CGU=1");
    }
    if let Some(level) = req.variant.as_paur_march() {
        // Pass the *level name* (v3 / v4) rather than the resolved
        // CFLAGS. build.sh owns the actual recipe (CachyOS-style
        // `-march=x86-64-vN -O2 -pipe -fno-plt`, CXXFLAGS=${CFLAGS},
        // RUSTFLAGS append), so changing the recipe does not
        // require a daemon rebuild. `Default` builds skip the env
        // entirely so the container uses its stock makepkg.conf.
        cmd.arg("-e")
            .arg(format!("PAUR_MARCH={}", level));
    }
    cmd.arg(&req.image)
        .arg(req.pkg.as_str())
        .arg(&req.aur_url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    tracing::info!(
        runtime = bin,
        image = %req.image,
        pkg = %req.pkg,
        work_dir = %req.work_dir.display(),
        "spawning build container"
    );

    let mut child = cmd.spawn().map_err(|e| {
        paur_core::Error::Build(format!("failed to spawn {bin} run: {e}"))
    })?;

    // We must take stdout and stderr *before* awaiting, otherwise they
    // may be dropped when child is awaited. We merge them by reading
    // both concurrently into the sink. (The container's makepkg
    // interleaves them by default anyway; an interleaved stream is
    // fine for log storage and the Web UI.)
    let stdout = child.stdout.take().ok_or_else(|| {
        paur_core::Error::Build("child stdout not captured".into())
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        paur_core::Error::Build("child stderr not captured".into())
    })?;

    let out_task = {
        let sink = std::sync::Arc::clone(&sink);
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let _ = sink.write(&line).await;
            }
        })
    };
    let err_task = {
        let sink = std::sync::Arc::clone(&sink);
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let _ = sink.write(&line).await;
            }
        })
    };

    // Wait for the container to exit, OR for the cancel token to
    // fire. If the token wins the race, we kill the container and
    // let `wait()` reap it; the resulting `cancelled = true` flag
    // tells the caller to record the build as `Cancelled` rather
    // than `Failed` (the exit code is whatever `kill` produces and
    // is not meaningful for a user-facing status).
    let mut child = child;
    let cancelled;
    let status = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            tracing::info!(pkg = %req.pkg, "build cancelled: killing container");
            // `start_kill` is non-blocking; `wait` will then reap
            // the child. The docker/podman `--rm` flag means the
            // container is removed automatically on exit.
            let _ = child.start_kill();
            cancelled = true;
            child.wait().await.map_err(|e| {
                paur_core::Error::Build(format!("waiting on {bin} run after kill: {e}"))
            })?
        }
        s = child.wait() => {
            cancelled = false;
            s.map_err(|e| {
                paur_core::Error::Build(format!("waiting on {bin} run: {e}"))
            })?
        }
    };
    // Drain log tasks (they should already be EOF by now).
    let _ = out_task.await;
    let _ = err_task.await;

    let exit_code = status.code().unwrap_or(-1);
    tracing::info!(pkg = %req.pkg, exit_code, cancelled, "container finished");

    let pkg_files = if exit_code == 0 && !cancelled {
        list_artifacts(&req.work_dir.join("out"))?
    } else {
        Vec::new()
    };

    // Attempt to read .SRCINFO from the cloned source. We try this on
    // success *and* on failure so a partial build still records what
    // got produced.
    let srcinfo = req.work_dir.join("src").join(".SRCINFO");
    let srcinfo = if srcinfo.is_file() {
        std::fs::read_to_string(&srcinfo)
            .ok()
            .and_then(|s| paur_aur::srcinfo::parse(&s).ok())
    } else {
        None
    };

    Ok(BuildOutcome {
        exit_code,
        pkg_files,
        srcinfo,
        cancelled,
    })
}

/// List `.pkg.tar.*` files under `out_dir`, sorted for determinism.
fn list_artifacts(out_dir: &Path) -> paur_core::Result<Vec<PathBuf>> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(out_dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.ends_with(".pkg.tar") || n.contains(".pkg.tar."))
                .unwrap_or(false)
        })
        .collect();
    entries.sort();
    Ok(entries)
}

/// What to ask the container to do to the repo DB. We split the
/// repo-update flow in two so the GPG private key can stay on the
/// host: the container just runs `repo-add` / `repo-remove` and the
/// daemon signs the result.
#[derive(Debug, Clone)]
pub struct RepoOpRequest {
    /// What to do.
    pub op: RepoOp,
    /// Bind-mounted architecture-specific repo directory
    /// (e.g. `/var/lib/paur/repo/x86_64-v3`). The container
    /// treats this as the working dir for `repo-add` /
    /// `repo-remove`.
    pub arch_dir: PathBuf,
    /// Bind-mounted staging directory containing the .pkg.tar.*
    /// files to register. Required for `Add`, ignored otherwise.
    pub stage_dir: PathBuf,
    /// Repo DB basename (e.g. `paur-v3.db.tar.gz`).
    pub db_name: String,
    /// For `Add`: package file names (relative to `stage_dir`).
    /// For `Remove`: the package name registered in the DB.
    pub names: Vec<String>,
    /// Container runtime to invoke.
    pub runtime: ContainerRuntime,
    /// Container image (defaults to `paur-builder:latest`).
    pub image: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoOp {
    /// `repo-add` — register the listed packages into the DB.
    Add,
    /// `repo-remove` — drop the named package from the DB.
    Remove,
}

impl RepoOp {
    fn as_str(self) -> &'static str {
        match self {
            RepoOp::Add => "repo",
            RepoOp::Remove => "unrepo",
        }
    }
}

/// Run a repo DB update inside the builder container. The container
/// only sees the repo dir and the staging dir; the GPG keyring stays
/// on the host. Returns the container's exit code (0 = success).
pub async fn run_repo_op(req: &RepoOpRequest) -> paur_core::Result<i32> {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(&req.arch_dir)?;
    std::fs::set_permissions(&req.arch_dir, std::fs::Permissions::from_mode(0o777))?;
    std::fs::create_dir_all(&req.stage_dir)?;
    std::fs::set_permissions(
        &req.stage_dir,
        std::fs::Permissions::from_mode(0o777),
    )?;

    let bin = req.runtime.binary();
    let mut cmd = Command::new(bin);
    cmd.args(["run", "--rm"])
        .arg("-v")
        .arg(format!("{}:/work/repo", req.arch_dir.display()))
        .arg("-v")
        .arg(format!("{}:/work/stage", req.stage_dir.display()))
        .arg(&req.image)
        .arg(req.op.as_str())
        .arg(&req.db_name)
        .arg(req.arch_dir.to_string_lossy().as_ref());
    for n in &req.names {
        // repo-add takes paths; repo-remove takes a bare name. We
        // pass the same value either way and let the script sort it
        // out (the script distinguishes via $#).
        let v = if req.op == RepoOp::Add {
            format!("/work/stage/{n}")
        } else {
            n.clone()
        };
        cmd.arg(v);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    tracing::info!(
        runtime = bin,
        image = %req.image,
        op = req.op.as_str(),
        arch_dir = %req.arch_dir.display(),
        stage_dir = %req.stage_dir.display(),
        "spawning repo-op container"
    );

    let mut child = cmd.spawn().map_err(|e| {
        paur_core::Error::Repo(format!("failed to spawn {bin} run: {e}"))
    })?;

    let stdout = child.stdout.take().ok_or_else(|| {
        paur_core::Error::Repo("child stdout not captured".into())
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        paur_core::Error::Repo("child stderr not captured".into())
    })?;
    let out_task = tokio::spawn(async move {
        let mut r = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = r.next_line().await {
            tracing::info!(target: "repo_op", "{line}");
        }
    });
    let err_task = tokio::spawn(async move {
        let mut r = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = r.next_line().await {
            tracing::warn!(target: "repo_op", "{line}");
        }
    });

    let status = child.wait().await.map_err(|e| {
        paur_core::Error::Repo(format!("waiting on {bin} run: {e}"))
    })?;
    let _ = out_task.await;
    let _ = err_task.await;
    let exit_code = status.code().unwrap_or(-1);
    if exit_code != 0 {
        return Err(paur_core::Error::Repo(format!(
            "repo-op container exited {exit_code}"
        )));
    }
    Ok(exit_code)
}

/// What to build from a *local* PKGBUILD directory. Used by
/// `keyring-build` to produce the `paur-keyring` and `paur-mirrorlist`
/// meta-packages without going through AUR.
#[derive(Debug, Clone)]
pub struct LocalBuildRequest {
    /// Display name (used for log labels and the workdir basename).
    pub label: String,
    /// Host path to a directory containing a single `PKGBUILD`. The
    /// entire directory is bind-mounted to `/work/src` in the
    /// container.
    pub pkgbuild_dir: PathBuf,
    /// Per-build work directory; the daemon (or CLI) hands out a
    /// fresh one.
    pub work_dir: PathBuf,
    /// Host path to a scratch directory mounted as `/work/build`
    /// inside the container. This is a fresh `mktemp -d` per
    /// invocation so makepkg can write its `pkg/`/`src/` trees and
    /// the final `.pkg.tar.*` without conflicting with leftover
    /// files from a previous run.
    pub tmp_build_dir: PathBuf,
    /// ccache dir to bind-mount into the container.
    pub ccache_dir: PathBuf,
    /// Container runtime to invoke.
    pub runtime: ContainerRuntime,
    /// Container image name.
    pub image: String,
}

/// Run a local PKGBUILD build in a container. Unlike `run_in_container`
/// this skips the AUR clone step — the host is expected to have
/// already laid out a directory containing the PKGBUILD and any
/// auxiliary files (`.install`, sources, etc.).
pub async fn run_local_in_container(
    req: &LocalBuildRequest,
    sink: std::sync::Arc<dyn LogSink>,
) -> paur_core::Result<BuildOutcome> {
    std::fs::create_dir_all(&req.work_dir)?;
    std::fs::set_permissions(&req.work_dir, std::fs::Permissions::from_mode(0o777))?;
    std::fs::create_dir_all(req.work_dir.join("out"))?;
    std::fs::set_permissions(
        req.work_dir.join("out"),
        std::fs::Permissions::from_mode(0o777),
    )?;
    std::fs::create_dir_all(&req.ccache_dir)?;
    std::fs::set_permissions(
        &req.ccache_dir,
        std::fs::Permissions::from_mode(0o777),
    )?;

    let bin = req.runtime.binary();
    let mut cmd = Command::new(bin);
    cmd.args(["run", "--rm"])
        // Mount the work dir at /work so the container's `builder`
        // user has a writable root to copy the PKGBUILD into. The
        // Dockerfile declares `VOLUME ["/work"]`, which without a
        // bind mount creates an anonymous root-owned volume that
        // `builder` cannot write to. Mounting first lets the more
        // specific /work/src and /work/out mounts layer on top.
        .arg("-v")
        .arg(format!("{}:/work", req.work_dir.display()))
        // Layer a tmpfs at /work/build on top of the work-dir
        // bind mount. makepkg's `pkg/` and `src/` trees (and the
        // .pkg.tar.* output) live in /work/build; keeping them in
        // a tmpfs avoids two issues we hit with the work_dir bind
        // mount:
        //   1. The host-side work_dir is owned by paur with mode
        //      0o777, but makepkg's intermediate files end up
        //      owned by the container's `builder` uid, which
        //      leaves stray files at the work_dir root between
        //      builds.
        //   2. makepkg resolves PKGDEST relative to its view of
        //      "the work dir" — when /work is itself a bind mount,
        //      some makepkg versions put .pkg.tar.* at /work
        //      instead of /work/build. Putting /work/build on a
        //      tmpfs makes that location a fresh, owned-by-builder
        //      directory every time.
        // The output is collected by build.sh and moved to
        // /work/out (which is layered back over the host's
        // work_dir/out).
        .arg("-v")
        .arg(format!("{}:/work/build:rw", req.tmp_build_dir.display()))
        // /work/src is the PKGBUILD dir; the local-build entrypoint
        // is hardcoded to look there. Mounted read-only because
        // makepkg copies it into /work/build before building.
        .arg("-v")
        .arg(format!("{}:/work/src:ro", req.pkgbuild_dir.display()))
        .arg("-v")
        .arg(format!("{}:/work/out", req.work_dir.join("out").display()))
        .arg("-v")
        .arg(format!("{}:/ccache", req.ccache_dir.display()))
        .arg("-e")
        .arg("CCACHE_DIR=/ccache")
        .arg(&req.image)
        .arg("local") // build-local.sh branch
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    tracing::info!(
        runtime = bin,
        image = %req.image,
        label = %req.label,
        pkgbuild_dir = %req.pkgbuild_dir.display(),
        work_dir = %req.work_dir.display(),
        "spawning local build container"
    );

    let mut child = cmd.spawn().map_err(|e| {
        paur_core::Error::Build(format!("failed to spawn {bin} run: {e}"))
    })?;

    let stdout = child.stdout.take().ok_or_else(|| {
        paur_core::Error::Build("child stdout not captured".into())
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        paur_core::Error::Build("child stderr not captured".into())
    })?;

    let out_task = {
        let sink = std::sync::Arc::clone(&sink);
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let _ = sink.write(&line).await;
            }
        })
    };
    let err_task = {
        let sink = std::sync::Arc::clone(&sink);
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let _ = sink.write(&line).await;
            }
        })
    };

    let status = child.wait().await.map_err(|e| {
        paur_core::Error::Build(format!("waiting on {bin} run: {e}"))
    })?;
    let _ = out_task.await;
    let _ = err_task.await;

    let exit_code = status.code().unwrap_or(-1);
    tracing::info!(label = %req.label, exit_code, "local build container finished");

    // Local builds don't have a real .SRCINFO until we parse the
    // PKGBUILD — but we don't need one for repo-add (which only
    // inspects the .pkg.tar.* files).
    let pkg_files = if exit_code == 0 {
        list_artifacts(&req.work_dir.join("out"))?
    } else {
        Vec::new()
    };

    Ok(BuildOutcome {
        exit_code,
        pkg_files,
        srcinfo: None,
        cancelled: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn list_artifacts_empty_dir() {
        let dir = tempdir().unwrap();
        let a = list_artifacts(dir.path()).unwrap();
        assert!(a.is_empty());
    }

    #[test]
    fn list_artifacts_filters() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("foo-1.0-1-x86_64.pkg.tar.zst"), b"").unwrap();
        std::fs::write(dir.path().join("foo-1.0-1-x86_64.pkg.tar.zst.sig"), b"").unwrap();
        std::fs::write(dir.path().join("not-a-package.txt"), b"").unwrap();
        let a = list_artifacts(dir.path()).unwrap();
        assert_eq!(a.len(), 2);
        for p in &a {
            let n = p.file_name().unwrap().to_str().unwrap();
            assert!(n.contains(".pkg.tar."));
        }
    }
}
