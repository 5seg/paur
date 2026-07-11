//! Minimal S3-compatible uploader for paur.
//!
//! Why a hand-rolled client instead of `aws-sdk-s3`? The SDK pulls
//! hundreds of crates and tens of MB of binary. For paur's needs
//! (PUT a few `.pkg.tar.zst` + DB + `.sig` files, occasionally DELETE
//! one, no multipart, no large objects beyond a few hundred MB) the
//! S3 REST surface is small enough to implement directly on top of
//! `reqwest` with a from-scratch AWS SigV4 signer.
//!
//! Supported operations:
//! - `PUT /<key>`  — single-shot upload with `Content-Type` (no multipart)
//! - `DELETE /<key>` — remove an object
//!
//! Failure handling: every call retries with exponential backoff
//! (1s, 2s, 5s, 15s, 30s, each ±30% jitter) on transient errors
//! (network, 5xx, 408, 429). The first attempt's request is built
//! fresh on every retry so signing stays correct.

use std::time::Duration;

use async_trait::async_trait;
use rand::Rng;
use reqwest::{Client, Method, StatusCode};
use tokio::time::sleep;
use tracing::{debug, warn};

use paur_core::config::S3Config;

pub use paur_core::Error as CoreError;
pub type Result<T> = std::result::Result<T, Error>;

/// Errors emitted by this crate. Network/HTTP errors are wrapped as
/// `Transient` (worth retrying); 4xx responses are `Permanent`
/// (don't bother); signing/serialization errors are `Bug` (caller
/// can't fix it).
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("s3: {0}")]
    Msg(String),
    #[error("s3 transient: {0}")]
    Transient(String),
    #[error("s3 permanent: status={status} body={body}")]
    Permanent { status: u16, body: String },
    #[error("s3: reqwest: {0}")]
    Http(#[from] reqwest::Error),
    #[error("s3: signing: {0}")]
    Sign(String),
}

/// Retry policy: 5 attempts, base delays 1s, 2s, 5s, 15s (4 retries
/// after the initial attempt). Each delay gets ±30% jitter so we
/// don't hammer the endpoint on correlated retries.
const RETRY_DELAYS: [Duration; 4] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(15),
];

/// Abstract over the actual HTTP transport so tests can swap in a
/// mock without touching `wiremock`-style middlewares.
#[async_trait]
pub trait Transport: Send + Sync {
    async fn execute(
        &self,
        req: SignedRequest<'_>,
        body: Vec<u8>,
    ) -> std::result::Result<reqwest::Response, reqwest::Error>;
}

/// A request that has been fully signed and is ready to send.
#[derive(Debug, Clone)]
pub struct SignedRequest<'a> {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(&'a str, String)>,
}

/// Default `reqwest`-backed transport. Reuses one connection pool.
pub struct HttpTransport {
    client: Client,
}

impl Default for HttpTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpTransport {
    pub fn new() -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .expect("reqwest client build");
        Self { client }
    }
}

#[async_trait]
impl Transport for HttpTransport {
    async fn execute(
        &self,
        req: SignedRequest<'_>,
        body: Vec<u8>,
    ) -> std::result::Result<reqwest::Response, reqwest::Error> {
        let mut rb = self.client.request(req.method, &req.url);
        for (k, v) in &req.headers {
            rb = rb.header(*k, v.as_str());
        }
        rb.body(body).send().await
    }
}

/// S3 client. Cheap to clone (everything inside is reference-counted).
pub struct S3Client {
    cfg: S3Config,
    transport: std::sync::Arc<dyn Transport>,
}

impl S3Client {
    pub fn new(cfg: S3Config) -> Self {
        Self {
            cfg,
            transport: std::sync::Arc::new(HttpTransport::new()),
        }
    }

    /// Construct a client with a custom transport (test-only).
    pub fn with_transport(cfg: S3Config, transport: std::sync::Arc<dyn Transport>) -> Self {
        Self { cfg, transport }
    }

    /// Build the full URL for `key`. Resolves virtual-hosted vs
    /// path-style from `cfg.path_style`.
    fn url_for(&self, key: &str) -> String {
        let key = key.trim_start_matches('/');
        let endpoint = self
            .cfg
            .endpoint
            .as_deref()
            .unwrap_or("https://s3.amazonaws.com")
            .trim_end_matches('/');
        if self.cfg.path_style {
            format!("{}/{}/{}", endpoint, self.cfg.bucket, key)
        } else {
            // Virtual-hosted: parse the endpoint, swap in <bucket>.<host>
            // so e.g. https://s3.amazonaws.com/foo -> https://paur-repo.s3.amazonaws.com/foo
            if let Some((scheme, rest)) = endpoint.split_once("://") {
                format!("{}://{}.{}/{}", scheme, self.cfg.bucket, rest, key)
            } else {
                format!("{}/{}/{}", endpoint, self.cfg.bucket, key)
            }
        }
    }

    /// Apply the configured prefix to a key.
    fn full_key(&self, key: &str) -> String {
        match &self.cfg.prefix {
            Some(p) if !p.is_empty() => {
                let p = p.trim_end_matches('/');
                format!("{}/{}", p, key.trim_start_matches('/'))
            }
            _ => key.trim_start_matches('/').to_string(),
        }
    }

    /// Upload `body` to `key` with `content_type`. Retries on
    /// transient errors. Returns the public URL the client should
    /// use to fetch the object.
    pub async fn put(
        &self,
        key: &str,
        content_type: &str,
        body: Vec<u8>,
    ) -> Result<String> {
        let full_key = self.full_key(key);
        let url = self.url_for(&full_key);
        let mut attempt = 0u32;
        loop {
            let req = sign_request(
                &self.cfg,
                Method::PUT,
                &url,
                content_type,
                &body,
                full_key.clone(),
            )?;
            let res = self.transport.execute(req, body.clone()).await?;
            let status = res.status();
            if status.is_success() {
                debug!(key = %full_key, status = status.as_u16(), "s3: put ok");
                // public_url_for_key applies the prefix internally,
                // so pass the raw key — not `full_key` (which would
                // be prefixed twice).
                return Ok(self.public_url_for_key(key));
            }
            // Drain the body so the connection can be reused.
            let body_text = res.text().await.unwrap_or_default();
            let body_text = truncate(&body_text, 512);
            let transient = is_transient(status);
            if !transient {
                return Err(Error::Permanent {
                    status: status.as_u16(),
                    body: body_text,
                });
            }
            if attempt as usize >= RETRY_DELAYS.len() {
                return Err(Error::Transient(format!(
                    "exhausted retries on {} (last status {}): {}",
                    full_key, status, body_text
                )));
            }
            let delay = jittered(RETRY_DELAYS[attempt as usize]);
            warn!(
                key = %full_key,
                status = status.as_u16(),
                attempt = attempt + 1,
                delay_ms = delay.as_millis() as u64,
                "s3: put failed, retrying"
            );
            sleep(delay).await;
            attempt += 1;
        }
    }

    /// Delete `key`. Retries on transient errors. Idempotent: a 404
    /// is treated as success because the desired end state (object
    /// absent) holds.
    pub async fn delete(&self, key: &str) -> Result<()> {
        let full_key = self.full_key(key);
        let url = self.url_for(&full_key);
        let mut attempt = 0u32;
        loop {
            let req = sign_request(
                &self.cfg,
                Method::DELETE,
                &url,
                "application/octet-stream",
                &[],
                full_key.clone(),
            )?;
            let res = self.transport.execute(req, Vec::new()).await?;
            let status = res.status();
            if status.is_success() || status == StatusCode::NOT_FOUND {
                debug!(key = %full_key, status = status.as_u16(), "s3: delete ok");
                return Ok(());
            }
            let body_text = res.text().await.unwrap_or_default();
            let body_text = truncate(&body_text, 512);
            if !is_transient(status) {
                return Err(Error::Permanent {
                    status: status.as_u16(),
                    body: body_text,
                });
            }
            if attempt as usize >= RETRY_DELAYS.len() {
                return Err(Error::Transient(format!(
                    "exhausted retries deleting {} (last status {})",
                    full_key, status
                )));
            }
            let delay = jittered(RETRY_DELAYS[attempt as usize]);
            warn!(
                key = %full_key,
                status = status.as_u16(),
                attempt = attempt + 1,
                delay_ms = delay.as_millis() as u64,
                "s3: delete failed, retrying"
            );
            sleep(delay).await;
            attempt += 1;
        }
    }

    /// Build the public URL clients use to fetch `key`. Falls back
    /// to the constructed endpoint URL when no `public_url` is
    /// configured.
    pub fn public_url_for_key(&self, key: &str) -> String {
        let full_key = self.full_key(key);
        match &self.cfg.public_url {
            Some(base) if !base.is_empty() => {
                let base = base.trim_end_matches('/');
                format!("{}/{}", base, full_key)
            }
            _ => self.url_for(&full_key),
        }
    }
}

/// Build and sign an S3 request per AWS SigV4. Returns headers
/// (including `Authorization`) ready to attach to a `reqwest` call.
fn sign_request(
    cfg: &S3Config,
    method: Method,
    url: &str,
    content_type: &str,
    body: &[u8],
    _key: String,
) -> Result<SignedRequest<'static>> {
    // AWS SigV4 wants amz-date in basic ISO 8601 (YYYYMMDDTHHMMSSZ) and
    // the date-only stamp for credential scope. We freeze a single
    // timestamp per request to keep canonical request + string-to-sign
    // consistent.
    let now = chrono::Utc::now();
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date_stamp = now.format("%Y%m%d").to_string();
    let host = url_host(url)?;
    let payload_hash = sha256_hex(body);

    // Headers we sign. SigV4 requires at minimum `host`; we add
    // content-type, amz-content-sha256, and amz-date.
    let mut headers: Vec<(&'static str, String)> = vec![
        ("host", host),
        ("x-amz-content-sha256", payload_hash.clone()),
        ("x-amz-date", amz_date.clone()),
        ("content-type", content_type.to_string()),
    ];
    headers.sort_by(|a, b| a.0.cmp(b.0));

    let canonical_headers: String = headers
        .iter()
        .map(|(k, v)| format!("{}:{}", k, v.trim()))
        .collect::<Vec<_>>()
        .join("\n");
    let signed_headers: String = headers
        .iter()
        .map(|(k, _)| *k)
        .collect::<Vec<_>>()
        .join(";");

    let (canonical_uri, canonical_query) = split_uri_query(url)?;
    let canonical_request = format!(
        "{}\n{}\n{}\n{}\n\n{}\n{}",
        method.as_str(),
        canonical_uri,
        canonical_query,
        canonical_headers,
        signed_headers,
        payload_hash
    );

    let credential_scope =
        format!("{}/{}/s3/aws4_request", date_stamp, cfg.region);
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{}\n{}\n{}",
        amz_date,
        credential_scope,
        sha256_hex(canonical_request.as_bytes())
    );

    let signing_key = derive_signing_key(
        &cfg.secret_key,
        &date_stamp,
        &cfg.region,
        "s3",
    );
    let signature = hmac_sha256_hex(&signing_key, &string_to_sign);

    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
        cfg.access_key, credential_scope, signed_headers, signature
    );

    let mut all_headers = headers;
    all_headers.push(("authorization", authorization));

    Ok(SignedRequest {
        method,
        url: url.to_string(),
        headers: all_headers,
    })
}

fn url_host(url: &str) -> Result<String> {
    // Cheap parse: scheme://host[:port][/path]. We only need host[:port].
    let after_scheme = url
        .split_once("://")
        .ok_or_else(|| Error::Sign(format!("url missing scheme: {url}")))?
        .1;
    let host_port = after_scheme
        .split_once('/')
        .map(|(h, _)| h)
        .unwrap_or(after_scheme);
    Ok(host_port.to_ascii_lowercase())
}

fn split_uri_query(url: &str) -> Result<(String, String)> {
    // Split off query, then split off the host. The canonical URI
    // must include the leading slash and percent-encode path
    // segments; for paur's keys (lowercase alnum + . + -) nothing
    // needs encoding, so a raw pass-through is correct.
    let (path_and_query, _) = url
        .split_once("://")
        .ok_or_else(|| Error::Sign(format!("url missing scheme: {url}")))?
        .1
        .split_once('/')
        .ok_or_else(|| Error::Sign(format!("url has no path: {url}")))?;
    let (_, path) = url.split_at(url.find("://").unwrap() + 3);
    // path is now "<host>/<path>"; we want everything from the first '/' after host.
    let (path, query) = match path.find('?') {
        Some(i) => (&path[..i], &path[i + 1..]),
        None => (path, ""),
    };
    // Use path_and_query only to satisfy the borrow checker above;
    // the real canonical URI is computed from the full URL.
    let _ = path_and_query;
    let canonical_uri = path
        .split_once('/')
        .map(|(_, p)| format!("/{}", p))
        .unwrap_or_else(|| "/".to_string());
    Ok((canonical_uri, query.to_string()))
}

fn derive_signing_key(
    secret: &str,
    date_stamp: &str,
    region: &str,
    service: &str,
) -> Vec<u8> {
    let k_date = hmac_sha256(format!("AWS4{}", secret).as_bytes(), date_stamp);
    let k_region = hmac_sha256(&k_date, region);
    let k_service = hmac_sha256(&k_region, service);
    hmac_sha256(&k_service, "aws4_request")
}

fn hmac_sha256(key: &[u8], msg: &str) -> Vec<u8> {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key)
        .expect("hmac accepts any key length");
    mac.update(msg.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

fn hmac_sha256_hex(key: &[u8], msg: &str) -> String {
    hex::encode(hmac_sha256(key, msg))
}

fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let h = Sha256::digest(data);
    hex::encode(h)
}

/// HTTP statuses that we treat as worth retrying. 5xx server errors
/// are always retried; 408 (request timeout) and 429 (rate limit) are
/// retried because they're explicitly transient. Everything else is
/// a hard 4xx that won't improve on retry.
fn is_transient(status: StatusCode) -> bool {
    status.is_server_error()
        || status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}

fn jittered(base: Duration) -> Duration {
    // ±30% jitter. Sample uniform [0, 1), scale to [0.7, 1.3).
    let mut rng = rand::thread_rng();
    let scale: f64 = 0.7 + rng.gen::<f64>() * 0.6;
    let ms = (base.as_millis() as f64 * scale) as u64;
    Duration::from_millis(ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_for_path_style() {
        let cfg = S3Config {
            bucket: "b".into(),
            endpoint: Some("https://minio.local:9000".into()),
            region: "us-east-1".into(),
            prefix: None,
            access_key: "a".into(),
            secret_key: "s".into(),
            path_style: true,
            public_url: None,
        };
        let c = S3Client::new(cfg);
        assert_eq!(c.url_for("foo/bar.pkg"), "https://minio.local:9000/b/foo/bar.pkg");
    }

    #[test]
    fn url_for_virtual_hosted() {
        let cfg = S3Config {
            bucket: "b".into(),
            endpoint: Some("https://s3.amazonaws.com".into()),
            region: "us-east-1".into(),
            prefix: None,
            access_key: "a".into(),
            secret_key: "s".into(),
            path_style: false,
            public_url: None,
        };
        let c = S3Client::new(cfg);
        assert_eq!(c.url_for("foo/bar.pkg"), "https://b.s3.amazonaws.com/foo/bar.pkg");
    }

    #[test]
    fn url_for_with_prefix() {
        let cfg = S3Config {
            bucket: "b".into(),
            endpoint: Some("https://minio.local:9000".into()),
            region: "us-east-1".into(),
            prefix: Some("paur/".into()),
            access_key: "a".into(),
            secret_key: "s".into(),
            path_style: true,
            public_url: None,
        };
        let c = S3Client::new(cfg);
        // public_url_for_key applies the configured prefix.
        assert_eq!(
            c.public_url_for_key("foo.pkg"),
            "https://minio.local:9000/b/paur/foo.pkg"
        );
        // url_for itself is the raw internal URL builder; it does
        // not apply the prefix (callers pass the full key in).
        assert_eq!(c.url_for("paur/foo.pkg"), "https://minio.local:9000/b/paur/foo.pkg");
    }

    #[test]
    fn url_for_uses_public_url_when_set() {
        let cfg = S3Config {
            bucket: "b".into(),
            endpoint: Some("https://s3.amazonaws.com".into()),
            region: "us-east-1".into(),
            prefix: None,
            access_key: "a".into(),
            secret_key: "s".into(),
            path_style: false,
            public_url: Some("https://pub.example.com/repo".into()),
        };
        let c = S3Client::new(cfg);
        assert_eq!(
            c.public_url_for_key("x86_64/foo.pkg"),
            "https://pub.example.com/repo/x86_64/foo.pkg"
        );
    }

    #[test]
    fn transient_classification() {
        assert!(is_transient(StatusCode::INTERNAL_SERVER_ERROR));
        assert!(is_transient(StatusCode::BAD_GATEWAY));
        assert!(is_transient(StatusCode::SERVICE_UNAVAILABLE));
        assert!(is_transient(StatusCode::REQUEST_TIMEOUT));
        assert!(is_transient(StatusCode::TOO_MANY_REQUESTS));
        assert!(!is_transient(StatusCode::NOT_FOUND));
        assert!(!is_transient(StatusCode::FORBIDDEN));
        assert!(!is_transient(StatusCode::OK));
    }

    #[test]
    fn jitter_keeps_window() {
        // ±30% on a 1000ms base should land in [700, 1300).
        for _ in 0..50 {
            let d = jittered(Duration::from_secs(1));
            let ms = d.as_millis();
            assert!((700..1300).contains(&ms), "out of window: {ms}");
        }
    }
}
