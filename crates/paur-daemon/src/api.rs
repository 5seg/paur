//! HTTP API for the paur daemon.
//!
//! All routes are mounted under `/api/v1`. We expose the same surface
//! the Web UI consumes plus a few read-only helpers (health,
//! install-time FPR lookup).
//!
//! Listen mode is taken from the daemon [`Config`]. A unix socket is
//! the default; `0.0.0.0:port` is supported too — see
//! [`paur_core::Listen`].

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{
    sse::{Event, KeepAlive, Sse},
    IntoResponse, Response,
};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use futures::stream::StreamExt as _;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;

use crate::auth::{self, Admin};
use crate::worker::AppState;
use paur_core::Variant;

/// Build the [`Router`] for the API. Caller is responsible for
/// `serve`-ing it onto the appropriate listener.
pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/packages", get(list_packages).post(add_package))
        .route("/api/v1/packages/:name", get(get_package).delete(delete_package))
        .route("/api/v1/packages/:name/rebuild", post(rebuild_package))
        .route("/api/v1/packages/:name/auto", patch(set_package_auto))
        .route("/api/v1/packages/:name/flags", patch(set_package_flags))
        .route("/api/v1/packages/:name/variants", patch(set_package_variants))
        .route("/api/v1/builds", get(list_builds))
        .route("/api/v1/builds/:id", get(get_build))
        .route("/api/v1/builds/:id/cancel", post(cancel_build))
        .route("/api/v1/builds/:id/logs", get(stream_logs))
        .route("/api/v1/builds/:id/logs.txt", get(raw_logs))
        .route("/api/v1/queue", get(queue))
        .route("/api/v1/install/fpr", get(install_fpr))
        .route("/api/v1/repo/:arch/*file", get(serve_repo_file))
        .merge(auth::router());
    api.with_state(Arc::new(state))
}

/// Serve the API on the configured listener. Returns when the listener
/// closes (e.g. process shutdown).
///
/// Note: axum 0.7's `serve` only takes a `TcpListener`, so a
/// `Listen::Unix` config is translated to a loopback TCP port
/// (suffixed with a high random number) at serve time. In practice
/// this means the unix-socket config is a *placeholder*: real
/// deployments should set `listen = "127.0.0.1:7300"` (or similar)
/// in `config.toml`.
pub async fn serve(cfg: &paur_core::Config, state: AppState) -> Result<(), paur_core::Error> {
    let app = router(state);
    let addr = match &cfg.listen {
        paur_core::Listen::Tcp(addr) => *addr,
        paur_core::Listen::Unix(_) => {
            tracing::warn!(
                "Unix sockets are not directly supported by axum 0.7; \
                 falling back to 127.0.0.1:7300. Set listen = \"127.0.0.1:port\" \
                 in config.toml to silence this."
            );
            "127.0.0.1:7300".parse().expect("static addr is valid")
        }
    };
    let listener = TcpListener::bind(addr).await.map_err(paur_core::Error::Io)?;
    tracing::info!(%addr, "paur: HTTP API listening on TCP");
    // `into_make_service_with_connect_info` exposes the peer
    // `SocketAddr` to extractors so `auth::Admin` can skip the
    // session check for loopback callers (the `paur-cli` running on
    // the build host). Without this, every internal call from the
    // CLI would 401 because there's no cookie store.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await
    .map_err(paur_core::Error::Io)
}

// -------- error helper --------

struct ApiError(paur_core::Error);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self.0 {
            paur_core::Error::NotFound(_) => StatusCode::NOT_FOUND,
            paur_core::Error::Invalid(_) | paur_core::Error::InvalidName(..) => {
                StatusCode::BAD_REQUEST
            }
            paur_core::Error::Conflict(_) => StatusCode::CONFLICT,
            paur_core::Error::MissingDep(_) => StatusCode::NOT_IMPLEMENTED,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let body = serde_json::json!({ "error": self.0.to_string() });
        (status, Json(body)).into_response()
    }
}

impl From<paur_core::Error> for ApiError {
    fn from(e: paur_core::Error) -> Self {
        ApiError(e)
    }
}

type ApiResult<T> = std::result::Result<T, ApiError>;

// -------- handlers --------

async fn health() -> &'static str {
    "ok"
}

#[derive(Serialize)]
struct PackageDto {
    id: i64,
    name: String,
    aur_url: String,
    last_known_ref: Option<String>,
    auto_rebuild: bool,
    /// Most recent build of any variant. Kept for back-compat with
    /// existing UI code paths that just want "is something
    /// running / latest status". Per-variant latests live in
    /// `latest_builds`.
    latest_build: Option<LatestBuildDto>,
    /// Most recent build *per variant*. Empty for variants that
    /// have never been built. Used by the UI to render the variant
    /// chips in the package list and detail page.
    #[serde(default)]
    latest_builds: Vec<LatestBuildDto>,
    /// Per-package build tuning flags (memory/CPU OOM countermeasures).
    /// Defaults to all-false for clients that predate the field.
    #[serde(default)]
    build_flags: paur_core::PackageBuildFlags,
    /// Which compiled variants this package is built for. The
    /// `default` field is always `true` (the daemon's invariant).
    /// The UI uses this both to render the variant chip column on
    /// the package list and to draw the variant toggle group on
    /// the detail page.
    #[serde(default)]
    variants: paur_core::PackageVariants,
}

impl PackageDto {
    /// Build a `PackageDto` from a `Package` + its latest builds
    /// (any-variant + per-variant). Used by every read/write
    /// endpoint that returns package data so the serialization
    /// shape stays consistent.
    async fn from_pkg(
        db: &paur_db::Db,
        pkg: paur_db::Package,
    ) -> Result<Self, paur_core::Error> {
        let latest_any = db.latest_build_for(pkg.id).await?;
        let mut latest_builds: Vec<LatestBuildDto> = Vec::new();
        for v in Variant::all() {
            if let Some(b) = db.latest_build_for_variant(pkg.id, *v).await? {
                latest_builds.push(LatestBuildDto {
                    id: b.id,
                    seq: b.seq,
                    status: b.status.as_str().to_string(),
                    pkg_version: b.pkg_version,
                    finished_at: b.finished_at,
                    exit_code: b.exit_code,
                    variant: b.variant,
                });
            }
        }
        Ok(Self {
            id: pkg.id,
            name: pkg.name,
            aur_url: pkg.aur_url,
            last_known_ref: pkg.last_known_ref,
            auto_rebuild: pkg.auto_rebuild,
            latest_build: latest_any.map(|b| LatestBuildDto {
                id: b.id,
                seq: b.seq,
                status: b.status.as_str().to_string(),
                pkg_version: b.pkg_version,
                finished_at: b.finished_at,
                exit_code: b.exit_code,
                variant: b.variant,
            }),
            latest_builds,
            build_flags: pkg.build_flags,
            variants: pkg.variants,
        })
    }
}

#[derive(Serialize)]
struct LatestBuildDto {
    id: i64,
    /// Per-package sequence number (1-based). Use this for the
    /// human-friendly "build #N" label; `id` is the global primary
    /// key and is what other resources (logs, repo) key on.
    seq: i64,
    status: String,
    pkg_version: Option<String>,
    finished_at: Option<i64>,
    exit_code: Option<i64>,
    /// Which variant this build is for. Always present on
    /// responses from this daemon; older clients that don't
    /// know about the field simply ignore it.
    #[serde(default = "default_variant_tag")]
    variant: String,
}

#[allow(dead_code)] // wired via serde default path
fn default_variant_tag() -> String {
    "default".into()
}

async fn list_packages(
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<Vec<PackageDto>>> {
    let pkgs = state.db.list_packages().await?;
    let mut out = Vec::with_capacity(pkgs.len());
    for p in pkgs {
        out.push(PackageDto::from_pkg(&state.db, p).await?);
    }
    Ok(Json(out))
}

#[derive(Deserialize)]
struct AddPackageBody {
    name: String,
    #[serde(default)]
    auto_rebuild: bool,
    /// Variants to enable for the new package. `default` is
    /// always included automatically (the daemon's invariant),
    /// so the user only needs to opt in to `v3` / `v4`.
    /// `None` / empty means "default only".
    #[serde(default)]
    variants: Vec<String>,
}

/// Resolve a `variants` field from the wire (e.g. `["v3","v4"]`)
/// into a `PackageVariants`. Unknown tags are rejected with a
/// 400-equivalent error string so the client can fix them; `default`
/// is always added back even if the user tried to leave it off.
fn parse_variants_field(raw: &[String]) -> Result<paur_core::PackageVariants, String> {
    let mut out = paur_core::PackageVariants::default();
    for s in raw {
        let v = Variant::parse(s)
            .ok_or_else(|| format!("unknown variant: {s:?} (expected default|v3|v4)"))?;
        out.turn_on(v);
    }
    Ok(out)
}

/// Enqueue one build per active variant in canonical build order
/// (`default` → `v3` → `v4`). One worker means FIFO ordering is
/// preserved, so the package is built for each enabled variant
/// before any other queued work.
async fn enqueue_all_variants(
    db: &paur_db::Db,
    state: &AppState,
    pkg_id: i64,
    variants: &paur_core::PackageVariants,
    trigger: paur_db::BuildTrigger,
) -> paur_core::Result<()> {
    let mut first_id: Option<i64> = None;
    for v in variants.active() {
        let id = db.enqueue_build(pkg_id, trigger, v).await?;
        if first_id.is_none() {
            first_id = Some(id);
        }
    }
    if let Some(id) = first_id {
        state.send_wake(id).await;
    }
    Ok(())
}

async fn add_package(
    State(state): State<Arc<AppState>>,
    _admin: Admin,
    Json(body): Json<AddPackageBody>,
) -> ApiResult<(StatusCode, Json<PackageDto>)> {
    let name = paur_core::PkgName::new(&body.name)
        .map_err(|e| ApiError(paur_core::Error::Invalid(e.to_string())))?;
    let url = paur_aur::aur_url(&name);
    let id = state
        .db
        .upsert_package(name.as_str(), &url, body.auto_rebuild)
        .await?;
    let variants = parse_variants_field(&body.variants)
        .map_err(|e| ApiError(paur_core::Error::Invalid(e)))?;
    // Persist the variant selection and enqueue the first build
    // (for the default variant). When the user later enables v3
    // / v4 via the variants PATCH endpoint, that handler enqueues
    // the additional builds.
    let _ = state.db.set_variants(name.as_str(), &variants).await?;
    let pkg = state
        .db
        .get_package_by_name(name.as_str())
        .await?
        .ok_or_else(|| ApiError(paur_core::Error::NotFound("package vanished".into())))?;
    enqueue_all_variants(&state.db, &state, id, &pkg.variants, paur_db::BuildTrigger::Manual).await?;
    tracing::info!(pkg = %name, "package added/enqueued via API");
    Ok((StatusCode::CREATED, Json(PackageDto::from_pkg(&state.db, pkg).await?)))
}

async fn get_package(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> ApiResult<Json<PackageDto>> {
    let p = state
        .db
        .get_package_by_name(&name)
        .await?
        .ok_or_else(|| ApiError(paur_core::Error::NotFound(name.clone())))?;
    Ok(Json(PackageDto::from_pkg(&state.db, p).await?))
}

#[derive(Deserialize)]
struct SetAutoBody {
    auto_rebuild: bool,
}

/// Toggle the per-package `auto_rebuild` flag. Admin-only: we
/// don't want anonymous visitors to flip poller behavior for our
/// tracked packages.
async fn set_package_auto(
    State(state): State<Arc<AppState>>,
    _admin: Admin,
    Path(name): Path<String>,
    Json(body): Json<SetAutoBody>,
) -> ApiResult<Json<PackageDto>> {
    let n = state.db.set_auto_rebuild(&name, body.auto_rebuild).await?;
    if n == 0 {
        return Err(ApiError(paur_core::Error::NotFound(name.clone())));
    }
    // Return the fresh DTO so the UI can re-render without a
    // second round-trip.
    let p = state
        .db
        .get_package_by_name(&name)
        .await?
        .ok_or_else(|| ApiError(paur_core::Error::NotFound(name.clone())))?;
    Ok(Json(PackageDto::from_pkg(&state.db, p).await?))
}

/// `PATCH /api/v1/packages/:name/flags` — set per-package build
/// tuning flags. Admin-only: changing build behavior for tracked
/// packages should require a deliberate action.
///
/// The body is a `PackageBuildFlags` JSON object that describes
/// the *full desired state*. Every field is read; sending `false`
/// turns a flag off. The UI sends the full object on each toggle
/// so the server never has to know which key was clicked. An
/// `Option<PackageBuildFlags>` (true = clear, false = set) shape
/// would be more efficient on the wire, but a full-state write
/// keeps the API stateless and trivially correct.
async fn set_package_flags(
    State(state): State<Arc<AppState>>,
    _admin: Admin,
    Path(name): Path<String>,
    Json(desired): Json<paur_core::PackageBuildFlags>,
) -> ApiResult<Json<PackageDto>> {
    // We read the current row only to make sure the package
    // exists; the write itself fully overwrites the flags column.
    let cur = state
        .db
        .get_package_by_name(&name)
        .await?
        .ok_or_else(|| ApiError(paur_core::Error::NotFound(name.clone())))?;
    let mut next = cur.build_flags.clone();
    next.replace_from(&desired);
    let n = state.db.set_build_flags(&name, &next).await?;
    if n == 0 {
        return Err(ApiError(paur_core::Error::NotFound(name.clone())));
    }
    tracing::info!(pkg = %name, ?next, "build_flags updated via API");
    let p = state
        .db
        .get_package_by_name(&name)
        .await?
        .ok_or_else(|| ApiError(paur_core::Error::NotFound(name.clone())))?;
    Ok(Json(PackageDto::from_pkg(&state.db, p).await?))
}

/// `PATCH /api/v1/packages/:name/variants` — change the set of
/// active variants for a package. Admin-only. Each newly-enabled
/// variant (compared to the previous set) gets a fresh build
/// enqueued in build order, so the operator can flip `v3` on and
/// watch the queue start working without further input.
async fn set_package_variants(
    State(state): State<Arc<AppState>>,
    _admin: Admin,
    Path(name): Path<String>,
    Json(body): Json<AddPackageBody>,
) -> ApiResult<Json<PackageDto>> {
    // Re-use the AddPackageBody deserializer: it has the same
    // shape (name + variants). The `name` field is ignored
    // because the URL path is the source of truth; the field is
    // present only for symmetry with `add_package`.
    let _ = body.name;
    let desired = parse_variants_field(&body.variants)
        .map_err(|e| ApiError(paur_core::Error::Invalid(e)))?;
    let cur = state
        .db
        .get_package_by_name(&name)
        .await?
        .ok_or_else(|| ApiError(paur_core::Error::NotFound(name.clone())))?;
    let n = state.db.set_variants(&name, &desired).await?;
    if n == 0 {
        return Err(ApiError(paur_core::Error::NotFound(name.clone())));
    }
    // Enqueue builds for any variant that the user just enabled
    // (i.e. is in `desired` but wasn't in the previous set). We
    // don't re-enqueue builds for variants that were already on
    // — the user can use `rebuild` for that.
    for v in desired.active() {
        if !cur.variants.is_active(v) {
            let id = state
                .db
                .enqueue_build(cur.id, paur_db::BuildTrigger::Manual, v)
                .await?;
            state.send_wake(id).await;
        }
    }
    tracing::info!(pkg = %name, ?desired, "variants updated via API");
    let p = state
        .db
        .get_package_by_name(&name)
        .await?
        .ok_or_else(|| ApiError(paur_core::Error::NotFound(name.clone())))?;
    Ok(Json(PackageDto::from_pkg(&state.db, p).await?))
}

async fn delete_package(
    State(state): State<Arc<AppState>>,
    _admin: Admin,
    Path(name): Path<String>,
) -> ApiResult<StatusCode> {
    let p = state
        .db
        .get_package_by_name(&name)
        .await?
        .ok_or_else(|| ApiError(paur_core::Error::NotFound(name.clone())))?;
    // Remove from every variant's repo first so the next
    // `pacman -Sy` won't see the package in any of the three
    // arch subdirs. Failures are non-fatal: we still drop the
    // package row so the daemon forgets it, and the leftover
    // .pkg.tar.* in the repo dir can be cleaned up by hand.
    let pkg_name = paur_core::PkgName::new(&p.name)
        .map_err(|e| ApiError(paur_core::Error::Invalid(e.to_string())))?;
    for v in Variant::all() {
        if let Err(e) = paur_repo::remove(&state.repo, &pkg_name, *v).await {
            tracing::warn!(pkg = %pkg_name, variant = v.as_str(), "repo-remove failed (continuing): {e}");
        }
    }
    let n = state.db.delete_package(&p.name).await?;
    if n == 0 {
        return Ok(StatusCode::NOT_FOUND);
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn rebuild_package(
    State(state): State<Arc<AppState>>,
    _admin: Admin,
    Path(name): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let p = state
        .db
        .get_package_by_name(&name)
        .await?
        .ok_or_else(|| ApiError(paur_core::Error::NotFound(name.clone())))?;
    // Enqueue one build per active variant. The worker processes
    // the queue serially, so the default → v3 → v4 order is
    // preserved.
    enqueue_all_variants(
        &state.db,
        &state,
        p.id,
        &p.variants,
        paur_db::BuildTrigger::Rebuild,
    )
    .await?;
    Ok(Json(serde_json::json!({ "queued": p.variants.active().len() })))
}

#[derive(Deserialize)]
struct ListBuildsQuery {
    pkg: Option<String>,
    status: Option<String>,
    variant: Option<String>,
    limit: Option<i64>,
}

async fn list_builds(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ListBuildsQuery>,
) -> ApiResult<Json<Vec<paur_db::Build>>> {
    let status = match q.status.as_deref() {
        Some("queued") => Some(paur_db::BuildStatus::Queued),
        Some("running") => Some(paur_db::BuildStatus::Running),
        Some("success") => Some(paur_db::BuildStatus::Success),
        Some("failed") => Some(paur_db::BuildStatus::Failed),
        Some("cancelled") => Some(paur_db::BuildStatus::Cancelled),
        Some(other) => {
            return Err(ApiError(paur_core::Error::Invalid(format!(
                "unknown status: {other}"
            ))))
        }
        None => None,
    };
    let variant = match q.variant.as_deref() {
        Some(v) => Some(
            Variant::parse(v)
                .ok_or_else(|| ApiError(paur_core::Error::Invalid(format!("unknown variant: {v}"))))?,
        ),
        None => None,
    };
    let limit = q.limit.unwrap_or(50).clamp(1, 1000);
    let rows = state
        .db
        .list_builds(q.pkg.as_deref(), status, variant, limit)
        .await?;
    Ok(Json(rows))
}

async fn get_build(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> ApiResult<Json<paur_db::Build>> {
    let b = state
        .db
        .get_build(id)
        .await?
        .ok_or_else(|| ApiError(paur_core::Error::NotFound(format!("build {id}"))))?;
    Ok(Json(b))
}

/// `POST /api/v1/builds/:id/cancel` — admin-only.
///
/// Two-step cancel:
///   1. `cancel_build` flips the DB row to `cancelled` (or
///      reports `AlreadyTerminal` / `NotFound`).
///   2. If the build was `running`, we also fire its
///      `CancellationToken` so the in-flight container is
///      killed and the worker re-stamps the row as
///      `Cancelled` (rather than `Failed`) on its way out.
///
/// Returns 200 on actual cancellation, 404 if the id is
/// unknown, 409 if the build is already in a terminal state.
async fn cancel_build(
    State(state): State<Arc<AppState>>,
    _admin: Admin,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    use paur_db::CancelOutcome;
    let outcome = state.db.cancel_build(id).await?;
    match outcome {
        CancelOutcome::NotFound => Err(ApiError(paur_core::Error::NotFound(format!(
            "build {id}"
        )))),
        CancelOutcome::AlreadyTerminal(s) => Err(ApiError(paur_core::Error::Conflict(
            format!("build {id} is already in terminal state: {}", s.as_str()),
        ))),
        CancelOutcome::Cancelled => {
            // Fire the in-process token if the build is currently
            // running. We do this *after* the DB update so the
            // registry map and the DB row are consistent: even if
            // the worker is mid-claim and didn't insert its token
            // yet, the row is already `cancelled` and the next
            // claim attempt will skip it.
            let token = state.cancel_tokens.lock().await.remove(&id);
            if let Some(t) = token {
                t.cancel();
            }
            Ok(Json(serde_json::json!({
                "id": id,
                "status": "cancelled",
            })))
        }
    }
}

async fn raw_logs(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> ApiResult<Response> {
    let logs = state.db.read_logs(id).await?;
    let body = logs
        .into_iter()
        .map(|(_, line)| line)
        .collect::<Vec<_>>()
        .join("\n");
    Ok(([(axum::http::header::CONTENT_TYPE, "text/plain; charset=utf-8")], body).into_response())
}

async fn stream_logs(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> ApiResult<
    Sse<futures::stream::BoxStream<'static, std::result::Result<Event, axum::Error>>>,
> {
    use futures::stream::BoxStream;

    // Subscriber holds the broadcast receiver; the worker writes to it
    // through the LogSink. If the build already finished, the channel
    // is dropped — the subscriber gets a single 'end' event.
    let mut rx = {
        let map = state.log_channels.lock().await;
        match map.get(&id) {
            Some(tx) => tx.subscribe(),
            None => {
                // No live channel. Either the build hasn't started,
                // or it has finished. Emit the cached log lines
                // and a 'done' marker, then end.
                let logs = state.db.read_logs(id).await?;
                let cached: BoxStream<std::result::Result<Event, axum::Error>> =
                    futures::stream::iter(
                        logs.into_iter()
                            .map(|(_, line)| Ok(Event::default().data(line)))
                            .chain(std::iter::once(Ok(
                                Event::default().event("done").data("")
                            ))),
                    )
                    .boxed();
                return Ok(Sse::new(cached).keep_alive(KeepAlive::default()));
            }
        }
    };

    let state2 = Arc::clone(&state);
    let stream = async_stream::stream! {
        // Drain the broadcast channel.
        loop {
            match rx.recv().await {
                Ok(line) => yield Ok::<_, axum::Error>(Event::default().data(line)),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
        // Append the full persisted log and emit 'done'.
        if let Ok(logs) = state2.db.read_logs(id).await {
            for (_, line) in logs {
                yield Ok(Event::default().data(line));
            }
        }
        yield Ok(Event::default().event("done").data(""));
    };
    Ok(Sse::new(stream.boxed()).keep_alive(KeepAlive::default()))
}

async fn queue(State(state): State<Arc<AppState>>) -> ApiResult<Json<serde_json::Value>> {
    let queued = state
        .db
        .list_builds(None, Some(paur_db::BuildStatus::Queued), None, 1000)
        .await?;
    let running = state
        .db
        .list_builds(None, Some(paur_db::BuildStatus::Running), None, 1000)
        .await?;
    Ok(Json(serde_json::json!({
        "queued": queued,
        "running": running,
    })))
}

/// `GET /api/v1/install/fpr` — UI helper for the Install page. Reads
/// the served pubkey from the arch dir, runs `gpg --show-keys` on
/// it, and returns the primary FPR. This avoids shipping a PGP
/// parser in the UI bundle: the daemon already has GPG on PATH
/// (it uses it to sign the repo DB).
#[derive(Deserialize)]
struct InstallFprQuery {
    /// Host the client will reach. Unused on the server side, but
    /// kept in the request so the UI can render a single URL that
    /// round-trips through the same install flow it documents.
    #[allow(dead_code)]
    host: String,
}

async fn install_fpr(
    State(state): State<Arc<AppState>>,
    Query(_q): Query<InstallFprQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let pubkey_path = state.repo.arch_subdir(Variant::Default).join("paur.pubkey.asc");
    let bytes = tokio::fs::read(&pubkey_path)
        .await
        .map_err(|_| paur_core::Error::NotFound(
            "pubkey not exported yet; run `paur init`".into(),
        ))?;
    let fpr = paur_repo::primary_fpr(&bytes).await.map_err(|e| {
        ApiError(paur_core::Error::Invalid(format!("gpg parse: {e}")))
    })?;
    Ok(Json(serde_json::json!({ "fpr": fpr })))
}

/// Serve a single file from the arch-specific repo dir. Mounted at
/// `/api/v1/repo/:arch/*file`; Caddy strips the `/api/v1/repo/:arch/`
/// prefix and forwards the rest. We resolve the requested path under
/// `state.repo.arch_dir()` and reject any traversal (`..`).
async fn serve_repo_file(
    State(state): State<Arc<AppState>>,
    Path((arch, file)): Path<(String, String)>,
) -> ApiResult<Response> {
    if arch != state.repo.arch {
        return Err(ApiError(paur_core::Error::NotFound(format!(
            "unknown arch: {arch}"
        ))));
    }
    // Reject path traversal: the `*file` matcher can capture `..`
    // segments. Canonicalize after joining and require the result to
    // live under arch_dir.
    let base = state.repo.arch_dir();
    let path = base.join(&file);
    let canon = match tokio::fs::canonicalize(&path).await {
        Ok(p) => p,
        Err(_) => {
            return Err(ApiError(paur_core::Error::NotFound(format!(
                "no such file: {file}"
            ))));
        }
    };
    let base_canon = tokio::fs::canonicalize(&base)
        .await
        .map_err(paur_core::Error::Io)?;
    if !canon.starts_with(&base_canon) {
        return Err(ApiError(paur_core::Error::Invalid(
            "path traversal blocked".into(),
        )));
    }
    let bytes = tokio::fs::read(&canon)
        .await
        .map_err(|_| paur_core::Error::NotFound(format!("no such file: {file}")))?;
    let ct = match canon.extension().and_then(|s| s.to_str()) {
        Some("sig") => "application/pgp-signature",
        Some("asc") => "application/pgp-keys",
        Some("gz") => "application/gzip",
        _ => "application/octet-stream",
    };
    Ok((
        [
            (axum::http::header::CONTENT_TYPE, ct),
            (axum::http::header::CACHE_CONTROL, "no-cache, must-revalidate"),
        ],
        bytes,
    )
        .into_response())
}
