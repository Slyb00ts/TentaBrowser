//! Loopback servers the transport tests drive: a raw HTTP/1.1 server that
//! records exactly what arrived on the wire, and a TLS server that captures
//! the ClientHello and the HTTP/2 preface.
//!
//! Everything here answers on `127.0.0.1` and lives for the length of one
//! test; nothing reaches the network.

#![allow(dead_code)]

pub mod proxy;
pub mod tls;

use std::io::Write as _;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// What the server saw.
#[derive(Debug, Default)]
pub struct ServerState {
    requests: Mutex<Vec<Recorded>>,
    connections: AtomicUsize,
}

/// One request as it arrived.
#[derive(Clone, Debug)]
pub struct Recorded {
    /// The request line and headers, verbatim, without the trailing blank line.
    pub head: String,
    pub body: Vec<u8>,
}

impl Recorded {
    /// Header names in the order they arrived, with their original casing.
    pub fn header_names(&self) -> Vec<String> {
        self.head
            .lines()
            .skip(1)
            .filter_map(|line| line.split_once(':'))
            .map(|(name, _)| name.trim().to_owned())
            .collect()
    }

    pub fn header(&self, name: &str) -> Option<String> {
        self.head
            .lines()
            .skip(1)
            .filter_map(|line| line.split_once(':'))
            .find(|(candidate, _)| candidate.trim().eq_ignore_ascii_case(name))
            .map(|(_, value)| value.trim().to_owned())
    }

    pub fn request_line(&self) -> &str {
        self.head.lines().next().unwrap_or_default()
    }

    pub fn path(&self) -> &str {
        self.request_line().split(' ').nth(1).unwrap_or_default()
    }
}

impl ServerState {
    pub fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().clone()
    }

    pub fn hits(&self) -> usize {
        self.requests.lock().len()
    }

    pub fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    pub fn last(&self) -> Recorded {
        self.requests.lock().last().cloned().expect("a request")
    }
}

/// A raw HTTP/1.1 server. Dropping it stops the accept loop.
pub struct TestServer {
    pub addr: SocketAddr,
    pub state: Arc<ServerState>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl TestServer {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let state = Arc::new(ServerState::default());
        let served = Arc::clone(&state);
        let task = tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    return;
                };
                served.connections.fetch_add(1, Ordering::SeqCst);
                let state = Arc::clone(&served);
                tokio::spawn(async move {
                    let _ = serve(socket, state).await;
                });
            }
        });
        Self { addr, state, task }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.addr.port())
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }
}

/// Waits (briefly) for a server-side condition the client cannot observe
/// directly, such as an accepted socket.
pub async fn wait_for(mut condition: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while !condition() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "condition never became true"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// A port nothing listens on, for connection-refused tests.
pub async fn dead_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("local addr").port();
    drop(listener);
    port
}

async fn serve(mut socket: TcpStream, state: Arc<ServerState>) -> std::io::Result<()> {
    let mut buffered = Vec::new();
    loop {
        let head_end = loop {
            if let Some(index) = find(&buffered, b"\r\n\r\n") {
                break index + 4;
            }
            let mut chunk = [0u8; 4096];
            let read = socket.read(&mut chunk).await?;
            if read == 0 {
                return Ok(());
            }
            buffered.extend_from_slice(&chunk[..read]);
        };
        let head = String::from_utf8_lossy(&buffered[..head_end - 4]).into_owned();
        buffered.drain(..head_end);

        let length = header_of(&head, "content-length").and_then(|v| v.parse::<usize>().ok());
        let chunked = header_of(&head, "transfer-encoding")
            .is_some_and(|value| value.eq_ignore_ascii_case("chunked"));
        let body = if let Some(length) = length {
            read_exact_from(&mut socket, &mut buffered, length).await?
        } else if chunked {
            read_chunked(&mut socket, &mut buffered).await?
        } else {
            Vec::new()
        };

        let recorded = Recorded {
            head: head.clone(),
            body,
        };
        let path = recorded.path().to_owned();
        let method = recorded
            .request_line()
            .split(' ')
            .next()
            .unwrap_or_default()
            .to_owned();
        let hit = {
            let mut requests = state.requests.lock();
            requests.push(recorded.clone());
            requests.len()
        };

        let keep_alive = respond(&mut socket, &method, &path, &recorded, hit).await?;
        if !keep_alive {
            return Ok(());
        }
    }
}

/// Writes the response for one route; returns whether the connection stays open.
async fn respond(
    socket: &mut TcpStream,
    method: &str,
    path: &str,
    request: &Recorded,
    hit: usize,
) -> std::io::Result<bool> {
    let (route, query) = path.split_once('?').unwrap_or((path, ""));
    match route {
        "/hello" => {
            write_body(socket, 200, "text/plain", b"hello world", &[]).await?;
        }
        "/echo" => {
            let content_type = request
                .header("content-type")
                .unwrap_or_else(|| "application/octet-stream".to_owned());
            write_body(socket, 200, &content_type, &request.body, &[]).await?;
        }
        "/headers1k" => {
            // A head with roughly one kilobyte of extra headers, for the
            // header-parsing budget.
            let mut extra = Vec::new();
            for index in 0..16 {
                extra.push((format!("X-Filler-{index:02}"), format!("{:0>44}", index)));
            }
            let extra: Vec<(&str, &str)> = extra
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str()))
                .collect();
            write_body(socket, 200, "text/plain", b"hello world", &extra).await?;
        }
        "/method" => {
            write_body(socket, 200, "text/plain", method.as_bytes(), &[]).await?;
        }
        "/chunked" => {
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nTransfer-Encoding: chunked\r\n\r\n\
                      5\r\none, \r\n5\r\ntwo, \r\n5\r\nthree\r\n0\r\n\r\n",
                )
                .await?;
        }
        "/gzip" => {
            let body = gzip(COMPRESSIBLE);
            write_body(
                socket,
                200,
                "text/plain",
                &body,
                &[("Content-Encoding", "gzip")],
            )
            .await?;
        }
        "/deflate" => {
            let body = zlib(COMPRESSIBLE);
            write_body(
                socket,
                200,
                "text/plain",
                &body,
                &[("Content-Encoding", "deflate")],
            )
            .await?;
        }
        "/gzip-large" => {
            let body = gzip(&large_compressible());
            write_body(
                socket,
                200,
                "text/plain",
                &body,
                &[("Content-Encoding", "gzip")],
            )
            .await?;
        }
        "/br-large" => {
            let body = brotli(&large_compressible());
            write_body(
                socket,
                200,
                "text/plain",
                &body,
                &[("Content-Encoding", "br")],
            )
            .await?;
        }
        "/br" => {
            let body = brotli(COMPRESSIBLE);
            write_body(
                socket,
                200,
                "text/plain",
                &body,
                &[("Content-Encoding", "br")],
            )
            .await?;
        }
        "/zstd" => {
            let body = zstd_compress(COMPRESSIBLE);
            write_body(
                socket,
                200,
                "text/plain",
                &body,
                &[("Content-Encoding", "zstd")],
            )
            .await?;
        }
        "/big" => {
            let size: usize = query
                .strip_prefix("n=")
                .and_then(|n| n.parse().ok())
                .unwrap_or(1024);
            let body = vec![b'x'; size];
            write_body(socket, 200, "application/octet-stream", &body, &[]).await?;
        }
        "/unique" => {
            tokio::time::sleep(Duration::from_millis(120)).await;
            let body = format!("unique-{hit}");
            write_body(socket, 200, "text/plain", body.as_bytes(), &[]).await?;
        }
        "/slow" => {
            tokio::time::sleep(Duration::from_millis(400)).await;
            write_body(socket, 200, "text/plain", b"late", &[]).await?;
        }
        "/never" => {
            std::future::pending::<()>().await;
        }
        "/close" => {
            write_body(
                socket,
                200,
                "text/plain",
                b"bye",
                &[("Connection", "close")],
            )
            .await?;
            return Ok(false);
        }
        "/cacheable" => {
            // A counter in the body tells a cache hit from a network hit.
            let body = format!("cacheable-{hit}");
            write_body(
                socket,
                200,
                "text/plain",
                body.as_bytes(),
                &[("Cache-Control", "max-age=60")],
            )
            .await?;
        }
        "/revalidate" => {
            if request.header("if-none-match").as_deref() == Some("\"v1\"") {
                socket
                    .write_all(
                        b"HTTP/1.1 304 Not Modified\r\nETag: \"v1\"\r\n\
                          Cache-Control: max-age=0, must-revalidate\r\n\r\n",
                    )
                    .await?;
            } else {
                write_body(
                    socket,
                    200,
                    "text/plain",
                    b"validated body",
                    &[
                        ("ETag", "\"v1\""),
                        ("Cache-Control", "max-age=0, must-revalidate"),
                    ],
                )
                .await?;
            }
        }
        "/bomb" => {
            // 8 MiB of zeros in about 8 KB on the wire: a client that buffers
            // the decoded body before checking the limit inflates a thousand
            // times what it read. (The fixture is compressed in a debug
            // build, so the bomb is kept just large enough to prove the
            // point.)
            static BOMB: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
            let body = BOMB.get_or_init(|| {
                // Level 1: zeros compress just as far, and the fixture is not
                // here to benchmark deflate.
                let mut encoder =
                    flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
                encoder
                    .write_all(&vec![0u8; 8 * 1024 * 1024])
                    .expect("gzip");
                encoder.finish().expect("gzip finish")
            });
            write_body(
                socket,
                200,
                "application/octet-stream",
                body,
                &[("Content-Encoding", "gzip")],
            )
            .await?;
        }
        "/vary-star" => {
            // `Vary: *` means no two requests are equivalent, so this answer
            // may not be handed to a coalesced follower.
            tokio::time::sleep(Duration::from_millis(120)).await;
            let body = format!("vary-{hit}");
            write_body(socket, 200, "text/plain", body.as_bytes(), &[("Vary", "*")]).await?;
        }
        "/stale-redirect" => {
            // Stored with an ETag and immediately stale; the revalidation is
            // answered with a redirect, whose target must not inherit the
            // validators.
            if request.header("if-none-match").is_some() {
                redirect(socket, 302, "/echo-conditional").await?;
            } else {
                write_body(
                    socket,
                    200,
                    "text/plain",
                    b"first",
                    &[
                        ("ETag", "\"v1\""),
                        ("Cache-Control", "max-age=0, must-revalidate"),
                    ],
                )
                .await?;
            }
        }
        "/echo-conditional" => {
            // Reports whichever validators arrived, so a leak is visible.
            let leaked = ["if-none-match", "if-modified-since", "if-match", "if-range"]
                .iter()
                .filter_map(|name| request.header(name).map(|value| format!("{name}: {value}")))
                .collect::<Vec<_>>()
                .join("; ");
            write_body(socket, 200, "text/plain", leaked.as_bytes(), &[]).await?;
        }
        "/swr" => {
            // Stale immediately, servable for a minute while it refreshes.
            let body = format!("swr-{hit}");
            write_body(
                socket,
                200,
                "text/plain",
                body.as_bytes(),
                &[("Cache-Control", "max-age=0, stale-while-revalidate=60")],
            )
            .await?;
        }
        "/no-store" => {
            let body = format!("volatile-{hit}");
            write_body(
                socket,
                200,
                "text/plain",
                body.as_bytes(),
                &[("Cache-Control", "no-store")],
            )
            .await?;
        }
        "/cookie-redirect" => {
            let head = "HTTP/1.1 302 Found\r\nLocation: /needs-cookie\r\n\
                        Set-Cookie: hop=1; Path=/\r\nContent-Length: 0\r\n\r\n";
            socket.write_all(head.as_bytes()).await?;
        }
        "/needs-cookie" => {
            let body = request
                .header("cookie")
                .unwrap_or_else(|| "none".to_owned());
            write_body(socket, 200, "text/plain", body.as_bytes(), &[]).await?;
        }
        "/set-cookie" => {
            write_body(
                socket,
                200,
                "text/plain",
                b"ok",
                &[("Set-Cookie", "sid=42; Path=/")],
            )
            .await?;
        }
        "/set-strict-cookie" => {
            write_body(
                socket,
                200,
                "text/plain",
                b"ok",
                &[("Set-Cookie", "strict_cookie=value; Path=/; SameSite=Strict")],
            )
            .await?;
        }
        "/cross-site-redirect" => {
            // Redirect to /strict-cookie-check where the strict cookie should be verified.
            // Chrome ignores the redirect chain for SameSite, so the Strict cookie set on
            // the original site will be sent even though there's a redirect.
            redirect(socket, 302, "/strict-cookie-check").await?;
        }
        "/strict-cookie-check" => {
            // Check if the strict cookie was sent. It should be present if the cookie
            // handling correctly ignores the redirect chain for SameSite.
            if request
                .header("Cookie")
                .is_some_and(|c| c.contains("strict_cookie=value"))
            {
                write_body(socket, 200, "text/plain", b"strict_cookie_present", &[]).await?;
            } else {
                write_body(socket, 200, "text/plain", b"strict_cookie_missing", &[]).await?;
            }
        }
        _ if route.starts_with("/redirect/") => {
            let left: u32 = route
                .trim_start_matches("/redirect/")
                .parse()
                .unwrap_or_default();
            let target = if left <= 1 {
                "/hello".to_owned()
            } else {
                format!("/redirect/{}", left - 1)
            };
            redirect(socket, 302, &target).await?;
        }
        "/redirect-to" => {
            // `?url=<absolute URL>`: a hop to another host.
            let target = query.strip_prefix("url=").unwrap_or("/hello");
            redirect(socket, 302, target).await?;
        }
        "/temporary" => {
            redirect(socket, 307, "/echo").await?;
        }
        "/see-other" => {
            redirect(socket, 303, "/echo").await?;
        }
        "/permanent" => {
            redirect(socket, 308, "/echo").await?;
        }
        "/loop" => {
            redirect(socket, 302, "/loop").await?;
        }
        "/elsewhere" => {
            let target = query.strip_prefix("to=").unwrap_or("/hello");
            redirect(socket, 302, target).await?;
        }
        "/ping" => {
            // Simple ping endpoint for latency testing
            write_body(socket, 200, "text/plain", b"pong", &[]).await?;
        }
        "/perm" => {
            // A cacheable 301 redirect for testing cache behavior.
            let head = "HTTP/1.1 301 Moved Permanently\r\nLocation: /final\r\n\
                        Cache-Control: max-age=60\r\nContent-Length: 0\r\n\r\n";
            socket.write_all(head.as_bytes()).await?;
        }
        "/final" => {
            // Target of /perm redirect.
            write_body(socket, 200, "text/plain", b"final", &[]).await?;
        }
        "/slow-plain" => {
            // Plain response with delay, 64 KiB body for coalescing test.
            tokio::time::sleep(Duration::from_millis(100)).await;
            let body = vec![b'x'; 65536];
            write_body(socket, 200, "text/plain", &body, &[]).await?;
        }
        "/slow-gzip" => {
            // Gzipped response with Content-Length and delay for coalescing test.
            // This tests the MAJOR-2 fix: coalescing with gzip + Content-Length.
            tokio::time::sleep(Duration::from_millis(100)).await;
            let body = gzip(&vec![b'x'; 65536]);
            // Write with explicit Content-Length so completeness check runs.
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\
                 Content-Encoding: gzip\r\n\r\n",
                body.len()
            );
            socket.write_all(head.as_bytes()).await?;
            socket.write_all(&body).await?;
        }
        "/slow-chunked-gzip" => {
            // Gzipped response chunked (no Content-Length) with delay.
            tokio::time::sleep(Duration::from_millis(100)).await;
            let body = gzip(&vec![b'x'; 65536]);
            let head = "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\
                        Content-Encoding: gzip\r\nTransfer-Encoding: chunked\r\n\r\n";
            socket.write_all(head.as_bytes()).await?;
            // Write chunked encoding.
            socket
                .write_all(format!("{:x}\r\n", body.len()).as_bytes())
                .await?;
            socket.write_all(&body).await?;
            socket.write_all(b"\r\n0\r\n\r\n").await?;
        }
        "/bomb-16mb" => {
            // 16 MiB of zeros in brotli compression for decompression bomb testing.
            // This tests that the decoder properly limits output and doesn't try to
            // materialize the entire 16 MiB decompressed payload at once.
            static BOMB_16MB: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
            let body = BOMB_16MB.get_or_init(|| {
                let payload = vec![0u8; 16 * 1024 * 1024];
                brotli(&payload)
            });
            write_body(
                socket,
                200,
                "application/octet-stream",
                body,
                &[("Content-Encoding", "br")],
            )
            .await?;
        }
        _ => {
            write_body(socket, 404, "text/plain", b"not found", &[]).await?;
        }
    }
    Ok(true)
}

pub const COMPRESSIBLE: &[u8] =
    b"compressible payload compressible payload compressible payload compressible payload";

/// `"compressible payload "` repeated past 512 KiB, so a decoded body spans
/// several of the transport's 256 KiB output pieces.
pub fn large_compressible() -> Vec<u8> {
    let unit = b"compressible payload ";
    let mut body = Vec::with_capacity(512 * 1024 + unit.len());
    while body.len() <= 512 * 1024 {
        body.extend_from_slice(unit);
    }
    body
}

async fn redirect(socket: &mut TcpStream, status: u16, location: &str) -> std::io::Result<()> {
    let head =
        format!("HTTP/1.1 {status} Redirect\r\nLocation: {location}\r\nContent-Length: 0\r\n\r\n");
    socket.write_all(head.as_bytes()).await
}

async fn write_body(
    socket: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
    extra: &[(&str, &str)],
) -> std::io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {status} OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (name, value) in extra {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    socket.write_all(head.as_bytes()).await?;
    socket.write_all(body).await
}

async fn read_exact_from(
    socket: &mut TcpStream,
    buffered: &mut Vec<u8>,
    length: usize,
) -> std::io::Result<Vec<u8>> {
    while buffered.len() < length {
        let mut chunk = [0u8; 4096];
        let read = socket.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        buffered.extend_from_slice(&chunk[..read]);
    }
    let taken = buffered.drain(..length.min(buffered.len())).collect();
    Ok(taken)
}

async fn read_chunked(socket: &mut TcpStream, buffered: &mut Vec<u8>) -> std::io::Result<Vec<u8>> {
    let mut body = Vec::new();
    loop {
        let line_end = loop {
            if let Some(index) = find(buffered, b"\r\n") {
                break index;
            }
            let mut chunk = [0u8; 4096];
            let read = socket.read(&mut chunk).await?;
            if read == 0 {
                return Ok(body);
            }
            buffered.extend_from_slice(&chunk[..read]);
        };
        let size_line: String = String::from_utf8_lossy(&buffered[..line_end]).into_owned();
        buffered.drain(..line_end + 2);
        let size = usize::from_str_radix(size_line.trim().split(';').next().unwrap_or("0"), 16)
            .unwrap_or_default();
        let chunk = read_exact_from(socket, buffered, size + 2).await?;
        if size == 0 {
            return Ok(body);
        }
        body.extend_from_slice(&chunk[..size]);
    }
}

fn header_of(head: &str, name: &str) -> Option<String> {
    head.lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .find(|(candidate, _)| candidate.trim().eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim().to_owned())
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

pub fn gzip(data: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(data).expect("gzip");
    encoder.finish().expect("gzip finish")
}

pub fn zlib(data: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(data).expect("deflate");
    encoder.finish().expect("deflate finish")
}

pub fn brotli(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = brotli::CompressorWriter::new(&mut out, 4096, 5, 22);
        encoder.write_all(data).expect("brotli");
    }
    out
}

pub fn zstd_compress(data: &[u8]) -> Vec<u8> {
    zstd::stream::encode_all(data, 3).expect("zstd")
}
