//! `paur` daemon entry point. Currently exposes only `serve` (the build
//! worker). HTTP API, init, and CLI live in their own subcommands
//! (added in later steps).

use clap::{Parser, Subcommand};
use paur_core::{logging, Config};
use paur_daemon::AppState;
use paur_db::Db;

#[derive(Parser, Debug)]
#[command(name = "paur", about = "Personal AUR pre-build service")]
struct Cli {
    /// Path to the config file. Defaults to `<data_dir>/config.toml`
    /// (overridable via `PAUR_DATA_DIR`).
    #[arg(long, global = true)]
    config: Option<std::path::PathBuf>,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Run the build worker in the foreground. This is the main
    /// long-running entry point.
    Serve {
        /// Override max workers from config.
        #[arg(long)]
        max_workers: Option<u32>,
    },
    /// Set or replace the admin password used to gate package
    /// add / rebuild / delete. Reads the new password twice from
    /// stdin (no echo).
    Passwd,
    /// Remove transient build artifacts from the data dir.
    /// Currently supported subcommands:
    ///   - work: delete every `<work_dir>/<build_id>/` directory
    ///     except those that belong to a build that is currently
    ///     `running`. Run while the daemon is stopped (or accept
    ///     the no-op for any in-flight build).
    Cleanup {
        #[command(subcommand)]
        target: CleanupTarget,
    },
}

#[derive(Subcommand, Debug)]
enum CleanupTarget {
    /// Delete leftover `<work_dir>/<build_id>/` directories. Safe to
    /// run repeatedly: a missing directory is treated as already
    /// clean. Skips any build that is currently `running` so a live
    /// container is not disrupted.
    Work,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    logging::init();
    let cli = Cli::parse();
    let cfg = load_config(cli.config.as_deref());

    let db_path = cfg.data_dir.join("paur.db");
    let db = Db::from_pool(paur_db_open(&db_path).await?).await?;

    match cli.cmd {
        Cmd::Serve { max_workers } => {
            // Worker / HTTP / poller all need the on-disk layout and a
            // signing context. Run these here so subcommands that
            // shouldn't need them (cleanup, passwd) can run on a
            // freshly-provisioned, uninitialized host.
            cfg.ensure_dirs()?;
            let repo = paur_daemon::build_repo_ctx(&cfg, &db).await?;
            let state = AppState::new(db.clone(), cfg.clone(), repo);
            let n = max_workers.unwrap_or(cfg.max_workers);
            tracing::info!(
                max_workers = n,
                data_dir = %cfg.data_dir.display(),
                "paur: starting daemon (worker + http api + aur poller)"
            );
            let api_cfg = cfg.clone();
            let api_state = state.clone();
            let api_task = tokio::spawn(async move {
                if let Err(e) = paur_daemon::serve(&api_cfg, api_state).await {
                    tracing::error!("http api: {e}");
                }
            });
            let poller_state = state.clone();
            let poller_task = tokio::spawn(async move {
                paur_daemon::poller::run(poller_state).await;
            });
            paur_daemon::run(state, n).await?;
            poller_task.abort();
            let _ = api_task.await;
        }
        Cmd::Passwd => {
            run_passwd(&db).await?;
        }
        Cmd::Cleanup { target } => match target {
            CleanupTarget::Work => run_cleanup_work(&db, &cfg).await?,
        },
    }
    Ok(())
}

/// Read a password from stdin with echo disabled. Falls back to a
/// regular line read when stdin is not a tty (e.g. in scripts), so
/// this also works in containers / CI.
fn read_password(prompt: &str) -> std::io::Result<String> {
    use std::io::{IsTerminal, Read, Write};
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        rpassword::prompt_password(prompt)
    } else {
        eprint!("{prompt}");
        std::io::stderr().flush().ok();
        // Best-effort: there's no portable way to disable echo on a
        // non-tty stdin, so on a pipe the password may end up in
        // process listings. Documented in `--help`.
        let mut s = String::new();
        stdin.lock().read_to_string(&mut s)?;
        Ok(s.trim_end_matches(['\n', '\r']).to_string())
    }
}

async fn run_passwd(db: &paur_db::Db) -> Result<(), Box<dyn std::error::Error>> {
    let pw1 = read_password("New admin password: ")?;
    if pw1.is_empty() {
        return Err("password cannot be empty".into());
    }
    let pw2 = read_password("Retype admin password: ")?;
    if pw1 != pw2 {
        return Err("passwords do not match".into());
    }
    let hash = paur_core::auth::hash_password(&pw1)
        .map_err(|e| format!("bcrypt: {e}"))?;
    db.set_setting(paur_daemon::auth::SETTING_PASSWORD_HASH, &hash)
        .await?;
    // Invalidate every existing session: a password rotation should
    // force re-auth everywhere.
    db.delete_all_sessions().await?;
    tracing::info!("admin password updated; existing sessions cleared");
    Ok(())
}

/// Delete every `<work_dir>/<build_id>/` directory except those
/// still in use by a running build. We treat the build id as the
/// directory name (matches the convention the worker uses when it
/// bind-mounts the work dir into the container), so the operation
/// is just a filtered `rm -rf`.
///
/// Refuses to walk a non-directory `work_dir`. Refuses to follow
/// symlinks: a hostile build id directory shouldn't be able to make
/// us delete `/etc`.
async fn run_cleanup_work(
    db: &paur_db::Db,
    cfg: &paur_core::Config,
) -> Result<(), Box<dyn std::error::Error>> {
    let work = &cfg.work_dir;
    if !work.exists() {
        println!("work dir {} does not exist; nothing to do", work.display());
        return Ok(());
    }
    if !work.is_dir() {
        return Err(format!(
            "work_dir {} is not a directory",
            work.display()
        )
        .into());
    }

    // Collect live build ids so we can skip their work dirs.
    let running = db
        .list_builds(None, Some(paur_db::BuildStatus::Running), None, 10_000)
        .await?;
    let live: std::collections::HashSet<i64> = running.into_iter().map(|b| b.id).collect();

    let mut total_removed = 0u64;
    let mut bytes_removed: u64 = 0;
    for entry in std::fs::read_dir(work)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        // The build id is the directory name. Skip anything that
        // doesn't parse as i64 — those are unrelated files a human
        // dropped here.
        let Ok(id) = name.parse::<i64>() else { continue };
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if live.contains(&id) {
            println!("skip {id}: build is running");
            continue;
        }
        let size = dir_size(&path).unwrap_or(0);
        match std::fs::remove_dir_all(&path) {
            Ok(()) => {
                total_removed += 1;
                bytes_removed += size;
                println!("removed {id} ({})", human_bytes(size));
            }
            Err(e) => {
                eprintln!("failed to remove {}: {e}", path.display());
            }
        }
    }
    println!(
        "done: removed {total_removed} work dir(s), {}",
        human_bytes(bytes_removed)
    );
    Ok(())
}

/// Best-effort recursive size of a directory. Returns `None` if
/// anything is unreadable; the caller treats that as 0.
fn dir_size(p: &std::path::Path) -> Option<u64> {
    let mut total = 0u64;
    fn walk(p: &std::path::Path, total: &mut u64) -> std::io::Result<()> {
        for e in std::fs::read_dir(p)? {
            let e = e?;
            let meta = e.metadata()?;
            if meta.is_dir() {
                walk(&e.path(), total)?;
            } else if meta.is_file() {
                *total += meta.len();
            }
        }
        Ok(())
    }
    walk(p, &mut total).ok().map(|_| total)
}

fn human_bytes(n: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", UNITS[i])
}

fn load_config(path: Option<&std::path::Path>) -> Config {
    if let Some(p) = path {
        Config::load(p).expect("config")
    } else {
        let p = std::env::var("PAUR_DATA_DIR")
            .map(|d| std::path::PathBuf::from(d).join("config.toml"))
            .unwrap_or_else(|_| std::path::PathBuf::from("/var/lib/paur/config.toml"));
        Config::load(&p).expect("config")
    }
}

/// Open a SQLite pool wrapped in a way that matches the lib's
/// `from_pool` signature. Lives here so the binary doesn't have to
/// depend on `sqlx` directly.
async fn paur_db_open(
    path: &std::path::Path,
) -> Result<sqlx::SqlitePool, paur_core::Error> {
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::str::FromStr;
    let url = format!("sqlite://{}", path.display());
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
    Ok(pool)
}
