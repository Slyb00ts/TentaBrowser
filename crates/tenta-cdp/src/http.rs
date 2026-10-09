//! The HTTP side of the port: `/json/*` endpoints and the WebSocket upgrade.
//!
//! The request head is read here, before tungstenite sees the socket,
//! because the same port also answers plain HTTP and must apply the host,
//! origin and token rules to both.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio::time::timeout;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::{Role, WebSocketConfig};

use crate::backend::{Backend, Target};
use crate::connection::{self, signalled};
use crate::domain::DomainRegistry;
use crate::error::CdpError;

const MAX_HEAD_BYTES: usize = 16 * 1024;
const MAX_HEADERS: usize = 64;
const MAX_DRAIN_BYTES: usize = 1024 * 1024;
const DRAIN_TIMEOUT: Duration = Duration::from_secs(1);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const PROTOCOL_VERSION: &str = "1.3";

/// What a connection task needs from the server.
pub(crate) struct Env {
    pub backend: Arc<Backend>,
    pub registry: Arc<DomainRegistry>,
    pub shutdown: watch::Receiver<bool>,
    pub local_addr: SocketAddr,
}

struct Request {
    method: String,
    path: String,
    query: String,
    headers: Vec<(String, String)>,
    /// Bytes received after the head.
    trailing: usize,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
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
                let (path, query) = target.split_once('?').unwrap_or((target, ""));
                return Head::Request(Request {
                    method: parsed.method.unwrap_or("").to_owned(),
                    path: path.to_owned(),
                    query: query.to_owned(),
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
                    trailing: buffer.len() - head_len,
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

async fn reply(stream: &mut TcpStream, status: u16, reason: &str, content_type: &str, body: &str) {
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let write = async {
        stream.write_all(head.as_bytes()).await?;
        stream.write_all(body.as_bytes()).await?;
        stream.shutdown().await
    };
    let _ = timeout(WRITE_TIMEOUT, write).await;
}

/// Reads and discards what the client still sends. Closing a socket with
/// unread data resets it, which can destroy the response just written.
async fn drain(stream: &mut TcpStream) {
    let mut sink = [0_u8; 4096];
    let rest = async {
        let mut total = 0_usize;
        while total < MAX_DRAIN_BYTES {
            match stream.read(&mut sink).await {
                Ok(0) | Err(_) => break,
                Ok(read) => total += read,
            }
        }
    };
    let _ = timeout(DRAIN_TIMEOUT, rest).await;
}

async fn reply_text(stream: &mut TcpStream, status: u16, reason: &str, body: &str) {
    reply(stream, status, reason, "text/plain; charset=UTF-8", body).await;
}

async fn reply_json(stream: &mut TcpStream, body: &Value) {
    reply(
        stream,
        200,
        "OK",
        "application/json; charset=UTF-8",
        &body.to_string(),
    )
    .await;
}

/// Serves one accepted socket: answers an HTTP request, or upgrades it and
/// runs the protocol until the connection ends.
pub(crate) async fn serve(mut stream: TcpStream, env: Env) {
    let mut shutdown = env.shutdown.clone();
    let upgraded = tokio::select! {
        () = signalled(&mut shutdown) => return,
        upgraded = handshake(&mut stream, &env) => upgraded,
    };
    let Some(root_target) = upgraded else {
        return;
    };
    let limit = Some(env.backend.options().message_limit());
    let config = WebSocketConfig::default()
        .max_message_size(limit)
        .max_frame_size(limit);
    let socket = WebSocketStream::from_raw_socket(stream, Role::Server, Some(config)).await;
    connection::serve(socket, env.backend, env.registry, env.shutdown, root_target).await;
}

/// Reads and answers a request. `Some(root)` when the socket was upgraded:
/// `root` is the page a `/devtools/page/{id}` connection is bound to.
async fn handshake(stream: &mut TcpStream, env: &Env) -> Option<Option<Arc<Target>>> {
    let head = match timeout(env.backend.options().head_timeout, read_head(stream)).await {
        Ok(Head::Request(request)) => request,
        Ok(Head::TooLarge) => {
            reply_text(
                stream,
                431,
                "Request Header Fields Too Large",
                "Request head too large",
            )
            .await;
            drain(stream).await;
            return None;
        }
        Ok(Head::Malformed) => {
            reply_text(stream, 400, "Bad Request", "Malformed HTTP request").await;
            return None;
        }
        Ok(Head::Closed) | Err(_) => return None,
    };
    if let Some((status, reason, message)) = admission_failure(&head, &env.backend) {
        reply_text(stream, status, reason, message).await;
        return None;
    }
    route(stream, &head, env).await
}

/// The host, origin and token rules of `docs/spec/19-security.md` §3.
fn admission_failure(
    request: &Request,
    backend: &Backend,
) -> Option<(u16, &'static str, &'static str)> {
    let options = backend.options();
    if let Some(host) = request.header("host")
        && !host_allowed(host, &options.allowed_hosts)
    {
        return Some((
            403,
            "Forbidden",
            "Host header is specified and is not an IP address or localhost",
        ));
    }
    if let Some(origin) = request.header("origin")
        && !options
            .allowed_origins
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(origin))
    {
        return Some((
            403,
            "Forbidden",
            "Rejected a request that carries an Origin header; add the origin to allowed_origins",
        ));
    }
    if let Some(expected) = &options.token {
        let given = query_token(&request.query).unwrap_or("");
        if !token_matches(expected, given) {
            return Some((401, "Unauthorized", "Missing or invalid token"));
        }
    }
    None
}

fn host_allowed(host: &str, allowed_hosts: &[String]) -> bool {
    let is_port = |port: &str| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit());
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

fn query_token(query: &str) -> Option<&str> {
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix("token="))
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

fn token_suffix(backend: &Backend) -> String {
    backend
        .options()
        .token
        .as_ref()
        .map_or_else(String::new, |token| format!("?token={token}"))
}

/// The authority clients should use to come back: what they connected to.
fn authority(request: &Request, env: &Env) -> String {
    request
        .header("host")
        .map_or_else(|| env.local_addr.to_string(), str::to_owned)
}

fn target_json(target: &Target, authority: &str, suffix: &str) -> Value {
    let url = target.url();
    json!({
        "description": "",
        "id": target.id(),
        "title": url,
        "type": "page",
        "url": url,
        "webSocketDebuggerUrl": format!("ws://{authority}/devtools/page/{}{suffix}", target.id()),
    })
}

async fn route(
    stream: &mut TcpStream,
    request: &Request,
    env: &Env,
) -> Option<Option<Arc<Target>>> {
    let backend = &env.backend;
    let segments: Vec<&str> = request.path.trim_matches('/').split('/').collect();
    let authority = authority(request, env);
    let suffix = token_suffix(backend);
    let is_get = request.method == "GET";

    match segments.as_slice() {
        ["devtools", "browser", id] => {
            if *id != backend.browser_id() {
                reply_text(stream, 404, "Not Found", "No such browser endpoint").await;
                return None;
            }
            upgrade(stream, request, None).await
        }
        ["devtools", "page", id] => {
            let Some(target) = backend.target(id) else {
                reply_text(
                    stream,
                    404,
                    "Not Found",
                    &format!("No such target id: {id}"),
                )
                .await;
                return None;
            };
            upgrade(stream, request, Some(target)).await
        }
        ["json", "new"] => {
            if request.method != "PUT" {
                let message = format!(
                    "Using unsafe HTTP verb {} to invoke /json/new. This action supports only PUT verb.",
                    request.method
                );
                reply_text(stream, 405, "Method Not Allowed", &message).await;
                return None;
            }
            let url: Vec<&str> = request
                .query
                .split('&')
                .filter(|pair| !pair.starts_with("token="))
                .collect();
            let url = url.join("&");
            match backend.create_target(None, Some(url.as_str())).await {
                Ok(target) => reply_json(stream, &target_json(&target, &authority, &suffix)).await,
                Err(error) => reply_text(stream, 400, "Bad Request", &describe(&error)).await,
            }
            None
        }
        _ if !is_get => {
            reply_text(
                stream,
                405,
                "Method Not Allowed",
                "Only GET is supported here",
            )
            .await;
            None
        }
        ["json", "version"] => {
            let options = backend.options();
            let body = json!({
                "Browser": options.product,
                "Protocol-Version": PROTOCOL_VERSION,
                "User-Agent": options.user_agent,
                "webSocketDebuggerUrl": format!(
                    "ws://{authority}/devtools/browser/{}{suffix}",
                    backend.browser_id()
                ),
            });
            reply_json(stream, &body).await;
            None
        }
        [""] | ["json"] | ["json", "list"] => {
            let list: Vec<Value> = backend
                .targets()
                .iter()
                .map(|target| target_json(target, &authority, &suffix))
                .collect();
            reply_json(stream, &Value::Array(list)).await;
            None
        }
        ["json", "protocol"] => {
            reply_json(stream, &env.registry.describe()).await;
            None
        }
        ["json", "close", id] => {
            match backend.close_target(id).await {
                Ok(true) => reply_text(stream, 200, "OK", "Target is closing").await,
                Ok(false) => {
                    reply_text(
                        stream,
                        404,
                        "Not Found",
                        &format!("No such target id: {id}"),
                    )
                    .await;
                }
                Err(error) => {
                    reply_text(stream, 500, "Internal Server Error", &describe(&error)).await;
                }
            }
            None
        }
        ["json", "activate", id] => {
            if backend.target(id).is_some() {
                reply_text(stream, 200, "OK", "Target activated").await;
            } else {
                reply_text(
                    stream,
                    404,
                    "Not Found",
                    &format!("No such target id: {id}"),
                )
                .await;
            }
            None
        }
        _ => {
            reply_text(stream, 404, "Not Found", "No such endpoint").await;
            None
        }
    }
}

fn describe(error: &CdpError) -> String {
    match &error.data {
        Some(data) => format!("{}: {data}", error.message),
        None => error.message.clone(),
    }
}

fn header_has_token(value: Option<&str>, token: &str) -> bool {
    value.is_some_and(|value| {
        value
            .split(',')
            .any(|part| part.trim().eq_ignore_ascii_case(token))
    })
}

async fn upgrade(
    stream: &mut TcpStream,
    request: &Request,
    root: Option<Arc<Target>>,
) -> Option<Option<Arc<Target>>> {
    let wants_upgrade = request.method == "GET"
        && header_has_token(request.header("upgrade"), "websocket")
        && header_has_token(request.header("connection"), "upgrade");
    if !wants_upgrade {
        reply_text(stream, 400, "Bad Request", "This endpoint speaks WebSocket").await;
        return None;
    }
    let Some(key) = request.header("sec-websocket-key").map(str::trim) else {
        reply_text(stream, 400, "Bad Request", "Missing Sec-WebSocket-Key").await;
        return None;
    };
    if request.header("sec-websocket-version").map(str::trim) != Some("13") {
        reply_text(
            stream,
            426,
            "Upgrade Required",
            "Only WebSocket version 13 is supported",
        )
        .await;
        return None;
    }
    // A client may not speak before the 101; bytes already queued behind the
    // head would be lost to the frame reader.
    if request.trailing > 0 {
        reply_text(
            stream,
            400,
            "Bad Request",
            "Data sent before the handshake finished",
        )
        .await;
        return None;
    }
    let accept = derive_accept_key(key.as_bytes());
    let head = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
    );
    match timeout(WRITE_TIMEOUT, stream.write_all(head.as_bytes())).await {
        Ok(Ok(())) => Some(root),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_are_ip_literals_localhost_or_listed() {
        let listed = vec!["cdp.internal".to_owned()];
        for host in [
            "127.0.0.1:9222",
            "[::1]:9222",
            "localhost",
            "LOCALHOST:1",
            "10.0.0.5",
            "cdp.internal:9222",
        ] {
            assert!(host_allowed(host, &listed), "{host}");
        }
        for host in [
            "evil.example",
            "evil.example:9222",
            "localhost.evil.example",
            "[::1]junk",
            "[::1]:",
            "[::1",
            "[::1]:80x",
        ] {
            assert!(!host_allowed(host, &listed), "{host}");
        }
    }

    #[test]
    fn tokens_compare_by_value() {
        assert!(token_matches("abc", "abc"));
        assert!(!token_matches("abc", "abd"));
        assert!(!token_matches("abc", "abcd"));
        assert!(!token_matches("abc", ""));
        assert_eq!(query_token("a=1&token=xyz"), Some("xyz"));
        assert_eq!(query_token("a=1"), None);
    }
}
