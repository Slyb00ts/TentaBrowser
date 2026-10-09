//! The streamable HTTP transport (`docs/spec/10-mcp.md` §1,
//! `docs/spec/19-security.md` §3).
//!
//! One endpoint takes `POST`: a request is answered with one JSON object, a
//! notification with `202 Accepted`. There is no server-initiated stream, so
//! `GET` is `405`, as the specification allows. The listener binds loopback
//! by default. Every request is checked for a `Host` that is an IP address,
//! `localhost` or an allowed name, for an `Origin` on the allow-list (a
//! request that carries none, as a non-browser client's does not, passes), and
//! for the bearer token when one is set. A bind that is not loopback refuses
//! to start without a token of [`MIN_REMOTE_TOKEN_LEN`] bytes.
//!
//! Both protocol eras share the endpoint: a request carrying the
//! 2026-07-28 per-request metadata is stateless and its HTTP headers must
//! mirror the body; `initialize` opens a session whose id the server returns
//! in `Mcp-Session-Id`.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, watch};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::jsonrpc::{self, Message, RpcError};
use crate::server::{
    Connection, McpServer, request_version, supported_versions, unsupported_version,
};

/// Shortest bearer token accepted for a bind reachable from the network.
pub const MIN_REMOTE_TOKEN_LEN: usize = 16;

const MAX_HEAD_BYTES: usize = 16 * 1024;
const MAX_HEADERS: usize = 64;
const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;
const HEAD_TIMEOUT: Duration = Duration::from_secs(10);
const BODY_TIMEOUT: Duration = Duration::from_secs(30);
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_CONNECTIONS: usize = 64;

/// JSON-RPC code for a request ended by `notifications/cancelled`.
const CANCELLED: i64 = -32800;

/// How the HTTP endpoint listens and who may use it.
#[derive(Clone)]
pub struct HttpOptions {
    pub bind: SocketAddr,
    /// The path of the MCP endpoint.
    pub path: String,
    /// Bearer token. Required (at least [`MIN_REMOTE_TOKEN_LEN`] bytes) when
    /// `bind` is not loopback; allowed on loopback too.
    pub token: Option<String>,
    /// `Origin` values accepted, compared exactly and without regard to case.
    /// A request without an `Origin` header is not a browser's and passes.
    pub allowed_origins: Vec<String>,
    /// Host names accepted in addition to IP addresses and `localhost`.
    pub allowed_hosts: Vec<String>,
}

impl Default for HttpOptions {
    fn default() -> Self {
        Self {
            bind: SocketAddr::from(([127, 0, 0, 1], 8931)),
            path: "/mcp".to_owned(),
            token: None,
            allowed_origins: Vec::new(),
            allowed_hosts: Vec::new(),
        }
    }
}

impl std::fmt::Debug for HttpOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HttpOptions")
            .field("bind", &self.bind)
            .field("path", &self.path)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .field("allowed_origins", &self.allowed_origins)
            .field("allowed_hosts", &self.allowed_hosts)
            .finish()
    }
}

impl HttpOptions {
    /// Checked before the socket is bound, so a refused configuration never
    /// opens a port.
    pub fn validate(&self) -> Result<(), String> {
        if !self.path.starts_with('/') {
            return Err("the endpoint path must start with '/'".to_owned());
        }
        if let Some(token) = &self.token
            && (token.is_empty() || !token.bytes().all(|byte| (0x21..=0x7e).contains(&byte)))
        {
            return Err("the token must be non-empty printable ASCII without spaces".to_owned());
        }
        if !self.bind.ip().is_loopback() {
            match &self.token {
                Some(token) if token.len() >= MIN_REMOTE_TOKEN_LEN => {}
                _ => {
                    return Err(format!(
                        "refusing to bind {}: a non-loopback MCP endpoint needs a bearer token of at least {MIN_REMOTE_TOKEN_LEN} bytes",
                        self.bind
                    ));
                }
            }
        }
        Ok(())
    }
}

/// A running HTTP endpoint.
pub struct HttpServer {
    local_addr: SocketAddr,
    path: String,
    server: McpServer,
    shutdown: watch::Sender<bool>,
    accept: JoinHandle<()>,
}

struct Env {
    server: McpServer,
    options: HttpOptions,
}

impl HttpServer {
    pub async fn bind(server: McpServer, options: HttpOptions) -> Result<Self, String> {
        options.validate()?;
        let listener = TcpListener::bind(options.bind)
            .await
            .map_err(|error| format!("binding {}: {error}", options.bind))?;
        let local_addr = listener.local_addr().map_err(|error| error.to_string())?;
        let (shutdown, mut stop) = watch::channel(false);
        let path = options.path.clone();
        let env = Arc::new(Env {
            server: server.clone(),
            options,
        });
        let permits = Arc::new(Semaphore::new(MAX_CONNECTIONS));
        let accept = tokio::spawn(async move {
            loop {
                let (stream, _) = tokio::select! {
                    _ = stop.changed() => return,
                    accepted = listener.accept() => match accepted {
                        Ok(accepted) => accepted,
                        Err(_) => continue,
                    },
                };
                let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
                    // Over the connection limit: drop the socket.
                    continue;
                };
                let env = Arc::clone(&env);
                tokio::spawn(async move {
                    serve_connection(stream, &env).await;
                    drop(permit);
                });
            }
        });
        Ok(Self {
            local_addr,
            path,
            server,
            shutdown,
            accept,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// The endpoint URL clients connect to.
    pub fn url(&self) -> String {
        format!("http://{}{}", self.local_addr, self.path)
    }

    /// Stops accepting and closes every session's browser.
    pub async fn shutdown(self) {
        let _ = self.shutdown.send(true);
        let _ = self.accept.await;
        self.server.close_all().await;
    }
}

// ---- requests -----------------------------------------------------------

struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    /// Bytes read after the head.
    leftover: Vec<u8>,
    /// The HTTP minor version (`1` for HTTP/1.1).
    version: u8,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.trim())
    }
}

enum Head {
    Request(Request),
    Closed,
    TooLarge,
    Malformed,
}

async fn read_head(stream: &mut TcpStream) -> Head {
    let mut buffer = Vec::with_capacity(1024);
    let mut chunk = [0_u8; 2048];
    loop {
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => return Head::Closed,
            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
        }
        let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
        let mut parsed = httparse::Request::new(&mut headers);
        match parsed.parse(&buffer) {
            Ok(httparse::Status::Complete(head_len)) => {
                let target = parsed.path.unwrap_or("/");
                let path = target.split_once('?').map_or(target, |(path, _)| path);
                return Head::Request(Request {
                    method: parsed.method.unwrap_or("").to_owned(),
                    path: path.to_owned(),
                    headers: parsed
                        .headers
                        .iter()
                        .map(|header| {
                            (
                                header.name.to_owned(),
                                String::from_utf8_lossy(header.value).into_owned(),
                            )
                        })
                        .collect(),
                    leftover: buffer[head_len..].to_vec(),
                    version: parsed.version.unwrap_or(1),
                });
            }
            Ok(httparse::Status::Partial) if buffer.len() >= MAX_HEAD_BYTES => {
                return Head::TooLarge;
            }
            Ok(httparse::Status::Partial) => {}
            Err(_) => return Head::Malformed,
        }
    }
}

struct Reply {
    status: u16,
    reason: &'static str,
    content_type: &'static str,
    body: String,
    extra_headers: Vec<(&'static str, String)>,
}

impl Reply {
    fn text(status: u16, reason: &'static str, body: &str) -> Self {
        Self {
            status,
            reason,
            content_type: "text/plain; charset=UTF-8",
            body: body.to_owned(),
            extra_headers: Vec::new(),
        }
    }

    fn json(status: u16, reason: &'static str, body: &Value) -> Self {
        Self {
            status,
            reason,
            content_type: "application/json",
            body: body.to_string(),
            extra_headers: Vec::new(),
        }
    }

    fn accepted() -> Self {
        Self::text(202, "Accepted", "")
    }

    fn with_header(mut self, name: &'static str, value: String) -> Self {
        self.extra_headers.push((name, value));
        self
    }

    /// A JSON-RPC error carried in an HTTP error status.
    fn rpc_error(status: u16, reason: &'static str, error: &RpcError) -> Self {
        Self::json(status, reason, &jsonrpc::failure(&Value::Null, error))
    }
}

async fn write_reply(stream: &mut TcpStream, reply: Reply) {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n",
        reply.status,
        reply.reason,
        reply.content_type,
        reply.body.len()
    );
    for (name, value) in &reply.extra_headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    let write = async {
        stream.write_all(head.as_bytes()).await?;
        stream.write_all(reply.body.as_bytes()).await?;
        stream.shutdown().await
    };
    let _ = timeout(WRITE_TIMEOUT, write).await;
}

async fn serve_connection(mut stream: TcpStream, env: &Env) {
    let source = stream.peer_addr().ok().map(|peer| peer.ip());
    let head = match timeout(HEAD_TIMEOUT, read_head(&mut stream)).await {
        Ok(Head::Request(request)) => request,
        Ok(Head::TooLarge) => {
            write_reply(
                &mut stream,
                Reply::text(
                    431,
                    "Request Header Fields Too Large",
                    "Request head too large",
                ),
            )
            .await;
            return;
        }
        Ok(Head::Malformed) => {
            write_reply(
                &mut stream,
                Reply::text(400, "Bad Request", "Malformed HTTP request"),
            )
            .await;
            return;
        }
        Ok(Head::Closed) | Err(_) => return,
    };
    if let Some(reply) = admission_failure(&head, &env.options) {
        write_reply(&mut stream, reply).await;
        return;
    }
    if head.path != env.options.path {
        write_reply(
            &mut stream,
            Reply::text(404, "Not Found", "No such endpoint"),
        )
        .await;
        return;
    }
    let reply = match head.method.as_str() {
        "POST" => post(&mut stream, head, source, env).await,
        "DELETE" => delete(&head, env).await,
        _ => Some(
            Reply::text(
                405,
                "Method Not Allowed",
                "Use POST; there is no server-initiated stream",
            )
            .with_header("Allow", "POST, DELETE".to_owned()),
        ),
    };
    if let Some(reply) = reply {
        write_reply(&mut stream, reply).await;
    }
}

// ---- admission ----------------------------------------------------------

/// The host, origin and token rules of `docs/spec/19-security.md` §3.
fn admission_failure(request: &Request, options: &HttpOptions) -> Option<Reply> {
    let hosts = request
        .headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("host"))
        .count();
    if hosts > 1 || (hosts == 0 && request.version >= 1) {
        return Some(Reply::text(
            400,
            "Bad Request",
            "Exactly one Host header is required",
        ));
    }
    if let Some(host) = request.header("host")
        && !host_allowed(host, &options.allowed_hosts)
    {
        return Some(Reply::text(
            403,
            "Forbidden",
            "Host header is not an IP address, localhost or an allowed host",
        ));
    }
    if let Some(origin) = request.header("origin")
        && !options
            .allowed_origins
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(origin))
    {
        return Some(Reply::rpc_error(
            403,
            "Forbidden",
            &RpcError::new(
                jsonrpc::INVALID_REQUEST,
                "this Origin is not allowed; add it with --allowed-origin",
            ),
        ));
    }
    if let Some(expected) = &options.token {
        let given = request
            .header("authorization")
            .and_then(|value| {
                value
                    .strip_prefix("Bearer ")
                    .or_else(|| value.strip_prefix("bearer "))
            })
            .unwrap_or("");
        if !token_matches(expected, given.trim()) {
            return Some(
                Reply::text(401, "Unauthorized", "Missing or invalid bearer token")
                    .with_header("WWW-Authenticate", "Bearer".to_owned()),
            );
        }
    }
    None
}

fn host_allowed(host: &str, allowed_hosts: &[String]) -> bool {
    let is_port = |port: &str| !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit());
    let name = if let Some(rest) = host.strip_prefix('[') {
        let Some((address, tail)) = rest.split_once(']') else {
            return false;
        };
        if !(tail.is_empty() || tail.strip_prefix(':').is_some_and(is_port)) {
            return false;
        }
        address
    } else {
        host.rsplit_once(':')
            .filter(|(_, port)| is_port(port))
            .map_or(host, |(name, _)| name)
    };
    name.parse::<IpAddr>().is_ok()
        || name.eq_ignore_ascii_case("localhost")
        || allowed_hosts
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(name))
}

/// Compares without stopping at the first difference.
fn token_matches(expected: &str, given: &str) -> bool {
    let (expected, given) = (expected.as_bytes(), given.as_bytes());
    let mut difference = expected.len() ^ given.len();
    for (index, byte) in expected.iter().enumerate() {
        difference |= usize::from(byte ^ given.get(index).copied().unwrap_or(0));
    }
    // Keeps the optimiser from turning the loop back into an early exit.
    std::hint::black_box(difference) == 0
}

// ---- POST and DELETE ----------------------------------------------------

async fn read_body(stream: &mut TcpStream, head: &Request) -> Result<Vec<u8>, Reply> {
    if head.header("transfer-encoding").is_some() {
        return Err(Reply::text(
            411,
            "Length Required",
            "Send the body with a Content-Length, not chunked",
        ));
    }
    let length = head
        .header("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .ok_or_else(|| Reply::text(411, "Length Required", "A Content-Length is required"))?;
    if length > MAX_BODY_BYTES {
        return Err(Reply::text(
            413,
            "Payload Too Large",
            "The message is too large",
        ));
    }
    let mut body = head.leftover.clone();
    body.truncate(length);
    let read = async {
        let mut chunk = [0_u8; 8192];
        while body.len() < length {
            let wanted = (length - body.len()).min(chunk.len());
            match stream.read(&mut chunk[..wanted]).await {
                Ok(0) | Err(_) => return false,
                Ok(read) => body.extend_from_slice(&chunk[..read]),
            }
        }
        true
    };
    match timeout(BODY_TIMEOUT, read).await {
        Ok(true) => Ok(body),
        _ => Err(Reply::text(400, "Bad Request", "The body ended early")),
    }
}

/// A header value, decoded when it uses the `=?base64?...?=` form.
fn decode_header_value(value: &str) -> Option<String> {
    match value
        .strip_prefix("=?base64?")
        .and_then(|rest| rest.strip_suffix("?="))
    {
        Some(encoded) => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .ok()?;
            String::from_utf8(bytes).ok()
        }
        None => Some(value.to_owned()),
    }
}

fn header_mismatch(detail: &str) -> Reply {
    Reply::rpc_error(
        400,
        "Bad Request",
        &RpcError::new(
            jsonrpc::HEADER_MISMATCH,
            format!("Header mismatch: {detail}"),
        ),
    )
}

/// The 2026-07-28 rule: the headers mirror the body.
fn check_mirrored_headers(head: &Request, message: &Value, version: &str) -> Result<(), Reply> {
    if head.header("mcp-protocol-version") != Some(version) {
        return Err(header_mismatch(
            "MCP-Protocol-Version is missing or differs from the request's protocolVersion",
        ));
    }
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    if head.header("mcp-method") != Some(method) {
        return Err(header_mismatch(
            "Mcp-Method is missing or differs from the method",
        ));
    }
    if method == "tools/call" {
        let name = message
            .pointer("/params/name")
            .and_then(Value::as_str)
            .unwrap_or("");
        let header = head.header("mcp-name").and_then(decode_header_value);
        if header.as_deref() != Some(name) {
            return Err(header_mismatch(
                "Mcp-Name is missing or differs from params.name",
            ));
        }
    }
    Ok(())
}

/// The HTTP status an error response travels with.
fn error_status(reply: &Value) -> (u16, &'static str) {
    match reply.pointer("/error/code").and_then(Value::as_i64) {
        Some(
            jsonrpc::PARSE_ERROR
            | jsonrpc::INVALID_REQUEST
            | jsonrpc::HEADER_MISMATCH
            | jsonrpc::UNSUPPORTED_PROTOCOL_VERSION,
        ) => (400, "Bad Request"),
        _ => (200, "OK"),
    }
}

async fn post(
    stream: &mut TcpStream,
    head: Request,
    source: Option<IpAddr>,
    env: &Env,
) -> Option<Reply> {
    let content_type = head.header("content-type").unwrap_or("");
    if !content_type
        .to_ascii_lowercase()
        .starts_with("application/json")
    {
        return Some(Reply::text(
            415,
            "Unsupported Media Type",
            "The body must be application/json",
        ));
    }
    let body = match read_body(stream, &head).await {
        Ok(body) => body,
        Err(reply) => return Some(reply),
    };
    let message: Value = match serde_json::from_slice(&body) {
        Ok(message) => message,
        Err(error) => {
            return Some(Reply::json(
                400,
                "Bad Request",
                &jsonrpc::parse_failure(&error.to_string()),
            ));
        }
    };
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let modern = request_version(&params);

    let (connection, is_new) = match resolve_session(&head, &method, modern.is_some(), source, env)
    {
        Ok(resolved) => resolved,
        Err(reply) => return Some(reply),
    };
    if let Some(version) = &modern {
        if !supported_versions().contains(&version.as_str()) {
            let error = unsupported_version(version);
            return Some(Reply::rpc_error(400, "Bad Request", &error));
        }
        if let Err(reply) = check_mirrored_headers(&head, &message, version) {
            return Some(reply);
        }
    } else if let Some(version) = head.header("mcp-protocol-version")
        && !supported_versions().contains(&version)
    {
        return Some(Reply::rpc_error(
            400,
            "Bad Request",
            &unsupported_version(version),
        ));
    }

    let is_request = matches!(
        jsonrpc::classify(message.clone()),
        Ok(Message::Request { .. })
    );
    let is_request = is_request || message.is_array();
    // A reset connection is the cancellation of its request; so is a
    // `notifications/cancelled` naming it, which `handle` honours.
    let answered = tokio::select! {
        reply = connection.handle(message) => Some(reply),
        () = client_reset(stream) => None,
    };
    let reply = answered?;
    let Some(reply) = reply else {
        return Some(if is_request {
            // Cancelled by a notification: the protocol sends no result.
            Reply::rpc_error(
                200,
                "OK",
                &RpcError::new(CANCELLED, "the request was cancelled"),
            )
        } else {
            Reply::accepted()
        });
    };
    let (status, reason) = error_status(&reply);
    let mut out = Reply::json(status, reason, &reply);
    if reply.pointer("/error/code").and_then(Value::as_i64) == Some(jsonrpc::METHOD_NOT_FOUND)
        && modern.is_some()
    {
        out.status = 404;
        out.reason = "Not Found";
    }
    if is_new && reply.get("result").is_some() {
        out = out.with_header("Mcp-Session-Id", connection.id().to_owned());
    } else if is_new {
        // `initialize` failed: no session to keep.
        env.server.close_session(connection.id()).await;
    }
    Some(out)
}

/// How often a connection whose client sent end of file is probed.
const PROBE_INTERVAL: Duration = Duration::from_millis(400);

/// Resolves when the client is gone.
///
/// End of file alone does not say so: a client may have shut down only its
/// write side and still wait for the answer, and a fetch that was aborted
/// closes both sides with the same FIN. After end of file the connection is
/// probed with an interim `100 Continue` response, which every HTTP/1.1
/// client accepts and ignores (TCP offers no way to tell the two apart without
/// writing); writing to a socket whose peer is gone fails
/// within a probe or two. A reset fails the read at once.
async fn client_reset(stream: &mut TcpStream) {
    let mut probe = [0_u8; 1];
    loop {
        match stream.read(&mut probe).await {
            Err(_) => return,
            Ok(0) => break,
            // Pipelined bytes are ignored: one request per connection.
            Ok(_) => {}
        }
    }
    loop {
        tokio::time::sleep(PROBE_INTERVAL).await;
        if stream
            .write_all(b"HTTP/1.1 100 Continue\r\n\r\n")
            .await
            .is_err()
        {
            return;
        }
    }
}

/// The session a POST belongs to, and whether it was just created.
fn resolve_session(
    head: &Request,
    method: &str,
    modern: bool,
    source: Option<IpAddr>,
    env: &Env,
) -> Result<(Arc<Connection>, bool), Reply> {
    let no_capacity = |error: RpcError| Reply::rpc_error(503, "Service Unavailable", &error);
    if !modern && method == "initialize" {
        return env
            .server
            .open_session(source)
            .map(|connection| (connection, true))
            .map_err(no_capacity);
    }
    // Stateless requests carry everything themselves: a session id left over
    // from an earlier handshake (or a restart) is ignored, not an error.
    let id = head.header("mcp-session-id");
    if modern {
        let known = id.and_then(|id| env.server.session(id));
        return match known {
            Some(connection) => Ok((connection, false)),
            None => env
                .server
                .default_session()
                .map(|connection| (connection, false))
                .map_err(no_capacity),
        };
    }
    match id {
        Some(id) => env
            .server
            .session(id)
            .map(|connection| (connection, false))
            .ok_or_else(|| {
                Reply::rpc_error(
                    404,
                    "Not Found",
                    &RpcError::new(
                        jsonrpc::INVALID_REQUEST,
                        "unknown or expired session; initialize again",
                    ),
                )
            }),
        // Stateless requests, and clients that skip the handshake, share one session.
        None => env
            .server
            .default_session()
            .map(|connection| (connection, false))
            .map_err(no_capacity),
    }
}

async fn delete(head: &Request, env: &Env) -> Option<Reply> {
    let Some(id) = head.header("mcp-session-id") else {
        return Some(Reply::text(
            400,
            "Bad Request",
            "Mcp-Session-Id is required",
        ));
    };
    Some(if env.server.close_session(id).await {
        Reply::text(200, "OK", "")
    } else {
        Reply::text(404, "Not Found", "Unknown session")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_must_be_addresses_localhost_or_listed() {
        let allowed = vec!["agent.internal".to_owned()];
        for host in [
            "127.0.0.1:8931",
            "localhost",
            "[::1]:80",
            "10.0.0.5",
            "agent.internal:9",
        ] {
            assert!(host_allowed(host, &allowed), "{host}");
        }
        for host in [
            "evil.example",
            "evil.example:80",
            "[::1",
            "127.0.0.1.evil.example",
        ] {
            assert!(!host_allowed(host, &allowed), "{host}");
        }
    }

    #[test]
    fn tokens_compare_whole() {
        assert!(token_matches("0123456789abcdef", "0123456789abcdef"));
        assert!(!token_matches("0123456789abcdef", "0123456789abcdeg"));
        assert!(!token_matches("0123456789abcdef", "0123456789abcde"));
        assert!(!token_matches("0123456789abcdef", ""));
    }

    #[test]
    fn a_remote_bind_needs_a_long_token() {
        let mut options = HttpOptions {
            bind: SocketAddr::from(([0, 0, 0, 0], 0)),
            ..HttpOptions::default()
        };
        assert!(options.validate().unwrap_err().contains("bearer token"));
        options.token = Some("short".to_owned());
        assert!(options.validate().is_err());
        options.token = Some("0123456789abcdef".to_owned());
        assert!(options.validate().is_ok());
        options.bind = SocketAddr::from(([127, 0, 0, 1], 0));
        options.token = None;
        assert!(options.validate().is_ok());
    }

    #[test]
    fn base64_header_values_decode() {
        assert_eq!(decode_header_value("plain").as_deref(), Some("plain"));
        assert_eq!(
            decode_header_value("=?base64?SGVsbG8=?=").as_deref(),
            Some("Hello")
        );
        assert_eq!(decode_header_value("=?base64?!!?="), None);
    }
}
