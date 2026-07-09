//! Centralized error type for paur.

use thiserror::Error;

/// Convenient `Result` alias for fallible paur functions.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Top-level error type for paur. Each variant maps to a clear failure mode
/// the user can act on. Use [`Error::Other`] for unclassified errors and
/// prefer the dedicated variants for known cases.
#[derive(Debug, Error)]
pub enum Error {
    /// I/O error (file, network, pipe, etc.).
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// Configuration file is missing or invalid.
    #[error("config: {0}")]
    Config(String),

    /// SQLite/database error.
    #[error("db: {0}")]
    Db(String),

    /// Migration error.
    #[error("migration: {0}")]
    Migration(String),

    /// Failed to parse or validate a package name.
    #[error("invalid package name '{0}': {1}")]
    InvalidName(String, String),

    /// AUR interaction failed (clone, fetch, ls-remote, etc.).
    #[error("aur: {0}")]
    Aur(String),

    /// Container build failed.
    #[error("build: {0}")]
    Build(String),

    /// Repo publish step failed (repo-add, signing, copy).
    #[error("repo: {0}")]
    Repo(String),

    /// GPG signing or key operation failed.
    #[error("gpg: {0}")]
    Gpg(String),

    /// HTTP API error.
    #[error("api: {0}")]
    Api(String),

    /// Requested package or build was not found.
    #[error("not found: {0}")]
    NotFound(String),

    /// Caller passed conflicting or invalid input.
    #[error("invalid: {0}")]
    Invalid(String),

    /// A required external tool is missing (docker, podman, repo-add, gpg).
    #[error("missing dependency: {0}")]
    MissingDep(String),

    /// Catch-all for unclassified errors.
    #[error("{0}")]
    Other(String),
}

impl Error {
    /// Convenience constructor for a plain message.
    pub fn msg(s: impl Into<String>) -> Self {
        Error::Other(s.into())
    }
}
