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
use paur_core::{ContainerRuntime, PkgName, S3Config, Variant};
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
    /// can read, then ask the container to `repo-add` them into the
    /// DB for the given variant.
    async fn add(
        &self,
        ctx: &RepoCtx,
        variant: Variant,
        pkg_files: &[PathBuf],
    ) -> paur_core::Result<()>;
    /// Ask the container to `repo-remove` the named package from
    /// the given variant's DB.
    async fn remove(
        &self,
        ctx: &RepoCtx,
        variant: Variant,
        pkgname: &str,
    ) -> paur_core::Result<()>;
}

/// Default implementation: shells out to the builder container.
pub struct ContainerRepoOps;

#[async_trait]
impl RepoOps for ContainerRepoOps {
    async fn add(
        &self,
        ctx: &RepoCtx,
        variant: Variant,
        pkg_files: &[PathBuf],
    ) -> paur_core::Result<()> {
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
        let arch_dir = ctx.arch_subdir(variant);
        let req = RepoOpRequest {
            op: RepoOp::Add,
            arch_dir,
            stage_dir,
            db_name: ctx.db_basename_for(variant),
            names,
            runtime: ctx.container_runtime,
            image: ctx.builder_image.clone(),
        };
        run_repo_op(&req).await?;
        Ok(())
    }

    async fn remove(
        &self,
        ctx: &RepoCtx,
        variant: Variant,
        pkgname: &str,
    ) -> paur_core::Result<()> {
        let req = RepoOpRequest {
            op: RepoOp::Remove,
            arch_dir: ctx.arch_subdir(variant),
            stage_dir: ctx.repo_dir.join(".stage"),
            db_name: ctx.db_basename_for(variant),
            names: vec![pkgname.to_string()],
            runtime: ctx.container_runtime,
            image: ctx.builder_image.clone(),
        };
        run_repo_op(&req).await?;
        Ok(())
    }
}

impl RepoCtx {
    /// Architecture-specific directory for the default variant,
    /// e.g. `<repo_dir>/x86_64`. Kept for callers that don't care
    /// about variants (e.g. legacy code paths and tests).
    pub fn arch_dir(&self) -> PathBuf {
        self.repo_dir.join(&self.arch)
    }

    /// Architecture-specific directory for a given variant, e.g.
    /// `<repo_dir>/x86_64` (default), `<repo_dir>/x86_64-v3`,
    /// `<repo_dir>/x86_64-v4`. This is where `.pkg.tar.*` files
    /// land and where `repo-add` writes the DB.
    pub fn arch_subdir(&self, variant: Variant) -> PathBuf {
        let name = match variant {
            Variant::Default => self.arch.clone(),
            Variant::V3 => format!("{}-v3", self.arch),
            Variant::V4 => format!("{}-v4", self.arch),
        };
        self.repo_dir.join(name)
    }

    /// Repo DB basename for a variant, e.g. `paur` / `paur-v3` /
    /// `paur-v4`. `repo-add` requires the full `.db.tar.gz` suffix
    /// on its first positional argument; this is the form we use
    /// for that tool.
    pub fn db_basename_for(&self, variant: Variant) -> String {
        match variant {
            Variant::Default => format!("{}.db.tar.gz", self.repo_name),
            Variant::V3 => format!("{}-v3.db.tar.gz", self.repo_name),
            Variant::V4 => format!("{}-v4.db.tar.gz", self.repo_name),
        }
    }

    /// Path to the database file for a given variant.
    pub fn db_path_for(&self, variant: Variant) -> PathBuf {
        self.arch_subdir(variant).join(self.db_basename_for(variant))
    }

    /// Path to the database signature for a given variant.
    pub fn db_sig_for(&self, variant: Variant) -> PathBuf {
        let mut p = self.db_path_for(variant).into_os_string();
        p.push(".sig");
        PathBuf::from(p)
    }
}

/// Add `.pkg.tar.zst` files to the repo (specific variant).
/// Returns the new DB sig path so callers can confirm signing
/// succeeded.
pub async fn publish(
    ctx: &RepoCtx,
    pkg_files: &[PathBuf],
    variant: Variant,
) -> paur_core::Result<PathBuf> {
    publish_with(ctx, pkg_files, variant, &ContainerRepoOps).await
}

/// Same as [`publish`] but lets the caller inject a [`RepoOps`].
/// Test-only in practice.
pub async fn publish_with(
    ctx: &RepoCtx,
    pkg_files: &[PathBuf],
    variant: Variant,
    ops: &dyn RepoOps,
) -> paur_core::Result<PathBuf> {
    if pkg_files.is_empty() {
        return Err(paur_core::Error::Build("no .pkg.tar.zst to publish".into()));
    }
    let arch_dir = ctx.arch_subdir(variant);
    std::fs::create_dir_all(&arch_dir)?;

    // Copy artifacts into the arch subdir (so pacman can fetch them
    // directly from Caddy). repo-add runs in the container; the
    // script reads the staged copies there.
    let mut staged: Vec<PathBuf> = Vec::new();
    for src in pkg_files {
        let name = src.file_name().ok_or_else(|| {
            paur_core::Error::Repo(format!("artifact has no file name: {}", src.display()))
        })?;
        let dst = arch_dir.join(name);
        std::fs::copy(src, &dst).map_err(|e| {
            paur_core::Error::Repo(format!("copy {} -> {}: {e}", src.display(), dst.display()))
        })?;
        staged.push(dst);
    }

    // Ask the container to update the DB.
    ops.add(ctx, variant, &staged).await?;

    // Re-sign the DB. The container's `repo-add` deletes the
    // existing .sig (because the DB content changes) so we always
    // produce a fresh one.
    let db_path = ctx.db_path_for(variant);
    let db_sig = ctx.db_sig_for(variant);
    let _ = std::fs::remove_file(&db_sig);
    sign(&ctx.gpg_home, &ctx.gpg_key, &db_path).await?;

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
        let db_key = format!("{}/{}", ctx.arch_subdir(variant).file_name().and_then(|n| n.to_str()).unwrap_or(&ctx.arch), ctx.db_basename_for(variant));
        if let Err(e) = upload_path(&client, &db_key, "application/x-gzip", &db_path).await {
            tracing::warn!(error = %e, key = %db_key, "s3: db upload failed");
        }
        // 2) Upload each pkg + its sig.
        for pkg in &staged {
            let name = match pkg.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            let subdir = ctx.arch_subdir(variant).file_name().and_then(|n| n.to_str()).unwrap_or(&ctx.arch).to_string();
            let pkg_key = format!("{}/{}", subdir, name);
            if let Err(e) = upload_path(&client, &pkg_key, "application/zstd", pkg).await {
                tracing::warn!(error = %e, key = %pkg_key, "s3: pkg upload failed");
            }
            let sig_key = format!("{}/{}.sig", subdir, name);
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
        // 3) Upload the .files tarball (clients fetch this too).
        let subdir = ctx.arch_subdir(variant).file_name().and_then(|n| n.to_str()).unwrap_or(&ctx.arch).to_string();
        let files_basename = format!("{}.files.tar.gz", match variant {
            Variant::Default => ctx.repo_name.clone(),
            Variant::V3 => format!("{}-v3", ctx.repo_name),
            Variant::V4 => format!("{}-v4", ctx.repo_name),
        });
        let files_path = arch_dir.join(&files_basename);
        if files_path.exists() {
            let files_key = format!("{}/{}", subdir, files_basename);
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
pub async fn remove(
    ctx: &RepoCtx,
    pkg: &PkgName,
    variant: Variant,
) -> paur_core::Result<()> {
    remove_with(ctx, pkg, variant, &ContainerRepoOps).await
}

/// Same as [`remove`] but with an injectable [`RepoOps`].
pub async fn remove_with(
    ctx: &RepoCtx,
    pkg: &PkgName,
    variant: Variant,
    ops: &dyn RepoOps,
) -> paur_core::Result<()> {
    ops.remove(ctx, variant, pkg.as_str()).await?;

    // Re-sign the DB.
    let db_sig = ctx.db_sig_for(variant);
    let _ = std::fs::remove_file(&db_sig);
    sign(&ctx.gpg_home, &ctx.gpg_key, &ctx.db_path_for(variant)).await?;
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

/// Extract the primary fingerprint from an ASCII-armored PGP
/// public key blob. Used by the daemon's `/api/v1/install/fpr`
/// helper to feed the Install page without bundling a PGP parser
/// into the UI.
///
/// We pipe the key into `gpg --show-keys --with-colons` so we
/// don't have to write a parser here — `gpg` is the source of
/// truth for the wire format anyway. The first `fpr:` line is
/// the primary key's fingerprint; subkeys would appear on later
/// lines.
pub async fn primary_fpr(pubkey_bytes: &[u8]) -> paur_core::Result<String> {
    let mut cmd = Command::new("gpg");
    cmd.args(["--show-keys", "--with-colons"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| {
        paur_core::Error::Gpg(format!("spawn gpg --show-keys: {e}"))
    })?;
    use tokio::io::AsyncWriteExt as _;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(pubkey_bytes).await.map_err(|e| {
            paur_core::Error::Gpg(format!("gpg stdin: {e}"))
        })?;
    }
    let out = child.wait_with_output().await.map_err(|e| {
        paur_core::Error::Gpg(format!("gpg --show-keys: {e}"))
    })?;
    if !out.status.success() {
        return Err(paur_core::Error::Gpg(format!(
            "gpg --show-keys failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    for line in stdout.lines() {
        // `--with-colons` writes `fpr::::::::::<HEX>:`; field 9
        // (0-indexed) is the FPR. We pick the first one, which
        // is the primary key; subkeys come after.
        if line.starts_with("fpr:") {
            let fpr = line.split(':').nth(9).unwrap_or("").trim();
            if !fpr.is_empty() {
                return Ok(fpr.to_string());
            }
        }
    }
    Err(paur_core::Error::Gpg(
        "no primary fingerprint in pubkey".into(),
    ))
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
        adds: std::sync::Arc<tokio::sync::Mutex<Vec<(Variant, Vec<String>)>>>,
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
        async fn add(
            &self,
            _ctx: &RepoCtx,
            variant: Variant,
            pkg_files: &[PathBuf],
        ) -> paur_core::Result<()> {
            let names: Vec<String> = pkg_files
                .iter()
                .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(|s| s.to_string()))
                .collect();
            // Simulate repo-add's behavior of (re)writing the DB file.
            // Touch a stub .db.tar.gz with the *variant's* basename
            // so the post-publish assertions find something on disk.
            if let Some(p) = pkg_files.first() {
                if let Some(dir) = p.parent() {
                    let db_name = match variant {
                        Variant::Default => "paur.db.tar.gz",
                        Variant::V3 => "paur-v3.db.tar.gz",
                        Variant::V4 => "paur-v4.db.tar.gz",
                    };
                    std::fs::write(dir.join(db_name), b"fake-db").unwrap();
                }
            }
            self.adds.lock().await.push((variant, names));
            Ok(())
        }
        async fn remove(
            &self,
            _ctx: &RepoCtx,
            _variant: Variant,
            pkgname: &str,
        ) -> paur_core::Result<()> {
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
        assert_eq!(ctx.db_basename_for(Variant::Default), "paur.db.tar.gz");
        assert_eq!(ctx.arch_subdir(Variant::Default), PathBuf::from("/var/lib/paur/repo/x86_64"));
        assert_eq!(ctx.arch_subdir(Variant::V3), PathBuf::from("/var/lib/paur/repo/x86_64-v3"));
        assert_eq!(ctx.arch_subdir(Variant::V4), PathBuf::from("/var/lib/paur/repo/x86_64-v4"));
        assert_eq!(ctx.db_basename_for(Variant::V3), "paur-v3.db.tar.gz");
        assert_eq!(ctx.db_basename_for(Variant::V4), "paur-v4.db.tar.gz");
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
        publish_with(&ctx, std::slice::from_ref(&artifact), Variant::V3, &mock)
            .await
            .unwrap();
        // .pkg.tar.* copied into the v3 arch dir
        let v3_dir = ctx.arch_subdir(Variant::V3);
        assert!(v3_dir.join(artifact.file_name().unwrap()).exists());
        // .db.tar.gz written and signed in the v3 arch dir
        assert!(v3_dir.join("paur-v3.db.tar.gz").exists());
        assert!(ctx.db_sig_for(Variant::V3).exists());
        // Mock recorded the add with the variant tag
        let recorded = mock.adds.lock().await.clone();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].0, Variant::V3);

        // Now remove.
        let pkg = paur_core::PkgName::new("foo").unwrap();
        remove_with(&ctx, &pkg, Variant::V3, &mock).await.unwrap();
        let removed = mock.removes.lock().await.clone();
        assert_eq!(removed, vec!["foo".to_string()]);
        assert!(ctx.db_sig_for(Variant::V3).exists());
    }
}
