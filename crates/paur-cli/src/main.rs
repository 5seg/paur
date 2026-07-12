//! `paur-cli` — CLI front-end for paur.
//!
//! Most commands talk to a running daemon over HTTP. A few (`init`,
//! `repo-init`, `config`, `doctor`, `print-pacman-conf`) work without
//! a daemon by reading/writing the local DB and config directly.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use paur_cli::{client::DaemonClient, cmd};
use paur_core::{Config, Variant};

#[derive(Parser, Debug)]
#[command(name = "paur-cli", about = "CLI front-end for the paur pre-build service", version)]
struct Cli {
    /// Path to paur's config file. Defaults to `$PAUR_DATA_DIR/config.toml`
    /// or `/var/lib/paur/config.toml`.
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    /// Override the daemon's HTTP base URL (default: derived from
    /// `listen` in the config).
    #[arg(long, global = true)]
    api: Option<String>,

    /// Verbosity: `-v` for info logs, `-vv` for debug.
    #[arg(long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Add a package and enqueue its first build. The default
    /// variant is always built; `--variant` is repeatable and adds
    /// v3 / v4 builds on top.
    Add {
        /// Package name (AUR base name, e.g. `paru-bin`).
        name: String,
        /// Mark this package for automatic rebuild on AUR HEAD change.
        #[arg(long)]
        auto_rebuild: bool,
        /// Repeatable. Adds the named variant (`v3` or `v4`) to
        /// the active set. `default` is implicit and cannot be
        /// passed here.
        #[arg(long = "variant", value_parser = parse_variant_arg)]
        variant: Vec<Variant>,
    },
    /// Remove a package from paur.
    Remove {
        /// Package name.
        name: String,
    },
    /// List all packages.
    List,
    /// Show the status of a single package.
    Status {
        /// Package name.
        name: String,
    },
    /// Show the build log for a package.
    Logs {
        /// Package name.
        name: String,
        /// Build id (default: most recent).
        #[arg(long)]
        build: Option<i64>,
        /// Follow the log in real time (SSE).
        #[arg(long, short)]
        follow: bool,
    },
    /// Enqueue a rebuild for an existing package.
    Rebuild {
        /// Package name.
        name: String,
    },
    /// Read or update per-package build tuning flags and active
    /// variants. With no flags, prints the current values.
    Flag {
        /// Package name.
        name: String,
        /// Cap parallel make jobs to `-j2` to cut peak RAM.
        #[arg(long, value_parser = parse_on_off)]
        low_memory: Option<bool>,
        /// Append `-C codegen-units=1` to `RUSTFLAGS` for Rust builds.
        #[arg(long, value_parser = parse_on_off)]
        rust_codegen_units_1: Option<bool>,
        /// Skip ccache bind mount for this package.
        #[arg(long, value_parser = parse_on_off)]
        no_ccache: Option<bool>,
        /// Toggle one of the package's variants. Repeatable.
        /// Each occurrence flips the named variant (on → off or
        /// off → on). `default` is invariant and rejected.
        #[arg(long = "variant", value_parser = parse_variant_arg)]
        variant: Vec<Variant>,
    },
    /// Show the current queue and running builds.
    Queue,
    /// Print the GPG public key the repo is signed with.
    Pubkey,
    /// First-run setup: create dirs, generate a signing key, export
    /// the pubkey, and write `gpg_key_id` to the DB.
    Init {
        /// Overwrite an existing key.
        #[arg(long)]
        force: bool,
        /// Key real name (default: "paur").
        #[arg(long)]
        key_name: Option<String>,
        /// Key email (default: `paur@localhost`).
        #[arg(long)]
        key_email: Option<String>,
    },
    /// Re-export the GPG pubkey and create the empty repo db file
    /// (idempotent). Useful after restoring a backup of the repo dir.
    RepoInit,
    /// Read a setting from the DB.
    #[command(subcommand)]
    Config(ConfigCmd),
    /// Sanity-check the local environment.
    Doctor,
    /// Print the lines to add to `/etc/pacman.conf`.
    PrintPacmanConf,
    /// Build and publish the `paur-keyring` and `paur-mirrorlist`
    /// meta-packages. After running this, a client can install both
    /// with `pacman -U` and never has to look up a fingerprint.
    KeyringBuild,
    /// Run the daemon (foreground). Equivalent to `paur serve`.
    Serve {
        /// Override `max_workers` from config.
        #[arg(long)]
        max_workers: Option<u32>,
    },
}

#[derive(Subcommand, Debug)]
enum ConfigCmd {
    /// Read a setting by key.
    Get { key: String },
    /// Set a setting key/value.
    Set { key: String, value: String },
    /// List all settings.
    List,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    init_tracing();
    let cli = Cli::parse();

    let cfg = match load_config(cli.config.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("paur-cli: {e}");
            std::process::exit(1);
        }
    };

    let exit_code = match run(cli, cfg).await {
        Ok(()) => 0,
        Err(e) => {
            let code = e.exit_code();
            eprintln!("paur-cli: {e}");
            code
        }
    };
    std::process::exit(exit_code);
}

async fn run(cli: Cli, cfg: Config) -> Result<(), cmd::CmdError> {
    match cli.cmd {
        Cmd::Add {
            name,
            auto_rebuild,
            variant,
        } => {
            let c = client(&cfg, cli.api.as_deref());
            cmd::add(&c, &name, auto_rebuild, &variant).await
        }
        Cmd::Remove { name } => {
            let c = client(&cfg, cli.api.as_deref());
            cmd::remove(&c, &name).await
        }
        Cmd::List => {
            let c = client(&cfg, cli.api.as_deref());
            cmd::list(&c).await
        }
        Cmd::Status { name } => {
            let c = client(&cfg, cli.api.as_deref());
            cmd::status(&c, &name).await
        }
        Cmd::Logs { name, build, follow } => {
            let c = client(&cfg, cli.api.as_deref());
            cmd::logs(&c, &name, build, follow).await
        }
        Cmd::Rebuild { name } => {
            let c = client(&cfg, cli.api.as_deref());
            cmd::rebuild(&c, &name).await
        }
        Cmd::Flag {
            name,
            low_memory,
            rust_codegen_units_1,
            no_ccache,
            variant,
        } => {
            let c = client(&cfg, cli.api.as_deref());
            cmd::flag(
                &c,
                &name,
                low_memory,
                rust_codegen_units_1,
                no_ccache,
                &variant,
            )
            .await
        }
        Cmd::Queue => {
            let c = client(&cfg, cli.api.as_deref());
            cmd::queue(&c).await
        }
        Cmd::Pubkey => {
            let c = client(&cfg, cli.api.as_deref());
            cmd::pubkey(&c).await
        }
        Cmd::Init { force, key_name, key_email } => {
            cmd::init(&cfg, force, key_name.as_deref(), key_email.as_deref()).await
        }
        Cmd::RepoInit => cmd::repo_init(&cfg).await,
        Cmd::Config(sub) => match sub {
            ConfigCmd::Get { key } => cmd::config_get(&cfg, &key).await,
            ConfigCmd::Set { key, value } => cmd::config_set(&cfg, &key, &value).await,
            ConfigCmd::List => cmd::config_list(&cfg).await,
        },
        Cmd::Doctor => cmd::doctor(&cfg).await,
        Cmd::PrintPacmanConf => {
            cmd::print_pacman_conf(&cfg);
            Ok(())
        }
        Cmd::KeyringBuild => cmd::keyring_build(&cfg).await,
        Cmd::Serve { max_workers } => {
            // Delegate to paur-daemon.
            paur_serve::serve(&cfg, max_workers).await
        }
    }
}

/// Build a daemon client. If `override_url` is given, use it; otherwise
/// derive from the config's listen address.
fn client(cfg: &Config, override_url: Option<&str>) -> DaemonClient {
    match override_url {
        Some(u) => DaemonClient::new(u),
        None => DaemonClient::from_config(cfg),
    }
}

fn load_config(path: Option<&std::path::Path>) -> Result<Config, String> {
    let p = match path {
        Some(p) => p.to_path_buf(),
        None => std::env::var("PAUR_DATA_DIR")
            .map(|d| PathBuf::from(d).join("config.toml"))
            .unwrap_or_else(|_| PathBuf::from("/var/lib/paur/config.toml")),
    };
    Config::load(&p).map_err(|e| format!("config: {e}"))
}

fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("paur_cli=info"));
    let _ = fmt().with_env_filter(filter).try_init();
}

/// Parse `on`/`off`/`true`/`false`/`1`/`0` for boolean flags. Case
/// insensitive. Errors are user-visible so clap prints them.
fn parse_on_off(s: &str) -> Result<bool, String> {
    match s.to_ascii_lowercase().as_str() {
        "on" | "true" | "1" | "yes" => Ok(true),
        "off" | "false" | "0" | "no" => Ok(false),
        other => Err(format!(
            "expected on/off (got {other:?}); use --list to view current values"
        )),
    }
}

/// Parse a `--variant` argument. Accepts `v3` / `v4` (case
/// insensitive). `default` is rejected because the daemon
/// enforces it as an invariant and silently dropping it would
/// confuse the user.
fn parse_variant_arg(s: &str) -> Result<Variant, String> {
    match s.to_ascii_lowercase().as_str() {
        "v3" => Ok(Variant::V3),
        "v4" => Ok(Variant::V4),
        "default" => Err("default is always on; cannot be set explicitly".into()),
        other => Err(format!("expected v3|v4 (got {other:?})")),
    }
}

/// Tiny facade that re-exports the daemon's `serve` entry points under
/// a stable name. The daemon's `paur_daemon::run` takes an `AppState`
/// and is meant to be used from the `paur` binary; we re-wrap it here
/// for the CLI.
mod paur_serve {
    use paur_core::Config;
    use paur_daemon::AppState;
    use paur_db::Db;

    pub async fn serve(cfg: &Config, max_workers: Option<u32>) -> Result<(), super::cmd::CmdError> {
        let db_path = cfg.data_dir.join("paur.db");
        let db = Db::from_pool(open_pool(&db_path).await.map_err(|e| {
            super::cmd::CmdError::Other(format!("db open: {e}"))
        })?)
        .await
        .map_err(super::cmd::CmdError::Core)?;
        let repo = paur_daemon::build_repo_ctx(&cfg, &db).await?;
        let state = AppState::new(db, cfg.clone(), repo);
        // Spawn the API, the poller, and the worker.
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
        let n = max_workers.unwrap_or(cfg.max_workers);
        if let Err(e) = paur_daemon::run(state, n).await {
            poller_task.abort();
            return Err(super::cmd::CmdError::Core(e));
        }
        poller_task.abort();
        let _ = api_task.await;
        Ok(())
    }

    async fn open_pool(
        path: &std::path::Path,
    ) -> Result<sqlx::SqlitePool, Box<dyn std::error::Error>> {
        use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
        use std::str::FromStr as _;
        let url = format!("sqlite://{}", path.display());
        let opts = SqliteConnectOptions::from_str(&url)?
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .busy_timeout(std::time::Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(opts)
            .await?;
        Ok(pool)
    }
}
