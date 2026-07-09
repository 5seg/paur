//! Path resolution helpers built on top of [`Config`].
//!
//! `Paths` centralises the "where does X live on disk" logic so that no
//! other crate has to know the layout. The resolver is intentionally
//! infallible: every input is a known subdirectory of the data dir.

use std::path::{Path, PathBuf};

use crate::Config;

/// Resolves canonical filesystem locations for all of paur's runtime data.
#[derive(Debug, Clone)]
pub struct Paths {
    cfg: Config,
}

impl Paths {
    /// Build a new resolver from a config.
    pub fn new(cfg: Config) -> Self {
        Self { cfg }
    }

    /// Borrow the underlying config.
    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// Top-level data dir.
    pub fn data_dir(&self) -> &Path {
        &self.cfg.data_dir
    }

    /// Repo dir (where `.pkg.tar.zst` and `paur.db` live).
    pub fn repo_dir(&self) -> &Path {
        &self.cfg.repo_dir
    }

    /// Architecture-specific subdir of the repo.
    pub fn arch_dir(&self) -> PathBuf {
        self.cfg.repo_dir.join(&self.cfg.arch)
    }

    /// Per-build workdir for a given build id.
    pub fn work_for(&self, build_id: i64) -> PathBuf {
        self.cfg.work_dir.join(build_id.to_string())
    }

    /// `src/` subdir inside a build workdir.
    pub fn work_src(&self, build_id: i64) -> PathBuf {
        self.work_for(build_id).join("src")
    }

    /// `out/` subdir inside a build workdir (where the container writes
    /// the resulting `.pkg.tar.zst`).
    pub fn work_out(&self, build_id: i64) -> PathBuf {
        self.work_for(build_id).join("out")
    }

    /// Log file path for a build.
    pub fn log_for(&self, build_id: i64) -> PathBuf {
        self.cfg.logs_dir.join(format!("{build_id}.log"))
    }

    /// ccache dir.
    pub fn ccache_dir(&self) -> &Path {
        &self.cfg.ccache_dir
    }

    /// GPG home.
    pub fn gpg_home(&self) -> &Path {
        &self.cfg.gpg_home
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_paths_nest_under_build_id() {
        let p = Paths::new(Config::default());
        let w = p.work_for(42);
        assert_eq!(w, p.config().work_dir.join("42"));
        assert_eq!(p.work_src(42), w.join("src"));
        assert_eq!(p.work_out(42), w.join("out"));
        assert_eq!(p.log_for(42), p.config().logs_dir.join("42.log"));
    }
}
