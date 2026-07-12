//! Thin HTTP client wrapping the daemon's `/api/v1/*` endpoints.
//!
//! The CLI never speaks SQLite directly: it goes through the daemon
//! when one is reachable. For commands that need DB writes (add,
//! rebuild, delete) we *require* the daemon; for read-only commands
//! (list, status) we can fall back to a direct DB read if no daemon
//! is listening, but that's currently only useful for debugging.
//!
//! The connection target is derived from `paur_core::Config::listen`.
//! axum 0.7's `serve` only takes a `TcpListener`, so in practice
//! `Listen::Unix` falls back to `127.0.0.1:7300` inside the daemon
//! itself. We mirror that here: if the config says Unix, we assume
//! the daemon is reachable on the well-known fallback port.

use std::time::Duration;

use reqwest::{Client, StatusCode};
use serde::de::DeserializeOwned;

use paur_core::{Config, PackageVariants, Variant};

/// Default TCP fallback the daemon uses when `listen = "unix ..."`.
const UNIX_FALLBACK_HOSTPORT: &str = "http://127.0.0.1:7300";

/// HTTP error returned by the daemon's API (non-2xx with JSON body).
#[derive(Debug, Clone)]
pub struct ApiHttpError {
    pub status: StatusCode,
    pub message: String,
}

impl std::fmt::Display for ApiHttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HTTP {}: {}", self.status, self.message)
    }
}

impl std::error::Error for ApiHttpError {}

/// Wrapper around a `reqwest::Client` targeting the daemon.
#[derive(Debug, Clone)]
pub struct DaemonClient {
    base: String,
    http: Client,
}

impl DaemonClient {
    /// Build a client from the resolved `Config`. Reads the listen
    /// address to pick the right base URL.
    pub fn from_config(cfg: &Config) -> Self {
        let base = match &cfg.listen {
            paur_core::Listen::Tcp(addr) => format!("http://{addr}"),
            // Mirror the daemon's unix→TCP fallback. The unix socket
            // path is not directly used; if the daemon is on a unix
            // socket, the user must also be on the same host, and
            // would be talking HTTP over the loopback fallback.
            paur_core::Listen::Unix(_) => UNIX_FALLBACK_HOSTPORT.to_string(),
        };
        Self::new(base)
    }

    /// Build a client from a literal base URL (e.g. `http://host:7300`).
    pub fn new(base: impl Into<String>) -> Self {
        let base = base.into().trim_end_matches('/').to_string();
        let http = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest client builds");
        Self { base, http }
    }

    /// The base URL this client targets. Used by the SSE follow path.
    pub fn base_url(&self) -> &str {
        &self.base
    }

    /// `GET /api/v1/health` — quick liveness probe.
    pub async fn health(&self) -> Result<String, ClientError> {
        let url = format!("{}/api/v1/health", self.base);
        let resp = self.http.get(&url).send().await?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if status.is_success() {
            Ok(text)
        } else {
            Err(ClientError::Api(ApiHttpError { status, message: text }))
        }
    }

    /// `GET /api/v1/packages` — list all packages.
    pub async fn list_packages(&self) -> Result<Vec<PackageDto>, ClientError> {
        self.get_json("/api/v1/packages").await
    }

    /// `POST /api/v1/packages` — add a package. Returns the new dto.
    ///
    /// `variants` is the set of variants to enable at add time. The
    /// daemon forces `default` on regardless of what's passed, so
    /// the caller may omit it. An empty slice means "default only".
    pub async fn add_package(
        &self,
        name: &str,
        auto_rebuild: bool,
        variants: &[Variant],
    ) -> Result<PackageDto, ClientError> {
        let url = format!("{}/api/v1/packages", self.base);
        let variant_strs: Vec<&'static str> =
            variants.iter().map(|v| v.as_str()).collect();
        let resp = self
            .http
            .post(&url)
            .json(&serde_json::json!({
                "name": name,
                "auto_rebuild": auto_rebuild,
                "variants": variant_strs,
            }))
            .send()
            .await?;
        self.parse(resp).await
    }

    /// `PATCH /api/v1/packages/:name/variants` — replace the
    /// active variant set for a package. The daemon forces
    /// `default` on regardless of what's passed, so the caller
    /// may omit it.
    pub async fn set_variants(
        &self,
        name: &str,
        variants: &[Variant],
    ) -> Result<PackageDto, ClientError> {
        let url = format!("{}/api/v1/packages/{}/variants", self.base, name);
        let variant_strs: Vec<&'static str> =
            variants.iter().map(|v| v.as_str()).collect();
        let resp = self
            .http
            .patch(&url)
            .json(&serde_json::json!({ "variants": variant_strs }))
            .send()
            .await?;
        self.parse(resp).await
    }

    /// `DELETE /api/v1/packages/:name` — remove a package.
    pub async fn delete_package(&self, name: &str) -> Result<(), ClientError> {
        let url = format!("{}/api/v1/packages/{}", self.base, name);
        let resp = self.http.delete(&url).send().await?;
        let status = resp.status();
        if status.is_success() || status == StatusCode::NO_CONTENT {
            Ok(())
        } else {
            let msg = resp.text().await.unwrap_or_default();
            Err(ClientError::Api(ApiHttpError { status, message: msg }))
        }
    }

    /// `GET /api/v1/packages/:name` — fetch a single package.
    pub async fn get_package(&self, name: &str) -> Result<PackageDto, ClientError> {
        self.get_json(&format!("/api/v1/packages/{name}")).await
    }

    /// `POST /api/v1/packages/:name/rebuild` — enqueue a rebuild.
    pub async fn rebuild_package(&self, name: &str) -> Result<i64, ClientError> {
        let url = format!("{}/api/v1/packages/{}/rebuild", self.base, name);
        let resp = self.http.post(&url).send().await?;
        let v: serde_json::Value = self.parse(resp).await?;
        v.get("build_id")
            .and_then(|x| x.as_i64())
            .ok_or_else(|| ClientError::Other("missing build_id in response".into()))
    }

    /// `PATCH /api/v1/packages/:name/flags` — set per-package build
    /// tuning flags. `flags` is serialized as JSON; any field set to
    /// `true` becomes active on the next build, and the daemon
    /// composes with the existing flags (existing `true` fields are
    /// not cleared by a PATCH).
    pub async fn set_build_flags(
        &self,
        name: &str,
        flags: &paur_core::PackageBuildFlags,
    ) -> Result<PackageDto, ClientError> {
        let url = format!("{}/api/v1/packages/{}/flags", self.base, name);
        let body = serde_json::to_string(flags)
            .map_err(|e| ClientError::Other(format!("serialize flags: {e}")))?;
        let resp = self
            .http
            .patch(&url)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await?;
        self.parse(resp).await
    }

    /// `GET /api/v1/builds` — list recent builds.
    pub async fn list_builds(
        &self,
        pkg: Option<&str>,
        status: Option<&str>,
        limit: Option<i64>,
    ) -> Result<Vec<BuildDto>, ClientError> {
        let mut url = format!("{}/api/v1/builds", self.base);
        let mut q: Vec<(&str, String)> = Vec::new();
        if let Some(p) = pkg {
            q.push(("pkg", p.to_string()));
        }
        if let Some(s) = status {
            q.push(("status", s.to_string()));
        }
        if let Some(l) = limit {
            q.push(("limit", l.to_string()));
        }
        if !q.is_empty() {
            url.push('?');
            url.push_str(
                &q.iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join("&"),
            );
        }
        let resp = self.http.get(&url).send().await?;
        self.parse(resp).await
    }

    /// `GET /api/v1/builds/:id` — fetch a single build.
    pub async fn get_build(&self, id: i64) -> Result<BuildDto, ClientError> {
        self.get_json(&format!("/api/v1/builds/{id}")).await
    }

    /// `GET /api/v1/queue` — current queue + running.
    pub async fn queue(&self) -> Result<QueueDto, ClientError> {
        self.get_json("/api/v1/queue").await
    }

    /// `GET /api/v1/builds/:id/logs.txt` — full log as text.
    pub async fn raw_logs(&self, id: i64) -> Result<String, ClientError> {
        let url = format!("{}/api/v1/builds/{id}/logs.txt", self.base);
        let resp = self.http.get(&url).send().await?;
        let status = resp.status();
        if status.is_success() {
            Ok(resp.text().await?)
        } else {
            let msg = resp.text().await.unwrap_or_default();
            Err(ClientError::Api(ApiHttpError { status, message: msg }))
        }
    }

    /// `GET /api/v1/pubkey` — raw armored GPG public key.
    pub async fn pubkey(&self) -> Result<String, ClientError> {
        let url = format!("{}/api/v1/pubkey", self.base);
        let resp = self.http.get(&url).send().await?;
        let status = resp.status();
        if status.is_success() {
            Ok(resp.text().await?)
        } else {
            let msg = resp.text().await.unwrap_or_default();
            Err(ClientError::Api(ApiHttpError { status, message: msg }))
        }
    }

    // -------- helpers --------

    async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, ClientError> {
        let url = format!("{}{}", self.base, path);
        let resp = self.http.get(&url).send().await?;
        self.parse(resp).await
    }

    async fn parse<T: DeserializeOwned>(
        &self,
        resp: reqwest::Response,
    ) -> Result<T, ClientError> {
        let status = resp.status();
        if status.is_success() {
            Ok(resp.json().await?)
        } else {
            let msg = resp.text().await.unwrap_or_default();
            Err(ClientError::Api(ApiHttpError { status, message: msg }))
        }
    }
}

/// Package DTO mirroring the daemon's response.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct PackageDto {
    pub id: i64,
    pub name: String,
    pub aur_url: String,
    pub last_known_ref: Option<String>,
    pub auto_rebuild: bool,
    pub latest_build: Option<LatestBuildDto>,
    /// Per-package build tuning flags. Defaults to all-false when
    /// the daemon predates the flags migration.
    #[serde(default)]
    pub build_flags: paur_core::PackageBuildFlags,
    /// Per-package active variant set. Default = `default` only.
    /// Older daemons that predate the variants migration omit
    /// this; the field defaults to `PackageVariants::default()`
    /// so `flag --list` etc. don't blow up on missing data.
    #[serde(default)]
    pub variants: PackageVariants,
}

/// Latest-build summary embedded in a package.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct LatestBuildDto {
    pub id: i64,
    pub status: String,
    pub pkg_version: Option<String>,
    pub finished_at: Option<i64>,
    pub exit_code: Option<i64>,
    /// Which variant this build produced. "default" for builds
    /// predating the variants migration.
    #[serde(default = "default_build_variant")]
    pub variant: String,
}

fn default_build_variant() -> String {
    "default".to_string()
}

/// Build DTO mirroring the daemon's response.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct BuildDto {
    pub id: i64,
    pub package_id: i64,
    pub status: String,
    pub queued_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub exit_code: Option<i64>,
    pub pkg_file: Option<String>,
    pub pkg_version: Option<String>,
    pub worker_id: Option<String>,
    pub trigger: String,
    /// Which variant this build produced. "default" for builds
    /// predating the variants migration.
    #[serde(default = "default_build_variant")]
    pub variant: String,
}

/// Queue DTO from `/api/v1/queue`.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct QueueDto {
    pub queued: Vec<BuildDto>,
    pub running: Vec<BuildDto>,
}

/// Errors from the HTTP client.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("api: {0}")]
    Api(#[from] ApiHttpError),
    #[error("{0}")]
    Other(String),
}
