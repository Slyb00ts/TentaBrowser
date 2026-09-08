//! The transport budgets of `docs/spec/03-network.md` §10.
//!
//! These are measurements, not behaviour: they are `#[ignore]`d so a slow or
//! loaded machine cannot fail an ordinary test run. Run them with
//! `cargo nextest run -p tenta-net --run-ignored all -E 'binary(budgets)'`
//! and read the printed numbers.

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use common::TestServer;
use common::tls::{TlsMode, TlsServer};
use tenta_net::client::{Client, ClientConfig, NetRuntime};
use tenta_net::dns::Resolver;
use tenta_net::policy::private_network::DestinationPolicy;
use tenta_net::policy::session_cache::SessionCache;
use tenta_net::profile::{ClientProfile, Profile};
use tenta_net::request::Request;
use tenta_net::tls::TlsOptions;

/// Counts live Rust heap bytes so the idle-connection budget can be measured
/// without an external profiler.
struct CountingAllocator;

static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the layout comes from the caller of `alloc` and is passed
        // to the system allocator unchanged.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            LIVE_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        LIVE_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
        // SAFETY: `pointer` and `layout` are the ones this allocator returned.
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: forwarded unchanged to the system allocator.
        let new_pointer = unsafe { System.realloc(pointer, layout, new_size) };
        if !new_pointer.is_null() {
            LIVE_BYTES.fetch_add(new_size, Ordering::Relaxed);
            LIVE_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
        }
        new_pointer
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn live_bytes() -> usize {
    LIVE_BYTES.load(Ordering::Relaxed)
}

/// Live bytes BoringSSL is holding.
///
/// BoringSSL has no `CRYPTO_set_mem_functions`; it exports the weak symbols
/// `OPENSSL_memory_alloc`/`_free`/`_get_size` instead, and uses them for every
/// allocation when a program defines all three (`crypto/mem.cc`). Defining
/// them here routes the library's heap through the Rust allocator with a size
/// prefix, which is what makes the connection budget countable rather than
/// guessed.
///
/// The hooks exist on ELF targets only: `crypto/mem.cc` guards them with
/// `#if !defined(__EDK2_BORINGSSL__) && defined(__ELF__) && defined(__GNUC__)`
/// and compiles them to a `nullptr` constant everywhere else, so on macOS and
/// Windows nothing can intercept them and the measurement falls back to RSS.
static BORING_BYTES: AtomicUsize = AtomicUsize::new(0);

/// The size stored before every block handed to BoringSSL. 16 bytes keeps the
/// returned pointer 16-aligned, which is what the library assumes.
const BORING_PREFIX: usize = 16;

fn boring_layout(size: usize) -> Layout {
    Layout::from_size_align(size + BORING_PREFIX, BORING_PREFIX).expect("layout")
}

#[unsafe(no_mangle)]
extern "C" fn OPENSSL_memory_alloc(size: usize) -> *mut std::ffi::c_void {
    // SAFETY: a non-zero layout, allocated and immediately size-tagged.
    let pointer = unsafe { System.alloc(boring_layout(size)) };
    if pointer.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: the block is at least `BORING_PREFIX` bytes and aligned for it.
    unsafe { pointer.cast::<usize>().write(size) };
    BORING_BYTES.fetch_add(size, Ordering::Relaxed);
    // SAFETY: still inside the same allocation.
    unsafe { pointer.add(BORING_PREFIX) }.cast()
}

#[unsafe(no_mangle)]
extern "C" fn OPENSSL_memory_free(pointer: *mut std::ffi::c_void) {
    if pointer.is_null() {
        return;
    }
    // SAFETY: `pointer` came from `OPENSSL_memory_alloc`, so the prefix is
    // there and holds the size that was allocated.
    unsafe {
        let base = pointer.cast::<u8>().sub(BORING_PREFIX);
        let size = base.cast::<usize>().read();
        BORING_BYTES.fetch_sub(size, Ordering::Relaxed);
        System.dealloc(base, boring_layout(size));
    }
}

#[unsafe(no_mangle)]
extern "C" fn OPENSSL_memory_get_size(pointer: *mut std::ffi::c_void) -> usize {
    if pointer.is_null() {
        return 0;
    }
    // SAFETY: as above; the prefix of a block this shim returned.
    unsafe {
        pointer
            .cast::<u8>()
            .sub(BORING_PREFIX)
            .cast::<usize>()
            .read()
    }
}

fn boring_bytes() -> usize {
    BORING_BYTES.load(Ordering::Relaxed)
}

/// Resident set size, for a cross-check that owes nothing to any allocator.
fn resident_bytes() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
        let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
        Some(pages * 4096)
    }
    #[cfg(not(target_os = "linux"))]
    {
        // `ps` is the dependency-free way to ask on macOS; the value is in
        // kilobytes.
        let output = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p", &std::process::id().to_string()])
            .output()
            .ok()?;
        let text = String::from_utf8(output.stdout).ok()?;
        text.trim().parse::<u64>().ok().map(|kb| kb * 1024)
    }
}

fn loopback_resolver() -> Arc<Resolver> {
    let resolver = Resolver::literals_only(SessionCache::ephemeral());
    resolver.remember(
        "localhost",
        &[IpAddr::V4(Ipv4Addr::LOCALHOST)],
        Duration::from_secs(60),
    );
    Arc::new(resolver)
}

fn chrome_config(roots: Vec<Vec<u8>>) -> ClientConfig {
    ClientConfig {
        profile: Profile::Browser(Arc::new(
            ClientProfile::builtin("chrome_144_macos").expect("profile"),
        )),
        tls: TlsOptions {
            extra_root_cas: roots,
            ..TlsOptions::default()
        },
        destination: DestinationPolicy::permissive(),
        resolver: Some(loopback_resolver()),
        ..ClientConfig::default()
    }
}

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

#[tokio::test]
#[ignore = "budget measurement"]
async fn budget_cold_client_construction() {
    let runtime = NetRuntime::new(1).expect("io runtime");
    // The first client pays for the root store; the budget is the steady cost.
    let _warm: Client = runtime.client(chrome_config(Vec::new())).expect("client");

    let mut samples = Vec::new();
    for _ in 0..20 {
        let config = chrome_config(Vec::new());
        let started = Instant::now();
        let client = runtime.client(config).expect("client");
        samples.push(started.elapsed());
        drop(client);
    }
    let median = median(samples);
    println!("cold client construction: median {median:?}");
    assert!(
        median <= Duration::from_millis(1),
        "budget is 1 ms, measured {median:?}"
    );
}

#[tokio::test]
#[ignore = "budget measurement"]
async fn budget_cold_client_to_first_request() {
    // `S-cold-fetch` (`docs/spec/01-requirements.md`): a cold process has 8 ms
    // to get its first request on the wire. This is the client's share of it:
    // a fresh `NetRuntime`, a fresh client with the shipping defaults (which
    // include the system resolver) and one plain request to an IP literal, so
    // nothing here waits for DNS.
    let server = TestServer::start().await;
    let started = Instant::now();
    let runtime = NetRuntime::new(1).expect("io runtime");
    let client = runtime
        .client(ClientConfig {
            destination: DestinationPolicy::permissive(),
            allow_h2: false,
            ..ClientConfig::default()
        })
        .expect("client");
    let constructed = started.elapsed();
    let response = client.get(&server.url("/hello")).await.expect("response");
    response.bytes().await.expect("body");
    let total = started.elapsed();

    println!(
        "cold runtime + client + first HTTP/1.1 request: {total:?} \
         (of which {constructed:?} is construction, including one loopback round trip)"
    );
    assert!(
        !client.resolver().is_initialized(),
        "an IP-literal URL must not read the system DNS configuration"
    );
    // 8 ms is a release-build budget; an unoptimized build spends milliseconds
    // parsing the root store alone (measured: 3.0 ms release, 4.4-5.0 ms
    // debug on the reference machine). Assert the budget where it applies and
    // a regression ceiling otherwise -- reading the system DNS configuration
    // on this path used to cost 47 ms on its own.
    let budget = if cfg!(debug_assertions) {
        Duration::from_millis(25)
    } else {
        Duration::from_millis(8)
    };
    assert!(
        total <= budget,
        "budget is {budget:?} to the first request, measured {total:?}"
    );
}

#[tokio::test]
#[ignore = "budget measurement"]
async fn budget_warm_http2_request_overhead() {
    let server = TlsServer::start(TlsMode::ServeH2).await;
    let runtime = NetRuntime::new(1).expect("io runtime");
    let client = runtime
        .client(chrome_config(vec![server.cert_pem.clone()]))
        .expect("client");

    // Warm the pool: the measurement must not include a handshake.
    client
        .get(&server.url("/warm"))
        .await
        .expect("response")
        .bytes()
        .await
        .expect("body");

    let mut samples = Vec::new();
    for index in 0..50 {
        let before = server.capture.arrivals().len();
        let started = Instant::now();
        let response = client
            .get(&server.url(&format!("/item/{index}")))
            .await
            .expect("response");
        let arrivals = server.capture.arrivals();
        assert!(arrivals.len() > before, "the server saw the request");
        // Both instants are taken in this process, so the difference is the
        // client-side overhead plus one loopback hop.
        samples.push(arrivals[before].duration_since(started));
        response.bytes().await.expect("body");
    }
    let median = median(samples);
    println!("warm HTTP/2 request overhead (includes one loopback hop): median {median:?}");
    assert!(
        median <= Duration::from_micros(200),
        "budget is 200 µs, measured {median:?}"
    );
}

#[tokio::test]
#[ignore = "budget measurement"]
async fn budget_idle_http2_connection_memory() {
    // One client, several origins: a connection per origin, so the client's
    // own state (BoringSSL context, root store, resolver) is not counted.
    //
    // Both endpoints of every connection live in this process, so the numbers
    // below cover the client *and* the idling server. They are an upper bound
    // on the client's own footprint, which is the honest way to read them.
    const CONNECTIONS: usize = 100;
    let mut servers = Vec::new();
    for _ in 0..=CONNECTIONS {
        // An idling server: its side of the connection allocates almost
        // nothing, so the delta is dominated by the client.
        servers.push(TlsServer::start(TlsMode::AcceptAndIdle).await);
    }
    let roots: Vec<Vec<u8>> = servers
        .iter()
        .map(|server| server.cert_pem.clone())
        .collect();
    let runtime = NetRuntime::new(1).expect("io runtime");
    let client = runtime.client(chrome_config(roots)).expect("client");

    // The first connection pays for everything that is allocated once.
    client
        .preconnect(&servers[0].url("/"))
        .await
        .expect("preconnect");
    tokio::time::sleep(Duration::from_millis(50)).await;

    let rust_before = live_bytes();
    let boring_before = boring_bytes();
    let rss_before = resident_bytes();
    for server in &servers[1..] {
        client
            .preconnect(&server.url("/"))
            .await
            .expect("preconnect");
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(client.pool_stats().await.h2_connections, CONNECTIONS + 1);

    let rust = live_bytes().saturating_sub(rust_before) / CONNECTIONS;
    let boring = boring_bytes().saturating_sub(boring_before) / CONNECTIONS;
    let intercepted = boring_bytes() > 0;
    let rss = match (rss_before, resident_bytes()) {
        (Some(before), Some(after)) => Some(after.saturating_sub(before) / CONNECTIONS as u64),
        _ => None,
    };
    println!(
        "idle HTTP/2 connection over {CONNECTIONS} connections (both endpoints in \
         this process): {rust} bytes of Rust heap, {boring} bytes of BoringSSL heap \
         (interception {}), {} of RSS",
        if intercepted { "active" } else { "NOT ACTIVE" },
        match rss {
            Some(bytes) => format!("{bytes} bytes"),
            None => "unknown".to_owned(),
        }
    );
    if !intercepted {
        println!(
            "  (BoringSSL's allocation hooks are ELF-only, so on this target its \
             heap is visible only through RSS)"
        );
    }

    // RSS owes nothing to any allocator and misses nothing, so it is the
    // number the budget is judged on when it is available. It covers both
    // endpoints, which makes it an upper bound on the client alone.
    let measured = rss.unwrap_or((rust + boring) as u64);
    assert!(
        measured <= 64 * 1024,
        "budget is 64 KB per connection, measured {measured} bytes for the \
         client and the server together ({rust} Rust heap, {boring} BoringSSL heap)"
    );
}

#[tokio::test]
#[ignore = "budget measurement"]
async fn budget_one_kilobyte_of_response_headers() {
    let server = TestServer::start().await;
    let runtime = NetRuntime::new(1).expect("io runtime");
    let client = runtime
        .client(ClientConfig {
            destination: DestinationPolicy::permissive(),
            resolver: Some(loopback_resolver()),
            allow_h2: false,
            ..ClientConfig::default()
        })
        .expect("client");

    // `/hello` answers with a minimal head; `/big` with the same head plus a
    // 1 KB body. The marginal cost of the bytes is what is measured: the
    // difference between a small head and one with 1 KB of extra headers.
    let mut small = Vec::new();
    let mut large = Vec::new();
    for _ in 0..200 {
        for (path, samples) in [("/hello", &mut small), ("/headers1k", &mut large)] {
            let started = Instant::now();
            let response = client.get(&server.url(path)).await.expect("response");
            response.bytes().await.expect("body");
            samples.push(started.elapsed());
        }
    }
    let small = median(small);
    let large = median(large);
    let marginal = large.saturating_sub(small);
    // The 2 µs budget covers the parser itself, which hyper owns and which
    // cannot be timed from here: this number is a round trip over loopback
    // with 1 KB more of response head, so it is an upper bound, not the
    // budget. It is printed rather than asserted.
    println!(
        "response head: {small:?} minimal, {large:?} with 1 KB of headers, \
         marginal {marginal:?} (includes one loopback round trip)"
    );
}

#[test]
#[ignore = "budget measurement"]
fn budget_response_head_conversion() {
    // The 2 µs budget of §10 covers one 1 KB response head. hyper owns the
    // parser and cannot be timed from outside it, so what is timed here is
    // every byte of head work this crate does add: turning the parsed
    // `HeaderMap` into the ordered pairs a `Response` carries, which is done
    // once per response, plus the lookups the body pump performs on it.
    let head = [
        ("content-type", "text/html; charset=utf-8"),
        ("content-length", "128000"),
        ("date", "Sun, 07 Sep 2026 10:11:12 GMT"),
        ("server", "nginx/1.27.1"),
        ("cache-control", "max-age=0, private, must-revalidate"),
        ("etag", "\"6a2f-63f0d1c2b4a80\""),
        ("vary", "Accept-Encoding, Cookie, User-Agent"),
        ("content-encoding", "gzip"),
        (
            "strict-transport-security",
            "max-age=63072000; includeSubDomains",
        ),
        (
            "content-security-policy",
            "default-src 'self'; img-src 'self' data: https:; script-src 'self' 'unsafe-inline'",
        ),
        ("x-frame-options", "SAMEORIGIN"),
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "strict-origin-when-cross-origin"),
        (
            "set-cookie",
            "sid=8f2a1c9b4e7d6a35; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=86400",
        ),
        (
            "alt-svc",
            "h3=\":443\"; ma=2592000, h2=\":443\"; ma=2592000",
        ),
        (
            "link",
            "</style.css>; rel=preload; as=style, </app.js>; rel=preload; as=script",
        ),
        ("accept-ranges", "bytes"),
        ("age", "17"),
        (
            "via",
            "1.1 varnish, 1.1 edge-fra-07.example.net (Varnish/7.4)",
        ),
        ("x-cache", "HIT, MISS"),
        ("x-request-id", "01J9ZK4Q7M8N2P5R6T0V3W1Y8B-fra-edge-07"),
        ("timing-allow-origin", "*"),
        (
            "server-timing",
            "cdn-cache;desc=HIT, edge;dur=12, origin;dur=134, db;dur=41",
        ),
    ];
    let mut map = http::HeaderMap::new();
    for (name, value) in head {
        map.append(
            http::header::HeaderName::from_static(name),
            http::HeaderValue::from_str(value).expect("value"),
        );
    }
    let wire_bytes: usize = head
        .iter()
        .map(|(name, value)| name.len() + value.len() + 4)
        .sum();
    assert!(
        (900..1400).contains(&wire_bytes),
        "the fixture head is about a kilobyte: {wire_bytes} bytes"
    );

    const ROUNDS: u32 = 20_000;
    let started = Instant::now();
    let mut sink = 0usize;
    for _ in 0..ROUNDS {
        let pairs: Vec<(String, http::HeaderValue)> = map
            .iter()
            .map(|(name, value)| (name.as_str().to_owned(), value.clone()))
            .collect();
        sink += pairs.len();
        // What the transport asks of every head it receives.
        sink += usize::from(
            pairs
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("content-encoding")),
        );
    }
    assert_eq!(sink, ROUNDS as usize * (head.len() + 1));
    let each = started.elapsed() / ROUNDS;
    println!(
        "response head of {wire_bytes} bytes in {fields} fields: {each:?} per head \
         (the conversion and lookups this crate adds; hyper's parser is not \
         separable from here)",
        fields = head.len()
    );
    assert!(
        each <= Duration::from_micros(2),
        "budget is 2 µs per 1 KB head, measured {each:?}"
    );
}

#[tokio::test]
#[ignore = "budget measurement"]
async fn budget_request_construction_is_allocation_light() {
    let server = TestServer::start().await;
    let url = server.url("/hello");
    let before = live_bytes();
    let requests: Vec<Request> = (0..100)
        .map(|_| Request::get(&url).expect("request"))
        .collect();
    let per_request = live_bytes().saturating_sub(before) / requests.len();
    println!("request construction: {per_request} bytes of live heap per request");
    drop(requests);
}

#[test]
#[ignore = "budget measurement"]
fn budget_http_header_parsing_is_fast() {
    // `docs/spec/03-network.md` §10: a 1 KB response head parses in ≤ 2 µs.
    // hyper parses with httparse, so that is what is measured here, on a
    // Chrome-like head of 25 headers.
    let head: &[u8] = b"\
        HTTP/1.1 200 OK\r\n\
        Date: Mon, 06 Sep 2026 12:00:00 GMT\r\n\
        Content-Type: text/html; charset=utf-8\r\n\
        Content-Length: 27421\r\n\
        Connection: keep-alive\r\n\
        Server: Apache/2.4.41\r\n\
        Cache-Control: max-age=3600, public\r\n\
        ETag: \"deadbeef\"\r\n\
        Vary: Accept-Encoding, Accept-Language, Cookie\r\n\
        Accept-Ranges: bytes\r\n\
        X-Frame-Options: SAMEORIGIN\r\n\
        X-Content-Type-Options: nosniff\r\n\
        Referrer-Policy: strict-origin-when-cross-origin\r\n\
        Permissions-Policy: geolocation=(), microphone=(), camera=()\r\n\
        Strict-Transport-Security: max-age=63072000; includeSubDomains; preload\r\n\
        Content-Security-Policy: default-src 'self'; script-src 'self' 'unsafe-inline' cdn.example.com\r\n\
        Set-Cookie: session_id=a1b2c3d4e5f6g7h8; Path=/; Domain=example.com; Expires=Tue, 06 Sep 2027 12:00:00 GMT; HttpOnly; Secure; SameSite=Lax\r\n\
        Link: </css/style.min.css>; rel=preload; as=style, </js/app.min.js>; rel=preload; as=script\r\n\
        Age: 1234\r\n\
        Via: 1.1 varnish-cache-01, 1.0 edge-proxy-02\r\n\
        X-Cache: HIT from browser, MISS from origin\r\n\
        X-Request-ID: req-0123456789abcdef-fra-edge\r\n\
        Server-Timing: cdn-cache;desc=HIT;dur=1, edge;dur=45, origin;dur=200\r\n\
        Alt-Svc: h3=\":443\"; ma=86400, h2=\":443\"; ma=86400\r\n\
        CF-Ray: 1a2b3c4d5e6f7g8h-ORD\r\n\
        CF-Cache-Status: HIT\r\n\
        \r\n";
    assert!(
        (900..=1200).contains(&head.len()),
        "head should be ~1 KB, got {} bytes",
        head.len()
    );

    const ROUNDS: u32 = 50_000;
    let mut parsed_headers = 0usize;
    let started = Instant::now();
    for _ in 0..ROUNDS {
        let mut headers = [httparse::EMPTY_HEADER; 64];
        let mut response = httparse::Response::new(&mut headers);
        let status = response
            .parse(std::hint::black_box(head))
            .expect("the head is well-formed");
        assert!(status.is_complete(), "the whole head was parsed");
        parsed_headers += response.headers.len();
    }
    let per_head = started.elapsed() / ROUNDS;
    println!(
        "1 KB HTTP/1.1 head: {per_head:?} per httparse parse ({ROUNDS} rounds, {} headers each)",
        parsed_headers / ROUNDS as usize
    );

    // The 2 µs budget is a release-build number; an unoptimized httparse is
    // an order of magnitude slower, so debug builds get a regression ceiling.
    let budget = if cfg!(debug_assertions) {
        Duration::from_micros(20)
    } else {
        Duration::from_micros(2)
    };
    assert!(
        per_head <= budget,
        "budget is {budget:?} per 1 KB head, measured {per_head:?}"
    );
}
