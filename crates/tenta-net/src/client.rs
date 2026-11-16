//! The client and the io runtime it submits work to
//! (`docs/spec/02-architecture.md` §3, `03-network.md` §1).
//!
//! One tokio `current_thread` runtime per io thread owns every socket, the
//! connection pool and the DNS cache. [`Client`] is a `Clone + Send + Sync`
//! handle that submits requests over a channel and awaits the answer, so
//! callers on page threads never touch a connection.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

use bytes::Bytes;
use http::{HeaderMap, HeaderValue, Method, StatusCode, Version};
use tokio::sync::{mpsc, oneshot};
use tokio::task::LocalSet;
use url::Url;

use crate::body::{Body, BodyStream, ContentEncoding, Decoder, OutgoingBody};
use crate::dns::Resolver;
use crate::error::{ErrorName, NetError, ReplayHint};
use crate::h1::{self, H1Handle};
use crate::h2::{self, H2Handle};
use crate::headers::{self, WireProtocol};
use crate::hooks::{Hooks, Interception};
use crate::policy::cache::{
    CacheControl, CacheDecision, CacheRequest, CachedResponse, HttpCache, is_cacheable_method,
    is_cacheable_status, is_unsafe_method,
};
use crate::policy::cookies::{CookieJar, Site};
use crate::policy::private_network::{DestinationPolicy, RebindGuard};
use crate::policy::redact;
use crate::policy::session_cache::SessionCache;
use crate::pool::{Pool, PoolConfig, PoolKey, PoolStats};
use crate::profile::Profile;
use crate::proxy::Proxy;
use crate::request::{FetchSite, RedirectPolicy, Request, check_port, extract_userinfo};
use crate::stream::{self, ConnectTiming, Destination, Destinations};
use crate::tls::{TlsConnector, TlsOptions};

/// Which protocol answered a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    Http11,
    Http2,
    /// Served from the HTTP cache or an interceptor, with no connection.
    Local,
}

impl Protocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Protocol::Http11 => "http/1.1",
            Protocol::Http2 => "h2",
            Protocol::Local => "local",
        }
    }
}

/// Where the time went. A pooled connection reports zero for the phases it
/// skipped, which is what the pool-reuse test asserts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Timing {
    pub dns: Duration,
    pub connect: Duration,
    pub tls: Duration,
    /// From `send()` to the first byte of the response head.
    pub ttfb: Duration,
    /// From `send()` to the response head; the body may still be streaming.
    pub total: Duration,
}

/// One hop in a redirect chain.
#[derive(Clone, Debug)]
pub struct RedirectHop {
    /// The URL that was requested and answered with a 3xx redirect.
    pub url: Url,
    /// The 3xx status code (301, 302, 303, 307, or 308).
    pub status: u16,
    /// The raw Location header value.
    pub location: Option<String>,
    /// Whether this hop was served from the HTTP cache.
    pub from_cache: bool,
}

/// A response head plus its still-streaming body.
#[derive(Debug)]
pub struct Response {
    pub status: StatusCode,
    pub version: Version,
    /// The URL that produced this response, after redirects.
    pub url: Url,
    /// Response headers with `Content-Encoding` and `Content-Length` removed
    /// when the transport decoded the body.
    pub headers: Vec<(String, HeaderValue)>,
    /// Exactly what the server sent, in order.
    pub raw_headers: Vec<(String, HeaderValue)>,
    pub body: BodyStream,
    pub timing: Timing,
    pub remote_addr: Option<std::net::SocketAddr>,
    pub protocol: Protocol,
    pub from_cache: bool,
    /// The redirect chain that led here, oldest first.
    pub redirects: Vec<RedirectHop>,
}

impl Response {
    /// The first value of a header, case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
            .and_then(|(_, value)| value.to_str().ok())
    }

    /// Reads the body to the end.
    pub async fn bytes(self) -> Result<Bytes, NetError> {
        self.body.collect().await
    }

    /// Reads the body to the end as text.
    pub async fn text(self) -> Result<String, NetError> {
        self.body.text().await
    }
}

/// How identical in-flight GETs are shared (`docs/spec/03-network.md` §3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coalescing {
    Off,
    /// Followers wait for the leader's complete body, which is kept in
    /// memory up to `max_bytes`. Beyond that the leader stops sharing and
    /// the followers run their own request.
    Shared {
        max_bytes: u64,
    },
}

impl Default for Coalescing {
    fn default() -> Self {
        Coalescing::Shared {
            max_bytes: 8 * 1024 * 1024,
        }
    }
}

/// The response body limit a client applies when neither the request nor the
/// configuration names one: 64 MiB, the per-request ceiling of
/// `docs/spec/14-memory.md`. It is a limit, not a buffer size; a streaming
/// caller still sees the body chunk by chunk.
pub const DEFAULT_MAX_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;

/// Everything a client needs to know before its first request.
pub struct ClientConfig {
    pub profile: Profile,
    pub tls: TlsOptions,
    pub hooks: Hooks,
    /// The cookie store of this BrowserContext; `None` sends and stores no
    /// cookies at all (`docs/spec/03-network.md` §6).
    pub cookie_jar: Option<CookieJar>,
    /// The HTTP cache; `None` means every request goes to the network
    /// (`docs/spec/03-network.md` §5).
    pub cache: Option<HttpCache>,
    /// DNS answers and TLS tickets that outlive the process
    /// (`docs/spec/03-network.md` §4). Also handed to [`TlsOptions`].
    pub session_cache: SessionCache,
    /// Which destinations may be reached (`docs/spec/19-security.md` §2).
    /// Private-network destinations are refused by default, so a test that
    /// talks to a loopback fixture must allow them.
    pub destination: DestinationPolicy,
    /// Log URLs and headers verbatim instead of redacted. Off by default
    /// (`docs/spec/19-security.md` §7).
    pub log_unredacted: bool,
    pub proxy: Option<Proxy>,
    pub pool: PoolConfig,
    pub connect_timeout: Duration,
    /// Applied when a request carries no deadline of its own.
    pub request_timeout: Option<Duration>,
    /// Response body limit when a request carries none. The default is
    /// [`DEFAULT_MAX_RESPONSE_BYTES`]; a context narrows it from its budgets.
    /// `None` means no limit at all, which no shipping configuration should
    /// use: the decoder's budget is what stops a decompression bomb.
    pub max_response_bytes: Option<u64>,
    /// Isolation unit; part of the pool key.
    pub context_id: u64,
    pub coalescing: Coalescing,
    /// Allow HTTP/2 when ALPN offers it.
    pub allow_h2: bool,
    /// Resolver override; tests use a literal-only resolver.
    pub resolver: Option<Arc<Resolver>>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            profile: Profile::None,
            tls: TlsOptions::default(),
            hooks: Hooks::default(),
            cookie_jar: None,
            cache: None,
            session_cache: SessionCache::ephemeral(),
            destination: DestinationPolicy::default(),
            log_unredacted: false,
            proxy: None,
            pool: PoolConfig::default(),
            connect_timeout: Duration::from_secs(30),
            request_timeout: Some(Duration::from_secs(120)),
            max_response_bytes: Some(DEFAULT_MAX_RESPONSE_BYTES),
            context_id: 0,
            coalescing: Coalescing::default(),
            allow_h2: true,
            resolver: None,
        }
    }
}

impl std::fmt::Debug for ClientConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientConfig")
            .field("profile", &self.profile)
            .field("proxy", &self.proxy)
            .field("cookies", &self.cookie_jar.is_some())
            .field("cache", &self.cache.is_some())
            .field("destination", &self.destination)
            .field("context_id", &self.context_id)
            .field("allow_h2", &self.allow_h2)
            .finish_non_exhaustive()
    }
}

/// The state a client shares with the io thread.
pub(crate) struct ClientShared {
    id: u64,
    profile: Profile,
    hooks: Hooks,
    cookies: Option<CookieJar>,
    cache: Option<HttpCache>,
    session_cache: SessionCache,
    destination: DestinationPolicy,
    rebind: RebindGuard,
    log_unredacted: bool,
    proxy: Option<Proxy>,
    connector: TlsConnector,
    resolver: Arc<Resolver>,
    pool: PoolConfig,
    connect_timeout: Duration,
    request_timeout: Option<Duration>,
    max_response_bytes: Option<u64>,
    context_id: u64,
    coalescing: Coalescing,
    allow_h2: bool,
    has_observers: AtomicBool,
}

impl ClientShared {
    /// The destination rules, bound to this client's rebinding guard.
    fn destinations(&self) -> Destinations<'_> {
        Destinations {
            policy: &self.destination,
            rebind: &self.rebind,
        }
    }

    /// A URL as it may appear in a log line: userinfo and query are dropped
    /// unless the client was explicitly told to log verbatim
    /// (`docs/spec/19-security.md` §7).
    fn log_url(&self, url: &Url) -> String {
        if self.log_unredacted {
            url.to_string()
        } else {
            redact::url_for_log(url)
        }
    }

    /// Headers as they may appear in a log line; `Cookie`, `Authorization`
    /// and friends are replaced by their length.
    fn log_headers(&self, headers: &[(String, HeaderValue)]) -> Vec<String> {
        headers
            .iter()
            .map(|(name, value)| {
                if self.log_unredacted {
                    format!("{name}: {}", String::from_utf8_lossy(value.as_bytes()))
                } else {
                    redact::header_for_log(name, value.as_bytes())
                }
            })
            .collect()
    }

    /// Writes the session cache to disk. Cheap and a no-op when ephemeral.
    /// Writes the session cache out. `File::create` + `sync_all` + `rename` is
    /// not something the connection runtime should block on, so a cache backed
    /// by a file is flushed on a blocking thread.
    fn flush_session_cache(&self) {
        let cache = self.session_cache.clone();
        let flush = move || {
            if let Err(error) = cache.flush(SystemTime::now()) {
                tracing::debug!(%error, "flushing the session cache");
            }
        };
        if self.session_cache.writes_block() && tokio::runtime::Handle::try_current().is_ok() {
            tokio::task::spawn_blocking(flush);
        } else {
            flush();
        }
    }

    /// The `Cookie` header value for `request`, from this context's jar.
    fn cookie_header(&self, request: &Request) -> Option<String> {
        if !request.sends_credentials() {
            return None;
        }
        self.cookies.as_ref()?.cookie_header(
            &request.url,
            request.first_party.as_ref(),
            true,
            request.same_site_context(),
            SystemTime::now(),
        )
    }

    /// Hands every `Set-Cookie` of one hop to the jar.
    fn store_cookies(&self, request: &Request, headers: &[(String, HeaderValue)]) {
        if !request.sends_credentials() {
            return;
        }
        let Some(jar) = &self.cookies else { return };
        let now = SystemTime::now();
        for (name, value) in headers {
            if name.eq_ignore_ascii_case("set-cookie") {
                jar.set_cookie_in(
                    value.as_bytes(),
                    &request.url,
                    true,
                    now,
                    request.first_party.as_ref(),
                );
            }
        }
    }
}

enum Command {
    Send {
        shared: Arc<ClientShared>,
        request: Box<Request>,
        reply: oneshot::Sender<Result<Response, NetError>>,
    },
    Preconnect {
        shared: Arc<ClientShared>,
        url: Url,
        reply: oneshot::Sender<Result<(), NetError>>,
    },
    CloseClient {
        id: u64,
    },
    PoolStats {
        id: u64,
        reply: oneshot::Sender<PoolStats>,
    },
    Shutdown,
}

/// The io threads (`EngineOptions.io_threads`).
///
/// Each thread runs one `current_thread` runtime; a client is pinned to one
/// of them for its lifetime, because connections cannot move between
/// runtimes.
pub struct NetRuntime {
    threads: Vec<IoThread>,
    next_client: AtomicU64,
    next_thread: AtomicU64,
}

struct IoThread {
    commands: mpsc::UnboundedSender<Command>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl std::fmt::Debug for NetRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetRuntime")
            .field("io_threads", &self.threads.len())
            .finish()
    }
}

impl NetRuntime {
    /// Starts `io_threads` runtimes.
    pub fn new(io_threads: usize) -> Result<Arc<Self>, NetError> {
        let io_threads = io_threads.max(1);
        let mut threads = Vec::with_capacity(io_threads);
        for index in 0..io_threads {
            let (commands, receiver) = mpsc::unbounded_channel();
            let join = std::thread::Builder::new()
                .name(format!("tenta-io-{index}"))
                .spawn(move || io_thread_main(receiver))
                .map_err(|err| {
                    NetError::new(
                        ErrorName::InsufficientResources,
                        format!("spawning io thread: {err}"),
                    )
                })?;
            threads.push(IoThread {
                commands,
                join: Some(join),
            });
        }
        Ok(Arc::new(Self {
            threads,
            next_client: AtomicU64::new(1),
            next_thread: AtomicU64::new(0),
        }))
    }

    pub fn io_threads(&self) -> usize {
        self.threads.len()
    }

    /// Builds a client pinned to one io thread.
    pub fn client(self: &Arc<Self>, mut config: ClientConfig) -> Result<Client, NetError> {
        // One session cache serves both halves of a cold start: the DNS
        // answers in front of the resolver and the TLS tickets in the
        // connector.
        // BLOCKING-2: Never share sessions from ignore_https_errors contexts.
        // A connector with verification off gets an ephemeral cache that never
        // touches the persistent store.
        if config.tls.ignore_https_errors {
            config.tls.session_cache = SessionCache::ephemeral();
        } else {
            config.tls.session_cache = config.session_cache.clone();
        }
        // A client that will not speak HTTP/2 must not offer it: ALPN is a
        // promise, and a server that takes it would be answered in HTTP/1.1.
        if !config.allow_h2 && config.tls.alpn_override.is_none() {
            config.tls.alpn_override = Some(vec!["http/1.1".to_owned()]);
        }
        let connector = TlsConnector::new(&config.profile, config.tls.clone())?;
        let resolver = match config.resolver.take() {
            Some(resolver) => resolver,
            None => Arc::new(Resolver::system(config.session_cache.clone())),
        };
        let id = self.next_client.fetch_add(1, Ordering::Relaxed);
        let thread =
            (self.next_thread.fetch_add(1, Ordering::Relaxed) as usize) % self.threads.len();
        let shared = Arc::new(ClientShared {
            id,
            has_observers: AtomicBool::new(!config.hooks.observers.is_empty()),
            profile: config.profile,
            hooks: config.hooks,
            cookies: config.cookie_jar,
            cache: config.cache,
            session_cache: config.session_cache,
            destination: config.destination,
            rebind: RebindGuard::new(),
            log_unredacted: config.log_unredacted,
            proxy: config.proxy,
            connector,
            resolver,
            pool: config.pool,
            connect_timeout: config.connect_timeout,
            request_timeout: config.request_timeout,
            max_response_bytes: config.max_response_bytes,
            context_id: config.context_id,
            coalescing: config.coalescing,
            allow_h2: config.allow_h2,
        });
        Ok(Client {
            shared,
            commands: self.threads[thread].commands.clone(),
            runtime: Arc::clone(self),
            handle_token: Arc::new(()),
        })
    }
}

impl Drop for NetRuntime {
    fn drop(&mut self) {
        for thread in &mut self.threads {
            let _ = thread.commands.send(Command::Shutdown);
        }
        for thread in &mut self.threads {
            if let Some(join) = thread.join.take() {
                let _ = join.join();
            }
        }
    }
}

/// A handle to the transport. Cloning is cheap and every clone talks to the
/// same io thread, pool and cookie jar.
#[derive(Clone)]
pub struct Client {
    shared: Arc<ClientShared>,
    commands: mpsc::UnboundedSender<Command>,
    #[allow(dead_code)]
    runtime: Arc<NetRuntime>,
    /// Token shared only by Client clones; when all handles are dropped,
    /// its strong count becomes 1 and the client is closed.
    handle_token: Arc<()>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("id", &self.shared.id)
            .field("profile", &self.shared.profile)
            .finish_non_exhaustive()
    }
}

impl Client {
    /// Sends a request and waits for its head. The body may still be
    /// streaming when this returns.
    pub async fn send(&self, request: Request) -> Result<Response, NetError> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(Command::Send {
                shared: Arc::clone(&self.shared),
                request: Box::new(request),
                reply,
            })
            .map_err(|_| NetError::new(ErrorName::Aborted, "io thread is gone"))?;
        answer
            .await
            .map_err(|_| NetError::new(ErrorName::Aborted, "request was dropped"))?
    }

    /// Convenience for `GET url`, reading the whole body.
    pub async fn get(&self, url: &str) -> Result<Response, NetError> {
        self.send(Request::get(url)?).await
    }

    /// Opens (and keeps) a connection to `url`'s host, TLS and HTTP/2
    /// included, so a later request pays no handshake
    /// (`docs/spec/03-network.md` §3).
    pub async fn preconnect(&self, url: &str) -> Result<(), NetError> {
        let url = crate::request::parse_url(url)?;
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(Command::Preconnect {
                shared: Arc::clone(&self.shared),
                url,
                reply,
            })
            .map_err(|_| NetError::new(ErrorName::Aborted, "io thread is gone"))?;
        answer
            .await
            .map_err(|_| NetError::new(ErrorName::Aborted, "preconnect was dropped"))?
    }

    /// What the pool currently holds; used by tests and diagnostics.
    pub async fn pool_stats(&self) -> PoolStats {
        let (reply, answer) = oneshot::channel();
        if self
            .commands
            .send(Command::PoolStats {
                id: self.shared.id,
                reply,
            })
            .is_err()
        {
            return PoolStats::default();
        }
        answer.await.unwrap_or_default()
    }

    /// The resolver, so a caller can pre-populate or clear the DNS cache.
    pub fn resolver(&self) -> &Arc<Resolver> {
        &self.shared.resolver
    }

    pub fn profile(&self) -> &Profile {
        &self.shared.profile
    }

    /// Drops this client's pool on the io thread. The handle stays usable;
    /// the next request opens fresh connections.
    pub fn close(&self) {
        let _ = self
            .commands
            .send(Command::CloseClient { id: self.shared.id });
    }
}

impl Drop for Client {
    /// Dropping the last handle closes the client.
    ///
    /// The pool, its live HTTP/2 connections and their keep-alive ping tasks
    /// live in the io thread's map, keyed by client id; without this they
    /// would outlive every handle to them for the life of the runtime.
    /// `handle_token` is held only by Client clones; when all are dropped,
    /// its strong count reaches one and the client is closed.
    fn drop(&mut self) {
        if Arc::strong_count(&self.handle_token) == 1 {
            self.close();
        }
    }
}

/// Per-client state that lives on the io thread and never crosses it.
struct ClientRuntime {
    shared: Arc<ClientShared>,
    pool: RefCell<Pool>,
    in_flight: RefCell<HashMap<String, Vec<oneshot::Sender<CoalesceResult>>>>,
}

/// What a coalescing follower receives.
type CoalesceResult = Result<Arc<SharedResponse>, NetError>;

/// Owns a leader's coalescing registration for the window between registering
/// the key and the body pump adopting it. If the leader's future is dropped in
/// that window (a cancelled navigation, a dropped `send` future), the guard
/// removes the key and drops the followers' senders; a follower treats a closed
/// channel as "run your own request", so it proceeds instead of waiting forever
/// on a leader that will never answer (BLOCKING-1).
struct LeaderGuard {
    state: Rc<ClientRuntime>,
    key: String,
    armed: bool,
}

impl LeaderGuard {
    fn new(state: Rc<ClientRuntime>, key: String) -> Self {
        Self {
            state,
            key,
            armed: true,
        }
    }

    /// The pump has adopted the key; the guard must no longer touch it.
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for LeaderGuard {
    fn drop(&mut self) {
        if self.armed {
            // Dropping the senders (rather than sending an error) is what makes
            // each follower fall through to its own request.
            let _ = self.state.in_flight.borrow_mut().remove(&self.key);
        }
    }
}

/// The leader's complete response, shared with its followers.
struct SharedResponse {
    status: StatusCode,
    version: Version,
    url: Url,
    headers: Vec<(String, HeaderValue)>,
    raw_headers: Vec<(String, HeaderValue)>,
    timing: Timing,
    remote_addr: Option<std::net::SocketAddr>,
    protocol: Protocol,
    redirects: Vec<RedirectHop>,
    body: Bytes,
}

fn io_thread_main(mut commands: mpsc::UnboundedReceiver<Command>) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            tracing::error!(error = %err, "io thread runtime");
            return;
        }
    };
    let local = LocalSet::new();
    local.block_on(&runtime, async move {
        let clients: Rc<RefCell<HashMap<u64, Rc<ClientRuntime>>>> =
            Rc::new(RefCell::new(HashMap::new()));
        while let Some(command) = commands.recv().await {
            match command {
                Command::Send {
                    shared,
                    request,
                    mut reply,
                } => {
                    let state = client_state(&clients, shared);
                    tokio::task::spawn_local(async move {
                        // The caller dropping its future cancels the request:
                        // the connection it was waiting on is closed, not
                        // parked until the server answers.
                        let execute = execute(state, *request);
                        tokio::pin!(execute);
                        tokio::select! {
                            _ = reply.closed() => {}
                            result = &mut execute => {
                                let _ = reply.send(result);
                            }
                        }
                    });
                }
                Command::Preconnect { shared, url, reply } => {
                    let state = client_state(&clients, shared);
                    tokio::task::spawn_local(async move {
                        let _ = reply.send(preconnect(state, url).await);
                    });
                }
                Command::CloseClient { id } => {
                    if let Some(state) = clients.borrow_mut().remove(&id) {
                        state.pool.borrow_mut().clear();
                    }
                }
                Command::PoolStats { id, reply } => {
                    let stats = clients
                        .borrow()
                        .get(&id)
                        .map(|state| {
                            state.pool.borrow_mut().purge_idle();
                            state.pool.borrow().stats()
                        })
                        .unwrap_or_default();
                    let _ = reply.send(stats);
                }
                Command::Shutdown => break,
            }
        }
    });
}

fn client_state(
    clients: &Rc<RefCell<HashMap<u64, Rc<ClientRuntime>>>>,
    shared: Arc<ClientShared>,
) -> Rc<ClientRuntime> {
    let mut clients = clients.borrow_mut();
    Rc::clone(clients.entry(shared.id).or_insert_with(|| {
        if !shared.session_cache.is_ephemeral() {
            // The session cache is only useful to the next process, so it is
            // written periodically rather than on every answer.
            let session = Arc::clone(&shared);
            tokio::task::spawn_local(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    if Arc::strong_count(&session) == 1 {
                        return;
                    }
                    session.flush_session_cache();
                }
            });
        }
        Rc::new(ClientRuntime {
            pool: RefCell::new(Pool::new(shared.pool)),
            in_flight: RefCell::new(HashMap::new()),
            shared,
        })
    }))
}

/// Opens a connection and leaves it in the pool.
async fn preconnect(state: Rc<ClientRuntime>, url: Url) -> Result<(), NetError> {
    let destination = Destination::from_url(&url)?;
    let shared = Arc::clone(&state.shared);
    shared.destinations().check_url(&url)?;
    let key = PoolKey::new(
        &destination,
        shared.proxy.as_ref(),
        shared.profile.name(),
        shared.context_id,
    );
    if state.pool.borrow_mut().h2(&key).is_some() {
        return Ok(());
    }
    let opened = stream::open(
        &destination,
        shared.proxy.as_ref(),
        &shared.resolver,
        &shared.connector,
        shared.destinations(),
        &url,
        shared.connect_timeout,
    )
    .await?;
    let use_h2 = shared.allow_h2 && opened.alpn.as_deref() == Some("h2");
    if use_h2 {
        let handle =
            h2::handshake(opened.stream, &shared.profile, shared.pool.h2_keep_alive).await?;
        state.pool.borrow_mut().put_h2(&key, handle);
    } else {
        let handle =
            h1::handshake(opened.stream, shared.profile.client_profile().is_some()).await?;
        state.pool.borrow_mut().put_h1(&key, handle);
    }
    Ok(())
}

/// Runs one request through interception, the cache, redirects and the pool.
async fn execute(state: Rc<ClientRuntime>, mut request: Request) -> Result<Response, NetError> {
    let started = Instant::now();
    let shared = Arc::clone(&state.shared);
    let deadline = request
        .limits
        .total
        .or(shared.request_timeout)
        .map(|budget| started + budget);
    let mut redirects: Vec<RedirectHop> = Vec::new();
    let max_redirects = match request.redirect {
        RedirectPolicy::Follow { max } => max,
        _ => 0,
    };

    // What the cache is revalidating, if this hop carries conditional
    // headers it did not ask for itself, and which names those are.
    let mut revalidating: Option<CachedResponse> = None;
    let mut conditional_names: Vec<String> = Vec::new();

    // BLOCKING-4: Strip userinfo at execute entry. Credentials must not reach
    // the wire; they are stored separately for 401 Basic challenges.
    if request.userinfo.is_none() {
        request.userinfo = extract_userinfo(request.url.as_str());
    }
    let _ = request.url.set_username("");
    let _ = request.url.set_password(None);

    loop {
        // Scheme, port and host name are checked before DNS, and again for
        // every redirect target.
        shared.destinations().check_url(&request.url)?;

        let before_hooks = request.url.clone();
        let mut fulfilled = None;
        for interceptor in &shared.hooks.interceptors {
            match interceptor.intercept(&mut request).await {
                Interception::Continue => {}
                Interception::Fail(err) => return Err(err),
                Interception::Fulfill(response) => {
                    fulfilled = Some(response);
                    break;
                }
            }
        }
        if fulfilled.is_none()
            && let Some(tap) = request.tap.clone()
        {
            match tap.intercept(&mut request).await {
                Interception::Continue => {}
                Interception::Fail(err) => return Err(err),
                Interception::Fulfill(response) => fulfilled = Some(response),
            }
        }

        if let Some(response) = fulfilled {
            // An answer a hook produced is a response like any other: its
            // cookies are stored and a redirect in it is followed.
            let response = local_response(
                &request.url,
                response.status,
                response.headers,
                response.body,
                started,
                redirects.clone(),
                false,
            );
            shared.store_cookies(&request, &response.headers);
            let policy = request.redirect;
            if let Some(next_request) = check_and_follow_redirect_hop(
                request,
                &response,
                policy,
                max_redirects,
                &mut redirects,
                false,
            )? {
                request = next_request;
                continue;
            }
            return Ok(response);
        }

        // A hook that changed the URL is a new destination: the policy and the
        // credential stripping of the loop head apply to it too, before the
        // cache can answer for it.
        if request.url != before_hooks {
            shared.destinations().check_url(&request.url)?;
            if request.userinfo.is_none() {
                request.userinfo = extract_userinfo(request.url.as_str());
            }
            let _ = request.url.set_username("");
            let _ = request.url.set_password(None);
        }

        if let Some(cache) = &shared.cache
            && !request.bypass_cache_lookup
        {
            let request_headers = headers::to_header_map(&request.headers);
            let cache_request =
                CacheRequest::new(request.method.as_str(), &request.url, &request_headers);
            match cache.lookup(&cache_request, SystemTime::now()) {
                CacheDecision::Fresh(cached) => {
                    // BLOCKING-3: Apply redirect logic to cache hits, not just
                    // network responses. A cached 301/308 must be followed.
                    let response =
                        cached_response(&request.url, cached, started, redirects.clone());
                    let policy = request.redirect;
                    if let Some(next_request) = check_and_follow_redirect_hop(
                        request,
                        &response,
                        policy,
                        max_redirects,
                        &mut redirects,
                        true,
                    )? {
                        request = next_request;
                        continue;
                    }
                    return Ok(response);
                }
                CacheDecision::Stale {
                    cached,
                    conditional_headers,
                } if cached.servable_stale => {
                    // `stale-while-revalidate` (RFC 5861 §3) and request
                    // `max-stale`: the caller gets the stored body now and the
                    // refresh runs behind it, so the window costs no latency.
                    spawn_revalidation(&state, &request, &conditional_headers);
                    // BLOCKING-3: Check if the stale cached response is a redirect.
                    let response =
                        cached_response(&request.url, cached, started, redirects.clone());
                    let policy = request.redirect;
                    if let Some(next_request) = check_and_follow_redirect_hop(
                        request,
                        &response,
                        policy,
                        max_redirects,
                        &mut redirects,
                        true,
                    )? {
                        request = next_request;
                        continue;
                    }
                    return Ok(response);
                }
                CacheDecision::Stale {
                    cached,
                    conditional_headers,
                } => {
                    for (name, value) in conditional_headers.iter() {
                        request.headers.retain(|(candidate, _)| {
                            !candidate.eq_ignore_ascii_case(name.as_str())
                        });
                        request
                            .headers
                            .push((name.as_str().to_owned(), value.clone()));
                        // Remembered so the next hop does not inherit the
                        // validators of the resource this one revalidated.
                        conditional_names.push(name.as_str().to_owned());
                    }
                    revalidating = Some(cached);
                }
                CacheDecision::OnlyIfCachedMiss => {
                    return Err(NetError::new(
                        ErrorName::CacheMiss,
                        "only-if-cached and nothing usable is stored",
                    ));
                }
                CacheDecision::Miss => {}
            }
        }

        if shared.has_observers.load(Ordering::Relaxed) {
            for observer in &shared.hooks.observers {
                observer.on_request(&request);
            }
        }

        // Owns this hop's coalescing registration while it is the leader. If
        // the leader's future is dropped before the body pump takes the key
        // over (a cancelled `goto`, `close()`, a dropped `send` future — all
        // within one head round-trip), the guard removes the key and drops the
        // waiters' senders so every follower runs its own request instead of
        // waiting forever on a leader that no longer exists (BLOCKING-1).
        let mut leader_guard: Option<LeaderGuard> = None;
        // A conditional request (a cache revalidation) must not coalesce: two
        // revalidations of one stale entry carry the same validators and so
        // share a key, and the leader's 304 — which belongs to the cache, not
        // the caller — would be handed to the follower as an empty-bodied 304
        // instead of the revalidated 200 (MAJOR-2). Each revalidation runs on
        // its own; the second reads the entry the first refreshed.
        let coalesce_key = if revalidating.is_some() {
            None
        } else {
            coalesce_key(&shared, &request, shared.cookie_header(&request).as_deref())
        };
        if let Some(key) = &coalesce_key {
            // Limit the number of in-flight coalescing followers to prevent unbounded growth.
            // Check the limit BEFORE registering to avoid racing with the leader.
            const MAX_COALESCE_WAITERS: usize = 10000;
            let in_flight_count: usize = state
                .in_flight
                .borrow()
                .values()
                .map(|waiters| waiters.len())
                .sum();

            let follower = if in_flight_count >= MAX_COALESCE_WAITERS {
                // Too many in-flight followers; bypass coalescing.
                None
            } else {
                let mut in_flight = state.in_flight.borrow_mut();
                match in_flight.get_mut(key) {
                    Some(waiters) => {
                        let (tx, rx) = oneshot::channel();
                        waiters.push(tx);
                        Some(rx)
                    }
                    None => {
                        in_flight.insert(key.clone(), Vec::new());
                        // We are the leader for this key until the pump adopts
                        // it in `finish`.
                        leader_guard = Some(LeaderGuard::new(Rc::clone(&state), key.clone()));
                        None
                    }
                }
            };

            if let Some(follower) = follower {
                // A follower keeps its own deadline: coalescing may never make
                // a request wait longer than running it alone would (MAJOR-1).
                let follower_result =
                    match with_deadline(deadline, follower, "waiting for a coalesced response")
                        .await
                    {
                        Ok(inner) => inner,
                        Err(timeout) => return Err(timeout),
                    };
                match follower_result {
                    Ok(Ok(shared)) => {
                        // Check if the shared body exceeds this request's limit.
                        let use_shared = request
                            .limits
                            .max_body_bytes
                            .map(|limit| shared.body.len() as u64 <= limit)
                            .unwrap_or(true);
                        if use_shared {
                            let mut response = shared.to_response(started);
                            // A shared response can be a redirect, and this
                            // request has its own chain to follow and record.
                            let policy = request.redirect;
                            if let Some(next_request) = check_and_follow_redirect_hop(
                                request,
                                &response,
                                policy,
                                max_redirects,
                                &mut redirects,
                                true,
                            )? {
                                request = next_request;
                                continue;
                            }
                            if response.status.is_redirection() && policy == RedirectPolicy::Error {
                                return Err(NetError::new(
                                    ErrorName::UnsafeRedirect,
                                    "redirects are refused by policy",
                                ));
                            }
                            response.redirects = redirects;
                            return Ok(response);
                        }
                        // Body too large; fall through to make our own request.
                    }
                    Ok(Err(err)) => return Err(err),
                    // The leader could not share (its body was too large, or
                    // it went away): run our own request.
                    Err(_) => {}
                }
            }
        }

        // The body is moved onto the wire; `replay` is what a redirect can
        // send again (never a stream, which cannot be rewound).
        let (body, mut replay) = split_body(std::mem::take(&mut request.body));
        let reused = Cell::new(false);
        let mut attempt = send_once(
            &state,
            &request,
            body,
            deadline,
            coalesce_key.clone(),
            &reused,
        )
        .await;

        // A remote HTTP/2 REFUSED_STREAM or GOAWAY guarantees that a stream
        // was not processed, even for a POST. A failed ready() call never
        // opened a stream. Other transport failures can happen after a server
        // applied a request, so only idempotent methods may be replayed then.
        // A replayable body is required in every case and only one retry is
        // attempted on this hop.
        let safe_to_replay = reused.get()
            && attempt.as_ref().err().is_some_and(|err| {
                err.replay_hint() != ReplayHint::None
                    || (err.retryable() && is_idempotent_method(&request.method))
            });
        if safe_to_replay && let Some(retry_body) = replay.take() {
            let (body, again) = split_body(retry_body);
            replay = again;
            attempt = send_once(
                &state,
                &request,
                body,
                deadline,
                coalesce_key.clone(),
                &reused,
            )
            .await;
        }
        let outcome = match attempt {
            Ok(outcome) => outcome,
            Err(err) => {
                tracing::debug!(
                    url = %shared.log_url(&request.url),
                    error = %err,
                    "request failed"
                );
                // A leader that fails before the head must not hand its error to
                // its followers: on a reused connection the failure often
                // evicted the socket (a silent-peer HTTP/2 timeout, a stale
                // keep-alive), so a follower's own request would open a fresh
                // one and succeed — coalescing may never make a request less
                // likely to complete than running it alone. Returning here
                // drops the still-armed `LeaderGuard`, which removes the key
                // and drops the waiters' senders so each follower runs its own
                // request (consistent with the cancel path and the body pump).
                return Err(err);
            }
        };
        // The head arrived and `finish` handed the coalescing key to the body
        // pump, which now owns removing it: the leader guard has nothing left
        // to clean up.
        if let Some(mut guard) = leader_guard.take() {
            guard.disarm();
        }

        // A 304 belongs to the cache, not to the caller: it merges into the
        // stored entry, which is what is returned.
        if outcome.status == StatusCode::NOT_MODIFIED
            && let (Some(cache), Some(cached)) = (&shared.cache, revalidating.take())
        {
            let request_headers = headers::to_header_map(&request.headers);
            let cache_request =
                CacheRequest::new(request.method.as_str(), &request.url, &request_headers);
            let merged = cache.revalidated(
                &cache_request,
                &cached,
                &headers::to_header_map(&outcome.raw_headers),
                SystemTime::now(),
            );
            return Ok(cached_response(&request.url, merged, started, redirects));
        }

        let Some(target) = attempt_redirect_target(request.redirect, &outcome, &request.url)?
        else {
            if outcome.status.is_redirection() && request.redirect == RedirectPolicy::Error {
                return Err(NetError::new(
                    ErrorName::UnsafeRedirect,
                    "redirects are refused by policy",
                ));
            }
            return Ok(outcome.into_response(started, redirects));
        };

        if redirects.len() as u32 >= max_redirects {
            return Err(NetError::new(
                ErrorName::TooManyRedirects,
                format!("more than {max_redirects} redirects"),
            ));
        }
        // The destination policy has its own redirect limit.
        shared
            .destination
            .check_redirect(redirects.len() + 1)
            .map_err(NetError::from_policy)?;
        // The next hop is a different resource: the conditional headers this
        // one borrowed from the cache do not belong to it.
        revalidating = None;
        if !conditional_names.is_empty() {
            request.headers.retain(|(name, _)| {
                !conditional_names
                    .iter()
                    .any(|added| added.eq_ignore_ascii_case(name))
            });
            conditional_names.clear();
        }
        let Some(replay) = replay else {
            return Err(NetError::new(
                ErrorName::InvalidRedirect,
                "a streaming request body cannot be replayed on a redirect",
            ));
        };
        request.body = replay;
        let location = outcome
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("location"))
            .and_then(|(_, value)| value.to_str().ok())
            .map(|s| s.to_owned());
        redirects.push(RedirectHop {
            url: request.url.clone(),
            status: outcome.status.as_u16(),
            location,
            from_cache: false,
        });

        // The keep-alive socket carries the next hop only once this body is
        // read (the pump returns the handle to the pool as it finishes), so
        // a small redirect body is drained first, as Chrome does.
        drain_redirect_body(outcome.body).await;

        request = follow_redirect(request, &outcome.status, target)?;
        if let (Some(tap), Some(hop)) = (&request.tap, redirects.last()) {
            tap.redirected(&request, hop, &outcome.headers);
        }
    }
}

/// Reads a 3xx body to its end when it is small, so the connection goes
/// back to the pool before the next hop instead of after it; a larger body
/// is dropped and the connection closes with it (Chrome drains at most its
/// 16 KiB read buffer before giving up on the socket).
async fn drain_redirect_body(mut body: BodyStream) {
    const MAX_DRAINED_BYTES: usize = 16 * 1024;
    let mut drained = 0;
    while let Some(chunk) = body.next_chunk().await {
        match chunk {
            Ok(chunk) => {
                drained += chunk.len();
                if drained > MAX_DRAINED_BYTES {
                    return;
                }
            }
            Err(_) => return,
        }
    }
}

/// Starts the background refresh a `stale-while-revalidate` hit owes the
/// cache. Failures are logged and dropped: the caller already has an answer.
fn spawn_revalidation(
    state: &Rc<ClientRuntime>,
    request: &Request,
    conditional_headers: &http::HeaderMap,
) {
    let mut refresh = request.without_body();
    // The refresh must reach the network, and must not be joined to the
    // request that started it.
    refresh.bypass_cache_lookup = true;
    refresh.allow_coalescing = false;
    for (name, value) in conditional_headers.iter() {
        refresh
            .headers
            .retain(|(candidate, _)| !candidate.eq_ignore_ascii_case(name.as_str()));
        refresh
            .headers
            .push((name.as_str().to_owned(), value.clone()));
    }

    let state = Rc::clone(state);
    tokio::task::spawn_local(async move {
        // Boxed so this future does not contain `execute`'s own type.
        let attempt: std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<Response, NetError>>>,
        > = Box::pin(execute(state, refresh));
        match attempt.await {
            // The entry is stored when the body has been read to the end.
            Ok(response) => {
                if let Err(error) = response.bytes().await {
                    tracing::debug!(%error, "background revalidation body failed");
                }
            }
            Err(error) => tracing::debug!(%error, "background revalidation failed"),
        }
    });
}

/// What one attempt produced.
struct Attempt {
    status: StatusCode,
    version: Version,
    url: Url,
    headers: Vec<(String, HeaderValue)>,
    raw_headers: Vec<(String, HeaderValue)>,
    body: BodyStream,
    timing: Timing,
    remote_addr: Option<std::net::SocketAddr>,
    protocol: Protocol,
}

impl Attempt {
    fn into_response(self, started: Instant, redirects: Vec<RedirectHop>) -> Response {
        let mut timing = self.timing;
        timing.total = started.elapsed();
        Response {
            status: self.status,
            version: self.version,
            url: self.url,
            headers: self.headers,
            raw_headers: self.raw_headers,
            body: self.body,
            timing,
            remote_addr: self.remote_addr,
            protocol: self.protocol,
            from_cache: false,
            redirects,
        }
    }
}

impl SharedResponse {
    fn to_response(&self, started: Instant) -> Response {
        let mut timing = self.timing;
        timing.total = started.elapsed();
        Response {
            status: self.status,
            version: self.version,
            url: self.url.clone(),
            headers: self.headers.clone(),
            raw_headers: self.raw_headers.clone(),
            body: BodyStream::once(self.body.clone()),
            timing,
            remote_addr: self.remote_addr,
            protocol: self.protocol,
            from_cache: false,
            redirects: self.redirects.clone(),
        }
    }
}

fn local_response(
    url: &Url,
    status: StatusCode,
    headers: Vec<(String, HeaderValue)>,
    body: Bytes,
    started: Instant,
    redirects: Vec<RedirectHop>,
    from_cache: bool,
) -> Response {
    Response {
        status,
        version: Version::HTTP_11,
        url: url.clone(),
        raw_headers: headers.clone(),
        headers,
        body: BodyStream::once(body),
        timing: Timing {
            total: started.elapsed(),
            ..Timing::default()
        },
        remote_addr: None,
        protocol: Protocol::Local,
        from_cache,
        redirects,
    }
}

/// A stored response as the caller sees it: no connection, no timing, and
/// the `Age` the cache computed (RFC 9111 §5.1).
fn cached_response(
    url: &Url,
    cached: CachedResponse,
    started: Instant,
    redirects: Vec<RedirectHop>,
) -> Response {
    let mut headers = header_pairs(&cached.headers);
    headers.retain(|(name, _)| !name.eq_ignore_ascii_case("age"));
    headers.push((
        "age".to_owned(),
        HeaderValue::from(cached.age.as_secs().min(u64::from(u32::MAX))),
    ));
    Response {
        status: StatusCode::from_u16(cached.status).unwrap_or(StatusCode::OK),
        version: Version::HTTP_11,
        url: url.clone(),
        raw_headers: headers.clone(),
        headers,
        body: BodyStream::once(cached.body),
        timing: Timing {
            total: started.elapsed(),
            ..Timing::default()
        },
        remote_addr: None,
        protocol: Protocol::Local,
        from_cache: true,
        redirects,
    }
}

/// The key two in-flight requests must agree on before one may be answered
/// with the other's response.
///
/// It is the whole request as it would go on the wire — method, URL and every
/// header `headers::build` produces, including the `Cookie` the jar computed
/// and everything the fetch context derives (`Range`, `Authorization`,
/// `Accept`, `Sec-Fetch-*`, `Origin`) — plus the first-party site, which
/// selects the cookie partition. Anything less shares a body between requests
/// that asked different questions.
fn coalesce_key(shared: &ClientShared, request: &Request, cookie: Option<&str>) -> Option<String> {
    match shared.coalescing {
        Coalescing::Off => None,
        Coalescing::Shared { .. } => {
            if !request.is_coalescable() {
                return None;
            }
            let destination = Destination::from_url(&request.url).ok()?;
            let built = headers::build(
                &shared.profile,
                request,
                &destination,
                WireProtocol::Http2,
                cookie.map(str::to_owned),
                None,
            );
            let mut key = format!("{} {}", request.method, request.url);
            if let Some(site) = &request.first_party {
                key.push_str("\nfirst-party: ");
                key.push_str(&site.to_string());
            }
            // Include the per-request proxy in the key so requests with
            // different proxies (scheme, host, port) don't coalesce.
            if let Some(proxy) = &request.proxy {
                key.push_str("\nproxy: ");
                key.push_str(&proxy.pool_key());
            }
            for (name, value) in &built {
                key.push('\n');
                key.push_str(name);
                key.push_str(": ");
                key.push_str(&String::from_utf8_lossy(value.as_bytes()));
            }
            Some(key)
        }
    }
}

/// Whether one response may be handed to another request that asked the same
/// question. A `206` answers a `Range` no other request is guaranteed to have
/// asked, and `Vary: *` means the origin says no two requests are equivalent.
fn is_shareable(status: StatusCode, headers: &[(String, HeaderValue)]) -> bool {
    if status == StatusCode::PARTIAL_CONTENT {
        return false;
    }
    // A 304 answers a conditional request and belongs to the cache; it must
    // never be handed to a coalesced follower as its response (MAJOR-2).
    // Revalidations no longer coalesce, so this is a second line of defence.
    if status == StatusCode::NOT_MODIFIED {
        return false;
    }
    !headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("vary")
            && value
                .as_bytes()
                .split(|b| *b == b',')
                .any(|part| part.trim_ascii() == b"*")
    })
}

/// One attempt: pool lookup, connection, request, response head.
async fn send_once(
    state: &Rc<ClientRuntime>,
    request: &Request,
    body: Body,
    deadline: Option<Instant>,
    coalesce_key: Option<String>,
    reused: &Cell<bool>,
) -> Result<Attempt, NetError> {
    reused.set(false);
    let shared = Arc::clone(&state.shared);
    let destination = Destination::from_url(&request.url)?;
    let proxy = request.proxy.clone().or_else(|| shared.proxy.clone());
    let key = PoolKey::new(
        &destination,
        proxy.as_ref(),
        shared.profile.name(),
        shared.context_id,
    );
    let cookie = shared.cookie_header(request);
    let start = Instant::now();

    // A warm HTTP/2 connection is the common case; nothing is opened.
    let pooled_h2 = state.pool.borrow_mut().h2(&key);
    // Read before any await: a `Ref` held across one would panic the next
    // task that borrows the pool mutably.
    let pooled_remote = state.pool.borrow().remote(&key);
    if let Some(handle) = pooled_h2 {
        reused.set(true);
        let attempt = run_h2(
            state,
            request,
            &key,
            handle.clone(),
            body,
            cookie.clone(),
            ConnectTiming::default(),
            pooled_remote,
            start,
            deadline,
            coalesce_key.clone(),
        )
        .await;
        if let Err(err) = &attempt {
            // GOAWAY and failure before a stream opens make this pooled
            // sender unusable. A transport error or head timeout likewise
            // evicts it; an ordinary stream reset does not affect the other
            // streams on a healthy connection.
            if should_evict_h2(err) {
                state.pool.borrow_mut().drop_h2(&key);
            }
        }
        // Stream-level errors (reset) do not drop the connection;
        // other streams on the same connection are still valid.
        return attempt;
    }

    let permit = {
        let slots = state.pool.borrow_mut().h1_slots(&key);
        with_deadline(
            deadline,
            slots.acquire_owned(),
            "waiting for a connection slot",
        )
        .await?
        .map_err(|_| NetError::new(ErrorName::Aborted, "connection slots closed"))?
    };
    let pooled_h1 = state.pool.borrow_mut().take_h1(&key);
    if let Some(handle) = pooled_h1 {
        reused.set(true);
        return run_h1(
            state,
            request,
            &key,
            handle,
            body,
            permit,
            cookie,
            ConnectTiming::default(),
            pooled_remote,
            start,
            deadline,
            coalesce_key,
        )
        .await;
    }

    let opened = with_deadline(
        deadline,
        stream::open(
            &destination,
            proxy.as_ref(),
            &shared.resolver,
            &shared.connector,
            shared.destinations(),
            &request.url,
            shared.connect_timeout,
        ),
        "connecting",
    )
    .await??;
    let remote_addr = opened.remote_addr;
    if let Some(remote) = remote_addr {
        state.pool.borrow_mut().note_remote(&key, remote);
    }
    let connect_timing = opened.timing;

    if shared.allow_h2 && opened.alpn.as_deref() == Some("h2") {
        let handle =
            h2::handshake(opened.stream, &shared.profile, shared.pool.h2_keep_alive).await?;
        state.pool.borrow_mut().put_h2(&key, handle.clone());
        drop(permit);
        return run_h2(
            state,
            request,
            &key,
            handle,
            body,
            cookie,
            connect_timing,
            remote_addr,
            start,
            deadline,
            coalesce_key,
        )
        .await;
    }

    let handle = h1::handshake(opened.stream, shared.profile.client_profile().is_some()).await?;
    run_h1(
        state,
        request,
        &key,
        handle,
        body,
        permit,
        cookie,
        connect_timing,
        remote_addr,
        start,
        deadline,
        coalesce_key,
    )
    .await
}

fn should_evict_h2(err: &NetError) -> bool {
    matches!(err.replay_hint(), ReplayHint::H2GoAway | ReplayHint::Unsent)
        || err.retryable()
        || err.name == ErrorName::TimedOut
}

#[allow(clippy::too_many_arguments)]
async fn run_h1(
    state: &Rc<ClientRuntime>,
    request: &Request,
    key: &PoolKey,
    mut handle: H1Handle,
    mut body: Body,
    permit: tokio::sync::OwnedSemaphorePermit,
    cookie: Option<String>,
    connect_timing: ConnectTiming,
    remote_addr: Option<std::net::SocketAddr>,
    start: Instant,
    deadline: Option<Instant>,
    coalesce_key: Option<String>,
) -> Result<Attempt, NetError> {
    let shared = Arc::clone(&state.shared);
    let destination = Destination::from_url(&request.url)?;
    // Wall-clock time the request left, which is what the cache needs to
    // correct the age of a slow response.
    let request_time = SystemTime::now();
    let length = body.length();
    let mut header_list = headers::build(
        &shared.profile,
        request,
        &destination,
        WireProtocol::Http11,
        cookie,
        length,
    );
    // hyper frames a body of unknown length as chunked and inserts the header
    // itself, out of the profile's order. Saying it here keeps the head we
    // hand to `h1_case` identical to the one hyper writes.
    if length.is_none()
        && !body.is_empty()
        && !matches!(
            request.method,
            http::Method::GET | http::Method::HEAD | http::Method::CONNECT
        )
    {
        header_list.push((
            "Transfer-Encoding".to_owned(),
            http::HeaderValue::from_static("chunked"),
        ));
    }

    let mut builder = http::Request::builder()
        .method(request.method.clone())
        .uri(request_target(&request.url))
        .version(Version::HTTP_11);
    if let Some(map) = builder.headers_mut() {
        *map = headers::to_header_map(&header_list);
    }
    let http_request = builder
        .body(OutgoingBody::new(std::mem::take(&mut body)))
        .map_err(|err| NetError::new(ErrorName::InvalidArgument, err.to_string()))?;

    let response = with_deadline(
        deadline,
        handle.send(http_request, &header_list),
        "waiting for headers",
    )
    .await??;
    let ttfb = start.elapsed();
    let (parts, incoming) = response.into_parts();
    let raw_headers = header_pairs(&parts.headers);
    let chunked = parts
        .headers
        .get_all(http::header::TRANSFER_ENCODING)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .any(|value| value.to_ascii_lowercase().contains("chunked"));

    finish(
        state,
        request,
        key,
        parts.status,
        parts.version,
        raw_headers,
        ChunkSource::H1 {
            body: incoming,
            chunked,
        },
        Protocol::Http11,
        Timing {
            dns: connect_timing.dns,
            connect: connect_timing.connect,
            tls: connect_timing.tls,
            ttfb,
            total: Duration::ZERO,
        },
        remote_addr,
        deadline,
        coalesce_key,
        Some((handle, permit)),
        request_time,
    )
}

#[allow(clippy::too_many_arguments)]
async fn run_h2(
    state: &Rc<ClientRuntime>,
    request: &Request,
    key: &PoolKey,
    mut handle: H2Handle,
    body: Body,
    cookie: Option<String>,
    connect_timing: ConnectTiming,
    remote_addr: Option<std::net::SocketAddr>,
    start: Instant,
    deadline: Option<Instant>,
    coalesce_key: Option<String>,
) -> Result<Attempt, NetError> {
    let shared = Arc::clone(&state.shared);
    let destination = Destination::from_url(&request.url)?;
    // Wall-clock time the request left, which is what the cache needs to
    // correct the age of a slow response.
    let request_time = SystemTime::now();
    let length = body.length();
    let header_list = headers::build(
        &shared.profile,
        request,
        &destination,
        WireProtocol::Http2,
        cookie,
        length,
    );

    let mut builder = http::Request::builder()
        .method(request.method.clone())
        .uri(request.url.as_str())
        .version(Version::HTTP_2);
    if let Some(map) = builder.headers_mut() {
        *map = headers::to_header_map(&header_list);
    }
    let http_request = builder
        .body(())
        .map_err(|err| NetError::new(ErrorName::InvalidArgument, err.to_string()))?;

    let end_of_stream = body.is_empty();
    let (response, send_stream) = with_deadline(
        deadline,
        handle.start(http_request, end_of_stream),
        "opening a stream",
    )
    .await??;
    if !end_of_stream {
        // The body is written while the response is pending; a server that
        // reads the whole request before answering must not deadlock.
        tokio::task::spawn_local(async move {
            if let Err(err) = send_h2_body(send_stream, body).await {
                tracing::debug!(error = %err, "http/2 request body");
            }
        });
    }
    let response = with_deadline(
        deadline,
        H2Handle::response(response),
        "waiting for headers",
    )
    .await??;
    let ttfb = start.elapsed();
    let (parts, recv) = response.into_parts();
    let raw_headers = header_pairs(&parts.headers);

    finish(
        state,
        request,
        key,
        parts.status,
        parts.version,
        raw_headers,
        ChunkSource::H2(recv),
        Protocol::Http2,
        Timing {
            dns: connect_timing.dns,
            connect: connect_timing.connect,
            tls: connect_timing.tls,
            ttfb,
            total: Duration::ZERO,
        },
        remote_addr,
        deadline,
        coalesce_key,
        None,
        request_time,
    )
}

/// Builds the response head and starts the body pump.
#[allow(clippy::too_many_arguments)]
fn finish(
    state: &Rc<ClientRuntime>,
    request: &Request,
    key: &PoolKey,
    status: StatusCode,
    version: Version,
    raw_headers: Vec<(String, HeaderValue)>,
    source: ChunkSource,
    protocol: Protocol,
    timing: Timing,
    remote_addr: Option<std::net::SocketAddr>,
    deadline: Option<Instant>,
    coalesce_key: Option<String>,
    h1: Option<(H1Handle, tokio::sync::OwnedSemaphorePermit)>,
    request_time: SystemTime,
) -> Result<Attempt, NetError> {
    let shared = Arc::clone(&state.shared);
    let encoding = raw_headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-encoding"))
        .and_then(|(_, value)| value.to_str().ok())
        .map(str::to_owned);
    let coding = match encoding.as_deref() {
        None => ContentEncoding::Identity,
        Some(value) => ContentEncoding::parse(value).unwrap_or(ContentEncoding::Identity),
    };
    // The size budget is enforced inside the decoder, so a compressed bomb
    // stops mid-chunk instead of after a gigabyte has been materialised.
    let max_bytes = request.limits.max_body_bytes.or(shared.max_response_bytes);
    let decoder = Decoder::new(coding, max_bytes)?;

    // The effective headers hide what the transport already undid.
    let mut effective = raw_headers.clone();
    if !decoder.is_identity() {
        effective.retain(|(name, _)| {
            !name.eq_ignore_ascii_case("content-encoding")
                && !name.eq_ignore_ascii_case("content-length")
        });
    }

    if shared.has_observers.load(Ordering::Relaxed) {
        let map = headers::to_header_map(&raw_headers);
        for observer in &shared.hooks.observers {
            observer.on_response(&request.url, status, &map);
        }
    }
    shared.store_cookies(request, &raw_headers);
    if tracing::enabled!(tracing::Level::TRACE) {
        tracing::trace!(
            url = %shared.log_url(&request.url),
            status = status.as_u16(),
            protocol = protocol.as_str(),
            headers = ?shared.log_headers(&raw_headers),
            "response head"
        );
    }

    // What the cache does with this hop is decided here, before any body
    // arrives, so an uncacheable response is never buffered.
    let cache_plan = cache_plan(&shared, request, status, &effective, request_time);

    let (chunks_tx, chunks_rx) = mpsc::channel(4);
    let coalesce_limit = match shared.coalescing {
        Coalescing::Shared { max_bytes } => Some(max_bytes),
        Coalescing::Off => None,
    };
    let cache_writes_block = cache_plan
        .as_ref()
        .is_some_and(|plan| plan.cache.writes_block());
    let pump = BodyPump {
        state: Rc::clone(state),
        key: key.clone(),
        url: request.url.clone(),
        source,
        decoder,
        chunks: chunks_tx,
        max_bytes,
        deadline,
        coalesce_key,
        coalesce_limit,
        cache_plan,
        cache_writes_block,
        h1,
        head: SharedHead {
            status,
            version,
            url: request.url.clone(),
            headers: effective.clone(),
            raw_headers: raw_headers.clone(),
            timing,
            remote_addr,
            protocol,
        },
    };
    tokio::task::spawn_local(pump.run());

    Ok(Attempt {
        status,
        version,
        url: request.url.clone(),
        headers: effective,
        raw_headers,
        body: BodyStream::new(chunks_rx),
        timing,
        remote_addr,
        protocol,
    })
}

/// What the pump must do for the cache once the body is complete.
struct CachePlan {
    cache: HttpCache,
    method: String,
    url: Url,
    request_headers: HeaderMap,
    status: u16,
    response_headers: HeaderMap,
    request_time: SystemTime,
    response_time: SystemTime,
    /// Beyond this the cache would refuse the entry anyway, so buffering
    /// stops and the body only streams.
    max_bytes: u64,
}

/// Decides whether this response is worth buffering for the cache, and
/// invalidates on the spot when the method is unsafe (RFC 9111 §4.4).
fn cache_plan(
    shared: &ClientShared,
    request: &Request,
    status: StatusCode,
    response_headers: &[(String, HeaderValue)],
    request_time: SystemTime,
) -> Option<CachePlan> {
    let cache = shared.cache.as_ref()?;
    let method = request.method.as_str().to_owned();
    let request_headers = headers::to_header_map(&request.headers);
    let response_map = headers::to_header_map(response_headers);

    if is_unsafe_method(&method) {
        if (200..400).contains(&status.as_u16()) {
            let cache_request = CacheRequest::new(&method, &request.url, &request_headers);
            cache.invalidate_for(&cache_request, &response_map);
        }
        return None;
    }
    if !is_cacheable_method(&method) || !is_cacheable_status(status.as_u16()) {
        return None;
    }
    // `no-store` on either side settles it without reading a byte.
    if CacheControl::parse(&request_headers).no_store || CacheControl::parse(&response_map).no_store
    {
        return None;
    }

    Some(CachePlan {
        max_bytes: cache.config().max_entry_bytes,
        cache: cache.clone(),
        method,
        url: request.url.clone(),
        request_headers,
        status: status.as_u16(),
        response_headers: response_map,
        request_time,
        response_time: SystemTime::now(),
    })
}

#[derive(Clone)]
struct SharedHead {
    status: StatusCode,
    version: Version,
    url: Url,
    headers: Vec<(String, HeaderValue)>,
    raw_headers: Vec<(String, HeaderValue)>,
    timing: Timing,
    remote_addr: Option<std::net::SocketAddr>,
    protocol: Protocol,
}

/// Where response body chunks come from.
enum ChunkSource {
    H1 {
        body: hyper::body::Incoming,
        /// `Transfer-Encoding: chunked`, which decides the Chromium name a
        /// truncated or malformed body gets.
        chunked: bool,
    },
    H2(tenta_h2::RecvStream),
}

/// The Chromium name for an HTTP/1.1 body that ended badly: a connection
/// closed before `Content-Length` bytes or the last chunk arrived, a chunk
/// header that does not parse, or a transport failure underneath.
fn h1_body_error(err: &hyper::Error, chunked: bool) -> NetError {
    let mut source: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(err);
    while let Some(inner) = source {
        if let Some(io) = inner.downcast_ref::<std::io::Error>() {
            let name = match io.kind() {
                std::io::ErrorKind::UnexpectedEof if chunked => {
                    ErrorName::IncompleteChunkedEncoding
                }
                std::io::ErrorKind::UnexpectedEof => ErrorName::ContentLengthMismatch,
                std::io::ErrorKind::InvalidInput | std::io::ErrorKind::InvalidData if chunked => {
                    ErrorName::InvalidChunkedEncoding
                }
                _ => return NetError::from_stream_io(io),
            };
            return NetError::new(name, err.to_string());
        }
        source = inner.source();
    }
    let name = if err.is_incomplete_message() {
        if chunked {
            ErrorName::IncompleteChunkedEncoding
        } else {
            ErrorName::ContentLengthMismatch
        }
    } else {
        ErrorName::ConnectionClosed
    };
    NetError::new(name, err.to_string())
}

impl ChunkSource {
    async fn next(&mut self) -> Option<Result<Bytes, NetError>> {
        match self {
            ChunkSource::H1 { body, chunked } => {
                use http_body_util::BodyExt;
                loop {
                    match body.frame().await {
                        None => return None,
                        Some(Err(err)) => return Some(Err(h1_body_error(&err, *chunked))),
                        Some(Ok(frame)) => match frame.into_data() {
                            Ok(data) => return Some(Ok(data)),
                            // Trailers: nothing to hand to the reader.
                            Err(_) => continue,
                        },
                    }
                }
            }
            ChunkSource::H2(recv) => match recv.data().await {
                None => None,
                Some(Err(err)) => Some(Err(h2::h2_error(&err))),
                Some(Ok(chunk)) => {
                    // Release flow-control capacity as the reader consumes.
                    let _ = recv.flow_control().release_capacity(chunk.len());
                    Some(Ok(chunk))
                }
            },
        }
    }
}

/// Streams one response body: decode, enforce limits, feed observers, share
/// with coalescing followers and return the connection to the pool.
struct BodyPump {
    state: Rc<ClientRuntime>,
    key: PoolKey,
    url: Url,
    source: ChunkSource,
    decoder: Decoder,
    chunks: mpsc::Sender<Result<Bytes, NetError>>,
    max_bytes: Option<u64>,
    deadline: Option<Instant>,
    coalesce_key: Option<String>,
    coalesce_limit: Option<u64>,
    cache_plan: Option<CachePlan>,
    /// Whether storing an entry touches the filesystem, in which case the
    /// write goes to a blocking thread.
    cache_writes_block: bool,
    h1: Option<(H1Handle, tokio::sync::OwnedSemaphorePermit)>,
    head: SharedHead,
}

impl BodyPump {
    /// Decodes one chunk using 8 KiB input slices, switching to a blocking thread when output exceeds 256 KiB.
    ///
    /// The io runtime is `current_thread` and owns every connection of this
    /// client, so a multi-megabyte brotli or zstd chunk decoded inline stalls
    /// all of them (`docs/spec/03-network.md` §1). Input is sliced into 8 KiB
    /// pieces fed to the decoder; once accumulated output exceeds 256 KiB,
    /// remaining input is handed to a blocking thread. Each inline decode call
    /// yields at most 256 KiB so the io thread keeps < 1 ms work per poll.
    /// Never drops bytes; splits large outputs into 256 KiB pieces.
    async fn decode(&mut self, chunk: Bytes) -> Result<Bytes, NetError> {
        const INPUT_SLICE_SIZE: usize = 8 * 1024; // 8 KiB slices
        const OUTPUT_THRESHOLD: u64 = 256 * 1024; // 256 KiB inline output limit

        if self.decoder.is_identity() {
            return self.decoder.decode(chunk);
        }

        // Slice input into 8 KiB pieces and decode each one inline until
        // accumulated output exceeds 256 KiB, then hand remaining to blocking thread.
        let mut output = Vec::new();
        let mut offset = 0;

        loop {
            let slice_end = std::cmp::min(offset + INPUT_SLICE_SIZE, chunk.len());
            let slice = chunk.slice(offset..slice_end);
            offset = slice_end;

            // Decode this slice
            let decoded = self.decoder.decode(slice)?;
            output.extend_from_slice(&decoded);

            // Check if we've exceeded the inline threshold
            if output.len() as u64 >= OUTPUT_THRESHOLD && offset < chunk.len() {
                // Output exceeded threshold and we have remaining input:
                // hand the rest to a blocking thread
                let remaining = chunk.slice(offset..);
                let decoder = std::mem::replace(
                    &mut self.decoder,
                    Decoder::new(ContentEncoding::Identity, None)?,
                );

                let (decoder, remaining_decoded) = tokio::task::spawn_blocking(move || {
                    let mut decoder = decoder;
                    let decoded = decoder.decode(remaining);
                    (decoder, decoded)
                })
                .await
                .map_err(|err| {
                    NetError::new(
                        ErrorName::Failed,
                        format!("the decompression thread went away: {err}"),
                    )
                })?;
                self.decoder = decoder;
                output.extend_from_slice(&remaining_decoded?);
                break;
            }

            if offset >= chunk.len() {
                break;
            }
        }

        Ok(Bytes::from(output))
    }

    async fn run(mut self) {
        let shared = Arc::clone(&self.state.shared);
        let observing = shared.has_observers.load(Ordering::Relaxed);
        let mut buffered: Option<Vec<Bytes>> = self.coalesce_key.as_ref().map(|_| Vec::new());
        // The cache gets the decoded body, so a hit serves the same bytes
        // the network path hands over.
        let mut for_cache: Option<Vec<Bytes>> = self.cache_plan.as_ref().map(|_| Vec::new());
        let mut total: u64 = 0;
        let mut failure: Option<NetError> = None;
        let mut clean = true;

        loop {
            // Race the next source chunk against the reader going away. A body
            // that stalls after its head (a cancelled navigation whose reader
            // dropped the `BodyStream`) leaves the pump parked on the source,
            // which never delivers a chunk to fail the send on — so without
            // this the connection is held until the deadline (B-R5-1). When the
            // reader is gone the pump aborts, and the h1 handle is dropped
            // rather than returned to the pool, closing the socket.
            let next = match self.deadline {
                Some(deadline) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    tokio::select! {
                        biased;
                        _ = self.chunks.closed() => {
                            failure = Some(NetError::new(
                                ErrorName::Aborted,
                                "the response reader went away before the body finished",
                            ));
                            clean = false;
                            break;
                        }
                        result = tokio::time::timeout(remaining, self.source.next()) => match result {
                            Ok(next) => next,
                            Err(_) => {
                                failure = Some(NetError::new(
                                    ErrorName::TimedOut,
                                    "response body exceeded the request deadline",
                                ));
                                clean = false;
                                break;
                            }
                        },
                    }
                }
                None => {
                    tokio::select! {
                        biased;
                        _ = self.chunks.closed() => {
                            failure = Some(NetError::new(
                                ErrorName::Aborted,
                                "the response reader went away before the body finished",
                            ));
                            clean = false;
                            break;
                        }
                        next = self.source.next() => next,
                    }
                }
            };
            let Some(chunk) = next else { break };
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(err) => {
                    failure = Some(err);
                    clean = false;
                    break;
                }
            };
            let decoded = match self.decode(chunk).await {
                Ok(decoded) => decoded,
                Err(err) => {
                    failure = Some(err);
                    clean = false;
                    break;
                }
            };
            if decoded.is_empty() {
                continue;
            }

            // Split large outputs into ≤ 256 KiB pieces and send with yield_now() between them.
            const CHUNK_SIZE: usize = 256 * 1024; // 256 KiB
            let mut offset = 0;
            while offset < decoded.len() {
                let end = (offset + CHUNK_SIZE).min(decoded.len());
                let piece = decoded.slice(offset..end);
                offset = end;

                total += piece.len() as u64;
                if let Some(max) = self.max_bytes
                    && total > max
                {
                    failure = Some(NetError::new(
                        ErrorName::FileTooBig,
                        format!("response body is larger than the {max}-byte limit"),
                    ));
                    clean = false;
                    break;
                }
                if let (Some(buffer), Some(limit)) = (buffered.as_mut(), self.coalesce_limit) {
                    if total <= limit {
                        buffer.push(piece.clone());
                    } else {
                        buffered = None;
                    }
                }
                if let (Some(buffer), Some(plan)) = (for_cache.as_mut(), self.cache_plan.as_ref()) {
                    if total <= plan.max_bytes {
                        buffer.push(piece.clone());
                    } else {
                        // Larger than the cache would accept: stop paying for it.
                        for_cache = None;
                    }
                }
                if observing {
                    for observer in &shared.hooks.observers {
                        observer.on_body_chunk(&self.url, &piece);
                    }
                }
                if self.chunks.send(Ok(piece)).await.is_err() {
                    // The reader went away; abandon the body and signal to followers.
                    failure = Some(NetError::new(
                        ErrorName::Aborted,
                        "navigation was cancelled before the body finished streaming",
                    ));
                    buffered = None;
                    for_cache = None;
                    clean = false;
                    break;
                }

                // Yield to the runtime between 256 KiB pieces.
                if offset < decoded.len() {
                    tokio::task::yield_now().await;
                }
            }

            if failure.is_some() {
                break;
            }
        }

        if failure.is_none() {
            match self.decoder.finish() {
                Ok(tail) if !tail.is_empty() => {
                    // Apply the same chunk-splitting logic to the tail.
                    const CHUNK_SIZE: usize = 256 * 1024; // 256 KiB
                    let mut offset = 0;
                    while offset < tail.len() {
                        let end = (offset + CHUNK_SIZE).min(tail.len());
                        let piece = tail.slice(offset..end);
                        offset = end;

                        total += piece.len() as u64;
                        if let Some(max) = self.max_bytes
                            && total > max
                        {
                            failure = Some(NetError::new(
                                ErrorName::FileTooBig,
                                format!("response body is larger than the {max}-byte limit"),
                            ));
                            clean = false;
                            break;
                        }
                        if let (Some(buffer), Some(limit)) =
                            (buffered.as_mut(), self.coalesce_limit)
                        {
                            if total <= limit {
                                buffer.push(piece.clone());
                            } else {
                                buffered = None;
                            }
                        }
                        if let (Some(buffer), Some(plan)) =
                            (for_cache.as_mut(), self.cache_plan.as_ref())
                        {
                            if total <= plan.max_bytes {
                                buffer.push(piece.clone());
                            } else {
                                for_cache = None;
                            }
                        }
                        if observing {
                            for observer in &shared.hooks.observers {
                                observer.on_body_chunk(&self.url, &piece);
                            }
                        }
                        if self.chunks.send(Ok(piece)).await.is_err() {
                            // Reader abandoned during the final tail; set failure.
                            failure = Some(NetError::new(
                                ErrorName::Aborted,
                                "navigation was cancelled before the body finished streaming",
                            ));
                            buffered = None;
                            for_cache = None;
                            clean = false;
                            break;
                        }

                        // Yield between pieces.
                        if offset < tail.len() {
                            tokio::task::yield_now().await;
                        }
                    }
                }
                Ok(_) => {}
                Err(err) => {
                    failure = Some(err);
                    clean = false;
                }
            }
        }

        if let Some(err) = &failure {
            let _ = self.chunks.send(Err(err.clone())).await;
        }

        if let Some(key) = self.coalesce_key.take() {
            let waiters = self.state.in_flight.borrow_mut().remove(&key);
            if let Some(waiters) = waiters
                && !waiters.is_empty()
            {
                match (&failure, buffered) {
                    (Some(_), _) => {
                        // The leader's own body failed — most often because its
                        // reader (a cancelled navigation) went away mid-stream.
                        // A follower's request was never cancelled, so it must
                        // not inherit the leader's error and commit an error
                        // page (B-R5-2). Dropping the senders lets each follower
                        // run its own request instead.
                        drop(waiters);
                    }
                    (None, Some(buffer))
                        if is_shareable(self.head.status, &self.head.raw_headers) =>
                    {
                        let body = concat(buffer);
                        // MAJOR-2: Refuse to share a body that doesn't match its Content-Length,
                        // or is incomplete due to an abandoned reader. For compressed bodies,
                        // the raw Content-Length is the compressed size, not the decoded size,
                        // so we check against effective headers (which have Content-Length
                        // removed if the body was decoded) or trust the transport's signal
                        // (the source ended cleanly after full decompression).
                        let is_compressed = self
                            .head
                            .raw_headers
                            .iter()
                            .any(|(name, _)| name.eq_ignore_ascii_case("content-encoding"));
                        let content_length_ok = if is_compressed {
                            // For compressed responses, the transport has already verified
                            // that the decompressed output is correct. Just trust it.
                            true
                        } else {
                            body_matches_content_length(&body, &self.head.raw_headers)
                        };
                        if content_length_ok {
                            let shared_response = Arc::new(SharedResponse {
                                status: self.head.status,
                                version: self.head.version,
                                url: self.head.url.clone(),
                                headers: self.head.headers.clone(),
                                raw_headers: self.head.raw_headers.clone(),
                                timing: self.head.timing,
                                remote_addr: self.head.remote_addr,
                                protocol: self.head.protocol,
                                redirects: Vec::new(),
                                body,
                            });
                            for waiter in waiters {
                                let _ = waiter.send(Ok(Arc::clone(&shared_response)));
                            }
                        } else {
                            // Body doesn't match Content-Length; let followers retry.
                            drop(waiters);
                        }
                    }
                    // Too large, or a response that answers only the request
                    // that asked for it: dropping the sender tells the
                    // followers to run their own request.
                    (None, _) => drop(waiters),
                }
            }
        }

        if let (Some(plan), Some(buffer), None) =
            (self.cache_plan.take(), for_cache, failure.as_ref())
        {
            let body = concat(buffer);
            // Only cache a body if it matches its declared Content-Length
            // to avoid caching incomplete bodies from abandoned readers.
            if body_matches_content_length_map(&body, &plan.response_headers) {
                // A disk store writes and flushes synchronously; that does not
                // belong on the runtime that owns every connection of this client.
                let store = move || {
                    let cache_request =
                        CacheRequest::new(&plan.method, &plan.url, &plan.request_headers);
                    plan.cache.store_with_timing(
                        &cache_request,
                        plan.status,
                        &plan.response_headers,
                        body,
                        plan.request_time,
                        plan.response_time,
                    );
                };
                if self.cache_writes_block {
                    tokio::task::spawn_blocking(store);
                } else {
                    store();
                }
            }
        }

        if let Some((handle, permit)) = self.h1.take() {
            if clean && failure.is_none() {
                self.state.pool.borrow_mut().put_h1(&self.key, handle);
            }
            drop(permit);
        }
    }
}

/// Whether a body may be shared with coalesced followers or stored: it must be
/// as long as its `Content-Length` says, when there is one (RFC 9112 §6.3).
fn body_matches_content_length(body: &Bytes, headers: &[(String, HeaderValue)]) -> bool {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .is_none_or(|(_, value)| declared_length_matches(value, body.len()))
}

/// `HeaderMap` form of [`body_matches_content_length`].
fn body_matches_content_length_map(body: &Bytes, headers: &HeaderMap) -> bool {
    headers
        .get("content-length")
        .is_none_or(|value| declared_length_matches(value, body.len()))
}

fn declared_length_matches(value: &HeaderValue, actual: usize) -> bool {
    value
        .to_str()
        .ok()
        .and_then(|text| text.trim().parse::<u64>().ok())
        .is_some_and(|declared| declared == actual as u64)
}

fn concat(chunks: Vec<Bytes>) -> Bytes {
    match chunks.len() {
        0 => Bytes::new(),
        1 => chunks.into_iter().next().expect("one chunk"),
        _ => {
            let total: usize = chunks.iter().map(Bytes::len).sum();
            let mut out = Vec::with_capacity(total);
            for chunk in chunks {
                out.extend_from_slice(&chunk);
            }
            Bytes::from(out)
        }
    }
}

/// Writes a request body into an HTTP/2 stream, respecting flow control.
async fn send_h2_body(
    mut send_stream: tenta_h2::SendStream<Bytes>,
    body: Body,
) -> Result<(), NetError> {
    match body {
        Body::Empty => {}
        Body::Bytes(bytes) => write_h2_chunk(&mut send_stream, bytes).await?,
        Body::Stream { mut chunks, .. } => {
            while let Some(chunk) = chunks.recv().await {
                write_h2_chunk(&mut send_stream, chunk?).await?;
            }
        }
    }
    send_stream
        .send_data(Bytes::new(), true)
        .map_err(|err| h2::h2_error(&err))
}

async fn write_h2_chunk(
    send_stream: &mut tenta_h2::SendStream<Bytes>,
    mut chunk: Bytes,
) -> Result<(), NetError> {
    while !chunk.is_empty() {
        send_stream.reserve_capacity(chunk.len());
        let available = std::future::poll_fn(|cx| send_stream.poll_capacity(cx))
            .await
            .transpose()
            .map_err(|err| h2::h2_error(&err))?
            .unwrap_or(0);
        if available == 0 {
            return Err(NetError::new(
                ErrorName::Http2ProtocolError,
                "the peer closed the stream before the body was sent",
            ));
        }
        let piece = chunk.split_to(available.min(chunk.len()));
        send_stream
            .send_data(piece, false)
            .map_err(|err| h2::h2_error(&err))?;
    }
    Ok(())
}

/// The origin-form target an HTTP/1.1 request line carries.
fn request_target(url: &Url) -> String {
    let mut target = url.path().to_owned();
    if let Some(query) = url.query() {
        target.push('?');
        target.push_str(query);
    }
    if target.is_empty() {
        target.push('/');
    }
    target
}

fn header_pairs(map: &http::HeaderMap) -> Vec<(String, HeaderValue)> {
    map.iter()
        .map(|(name, value)| (name.as_str().to_owned(), value.clone()))
        .collect()
}

/// Splits a body into the one that goes on the wire and the one a redirect
/// may replay. A streamed body has no replay.
fn split_body(body: Body) -> (Body, Option<Body>) {
    match body {
        Body::Empty => (Body::Empty, Some(Body::Empty)),
        Body::Bytes(bytes) => (Body::Bytes(bytes.clone()), Some(Body::Bytes(bytes))),
        stream @ Body::Stream { .. } => (stream, None),
    }
}

fn is_idempotent_method(method: &Method) -> bool {
    method == Method::GET
        || method == Method::HEAD
        || method == Method::PUT
        || method == Method::DELETE
        || method == Method::OPTIONS
        || method == Method::TRACE
}

/// The redirect a policy allows this response to be followed to, or
/// `ERR_INVALID_REDIRECT` when the `Location` cannot be resolved.
fn redirect_target(
    policy: RedirectPolicy,
    response: &Response,
    base: &Url,
) -> Result<Option<Url>, NetError> {
    match policy {
        RedirectPolicy::Follow { .. } => {
            redirect_location(&response.status, &response.headers, base)
        }
        _ => Ok(None),
    }
}

/// The same decision for an attempt that has not become a [`Response`] yet.
fn attempt_redirect_target(
    policy: RedirectPolicy,
    attempt: &Attempt,
    base: &Url,
) -> Result<Option<Url>, NetError> {
    match policy {
        RedirectPolicy::Follow { .. } => redirect_location(&attempt.status, &attempt.headers, base),
        _ => Ok(None),
    }
}

/// Checks if a response should be followed as a redirect and if so, validates
/// limits, creates a hop record, and returns the modified request ready to follow.
/// Returns None if no redirect; caller decides whether to return the response.
fn check_and_follow_redirect_hop(
    request: Request,
    response: &Response,
    policy: RedirectPolicy,
    max_redirects: u32,
    redirects: &mut Vec<RedirectHop>,
    from_cache: bool,
) -> Result<Option<Request>, NetError> {
    let Some(target) = redirect_target(policy, response, &request.url)? else {
        return Ok(None);
    };

    if redirects.len() as u32 >= max_redirects {
        return Err(NetError::new(
            ErrorName::TooManyRedirects,
            format!("more than {max_redirects} redirects"),
        ));
    }

    let location = response
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("location"))
        .and_then(|(_, value)| value.to_str().ok())
        .map(|s| s.to_owned());

    redirects.push(RedirectHop {
        url: request.url.clone(),
        status: response.status.as_u16(),
        location,
        from_cache,
    });

    let next = follow_redirect(request, &response.status, target)?;
    if let (Some(tap), Some(hop)) = (&next.tap, redirects.last()) {
        tap.redirected(&next, hop, &response.headers);
    }
    Ok(Some(next))
}

/// The redirect target of a 3xx response, resolved against the request URL.
/// A 3xx without `Location` is not a redirect; one whose `Location` does
/// not resolve fails the way Chrome fails it.
fn redirect_location(
    status: &StatusCode,
    headers: &[(String, HeaderValue)],
    base: &Url,
) -> Result<Option<Url>, NetError> {
    if !matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308) {
        return Ok(None);
    }
    let Some(location) = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("location"))
        .and_then(|(_, value)| value.to_str().ok())
    else {
        return Ok(None);
    };
    base.join(location).map(Some).map_err(|error| {
        NetError::new(
            ErrorName::InvalidRedirect,
            format!("Location {location:?} does not resolve against {base}: {error}"),
        )
    })
}

/// Applies Fetch's redirect rules to the next request.
fn follow_redirect(
    mut request: Request,
    status: &StatusCode,
    target: Url,
) -> Result<Request, NetError> {
    if target.scheme() != "http" && target.scheme() != "https" {
        return Err(NetError::new(
            ErrorName::UnsafeRedirect,
            format!("redirect to scheme {:?}", target.scheme()),
        ));
    }
    let previous = request.url.clone();
    let cross_origin = origin(&previous) != origin(&target);
    if cross_origin && request.same_origin_redirects {
        return Err(NetError::new(
            ErrorName::UnsafeRedirect,
            "a redirect to another origin is refused for this request",
        ));
    }

    // 303, and 301/302 on POST, become GET without a body (Fetch §4.4).
    let drop_body = match status.as_u16() {
        303 => request.method != Method::HEAD,
        301 | 302 => request.method == Method::POST,
        _ => false,
    };
    if drop_body {
        request.method = Method::GET;
        request.body = Body::Empty;
        request.headers.retain(|(name, _)| !is_payload_header(name));
    }

    if cross_origin {
        // Credentials never follow a cross-origin redirect.
        request
            .headers
            .retain(|(name, _)| !is_credential_header(name));
    }

    // `Sec-Fetch-Site` is monotone over the chain: classify the target
    // against the original initiator (Chromium's `SetSecFetchSiteHeader`).
    // With no initiator (a driver-issued navigation), every hop is `none`.
    // The request's own referrer is untouched: `Referer` is computed from it,
    // not from this hop, so an intermediate redirector is never disclosed and
    // a request that started without a referrer never gains one.
    if let Some(initiator) = &request.initiator {
        request.site_floor = request
            .site_floor
            .min(FetchSite::classify(Some(initiator), &target));
    }

    // BLOCKING-4: Strip userinfo from the redirect target and validate port/scheme.
    let mut target = target;
    if request.userinfo.is_none() {
        request.userinfo = extract_userinfo(target.as_str());
    }
    let _ = target.set_username("");
    let _ = target.set_password(None);
    // Apply the same validation as parse_url does for the initial request.
    check_port(&target)?;

    // The same-site classification is initiator versus the hop's target
    // (Chrome with `CookieSameSiteConsidersRedirectChain` off: intermediate
    // hops are ignored, the original initiator is not). A navigation without
    // an initiator stays a top-level typed navigation on every hop, so
    // `Strict` cookies ride along; one with a cross-site initiator sends
    // `Lax` only. The URL is updated first so the context is computed
    // against the target, and nothing is pinned for later hops.
    request.url = target;
    let context = request.same_site_context();
    // A top-level navigation that changes site changes the site for cookies
    // with it, which is what CHIPS partitions and `SameSite` are keyed on.
    if context.top_level_navigation
        && let Some(site) = Site::from_url(&request.url)
    {
        request.first_party = Some(site);
    }
    Ok(request)
}

fn is_payload_header(name: &str) -> bool {
    name.eq_ignore_ascii_case("content-length")
        || name.eq_ignore_ascii_case("content-type")
        || name.eq_ignore_ascii_case("content-encoding")
        || name.eq_ignore_ascii_case("content-language")
        || name.eq_ignore_ascii_case("content-location")
}

fn is_credential_header(name: &str) -> bool {
    name.eq_ignore_ascii_case("authorization")
        || name.eq_ignore_ascii_case("cookie")
        || name.eq_ignore_ascii_case("proxy-authorization")
}

fn origin(url: &Url) -> (String, String, u16) {
    (
        url.scheme().to_owned(),
        url.host_str().unwrap_or_default().to_owned(),
        url.port_or_known_default().unwrap_or(0),
    )
}

/// `strict-origin-when-cross-origin`: the full URL within an origin, the
/// origin across origins, nothing when https downgrades to http.
/// Runs `future` under the request deadline.
async fn with_deadline<F: std::future::Future>(
    deadline: Option<Instant>,
    future: F,
    what: &str,
) -> Result<F::Output, NetError> {
    match deadline {
        None => Ok(future.await),
        Some(deadline) => {
            let remaining = deadline.saturating_duration_since(Instant::now());
            tokio::time::timeout(remaining, future).await.map_err(|_| {
                NetError::new(ErrorName::TimedOut, format!("{what} exceeded the deadline"))
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goaway_readiness_failure_evicts_the_pool_but_refused_stream_does_not() {
        let goaway = NetError::new(ErrorName::Http2ProtocolError, "GOAWAY")
            .with_replay_hint(ReplayHint::H2GoAway)
            .mark_unsent_if_unknown();
        assert!(should_evict_h2(&goaway));

        let unsent = NetError::new(ErrorName::Http2ProtocolError, "readiness failure")
            .mark_unsent_if_unknown();
        assert!(should_evict_h2(&unsent));

        let refused = NetError::new(ErrorName::Http2ProtocolError, "REFUSED_STREAM")
            .with_replay_hint(ReplayHint::H2RefusedStream);
        assert!(!should_evict_h2(&refused));
    }

    fn url(text: &str) -> Url {
        Url::parse(text).expect("url")
    }

    #[test]
    fn redirect_targets_are_resolved_against_the_request_url() {
        let headers = vec![("location".to_owned(), HeaderValue::from_static("/next?a=1"))];
        let base = url("https://example.com/here");
        assert_eq!(
            redirect_location(&StatusCode::FOUND, &headers, &base).expect("resolves"),
            Some(url("https://example.com/next?a=1"))
        );
        assert_eq!(
            redirect_location(&StatusCode::OK, &headers, &base).expect("not a redirect"),
            None
        );
        assert_eq!(
            redirect_location(&StatusCode::FOUND, &[], &base).expect("not a redirect"),
            None,
            "a 302 without Location is not a redirect"
        );
        let broken = vec![(
            "location".to_owned(),
            HeaderValue::from_static("http://[::1"),
        )];
        let error = redirect_location(&StatusCode::FOUND, &broken, &base)
            .expect_err("an unresolvable Location fails like Chrome");
        assert_eq!(error.name, ErrorName::InvalidRedirect);
    }

    #[test]
    fn a_303_turns_a_post_into_a_get_and_drops_the_body() {
        let request = Request::post("https://example.com/form", "a=1")
            .expect("request")
            .header("Content-Type", "text/plain")
            .expect("header");
        let followed = follow_redirect(
            request,
            &StatusCode::SEE_OTHER,
            url("https://example.com/done"),
        )
        .expect("redirect");
        assert_eq!(followed.method, Method::GET);
        assert!(followed.body.is_empty());
        assert!(followed.header_value("content-type").is_none());
    }

    #[test]
    fn a_307_keeps_the_method_and_the_body() {
        let request = Request::post("https://example.com/form", "a=1").expect("request");
        let followed = follow_redirect(
            request,
            &StatusCode::TEMPORARY_REDIRECT,
            url("https://example.com/elsewhere"),
        )
        .expect("redirect");
        assert_eq!(followed.method, Method::POST);
        assert_eq!(followed.body.length(), Some(3));
    }

    #[test]
    fn credentials_are_stripped_across_origins_only() {
        let request = Request::get("https://example.com/")
            .expect("request")
            .header("Authorization", "Bearer secret")
            .expect("header");
        let same_site = follow_redirect(
            request,
            &StatusCode::FOUND,
            url("https://example.com/other"),
        )
        .expect("redirect");
        assert!(same_site.header_value("authorization").is_some());

        let cross = follow_redirect(same_site, &StatusCode::FOUND, url("https://other.example/"))
            .expect("redirect");
        assert!(cross.header_value("authorization").is_none());
    }

    #[test]
    fn a_redirect_keeps_the_original_referrer_and_never_invents_one() {
        // A driver navigation has no referrer; Chrome does not add one on a
        // redirect, and the intermediate hop is never disclosed.
        let request = Request::get("https://example.com/start").expect("request");
        let hop = follow_redirect(request, &StatusCode::FOUND, url("https://example.com/next"))
            .expect("redirect");
        assert_eq!(hop.referrer, None);
        assert!(hop.header_value("referer").is_none());

        // A request that had one keeps it verbatim across hops.
        let with_referrer = Request::get("https://example.com/start")
            .expect("request")
            .referrer(url("https://example.com/page"));
        let hop = follow_redirect(
            with_referrer,
            &StatusCode::FOUND,
            url("https://other.example/next"),
        )
        .expect("redirect");
        assert_eq!(
            hop.referrer.as_ref().map(Url::as_str),
            Some("https://example.com/page"),
            "hop N's Referer comes from the original referrer, not from hop N-1"
        );
    }

    #[test]
    fn a_cross_site_hop_on_top_level_navigation_ignores_redirect_chain_for_cookies() {
        // Chrome computes the SameSite context of every hop from the
        // navigation's initiator and the hop's own URL, never from the
        // previous hop (CookieSameSiteConsidersRedirectChain is off): a hop
        // to another site is cross-site (Lax cookies ride along on the
        // top-level GET, Strict ones do not), and a bounce back to the
        // initiator's site is same-site again as if the bounce never happened.
        let request = Request::get("https://example.com/start")
            .expect("request")
            .resource_type(crate::request::ResourceType::Document)
            .initiator(url("https://example.com/"))
            .first_party(Site::from_url(&url("https://example.com/")).expect("site"));
        assert_eq!(request.fetch_site(), FetchSite::SameOrigin);
        assert!(request.same_site_context().same_site);

        let hop = follow_redirect(
            request,
            &StatusCode::FOUND,
            url("https://other.example/next"),
        )
        .expect("redirect");
        assert_eq!(hop.fetch_site(), FetchSite::CrossSite);
        let context = hop.same_site_context();
        assert!(
            !context.same_site,
            "initiator example.com to other.example is cross-site: Lax yes, Strict no"
        );
        assert!(context.top_level_navigation && context.safe_method);
        assert_eq!(
            hop.initiator.as_ref().map(Url::as_str),
            Some("https://example.com/"),
            "the initiator survives the hop"
        );
        assert_eq!(
            hop.first_party
                .as_ref()
                .map(|site| site.registrable_domain.as_str()),
            Some("other.example"),
            "a top-level navigation moves the site for cookies with it"
        );

        let back = follow_redirect(hop, &StatusCode::FOUND, url("https://example.com/landing"))
            .expect("redirect");
        assert!(
            back.same_site_context().same_site,
            "back on the initiator's site the bounce through other.example is ignored"
        );
        assert_eq!(
            back.first_party
                .as_ref()
                .map(|site| site.registrable_domain.as_str()),
            Some("example.com")
        );
    }

    #[test]
    fn a_redirect_to_another_scheme_is_refused() {
        let request = Request::get("https://example.com/").expect("request");
        let err = follow_redirect(request, &StatusCode::FOUND, url("ftp://example.com/"))
            .expect_err("must refuse");
        assert_eq!(err.name, ErrorName::UnsafeRedirect);
    }

    #[test]
    fn request_targets_are_origin_form() {
        assert_eq!(request_target(&url("https://example.com")), "/");
        assert_eq!(request_target(&url("https://example.com/a/b")), "/a/b");
        assert_eq!(
            request_target(&url("https://example.com/a?b=c#d")),
            "/a?b=c"
        );
    }
}
