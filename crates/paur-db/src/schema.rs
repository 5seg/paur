//! Embedded migrations. Each migration is loaded at compile time and
//! applied at first open. Migrations are append-only; never edit a
//! file that has shipped.

use sqlx::migrate::Migrator;
use sqlx::SqlitePool;

pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// Run all pending migrations on `pool`. Idempotent.
pub async fn run(pool: &SqlitePool) -> paur_core::Result<()> {
    MIGRATOR
        .run(pool)
        .await
        .map_err(|e| paur_core::Error::Migration(e.to_string()))
}
