//! Integration tests for `paur-s3` against a `wiremock` server.
//!
//! These exercise the public surface end-to-end: a real `reqwest`
//! client talks to a real HTTP listener, so the SigV4 signer, retry
//! loop, and URL builder are all under test together. Wiremock's
//! matchers let us assert on the `Authorization` header shape without
//! re-implementing SigV4 in the test.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use paur_core::S3Config;
use paur_s3::{Error, S3Client, SignedRequest, Transport};
use reqwest::{Client, Response};
use wiremock::matchers::{header, header_exists, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// An override `Transport` that points the client at the wiremock
/// server's base URL, regardless of what the S3Config says the
/// endpoint is. We can't just point `endpoint` at the mock because
/// the signer bakes the host header into SigV4 — so we have to
/// either rebuild the canonical request with a fake host (no) or
/// have the mock listen on a hostname we can sign for (yes).
///
/// Easiest: have the mock accept any `Host` header, and override
/// the transport to rewrite `req.url` to point at the mock. The
/// signer ran on the original URL, so the signature won't actually
/// validate against S3, but the mock doesn't check signatures — we
/// only assert the *shape* of the Authorization header.
struct RewriteTransport {
    inner: Client,
    base: String,
}

#[async_trait]
impl Transport for RewriteTransport {
    async fn execute(
        &self,
        req: SignedRequest<'_>,
        body: Vec<u8>,
    ) -> Result<Response, reqwest::Error> {
        // The signed URL is the S3 endpoint; rewrite path to mock,
        // keep query string. (No query in our usage today.)
        let path_and_q = req.url.split_once("://").unwrap().1;
        let slash = path_and_q.find('/').unwrap_or(path_and_q.len());
        let path = &path_and_q[slash..];
        let url = format!("{}{}", self.base, path);
        let mut rb = self.inner.request(req.method, &url);
        for (k, v) in &req.headers {
            rb = rb.header(*k, v.as_str());
        }
        rb.body(body).send().await
    }
}

fn client_for(base: &str) -> S3Client {
    let cfg = S3Config {
        bucket: "paur-repo".into(),
        // Endpoint will be rewritten by RewriteTransport, but the
        // signer still needs *something* plausible here.
        endpoint: Some("https://s3.example".into()),
        region: "us-east-1".into(),
        prefix: None,
        access_key: "AKIATEST".into(),
        secret_key: "secret".into(),
        path_style: true,
        public_url: None,
    };
    let transport = Arc::new(RewriteTransport {
        inner: Client::new(),
        base: base.to_string(),
    });
    S3Client::with_transport(cfg, transport)
}

#[tokio::test]
async fn put_sends_sigv4_authorization_and_correct_url() {
    let server = MockServer::start().await;

    Mock::given(method("PUT"))
        .and(path("/paur-repo/x86_64/paur.db.tar.gz"))
        .and(header_exists("Authorization"))
        .and(header_exists("x-amz-content-sha256"))
        .and(header("host", "s3.example"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let url = client
        .put("x86_64/paur.db.tar.gz", "application/x-gzip", b"hello".to_vec())
        .await
        .expect("put ok");
    // Without a public_url configured, public_url_for_key returns
    // the constructed S3 endpoint URL — the path is what matters.
    let path = url.split_once("x86_64/").expect("key in url");
    assert!(path.0.ends_with("/paur-repo/"), "unexpected prefix: {url}");
    assert!(url.ends_with("/x86_64/paur.db.tar.gz"), "url: {url}");
}

#[tokio::test]
async fn put_retries_then_succeeds_on_transient_5xx() {
    let server = MockServer::start().await;

    // First call: 503. Second call: 200. We use a counter via two
    // named Mocks — wiremock matches the most-recently-mounted first.
    Mock::given(method("PUT"))
        .and(path("/paur-repo/x86_64/foo.pkg.tar.zst"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/paur-repo/x86_64/foo.pkg.tar.zst"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    // Shrink the retry delays so the test doesn't take 1+2+5+15s.
    // We can't reach into RETRY_DELAYS, so instead we use a fresh
    // client with a server that responds 503 *twice* then 200; the
    // first attempt fails fast (no sleep), the second sleeps ~1s
    // (±30%). Total budget ~1.3s — acceptable.
    let client = client_for(&server.uri());
    let start = std::time::Instant::now();
    let res = client
        .put("x86_64/foo.pkg.tar.zst", "application/octet-stream", b"x".to_vec())
        .await;
    let elapsed = start.elapsed();
    assert!(res.is_ok(), "expected retry to succeed: {res:?}");
    // Lower bound: at least one jittered delay (≥700ms) consumed.
    assert!(elapsed >= Duration::from_millis(600), "elapsed: {elapsed:?}");
}

#[tokio::test]
async fn put_does_not_retry_on_4xx() {
    let server = MockServer::start().await;

    // 403 with no second response registered. If the client retried,
    // wiremock would return 404 on the second hit (no matcher) —
    // and we'd see the wrong final status.
    Mock::given(method("PUT"))
        .and(path("/paur-repo/x86_64/bad.pkg.tar.zst"))
        .respond_with(ResponseTemplate::new(403).set_body_string("nope"))
        .expect(1) // exactly one call
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let res = client
        .put("x86_64/bad.pkg.tar.zst", "application/octet-stream", b"x".to_vec())
        .await;
    match res {
        Err(Error::Permanent { status, body }) => {
            assert_eq!(status, 403);
            assert!(body.contains("nope"));
        }
        other => panic!("expected Permanent(403), got {other:?}"),
    }
}

#[tokio::test]
async fn delete_treats_404_as_success() {
    let server = MockServer::start().await;

    Mock::given(method("DELETE"))
        .and(path("/paur-repo/x86_64/missing.pkg.tar.zst"))
        .respond_with(ResponseTemplate::new(404))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    client
        .delete("x86_64/missing.pkg.tar.zst")
        .await
        .expect("delete of missing key should be ok");
}

#[tokio::test]
async fn delete_happy_path() {
    let server = MockServer::start().await;

    Mock::given(method("DELETE"))
        .and(path("/paur-repo/x86_64/gone.pkg.tar.zst"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    client
        .delete("x86_64/gone.pkg.tar.zst")
        .await
        .expect("delete ok");
}

#[tokio::test]
async fn prefix_is_applied_to_keys() {
    let server = MockServer::start().await;

    // We expect the path *with* the prefix appended.
    Mock::given(method("PUT"))
        .and(path("/paur-repo/paur/x86_64/foo.pkg.tar.zst"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    let cfg = S3Config {
        bucket: "paur-repo".into(),
        endpoint: Some("https://s3.example".into()),
        region: "us-east-1".into(),
        prefix: Some("paur/".into()),
        access_key: "AKIATEST".into(),
        secret_key: "secret".into(),
        path_style: true,
        public_url: Some("https://cdn.example/repo".into()),
    };
    let transport = Arc::new(RewriteTransport {
        inner: Client::new(),
        base: server.uri(),
    });
    let client = S3Client::with_transport(cfg, transport);
    let url = client
        .put("x86_64/foo.pkg.tar.zst", "application/octet-stream", b"y".to_vec())
        .await
        .expect("put ok with prefix");
    // Regression: the returned public URL must contain the prefix
    // exactly once. Earlier this was double-prefixed because
    // put() passed the already-prefixed full_key to
    // public_url_for_key, which re-applied the prefix.
    assert_eq!(
        url, "https://cdn.example/repo/paur/x86_64/foo.pkg.tar.zst",
        "expected single-prefix public URL, got {url}"
    );
}
