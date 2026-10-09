//! A loopback TLS server with a throw-away certificate.
//!
//! It either captures the raw ClientHello (and, after the handshake, the
//! decrypted HTTP/2 client preface) or serves real HTTP/2, which is what the
//! multiplexing test needs.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use boring::ssl::{AlpnError, SslAcceptor, SslMethod, select_next_proto};
use bytes::Bytes;
use parking_lot::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Notify;

/// What the server should do once a client connects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TlsMode {
    /// Record the ClientHello and close; nothing is negotiated.
    HelloOnly,
    /// Record the ClientHello, finish the handshake and record the decrypted
    /// HTTP/2 preface up to the first HEADERS frame.
    CaptureH2Preface,
    /// Record the ClientHello and serve for real, HTTP/2 or HTTP/1.1
    /// depending on what the client negotiated.
    ServeH2,
    /// Serve one HTTP/2 request then close the TCP connection without GOAWAY.
    /// Used for testing BLOCKING-1: handling of TCP close without GOAWAY.
    ServeH2DropAfterOne,
    /// Refuse the first POST stream on a healthy reused HTTP/2 connection.
    ServeH2RefusePostOnce,
    /// Refuse two POST streams to prove automatic replay is bounded.
    ServeH2RefusePostTwice,
    /// Reset a POST with CANCEL, which does not guarantee safe replay.
    ServeH2CancelPost,
    /// Read a POST body, then close without sending response headers.
    ServeH2CloseAfterPostBody,
    /// Serve one HTTP/2 request then go silent: hold the TCP connection open
    /// but answer no further stream and no PING. Models a peer that a NAT or
    /// firewall dropped mid-connection (BLOCKING-2, round 5): the socket looks
    /// alive, so only eviction on a head timeout (or the keep-alive ping
    /// timeout) recovers the origin.
    ServeH2HangAfterOne,
    /// Finish the handshake and then do nothing but read. The client's HTTP/2
    /// preface needs no answer, so this keeps the server side of a connection
    /// as cheap as possible for the memory budget.
    AcceptAndIdle,
    /// Offer only `http/1.1` in ALPN and record every request head verbatim,
    /// which is what the byte-exact HTTP/1.1 conformance test needs.
    ServeHttp1,
}

impl TlsMode {
    /// The ALPN protocol list the server offers.
    fn alpn_protocols(self) -> &'static [u8] {
        match self {
            TlsMode::ServeHttp1 => b"\x08http/1.1",
            _ => b"\x02h2\x08http/1.1",
        }
    }
}

/// The bytes a client sent before anything was answered.
#[derive(Debug, Default)]
pub struct TlsCapture {
    client_hello: Mutex<Option<Vec<u8>>>,
    /// Every ClientHello, in accept order, for comparing a resumption
    /// handshake with the full one that preceded it.
    client_hellos: Mutex<Vec<Vec<u8>>>,
    h2_preface: Mutex<Option<Vec<u8>>>,
    alpn: Mutex<Option<Vec<u8>>>,
    /// When each HTTP/2 request head arrived, for the warm-request budget.
    arrivals: Mutex<Vec<std::time::Instant>>,
    /// Whether each accepted handshake resumed a session, in accept order.
    resumptions: Mutex<Vec<bool>>,
    /// Every HTTP/1.1 request head, verbatim, including the blank line.
    http1_heads: Mutex<Vec<Vec<u8>>>,
    /// Completed close events for the drop-after-one fixture.
    h2_closed: AtomicUsize,
    h2_closed_notify: Notify,
    h2_drop_request: Notify,
    /// Paths accepted in the deterministic reset/retry fixture.
    replay_paths: Mutex<Vec<String>>,
    /// Bodies processed after a stream was accepted by the retry fixture.
    replay_bodies: Mutex<Vec<(String, Vec<u8>)>>,
}

impl TlsCapture {
    pub fn client_hello(&self) -> Option<Vec<u8>> {
        self.client_hello.lock().clone()
    }

    /// Every ClientHello the server saw, in accept order.
    pub fn client_hellos(&self) -> Vec<Vec<u8>> {
        self.client_hellos.lock().clone()
    }

    pub fn h2_preface(&self) -> Option<Vec<u8>> {
        self.h2_preface.lock().clone()
    }

    pub fn alpn(&self) -> Option<Vec<u8>> {
        self.alpn.lock().clone()
    }

    /// The instant each HTTP/2 request head was decoded, in arrival order.
    pub fn arrivals(&self) -> Vec<std::time::Instant> {
        self.arrivals.lock().clone()
    }

    /// One entry per accepted handshake: whether it resumed a session.
    pub fn resumptions(&self) -> Vec<bool> {
        self.resumptions.lock().clone()
    }

    /// Every HTTP/1.1 request head exactly as it arrived, blank line included.
    pub fn http1_heads(&self) -> Vec<Vec<u8>> {
        self.http1_heads.lock().clone()
    }

    pub async fn wait_h2_closed(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let notified = self.h2_closed_notify.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if self.h2_closed.load(Ordering::Acquire) >= count {
                    break;
                }
                notified.await;
            }
        })
        .await
        .expect("the HTTP/2 fixture did not close its connection");
    }

    pub fn drop_h2_after_response(&self) {
        self.h2_drop_request.notify_one();
    }

    pub fn replay_paths(&self) -> Vec<String> {
        self.replay_paths.lock().clone()
    }

    pub fn replay_bodies(&self) -> Vec<(String, Vec<u8>)> {
        self.replay_bodies.lock().clone()
    }
}

pub struct TlsServer {
    pub addr: SocketAddr,
    pub capture: Arc<TlsCapture>,
    /// The server certificate in PEM form, for `TlsOptions::extra_root_cas`.
    pub cert_pem: Vec<u8>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for TlsServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl TlsServer {
    pub async fn start(mode: TlsMode) -> Self {
        // A distinct subject per server: several of these certificates end
        // up in one trust store, and BoringSSL looks candidates up by
        // subject name.
        static SERIAL: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let serial = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut params =
            rcgen::CertificateParams::new(vec!["localhost".to_owned(), "127.0.0.1".to_owned()])
                .expect("certificate parameters");
        params.distinguished_name.push(
            rcgen::DnType::CommonName,
            format!("tenta test server {serial}"),
        );
        let signing_key = rcgen::KeyPair::generate().expect("key pair");
        let cert = params
            .self_signed(&signing_key)
            .expect("self-signed certificate");
        let certified = rcgen::CertifiedKey { cert, signing_key };
        let cert_pem = certified.cert.pem().into_bytes();
        let cert = boring::x509::X509::from_pem(&cert_pem).expect("certificate");
        let key = boring::pkey::PKey::private_key_from_pem(
            certified.signing_key.serialize_pem().as_bytes(),
        )
        .expect("private key");

        let mut builder =
            SslAcceptor::mozilla_intermediate_v5(SslMethod::tls()).expect("acceptor builder");
        builder.set_certificate(&cert).expect("set certificate");
        builder.set_private_key(&key).expect("set private key");
        let offered = mode.alpn_protocols();
        builder.set_alpn_select_callback(move |_ssl, client| {
            select_next_proto(offered, client).ok_or(AlpnError::NOACK)
        });
        let acceptor = Arc::new(builder.build());

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let capture = Arc::new(TlsCapture::default());
        let served = Arc::clone(&capture);
        let task = tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    return;
                };
                let acceptor = Arc::clone(&acceptor);
                let capture = Arc::clone(&served);
                tokio::spawn(async move {
                    let hello = peek_client_hello(&socket).await;
                    if let Some(hello) = hello {
                        capture.client_hellos.lock().push(hello.clone());
                        let mut slot = capture.client_hello.lock();
                        if slot.is_none() {
                            *slot = Some(hello);
                        }
                    }
                    if mode == TlsMode::HelloOnly {
                        return;
                    }
                    let Ok(mut tls) = tokio_boring::accept(&acceptor, socket).await else {
                        return;
                    };
                    {
                        let negotiated = tls.ssl().selected_alpn_protocol().map(<[u8]>::to_vec);
                        let mut slot = capture.alpn.lock();
                        if slot.is_none() {
                            *slot = negotiated;
                        }
                    }
                    capture.resumptions.lock().push(tls.ssl().session_reused());
                    match mode {
                        TlsMode::CaptureH2Preface => {
                            if let Some(preface) = read_h2_preface(&mut tls).await {
                                let mut slot = capture.h2_preface.lock();
                                if slot.is_none() {
                                    *slot = Some(preface);
                                }
                            }
                            // Let the client see a clean shutdown rather than a reset.
                            let _ = tls.shutdown().await;
                        }
                        TlsMode::ServeH2 => {
                            // The client decides: this server takes either,
                            // which is what makes an ALPN offer meaningful.
                            if tls.ssl().selected_alpn_protocol() == Some(b"http/1.1") {
                                serve_http1(tls, Arc::clone(&capture)).await;
                            } else {
                                serve_h2(tls, Arc::clone(&capture)).await;
                            }
                        }
                        TlsMode::ServeH2DropAfterOne => {
                            // Serve one HTTP/2 request then drop the connection without GOAWAY.
                            // This tests BLOCKING-1: TCP close without GOAWAY should be handled.
                            if tls.ssl().selected_alpn_protocol() == Some(b"http/1.1") {
                                serve_http1(tls, Arc::clone(&capture)).await;
                            } else {
                                serve_h2_drop_after_one(tls, Arc::clone(&capture)).await;
                            }
                        }
                        TlsMode::ServeH2RefusePostOnce
                        | TlsMode::ServeH2RefusePostTwice
                        | TlsMode::ServeH2CancelPost => {
                            serve_h2_reset_post(tls, Arc::clone(&capture), mode).await;
                        }
                        TlsMode::ServeH2CloseAfterPostBody => {
                            serve_h2_close_after_post_body(tls, Arc::clone(&capture)).await;
                        }
                        TlsMode::ServeH2HangAfterOne => {
                            serve_h2_hang_after_one(tls, Arc::clone(&capture)).await;
                        }
                        TlsMode::AcceptAndIdle => {
                            let mut sink = [0u8; 1024];
                            while let Ok(read) = tls.read(&mut sink).await {
                                if read == 0 {
                                    break;
                                }
                            }
                        }
                        TlsMode::ServeHttp1 => serve_http1(tls, Arc::clone(&capture)).await,
                        TlsMode::HelloOnly => {}
                    }
                });
            }
        });

        Self {
            addr,
            capture,
            cert_pem,
            task,
        }
    }

    /// An `https://localhost:<port>` URL; the client must be told that
    /// `localhost` is `127.0.0.1`.
    pub fn url(&self, path: &str) -> String {
        format!("https://localhost:{}{path}", self.addr.port())
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }
}

/// Answers every stream with `200` and the request path as the body.
async fn serve_h2(tls: tokio_boring::SslStream<tokio::net::TcpStream>, capture: Arc<TlsCapture>) {
    let Ok(mut connection) = tenta_h2::server::handshake(tls).await else {
        return;
    };
    while let Some(Ok((request, mut responder))) = connection.accept().await {
        capture.arrivals.lock().push(std::time::Instant::now());
        tokio::spawn(async move {
            let mut body = Vec::new();
            let mut incoming = request.into_body();
            while let Some(Ok(chunk)) = incoming.data().await {
                let _ = incoming.flow_control().release_capacity(chunk.len());
                body.extend_from_slice(&chunk);
            }
            let payload = if body.is_empty() {
                Bytes::from_static(b"hello from h2")
            } else {
                Bytes::from(body)
            };
            let response = http::Response::builder()
                .status(200)
                .header("content-type", "text/plain")
                .header("content-length", payload.len().to_string())
                .body(())
                .expect("response");
            if let Ok(mut stream) = responder.send_response(response, false) {
                let _ = stream.send_data(payload, true);
            }
        });
    }
}

/// Serves one HTTP/2 request then drops the connection without GOAWAY.
/// The test releases the socket after consuming its response, so no timing
/// window can swallow a later request head.
async fn serve_h2_drop_after_one(
    tls: tokio_boring::SslStream<tokio::net::TcpStream>,
    capture: Arc<TlsCapture>,
) {
    let Ok(mut connection) = tenta_h2::server::handshake(tls).await else {
        return;
    };
    if let Some(Ok((request, mut responder))) = connection.accept().await {
        capture.arrivals.lock().push(std::time::Instant::now());
        tokio::spawn(async move {
            let mut body = Vec::new();
            let mut incoming = request.into_body();
            while let Some(Ok(chunk)) = incoming.data().await {
                let _ = incoming.flow_control().release_capacity(chunk.len());
                body.extend_from_slice(&chunk);
            }
            let payload = if body.is_empty() {
                Bytes::from_static(b"hello from h2")
            } else {
                Bytes::from(body)
            };
            let response = http::Response::builder()
                .status(200)
                .header("content-type", "text/plain")
                .header("content-length", payload.len().to_string())
                .body(())
                .expect("response");
            if let Ok(mut stream) = responder.send_response(response, false) {
                let _ = stream.send_data(payload, true);
            }
        });
        // The connection must keep polling while the handler reads the body
        // and sends the response. poll_closed drives frames without accepting
        // a second stream whose responder the fixture would then discard.
        tokio::select! {
            _ = capture.h2_drop_request.notified() => {}
            _ = std::future::poll_fn(|cx| connection.poll_closed(cx)) => {}
        }
    }
    drop(connection);
    capture.h2_closed.fetch_add(1, Ordering::Release);
    capture.h2_closed_notify.notify_waiters();
}

/// Resets a reused POST stream before processing, then serves the retry and
/// redirect on the same healthy connection. This makes replay observable
/// without any timer-based race against a closed socket.
async fn serve_h2_reset_post(
    tls: tokio_boring::SslStream<tokio::net::TcpStream>,
    capture: Arc<TlsCapture>,
    mode: TlsMode,
) {
    let (reason, mut resets_left) = match mode {
        TlsMode::ServeH2RefusePostOnce => (tenta_h2::Reason::REFUSED_STREAM, 1),
        TlsMode::ServeH2RefusePostTwice => (tenta_h2::Reason::REFUSED_STREAM, 2),
        TlsMode::ServeH2CancelPost => (tenta_h2::Reason::CANCEL, 1),
        _ => unreachable!("reset fixture mode"),
    };
    let Ok(mut connection) = tenta_h2::server::handshake(tls).await else {
        return;
    };
    while let Some(Ok((request, mut responder))) = connection.accept().await {
        capture.arrivals.lock().push(std::time::Instant::now());
        let path = request.uri().path().to_owned();
        capture.replay_paths.lock().push(path.clone());
        if path == "/temporary" && resets_left > 0 {
            resets_left -= 1;
            // No request body is read or application action performed.
            responder.send_reset(reason);
            continue;
        }
        let capture = Arc::clone(&capture);
        tokio::spawn(async move {
            let mut body = Vec::new();
            let mut incoming = request.into_body();
            while let Some(Ok(chunk)) = incoming.data().await {
                let _ = incoming.flow_control().release_capacity(chunk.len());
                body.extend_from_slice(&chunk);
            }
            capture
                .replay_bodies
                .lock()
                .push((path.clone(), body.clone()));
            let payload = if body.is_empty() {
                Bytes::from_static(b"hello from h2")
            } else {
                Bytes::from(body)
            };
            let response = if path == "/temporary" {
                http::Response::builder()
                    .status(307)
                    .header("location", "/echo")
                    .header("content-length", "0")
                    .body(())
                    .expect("response")
            } else {
                http::Response::builder()
                    .status(200)
                    .header("content-type", "text/plain")
                    .header("content-length", payload.len().to_string())
                    .body(())
                    .expect("response")
            };
            let end_of_stream = path == "/temporary";
            if let Ok(mut stream) = responder.send_response(response, end_of_stream)
                && !end_of_stream
            {
                let _ = stream.send_data(payload, true);
            }
        });
    }
}

/// Proves that a pooled transport error after a POST body was received does
/// not grant permission to replay that POST. The response head is never sent.
async fn serve_h2_close_after_post_body(
    tls: tokio_boring::SslStream<tokio::net::TcpStream>,
    capture: Arc<TlsCapture>,
) {
    let Ok(mut connection) = tenta_h2::server::handshake(tls).await else {
        return;
    };
    while let Some(Ok((request, mut responder))) = connection.accept().await {
        let path = request.uri().path().to_owned();
        capture.replay_paths.lock().push(path.clone());
        if path == "/temporary" {
            let mut incoming = request.into_body();
            let observed = Arc::clone(&capture);
            let (done, received) = tokio::sync::oneshot::channel();
            tokio::spawn(async move {
                let mut body = Vec::new();
                while let Some(Ok(chunk)) = incoming.data().await {
                    let _ = incoming.flow_control().release_capacity(chunk.len());
                    body.extend_from_slice(&chunk);
                }
                observed.replay_bodies.lock().push((path, body));
                let _ = done.send(());
            });
            // Keep the responder alive while the connection driver receives
            // the body. Dropping it earlier would send RST_STREAM instead of
            // exercising an ambiguous transport close.
            tokio::select! {
                _ = received => {}
                _ = std::future::poll_fn(|cx| connection.poll_closed(cx)) => {}
            }
            drop(connection);
            drop(responder);
            return;
        }
        let observed = Arc::clone(&capture);
        tokio::spawn(async move {
            let mut incoming = request.into_body();
            let mut body = Vec::new();
            while let Some(Ok(chunk)) = incoming.data().await {
                let _ = incoming.flow_control().release_capacity(chunk.len());
                body.extend_from_slice(&chunk);
            }
            observed.replay_bodies.lock().push((path, body));
            let payload = Bytes::from_static(b"hello from h2");
            let response = http::Response::builder()
                .status(200)
                .header("content-type", "text/plain")
                .header("content-length", payload.len().to_string())
                .body(())
                .expect("response");
            if let Ok(mut stream) = responder.send_response(response, false) {
                let _ = stream.send_data(payload, true);
            }
        });
    }
}

/// Serves one HTTP/2 request, then holds the connection open forever without
/// answering another stream or a PING — a peer that went silent (BLOCKING-2).
async fn serve_h2_hang_after_one(
    tls: tokio_boring::SslStream<tokio::net::TcpStream>,
    capture: Arc<TlsCapture>,
) {
    let Ok(mut connection) = tenta_h2::server::handshake(tls).await else {
        return;
    };
    if let Some(Ok((request, mut responder))) = connection.accept().await {
        capture.arrivals.lock().push(std::time::Instant::now());
        let mut incoming = request.into_body();
        while let Some(Ok(chunk)) = incoming.data().await {
            let _ = incoming.flow_control().release_capacity(chunk.len());
        }
        let payload = Bytes::from_static(b"hello from h2");
        let response = http::Response::builder()
            .status(200)
            .header("content-type", "text/plain")
            .header("content-length", payload.len().to_string())
            .body(())
            .expect("response");
        if let Ok(mut stream) = responder.send_response(response, false) {
            let _ = stream.send_data(payload, true);
        }
        // Drive the connection just long enough to flush the first response,
        // then go silent: hold the socket open and never poll the connection
        // again, so no later stream and no PING is ever answered.
        let _ = tokio::time::timeout(Duration::from_millis(200), connection.accept()).await;
        std::future::pending::<()>().await;
    }
}

/// Records each request head verbatim and answers `200 OK`.
async fn serve_http1(
    mut tls: tokio_boring::SslStream<tokio::net::TcpStream>,
    capture: Arc<TlsCapture>,
) {
    let mut buffered: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let head_end = loop {
            if let Some(index) = buffered.windows(4).position(|window| window == b"\r\n\r\n") {
                break index + 4;
            }
            match tls.read(&mut chunk).await {
                Ok(0) | Err(_) => return,
                Ok(read) => buffered.extend_from_slice(&chunk[..read]),
            }
        };
        let head: Vec<u8> = buffered.drain(..head_end).collect();
        let length = content_length(&head);
        while buffered.len() < length {
            match tls.read(&mut chunk).await {
                Ok(0) | Err(_) => return,
                Ok(read) => buffered.extend_from_slice(&chunk[..read]),
            }
        }
        buffered.drain(..length);
        capture.http1_heads.lock().push(head);
        if tls
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\r\nok",
            )
            .await
            .is_err()
        {
            return;
        }
    }
}

fn content_length(head: &[u8]) -> usize {
    String::from_utf8_lossy(head)
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse().ok())
        .unwrap_or(0)
}

/// Reads the first TLS record without consuming it.
async fn peek_client_hello(socket: &tokio::net::TcpStream) -> Option<Vec<u8>> {
    let mut buffer = vec![0u8; 16 * 1024];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if tokio::time::Instant::now() > deadline {
            return None;
        }
        socket.readable().await.ok()?;
        let read = match socket.peek(&mut buffer).await {
            Ok(read) => read,
            Err(_) => return None,
        };
        if read >= 5 {
            if buffer[0] != 0x16 {
                return None;
            }
            let length = usize::from(u16::from_be_bytes([buffer[3], buffer[4]]));
            if read >= 5 + length {
                return Some(buffer[..5 + length].to_vec());
            }
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

/// Reads decrypted client bytes until they hold a complete HEADERS frame.
async fn read_h2_preface(
    tls: &mut tokio_boring::SslStream<tokio::net::TcpStream>,
) -> Option<Vec<u8>> {
    const MAGIC: usize = 24;
    let mut collected = Vec::new();
    let mut chunk = [0u8; 4096];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let read = tokio::time::timeout_at(deadline, tls.read(&mut chunk))
            .await
            .ok()?
            .ok()?;
        if read == 0 {
            return (!collected.is_empty()).then_some(collected);
        }
        collected.extend_from_slice(&chunk[..read]);
        if has_headers_frame(&collected, MAGIC) {
            return Some(collected);
        }
    }
}

/// Walks the frame headers after the connection magic looking for HEADERS (1).
fn has_headers_frame(bytes: &[u8], skip: usize) -> bool {
    let mut offset = skip;
    while offset + 9 <= bytes.len() {
        let length = usize::from(bytes[offset]) << 16
            | usize::from(bytes[offset + 1]) << 8
            | usize::from(bytes[offset + 2]);
        let kind = bytes[offset + 3];
        let end = offset + 9 + length;
        if end > bytes.len() {
            return false;
        }
        if kind == 0x1 {
            return true;
        }
        offset = end;
    }
    false
}
