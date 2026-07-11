//! Runtime configuration for paur. Loaded from TOML and overridable via
//! environment variables. The config struct intentionally keeps every
//! filesystem path explicit so that the operator can move the runtime
//! data root (e.g. onto a separate disk) without code changes.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Top-level configuration for the paur daemon. Mirrors `paur.toml` on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Where paur stores all mutable runtime data (db, work dirs, ccache,
    /// gpg keyring, logs). Default: `/var/lib/paur`.
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,

    /// Where the published pacman repo lives. The daemon writes `.pkg.tar.zst`
    /// files and the `paur.db.tar.gz` here, and the HTTPS static server
    /// reads from this path. Default: `<data_dir>/repo`.
    #[serde(default = "default_repo_dir")]
    pub repo_dir: PathBuf,

    /// Where individual build work directories are created.
    /// Default: `<data_dir>/work`.
    #[serde(default = "default_work_dir")]
    pub work_dir: PathBuf,

    /// Where build logs are stored as text files (in addition to the DB).
    /// Default: `<data_dir>/logs`.
    #[serde(default = "default_logs_dir")]
    pub logs_dir: PathBuf,

    /// ccache directory; bind-mounted into the build container.
    /// Default: `<data_dir>/ccache`.
    #[serde(default = "default_ccache_dir")]
    pub ccache_dir: PathBuf,

    /// GPG home directory used for signing the repo. Default: `<data_dir>/.gnupg`.
    #[serde(default = "default_gpg_home")]
    pub gpg_home: PathBuf,

    /// Name of the pacman repo as exposed to clients (e.g. `paur` -> `[paur]`
    /// in pacman.conf). Default: `paur`.
    #[serde(default = "default_repo_name")]
    pub repo_name: String,

    /// Architecture subdirectory of the repo. Default: `x86_64`.
    #[serde(default = "default_arch")]
    pub arch: String,

    /// Container runtime used to spawn build containers.
    #[serde(default)]
    pub container_runtime: ContainerRuntime,

    /// Docker/Podman image used for builds. Default: `paur-builder:latest`.
    #[serde(default = "default_builder_image")]
    pub builder_image: String,

    /// Number of concurrent build workers. Default: 1 (recommended for a
    /// single host; raising this requires serializing repo-add).
    #[serde(default = "default_max_workers")]
    pub max_workers: u32,

    /// Interval in seconds for the AUR poller (when auto_rebuild is on).
    /// Default: 600.
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,

    /// Where the daemon listens. Default: a unix socket at
    /// `/run/paur/paur.sock` (set up by systemd's `RuntimeDirectory=`).
    #[serde(default = "default_listen")]
    pub listen: Listen,

    /// Public base URL the repo is served at. Used in logs and the
    /// "Add to pacman.conf" hint.
    #[serde(default = "default_public_base_url")]
    pub public_base_url: String,

    /// Optional S3-compatible backend for publishing artifacts. When
    /// `Some`, `paur-repo` uploads each `.pkg.tar.zst`, the signed
    /// `paur.db.tar.gz`, and `.sig` files to S3 in addition to (or
    /// instead of, see `local_repo`) keeping the local copy. Caddy
    /// (or CloudFront/R2 public URL) serves the objects directly.
    #[serde(default)]
    pub s3: Option<S3Config>,

    /// When `true` (default), keep the local copy in `repo_dir` even
    /// when S3 is configured. Set to `false` to publish to S3 only
    /// and skip the local `repo_dir` writes (saves disk).
    #[serde(default = "default_true")]
    pub local_repo: bool,
}

/// S3-compatible object storage configuration. Any provider that
/// implements the S3 API works (AWS S3, Cloudflare R2, Backblaze B2,
/// MinIO, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S3Config {
    /// Bucket name (e.g. "paur-repo").
    pub bucket: String,
    /// S3 endpoint URL. For R2 this is
    /// `https://<accountid>.r2.cloudflarestorage.com`. For AWS S3
    /// leave empty to use the regional default.
    #[serde(default)]
    pub endpoint: Option<String>,
    /// Region (e.g. "auto" for R2, "us-east-1" for AWS).
    pub region: String,
    /// Optional key prefix inside the bucket (e.g. "paur/").
    #[serde(default)]
    pub prefix: Option<String>,
    /// Access key id. Use a per-bucket scoped key where possible.
    pub access_key: String,
    /// Secret access key. Treat like a password.
    pub secret_key: String,
    /// Force path-style addressing. Required for MinIO and some R2
    /// setups; AWS S3 uses virtual-hosted by default.
    #[serde(default)]
    pub path_style: bool,
    /// Optional public URL prefix clients use to fetch objects. When
    /// set, the daemon logs a hint to set `Server = <public_url>` in
    /// pacman.conf. Example: `https://pub-xxx.r2.dev`.
    #[serde(default)]
    pub public_url: Option<String>,
}

fn default_true() -> bool {
    true
}

/// Where the daemon exposes its HTTP API.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Listen {
    /// Unix domain socket path.
    Unix(PathBuf),
    /// TCP socket address (e.g. `0.0.0.0:7300`).
    Tcp(SocketAddr),
}

impl Default for Config {
    fn default() -> Self {
        Self::with_data_dir(default_data_dir())
    }
}

impl Config {
    /// Build a config that uses `data_dir` and derives all sub-paths from it.
    pub fn with_data_dir(data_dir: PathBuf) -> Self {
        Self {
            repo_dir: data_dir.join("repo"),
            work_dir: data_dir.join("work"),
            logs_dir: data_dir.join("logs"),
            ccache_dir: data_dir.join("ccache"),
            gpg_home: data_dir.join(".gnupg"),
            data_dir,
            repo_name: default_repo_name(),
            arch: default_arch(),
            container_runtime: ContainerRuntime::default(),
            builder_image: default_builder_image(),
            max_workers: default_max_workers(),
            poll_interval_secs: default_poll_interval(),
            listen: default_listen(),
            public_base_url: default_public_base_url(),
            s3: None,
            local_repo: true,
        }
    }
}

fn default_data_dir() -> PathBuf {
    PathBuf::from("/var/lib/paur")
}
fn default_repo_dir() -> PathBuf {
    default_data_dir().join("repo")
}
fn default_work_dir() -> PathBuf {
    default_data_dir().join("work")
}
fn default_logs_dir() -> PathBuf {
    default_data_dir().join("logs")
}
fn default_ccache_dir() -> PathBuf {
    default_data_dir().join("ccache")
}
fn default_gpg_home() -> PathBuf {
    default_data_dir().join(".gnupg")
}
fn default_repo_name() -> String {
    "paur".into()
}
fn default_arch() -> String {
    "x86_64".into()
}
fn default_builder_image() -> String {
    "paur-builder:latest".into()
}
fn default_max_workers() -> u32 {
    1
}
fn default_poll_interval() -> u64 {
    600
}
fn default_listen() -> Listen {
    Listen::Unix(PathBuf::from("/run/paur/paur.sock"))
}
fn default_public_base_url() -> String {
    "https://localhost".into()
}

impl Config {
    /// Load config from a TOML file at `path`, falling back to defaults
    /// for any missing field. `PAUR_DATA_DIR` overrides `data_dir` from
    /// the environment, allowing a one-liner override for unit testing
    /// and quick re-runs.
    pub fn load(path: &Path) -> Result<Self> {
        let mut cfg = if path.exists() {
            let s = std::fs::read_to_string(path)?;
            toml::from_str::<Config>(&s)
                .map_err(|e| Error::Config(format!("parse {}: {e}", path.display())))?
        } else {
            Config::default()
        };
        if let Ok(env_dir) = std::env::var("PAUR_DATA_DIR") {
            let p = PathBuf::from(env_dir);
            cfg.data_dir = p.clone();
            // Refresh derived paths to follow the env override.
            cfg.repo_dir = p.join("repo");
            cfg.work_dir = p.join("work");
            cfg.logs_dir = p.join("logs");
            cfg.ccache_dir = p.join("ccache");
            cfg.gpg_home = p.join(".gnupg");
        }
        Ok(cfg)
    }

    /// Create all required directories on disk. Idempotent.
    pub fn ensure_dirs(&self) -> Result<()> {
        for d in [
            &self.data_dir,
            &self.repo_dir,
            &self.work_dir,
            &self.logs_dir,
            &self.ccache_dir,
            &self.gpg_home,
            &self.repo_dir.join(&self.arch),
        ] {
            std::fs::create_dir_all(d).map_err(|e| {
                Error::Config(format!("create_dir_all {}: {e}", d.display()))
            })?;
        }
        Ok(())
    }
}

/// Container runtime used to spawn build containers.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ContainerRuntime {
    /// Docker (`docker run ...`).
    #[default]
    Docker,
    /// Podman (`podman run ...`).
    Podman,
}

impl ContainerRuntime {
    /// The binary name used to spawn containers.
    pub fn binary(&self) -> &'static str {
        match self {
            ContainerRuntime::Docker => "docker",
            ContainerRuntime::Podman => "podman",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_has_under_data_dir() {
        let c = Config::default();
        assert!(c.repo_dir.starts_with(&c.data_dir));
        assert_eq!(c.repo_name, "paur");
    }

    #[test]
    fn load_missing_file_returns_default() {
        let c = Config::load(Path::new("/nonexistent/paur.toml")).unwrap();
        assert_eq!(c.repo_name, "paur");
    }

    #[test]
    fn load_partial_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("paur.toml");
        std::fs::write(&p, "repo_name = \"myrepo\"\nmax_workers = 4\n").unwrap();
        let c = Config::load(&p).unwrap();
        assert_eq!(c.repo_name, "myrepo");
        assert_eq!(c.max_workers, 4);
        // Defaults preserved for the rest.
        assert_eq!(c.container_runtime, ContainerRuntime::Docker);
    }

    #[test]
    fn env_override_data_dir() {
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: tests are not multi-threaded w.r.t. this env var.
        unsafe { std::env::set_var("PAUR_DATA_DIR", dir.path()) };
        let c = Config::load(Path::new("/nonexistent")).unwrap();
        unsafe { std::env::remove_var("PAUR_DATA_DIR") };
        assert_eq!(c.data_dir, dir.path());
        assert_eq!(c.repo_dir, dir.path().join("repo"));
    }
}
