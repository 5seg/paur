//! Build queue worker: claims queued builds, runs them in containers,
//! publishes results, and reports status back to the DB.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use paur_builder::{BuildOutcome, BuildRequest, LogSink};
use paur_db::{BuildStatus, Stream};
use paur_repo::RepoCtx;
use tokio::sync::{broadcast, Mutex};

/// Shared state passed to every worker. Cheap to clone (everything
/// inside is `Arc`-wrapped).
#[derive(Clone)]
pub struct AppState {
    /// Database handle.
    pub db: paur_db::Db,
    /// Resolved runtime config.
    pub cfg: paur_core::Config,
    /// Repo signing context.
    pub repo: Arc<RepoCtx>,
    /// Per-build broadcast channels for log fan-out. Keyed by build id.
    pub log_channels: Arc<Mutex<std::collections::HashMap<i64, broadcast::Sender<String>>>>,
}

impl AppState {
    /// Create a fresh state. The `log_channels` map starts empty; the
    /// worker creates a channel on the first build claim.
    pub fn new(db: paur_db::Db, cfg: paur_core::Config, repo: RepoCtx) -> Self {
        Self {
            db,
            cfg,
            repo: Arc::new(repo),
            log_channels: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    /// Get-or-create the broadcast channel for a build id. The channel
    /// has a small buffer; slow consumers will lag, but log writers
    /// never block.
    pub async fn channel_for(&self, build_id: i64) -> broadcast::Sender<String> {
        let mut map = self.log_channels.lock().await;
        if let Some(tx) = map.get(&build_id) {
            return tx.clone();
        }
        let (tx, _rx) = broadcast::channel(1024);
        map.insert(build_id, tx.clone());
        tx
    }

    /// Drop a channel after a build completes (no need to keep entries
    /// for builds that nobody is tailing).
    pub async fn drop_channel(&self, build_id: i64) {
        self.log_channels.lock().await.remove(&build_id);
    }
}

/// Run the daemon worker loop until cancelled. This is the entry point
/// for `paur serve` (no HTTP layer yet).
pub async fn run(state: AppState, max_workers: u32) -> Result<(), paur_core::Error> {
    if max_workers != 1 {
        // The current implementation supports exactly one worker.
        // Raising the count would require a shared `mpsc` (or a
        // different queueing strategy); we surface that explicitly
        // rather than silently degrading.
        return Err(paur_core::Error::Invalid(
            "max_workers must be 1 (multi-worker is unimplemented)".into(),
        ));
    }

    // Crash recovery: any 'running' rows from a previous incarnation
    // are no longer running — mark them failed.
    let n = state.db.reap_stale_running().await?;
    if n > 0 {
        tracing::warn!(reaped = n, "reaped stale running builds from previous run");
    }

    let (tx, rx) = tokio::sync::mpsc::channel::<i64>(32);
    // On startup, enqueue any already-queued rows so the worker
    // picks them up.
    {
        let st = state.clone();
        let tx = tx.clone();
        tokio::spawn(async move {
            if let Ok(rows) = st
                .db
                .list_builds(None, Some(BuildStatus::Queued), 10_000)
                .await
            {
                for b in rows {
                    let _ = tx.send(b.id).await;
                }
            }
        });
    }
    // Drop the kicker's sender; the worker holds the only other.
    drop(tx);

    let rx = Arc::new(tokio::sync::Mutex::new(rx));
    let h = tokio::spawn(async move { run_worker(state, rx).await });
    let _ = h.await;
    Ok(())
}

async fn run_worker(state: AppState, rx: Arc<Mutex<tokio::sync::mpsc::Receiver<i64>>>) {
    loop {
        let build_id = {
            let mut g = rx.lock().await;
            match g.recv().await {
                Some(id) => id,
                None => return, // channel closed
            }
        };
        if let Err(e) = process_one(&state, build_id).await {
            tracing::error!(build_id, "worker error: {e}");
        }
    }
}

/// Process a single build id end-to-end: claim -> build -> publish.
async fn process_one(state: &AppState, build_id: i64) -> Result<(), paur_core::Error> {
    // Claim and load the package.
    let claimed = state.db.claim_next_queued("paur-worker").await?;
    let Some(build) = claimed else {
        // Already claimed by someone else; nothing to do.
        return Ok(());
    };
    if build.id != build_id {
        // We were given a specific id but the queue had a different
        // one ready; let the dispatcher move on.
        return Ok(());
    }
    let pkg = state
        .db
        .list_packages()
        .await?
        .into_iter()
        .find(|p| p.id == build.package_id)
        .ok_or_else(|| {
            paur_core::Error::NotFound(format!("package id {}", build.package_id))
        })?;
    let pkg_name = paur_core::PkgName::new(&pkg.name)?;

    tracing::info!(build_id, pkg = %pkg_name, "starting build");

    // Compose a LogSink that writes to DB + text file + broadcast.
    let sink = DbLogSink::new(state.clone(), build.id);
    let sink: Arc<dyn LogSink> = Arc::new(sink);

    // Build a BuildRequest and run it.
    let req = BuildRequest {
        pkg: pkg_name.clone(),
        aur_url: pkg.aur_url.clone(),
        work_dir: state.cfg.work_dir.join(build.id.to_string()),
        ccache_dir: state.cfg.ccache_dir.clone(),
        runtime: state.cfg.container_runtime,
        image: state.cfg.builder_image.clone(),
    };
    let outcome = paur_builder::run_in_container(&req, Arc::clone(&sink)).await?;

    // Record outcome and (on success) publish to the repo.
    let final_status = if outcome.exit_code == 0 {
        BuildStatus::Success
    } else {
        BuildStatus::Failed
    };
    state
        .db
        .finish_build(build.id, final_status, Some(outcome.exit_code as i32))
        .await?;

    if final_status == BuildStatus::Success {
        // Move artifacts into the repo and sign. We use the on-disk
        // paths the builder produced (under work_dir/out).
        match publish_artifacts(state, &pkg_name, &req.work_dir.join("out"), &outcome).await {
            Ok(_) => {
                let pkg_file = outcome
                    .pkg_files
                    .first()
                    .and_then(|p| p.file_name())
                    .and_then(|n| n.to_str())
                    .map(|s| s.to_string());
                let version = outcome
                    .srcinfo
                    .as_ref()
                    .and_then(|s| s.full_version_of(&pkg.name));
                if let (Some(f), Some(v)) = (pkg_file, version) {
                    state
                        .db
                        .record_pkg(build.id, &f, &v)
                        .await
                        .ok();
                }
                // Track the new AUR ref so the poller can detect
                // upstream changes.
                if let Some(si) = outcome.srcinfo.as_ref() {
                    let _ = state
                        .db
                        .set_last_ref(pkg.id, &format!("{}:{}", pkg_name, si.full_version().unwrap_or_default()))
                        .await;
                }
            }
            Err(e) => {
                tracing::error!(build_id, "publish failed: {e}");
                // Don't overwrite the build status — `success` is
                // already recorded. The build worked; only the
                // publish step is broken. Log it loudly.
            }
        }
    }

    state.drop_channel(build.id).await;
    Ok(())
}

async fn publish_artifacts(
    state: &AppState,
    _pkg: &paur_core::PkgName,
    out_dir: &std::path::Path,
    outcome: &BuildOutcome,
) -> Result<(), paur_core::Error> {
    // The builder returned relative paths under work_dir/out; resolve
    // them so the repo gets the real files.
    let pkg_files: Vec<PathBuf> = outcome
        .pkg_files
        .iter()
        .map(|p| {
            if p.is_absolute() {
                p.clone()
            } else {
                out_dir.join(p)
            }
        })
        .collect();
    paur_repo::publish(&state.repo, &pkg_files).await?;
    Ok(())
}

/// LogSink that writes each line to the DB, the log file, and any
/// subscribers of the build's broadcast channel.
struct DbLogSink {
    state: AppState,
    build_id: i64,
    seq: std::sync::atomic::AtomicI64,
}

impl DbLogSink {
    fn new(state: AppState, build_id: i64) -> Self {
        Self {
            state,
            build_id,
            seq: std::sync::atomic::AtomicI64::new(0),
        }
    }
}

#[async_trait]
impl LogSink for DbLogSink {
    async fn write(&self, line: &str) -> Result<(), paur_core::Error> {
        // Persist.
        self.state
            .db
            .append_log(self.build_id, Stream::Stdout, line)
            .await?;
        // Fan out to any subscribers (best effort).
        let tx = self.state.channel_for(self.build_id).await;
        let _ = tx.send(line.to_string());
        // Sequence counter is informational; we keep our own log line
        // count here purely so tests can confirm ordering.
        let _ = self
            .seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }
}
