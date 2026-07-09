//! paur-aur: talk to the Arch User Repository.
//!
//! Three operations, kept narrow and async:
//! - [`clone`] a package git repo into a working dir
//! - [`latest_ref`] for polling (HEAD commit hash without cloning)
//! - [`parse_srcinfo`] for inspecting the package metadata
//!
//! We shell out to `git` rather than linking libgit2 — paur runs on
//! Arch, the binary is always present, and shelling avoids a heavy
//! native dep.

pub mod srcinfo;

use std::path::Path;
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use tokio::process::Command;

use paur_core::PkgName;
pub use srcinfo::SrcInfo;

/// Canonical AUR base URL.
pub const AUR_BASE: &str = "https://aur.archlinux.org";

/// Build the canonical AUR git URL for a package.
pub fn aur_url(pkg: &PkgName) -> String {
    format!("{}/{}.git", AUR_BASE, pkg)
}

/// AUR clone options.
#[derive(Debug, Clone)]
pub struct CloneOpts {
    /// Whether to allow git output on stderr (e.g. progress). Default false.
    pub quiet: bool,
}

impl Default for CloneOpts {
    fn default() -> Self {
        Self { quiet: true }
    }
}

/// Result of a successful clone: the HEAD commit hash, and the path the
/// repo was cloned into.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloneResult {
    /// Commit hash at HEAD after clone.
    pub head_ref: String,
    /// Path the repo was cloned into.
    pub path: std::path::PathBuf,
}

/// Clone `pkg` from AUR into `into`. The `into` directory must not
/// already exist (git's `clone <url> <dir>` semantics).
pub async fn clone(pkg: &PkgName, into: &Path, opts: CloneOpts) -> paur_core::Result<CloneResult> {
    let url = aur_url(pkg);
    let mut cmd = Command::new("git");
    cmd.args(["clone", &url, into.to_str().ok_or_else(|| {
        paur_core::Error::Aur(format!("non-utf8 path: {}", into.display()))
    })?]);
    if opts.quiet {
        cmd.args(["-q"]);
    }
    cmd.stdin(Stdio::null());
    let status = cmd
        .status()
        .await
        .map_err(|e| paur_core::Error::Aur(format!("git clone: {e}")))?;
    if !status.success() {
        return Err(paur_core::Error::Aur(format!(
            "git clone of {url} failed with status {status}"
        )));
    }
    let head_ref = head_commit(into).await?;
    Ok(CloneResult {
        head_ref,
        path: into.to_path_buf(),
    })
}

/// Return the current HEAD commit hash of the git repo at `repo`.
pub async fn head_commit(repo: &Path) -> paur_core::Result<String> {
    let out = Command::new("git")
        .args(["-C"])
        .arg(repo)
        .args(["rev-parse", "HEAD"])
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|e| paur_core::Error::Aur(format!("git rev-parse: {e}")))?;
    if !out.status.success() {
        return Err(paur_core::Error::Aur(format!(
            "git rev-parse HEAD failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Return the latest commit hash on the default branch of the AUR repo
/// for `pkg`, without cloning. Uses `git ls-remote`. Returns the *first*
/// hash in the response, which is HEAD.
pub async fn latest_ref(pkg: &PkgName) -> paur_core::Result<String> {
    let url = aur_url(pkg);
    let out = Command::new("git")
        .args(["ls-remote", &url, "HEAD"])
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|e| paur_core::Error::Aur(format!("git ls-remote: {e}")))?;
    if !out.status.success() {
        return Err(paur_core::Error::Aur(format!(
            "git ls-remote {url} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    let s = String::from_utf8_lossy(&out.stdout);
    // Format: "<hash>\tHEAD\n"
    let hash = s
        .split_whitespace()
        .next()
        .ok_or_else(|| paur_core::Error::Aur(format!("empty ls-remote response: {s:?}")))?;
    if hash.len() != 40 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(paur_core::Error::Aur(format!(
            "ls-remote returned non-hash: {hash:?}"
        )));
    }
    Ok(hash.to_string())
}

/// Pull the latest changes into an existing local clone. Caller is
/// responsible for ensuring `repo` is a git repo of `pkg`.
pub async fn pull(repo: &Path) -> paur_core::Result<String> {
    let out = Command::new("git")
        .args(["-C"])
        .arg(repo)
        .args(["pull", "-q", "--ff-only"])
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|e| paur_core::Error::Aur(format!("git pull: {e}")))?;
    if !out.status.success() {
        return Err(paur_core::Error::Aur(format!(
            "git pull failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    head_commit(repo).await
}

/// Generate `.SRCINFO` from a checked-out repo. `makepkg` is the
/// authoritative tool for this — we delegate.
pub async fn generate_srcinfo(repo: &Path) -> paur_core::Result<String> {
    let out = Command::new("makepkg")
        .args(["--printsrcinfo"])
        .current_dir(repo)
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|e| paur_core::Error::Aur(format!("makepkg --printsrcinfo: {e}")))?;
    if !out.status.success() {
        return Err(paur_core::Error::Aur(format!(
            "makepkg --printsrcinfo failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Skip the network-touching tests when offline. We can detect by
    /// attempting a cheap `git ls-remote` and bailing on failure.
    async fn aur_reachable() -> bool {
        let n = PkgName::new("paru-bin").unwrap();
        latest_ref(&n).await.is_ok()
    }

    #[tokio::test]
    async fn url_format() {
        let n = PkgName::new("paru-bin").unwrap();
        assert_eq!(aur_url(&n), "https://aur.archlinux.org/paru-bin.git");
    }

    #[tokio::test]
    async fn latest_ref_returns_40_char_hash() {
        if !aur_reachable().await {
            eprintln!("AUR unreachable; skipping");
            return;
        }
        let n = PkgName::new("paru-bin").unwrap();
        let h = latest_ref(&n).await.unwrap();
        assert_eq!(h.len(), 40);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[tokio::test]
    async fn clone_then_head() {
        if !aur_reachable().await {
            eprintln!("AUR unreachable; skipping");
            return;
        }
        let dir = tempdir().unwrap();
        let n = PkgName::new("paru-bin").unwrap();
        let res = clone(&n, &dir.path().join("src"), CloneOpts::default())
            .await
            .unwrap();
        assert_eq!(res.head_ref.len(), 40);
    }
}
