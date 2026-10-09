//! The protocol engine: sessions, version handling and request dispatch.
//!
//! One [`Connection`] is one agent: a stdio process, or one HTTP session. It
//! owns the agent's browser (opened on its first tool call) and answers
//! JSON-RPC messages. The transports only move messages in and out.
//!
//! Two protocol eras are served side by side, as the 2026-07-28 revision
//! allows. A request that carries `_meta["io.modelcontextprotocol/protocolVersion"]`
//! is stateless and gets the 2026-07-28 result shapes (`resultType`,
//! `ttlMs`, `cacheScope`). `initialize` opens the session-based flow of the
//! 2024-11-05 to 2025-11-25 revisions, which most clients still speak.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};
use tenta_browser::{BrowserContext, Engine};

use crate::error::{ErrorKind, ToolError};
use crate::jsonrpc::{self, INTERNAL_ERROR, Message, RpcError};
use crate::session::{BrowserSession, McpOptions};
use crate::tools;

pub const SERVER_NAME: &str = "tentabrowser";

/// The revision without handshake or sessions.
pub const MODERN_VERSION: &str = "2026-07-28";

/// Revisions that open with `initialize`, newest first.
pub const LEGACY_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const META_PROTOCOL_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const META_SERVER_INFO: &str = "io.modelcontextprotocol/serverInfo";

/// The longest one tool call may run, whatever its own deadlines say.
const CALL_CAP: Duration = Duration::from_secs(180);

/// Sessions idle this long are dropped when a new one is needed.
const SESSION_IDLE: Duration = Duration::from_secs(30 * 60);

/// Sessions one source address may hold at once.
pub const MAX_SESSIONS_PER_SOURCE: usize = 8;

const INSTRUCTIONS: &str = "TentaBrowser is a headless browser. Open a page with browser_navigate, read it with \
browser_snapshot (an outline whose elements carry refs such as e12), and act with browser_click, browser_type, \
browser_fill and browser_press_key using those refs. Refs go stale when the page navigates or the element is \
removed: take a new snapshot. Acting tools wait for the page to settle, which means load-driven work finished, \
not that content arriving later over sockets or workers has rendered: use browser_wait_for for that. Prefer \
browser_snapshot to browser_screenshot; use a screenshot to check layout or visuals.";

/// Every protocol revision this server speaks, newest first.
pub fn supported_versions() -> Vec<&'static str> {
    std::iter::once(MODERN_VERSION)
        .chain(LEGACY_VERSIONS)
        .collect()
}

struct Shared {
    engine: Engine,
    options: Arc<McpOptions>,
    shared_context: Mutex<Option<BrowserContext>>,
    sessions: Mutex<HashMap<String, Arc<Connection>>>,
    open_connections: AtomicUsize,
}

/// The MCP server: an engine, how sessions are made, and the registry of
/// streamable HTTP sessions.
#[derive(Clone)]
pub struct McpServer {
    shared: Arc<Shared>,
}

impl McpServer {
    pub fn new(engine: Engine, options: McpOptions) -> Self {
        Self {
            shared: Arc::new(Shared {
                engine,
                options: Arc::new(options),
                shared_context: Mutex::default(),
                sessions: Mutex::default(),
                open_connections: AtomicUsize::new(0),
            }),
        }
    }

    pub fn engine(&self) -> &Engine {
        &self.shared.engine
    }

    pub fn options(&self) -> &McpOptions {
        &self.shared.options
    }

    /// A new agent connection (a stdio process, or an in-process client).
    /// Fails when `max_sessions` connections are already open.
    pub fn connect(&self) -> Result<Arc<Connection>, RpcError> {
        let open = self.shared.open_connections.fetch_add(1, Ordering::SeqCst);
        if open >= self.shared.options.max_sessions {
            self.shared.open_connections.fetch_sub(1, Ordering::SeqCst);
            return Err(too_many(self.shared.options.max_sessions));
        }
        Ok(self.make(random_id(), None))
    }

    /// A connection that counts toward `open_connections` without checking
    /// it: registered sessions are limited by [`McpServer::make_room`].
    fn make(&self, id: String, source: Option<IpAddr>) -> Arc<Connection> {
        if source.is_some() || id == "default" {
            self.shared.open_connections.fetch_add(1, Ordering::SeqCst);
        }
        Arc::new(Connection {
            shared: Arc::clone(&self.shared),
            id,
            source,
            created: Instant::now(),
            called: AtomicBool::new(false),
            in_flight: AtomicUsize::new(0),
            cancels: Mutex::default(),
            browser: tokio::sync::Mutex::new(None),
            negotiated: Mutex::new(None),
            last_used: Mutex::new(Instant::now()),
        })
    }

    /// Opens an HTTP session for a client at `source` and registers it.
    ///
    /// Room is made first: expired sessions go (never used for 60 s, or idle
    /// for 30 min). If `source` is still at its cap or the server is full the
    /// answer is 503; a session that is in use is never taken. Unauthenticated
    /// `initialize` calls can hold slots only for the 60 s of the unused TTL.
    pub fn open_session(&self, source: Option<IpAddr>) -> Result<Arc<Connection>, RpcError> {
        self.make_room(source)?;
        let connection = self.make(
            random_id(),
            Some(source.unwrap_or(IpAddr::from([0, 0, 0, 0]))),
        );
        lock(&self.shared.sessions).insert(connection.id.clone(), Arc::clone(&connection));
        Ok(connection)
    }

    pub fn session(&self, id: &str) -> Option<Arc<Connection>> {
        let session = lock(&self.shared.sessions).get(id).cloned()?;
        session.touch();
        Some(session)
    }

    /// The session stateless requests without a session id share. Every
    /// authorised stateless client of one server lands in it.
    pub fn default_session(&self) -> Result<Arc<Connection>, RpcError> {
        if let Some(session) = self.session("default") {
            return Ok(session);
        }
        self.make_room(None)?;
        let connection = self.make("default".to_owned(), None);
        // Two first requests may race; the registered one wins.
        Ok(Arc::clone(
            lock(&self.shared.sessions)
                .entry("default".to_owned())
                .or_insert(connection),
        ))
    }

    fn make_room(&self, source: Option<IpAddr>) -> Result<(), RpcError> {
        let limit = self.shared.options.max_sessions;
        let mut evicted: Vec<Arc<Connection>> = Vec::new();
        let outcome = {
            let mut sessions = lock(&self.shared.sessions);
            let expired: Vec<String> = sessions
                .iter()
                .filter(|(_, c)| c.evictable() && c.expired())
                .map(|(id, _)| id.clone())
                .collect();
            evicted.extend(expired.iter().filter_map(|id| sessions.remove(id)));
            // Only sessions that never made a call or sat idle past the TTL
            // were removed above. A session between two calls of a working
            // client is never taken: when nothing has expired, the answer
            // is "full".
            let mut outcome = Ok(());
            if let Some(source) = source {
                let held = sessions
                    .values()
                    .filter(|c| c.source == Some(source))
                    .count();
                if held >= MAX_SESSIONS_PER_SOURCE {
                    outcome = Err(too_many(MAX_SESSIONS_PER_SOURCE));
                }
            }
            if outcome.is_ok() && sessions.len() >= limit {
                outcome = Err(too_many(limit));
            }
            outcome
        };
        for connection in evicted {
            tokio::spawn(async move { connection.close().await });
        }
        outcome
    }

    /// Ends an HTTP session; its browser is closed.
    pub async fn close_session(&self, id: &str) -> bool {
        let removed = lock(&self.shared.sessions).remove(id);
        match removed {
            Some(connection) => {
                connection.close().await;
                true
            }
            None => false,
        }
    }

    /// Closes every HTTP session. The engine stays the caller's.
    pub async fn close_all(&self) {
        let all: Vec<Arc<Connection>> = lock(&self.shared.sessions)
            .drain()
            .map(|(_, connection)| connection)
            .collect();
        for connection in all {
            connection.close().await;
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn too_many(limit: usize) -> RpcError {
    RpcError::new(
        INTERNAL_ERROR,
        format!("too many sessions (limit {limit}); try again shortly"),
    )
}

pub(crate) fn random_id() -> String {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).expect("the operating system provides randomness");
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// One agent's protocol state and browser.
pub struct Connection {
    shared: Arc<Shared>,
    id: String,
    /// Where an HTTP session was opened from, for the per-source cap.
    source: Option<IpAddr>,
    created: Instant,
    /// Whether the session ever made a tool call.
    called: AtomicBool,
    /// Requests being answered now; a session with any is never evicted.
    in_flight: AtomicUsize,
    /// Requests that `notifications/cancelled` can stop, by request id.
    cancels: Mutex<HashMap<String, Arc<tokio::sync::Notify>>>,
    browser: tokio::sync::Mutex<Option<BrowserSession>>,
    /// The revision `initialize` agreed on, for the legacy flow.
    negotiated: Mutex<Option<String>>,
    last_used: Mutex<Instant>,
}

impl Connection {
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The revision `initialize` agreed on, if the client has initialized.
    pub fn negotiated_version(&self) -> Option<String> {
        lock(&self.negotiated).clone()
    }

    fn evictable(&self) -> bool {
        self.in_flight.load(Ordering::SeqCst) == 0
    }

    fn expired(&self) -> bool {
        let idle = self.idle_for();
        idle > SESSION_IDLE
            || (!self.called.load(Ordering::SeqCst)
                && self.created.elapsed() > self.shared.options.unused_session_ttl)
    }

    fn touch(&self) {
        *lock(&self.last_used) = Instant::now();
    }

    fn idle_for(&self) -> Duration {
        lock(&self.last_used).elapsed()
    }

    /// Answers one message. `None` for a notification or a client response.
    ///
    /// A JSON array is a batch, which only the 2024-11-05 and 2025-03-26
    /// revisions have; it is answered with an array of the responses.
    pub async fn handle(&self, message: Value) -> Option<Value> {
        let Value::Array(items) = message else {
            return self.handle_one(message).await;
        };
        let allowed = matches!(
            self.negotiated_version().as_deref(),
            Some("2024-11-05" | "2025-03-26")
        );
        if !allowed || items.is_empty() {
            return Some(jsonrpc::failure(
                &Value::Null,
                &RpcError::new(
                    jsonrpc::INVALID_REQUEST,
                    "JSON-RPC batches exist only in protocol revisions 2024-11-05 and 2025-03-26, and may not be empty",
                ),
            ));
        }
        let mut replies = Vec::new();
        for item in items {
            replies.extend(self.handle_one(item).await);
        }
        (!replies.is_empty()).then_some(Value::Array(replies))
    }

    async fn handle_one(&self, message: Value) -> Option<Value> {
        self.touch();
        match jsonrpc::classify(message) {
            Err((id, error)) => Some(jsonrpc::failure(&id, &error)),
            Ok(Message::Response) => None,
            Ok(Message::Notification { method, params }) => {
                if method == "notifications/cancelled"
                    && let Some(id) = params.get("requestId")
                    && let Some(cancel) = lock(&self.cancels).get(&id.to_string())
                {
                    cancel.notify_one();
                }
                None
            }
            Ok(Message::Request { id, method, params }) => {
                let cancel = Arc::new(tokio::sync::Notify::new());
                let _in_flight = InFlight::enter(self, id.to_string(), &cancel);
                let reply = tokio::select! {
                    reply = self.dispatch(&method, params) => reply,
                    () = cancel.notified() => return None,
                };
                Some(match reply {
                    Ok(result) => jsonrpc::success(&id, result),
                    Err(error) => jsonrpc::failure(&id, &error),
                })
            }
        }
    }

    async fn dispatch(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let modern = request_version(&params).is_some();
        if let Some(requested) = request_version(&params)
            && !supported_versions().contains(&requested.as_str())
        {
            return Err(unsupported_version(&requested));
        }
        match method {
            "initialize" => self.initialize(&params),
            "server/discover" => Ok(discover()),
            // `ping` was removed in 2026-07-28; the stateless era has no use for it.
            "ping" if !modern => Ok(json!({})),
            "tools/list" => Ok(with_era(
                json!({ "tools": tools::list() }),
                modern,
                Some(json!({"ttlMs": 3_600_000, "cacheScope": "public"})),
            )),
            "tools/call" => {
                let result = self.call_tool(&params).await?;
                Ok(with_era(result, modern, None))
            }
            other => Err(RpcError::method_not_found(other)),
        }
    }

    fn initialize(&self, params: &Value) -> Result<Value, RpcError> {
        let requested = params
            .get("protocolVersion")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::invalid_params("initialize needs a protocolVersion string"))?;
        // The client's revision when this server speaks it, else its newest
        // session-based one; the client then decides whether to continue.
        let agreed = LEGACY_VERSIONS
            .iter()
            .find(|version| **version == requested)
            .copied()
            .unwrap_or(LEGACY_VERSIONS[0]);
        *lock(&self.negotiated) = Some(agreed.to_owned());
        Ok(json!({
            "protocolVersion": agreed,
            "capabilities": capabilities(),
            "serverInfo": server_info(),
            "instructions": INSTRUCTIONS,
        }))
    }

    async fn call_tool(&self, params: &Value) -> Result<Value, RpcError> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::invalid_params("tools/call needs a tool name"))?;
        let tool = tools::find(name)
            .ok_or_else(|| RpcError::invalid_params(format!("unknown tool: {name}")))?;
        let arguments = match params.get("arguments") {
            None | Some(Value::Null) => json!({}),
            Some(arguments @ Value::Object(_)) => arguments.clone(),
            Some(_) => return Err(RpcError::invalid_params("arguments must be an object")),
        };
        tracing::debug!(tool = name, "tools/call");
        self.called.store(true, Ordering::SeqCst);
        let mut browser = self.browser.lock().await;
        if browser.is_none() {
            match self.open_browser() {
                Ok(session) => *browser = Some(session),
                Err(error) => return Ok(error.to_result()),
            }
        }
        let session = browser.as_mut().expect("opened above");
        let outcome = tokio::time::timeout(CALL_CAP, tool.call(session, arguments)).await;
        Ok(match outcome {
            Ok(Ok(output)) => output.into_result(),
            Ok(Err(error)) => error.to_result(),
            Err(_) => ToolError::new(
                ErrorKind::Timeout,
                format!("{name} did not finish within {} s", CALL_CAP.as_secs()),
            )
            .retryable()
            .to_result(),
        })
    }

    fn open_browser(&self) -> Result<BrowserSession, ToolError> {
        let shared_context = if self.shared.options.share_context {
            let mut slot = lock(&self.shared.shared_context);
            if slot.is_none() {
                *slot = Some(
                    self.shared
                        .engine
                        .new_context(self.shared.options.context.clone())?,
                );
            }
            slot.clone()
        } else {
            None
        };
        BrowserSession::new(
            self.shared.engine.clone(),
            Arc::clone(&self.shared.options),
            shared_context,
        )
    }

    /// Closes the agent's browser. Idempotent.
    pub async fn close(&self) {
        // Calls in flight hold the browser; end them so closing is prompt.
        for cancel in lock(&self.cancels).values() {
            cancel.notify_one();
        }
        let session = self.browser.lock().await.take();
        if let Some(mut session) = session {
            session.close().await;
        }
    }
}

/// A request being answered: counts against eviction and can be cancelled.
struct InFlight<'a> {
    connection: &'a Connection,
    key: String,
}

impl<'a> InFlight<'a> {
    fn enter(connection: &'a Connection, key: String, cancel: &Arc<tokio::sync::Notify>) -> Self {
        connection.in_flight.fetch_add(1, Ordering::SeqCst);
        lock(&connection.cancels).insert(key.clone(), Arc::clone(cancel));
        Self { connection, key }
    }
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        lock(&self.connection.cancels).remove(&self.key);
        self.connection.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.shared.open_connections.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The protocol version a stateless request declares.
pub fn request_version(params: &Value) -> Option<String> {
    params
        .get("_meta")?
        .get(META_PROTOCOL_VERSION)?
        .as_str()
        .map(str::to_owned)
}

pub fn unsupported_version(requested: &str) -> RpcError {
    RpcError::new(
        jsonrpc::UNSUPPORTED_PROTOCOL_VERSION,
        "Unsupported protocol version",
    )
    .with_data(json!({
        "supported": supported_versions(),
        "requested": requested,
    }))
}

fn server_info() -> Value {
    json!({
        "name": SERVER_NAME,
        "title": "TentaBrowser",
        "version": env!("CARGO_PKG_VERSION"),
    })
}

fn capabilities() -> Value {
    json!({ "tools": { "listChanged": false } })
}

fn discover() -> Value {
    with_era(
        json!({
            "supportedVersions": supported_versions(),
            "capabilities": capabilities(),
            "instructions": INSTRUCTIONS,
        }),
        true,
        Some(json!({"ttlMs": 3_600_000, "cacheScope": "public"})),
    )
}

/// Adds the 2026-07-28 result fields to a result for a stateless request.
fn with_era(mut result: Value, modern: bool, cacheable: Option<Value>) -> Value {
    if !modern {
        return result;
    }
    if let Some(object) = result.as_object_mut() {
        object.insert("resultType".to_owned(), json!("complete"));
        if let Some(Value::Object(cache)) = cacheable {
            object.extend(cache);
        }
        let mut meta = Map::new();
        meta.insert(META_SERVER_INFO.to_owned(), server_info());
        object.insert("_meta".to_owned(), Value::Object(meta));
    }
    result
}
