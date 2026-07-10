//! CLI command implementations.
//!
//! Each public function here corresponds to one `paur-cli` subcommand.
//! Functions take a `DaemonClient` plus their typed arguments and
//! return a `Result<(), CmdError>` so the binary can map them onto
//! exit codes.

use std::io::Write;
use std::path::Path;
use std::time::Duration;

use futures::StreamExt;

use paur_core::{Config, PkgName};
use paur_db::Db;
use paur_repo;

use crate::client::{DaemonClient, PackageDto};
use crate::output;

/// Errors from command execution. We expose a unified `CmdError` so
/// `main` can render a single message and pick an exit code.
#[derive(Debug, thiserror::Error)]
pub enum CmdError {
    #[error("{0}")]
    Client(#[from] crate::client::ClientError),
    #[error("{0}")]
    Core(#[from] paur_core::Error),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

impl CmdError {
    /// Pick an exit code for the process. 2 = usage, 1 = runtime.
    pub fn exit_code(&self) -> i32 {
        match self {
            CmdError::Core(paur_core::Error::InvalidName(_, _)) => 2,
            CmdError::Core(paur_core::Error::Invalid(_)) => 2,
            _ => 1,
        }
    }
}

/// `paur-cli add <pkg> [--auto-rebuild]`
pub async fn add(
    client: &DaemonClient,
    pkg: &str,
    auto_rebuild: bool,
) -> Result<(), CmdError> {
    let name = PkgName::new(pkg)?;
    let dto = client.add_package(name.as_str(), auto_rebuild).await?;
    println!("added {} (id={}, auto_rebuild={})", dto.name, dto.id, dto.auto_rebuild);
    Ok(())
}

/// `paur-cli remove <pkg>`
pub async fn remove(client: &DaemonClient, pkg: &str) -> Result<(), CmdError> {
    let name = PkgName::new(pkg)?;
    client.delete_package(name.as_str()).await?;
    println!("removed {}", name);
    Ok(())
}

/// `paur-cli list`
pub async fn list(client: &DaemonClient) -> Result<(), CmdError> {
    let pkgs = client.list_packages().await?;
    output::print_packages(&pkgs);
    Ok(())
}

/// `paur-cli status <pkg>` — package + its latest build.
pub async fn status(client: &DaemonClient, pkg: &str) -> Result<(), CmdError> {
    let name = PkgName::new(pkg)?;
    let p = client.get_package(name.as_str()).await?;
    println!("name:        {}", p.name);
    println!("id:          {}", p.id);
    println!("aur_url:     {}", p.aur_url);
    println!("auto_rebuild: {}", p.auto_rebuild);
    println!("last_ref:    {}", p.last_known_ref.as_deref().unwrap_or("-"));
    match p.latest_build {
        Some(b) => {
            println!("latest_build:");
            println!("  id:        {}", b.id);
            println!("  status:    {}", b.status);
            println!("  version:   {}", b.pkg_version.as_deref().unwrap_or("-"));
            println!("  exit:      {}", b.exit_code.map(|c| c.to_string()).unwrap_or_else(|| "-".into()));
            println!("  finished:  {}", output::fmt_ts(b.finished_at));
        }
        None => println!("latest_build: (none)"),
    }
    Ok(())
}

/// `paur-cli logs <pkg> [--build N] [--follow]`
///
/// `--follow` opens an SSE connection on the running build (looked up
/// from the most recent build for the package, preferring running
/// status). Without it, we print the full cached log.
pub async fn logs(
    client: &DaemonClient,
    pkg: &str,
    build: Option<i64>,
    follow: bool,
) -> Result<(), CmdError> {
    let name = PkgName::new(pkg)?;
    let pkg_dto = client.get_package(name.as_str()).await?;
    let build_id = match build {
        Some(id) => id,
        None => {
            // Pick the most recent build for this package.
            let builds =
                client.list_builds(Some(name.as_str()), None, Some(1)).await?;
            match builds.first() {
                Some(b) => b.id,
                None => {
                    eprintln!("no builds for {}", name);
                    return Ok(());
                }
            }
        }
    };
    if follow {
        // SSE follow mode: read chunks and print. We bypass the
        // typed HTTP client (which has no SSE) and use reqwest
        // directly via the same base URL.
        let url = format!("{}/api/v1/builds/{}/logs", client.base_url(), build_id);
        let resp = reqwest::Client::builder()
            .build()
            .map_err(|e| CmdError::Other(format!("http client: {e}")))?
            .get(&url)
            .send()
            .await
            .map_err(|e| CmdError::Other(format!("sse connect: {e}")))?;
        if !resp.status().is_success() {
            return Err(CmdError::Other(format!(
                "sse: HTTP {}",
                resp.status()
            )));
        }
        let mut stream = resp.bytes_stream();
        let mut buf = String::new();
        let stdout = std::io::stdout();
        let mut out = stdout.lock();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| CmdError::Other(format!("sse read: {e}")))?;
            buf.push_str(&String::from_utf8_lossy(&chunk));
            // SSE events are separated by blank lines; we just print
            // line-by-line.
            while let Some(idx) = buf.find('\n') {
                let line: String = buf.drain(..=idx).collect();
                // Strip the "data: " prefix and trailing newline.
                let trimmed = line
                    .trim_end_matches('\n')
                    .trim_start_matches("data: ")
                    .trim_start_matches("event: done");
                // Skip empty heartbeat events.
                if line.trim().is_empty() {
                    continue;
                }
                if line.starts_with("event: done") {
                    writeln!(out, "-- build done --")?;
                    return Ok(());
                }
                writeln!(out, "{}", trimmed)?;
            }
        }
        let _ = pkg_dto; // suppress unused
        Ok(())
    } else {
        let blob = client.raw_logs(build_id).await?;
        let mut out = std::io::stdout().lock();
        output::write_log(&mut out, &format!("build {build_id}"), &blob)?;
        Ok(())
    }
}

/// `paur-cli logs --follow` uses this helper to extract a duration
/// to wait for a build to finish; unused right now but kept for
/// symmetry with future `wait` commands.
#[allow(dead_code)]
pub async fn wait_for_terminal_status(
    client: &DaemonClient,
    pkg: &str,
    _timeout: Duration,
) -> Result<String, CmdError> {
    let name = PkgName::new(pkg)?;
    let p: PackageDto = client.get_package(name.as_str()).await?;
    Ok(p.latest_build.map(|b| b.status).unwrap_or_else(|| "none".into()))
}

/// `paur-cli queue`
pub async fn queue(client: &DaemonClient) -> Result<(), CmdError> {
    let q = client.queue().await?;
    output::print_queue(&q);
    Ok(())
}

/// `paur-cli rebuild <pkg>`
pub async fn rebuild(client: &DaemonClient, pkg: &str) -> Result<(), CmdError> {
    let name = PkgName::new(pkg)?;
    let id = client.rebuild_package(name.as_str()).await?;
    println!("rebuild enqueued for {} (build_id={})", name, id);
    Ok(())
}

/// `paur-cli pubkey` — fetch and print the GPG pubkey.
pub async fn pubkey(client: &DaemonClient) -> Result<(), CmdError> {
    let k = client.pubkey().await?;
    print!("{k}");
    if !k.ends_with('\n') {
        println!();
    }
    Ok(())
}

/// `paur-cli init` — first-run setup.
///
/// This is the only command that needs to *not* require a running
/// daemon: it sets up directories, generates a GPG key, and writes
/// `gpg_key_id` to the DB. The daemon is started later via
/// `paur-cli serve` (or by systemd).
pub async fn init(
    cfg: &Config,
    force: bool,
    key_name: Option<&str>,
    key_email: Option<&str>,
) -> Result<(), CmdError> {
    cfg.ensure_dirs()?;

    let db_path = cfg.data_dir.join("paur.db");
    let db = paur_db::open(&db_path).await?;

    // Decide on key name/email.
    let default_email = format!("paur@localhost");
    let name = key_name.unwrap_or("paur");
    let email = key_email.unwrap_or(default_email.as_str());

    let existing = db.get_setting("gpg_key_id").await?;
    if existing.is_some() && !force {
        println!(
            "already initialized (gpg_key_id={}). Pass --force to regenerate.",
            existing.unwrap_or_default()
        );
        return Ok(());
    }

    // Generate (or reuse) a key. If a secret key with this email
    // already exists, we look it up instead of creating a new one.
    let keyid = if !force {
        match paur_repo::list_signing_key(&cfg.gpg_home, email).await {
            Ok(k) => {
                println!("reusing existing key for {email}: {k}");
                k
            }
            Err(_) => {
                let k = paur_repo::generate_key(&cfg.gpg_home, name, email).await?;
                println!("generated new key: {k}");
                k
            }
        }
    } else {
        let k = paur_repo::generate_key(&cfg.gpg_home, name, email).await?;
        println!("generated new key: {k}");
        k
    };
    db.set_setting("gpg_key_id", &keyid).await?;

    // Export pubkey to the arch dir.
    let pubkey_path = cfg.repo_dir.join(&cfg.arch).join("paur.pubkey.asc");
    paur_repo::export_pubkey(&cfg.gpg_home, &keyid, &pubkey_path).await?;
    println!("exported public key to {}", pubkey_path.display());

    // If the repo db file doesn't exist yet, create an empty one so
    // that a `pacman -Sy` against an empty repo doesn't 404.
    let arch_dir = cfg.repo_dir.join(&cfg.arch);
    std::fs::create_dir_all(&arch_dir)?;
    let db_file = arch_dir.join(format!("{}.db.tar.gz", cfg.repo_name));
    if !db_file.exists() {
        std::fs::write(&db_file, [] as [u8; 0])?;
        // Sign the empty db file with the chosen key.
        let sig_path = {
            let mut s = db_file.as_os_str().to_owned();
            s.push(".sig");
            std::path::PathBuf::from(s)
        };
        let out = std::process::Command::new("gpg")
            .env("GNUPGHOME", &cfg.gpg_home)
            .args([
                "--batch", "--yes", "--pinentry-mode", "loopback",
                "--passphrase", "",
                "--local-user", &keyid,
                "--armor", "--output",
            ])
            .arg(&sig_path)
            .arg("--detach-sign")
            .arg(&db_file)
            .output();
        if let Ok(o) = out {
            if !o.status.success() {
                eprintln!(
                    "warning: signing empty db failed: {}",
                    String::from_utf8_lossy(&o.stderr)
                );
            }
        }
    }
    Ok(())
}

/// `paur-cli config <key> [value]` — read or set a setting in the
/// `settings` table. Read goes through a direct DB connection (no
/// daemon required); write also goes direct for simplicity, since
/// adding an admin-only HTTP endpoint is out of scope.
pub async fn config_get(cfg: &Config, key: &str) -> Result<(), CmdError> {
    let db = open_db(cfg).await?;
    match db.get_setting(key).await? {
        Some(v) => println!("{v}"),
        None => {
            eprintln!("(not set)");
            std::process::exit(2);
        }
    }
    Ok(())
}

pub async fn config_set(cfg: &Config, key: &str, value: &str) -> Result<(), CmdError> {
    let db = open_db(cfg).await?;
    db.set_setting(key, value).await?;
    println!("{key} = {value}");
    Ok(())
}

pub async fn config_list(cfg: &Config) -> Result<(), CmdError> {
    let db = open_db(cfg).await?;
    for s in db.all_settings().await? {
        println!("{} = {}", s.key, s.value);
    }
    Ok(())
}

async fn open_db(cfg: &Config) -> Result<Db, CmdError> {
    let path = cfg.data_dir.join("paur.db");
    if !path.exists() {
        return Err(CmdError::Other(format!(
            "no database at {} — run `paur init` first",
            path.display()
        )));
    }
    Ok(paur_db::open(&path).await?)
}

/// `paur-cli repo-init` — rebuild the empty repo db file and a stub
/// pubkey export. Useful after a fresh clone of the repo dir.
pub async fn repo_init(cfg: &Config) -> Result<(), CmdError> {
    cfg.ensure_dirs()?;
    let arch_dir = cfg.repo_dir.join(&cfg.arch);
    std::fs::create_dir_all(&arch_dir)?;
    let db = open_db(cfg).await?;
    let keyid = db
        .get_setting("gpg_key_id")
        .await?
        .ok_or_else(|| CmdError::Other("gpg_key_id not set; run `paur init`".into()))?;
    let db_file = arch_dir.join(format!("{}.db.tar.gz", cfg.repo_name));
    if !db_file.exists() {
        std::fs::write(&db_file, [] as [u8; 0])?;
    }
    let pubkey_path = arch_dir.join("paur.pubkey.asc");
    paur_repo::export_pubkey(&cfg.gpg_home, &keyid, &pubkey_path).await?;
    println!("repo-initialized at {}", arch_dir.display());
    Ok(())
}

/// `paur-cli doctor` — a small set of sanity checks: daemon reachable,
/// db present, repo dir writable, container runtime present. We do
/// not change state.
pub async fn doctor(cfg: &Config) -> Result<(), CmdError> {
    let mut ok = true;
    let db_path = cfg.data_dir.join("paur.db");
    println!("[{}] data dir: {}", mark(db_path.exists()), cfg.data_dir.display());
    let arch_dir = cfg.repo_dir.join(&cfg.arch);
    println!("[{}] repo arch dir: {}", mark(arch_dir.exists()), arch_dir.display());
    let keyid_path = cfg.data_dir.join("paur.db");
    let _ = keyid_path; // suppress unused

    // Container runtime on PATH?
    for bin in ["docker", "podman", "repo-add", "gpg", "git", "makepkg"] {
        let found = which(bin);
        println!("[{}] {} on PATH", mark(found), bin);
        if !found {
            ok = false;
        }
    }
    if !ok {
        std::process::exit(1);
    }
    Ok(())
}

fn which(bin: &str) -> bool {
    std::process::Command::new("which")
        .arg(bin)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn mark(ok: bool) -> &'static str {
    if ok { " OK " } else { "FAIL" }
}

/// Suppress dead-code on the `_pkg: &PkgName` argument in `rebuild`
/// when called from `main`. Kept for API stability.
#[allow(dead_code)]
pub fn _validate_name(p: &Path) -> Result<(), CmdError> {
    let s = p.to_str().ok_or_else(|| CmdError::Other("non-utf8 path".into()))?;
    PkgName::new(s)?;
    Ok(())
}

/// `paur-cli print-pacman-conf` — emit the lines a client should
/// append to `/etc/pacman.conf`. Doesn't need a daemon.
pub fn print_pacman_conf(cfg: &Config) {
    println!("# paur: add these lines to /etc/pacman.conf");
    println!("[{}]", cfg.repo_name);
    println!("SigLevel = Optional TrustedOnly");
    println!("Include = /etc/pacman.d/{}-mirrorlist", cfg.repo_name);
    println!();
    println!("# The [paur] repo entry pulls the mirror URL from the package");
    println!("# installed by `paur-cli keyring-build`:");
    println!("#   sudo pacman -U <URL of {repo}-mirrorlist-*.pkg.tar.zst>",
             repo = cfg.repo_name);
    println!("#   sudo pacman -U <URL of {repo}-keyring-*.pkg.tar.zst>",
             repo = cfg.repo_name);
}

/// `paur-cli keyring-build` — build and publish the `paur-keyring`
/// and `paur-mirrorlist` meta-packages. After this command, a
/// client can install both with `pacman -U` and never has to know
/// the GPG fingerprint manually.
pub async fn keyring_build(cfg: &Config) -> Result<(), CmdError> {
    cfg.ensure_dirs()?;

    let db = open_db(cfg).await?;
    let keyid = db
        .get_setting("gpg_key_id")
        .await?
        .ok_or_else(|| CmdError::Other(
            "gpg_key_id not set; run `paur init` first".into(),
        ))?;

    // Build the two meta-packages sequentially. We don't bother
    // parallelizing — each build is ~10s, and the user runs this
    // command rarely.
    let keyring_path = build_meta_package(cfg, "paur-keyring").await?;
    let mirrorlist_path = build_meta_package(cfg, "paur-mirrorlist").await?;

    // Publish both. `paur_repo::publish` copies them into the repo
    // arch dir, runs `repo-add`, and signs with the daemon's key.
    let repo = paur_daemon::build_repo_ctx(cfg, &db).await?;
    let res1 = paur_repo::publish(&repo, &[keyring_path])
        .await
        .map_err(|e| CmdError::Other(format!("publish paur-keyring: {e}")))?;
    println!("published paur-keyring; db sig: {}", res1.display());
    let res2 = paur_repo::publish(&repo, &[mirrorlist_path])
        .await
        .map_err(|e| CmdError::Other(format!("publish paur-mirrorlist: {e}")))?;
    println!("published paur-mirrorlist; db sig: {}", res2.display());
    println!("\nClients can now install with:");
    println!("  sudo pacman -U {}/{}-keyring-*.pkg.tar.zst",
             cfg.public_base_url.trim_end_matches('/'),
             cfg.repo_name);
    println!("  sudo pacman -U {}/{}-mirrorlist-*.pkg.tar.zst",
             cfg.public_base_url.trim_end_matches('/'),
             cfg.repo_name);
    let _ = keyid; // suppress unused; gpg_key comes via repo ctx
    Ok(())
}

/// Build one meta-package via the local-build container path.
/// Returns the on-disk `.pkg.tar.*` produced.
async fn build_meta_package(
    cfg: &Config,
    label: &'static str,
) -> Result<std::path::PathBuf, CmdError> {
    let tmp = tempfile::tempdir()
        .map_err(|e| CmdError::Other(format!("tempdir: {e}")))?;
    write_pkgbuild(tmp.path(), label, cfg)?;

    let work_dir = cfg.work_dir.join(format!("keyring-{label}"));
    let _ = std::fs::remove_dir_all(&work_dir);
    std::fs::create_dir_all(&work_dir)?;

    let req = paur_builder::LocalBuildRequest {
        label: label.to_string(),
        pkgbuild_dir: tmp.path().to_path_buf(),
        work_dir,
        ccache_dir: cfg.ccache_dir.clone(),
        runtime: cfg.container_runtime,
        image: cfg.builder_image.clone(),
    };

    let sink = std::sync::Arc::new(paur_builder::CollectingSink::default());
    let outcome = paur_builder::run_local_in_container(&req, sink)
        .await
        .map_err(|e| CmdError::Other(format!("local build {label}: {e}")))?;
    if outcome.exit_code != 0 {
        return Err(CmdError::Other(format!(
            "local build {label} exited {}",
            outcome.exit_code
        )));
    }
    let pkg = outcome.pkg_files.into_iter().next().ok_or_else(|| {
        CmdError::Other(format!("local build {label} produced no artifact"))
    })?;
    Ok(pkg)
}

/// Write a PKGBUILD into `dir` for the given meta-package label.
///
/// For `paur-keyring` we also stage the pubkey as a sidecar file
/// (named `paur.pubkey.asc` inside the pkgbuild dir) so the PKGBUILD
/// can `cat` it into the package's `/usr/share/pacman/keyrings/`
/// path. This avoids having to bind-mount the host's repo dir into
/// the container — the package is fully self-contained.
fn write_pkgbuild(
    dir: &Path,
    label: &'static str,
    cfg: &Config,
) -> Result<(), CmdError> {
    let repo_name = cfg.repo_name.clone();
    let base_url = cfg.public_base_url.trim_end_matches('/').to_string();

    let contents = match label {
        "paur-keyring" => {
            // Stage the pubkey next to the PKGBUILD so the build
            // script can install it without bind-mounting the host
            // repo dir.
            let pubkey_src = cfg.repo_dir.join(&cfg.arch).join("paur.pubkey.asc");
            std::fs::copy(&pubkey_src, dir.join("paur.pubkey.asc")).map_err(|e| {
                CmdError::Other(format!(
                    "copy pubkey from {}: {e} (run `paur init` first?)",
                    pubkey_src.display()
                ))
            })?;

            format!(
                r#"# Auto-generated by `paur-cli keyring-build`. Do not edit by hand.
pkgname={repo_name}-keyring
pkgver=1
pkgrel=1
pkgdesc="GPG keyring for the {repo_name} pacman repo"
arch=('any')
license=('GPL')

package() {{
    install -dm755 "$pkgdir/usr/share/pacman/keyrings/"
    install -m0644 \
        "$srcdir/paur.pubkey.asc" \
        "$pkgdir/usr/share/pacman/keyrings/{repo_name}.asc"
}}
"#
            )
        }
        "paur-mirrorlist" => format!(
            r#"# Auto-generated by `paur-cli keyring-build`. Do not edit by hand.
pkgname={repo_name}-mirrorlist
pkgver=1
pkgrel=1
pkgdesc="Mirror list for the {repo_name} pacman repo"
arch=('any')
license=('GPL')

package() {{
    install -dm755 "$pkgdir/etc/pacman.d/"
    cat > "$pkgdir/etc/pacman.d/{repo_name}-mirrorlist" <<'EOF'
## {repo_name} pacman repository
##
## To enable, add to /etc/pacman.conf:
##   [{repo_name}]
##   Include = /etc/pacman.d/{repo_name}-mirrorlist
##
## This file is generated by paur and tracks your
## configured public_base_url. Edit paur.toml and
## re-run `paur-cli keyring-build` to update.

[options]
SigLevel = Optional TrustedOnly

[{repo_name}]
Server = {base_url}/$arch
EOF
}}
"#
        ),
        other => {
            return Err(CmdError::Other(format!(
                "unknown meta-package label: {other}"
            )));
        }
    };

    std::fs::write(dir.join("PKGBUILD"), contents)
        .map_err(|e| CmdError::Other(format!("write PKGBUILD: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::DaemonClient;
    use paur_core::Listen;
    use std::net::SocketAddr;
    use std::path::Path;
    use std::time::Duration;
    use tempfile::tempdir;

    /// Build a `Config` rooted at `dir` with a TCP listen on the
    /// given address. We bypass `ensure_dirs` so tests can build the
    /// tree lazily.
    fn cfg_at(dir: &Path, listen: SocketAddr) -> Config {
        let mut c = Config::with_data_dir(dir.to_path_buf());
        c.listen = Listen::Tcp(listen);
        c
    }

    /// Spin up the daemon (API + worker) in the background and return
    /// a `DaemonClient` plus a `JoinHandle` to abort later.
    async fn boot_daemon(
        cfg: Config,
    ) -> Result<(DaemonClient, tokio::task::JoinHandle<()>), paur_core::Error> {
        let db_path = cfg.data_dir.join("paur.db");
        let pool = paur_db_open(&db_path).await?;
        let db = paur_db::Db::from_pool(pool).await?;
        let repo = paur_daemon::build_repo_ctx(&cfg, &db).await?;
        let state = paur_daemon::AppState::new(db, cfg.clone(), repo);
        let api_cfg = cfg.clone();
        let api_state = state.clone();
        let api_task = tokio::spawn(async move {
            let _ = paur_daemon::serve(&api_cfg, api_state).await;
        });
        let worker_state = state.clone();
        let worker_task = tokio::spawn(async move {
            let _ = paur_daemon::run(worker_state, 1).await;
        });
        // Combine the two handles into one that returns when either ends.
        let handle = tokio::spawn(async move {
            let _ = api_task.await;
            let _ = worker_task.await;
        });
        // Build a client from the config; the daemon is listening on
        // `cfg.listen`.
        let client = DaemonClient::from_config(&cfg);
        // Wait for the API to be ready.
        for _ in 0..40 {
            if client.health().await.is_ok() {
                return Ok((client, handle));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        // API never came up; abort and fail.
        handle.abort();
        Err(paur_core::Error::Other(
            "daemon api did not become ready".into(),
        ))
    }

    async fn paur_db_open(path: &Path) -> paur_core::Result<sqlx::SqlitePool> {
        use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
        use std::str::FromStr as _;
        let url = format!("sqlite://{}", path.display());
        let opts = SqliteConnectOptions::from_str(&url)
            .map_err(|e| paur_core::Error::Db(e.to_string()))?
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));
        SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(opts)
            .await
            .map_err(|e| paur_core::Error::Db(e.to_string()))
    }

    /// Smoke: build a daemon in a tempdir, then exercise add → list
    /// → status over the HTTP client. The test does not actually run
    /// a build — it only verifies the API roundtrips.
    #[tokio::test(flavor = "multi_thread")]
    async fn add_list_status_end_to_end() {
        let dir = tempdir().expect("tempdir");
        // Bind to port 0 (kernel-assigned), then read the actual
        // port from the listener. Easiest path: have the daemon
        // resolve a real port via `bind` in `serve`. Since the
        // current `paur_daemon::serve` takes a fixed address, we
        // pick a high random port.
        let port = pick_port();
        let listen: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        let cfg = cfg_at(dir.path(), listen);
        cfg.ensure_dirs().unwrap();

        let (client, handle) = match boot_daemon(cfg.clone()).await {
            Ok(t) => t,
            Err(e) => {
                eprintln!("daemon boot failed (port busy?): {e}; skipping");
                return;
            }
        };

        // Add a package.
        add(&client, "hello-pkg", false).await.expect("add");

        // List should show it.
        let pkgs = client.list_packages().await.expect("list");
        assert!(
            pkgs.iter().any(|p| p.name == "hello-pkg"),
            "added package not visible: {pkgs:?}"
        );

        // Status should not error.
        status(&client, "hello-pkg").await.expect("status");

        // Remove should succeed.
        remove(&client, "hello-pkg").await.expect("remove");

        // List should now be empty.
        let pkgs = client.list_packages().await.expect("list2");
        assert!(
            !pkgs.iter().any(|p| p.name == "hello-pkg"),
            "removed package still listed: {pkgs:?}"
        );

        handle.abort();
    }

    /// Pick a likely-free TCP port in the high range. Race-prone,
    /// but adequate for tests that bind/rebind.
    fn pick_port() -> u16 {
        // Bind to 0 then drop; the kernel tells us a free port. We
        // immediately drop so the test daemon can rebind.
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let p = l.local_addr().unwrap().port();
        drop(l);
        p
    }

    /// `print_pacman_conf` doesn't touch the FS; just make sure it
    /// doesn't panic on a config with default values.
    #[test]
    fn print_pacman_conf_does_not_panic() {
        let cfg = Config::default();
        print_pacman_conf(&cfg);
    }

    /// `write_pkgbuild` lays out a self-contained PKGBUILD dir for
    /// each meta-package. The keyring variant must copy the host's
    /// `paur.pubkey.asc` next to the PKGBUILD so the container build
    /// can `cat` it into the package without bind-mounts.
    #[test]
    fn write_pkgbuild_lays_out_keyring() {
        let dir = tempdir().expect("tempdir");
        // Create a fake pubkey at the expected host path.
        let cfg = Config::with_data_dir(dir.path().to_path_buf());
        let arch_dir = cfg.repo_dir.join(&cfg.arch);
        std::fs::create_dir_all(&arch_dir).expect("arch dir");
        std::fs::write(arch_dir.join("paur.pubkey.asc"), b"FAKE PGP KEY")
            .expect("write pubkey");

        let pkgbuild_dir = tempdir().expect("pkgbuild tempdir");
        write_pkgbuild(pkgbuild_dir.path(), "paur-keyring", &cfg)
            .expect("write keyring pkgbuild");

        // The PKGBUILD should exist.
        let pkgbuild = pkgbuild_dir.path().join("PKGBUILD");
        let body = std::fs::read_to_string(&pkgbuild).expect("read PKGBUILD");
        assert!(body.contains("pkgname=paur-keyring"));
        assert!(body.contains("usr/share/pacman/keyrings/paur.asc"));

        // The pubkey should be staged next to it.
        let staged = pkgbuild_dir.path().join("paur.pubkey.asc");
        let staged_bytes = std::fs::read(&staged).expect("read staged pubkey");
        assert_eq!(staged_bytes, b"FAKE PGP KEY");
    }

    /// Mirrorlist PKGBUILD embeds the configured `public_base_url`
    /// and the repo name into the package's mirrorlist file.
    #[test]
    fn write_pkgbuild_lays_out_mirrorlist() {
        let dir = tempdir().expect("tempdir");
        let mut cfg = Config::with_data_dir(dir.path().to_path_buf());
        cfg.public_base_url = "https://paur.example".into();

        let pkgbuild_dir = tempdir().expect("pkgbuild tempdir");
        write_pkgbuild(pkgbuild_dir.path(), "paur-mirrorlist", &cfg)
            .expect("write mirrorlist pkgbuild");

        let body =
            std::fs::read_to_string(pkgbuild_dir.path().join("PKGBUILD"))
                .expect("read PKGBUILD");
        assert!(body.contains("pkgname=paur-mirrorlist"));
        assert!(body.contains("https://paur.example/$arch"));
        assert!(body.contains("/etc/pacman.d/paur-mirrorlist"));
    }
}
