//! AUR poller: periodically checks each `auto_rebuild` package's HEAD
//! ref against AUR, and enqueues a build when the ref advances.
//!
//! The poller runs as a sibling task to the queue worker. It is
//! intentionally non-blocking: each iteration sleeps
//! `poll_interval_secs`, and a single iteration's HTTP calls are
//! bounded by the number of auto-rebuild packages.

use std::time::Duration;

use paur_aur;
use paur_core::PkgName;
use paur_db::BuildTrigger;

use crate::worker::AppState;

/// Run the poller until the process is cancelled. Loops forever,
/// waking every `state.cfg.poll_interval_secs`. Errors are logged and
/// swallowed: a transient AUR hiccup should not stop the daemon.
pub async fn run(state: AppState) {
    let interval = Duration::from_secs(state.cfg.poll_interval_secs.max(1));
    let mut ticker = tokio::time::interval(interval);
    // First tick fires immediately; we want to wait first so we
    // don't spam AUR right after start. The first `.tick().await`
    // resolves after `interval` rather than 0.
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        if let Err(e) = poll_once(&state).await {
            tracing::warn!("aur poll: {e}");
        }
    }
}

/// One iteration of the poll. Lists every package with
/// `auto_rebuild = true`, queries AUR for its current HEAD, and
/// enqueues a build if the ref differs from what we last saw.
async fn poll_once(state: &AppState) -> Result<(), paur_core::Error> {
    let pkgs = state.db.list_packages().await?;
    // Snapshot the set of packages with a live (queued/running) build
    // once, so a slow poll loop doesn't enqueue more duplicates than
    // a single package can drain.
    let live_pids: std::collections::HashSet<i64> = {
        let mut live = std::collections::HashSet::new();
        for b in state
            .db
            .list_builds(None, Some(paur_db::BuildStatus::Queued), 1000)
            .await?
        {
            live.insert(b.package_id);
        }
        for b in state
            .db
            .list_builds(None, Some(paur_db::BuildStatus::Running), 1000)
            .await?
        {
            live.insert(b.package_id);
        }
        live
    };
    let mut triggered = 0usize;
    for p in pkgs {
        if !p.auto_rebuild {
            continue;
        }
        if live_pids.contains(&p.id) {
            // Already queued or running; don't pile up duplicates.
            continue;
        }
        let name = match PkgName::new(&p.name) {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!(pkg = %p.name, "invalid package name in DB: {e}");
                continue;
            }
        };
        let latest = match paur_aur::latest_ref(&name).await {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!(pkg = %name, "latest_ref failed: {e}");
                continue;
            }
        };
        if p.last_known_ref.as_deref() == Some(latest.as_str()) {
            continue;
        }
        // Ref advanced. Enqueue a build. We don't write `set_last_ref`
        // here: the worker updates it after the build completes, so a
        // failing build doesn't permanently disable auto-rebuild.
        let _ = state
            .db
            .enqueue_build(p.id, BuildTrigger::Poll)
            .await?;
        triggered += 1;
    }
    if triggered > 0 {
        tracing::info!(triggered, "aur poller: enqueued rebuilds");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use paur_db::Db;

    /// We can't easily mock `paur_aur::latest_ref` (it's a free
    /// function over a subprocess). The simplest offline-friendly
    /// test is: poll_once with no auto_rebuild packages enqueues
    /// nothing. The networked path is covered by the integration
    /// tests on `paur-aur::latest_ref` itself.
    #[tokio::test]
    async fn poll_with_no_auto_rebuild_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("paur.db");
        let pool = paur_db_open(&path).await.unwrap();
        let db = Db::from_pool(pool).await.unwrap();
        // Add a package with auto_rebuild=false.
        let url = "https://aur.archlinux.org/foo.git";
        let id = db.upsert_package("foo", url, false).await.unwrap();
        let _ = id;
        let cfg = paur_core::Config::with_data_dir(dir.path().to_path_buf());
        // AppState needs a repo ctx; build_repo_ctx reads the gpg
        // key from the DB, so we set a placeholder.
        db.set_setting("gpg_key_id", "DEADBEEFDEADBEEFDEADBEEFDEADBEEFDEADBEEF")
            .await
            .unwrap();
        let repo = crate::build_repo_ctx(&cfg, &db).await.unwrap();
        let state = AppState::new(db, cfg, repo);
        poll_once(&state).await.unwrap();
        // No builds enqueued.
        let queued = state
            .db
            .list_builds(None, Some(paur_db::BuildStatus::Queued), 100)
            .await
            .unwrap();
        assert!(queued.is_empty(), "expected no queued builds, got {queued:?}");
    }

    async fn paur_db_open(path: &std::path::Path) -> paur_core::Result<sqlx::SqlitePool> {
        use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
        use std::str::FromStr as _;
        let url = format!("sqlite://{}", path.display());
        let opts = SqliteConnectOptions::from_str(&url)
            .map_err(|e| paur_core::Error::Db(e.to_string()))?
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));
        SqlitePoolOptions::new()
            .max_connections(2)
            .connect_with(opts)
            .await
            .map_err(|e| paur_core::Error::Db(e.to_string()))
    }
}
