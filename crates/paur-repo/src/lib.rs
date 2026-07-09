//! paur-repo: manage the published pacman repository.
//!
//! The flow:
//! 1. The build step finishes with `.pkg.tar.zst` artifacts in a
//!    per-build `out/` directory.
//! 2. `publish` copies those artifacts into the architecture-specific
//!    subdir of the repo, then runs `repo-add` to register them with
//!    the repo's `<name>.db.tar.gz` (and `<name>.files.tar.gz`).
//! 3. After the DB is updated, the crate produces detached GPG
//!    signatures for the DB and for each new `.pkg.tar.zst`.
//!
//! The signing key is identified by keyid/fingerprint; the calling
//! code is responsible for the actual GPG key creation (typically at
//! `paur init` time). The keyid is stored in the DB `settings` table.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use tokio::process::Command;

use paur_core::PkgName;

/// Where to find the GPG keyring, which key to use, and which repo
/// database files to manage. Cheap to clone.
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
}

/// Add `.pkg.tar.zst` files to the repo. Returns the new DB sig path
/// so callers can confirm signing succeeded.
pub async fn publish(ctx: &RepoCtx, pkg_files: &[PathBuf]) -> paur_core::Result<PathBuf> {
    if pkg_files.is_empty() {
        return Err(paur_core::Error::Build("no .pkg.tar.zst to publish".into()));
    }
    std::fs::create_dir_all(ctx.arch_dir())?;

    // Copy artifacts to the arch dir.
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

    // Run repo-add. We pass `<name>.db.tar.gz`; the tool will create
    // `<name>.db` (and `<name>.files`) alongside it.
    let db_arg = ctx
        .db_path_for_repo_add()
        .to_str()
        .ok_or_else(|| paur_core::Error::Repo("non-utf8 db path".into()))?
        .to_string();
    let mut cmd = Command::new("repo-add");
    cmd.arg(&db_arg);
    for s in &staged {
        cmd.arg(s);
    }
    cmd.stdin(Stdio::null());
    let out = cmd
        .output()
        .await
        .map_err(|e| paur_core::Error::Repo(format!("spawn repo-add: {e}")))?;
    if !out.status.success() {
        return Err(paur_core::Error::Repo(format!(
            "repo-add failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }

    // Sign the DB and each new package. Old .sig files become stale
    // (the file contents changed) so we delete them first; the .sig
    // will be regenerated below.
    let db_sig = ctx.db_sig();
    let _ = std::fs::remove_file(&db_sig);
    sign(&ctx.gpg_home, &ctx.gpg_key, &ctx.db_path()).await?;

    for pkg in &staged {
        // Each package has its own .sig alongside it.
        let sig = {
            let mut s = pkg.as_os_str().to_owned();
            s.push(".sig");
            PathBuf::from(s)
        };
        let _ = std::fs::remove_file(&sig);
        sign(&ctx.gpg_home, &ctx.gpg_key, pkg).await?;
    }

    Ok(db_sig)
}

/// Remove a package from the repo DB and sign the updated DB.
pub async fn remove(ctx: &RepoCtx, pkg: &PkgName) -> paur_core::Result<()> {
    let db_arg = ctx
        .db_path_for_repo_add()
        .to_str()
        .ok_or_else(|| paur_core::Error::Repo("non-utf8 db path".into()))?
        .to_string();
    let mut cmd = Command::new("repo-remove");
    cmd.arg(&db_arg).arg(pkg.as_str());
    cmd.stdin(Stdio::null());
    let out = cmd
        .output()
        .await
        .map_err(|e| paur_core::Error::Repo(format!("spawn repo-remove: {e}")))?;
    if !out.status.success() {
        return Err(paur_core::Error::Repo(format!(
            "repo-remove failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
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
        .stderr(Stdio::piped());
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
    // The "fpr" line carries the long fingerprint; the first such
    // record belongs to the primary key.
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

/// Produce a detached armored GPG signature for `target` inside
/// `gpg_home`. The signature is written next to `target` with the
/// `.sig` extension (pacman convention). Removes any existing `.sig`
/// first.
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
            "--armor",
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

    /// Build a minimal real arch package using `makepkg`. The result
    /// is accepted by `repo-add`/`repo-remove`; the synthetic
    /// archives we build in unit tests aren't.
    async fn make_real_pkg(work: &Path) -> PathBuf {
        let pkgbuild = work.join("PKGBUILD");
        std::fs::write(
            &pkgbuild,
            b"pkgname=paur-fixture\n\
              pkgver=1.0\n\
              pkgrel=1\n\
              arch=('x86_64')\n\
              package() { install -d \"$pkgdir/usr\"; \
                          echo hello > \"$pkgdir/usr/hello\"; }\n",
        )
        .unwrap();
        let status = std::process::Command::new("makepkg")
            .args(["-sf", "--noconfirm"])
            .current_dir(work)
            .status()
            .unwrap();
        assert!(status.success(), "makepkg failed in fixture build");
        let pkg = std::fs::read_dir(work)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with("paur-fixture-") && n.ends_with(".pkg.tar.zst"))
                    .unwrap_or(false)
            })
            .expect("no .pkg.tar.zst produced");
        pkg
    }

    /// Make a minimal valid `.pkg.tar.zst` for testing. The contents
    /// aren't a real arch package; we craft the *shape* (compressed
    /// tar with the right per-package metadata) just well enough that
    /// `repo-add` accepts the file.
    fn make_fake_pkg(path: &Path) {
        let tmp = tempfile::tempdir().unwrap();
        let staging = tmp.path().join("staging");
        std::fs::create_dir_all(&staging).unwrap();
        // repo-add inspects .PKGINFO; missing or malformed fields
        // (pkgver, pkgrel, arch) cause it to reject the package.
        std::fs::write(
            staging.join(".PKGINFO"),
            b"pkgname = foo\npkgver = 1.0\npkgrel = 1\narch = x86_64\n",
        )
        .unwrap();
        std::fs::write(
            staging.join(".MTREE"),
            b"fake mtree\n",
        )
        .unwrap();
        std::fs::create_dir_all(staging.join("usr")).unwrap();
        std::fs::write(staging.join("usr").join("hello"), b"hi\n").unwrap();

        let tar_path = tmp.path().join("pkg.tar");
        let status = std::process::Command::new("bsdtar")
            .args([
                "--format=ustar",
                "-C",
                staging.to_str().unwrap(),
                "-cf",
                tar_path.to_str().unwrap(),
                ".PKGINFO",
                ".MTREE",
                "usr",
            ])
            .status()
            .unwrap();
        assert!(status.success(), "bsdtar failed");

        let status = std::process::Command::new("zstd")
            .args([
                "-q",
                "-f",
                "-o",
                path.to_str().unwrap(),
                tar_path.to_str().unwrap(),
            ])
            .status()
            .unwrap();
        assert!(status.success(), "zstd failed");
    }

    /// `repo-add`, `gpg`, and `makepkg` are system tools. Skip their
    /// tests when unavailable so unit tests still pass on minimal CI
    /// images.
    fn have_repo_add() -> bool {
        which("repo-add")
    }
    fn have_gpg() -> bool {
        which("gpg")
    }
    fn have_makepkg() -> bool {
        which("makepkg")
    }
    fn have_bsdtar() -> bool {
        which("bsdtar")
    }
    fn have_zstd() -> bool {
        which("zstd")
    }
    fn which(bin: &str) -> bool {
        std::process::Command::new("which")
            .arg(bin)
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
        };
        assert_eq!(ctx.arch_dir(), PathBuf::from("/var/lib/paur/repo/x86_64"));
    }

    #[tokio::test]
    async fn publish_then_remove_roundtrip() {
        if !(have_repo_add() && have_gpg() && have_makepkg()) {
            eprintln!("repo-add/gpg/makepkg missing; skipping");
            return;
        }
        let tmp = tempdir().unwrap();
        let repo_dir = tmp.path().join("repo");
        let gpg_home = tmp.path().join("gnupg");
        let pkg_work = tmp.path().join("pkgbuild");
        std::fs::create_dir_all(&repo_dir).unwrap();
        std::fs::create_dir_all(&pkg_work).unwrap();

        // Generate a throwaway signing key.
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
        };

        // Build a real arch package with makepkg.
        let artifact = make_real_pkg(&pkg_work).await;

        publish(&ctx, &[artifact.clone()]).await.unwrap();
        assert!(ctx.arch_dir().join("paur.db.tar.gz").exists());
        assert!(ctx.db_sig().exists());
        assert!(ctx
            .arch_dir()
            .join(artifact.file_name().unwrap())
            .exists());

        // Remove the package; use the canonical name from .PKGINFO.
        let canonical = discover_pkgname(&ctx).await.unwrap();
        remove(&ctx, &canonical).await.unwrap();
        assert!(ctx.db_sig().exists());
    }

    /// `repo-add` records the package under whatever pkgname appears
    /// in the archive's `.PKGINFO`. Our test sets that to `foo`, but
    /// in practice the `bsdtar` produced for the roundtrip also gets
    /// re-archived. Read the `.PKGINFO` from the on-disk artifact to
    /// be sure of the name before removing.
    async fn discover_pkgname(
        ctx: &RepoCtx,
    ) -> Result<paur_core::PkgName, paur_core::Error> {
        use std::process::Stdio;
        for entry in std::fs::read_dir(ctx.arch_dir())
            .map_err(|e| paur_core::Error::Repo(e.to_string()))?
        {
            let entry = entry.map_err(|e| paur_core::Error::Repo(e.to_string()))?;
            let p = entry.path();
            let name = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();
            if name.ends_with(".pkg.tar.zst") {
                let out = std::process::Command::new("bsdtar")
                    .args(["xOf"])
                    .arg(&p)
                    .arg(".PKGINFO")
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .output()
                    .unwrap();
                let s = String::from_utf8_lossy(&out.stdout);
                for line in s.lines() {
                    if let Some(rest) = line.strip_prefix("pkgname =") {
                        let n = rest.trim();
                        return paur_core::PkgName::new(n);
                    }
                }
            }
        }
        Err(paur_core::Error::Repo("no pkgname found".into()))
    }
}
