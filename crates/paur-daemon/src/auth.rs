//! Admin authentication for the paur HTTP API.
//!
//! - One user (`admin`) whose password hash is stored in the
//!   `settings` table under the key `admin_password_hash`.
//! - Login hands the browser a 32-byte random token in an HttpOnly
//!   `paur_session` cookie. Only the SHA-256 of the token is persisted
//!   in the `sessions` table, so a leaked DB does not let the
//!   attacker reuse live sessions.
//! - [`Admin`] is an axum [`FromRequestParts`] extractor; using it in a
//!   handler signature gates that route. A missing/expired session
//!   responds 401, a session whose login is required (no password set
//!   yet) responds 503 with a hint to run `paur passwd`.

use std::sync::Arc;

use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::worker::AppState;

/// Name of the session cookie. Kept short to fit inside header limits
/// even with a long path-prefixed domain.
pub const SESSION_COOKIE: &str = "paur_session";

/// Default session lifetime.
pub const SESSION_TTL_SECS: i64 = 24 * 60 * 60;

/// Settings key for the bcrypt hash of the admin password.
pub const SETTING_PASSWORD_HASH: &str = "admin_password_hash";

/// Sub-router for the auth endpoints. Mounted at the API root (no
/// additional prefix) by the parent router's `merge`.
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/auth/logout", post(logout))
        .route("/api/v1/auth/status", axum::routing::get(status))
}

// -------- request/response payloads --------

#[derive(Deserialize)]
struct LoginBody {
    username: String,
    password: String,
}

#[derive(Serialize)]
struct StatusResponse {
    /// True if a valid session cookie was presented.
    authenticated: bool,
    /// True if the admin password has been configured. When false, all
    /// write endpoints respond 503 until the operator sets a password.
    password_set: bool,
}

// -------- handlers --------

async fn login(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Json(body): Json<LoginBody>,
) -> Result<(CookieJar, Json<StatusResponse>), AuthHttpError> {
    if body.username != "admin" {
        // Same error path as bad password to avoid leaking which one
        // was wrong.
        return Err(AuthHttpError::InvalidCredentials);
    }
    let stored = state
        .db
        .get_setting(SETTING_PASSWORD_HASH)
        .await
        .map_err(AuthHttpError::from)?
        .unwrap_or_default();
    if stored.is_empty() {
        return Err(AuthHttpError::PasswordNotSet);
    }
    let ok = paur_core::auth::verify_password(&body.password, &stored)
        .map_err(AuthHttpError::from)?;
    if !ok {
        return Err(AuthHttpError::InvalidCredentials);
    }

    let token = paur_core::auth::new_session_token();
    let token_hash = paur_core::auth::hash_session_token(&token);
    let expires_at = paur_db::Db::now() + SESSION_TTL_SECS;
    state
        .db
        .create_session(&token_hash, "admin", expires_at)
        .await
        .map_err(AuthHttpError::from)?;

    // Hand the client the *raw* token in the cookie. The DB only knows
    // the hash, so a DB dump is not enough to forge a session.
    let cookie = Cookie::build((SESSION_COOKIE, hex::encode(token)))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .max_age(time::Duration::seconds(SESSION_TTL_SECS))
        .build();
    Ok((
        jar.add(cookie),
        Json(StatusResponse {
            authenticated: true,
            password_set: true,
        }),
    ))
}

async fn logout(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
) -> Result<(CookieJar, Json<serde_json::Value>), AuthHttpError> {
    if let Some(c) = jar.get(SESSION_COOKIE) {
        let raw = c.value();
        if let Ok(bytes) = hex::decode(raw) {
            let hash = paur_core::auth::hash_session_token(&bytes);
            // Errors here are best-effort: even if the delete fails, we
            // still clear the cookie so the client forgets the token.
            let _ = state.db.delete_session(&hash).await;
        }
    }
    let cleared = Cookie::build((SESSION_COOKIE, ""))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .max_age(time::Duration::seconds(0))
        .build();
    Ok((
        jar.add(cleared),
        Json(json!({ "ok": true })),
    ))
}

async fn status(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
) -> Result<Json<StatusResponse>, AuthHttpError> {
    let password_set = state
        .db
        .get_setting(SETTING_PASSWORD_HASH)
        .await
        .map_err(AuthHttpError::from)?
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    let authenticated = session_user(&state, &jar).await?.is_some();
    Ok(Json(StatusResponse {
        authenticated,
        password_set,
    }))
}

// -------- extractor: gate write routes behind a valid session --------

/// Marker extractor: present this in a handler signature to require a
/// valid admin session. Responds 503 (password not yet configured) or
/// 401 (no session / expired) on failure.
pub struct Admin;

#[axum::async_trait]
impl FromRequestParts<Arc<AppState>> for Admin {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        // If the admin password has never been set, every write must
        // fail closed with a hint to run `paur passwd`. We check this
        // before accepting any session, because a session issued
        // before the password was set is not really meaningful.
        let stored = state
            .db
            .get_setting(SETTING_PASSWORD_HASH)
            .await
            .map_err(|e| AuthHttpError::from(e).into_response())?;
        if stored.as_deref().map(str::is_empty).unwrap_or(true) {
            return Err(AuthHttpError::PasswordNotSet.into_response());
        }

        let jar = CookieJar::from_request_parts(parts, state)
            .await
            .map_err(|e| e.into_response())?;
        match session_user(state, &jar).await {
            Ok(Some(_)) => Ok(Admin),
            Ok(None) => Err(AuthHttpError::Unauthenticated.into_response()),
            Err(e) => Err(e.into_response()),
        }
    }
}

// -------- helpers --------

async fn session_user(
    state: &Arc<AppState>,
    jar: &CookieJar,
) -> Result<Option<String>, AuthHttpError> {
    let Some(cookie) = jar.get(SESSION_COOKIE) else {
        return Ok(None);
    };
    let bytes = match hex::decode(cookie.value()) {
        Ok(b) => b,
        Err(_) => return Ok(None),
    };
    let hash = paur_core::auth::hash_session_token(&bytes);
    Ok(state.db.lookup_session(&hash).await.map_err(AuthHttpError::from)?)
}

// -------- error type for auth endpoints --------

#[derive(Debug)]
enum AuthHttpError {
    InvalidCredentials,
    PasswordNotSet,
    Unauthenticated,
    Internal(paur_core::Error),
}

impl From<paur_core::Error> for AuthHttpError {
    fn from(e: paur_core::Error) -> Self {
        AuthHttpError::Internal(e)
    }
}

impl IntoResponse for AuthHttpError {
    fn into_response(self) -> Response {
        match self {
            AuthHttpError::InvalidCredentials => (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "invalid username or password" })),
            )
                .into_response(),
            AuthHttpError::PasswordNotSet => (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": "admin password not configured; run `paur passwd` on the host"
                })),
            )
                .into_response(),
            AuthHttpError::Unauthenticated => (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "login required" })),
            )
                .into_response(),
            AuthHttpError::Internal(e) => {
                tracing::error!("auth internal: {e}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": e.to_string() })),
                )
                    .into_response()
            }
        }
    }
}
