//! Captures a real browser's TLS ClientHello, HTTP/2 client preface and
//! HTTP/1.1 request off a loopback socket, and prints their JA4, JA4_r, JA3 and
//! Akamai fingerprints.
//!
//! This is how `profiles/*.toml` is produced. Chrome ships every four weeks, so
//! the profile is re-measured on that cadence (`docs/spec/21-project-operations.md`
//! §3); `docs/dev/fingerprint.md` is the operator's guide.
//!
//! The tool binds three loopback listeners, launches the browser at each of them
//! in turn, and records what arrives:
//!
//! * the ClientHello is read with `MSG_PEEK` before BoringSSL touches the socket,
//!   so the stored bytes are exactly what the browser sent;
//! * the handshake is then completed with a throw-away self-signed certificate
//!   and ALPN `h2`, which is the only way to see the HTTP/2 preface;
//! * a second listener forces ALPN `http/1.1`, which is where the HTTP/1.1
//!   header order and casing of a secure origin come from; its page sets a
//!   cookie and pulls in a sub-resource, so the follow-up request shows where
//!   Chrome places `Referer` and `Cookie`;
//! * a third, plain-`http://` listener records what an insecure origin sees:
//!   no client hints, no `Sec-Fetch-*`, no brotli.
//!
//! Run it several times: Chrome permutes its TLS extension order per connection,
//! so JA3 changes between runs while JA4 and the Akamai fingerprint must not.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use boring::ssl::{AlpnError, SslAcceptor, SslMethod, select_next_proto};
use tenta_testutil::fingerprint::{
    HpackDecoder, akamai_h2, ja3, ja4, parse_client_hello, parse_h2_preface,
};

/// Default browser on macOS; override with `--chrome`.
const DEFAULT_CHROME: &str = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";

/// Flags shared by both capture modes (`bench/scenarios.toml`).
const CHROME_FLAGS: &[&str] = &[
    "--disable-gpu",
    "--no-first-run",
    "--no-default-browser-check",
    "--incognito",
    "--ignore-certificate-errors",
];

/// Flags that make Chrome 144 headless load one page and exit cleanly.
///
/// The headless build reports `HeadlessChrome/<version>` in `User-Agent`, so a
/// profile's header block is measured with `--headful`.
const HEADLESS_FLAGS: &[&str] = &["--headless=new", "--dump-dom"];

/// The historical flag that turned Chrome's extension permutation off. Chrome
/// 144 no longer carries the feature name at all, so `--canonical` is a way to
/// re-check that on each refresh, not a way to obtain a canonical order.
const NO_PERMUTE_FLAG: &str = "--disable-features=PermuteTLSExtensions";

/// The name the browser is pointed at, so that the ClientHello carries SNI as it
/// would on a real navigation. Chrome resolves it to loopback through
/// `--host-resolver-rules`; an IP literal would suppress SNI and change JA4's
/// `a` component from `d` to `i`.
const CAPTURE_HOST: &str = "fp-capture.test";

/// A different origin on the same registrable domain: `Sec-Fetch-Site: same-site`.
const SAME_SITE_HOST: &str = "sub.fp-capture.test";

/// A different registrable domain: `Sec-Fetch-Site: cross-site`.
const CROSS_SITE_HOST: &str = "fp-other.test";

/// Every name the capture page uses, all pointed at the one loopback listener.
fn host_resolver_rules() -> String {
    format!(
        "MAP {CAPTURE_HOST} 127.0.0.1, MAP {SAME_SITE_HOST} 127.0.0.1, \
         MAP {CROSS_SITE_HOST} 127.0.0.1"
    )
}

/// How long to wait for the browser to make its request.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(25);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut argv = std::env::args().skip(1);
    if argv.next().as_deref() == Some("--dump") {
        let dir = argv.next().ok_or("--dump needs a directory")?;
        return dump(Path::new(&dir));
    }
    let options = Options::parse(std::env::args().skip(1))?;
    let version = browser_version(&options.chrome)?;
    println!("browser: {version}");

    let mut runs = Vec::new();
    for run in 1..=options.runs {
        println!("--- run {run}/{} (permuted extensions) ---", options.runs);
        let capture = capture(&options, false)?;
        report(&capture);
        runs.push(capture);
    }

    let mut canonical_runs = Vec::new();
    if options.canonical {
        for run in 1..=options.runs {
            println!(
                "--- canonical run {run}/{} ({NO_PERMUTE_FLAG}) ---",
                options.runs
            );
            match capture(&options, true) {
                Ok(capture) => {
                    report(&capture);
                    canonical_runs.push(capture);
                }
                Err(err) => eprintln!("canonical run failed: {err}"),
            }
        }
    }

    check_stability("permuted", &runs);
    let mut canonical = None;
    if !canonical_runs.is_empty() {
        check_stability("canonical", &canonical_runs);
        if report_permutation_flag(&runs, &canonical_runs) {
            canonical = canonical_runs.first();
        }
    }

    write_outputs(&options.out, &version, &options, &runs[0], canonical)?;
    println!("\nwrote {}", options.out.display());
    Ok(())
}

/// Command line of `fp-capture`.
struct Options {
    chrome: PathBuf,
    out: PathBuf,
    runs: usize,
    canonical: bool,
    ip_literal: bool,
    headful: bool,
    lang: String,
}

impl Options {
    fn parse(args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut options = Self {
            chrome: PathBuf::from(DEFAULT_CHROME),
            out: repository_root().join("profiles/captures/chrome_144_macos"),
            runs: 3,
            canonical: false,
            ip_literal: false,
            headful: false,
            lang: "en-US".to_owned(),
        };
        let mut args = args.peekable();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--chrome" => options.chrome = args.next().ok_or("--chrome needs a path")?.into(),
                "--out" => options.out = args.next().ok_or("--out needs a path")?.into(),
                "--runs" => {
                    options.runs = args
                        .next()
                        .ok_or("--runs needs a number")?
                        .parse()
                        .map_err(|_| "--runs needs a number")?;
                }
                "--canonical" => options.canonical = true,
                "--ip" => options.ip_literal = true,
                "--headful" => options.headful = true,
                "--lang" => options.lang = args.next().ok_or("--lang needs a tag")?,
                "--help" | "-h" => {
                    println!(
                        "fp-capture [--chrome PATH] [--out DIR] [--runs N] [--canonical] [--ip] \
                         [--headful] [--lang TAG]\nfp-capture --dump DIR"
                    );
                    std::process::exit(0);
                }
                other => return Err(format!("unknown argument {other}")),
            }
        }
        if options.runs == 0 {
            return Err("--runs must be at least 1".to_owned());
        }
        Ok(options)
    }
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

/// What one browser launch pair produced.
#[derive(Default)]
struct Capture {
    client_hello: Option<Vec<u8>>,
    h2_preface: Option<Vec<u8>>,
    /// The request from the plain-`http://` run: an insecure origin, so Chrome
    /// sends neither client hints nor `Sec-Fetch-*` and offers no brotli.
    http1_request: Option<Vec<u8>>,
    /// Requests from a TLS run where the server picked ALPN `http/1.1`, which is
    /// the header set a secure HTTP/1.1 origin really sees. The first is the
    /// navigation; the second is the sub-resource it pulls in, which carries the
    /// `Cookie` header the navigation could not.
    https_http1_requests: Vec<Vec<u8>>,
    /// Every request of a real HTTP/2 page load: the navigation, the
    /// sub-resources it pulls in, a `fetch()`, a form POST and a second,
    /// cookie-bearing navigation. This is where the `priority` header's
    /// per-resource values and the position of `cookie`, `referer`,
    /// `content-type` and `content-length` come from.
    h2_requests: Vec<H2Request>,
    alpn: Option<Vec<u8>>,
}

/// One decoded HTTP/2 request from the page-load capture.
struct H2Request {
    stream_id: u32,
    /// Header fields in wire order, pseudo-headers included.
    headers: Vec<(String, String)>,
    /// The PRIORITY flag's payload, when the HEADERS frame carried one.
    priority: Option<(bool, u32, u16)>,
}

impl H2Request {
    fn path(&self) -> &str {
        self.header(":path").unwrap_or("")
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, value)| value.as_str())
    }
}

fn browser_version(chrome: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let out = Command::new(chrome).arg("--version").output()?;
    if !out.status.success() {
        return Err(format!("{} --version failed", chrome.display()).into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// Runs the browser twice — once at the TLS listener, once at the plain-HTTP
/// listener — and returns what both recorded.
fn capture(options: &Options, canonical: bool) -> Result<Capture, Box<dyn std::error::Error>> {
    let shared = Arc::new(Mutex::new(Capture::default()));
    let stop = Arc::new(AtomicBool::new(false));

    let h2_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let h1_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let http_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let page_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let h2_port = h2_listener.local_addr()?.port();
    let h1_port = h1_listener.local_addr()?.port();
    let http_port = http_listener.local_addr()?.port();
    let page_port = page_listener.local_addr()?.port();

    let h2_acceptor = Arc::new(build_acceptor(b"\x02h2\x08http/1.1")?);
    let h1_acceptor = Arc::new(build_acceptor(b"\x08http/1.1")?);
    let page_acceptor = Arc::new(build_acceptor(b"\x02h2")?);
    let h2_thread = spawn_listener(h2_listener, stop.clone(), {
        let shared = shared.clone();
        move |stream| serve_tls(stream, &h2_acceptor, &shared)
    });
    let page_thread = spawn_listener(page_listener, stop.clone(), {
        let shared = shared.clone();
        move |stream| {
            if let Err(err) = serve_h2_page(stream, &page_acceptor, &shared, page_port) {
                eprintln!("  h2 page connection: {err}");
            }
        }
    });
    let h1_thread = spawn_listener(h1_listener, stop.clone(), {
        let shared = shared.clone();
        move |stream| serve_tls(stream, &h1_acceptor, &shared)
    });
    let http_thread = spawn_listener(http_listener, stop.clone(), {
        let shared = shared.clone();
        move |stream| serve_http1(stream, &shared)
    });

    let host = if options.ip_literal {
        "127.0.0.1"
    } else {
        CAPTURE_HOST
    };
    run_browser(
        options,
        canonical,
        &format!("https://{host}:{h2_port}/"),
        || shared.lock().expect("capture lock").h2_preface.is_some(),
    )?;
    run_browser(
        options,
        canonical,
        &format!("https://{host}:{h1_port}/"),
        || {
            shared
                .lock()
                .expect("capture lock")
                .https_http1_requests
                .len()
                >= 2
        },
    )?;
    run_browser(
        options,
        canonical,
        &format!("http://{host}:{http_port}/"),
        || shared.lock().expect("capture lock").http1_request.is_some(),
    )?;
    // A whole page over HTTP/2: navigation, sub-resources, fetch, POST and a
    // second navigation that carries the cookie the first one set.
    run_browser(
        options,
        canonical,
        &format!("https://{host}:{page_port}/"),
        || {
            let capture = shared.lock().expect("capture lock");
            let paths: Vec<&str> = capture
                .h2_requests
                .iter()
                .map(|request| request.path())
                .collect();
            ["/second", "/api-post", "/cross-site.png", "/same-site.png"]
                .iter()
                .all(|wanted| paths.contains(wanted))
        },
    )?;

    stop.store(true, Ordering::Relaxed);
    // The listeners poll the stop flag; connecting once wakes any that are mid-sleep.
    for port in [h2_port, h1_port, http_port, page_port] {
        let _ = TcpStream::connect(SocketAddr::from((Ipv4Addr::LOCALHOST, port)));
    }
    let _ = h2_thread.join();
    let _ = h1_thread.join();
    let _ = http_thread.join();
    let _ = page_thread.join();

    let capture = Arc::try_unwrap(shared)
        .map_err(|_| "capture still borrowed")?
        .into_inner()
        .map_err(|_| "capture lock poisoned")?;
    if capture.client_hello.is_none() {
        return Err("browser sent no ClientHello".into());
    }
    Ok(capture)
}

/// Accepts connections until `stop` is set, handling each on its own thread.
fn spawn_listener<F>(
    listener: TcpListener,
    stop: Arc<AtomicBool>,
    handler: F,
) -> thread::JoinHandle<()>
where
    F: Fn(TcpStream) + Send + Sync + 'static,
{
    thread::spawn(move || {
        listener
            .set_nonblocking(true)
            .expect("listener non-blocking");
        let handler = Arc::new(handler);
        let mut workers = Vec::new();
        while !stop.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((stream, _)) => {
                    let handler = handler.clone();
                    workers.push(thread::spawn(move || handler(stream)));
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(2));
                }
                Err(_) => break,
            }
        }
        for worker in workers {
            let _ = worker.join();
        }
    })
}

/// A TLS server with a throw-away certificate that offers `alpn` (in the wire
/// format of the ALPN extension: one length byte per protocol name).
fn build_acceptor(alpn: &'static [u8]) -> Result<SslAcceptor, Box<dyn std::error::Error>> {
    let certified =
        rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_owned(), "localhost".to_owned()])?;
    let cert = boring::x509::X509::from_pem(certified.cert.pem().as_bytes())?;
    let key =
        boring::pkey::PKey::private_key_from_pem(certified.signing_key.serialize_pem().as_bytes())?;

    let mut builder = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls())?;
    builder.set_certificate(&cert)?;
    builder.set_private_key(&key)?;
    builder.set_alpn_select_callback(move |_ssl, client| {
        select_next_proto(alpn, client).ok_or(AlpnError::NOACK)
    });
    Ok(builder.build())
}

/// Records the ClientHello, completes the handshake, records the HTTP/2 preface
/// and answers with an empty 200 so the browser finishes its navigation.
fn serve_tls(stream: TcpStream, acceptor: &SslAcceptor, shared: &Mutex<Capture>) {
    if let Err(err) = serve_tls_inner(stream, acceptor, shared) {
        // Chrome preconnects and abandons sockets; a failed connection is normal
        // as long as one of them produced a capture.
        eprintln!("  tls connection: {err}");
    }
}

fn serve_tls_inner(
    stream: TcpStream,
    acceptor: &SslAcceptor,
    shared: &Mutex<Capture>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Accepted sockets inherit the listener's non-blocking flag on macOS, and
    // BoringSSL's blocking handshake cannot drive a non-blocking socket.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let hello = peek_client_hello(&stream)?;
    {
        let mut capture = shared.lock().map_err(|_| "capture lock poisoned")?;
        capture.client_hello.get_or_insert(hello);
    }

    let mut tls = acceptor.accept(stream)?;
    let alpn = tls.ssl().selected_alpn_protocol().map(<[u8]>::to_vec);
    {
        let mut capture = shared.lock().map_err(|_| "capture lock poisoned")?;
        if capture.alpn.is_none() {
            capture.alpn.clone_from(&alpn);
        }
    }
    if alpn.as_deref() != Some(b"h2") {
        // Chrome keeps the connection alive, so the sub-resource request arrives
        // on the same socket.
        loop {
            let Ok(request) = read_http1_request(&mut tls) else {
                return Ok(());
            };
            let subresource = request.starts_with(b"GET /sub.css");
            {
                let mut capture = shared.lock().map_err(|_| "capture lock poisoned")?;
                capture.https_http1_requests.push(request);
            }
            tls.write_all(http1_response(subresource).as_bytes())?;
            tls.flush()?;
            if subresource {
                return Ok(());
            }
        }
    }

    // Our SETTINGS first: the client does not wait for them, but a well-formed
    // server preface keeps Chrome from tearing the connection down early.
    tls.write_all(&frame(0x4, 0, 0, &[]))?;
    let preface = read_h2_preface(&mut tls)?;
    let stream_id = parse_h2_preface(&preface)
        .ok()
        .and_then(|p| p.first_headers.map(|h| h.stream_id))
        .unwrap_or(1);
    {
        let mut capture = shared.lock().map_err(|_| "capture lock poisoned")?;
        capture.h2_preface.get_or_insert(preface);
    }

    tls.write_all(&frame(0x4, 0x1, 0, &[]))?; // SETTINGS ack
    // ":status: 200" (static index 8) and "content-type: text/html" (literal
    // without indexing, static name index 31).
    let mut block = vec![0x88, 0x0f, 0x10, 9];
    block.extend_from_slice(b"text/html");
    tls.write_all(&frame(0x1, 0x4 | 0x1, stream_id, &block))?;
    let mut goaway = Vec::new();
    goaway.extend_from_slice(&stream_id.to_be_bytes());
    goaway.extend_from_slice(&0u32.to_be_bytes());
    tls.write_all(&frame(0x7, 0, 0, &goaway))?;
    let _ = tls.flush();
    Ok(())
}

/// Serves a small HTTP/2 site and records every request the browser makes.
///
/// The navigation sets a cookie and pulls in a stylesheet, a script, an image,
/// a font and a `fetch()`; the page then submits a form (POST) and finally
/// navigates to `/second`, which is the only way to see where Chrome places
/// `cookie` and `referer` in an HTTP/2 *navigation*. Everything is recorded
/// decoded, in wire order, with the HEADERS frame's PRIORITY payload.
fn serve_h2_page(
    stream: TcpStream,
    acceptor: &SslAcceptor,
    shared: &Mutex<Capture>,
    port: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut tls = acceptor.accept(stream)?;
    if tls.ssl().selected_alpn_protocol() != Some(b"h2") {
        return Ok(());
    }

    // Our SETTINGS, then the connection window Chrome's own server would give.
    tls.write_all(&frame(0x4, 0, 0, &[]))?;
    tls.flush()?;

    let mut decoder = HpackDecoder::new(4096);
    let mut buffer: Vec<u8> = Vec::with_capacity(16 * 1024);
    let mut chunk = [0u8; 8192];
    let mut pending_block: Vec<u8> = Vec::new();
    let mut pending_stream = 0u32;
    let mut pending_priority = None;
    // Streams whose HEADERS arrived without END_STREAM: a request with a body.
    let mut open_streams: std::collections::HashMap<u32, String> = std::collections::HashMap::new();
    let deadline = Instant::now() + Duration::from_secs(20);

    while Instant::now() < deadline {
        let read = match tls.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => n,
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(err) if err.kind() == std::io::ErrorKind::TimedOut => continue,
            Err(_) => break,
        };
        buffer.extend_from_slice(&chunk[..read]);

        // The client preface precedes the first frame.
        const MAGIC: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
        if buffer.starts_with(&MAGIC[..buffer.len().min(MAGIC.len())]) {
            if buffer.len() < MAGIC.len() {
                continue;
            }
            buffer.drain(..MAGIC.len());
        }

        while buffer.len() >= 9 {
            let length =
                usize::from(buffer[0]) << 16 | usize::from(buffer[1]) << 8 | usize::from(buffer[2]);
            if buffer.len() < 9 + length {
                break;
            }
            let kind = buffer[3];
            let flags = buffer[4];
            let stream_id =
                u32::from_be_bytes([buffer[5], buffer[6], buffer[7], buffer[8]]) & 0x7fff_ffff;
            let payload: Vec<u8> = buffer[9..9 + length].to_vec();
            buffer.drain(..9 + length);

            match kind {
                0x0 => {
                    // DATA: a form POST body. Reply once the client is done.
                    if flags & 0x1 != 0 {
                        let path = open_streams
                            .get(&stream_id)
                            .cloned()
                            .unwrap_or_else(|| "/form".to_owned());
                        respond(&mut tls, stream_id, &path, port)?;
                    }
                }
                0x1 | 0x9 => {
                    let (priority, block) = if kind == 0x1 {
                        let mut rest = payload.as_slice();
                        let mut priority = None;
                        if flags & 0x8 != 0 {
                            // PADDED: one length byte, padding at the end.
                            let pad = usize::from(rest[0]);
                            rest = &rest[1..rest.len() - pad];
                        }
                        if flags & 0x20 != 0 {
                            let dependency =
                                u32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]);
                            priority = Some((
                                dependency & 0x8000_0000 != 0,
                                dependency & 0x7fff_ffff,
                                u16::from(rest[4]) + 1,
                            ));
                            rest = &rest[5..];
                        }
                        (priority, rest.to_vec())
                    } else {
                        (pending_priority, payload)
                    };
                    if kind == 0x1 {
                        pending_block.clear();
                        pending_stream = stream_id;
                        pending_priority = priority;
                    }
                    pending_block.extend_from_slice(&block);
                    if flags & 0x4 == 0 {
                        // Waiting for CONTINUATION.
                        continue;
                    }
                    let headers = decoder.decode(&pending_block)?;
                    pending_block.clear();
                    let request = H2Request {
                        stream_id: pending_stream,
                        headers,
                        priority: pending_priority,
                    };
                    let path = request.path().to_owned();
                    let end_stream = flags & 0x1 != 0;
                    {
                        let mut capture = shared.lock().map_err(|_| "capture lock poisoned")?;
                        capture.h2_requests.push(request);
                    }
                    if end_stream {
                        respond(&mut tls, pending_stream, &path, port)?;
                    } else {
                        open_streams.insert(pending_stream, path);
                    }
                }
                0x4 if flags & 0x1 == 0 => {
                    tls.write_all(&frame(0x4, 0x1, 0, &[]))?;
                    tls.flush()?;
                }
                0x6 if flags & 0x1 == 0 => {
                    tls.write_all(&frame(0x6, 0x1, 0, &payload))?;
                    tls.flush()?;
                }
                0x7 => return Ok(()),
                _ => {}
            }
        }
    }
    Ok(())
}

/// Answers one HTTP/2 stream with the body `path` asks for.
fn respond(
    tls: &mut impl Write,
    stream_id: u32,
    path: &str,
    port: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    let (content_type, body, set_cookie) = h2_page_body(path, port);
    let mut fields: Vec<(&str, String)> = vec![
        ("content-type", content_type.to_owned()),
        ("content-length", body.len().to_string()),
    ];
    if set_cookie {
        fields.push(("set-cookie", "fp=1; Path=/".to_owned()));
    }
    let mut block = vec![0x08, 3];
    block.extend_from_slice(b"200");
    for (name, value) in &fields {
        block.push(0x00);
        block.push(u8::try_from(name.len())?);
        block.extend_from_slice(name.as_bytes());
        block.push(u8::try_from(value.len())?);
        block.extend_from_slice(value.as_bytes());
    }
    tls.write_all(&frame(0x1, 0x4, stream_id, &block))?;
    tls.write_all(&frame(0x0, 0x1, stream_id, body.as_bytes()))?;
    tls.flush()?;
    Ok(())
}

/// The site the HTTP/2 capture serves: one page per path.
fn h2_page_body(path: &str, port: u16) -> (&'static str, String, bool) {
    match path {
        "/sub.css" => (
            "text/css",
            "@font-face{font-family:fp;src:url(/sub.woff2) format('woff2')}\
             body{font-family:fp;color:#000}"
                .to_owned(),
            false,
        ),
        "/sub.js" => (
            "application/javascript",
            "globalThis.fp=1;".to_owned(),
            false,
        ),
        "/sub.png" => ("image/png", "not-a-png".to_owned(), false),
        "/sub.woff2" => ("font/woff2", "not-a-font".to_owned(), false),
        "/api" | "/api-post" => ("application/json", "{\"fp\":1}".to_owned(), false),
        "/same-site.png" | "/cross-site.png" => ("image/png", "not-a-png".to_owned(), false),
        "/form" => (
            "text/html; charset=utf-8",
            "<!doctype html><title>fp form</title>\
             <script>location.href='/second'</script>"
                .to_owned(),
            false,
        ),
        "/second" => (
            "text/html; charset=utf-8",
            "<!doctype html><title>fp second</title><p>done".to_owned(),
            false,
        ),
        // The navigation: sets a cookie, pulls in every sub-resource kind and
        // then posts a form, which is the only way to see `content-type` and
        // `content-length` in a real Chrome HEADERS block.
        _ => (
            "text/html; charset=utf-8",
            format!(
                "<!doctype html><title>fp-capture</title>\
                 <link rel=stylesheet href=/sub.css>\
                 <script src=/sub.js></script>\
                 <img src=/sub.png alt=x>\
                 <img src=\"https://{SAME_SITE_HOST}:{port}/same-site.png\" alt=s>\
                 <img src=\"https://{CROSS_SITE_HOST}:{port}/cross-site.png\" alt=c>\
                 <form id=f method=post action=/form><input name=a value=1></form>\
                 <p>captured\
                 <script>fetch('/api').then(function(){{\
                 fetch('/api-post',{{method:'POST',\
                 headers:{{'Content-Type':'application/json'}},body:'{{\"a\":1}}'}});\
                 setTimeout(function(){{document.getElementById('f').submit();}},600);}});\
                 </script>"
            ),
            true,
        ),
    }
}

/// Reads the ClientHello without consuming it, so BoringSSL still sees it.
fn peek_client_hello(stream: &TcpStream) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut buf = vec![0u8; 16 * 1024];
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let n = stream.peek(&mut buf)?;
        if n >= 5 {
            if buf[0] != 0x16 {
                return Err("first record is not a handshake".into());
            }
            let length = usize::from(u16::from_be_bytes([buf[3], buf[4]]));
            if n >= 5 + length {
                return Ok(buf[..5 + length].to_vec());
            }
        }
        if n == buf.len() {
            return Err("ClientHello larger than the peek buffer".into());
        }
        thread::sleep(Duration::from_millis(1));
    }
    Err("timed out waiting for a ClientHello".into())
}

/// Reads client bytes until they contain a complete first HEADERS frame.
fn read_h2_preface(tls: &mut impl Read) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let n = tls.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if parse_h2_preface(&buf).is_ok_and(|p| p.first_headers.is_some()) {
            return Ok(buf);
        }
    }
    Err("no HEADERS frame before the deadline".into())
}

/// Builds an HTTP/2 frame.
fn frame(kind: u8, flags: u8, stream_id: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(9 + payload.len());
    let length = u32::try_from(payload.len()).expect("frame payload fits in 24 bits");
    out.extend_from_slice(&length.to_be_bytes()[1..]);
    out.push(kind);
    out.push(flags);
    out.extend_from_slice(&stream_id.to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// Reads request bytes up to the end of the header block.
fn read_http1_request(stream: &mut impl Read) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut request = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..n]);
        if request.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    if request.is_empty() {
        return Err("empty HTTP/1.1 request".into());
    }
    Ok(request)
}

/// Records the raw HTTP/1.1 request bytes, then answers with a tiny page.
fn serve_http1(mut stream: TcpStream, shared: &Mutex<Capture>) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let Ok(request) = read_http1_request(&mut stream) else {
        return;
    };
    if let Ok(mut capture) = shared.lock() {
        capture.http1_request.get_or_insert(request);
    }
    let _ = stream.write_all(http1_response(false).as_bytes());
    let _ = stream.flush();
}

/// A page that sets a cookie and pulls in one sub-resource, so that the next
/// request on the connection shows where Chrome places `Cookie`.
fn http1_response(subresource: bool) -> String {
    if subresource {
        let body = "body{color:#000}";
        return format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/css\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
    }
    let body =
        "<!doctype html><title>fp-capture</title><link rel=stylesheet href=/sub.css><p>captured";
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nSet-Cookie: fp=1; Path=/\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
}

/// Launches the browser at `url` and waits until `done` or the timeout.
fn run_browser(
    options: &Options,
    canonical: bool,
    url: &str,
    done: impl Fn() -> bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let profile = tenta_testutil::temp_dir("fp-capture");
    let mut command = Command::new(&options.chrome);
    command.args(CHROME_FLAGS);
    if !options.headful {
        command.args(HEADLESS_FLAGS);
    }
    if !options.ip_literal {
        command.arg(format!("--host-resolver-rules={}", host_resolver_rules()));
    }
    if canonical {
        command.arg(NO_PERMUTE_FLAG);
    }
    command
        .arg(format!("--lang={}", options.lang))
        .arg(format!("--accept-lang={}", options.lang))
        .arg(format!("--user-data-dir={}", profile.path().display()))
        .arg(url)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = command.spawn()?;

    let deadline = Instant::now() + CAPTURE_TIMEOUT;
    while Instant::now() < deadline {
        if done() {
            break;
        }
        if child.try_wait()?.is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    // Give the browser a moment to finish the exchange it started.
    thread::sleep(Duration::from_millis(300));
    let _ = child.kill();
    let _ = child.wait();
    Ok(())
}

/// The fingerprints of one capture.
struct Fingerprints {
    ja4: String,
    ja4_r: String,
    ja3: String,
    akamai: String,
}

fn fingerprints(capture: &Capture) -> Result<Fingerprints, Box<dyn std::error::Error>> {
    let hello = parse_client_hello(capture.client_hello.as_deref().ok_or("no ClientHello")?)?;
    let ja4 = ja4(&hello);
    let akamai = match &capture.h2_preface {
        Some(bytes) => akamai_h2(&parse_h2_preface(bytes)?),
        None => String::new(),
    };
    Ok(Fingerprints {
        ja4: ja4.full.clone(),
        ja4_r: ja4.raw(),
        ja3: ja3(&hello),
        akamai,
    })
}

fn report(capture: &Capture) {
    match fingerprints(capture) {
        Ok(f) => {
            println!("  JA4      {}", f.ja4);
            println!("  JA4_r    {}", f.ja4_r);
            println!(
                "  JA3      {}  (per-run: Chrome permutes extensions)",
                f.ja3
            );
            println!("  Akamai   {}", f.akamai);
        }
        Err(err) => eprintln!("  fingerprint failed: {err}"),
    }
    if let Some(hello) = &capture.client_hello
        && let Ok(hello) = parse_client_hello(hello)
    {
        println!(
            "  extensions (wire order) {:?}",
            hello.extensions.iter().map(|e| e.kind).collect::<Vec<_>>()
        );
        println!("  key_share groups {:?}", hello.key_share_groups());
        println!(
            "  ALPS {:?}",
            hello.application_settings().map(|a| a.codepoint)
        );
    }
    if let Some(bytes) = &capture.h2_preface
        && let Ok(preface) = parse_h2_preface(bytes)
    {
        println!("  h2 settings {:?}", preface.settings);
        println!("  h2 window_update {:?}", preface.window_update);
        println!("  h2 priority frames {:?}", preface.priority_frames);
        if let Some(headers) = &preface.first_headers {
            println!("  h2 headers priority {:?}", headers.priority);
            println!("  h2 header order {:?}", headers.header_order());
        }
    }
    for request in &capture.h2_requests {
        println!(
            "  h2 request {:<16} {:<28} priority header {:?} frame priority {:?}",
            request.header(":authority").unwrap_or_default(),
            request.path(),
            request.header("priority"),
            request.priority
        );
        println!(
            "    site {:?} mode {:?} dest {:?}",
            request.header("sec-fetch-site"),
            request.header("sec-fetch-mode"),
            request.header("sec-fetch-dest"),
        );
        println!(
            "    order {:?}",
            request
                .headers
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>()
        );
    }
}

/// Reports whether JA4 and Akamai held across runs; JA3 is expected to move.
fn check_stability(label: &str, runs: &[Capture]) {
    let all: Vec<Fingerprints> = runs.iter().filter_map(|c| fingerprints(c).ok()).collect();
    let Some(first) = all.first() else { return };
    println!("\nstability across {} {label} runs:", all.len());
    let ja4_stable = all.iter().all(|f| f.ja4 == first.ja4);
    let akamai_stable = all.iter().all(|f| f.akamai == first.akamai);
    let ja3_stable = all.iter().all(|f| f.ja3 == first.ja3);
    println!(
        "  JA4    {}",
        if ja4_stable { "stable" } else { "UNSTABLE" }
    );
    println!(
        "  Akamai {}",
        if akamai_stable { "stable" } else { "UNSTABLE" }
    );
    println!(
        "  JA3    {} (permutation {})",
        if ja3_stable { "stable" } else { "varies" },
        if ja3_stable {
            "not observed"
        } else {
            "observed"
        }
    );
}

/// Says whether the canonical runs really produced one unpermuted extension order.
///
/// A single canonical run proves nothing — any permutation looks like an order.
/// The flag worked only if every canonical run agrees while the permuted runs do
/// not.
fn report_permutation_flag(permuted: &[Capture], canonical: &[Capture]) -> bool {
    let orders = |runs: &[Capture]| -> Vec<Vec<u16>> {
        runs.iter()
            .filter_map(|c| c.client_hello.as_deref())
            .filter_map(|b| parse_client_hello(b).ok())
            .map(|h| h.extensions.iter().map(|e| e.kind).collect())
            .collect()
    };
    let permuted_orders = orders(permuted);
    let canonical_orders = orders(canonical);
    let permuted_vary = permuted_orders.windows(2).any(|w| w[0] != w[1]);
    let canonical_stable =
        canonical_orders.len() > 1 && canonical_orders.windows(2).all(|w| w[0] == w[1]);
    println!("\n{NO_PERMUTE_FLAG}:");
    println!("  permuted runs vary:     {permuted_vary}");
    println!("  canonical runs agree:   {canonical_stable}");
    println!(
        "  verdict: the flag {}",
        if canonical_stable && permuted_vary {
            "works; the canonical order below is Chrome's built-in order"
        } else if canonical_stable {
            "may work, but the permuted runs did not vary either"
        } else {
            "does NOT stop permutation in this build"
        }
    );
    if let Some(order) = canonical_orders.first() {
        println!("  extension order of the first canonical run {order:?}");
    }
    canonical_stable && permuted_vary
}

fn write_outputs(
    out: &Path,
    version: &str,
    options: &Options,
    capture: &Capture,
    canonical: Option<&Capture>,
) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(out)?;
    if let Some(bytes) = &capture.client_hello {
        std::fs::write(out.join("client_hello.bin"), bytes)?;
    }
    if let Some(bytes) = &capture.h2_preface {
        std::fs::write(out.join("h2_preface.bin"), bytes)?;
    }
    if let Some(bytes) = &capture.http1_request {
        std::fs::write(out.join("http1_request.txt"), bytes)?;
    }
    if let Some(bytes) = capture.https_http1_requests.first() {
        std::fs::write(out.join("https_http1_request.txt"), bytes)?;
    }
    if let Some(bytes) = capture.https_http1_requests.get(1) {
        std::fs::write(out.join("https_http1_subresource.txt"), bytes)?;
    }
    if !capture.h2_requests.is_empty() {
        let requests: Vec<serde_json::Value> = capture
            .h2_requests
            .iter()
            .map(|request| {
                serde_json::json!({
                    "stream_id": request.stream_id,
                    "priority": request.priority.map(|(exclusive, dependency, weight)| {
                        serde_json::json!({
                            "exclusive": exclusive,
                            "dependency": dependency,
                            "weight": weight,
                        })
                    }),
                    "headers": request
                        .headers
                        .iter()
                        .map(|(name, value)| serde_json::json!([name, value]))
                        .collect::<Vec<_>>(),
                })
            })
            .collect();
        std::fs::write(
            out.join("h2_requests.json"),
            format!("{}\n", serde_json::to_string_pretty(&requests)?),
        )?;
    }
    if let Some(bytes) = canonical.and_then(|c| c.client_hello.as_ref()) {
        std::fs::write(out.join("client_hello_canonical.bin"), bytes)?;
    }

    let f = fingerprints(capture)?;
    let mut flags: Vec<String> = CHROME_FLAGS.iter().map(|s| (*s).to_owned()).collect();
    if !options.headful {
        flags.extend(HEADLESS_FLAGS.iter().map(|s| (*s).to_owned()));
    }
    if !options.ip_literal {
        flags.push(format!("--host-resolver-rules={}", host_resolver_rules()));
    }
    flags.push(format!("--lang={}", options.lang));
    flags.push(format!("--accept-lang={}", options.lang));
    flags.push("--user-data-dir=<temporary>".to_owned());
    let notes = serde_json::json!({
        "chrome_build": version,
        "captured": iso_date(),
        "flags": flags,
        "canonical_flag": if canonical.is_some() { NO_PERMUTE_FLAG } else { "" },
        "headful": options.headful,
        "sni": !options.ip_literal,
        "runs": options.runs,
        "alpn_selected": capture.alpn.as_ref().map(|a| String::from_utf8_lossy(a).into_owned()),
        "ja4": f.ja4,
        "ja4_r": f.ja4_r,
        "ja3_this_run": f.ja3,
        "akamai_h2": f.akamai,
        "tool": "cargo run -p tenta-testutil --features capture --bin fp-capture",
    });
    std::fs::write(
        out.join("notes.json"),
        format!("{}\n", serde_json::to_string_pretty(&notes)?),
    )?;
    Ok(())
}

/// Prints everything a profile records from an existing capture directory.
///
/// This is the view used when writing or reviewing `profiles/*.toml`: every
/// field of the TOML has a line here.
fn dump(dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let hello = parse_client_hello(&std::fs::read(dir.join("client_hello.bin"))?)?;
    let fingerprint = ja4(&hello);
    println!("ja4        {fingerprint}");
    println!("ja4_r      {}", fingerprint.raw());
    println!("ja3        {} (this capture only)", ja3(&hello));
    println!("ciphers    {}", hex_list(&hello.ciphers_without_grease()));
    println!(
        "extensions {:?} (one permuted sample)",
        hello.extensions_without_grease()
    );
    println!("groups     {:?}", hello.supported_groups());
    println!("key_share  {:?}", hello.key_share_groups());
    println!("sigalgs    {:?}", hello.signature_algorithms());
    println!(
        "alpn       {:?}",
        hello.alpn().map(|v| v
            .iter()
            .map(|p| String::from_utf8_lossy(p).into_owned())
            .collect::<Vec<_>>())
    );
    println!(
        "alps       codepoint {:?}",
        hello.application_settings().map(|a| a.codepoint)
    );
    println!(
        "cert_comp  {:?}  psk_modes {:?}  ec_point_formats {:?}",
        hello.certificate_compression_algorithms(),
        hello.psk_key_exchange_modes(),
        hello.ec_point_formats()
    );
    println!(
        "ech {:?}  sct {}  ems {}  padding {:?}  grease {}",
        hello.encrypted_client_hello(),
        hello.signed_certificate_timestamp(),
        hello.extended_master_secret(),
        hello.padding_len(),
        hello.uses_grease()
    );

    let preface = parse_h2_preface(&std::fs::read(dir.join("h2_preface.bin"))?)?;
    println!("\nakamai_h2  {}", akamai_h2(&preface));
    println!("settings   {:?}", preface.settings);
    println!("window     {:?}", preface.window_update);
    println!("priority   {:?}", preface.priority_frames);
    if let Some(headers) = &preface.first_headers {
        println!("pseudo     {}", headers.pseudo_header_order);
        println!("h_priority {:?}", headers.priority);
        println!("order      {:?}", headers.header_order());
        for (name, value) in &headers.headers {
            println!("  {name}: {value}");
        }
    }
    Ok(())
}

fn hex_list(values: &[u16]) -> String {
    values
        .iter()
        .map(|v| format!("{v:#06x}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Today's date as `YYYY-MM-DD` in UTC, from the civil-from-days algorithm.
fn iso_date() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}
