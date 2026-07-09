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

use paur_core::{Config, ContainerRuntime, PkgName};

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
pub async fn run_in_container(
    req: &BuildRequest,
    sink: std::sync::Arc<dyn LogSink>,
) -> paur_core::Result<BuildOutcome> {
    // Pre-create the expected on-disk layout. The container is bind-
    // mounted to /work and /ccache, both of which must exist.
    std::fs::create_dir_all(&req.work_dir)?;
    std::fs::create_dir_all(req.work_dir.join("out"))?;
    std::fs::create_dir_all(&req.ccache_dir)?;

    // Build the docker/podman command line. Keep it short and
    // explicit; do not introduce configuration knobs until a use case
    // demands them.
    let bin = req.runtime.binary();
    let mut cmd = Command::new(bin);
    cmd.args(["run", "--rm"])
        .arg("-v")
        .arg(format!("{}:/work", req.work_dir.display()))
        .arg("-v")
        .arg(format!("{}:/ccache", req.ccache_dir.display()))
        .arg("-e")
        .arg("CCACHE_DIR=/ccache")
        // `makepkg` writes to /etc/makepkg.conf which lives in the
        // image; we don't override it.
        .arg(&req.image)
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

    let status = child.wait().await.map_err(|e| {
        paur_core::Error::Build(format!("waiting on {bin} run: {e}"))
    })?;
    // Drain log tasks (they should already be EOF by now).
    let _ = out_task.await;
    let _ = err_task.await;

    let exit_code = status.code().unwrap_or(-1);
    tracing::info!(pkg = %req.pkg, exit_code, "container finished");

    let pkg_files = if exit_code == 0 {
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

/// Build a `BuildRequest` for a package, given the daemon's [`Config`].
/// Caller is responsible for the work_dir existing and being unique.
pub fn request_for(cfg: &Config, pkg: &PkgName, build_id: i64) -> BuildRequest {
    let aur_url = paur_aur::aur_url(pkg);
    let work_dir = cfg.work_dir.join(build_id.to_string());
    BuildRequest {
        pkg: pkg.clone(),
        aur_url,
        work_dir,
        ccache_dir: cfg.ccache_dir.clone(),
        runtime: cfg.container_runtime,
        image: cfg.builder_image.clone(),
    }
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
