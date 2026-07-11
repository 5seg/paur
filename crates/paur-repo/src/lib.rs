//! paur-repo: manage the published pacman repository.
//!
//! The flow:
//! 1. The build step finishes with `.pkg.tar.zst` artifacts in a
//!    per-build `out/` directory.
//! 2. `publish` copies those artifacts into the architecture-specific
//!    subdir of the repo, then asks the builder container to run
//!    `repo-add` on them (we can't run `repo-add` on the host
//!    because Debian/Ubuntu don't ship `pacman-contrib`).
//! 3. After `repo-add` updates `<name>.db.tar.gz`, the daemon
//!    produces detached GPG signatures for the DB and for each new
//!    `.pkg.tar.zst` on the host. The signing key never leaves the
//!    host's GNUPGHOME.
//!
//! The signing key is identified by keyid/fingerprint; the calling
//! code is responsible for the actual GPG key creation (typically at
//! `paur init` time). The keyid is stored in the DB `settings` table.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use paur_builder::{run_repo_op, RepoOp, RepoOpRequest};
use paur_core::{ContainerRuntime, PkgName, S3Config};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoCtx {
    /// Path to `<name>.db.tar.gz` (the `*` is implicit; pass without
    /// `.db.tar.gz`).
    pub repo_name: String,
    /// Architecture subdir (e.g. `x86_64`).
    pub arch: String,
    /// Repo root (containing `<arch>/<name>.db.tar.gz`).
    pub repo_dir: PathBuf,
    /// `GNUPGHOME` to use for signing.
    pub gpg_home: PathBuf,
    /// GPG key id / fingerprint to sign with.
    pub gpg_key: String,
    /// Container runtime used to run `repo-add`/`repo-remove`.
    pub container_runtime: ContainerRuntime,
    /// Builder image used to run `repo-add`/`repo-remove`.
    pub builder_image: String,
    /// Optional S3 configuration. When `Some`, every publish also
    /// uploads artifacts to S3. The local copy is kept unless
    /// `local_repo` is `false` (config-level flag, not in this ctx).
    #[serde(default)]
    pub s3: Option<S3Config>,
}

/// Abstracts the host-side work the repo crate needs from the
/// builder: staging .pkg.tar.* files and asking the container to
/// run `repo-add` / `repo-remove`. Keeping this behind a trait lets
/// `publish` / `remove` be unit-tested with a mock implementation.
#[async_trait]
pub trait RepoOps: Send + Sync {
    /// Stage `pkg_files` into a per-publish directory the container
    /// can read, then ask the container to `repo-add` them.
    async fn add(&self, ctx: &RepoCtx, db_name: &str, pkg_files: &[PathBuf]) -> paur_core::Result<()>;
    /// Ask the container to `repo-remove` the named package.
    async fn remove(&self, ctx: &RepoCtx, db_name: &str, pkgname: &str) -> paur_core::Result<()>;
}

/// Default implementation: shells out to the builder container.
pub struct ContainerRepoOps;

#[async_trait]
impl RepoOps for ContainerRepoOps {
    async fn add(&self, ctx: &RepoCtx, db_name: &str, pkg_files: &[PathBuf]) -> paur_core::Result<()> {
        let stage_dir = ctx.repo_dir.join(".stage");
        std::fs::create_dir_all(&stage_dir)?;
        let mut names = Vec::with_capacity(pkg_files.len());
        for src in pkg_files {
            let name = src.file_name().ok_or_else(|| {
                paur_core::Error::Repo(format!("artifact has no file name: {}", src.display()))
            })?;
            let dst = stage_dir.join(name);
            std::fs::copy(src, &dst).map_err(|e| {
                paur_core::Error::Repo(format!("stage {} -> {}: {e}", src.display(), dst.display()))
            })?;
            names.push(name.to_string_lossy().into_owned());
        }
        let req = RepoOpRequest {
            op: RepoOp::Add,
            repo_dir: ctx.arch_dir(),
            stage_dir,
            db_name: db_name.to_string(),
            names,
            runtime: ctx.container_runtime,
            image: ctx.builder_image.clone(),
        };
        run_repo_op(&req).await?;
        Ok(())
    }

    async fn remove(&self, ctx: &RepoCtx, db_name: &str, pkgname: &str) -> paur_core::Result<()> {
        let req = RepoOpRequest {
            op: RepoOp::Remove,
            repo_dir: ctx.arch_dir(),
            stage_dir: ctx.repo_dir.join(".stage"),
            db_name: db_name.to_string(),
            names: vec![pkgname.to_string()],
            runtime: ctx.container_runtime,
            image: ctx.builder_image.clone(),
        };
        run_repo_op(&req).await?;
        Ok(())
    }
}

impl RepoCtx {
    /// Architecture-specific directory of the repo.
    pub fn arch_dir(&self) -> PathBuf {
        self.repo_dir.join(&self.arch)
    }

    /// Path to the canonical database file (`<name>.db.tar.gz`).
    /// `repo-add` requires the full `.db.tar.gz` suffix on its first
    /// positional argument; this is the form we use for that tool.
    pub fn db_path_for_repo_add(&self) -> PathBuf {
        self.arch_dir()
            .join(format!("{}.db.tar.gz", self.repo_name))
    }

    /// Path to the database file we sign. The signed file is the
    /// canonical `.db.tar.gz` (the file `repo-add` produces).
    fn db_path(&self) -> PathBuf {
        self.db_path_for_repo_add()
    }

    /// Path to the database signature. We sign the .db.tar.gz file.
    fn db_sig(&self) -> PathBuf {
        let mut p = self.db_path_for_repo_add().into_os_string();
        p.push(".sig");
        PathBuf::from(p)
    }

    /// The `<name>.db.tar.gz` basename (used by the builder script).
    fn db_basename(&self) -> String {
        format!("{}.db.tar.gz", self.repo_name)
    }
}

/// Add `.pkg.tar.zst` files to the repo. Returns the new DB sig path
/// so callers can confirm signing succeeded.
pub async fn publish(ctx: &RepoCtx, pkg_files: &[PathBuf]) -> paur_core::Result<PathBuf> {
    publish_with(ctx, pkg_files, &ContainerRepoOps).await
}

/// Same as [`publish`] but lets the caller inject a [`RepoOps`].
/// Test-only in practice.
pub async fn publish_with(
    ctx: &RepoCtx,
    pkg_files: &[PathBuf],
    ops: &dyn RepoOps,
) -> paur_core::Result<PathBuf> {
    if pkg_files.is_empty() {
        return Err(paur_core::Error::Build("no .pkg.tar.zst to publish".into()));
    }
    std::fs::create_dir_all(ctx.arch_dir())?;

    // Copy artifacts into the arch dir (so pacman can fetch them
    // directly from Caddy). repo-add runs in the container; the
    // script reads the staged copies there.
    let mut staged: Vec<PathBuf> = Vec::new();
    for src in pkg_files {
        let name = src.file_name().ok_or_else(|| {
            paur_core::Error::Repo(format!("artifact has no file name: {}", src.display()))
        })?;
        let dst = ctx.arch_dir().join(name);
        std::fs::copy(src, &dst).map_err(|e| {
            paur_core::Error::Repo(format!("copy {} -> {}: {e}", src.display(), dst.display()))
        })?;
        staged.push(dst);
    }

    // Ask the container to update the DB.
    ops.add(ctx, &ctx.db_basename(), &staged).await?;

    // Re-sign the DB. The container's `repo-add` deletes the
    // existing .sig (because the DB content changes) so we always
    // produce a fresh one.
    let db_sig = ctx.db_sig();
    let _ = std::fs::remove_file(&db_sig);
    sign(&ctx.gpg_home, &ctx.gpg_key, &ctx.db_path()).await?;

    for pkg in &staged {
        let sig = {
            let mut s = pkg.as_os_str().to_owned();
            s.push(".sig");
            PathBuf::from(s)
        };
        let _ = std::fs::remove_file(&sig);
        sign(&ctx.gpg_home, &ctx.gpg_key, pkg).await?;
    }

    // Best-effort cleanup of the staging dir.
    if let Some(stage_dir) = staged.first().and_then(|p| p.parent().map(|_| ctx.repo_dir.join(".stage"))) {
        let _ = std::fs::remove_dir_all(&stage_dir);
    }

    // Optional S3 upload. The DB, every newly signed .pkg.tar.zst,
    // and every .sig are uploaded. Failures here are non-fatal:
    // the build succeeded and the local repo is consistent; we log
    // and keep going. S3 re-sync would be a separate maintenance
    // task. `paur-s3` already retries transient errors.
    if let Some(s3_cfg) = ctx.s3.clone() {
        let client = paur_s3::S3Client::new(s3_cfg);
        // 1) Upload the DB and its sig.
        let db_key = format!("{}/{}.db.tar.gz", ctx.arch, ctx.db_basename());
        let db_path = ctx.db_path();
        if let Err(e) = upload_path(&client, &db_key, "application/x-gzip", &db_path).await {
            tracing::warn!(error = %e, key = %db_key, "s3: db upload failed");
        }
        // 2) Upload each pkg + its sig.
        for pkg in &staged {
            let name = match pkg.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            let pkg_key = format!("{}/{}", ctx.arch, name);
            if let Err(e) = upload_path(&client, &pkg_key, "application/zstd", pkg).await {
                tracing::warn!(error = %e, key = %pkg_key, "s3: pkg upload failed");
            }
            let sig_key = format!("{}/{}.sig", ctx.arch, name);
            let sig_path = {
                let mut s = pkg.as_os_str().to_owned();
                s.push(".sig");
                PathBuf::from(s)
            };
            if sig_path.exists() {
                if let Err(e) =
                    upload_path(&client, &sig_key, "application/pgp-signature", &sig_path).await
                {
                    tracing::warn!(error = %e, key = %sig_key, "s3: sig upload failed");
                }
            }
        }
        // 3) Upload the paur.files tarball (clients fetch this too).
        let files_key = format!("{}/{}.files.tar.gz", ctx.arch, ctx.db_basename());
        let files_path = {
            let mut s = ctx.db_path().into_os_string();
            s.push(".files.tar.gz");
            let p = PathBuf::from(s);
            // The DB basename is e.g. "paur"; the files tarball is
            // "paur.files.tar.gz". Strip the duplicated ".db" first.
            // Easier: re-derive from repo_name.
            drop(p);
            ctx.repo_dir.join(&ctx.arch).join(format!("{}.files.tar.gz", ctx.repo_name))
        };
        if files_path.exists() {
            if let Err(e) =
                upload_path(&client, &files_key, "application/x-gzip", &files_path).await
            {
                tracing::warn!(error = %e, key = %files_key, "s3: files upload failed");
            }
        }
    }

    Ok(db_sig)
}

/// Read `path` and PUT it to S3 at `key`. Logs success at INFO.
async fn upload_path(
    client: &paur_s3::S3Client,
    key: &str,
    content_type: &str,
    path: &Path,
) -> paur_core::Result<String> {
    let bytes = tokio::fs::read(path).await.map_err(|e| {
        paur_core::Error::Repo(format!("read {} for s3: {e}", path.display()))
    })?;
    let url = client.put(key, content_type, bytes).await.map_err(|e| {
        paur_core::Error::Repo(format!("s3 put {key}: {e}"))
    })?;
    tracing::info!(key = %key, url = %url, "s3: uploaded");
    Ok(url)
}

/// Remove a package from the repo DB and sign the updated DB. The
/// `.pkg.tar.*` file itself is left in place; callers should unlink
/// it after this returns.
pub async fn remove(ctx: &RepoCtx, pkg: &PkgName) -> paur_core::Result<()> {
    remove_with(ctx, pkg, &ContainerRepoOps).await
}

/// Same as [`remove`] but with an injectable [`RepoOps`].
pub async fn remove_with(
    ctx: &RepoCtx,
    pkg: &PkgName,
    ops: &dyn RepoOps,
) -> paur_core::Result<()> {
    ops.remove(ctx, &ctx.db_basename(), pkg.as_str()).await?;

    // Re-sign the DB.
    let db_sig = ctx.db_sig();
    let _ = std::fs::remove_file(&db_sig);
    sign(&ctx.gpg_home, &ctx.gpg_key, &ctx.db_path()).await?;
    Ok(())
}

/// Export the public key in armored form to `out`.
pub async fn export_pubkey(
    gpg_home: &Path,
    keyid: &str,
    out: &Path,
) -> paur_core::Result<()> {
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut cmd = Command::new("gpg");
    cmd.env("GNUPGHOME", gpg_home)
        .args(["--armor", "--export", keyid])
        .stdin(Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let output = cmd
        .output()
        .await
        .map_err(|e| paur_core::Error::Gpg(format!("spawn gpg --export: {e}")))?;
    if !output.status.success() {
        return Err(paur_core::Error::Gpg(format!(
            "gpg --export failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    std::fs::write(out, &output.stdout)?;
    Ok(())
}

/// Generate a fresh signing key in `gpg_home`. Returns the long keyid
/// (fingerprint suffix). Uses `--batch --passphrase ''` so the call
/// does not block on a pinentry.
pub async fn generate_key(
    gpg_home: &Path,
    name: &str,
    email: &str,
) -> paur_core::Result<String> {
    std::fs::create_dir_all(gpg_home)?;
    let mut cmd = Command::new("gpg");
    cmd.env("GNUPGHOME", gpg_home)
        .args([
            "--batch",
            "--pinentry-mode",
            "loopback",
            "--passphrase",
            "",
            "--quick-generate-key",
            &format!("{name} <{email}>"),
            "ed25519",
            "sign",
            "0",
        ])
        .stdin(Stdio::null());
    let out = cmd
        .output()
        .await
        .map_err(|e| paur_core::Error::Gpg(format!("spawn gpg --quick-generate-key: {e}")))?;
    if !out.status.success() {
        return Err(paur_core::Error::Gpg(format!(
            "gpg --quick-generate-key failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    // Look up the freshly-created key's fingerprint.
    list_signing_key(gpg_home, email).await
}

/// Look up the fingerprint of the signing key matching `email`.
pub async fn list_signing_key(gpg_home: &Path, email: &str) -> paur_core::Result<String> {
    let mut cmd = Command::new("gpg");
    cmd.env("GNUPGHOME", gpg_home)
        .args(["--list-secret-keys", "--with-colons", email])
        .stdin(Stdio::null());
    let out = cmd
        .output()
        .await
        .map_err(|e| paur_core::Error::Gpg(format!("spawn gpg --list-secret-keys: {e}")))?;
    if !out.status.success() {
        return Err(paur_core::Error::Gpg(format!(
            "gpg --list-secret-keys failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    for line in stdout.lines() {
        if line.starts_with("fpr:") {
            let fpr = line.split(':').nth(9).unwrap_or("").trim();
            if !fpr.is_empty() {
                return Ok(fpr.to_string());
            }
        }
    }
    Err(paur_core::Error::Gpg(format!(
        "no fingerprint found for {email}"
    )))
}

/// Produce a detached GPG signature for `target` inside
/// `gpg_home`. The signature is written next to `target` with the
/// `.sig` extension (pacman convention). Removes any existing `.sig`
/// first.
///
/// Note: pacman expects the *binary* GPG signature format
/// (RFC 4880 raw signature packet), not the ASCII-armored
/// `-----BEGIN PGP SIGNATURE-----` block. We do *not* pass
/// `--armor` here, even though it makes the file human-readable.
async fn sign(
    gpg_home: &Path,
    keyid: &str,
    target: &Path,
) -> paur_core::Result<()> {
    let sig_path = {
        let mut s = target.as_os_str().to_owned();
        s.push(".sig");
        PathBuf::from(s)
    };
    let mut cmd = Command::new("gpg");
    cmd.env("GNUPGHOME", gpg_home)
        .args([
            "--batch",
            "--yes",
            "--pinentry-mode",
            "loopback",
            "--passphrase",
            "",
            "--local-user",
            keyid,
            "--output",
        ])
        .arg(&sig_path)
        .arg("--detach-sign")
        .arg(target)
        .stdin(Stdio::null());
    let out = cmd
        .output()
        .await
        .map_err(|e| paur_core::Error::Gpg(format!("spawn gpg --detach-sign: {e}")))?;
    if !out.status.success() {
        return Err(paur_core::Error::Gpg(format!(
            "gpg --detach-sign failed for {}: {}",
            target.display(),
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Test double that records calls instead of touching the host or
    /// a container. Lets the publish/remove code paths run end-to-end
    /// in a unit test.
    struct MockOps {
        adds: std::sync::Arc<tokio::sync::Mutex<Vec<Vec<String>>>>,
        removes: std::sync::Arc<tokio::sync::Mutex<Vec<String>>>,
    }

    impl MockOps {
        fn new() -> Self {
            Self {
                adds: std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new())),
                removes: std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new())),
            }
        }
    }

    #[async_trait]
    impl RepoOps for MockOps {
        async fn add(&self, _ctx: &RepoCtx, _db: &str, pkg_files: &[PathBuf]) -> paur_core::Result<()> {
            let names: Vec<String> = pkg_files
                .iter()
                .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(|s| s.to_string()))
                .collect();
            // Simulate repo-add's behavior of (re)writing the DB file.
            // Touch a stub .db.tar.gz so the post-publish assertions
            // find something on disk.
            if let Some(p) = pkg_files.first() {
                if let Some(dir) = p.parent() {
                    std::fs::write(dir.join("paur.db.tar.gz"), b"fake-db").unwrap();
                }
            }
            self.adds.lock().await.push(names);
            Ok(())
        }
        async fn remove(&self, _ctx: &RepoCtx, _db: &str, pkgname: &str) -> paur_core::Result<()> {
            self.removes.lock().await.push(pkgname.to_string());
            Ok(())
        }
    }

    fn have_gpg() -> bool {
        std::process::Command::new("which")
            .arg("gpg")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    #[tokio::test]
    async fn arch_dir_nests_under_repo_dir() {
        let ctx = RepoCtx {
            repo_name: "paur".into(),
            arch: "x86_64".into(),
            repo_dir: PathBuf::from("/var/lib/paur/repo"),
            gpg_home: PathBuf::from("/var/lib/paur/.gnupg"),
            gpg_key: "DEADBEEF".into(),
            container_runtime: ContainerRuntime::Docker,
            builder_image: "paur-builder:latest".into(),
            s3: None,
        };
        assert_eq!(ctx.arch_dir(), PathBuf::from("/var/lib/paur/repo/x86_64"));
        assert_eq!(ctx.db_basename(), "paur.db.tar.gz");
    }

    #[tokio::test]
    async fn publish_then_remove_roundtrip_via_mock() {
        if !have_gpg() {
            eprintln!("gpg missing; skipping");
            return;
        }
        let tmp = tempdir().unwrap();
        let repo_dir = tmp.path().join("repo");
        let gpg_home = tmp.path().join("gnupg");
        let stage = tmp.path().join("stage");
        std::fs::create_dir_all(&repo_dir).unwrap();
        std::fs::create_dir_all(&stage).unwrap();

        let fpr = generate_key(&gpg_home, "paur-test", "paur-test@example.invalid")
            .await
            .unwrap();
        assert!(fpr.len() >= 40);

        let ctx = RepoCtx {
            repo_name: "paur".into(),
            arch: "x86_64".into(),
            repo_dir: repo_dir.clone(),
            gpg_home: gpg_home.clone(),
            gpg_key: fpr.clone(),
            container_runtime: ContainerRuntime::Docker,
            builder_image: "paur-builder:latest".into(),
            s3: None,
        };

        // Fake a .pkg.tar.* that makepkg would have produced.
        let artifact = stage.join("foo-1.0-1-x86_64.pkg.tar.zst");
        std::fs::write(&artifact, b"fake").unwrap();

        let mock = MockOps::new();
        publish_with(&ctx, std::slice::from_ref(&artifact), &mock)
            .await
            .unwrap();
        // .pkg.tar.* copied into the arch dir
        assert!(ctx.arch_dir().join(artifact.file_name().unwrap()).exists());
        // .db.tar.gz written and signed
        assert!(ctx.arch_dir().join("paur.db.tar.gz").exists());
        assert!(ctx.db_sig().exists());
        // Mock recorded the add
        assert_eq!(mock.adds.lock().await.len(), 1);

        // Now remove.
        let pkg = paur_core::PkgName::new("foo").unwrap();
        remove_with(&ctx, &pkg, &mock).await.unwrap();
        let removed = mock.removes.lock().await.clone();
        assert_eq!(removed, vec!["foo".to_string()]);
        assert!(ctx.db_sig().exists());
    }
}
