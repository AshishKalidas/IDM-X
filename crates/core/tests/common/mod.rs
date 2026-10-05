//! A tiny origin server for tests: deterministic bytes, real range support, and knobs to make
//! it misbehave in the ways real servers do.
#![allow(dead_code)]

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use parking_lot::Mutex;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::Arc;
use std::time::Duration;

pub fn byte_at(i: u64) -> u8 {
    (i.wrapping_mul(2654435761) >> 13) as u8 ^ (i >> 9) as u8
}

pub fn expected(size: u64) -> Vec<u8> {
    (0..size).map(byte_at).collect()
}

#[derive(Clone)]
pub struct Config {
    pub size: u64,
    pub ranges: bool,
    pub etag: String,
    /// Bytes/second per response (0 = as fast as possible).
    pub per_conn_bps: u64,
    /// The first N responses die this many bytes in.
    pub drop_first: u64,
    pub drop_after_bytes: u64,
    /// Answer 503 to every request beyond this many concurrent ones.
    pub max_conns: u64,
    pub disposition: Option<String>,
    /// Digest header to publish.
    pub digest_sha256_b64: Option<String>,
    /// First N requests answer 403 (an expired signed URL).
    pub forbid_first: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            size: 8 << 20,
            ranges: true,
            etag: "\"v1\"".into(),
            per_conn_bps: 0,
            drop_first: 0,
            drop_after_bytes: 300_000,
            max_conns: u64::MAX,
            disposition: None,
            digest_sha256_b64: None,
            forbid_first: 0,
        }
    }
}

pub struct Origin {
    pub addr: SocketAddr,
    pub cfg: Arc<Mutex<Config>>,
    pub requests: Arc<AtomicU64>,
    pub active: Arc<AtomicU64>,
    pub peak: Arc<AtomicU64>,
    pub bytes_served: Arc<AtomicU64>,
}

impl Origin {
    pub fn url(&self, path: &str) -> String {
        format!("http://{}/{}", self.addr, path)
    }
}

#[derive(Clone)]
struct S {
    cfg: Arc<Mutex<Config>>,
    requests: Arc<AtomicU64>,
    active: Arc<AtomicU64>,
    peak: Arc<AtomicU64>,
    bytes: Arc<AtomicU64>,
}

pub async fn start(cfg: Config) -> Origin {
    start_on(cfg, "127.0.0.1:0".to_string()).await
}

pub async fn start_on(cfg: Config, bind: String) -> Origin {
    let cfg = Arc::new(Mutex::new(cfg));
    let s = S {
        cfg: cfg.clone(),
        requests: Default::default(),
        active: Default::default(),
        peak: Default::default(),
        bytes: Default::default(),
    };
    let app = Router::new().route("/{*path}", get(serve)).with_state(s.clone());
    let listener = tokio::net::TcpListener::bind(bind).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Origin { addr, cfg, requests: s.requests, active: s.active, peak: s.peak, bytes_served: s.bytes }
}

struct ActiveGuard(Arc<AtomicU64>);
impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Relaxed);
    }
}

async fn serve(State(s): State<S>, headers: HeaderMap) -> Response {
    let n = s.requests.fetch_add(1, Relaxed);
    let cfg = s.cfg.lock().clone();
    let now_active = s.active.fetch_add(1, Relaxed) + 1;
    s.peak.fetch_max(now_active, Relaxed);
    let guard = ActiveGuard(s.active.clone());

    if n < cfg.forbid_first {
        return Response::builder().status(StatusCode::FORBIDDEN).body(Body::empty()).unwrap();
    }
    if now_active > cfg.max_conns {
        return Response::builder().status(StatusCode::SERVICE_UNAVAILABLE).body(Body::empty()).unwrap();
    }

    let size = cfg.size;
    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("bytes="))
        .and_then(|v| {
            let (a, b) = v.split_once('-')?;
            let a: u64 = a.parse().ok()?;
            let b: u64 = if b.is_empty() { size.saturating_sub(1) } else { b.parse().ok()? };
            Some((a, b.min(size.saturating_sub(1))))
        });

    // If-Range with a stale validator means "send the whole thing".
    let stale = headers.get(header::IF_RANGE).and_then(|v| v.to_str().ok()).is_some_and(|v| v != cfg.etag);

    let mut rb = Response::builder().header(header::ETAG, &cfg.etag).header(header::CONTENT_TYPE, "application/octet-stream");
    if cfg.ranges {
        rb = rb.header(header::ACCEPT_RANGES, "bytes");
    }
    if let Some(d) = &cfg.disposition {
        rb = rb.header(header::CONTENT_DISPOSITION, d);
    }
    if let Some(d) = &cfg.digest_sha256_b64 {
        rb = rb.header("digest", format!("sha-256={d}"));
    }

    let (start, end, status) = match range {
        Some((a, b)) if cfg.ranges && !stale => {
            if a >= size {
                return rb
                    .status(StatusCode::RANGE_NOT_SATISFIABLE)
                    .header(header::CONTENT_RANGE, format!("bytes */{size}"))
                    .body(Body::empty())
                    .unwrap();
            }
            rb = rb.header(header::CONTENT_RANGE, format!("bytes {a}-{b}/{size}"));
            (a, b, StatusCode::PARTIAL_CONTENT)
        }
        _ => (0, size.saturating_sub(1), StatusCode::OK),
    };
    let len = if size == 0 { 0 } else { end - start + 1 };
    rb = rb.header(header::CONTENT_LENGTH, len);

    let drop_at = (n < cfg.drop_first).then_some(cfg.drop_after_bytes);
    let bps = cfg.per_conn_bps;
    let bytes_counter = s.bytes.clone();
    let stream = futures_util::stream::unfold((start, 0u64, guard), move |(pos, sent, guard)| {
        let bytes_counter = bytes_counter.clone();
        async move {
            if sent >= len {
                return None;
            }
            if let Some(d) = drop_at {
                if sent >= d {
                    return Some((Err(std::io::Error::other("injected drop")), (pos, len, guard)));
                }
            }
            let n = (64 * 1024u64).min(len - sent);
            if bps > 0 {
                tokio::time::sleep(Duration::from_secs_f64(n as f64 / bps as f64)).await;
            }
            let chunk: Vec<u8> = (pos..pos + n).map(byte_at).collect();
            bytes_counter.fetch_add(n, Relaxed);
            Some((Ok::<_, std::io::Error>(bytes::Bytes::from(chunk)), (pos + n, sent + n, guard)))
        }
    });
    let mut resp = rb.status(status).body(Body::from_stream(stream)).unwrap();
    resp.headers_mut().insert("x-test", HeaderValue::from_static("1"));
    resp
}

/// A stand-in for GitHub's "latest release" endpoint.
pub async fn release_feed(status: u16, body: &'static str) -> std::net::SocketAddr {
    let app = Router::new().route(
        "/latest",
        get(move || async move {
            Response::builder().status(status).header("content-type", "application/json").body(Body::from(body)).unwrap()
        }),
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let a = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    a
}
