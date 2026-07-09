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
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    logging::init();
    let cli = Cli::parse();
    let cfg = load_config(cli.config.as_deref());
    cfg.ensure_dirs()?;

    let db_path = cfg.data_dir.join("paur.db");
    let db = Db::from_pool(paur_db_open(&db_path).await?).await?;

    let repo = paur_daemon::build_repo_ctx(&cfg, &db).await?;
    let state = AppState::new(db, cfg.clone(), repo);

    match cli.cmd {
        Cmd::Serve { max_workers } => {
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
    }
    Ok(())
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
