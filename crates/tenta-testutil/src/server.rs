//! Loopback HTTP/1.1 fixture server.
//!
//! Serves files from a root directory (`tests/fixtures/` by default) and the
//! `/api/*` endpoints the fixtures use. Simulated latency: `rtt / 2` before
//! the response head and `rtt / 2` before the body, plus optional per-path
//! delays, so a page's request waterfall looks like a real network.

use std::convert::Infallible;
use std::future::Future;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll, ready};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::body::{Body, Frame, Incoming, SizeHint};
use hyper::header::{self, HeaderValue};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tenta_core::FsPolicy;
use tokio::net::TcpListener;
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::{Interval, Sleep};

use crate::glob::Glob;

/// Interval between server-sent events on `/api/sse`.
pub const SSE_INTERVAL: Duration = Duration::from_millis(100);

/// A per-path delay rule: `glob=milliseconds`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathDelay {
    pub pattern: Glob,
    pub delay: Duration,
}

impl PathDelay {
    /// Parses `"/slow/*.js=300"`.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let (pattern, millis) = spec
            .rsplit_once('=')
            .ok_or_else(|| format!("expected GLOB=MS, got {spec:?}"))?;
        let millis: u64 = millis
            .trim()
            .parse()
            .map_err(|_| format!("invalid delay in {spec:?}: {millis:?} is not a number"))?;
        if pattern.is_empty() {
            return Err(format!("empty glob in {spec:?}"));
        }
        Ok(Self {
            pattern: Glob::new(pattern),
            delay: Duration::from_millis(millis),
        })
    }
}

#[derive(Clone, Debug)]
pub struct FixtureServerConfig {
    /// Directory served at `/`.
    pub root: PathBuf,
    /// Port on `127.0.0.1`; `0` picks a free one.
    pub port: u16,
    /// Simulated round-trip time; half is spent before the head, half before the body.
    pub rtt: Duration,
    /// Extra delay before the head of every response whose path matches.
    pub path_delays: Vec<PathDelay>,
    /// Print `REQ <unix_nanos> <method> <path>` to stderr on arrival of every request.
    pub log_requests: bool,
}

impl Default for FixtureServerConfig {
    fn default() -> Self {
        Self {
            root: crate::fixtures_root(),
            port: 0,
            rtt: Duration::ZERO,
            path_delays: Vec::new(),
            log_requests: false,
        }
    }
}

impl FixtureServerConfig {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            ..Self::default()
        }
    }

    pub fn rtt(mut self, rtt: Duration) -> Self {
        self.rtt = rtt;
        self
    }

    pub fn port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    pub fn delay_path(mut self, glob: &str, delay: Duration) -> Self {
        self.path_delays.push(PathDelay {
            pattern: Glob::new(glob),
            delay,
        });
        self
    }

    pub fn log_requests(mut self, enabled: bool) -> Self {
        self.log_requests = enabled;
        self
    }
}

struct State {
    root: PathBuf,
    policy: FsPolicy,
    rtt: Duration,
    path_delays: Vec<PathDelay>,
    log_requests: bool,
    requests: AtomicU64,
    /// Connections accepted and not yet closed by either side.
    open_connections: AtomicU64,
    started: Instant,
}

/// A running fixture server. Dropping it stops the accept loop and every
/// connection, including the ones parked on `/api/never`.
#[derive(Debug)]
pub struct FixtureServer {
    addr: SocketAddr,
    accept_loop: JoinHandle<()>,
    state: Arc<State>,
}

impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("State")
            .field("root", &self.root)
            .field("rtt", &self.rtt)
            .field("path_delays", &self.path_delays)
            .finish_non_exhaustive()
    }
}

impl FixtureServer {
    /// Binds and starts serving on the current Tokio runtime.
    pub async fn start(config: FixtureServerConfig) -> io::Result<Self> {
        let root = std::fs::canonicalize(&config.root).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("fixture root {}: {e}", config.root.display()),
            )
        })?;
        let listener =
            TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, config.port))).await?;
        let addr = listener.local_addr()?;
        let state = Arc::new(State {
            policy: FsPolicy::none().allow_read(&root),
            root,
            rtt: config.rtt,
            path_delays: config.path_delays,
            log_requests: config.log_requests,
            requests: AtomicU64::new(0),
            open_connections: AtomicU64::new(0),
            started: Instant::now(),
        });
        let accept_state = Arc::clone(&state);
        let accept_loop = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                let (stream, _) = match listener.accept().await {
                    Ok(pair) => pair,
                    Err(_) => continue,
                };
                let state = Arc::clone(&accept_state);
                state.open_connections.fetch_add(1, Ordering::SeqCst);
                connections.spawn(async move {
                    let io = TokioIo::new(stream);
                    let handler_state = Arc::clone(&state);
                    let service = service_fn(move |req| handle(req, Arc::clone(&handler_state)));
                    // Errors here are clients hanging up mid-response; nothing to report.
                    let _ = http1::Builder::new()
                        .keep_alive(true)
                        .serve_connection(io, service)
                        .await;
                    state.open_connections.fetch_sub(1, Ordering::SeqCst);
                });
                // Reap finished connections so the set does not grow unbounded.
                while connections.try_join_next().is_some() {}
            }
        });
        Ok(Self {
            addr,
            accept_loop,
            state,
        })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// `http://127.0.0.1:<port><path>`; `path` should start with `/`.
    pub fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }

    /// Number of requests received since start.
    pub fn requests_served(&self) -> u64 {
        self.state.requests.load(Ordering::Relaxed)
    }

    /// Connections accepted and not yet closed, including ones parked on
    /// `/api/never`: a cancelled request must bring this back down.
    pub fn open_connections(&self) -> u64 {
        self.state.open_connections.load(Ordering::SeqCst)
    }

    pub fn uptime(&self) -> Duration {
        self.state.started.elapsed()
    }

    /// Stops the server. Equivalent to dropping it.
    pub fn shutdown(self) {
        drop(self);
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        self.accept_loop.abort();
    }
}

/// Response body that can wait before its first frame or stream events forever.
pub enum FixtureBody {
    Once {
        delay: Option<Pin<Box<Sleep>>>,
        data: Option<Bytes>,
    },
    Sse {
        interval: Interval,
        sent: u64,
    },
    /// Head sent, body never arrives and never ends.
    Stall,
}

impl FixtureBody {
    fn bytes(data: impl Into<Bytes>, delay: Duration) -> Self {
        let data: Bytes = data.into();
        FixtureBody::Once {
            delay: (!delay.is_zero()).then(|| Box::pin(tokio::time::sleep(delay))),
            data: (!data.is_empty()).then_some(data),
        }
    }
}

impl Body for FixtureBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        match self.get_mut() {
            FixtureBody::Once { delay, data } => {
                if let Some(sleep) = delay {
                    ready!(sleep.as_mut().poll(cx));
                    *delay = None;
                }
                Poll::Ready(data.take().map(|d| Ok(Frame::data(d))))
            }
            FixtureBody::Sse { interval, sent } => {
                ready!(interval.poll_tick(cx));
                *sent += 1;
                let n = *sent;
                let unix_ms = unix_nanos() / 1_000_000;
                let event = format!("id: {n}\ndata: {{\"n\":{n},\"unix_ms\":{unix_ms}}}\n\n");
                Poll::Ready(Some(Ok(Frame::data(Bytes::from(event)))))
            }
            // The head is already on the wire; the body never produces a frame
            // and never ends. Models a slow origin that sends headers and then
            // stalls, which a cancelled navigation must abort at once rather
            // than wait out (`/api/stall`).
            FixtureBody::Stall => Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        matches!(self, FixtureBody::Once { data: None, .. })
    }

    fn size_hint(&self) -> SizeHint {
        match self {
            FixtureBody::Once { data, .. } => {
                SizeHint::with_exact(data.as_ref().map_or(0, |d| d.len() as u64))
            }
            FixtureBody::Sse { .. } | FixtureBody::Stall => SizeHint::default(),
        }
    }
}

fn unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

async fn handle(
    req: Request<Incoming>,
    state: Arc<State>,
) -> Result<Response<FixtureBody>, Infallible> {
    state.requests.fetch_add(1, Ordering::Relaxed);
    let path = req.uri().path().to_owned();
    if state.log_requests {
        eprintln!("REQ {} {} {}", unix_nanos(), req.method(), path);
    }

    let half_rtt = state.rtt / 2;
    let head_delay = half_rtt
        + state
            .path_delays
            .iter()
            .filter(|rule| rule.pattern.matches(&path))
            .map(|rule| rule.delay)
            .sum::<Duration>();
    if !head_delay.is_zero() {
        tokio::time::sleep(head_delay).await;
    }

    let response = if let Some(api) = path.strip_prefix("/api/") {
        handle_api(api, req, half_rtt).await
    } else {
        serve_file(&state, &path, req.uri().query(), half_rtt).await
    };
    Ok(response)
}

async fn handle_api(
    endpoint: &str,
    req: Request<Incoming>,
    body_delay: Duration,
) -> Response<FixtureBody> {
    match endpoint {
        "echo" => echo(req, body_delay).await,
        "slow" => {
            let millis = query_param(req.uri().query(), "ms")
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(1000);
            tokio::time::sleep(Duration::from_millis(millis)).await;
            json_response(format!("{{\"slept_ms\":{millis}}}"), body_delay)
        }
        "never" => std::future::pending().await,
        "stall" => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/html")
            .body(FixtureBody::Stall)
            .expect("static response"),
        "redirect" => redirect(req.uri().query(), body_delay),
        "cookie" => cookie(req, body_delay),
        "status" => status(req.uri().query(), body_delay),
        "sse" => {
            let first = tokio::time::Instant::now() + body_delay + SSE_INTERVAL;
            let mut interval = tokio::time::interval_at(first, SSE_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/event-stream")
                .header(header::CACHE_CONTROL, "no-cache")
                .body(FixtureBody::Sse { interval, sent: 0 })
                .expect("static response")
        }
        _ => text_response(StatusCode::NOT_FOUND, "no such API endpoint\n", body_delay),
    }
}

/// `/api/redirect?to=<path>&n=<hops>`: `n - 1` self-redirects, then one to
/// `to`. `n` defaults to 1, so `/api/redirect?to=/tiny/` is a single hop.
fn redirect(query: Option<&str>, body_delay: Duration) -> Response<FixtureBody> {
    let target = query_param(query, "to")
        .map(percent_decode)
        .unwrap_or_default();
    if target.is_empty() {
        return text_response(StatusCode::BAD_REQUEST, "redirect needs ?to=\n", body_delay);
    }
    let hops: u32 = query_param(query, "n")
        .and_then(|value| value.parse().ok())
        .unwrap_or(1);
    let status = query_param(query, "status")
        .and_then(|value| value.parse::<u16>().ok())
        .and_then(|value| StatusCode::from_u16(value).ok())
        .unwrap_or(StatusCode::FOUND);
    let location = if hops > 1 {
        format!("/api/redirect?to={target}&n={}", hops - 1)
    } else {
        target
    };
    let Ok(location) = HeaderValue::from_str(&location) else {
        return text_response(
            StatusCode::BAD_REQUEST,
            "invalid redirect target\n",
            body_delay,
        );
    };
    Response::builder()
        .status(status)
        .header(header::LOCATION, location)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(FixtureBody::bytes("redirecting\n", body_delay))
        .expect("static response")
}

/// `/api/status?code=<code>&retry_after=<seconds|http-date>`: answers with any
/// status and, when asked, a `Retry-After`. What a crawler's politeness paths
/// need (`docs/spec/20-platform-behaviour.md` §11): a 429 or 503 that says
/// when to come back.
fn status(query: Option<&str>, body_delay: Duration) -> Response<FixtureBody> {
    let code = query_param(query, "code")
        .and_then(|value| value.parse::<u16>().ok())
        .and_then(|value| StatusCode::from_u16(value).ok())
        .unwrap_or(StatusCode::OK);
    let mut response = Response::builder()
        .status(code)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store");
    if let Some(retry_after) = query_param(query, "retry_after").map(percent_decode)
        && let Ok(value) = HeaderValue::from_str(&retry_after)
    {
        response = response.header(header::RETRY_AFTER, value);
    }
    let body = format!(
        "<!doctype html><title>{code}</title><h1>HTTP {}</h1>",
        code.as_u16()
    );
    response
        .body(FixtureBody::bytes(body, body_delay))
        .expect("static response")
}

/// `/api/cookie?set=<Set-Cookie value>`: sets the cookie and answers with the
/// `Cookie` header the request carried, as JSON.
fn cookie(req: Request<Incoming>, body_delay: Duration) -> Response<FixtureBody> {
    let received = req
        .headers()
        .get(header::COOKIE)
        .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
        .unwrap_or_default();
    let set = query_param(req.uri().query(), "set").map(percent_decode);
    let body = serde_json::json!({ "cookie": received }).to_string();
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "no-store");
    if let Some(set) = set
        && let Ok(value) = HeaderValue::from_str(&set)
    {
        response = response.header(header::SET_COOKIE, value);
    }
    response
        .body(FixtureBody::bytes(body, body_delay))
        .expect("static response")
}

async fn echo(req: Request<Incoming>, body_delay: Duration) -> Response<FixtureBody> {
    let method = req.method().to_string();
    let path = req.uri().path().to_owned();
    let query = req.uri().query().unwrap_or("").to_owned();
    let headers: serde_json::Map<String, serde_json::Value> = req
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_owned(),
                serde_json::Value::String(String::from_utf8_lossy(value.as_bytes()).into_owned()),
            )
        })
        .collect();
    let body = match req.into_body().collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => Bytes::new(),
    };
    let body_text = String::from_utf8_lossy(&body).into_owned();
    let mut reply = serde_json::json!({
        "method": method,
        "path": path,
        "query": query,
        "headers": headers,
        "body": body_text,
    });
    if let Ok(parsed) = serde_json::from_slice::<serde_json::Value>(&body) {
        reply["json"] = parsed;
    }
    json_response(reply.to_string(), body_delay)
}

/// Serves a file from the fixture root. `?content_type=` overrides the type
/// derived from the extension, which is how a fixture is served without a
/// `charset` parameter (so its `<meta charset>` decides) or with one that
/// contradicts the markup.
async fn serve_file(
    state: &State,
    path: &str,
    query: Option<&str>,
    body_delay: Duration,
) -> Response<FixtureBody> {
    let Some(relative) = safe_relative_path(path) else {
        return text_response(StatusCode::FORBIDDEN, "path refused\n", body_delay);
    };
    let mut candidate = state.root.join(&relative);
    if candidate.is_dir() {
        candidate.push("index.html");
    }
    if !candidate.is_file() {
        return text_response(StatusCode::NOT_FOUND, "not found\n", body_delay);
    }
    let Ok(canonical) = state.policy.check_read(&candidate) else {
        return text_response(StatusCode::FORBIDDEN, "path refused\n", body_delay);
    };
    match tokio::fs::read(&canonical).await {
        Ok(contents) => {
            let mut response = Response::new(FixtureBody::bytes(contents, body_delay));
            let content_type = query_param(query, "content_type")
                .map(percent_decode)
                .and_then(|value| HeaderValue::from_str(&value).ok())
                .unwrap_or_else(|| HeaderValue::from_static(content_type_for(&canonical)));
            response
                .headers_mut()
                .insert(header::CONTENT_TYPE, content_type);
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        Err(_) => text_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "read failed\n",
            body_delay,
        ),
    }
}

/// Decodes the request path and refuses anything that could leave the root.
fn safe_relative_path(path: &str) -> Option<PathBuf> {
    let decoded = percent_decode(path);
    if decoded.contains('\0') {
        return None;
    }
    let mut relative = PathBuf::new();
    for segment in decoded.split('/') {
        match segment {
            "" | "." => {}
            ".." => return None,
            s if s.contains('\\') => return None,
            s => relative.push(s),
        }
    }
    Some(relative)
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3])
            && let Ok(value) = u8::from_str_radix(hex, 16)
        {
            out.push(value);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn query_param<'a>(query: Option<&'a str>, key: &str) -> Option<&'a str> {
    query?
        .split('&')
        .filter_map(|pair| pair.split_once('=').or(Some((pair, ""))))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v)
}

fn json_response(body: String, body_delay: Duration) -> Response<FixtureBody> {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "no-store")
        .body(FixtureBody::bytes(body, body_delay))
        .expect("static response")
}

fn text_response(
    status: StatusCode,
    body: &'static str,
    body_delay: Duration,
) -> Response<FixtureBody> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(FixtureBody::bytes(body, body_delay))
        .expect("static response")
}

/// Content type by file extension; unknown extensions are `application/octet-stream`.
pub fn content_type_for(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json",
        "txt" | "md" => "text/plain; charset=utf-8",
        "xml" => "application/xml",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "ogg" | "oga" => "audio/ogg",
        "opus" => "audio/opus",
        "wav" => "audio/wav",
        "wasm" => "application/wasm",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}
