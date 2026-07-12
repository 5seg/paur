//! paur-db: SQLite persistence layer.

pub mod models;
pub mod queries;
pub mod schema;

pub use models::{Build, BuildStatus, BuildTrigger, Package, Setting, Stream};
pub use queries::{CancelOutcome, Db};

use std::path::Path;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::str::FromStr;

/// Open a connection pool to the SQLite database at `path` (or `:memory:`).
/// Runs migrations on first open.
pub async fn open(path: &Path) -> paur_core::Result<Db> {
    let url = if path == Path::new(":memory:") {
        "sqlite::memory:".to_string()
    } else {
        format!("sqlite://{}", path.display())
    };
    let opts = SqliteConnectOptions::from_str(&url)
        .map_err(|e| paur_core::Error::Db(e.to_string()))?
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .busy_timeout(std::time::Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(opts)
        .await
        .map_err(|e| paur_core::Error::Db(e.to_string()))?;
    Db::from_pool(pool).await
}

/// Open with a pre-built pool. Used by tests.
pub async fn from_pool(pool: SqlitePool) -> paur_core::Result<Db> {
    Db::from_pool(pool).await
}
