//! Conformance of what the transport actually puts on the wire with the
//! fingerprint the profile promises (`docs/spec/03-network.md` §2).
//!
//! The ClientHello is captured off a loopback TLS server before any
//! handshake state exists, and the HTTP/2 preface is read decrypted on the
//! server side; both are then run through the open-standard algorithms in
//! `tenta-testutil`, never through the code that produced them.

mod common;

use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::Duration;

use common::tls::{TlsMode, TlsServer};
use tenta_net::client::{Client, ClientConfig, NetRuntime};
use tenta_net::dns::Resolver;
use tenta_net::error::ErrorName;
use tenta_net::policy::private_network::DestinationPolicy;
use tenta_net::policy::session_cache::SessionCache;
use tenta_net::profile::{ClientProfile, Profile};
use tenta_net::request::{Request, ResourceType};
use tenta_net::tls::TlsOptions;
use tenta_testutil::fingerprint::{
    ClientHello, akamai_h2, ext, is_grease, ja4, parse_client_hello, parse_h2_preface,
};

const BUILTIN_PROFILES: [&str; 3] = ["chrome_144_macos", "chrome_144_linux", "chrome_144_windows"];

fn loopback_resolver() -> Arc<Resolver> {
    let resolver = Resolver::literals_only(SessionCache::ephemeral());
    resolver.remember(
        "localhost",
        &[IpAddr::V4(Ipv4Addr::LOCALHOST)],
        Duration::from_secs(60),
    );
    Arc::new(resolver)
}

fn profile_client(runtime: &Arc<NetRuntime>, profile: &str, roots: Vec<Vec<u8>>) -> Client {
    let profile = ClientProfile::builtin(profile).expect("builtin profile");
    runtime
        .client(ClientConfig {
            profile: Profile::Browser(Arc::new(profile)),
            tls: TlsOptions {
                extra_root_cas: roots,
                ..TlsOptions::default()
            },
            destination: DestinationPolicy::permissive(),
            resolver: Some(loopback_resolver()),
            request_timeout: Some(Duration::from_secs(10)),
            ..ClientConfig::default()
        })
        .expect("client")
}

/// Connects once and returns whatever ClientHello the server saw.
async fn capture_hello(profile: &str) -> ClientHello {
    let server = TlsServer::start(TlsMode::HelloOnly).await;
    let runtime = NetRuntime::new(1).expect("io runtime");
    let client = profile_client(&runtime, profile, vec![server.cert_pem.clone()]);

    // The server closes after the ClientHello, so the request must fail.
    let _ = client.get(&server.url("/")).await;
    common::wait_for(|| server.capture.client_hello().is_some()).await;
    let bytes = server.capture.client_hello().expect("a ClientHello");
    parse_client_hello(&bytes).expect("parse ClientHello")
}

#[tokio::test]
async fn every_builtin_profile_produces_its_stored_ja4() {
    for name in BUILTIN_PROFILES {
        let profile = ClientProfile::builtin(name).expect("profile");
        let hello = capture_hello(name).await;
        let computed = ja4(&hello);
        assert_eq!(
            computed.full,
            profile.meta.ja4,
            "{name}: JA4 mismatch\n  raw: {}\n  stored raw: {}",
            computed.raw(),
            profile.meta.ja4_r
        );
        assert_eq!(computed.raw(), profile.meta.ja4_r, "{name}: JA4_r mismatch");
    }
}

#[tokio::test]
async fn the_cipher_list_is_the_profiles_in_order() {
    let profile = ClientProfile::builtin("chrome_144_macos").expect("profile");
    let hello = capture_hello("chrome_144_macos").await;
    assert_eq!(
        hello.ciphers_without_grease(),
        profile.cipher_ids(),
        "BoringSSL must offer the profile's ciphers in the profile's order"
    );
    assert!(hello.uses_grease(), "Chrome always sends GREASE (RFC 8701)");
}

#[tokio::test]
async fn the_extension_set_groups_and_signature_algorithms_match_the_profile() {
    let profile = ClientProfile::builtin("chrome_144_macos").expect("profile");
    let hello = capture_hello("chrome_144_macos").await;

    let mut sent: Vec<u16> = hello.extensions_without_grease();
    let mut expected: Vec<u16> = profile.tls.extensions.clone();
    sent.sort_unstable();
    expected.sort_unstable();
    assert_eq!(sent, expected, "extension set");

    let groups: Vec<u16> = hello
        .supported_groups()
        .expect("supported_groups")
        .into_iter()
        .filter(|group| !is_grease(*group))
        .collect();
    assert_eq!(
        groups,
        vec![0x11ec, 0x001d, 0x0017, 0x0018],
        "X25519MLKEM768, X25519, P-256, P-384"
    );

    let sigalgs = hello.signature_algorithms().expect("signature_algorithms");
    assert_eq!(
        sigalgs,
        vec![
            0x0403, 0x0804, 0x0401, 0x0503, 0x0805, 0x0501, 0x0806, 0x0601
        ],
        "signature algorithms are sent in the profile's order"
    );
}

#[tokio::test]
async fn alps_announces_h2_on_the_new_codepoint() {
    let hello = capture_hello("chrome_144_macos").await;
    let settings = hello
        .application_settings()
        .expect("an application_settings extension");
    assert!(
        settings.uses_new_codepoint(),
        "Chrome 144 sends ALPS on 17613, not 17513"
    );
    assert_eq!(
        settings.protocols,
        vec![b"h2".to_vec()],
        "ALPS is announced for h2 only"
    );
    assert_eq!(
        hello.certificate_compression_algorithms(),
        Some(vec![2]),
        "brotli is the only certificate compression algorithm Chrome offers"
    );
    assert_eq!(
        hello.alpn(),
        Some(vec![b"h2".to_vec(), b"http/1.1".to_vec()])
    );
    assert_eq!(hello.server_name().as_deref(), Some("localhost"));
}

#[tokio::test]
async fn the_extension_order_is_permuted_per_connection() {
    let server = TlsServer::start(TlsMode::HelloOnly).await;
    let runtime = NetRuntime::new(1).expect("io runtime");
    let client = profile_client(&runtime, "chrome_144_macos", vec![server.cert_pem.clone()]);

    // One connection per attempt: the capture keeps the first ClientHello, so
    // each order is read from a fresh server.
    let mut orders = Vec::new();
    for _ in 0..4 {
        let server = TlsServer::start(TlsMode::HelloOnly).await;
        let client = profile_client(&runtime, "chrome_144_macos", vec![server.cert_pem.clone()]);
        let _ = client.get(&server.url("/")).await;
        common::wait_for(|| server.capture.client_hello().is_some()).await;
        let hello =
            parse_client_hello(&server.capture.client_hello().expect("hello")).expect("parse");
        orders.push(hello.extensions_without_grease());
    }
    drop(client);
    drop(server);

    assert!(
        orders.windows(2).any(|pair| pair[0] != pair[1]),
        "extensions must be permuted per connection: {orders:?}"
    );
    let mut sorted: Vec<Vec<u16>> = orders
        .iter()
        .map(|order| {
            let mut order = order.clone();
            order.sort_unstable();
            order
        })
        .collect();
    sorted.dedup();
    assert_eq!(sorted.len(), 1, "the set never changes, only the order");
}

#[tokio::test]
async fn grease_is_pinned_first_and_last() {
    let hello = capture_hello("chrome_144_linux").await;
    let all = &hello.extensions;
    assert!(
        is_grease(all.first().expect("an extension").kind),
        "the first extension is GREASE"
    );
    assert!(
        is_grease(all.last().expect("an extension").kind),
        "the last extension is GREASE"
    );
    assert!(
        hello.has_extension(ext::SERVER_NAME),
        "SNI is always present"
    );
}

#[tokio::test]
async fn the_http2_preface_matches_the_profiles_akamai_string() {
    let server = TlsServer::start(TlsMode::CaptureH2Preface).await;
    let runtime = NetRuntime::new(1).expect("io runtime");
    let profile = ClientProfile::builtin("chrome_144_macos").expect("profile");
    let client = profile_client(&runtime, "chrome_144_macos", vec![server.cert_pem.clone()]);

    let _ = client.get(&server.url("/index.html")).await;
    common::wait_for(|| server.capture.h2_preface().is_some()).await;
    assert_eq!(
        server.capture.alpn().as_deref(),
        Some(b"h2".as_slice()),
        "ALPN must have selected h2"
    );

    let bytes = server.capture.h2_preface().expect("a preface");
    let preface = parse_h2_preface(&bytes).expect("parse preface");
    assert_eq!(
        akamai_h2(&preface),
        profile.meta.akamai_h2,
        "the Akamai HTTP/2 fingerprint must be what the profile promises"
    );
    assert_eq!(preface.settings, profile.h2.settings);
    assert_eq!(preface.window_update, profile.h2.window_update);
    assert!(
        preface.priority_frames.is_empty(),
        "Chrome 144 sends no standalone PRIORITY frames"
    );

    let headers = preface.first_headers.expect("a HEADERS frame");
    assert_eq!(headers.pseudo_header_order, "m,a,s,p");
    let priority = headers.priority.expect("HEADERS carries PRIORITY");
    let expected = profile.h2.headers_priority.expect("profile priority");
    assert_eq!(priority.exclusive, expected.exclusive);
    assert_eq!(priority.dependency, expected.dep);
    assert_eq!(
        priority.weight,
        u16::from(expected.weight) + 1,
        "the wire carries the weight minus one"
    );
    assert_eq!(
        headers.header(":authority"),
        Some(format!("localhost:{}", server.port()).as_str())
    );
}

/// The header names of one request the capture recorded from real Chrome.
fn captured_h2_header_names(path: &str) -> Vec<String> {
    let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../profiles/captures/chrome_144_macos/h2_requests.json");
    let text = std::fs::read_to_string(&file)
        .unwrap_or_else(|err| panic!("reading {}: {err}", file.display()));
    let requests: serde_json::Value = serde_json::from_str(&text).expect("capture is JSON");
    let request = requests
        .as_array()
        .expect("array of requests")
        .iter()
        .find(|request| {
            request["headers"]
                .as_array()
                .expect("headers")
                .iter()
                .any(|field| field[0] == ":path" && field[1] == path)
        })
        .unwrap_or_else(|| panic!("the capture has no request for {path}"));
    request["headers"]
        .as_array()
        .expect("headers")
        .iter()
        .map(|field| field[0].as_str().expect("header name").to_owned())
        .filter(|name| !name.starts_with(':'))
        .collect()
}

async fn h2_header_names(client: &Client, server: &TlsServer, request: Request) -> Vec<String> {
    let _ = client.send(request).await;
    common::wait_for(|| server.capture.h2_preface().is_some()).await;
    let preface =
        parse_h2_preface(&server.capture.h2_preface().expect("preface")).expect("parse preface");
    preface.first_headers.expect("HEADERS").header_order()
}

#[tokio::test]
async fn an_http2_navigation_sends_exactly_the_headers_chrome_sends() {
    let server = TlsServer::start(TlsMode::CaptureH2Preface).await;
    let runtime = NetRuntime::new(1).expect("io runtime");
    let client = profile_client(&runtime, "chrome_144_macos", vec![server.cert_pem.clone()]);

    // The capture's `/`: a user-typed navigation, no cookie, no referrer.
    let request = Request::get(&server.url("/"))
        .expect("request")
        .resource_type(ResourceType::Document)
        .user_activated(true);
    let names = h2_header_names(&client, &server, request).await;

    assert_eq!(
        names,
        captured_h2_header_names("/"),
        "our HTTP/2 navigation header list is not Chrome's"
    );
    assert!(
        names
            .iter()
            .all(|name| name.chars().all(|c| !c.is_ascii_uppercase())),
        "HTTP/2 names are lower-case: {names:?}"
    );
    assert!(
        !names
            .iter()
            .any(|name| name == "host" || name == "connection"),
        "HTTP/2 drops Host and Connection: {names:?}"
    );
}

#[tokio::test]
async fn an_http2_subresource_sends_exactly_the_headers_chrome_sends() {
    let server = TlsServer::start(TlsMode::CaptureH2Preface).await;
    let runtime = NetRuntime::new(1).expect("io runtime");
    let client = profile_client(&runtime, "chrome_144_macos", vec![server.cert_pem.clone()]);

    // The capture's `/sub.css`: a same-origin stylesheet with a referrer and
    // a cookie, which is where Chrome's other header order was measured.
    let page = url::Url::parse(&server.url("/")).expect("page url");
    let request = Request::get(&server.url("/sub.css"))
        .expect("request")
        .resource_type(ResourceType::Stylesheet)
        .initiator(page.clone())
        .referrer(page)
        .header("Cookie", "fp=1")
        .expect("header");
    let names = h2_header_names(&client, &server, request).await;

    assert_eq!(
        names,
        captured_h2_header_names("/sub.css"),
        "our HTTP/2 sub-resource header list is not Chrome's"
    );
}

#[tokio::test]
async fn the_priority_header_carries_chromes_urgency_per_resource_type() {
    // Values measured from the capture; the header is HTTP/2 only.
    let profile = ClientProfile::builtin("chrome_144_macos").expect("profile");
    for (path, resource_type) in [
        ("/", ResourceType::Document),
        ("/sub.css", ResourceType::Stylesheet),
        ("/sub.js", ResourceType::BlockingScript),
        ("/sub.png", ResourceType::Image),
        ("/api", ResourceType::Fetch),
        ("/sub.woff2", ResourceType::Font),
    ] {
        let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../profiles/captures/chrome_144_macos/h2_requests.json");
        let text = std::fs::read_to_string(&file).expect("capture");
        let requests: serde_json::Value = serde_json::from_str(&text).expect("JSON");
        let captured = requests
            .as_array()
            .expect("array")
            .iter()
            .find(|request| {
                request["headers"]
                    .as_array()
                    .expect("headers")
                    .iter()
                    .any(|field| field[0] == ":path" && field[1] == path)
            })
            .and_then(|request| {
                request["headers"]
                    .as_array()
                    .expect("headers")
                    .iter()
                    .find(|field| field[0] == "priority")
                    .and_then(|field| field[1].as_str())
            })
            .unwrap_or_else(|| panic!("no priority header captured for {path}"));
        assert_eq!(
            profile.headers.priority.for_resource(resource_type),
            captured,
            "priority for {path}"
        );
    }
}

#[tokio::test]
async fn an_untrusted_certificate_fails_unless_the_caller_asks_otherwise() {
    let server = TlsServer::start(TlsMode::ServeH2).await;
    let runtime = NetRuntime::new(1).expect("io runtime");

    // No extra root: the throw-away certificate is not trusted.
    let strict = profile_client(&runtime, "chrome_144_macos", Vec::new());
    let err = strict
        .get(&server.url("/"))
        .await
        .expect_err("an unknown issuer must fail");
    assert!(
        matches!(
            err.name,
            ErrorName::CertAuthorityInvalid | ErrorName::CertCommonNameInvalid
        ),
        "unexpected error: {err:?}"
    );

    let lenient = runtime
        .client(ClientConfig {
            profile: Profile::Browser(Arc::new(
                ClientProfile::builtin("chrome_144_macos").expect("profile"),
            )),
            tls: TlsOptions {
                ignore_https_errors: true,
                ..TlsOptions::default()
            },
            destination: DestinationPolicy::permissive(),
            resolver: Some(loopback_resolver()),
            ..ClientConfig::default()
        })
        .expect("client");
    let response = lenient.get(&server.url("/")).await.expect("response");
    assert_eq!(response.status, 200);
}

#[tokio::test]
async fn a_reset_during_the_handshake_is_a_connection_error_not_a_certificate_error() {
    // `SSL_get_verify_result` is not `X509_V_OK` on a connection that died
    // before verification ran; reporting that as ERR_CERT_INVALID hides the
    // retryable class the caller needs.
    let server = tenta_testutil::HandshakeFaultServer::start(tenta_testutil::HandshakeFault::Reset)
        .await
        .expect("fault server");
    let runtime = NetRuntime::new(1).expect("io runtime");
    let client = profile_client(&runtime, "chrome_144_macos", Vec::new());

    let err = client.get(&server.url("/")).await.expect_err("must fail");
    assert!(
        matches!(
            err.name,
            ErrorName::ConnectionReset | ErrorName::ConnectionClosed | ErrorName::ConnectionAborted
        ),
        "a reset handshake must report a connection error, got {}",
        err.error_text()
    );
    assert!(
        !err.error_text().contains("CERT"),
        "not a certificate error: {}",
        err.error_text()
    );
}

#[tokio::test]
async fn a_clean_close_during_the_handshake_is_reported_as_connection_closed() {
    let server = tenta_testutil::HandshakeFaultServer::start(tenta_testutil::HandshakeFault::Close)
        .await
        .expect("fault server");
    let runtime = NetRuntime::new(1).expect("io runtime");
    let client = profile_client(&runtime, "chrome_144_macos", Vec::new());

    let err = client.get(&server.url("/")).await.expect_err("must fail");
    assert!(
        matches!(
            err.name,
            ErrorName::ConnectionClosed | ErrorName::ConnectionReset | ErrorName::EmptyResponse
        ),
        "a half-closed handshake must report a connection error, got {}",
        err.error_text()
    );
    assert!(!err.error_text().contains("CERT"), "{}", err.error_text());
}

#[tokio::test]
async fn an_ip_literal_destination_sends_no_server_name_extension() {
    // RFC 6066 §3: no SNI for an IP literal, which is also what flips JA4's
    // first field from `d` to `i`.
    let server = TlsServer::start(TlsMode::HelloOnly).await;
    let runtime = NetRuntime::new(1).expect("io runtime");
    let client = profile_client(&runtime, "chrome_144_macos", vec![server.cert_pem.clone()]);

    let _ = client
        .get(&format!("https://127.0.0.1:{}/", server.port()))
        .await;
    common::wait_for(|| server.capture.client_hello().is_some()).await;
    let hello = parse_client_hello(&server.capture.client_hello().expect("hello")).expect("parse");
    assert_eq!(hello.server_name(), None, "an IP literal carries no SNI");
    assert!(
        !hello.has_extension(ext::SERVER_NAME),
        "the server_name extension is absent entirely"
    );
    assert_eq!(
        ja4(&hello).full.chars().nth(3),
        Some('i'),
        "JA4 reports an IP destination"
    );
}

/// A profile client for `http://fp-capture.test:<port>`, an origin that is
/// neither loopback by name nor secure, resolved to loopback.
fn insecure_origin_client(runtime: &Arc<NetRuntime>) -> Client {
    let resolver = Resolver::literals_only(SessionCache::ephemeral());
    resolver.remember(
        "fp-capture.test",
        &[IpAddr::V4(Ipv4Addr::LOCALHOST)],
        Duration::from_secs(60),
    );
    runtime
        .client(ClientConfig {
            profile: Profile::Browser(Arc::new(
                ClientProfile::builtin("chrome_144_macos").expect("profile"),
            )),
            destination: DestinationPolicy::permissive(),
            resolver: Some(Arc::new(resolver)),
            request_timeout: Some(Duration::from_secs(10)),
            ..ClientConfig::default()
        })
        .expect("client")
}

/// One request head as real Chrome 144 sent it, with the capture's authority
/// replaced by this test's. The capture ran against `fp-capture.test` on an
/// ephemeral port, and the authority appears in `Host` and inside `Referer`.
fn captured_http1_head(file: &str, authority: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../profiles/captures/chrome_144_macos")
        .join(file);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("reading {}: {err}", path.display()));
    let captured = text
        .lines()
        .find_map(|line| line.strip_prefix("Host: "))
        .expect("the capture has a Host header")
        .trim()
        .to_owned();
    text.replace(&captured, authority)
}

/// The request head the server saw, as text.
fn head_text(head: &[u8]) -> String {
    String::from_utf8(head.to_vec()).expect("the head is ASCII")
}

#[tokio::test]
async fn an_https_http1_navigation_is_byte_equal_to_the_chrome_capture() {
    // ALPN offers only http/1.1, so this exercises the hyper path including
    // the header casing and order `h1_case` restores.
    let server = TlsServer::start(TlsMode::ServeHttp1).await;
    let runtime = NetRuntime::new(1).expect("io runtime");
    let client = profile_client(&runtime, "chrome_144_macos", vec![server.cert_pem.clone()]);

    let request = Request::get(&server.url("/"))
        .expect("request")
        .resource_type(ResourceType::Document)
        .user_activated(true);
    let response = client.send(request).await.expect("response");
    assert_eq!(response.status, 200);

    let heads = server.capture.http1_heads();
    let head = head_text(heads.first().expect("a recorded head"));
    let authority = format!("localhost:{}", server.port());
    assert_eq!(
        head,
        captured_http1_head("https_http1_request.txt", &authority),
        "the HTTPS/1.1 request head is not byte-equal to Chrome's"
    );
}

#[tokio::test]
async fn an_https_http1_subresource_is_byte_equal_to_the_chrome_capture() {
    let server = TlsServer::start(TlsMode::ServeHttp1).await;
    let runtime = NetRuntime::new(1).expect("io runtime");
    let client = profile_client(&runtime, "chrome_144_macos", vec![server.cert_pem.clone()]);

    let page = url::Url::parse(&server.url("/")).expect("page url");
    let request = Request::get(&server.url("/sub.css"))
        .expect("request")
        .resource_type(ResourceType::Stylesheet)
        .initiator(page.clone())
        .referrer(page)
        .header("Cookie", "fp=1")
        .expect("header");
    let response = client.send(request).await.expect("response");
    assert_eq!(response.status, 200);

    let heads = server.capture.http1_heads();
    let head = head_text(heads.first().expect("a recorded head"));
    let authority = format!("localhost:{}", server.port());
    assert_eq!(
        head,
        captured_http1_head("https_http1_subresource.txt", &authority),
        "the HTTPS/1.1 sub-resource head is not byte-equal to Chrome's"
    );
}

#[tokio::test]
async fn a_plaintext_http1_navigation_is_byte_equal_to_the_chrome_capture() {
    // An insecure, non-loopback origin: no client hints, no `Sec-Fetch-*`,
    // and `Accept-Encoding` without `br`/`zstd`. The name resolves to
    // loopback, but trustworthiness is decided on the URL, not the address.
    let server = common::TestServer::start().await;
    let runtime = NetRuntime::new(1).expect("io runtime");
    let client = insecure_origin_client(&runtime);
    let authority = format!("fp-capture.test:{}", server.port());
    let request = Request::get(&format!("http://{authority}/hello"))
        .expect("request")
        .resource_type(ResourceType::Document)
        .user_activated(true);
    let response = client.send(request).await.expect("response");
    assert_eq!(response.status, 200);

    // The capture's path is `/`; this server answers on `/hello`.
    let head = format!("{}\r\n\r\n", server.state.last().head);
    let expected = captured_http1_head("http1_request.txt", &authority)
        .replace("GET / HTTP", "GET /hello HTTP");
    assert_eq!(
        head, expected,
        "the plaintext HTTP/1.1 request head is not byte-equal to Chrome's"
    );
}

#[tokio::test]
async fn a_request_body_survives_the_http1_head_rewrite() {
    // The head substitution must not touch a single byte of the body, even
    // when hyper hands head and body to the socket in one write.
    let server = common::TestServer::start().await;
    let runtime = NetRuntime::new(1).expect("io runtime");
    let client = insecure_origin_client(&runtime);
    let authority = format!("fp-capture.test:{}", server.port());

    let payload = b"GET / HTTP/1.1\r\nHost: not-a-real-head\r\n\r\n".repeat(4);
    let request = Request::post(&format!("http://{authority}/echo"), payload.clone())
        .expect("request")
        .header("Content-Type", "application/octet-stream")
        .expect("header");
    let response = client.send(request).await.expect("response");
    assert_eq!(response.status, 200);
    let echoed = response.body.collect().await.expect("body");
    assert_eq!(
        echoed.as_ref(),
        payload.as_slice(),
        "the echoed body is not what was sent"
    );

    let recorded = server.state.last();
    assert_eq!(
        recorded.body, payload,
        "the server received the body intact"
    );
    assert_eq!(
        recorded.request_line(),
        "POST /echo HTTP/1.1",
        "only the first head was rewritten"
    );
}

#[tokio::test]
async fn a_streaming_body_is_chunked_in_the_profiles_own_head() {
    // hyper inserts `Transfer-Encoding: chunked` for a body of unknown length;
    // the transport says it first, so the head it writes is the head we
    // predicted and the header appears exactly once.
    let server = common::TestServer::start().await;
    let runtime = NetRuntime::new(1).expect("io runtime");
    let client = insecure_origin_client(&runtime);
    let authority = format!("fp-capture.test:{}", server.port());

    let (sender, body) = tenta_net::Body::channel(None);
    let request = Request::post(&format!("http://{authority}/echo"), body)
        .expect("request")
        .header("Content-Type", "text/plain")
        .expect("header");
    let sending = tokio::spawn(async move {
        sender
            .send(Ok(bytes::Bytes::from_static(b"one, ")))
            .await
            .expect("first chunk");
        sender
            .send(Ok(bytes::Bytes::from_static(b"two")))
            .await
            .expect("second chunk");
    });
    let response = client.send(request).await.expect("response");
    sending.await.expect("sender");
    assert_eq!(response.status, 200);
    let echoed = response.body.collect().await.expect("body");
    assert_eq!(echoed.as_ref(), b"one, two");

    let recorded = server.state.last();
    let names = recorded.header_names();
    assert_eq!(
        names
            .iter()
            .filter(|name| name.eq_ignore_ascii_case("transfer-encoding"))
            .count(),
        1,
        "chunked is declared exactly once: {names:?}"
    );
    assert!(
        !names
            .iter()
            .any(|name| name.eq_ignore_ascii_case("content-length")),
        "a chunked request has no content-length: {names:?}"
    );
}

#[tokio::test]
async fn a_client_without_http2_offers_only_http1_in_alpn() {
    // The server would take `h2`; a client that cannot speak it must not
    // offer it, or the ALPN list gives away that the profile is a costume.
    let server = TlsServer::start(TlsMode::ServeH2).await;
    let runtime = NetRuntime::new(1).expect("io runtime");
    let client = runtime
        .client(ClientConfig {
            profile: Profile::Browser(Arc::new(
                ClientProfile::builtin("chrome_144_macos").expect("profile"),
            )),
            tls: TlsOptions {
                extra_root_cas: vec![server.cert_pem.clone()],
                ..TlsOptions::default()
            },
            destination: DestinationPolicy::permissive(),
            resolver: Some(loopback_resolver()),
            allow_h2: false,
            request_timeout: Some(Duration::from_secs(10)),
            ..ClientConfig::default()
        })
        .expect("client");

    let response = client.get(&server.url("/")).await.expect("response");
    assert_eq!(response.protocol, tenta_net::client::Protocol::Http11);
    common::wait_for(|| server.capture.client_hello().is_some()).await;

    let hello = parse_client_hello(&server.capture.client_hello().expect("hello")).expect("parse");
    let alpn = hello
        .extensions
        .iter()
        .find(|extension| extension.kind == ext::ALPN)
        .expect("the ClientHello offers ALPN");
    let offered = alpn_protocols(&alpn.body);
    assert_eq!(
        offered,
        vec!["http/1.1".to_owned()],
        "only the protocol the client can speak is offered"
    );
    assert_eq!(
        server.capture.alpn().as_deref(),
        Some(b"http/1.1".as_slice())
    );
}

/// The protocol names inside an ALPN extension body.
fn alpn_protocols(body: &[u8]) -> Vec<String> {
    let mut names = Vec::new();
    // `ProtocolNameList`: a 16-bit length, then length-prefixed names.
    let mut offset = 2;
    while offset < body.len() {
        let length = usize::from(body[offset]);
        offset += 1;
        if offset + length > body.len() {
            break;
        }
        names.push(String::from_utf8_lossy(&body[offset..offset + length]).into_owned());
        offset += length;
    }
    names
}

#[tokio::test]
async fn a_resumption_hello_keeps_the_full_handshakes_shape() {
    // A resumed handshake is the one a long-lived browser sends most often.
    // It has to look like the first one — same ciphers, same extension set
    // plus the PSK pair, GREASE still in place — or resumption itself becomes
    // the tell (`docs/spec/03-network.md` §4).
    let server = TlsServer::start(TlsMode::ServeH2).await;
    let dir = tenta_testutil::temp_dir("resumption-shape");
    let key = tenta_net::policy::session_cache::SessionKey::new("chrome_144_macos", "shape");
    let session = SessionCache::open(dir.path(), &key).expect("session cache");
    let runtime = NetRuntime::new(1).expect("io runtime");

    for path in ["/first", "/second"] {
        let resolver = Resolver::literals_only(session.clone());
        resolver.remember(
            "localhost",
            &[IpAddr::V4(Ipv4Addr::LOCALHOST)],
            Duration::from_secs(60),
        );
        let client = runtime
            .client(ClientConfig {
                profile: Profile::Browser(Arc::new(
                    ClientProfile::builtin("chrome_144_macos").expect("profile"),
                )),
                tls: TlsOptions {
                    extra_root_cas: vec![server.cert_pem.clone()],
                    ..TlsOptions::default()
                },
                destination: DestinationPolicy::permissive(),
                resolver: Some(Arc::new(resolver)),
                session_cache: session.clone(),
                request_timeout: Some(Duration::from_secs(10)),
                ..ClientConfig::default()
            })
            .expect("client");
        client
            .get(&server.url(path))
            .await
            .expect("response")
            .bytes()
            .await
            .expect("body");
        // The TLS 1.3 ticket arrives after the handshake.
        tokio::time::sleep(Duration::from_millis(100)).await;
        client.close();
    }

    common::wait_for(|| server.capture.resumptions().len() >= 2).await;
    let resumptions = server.capture.resumptions();
    assert!(!resumptions[0], "the first handshake had nothing to resume");
    assert!(
        resumptions[1],
        "the second handshake resumed: {resumptions:?}"
    );

    let hellos = server.capture.client_hellos();
    assert!(hellos.len() >= 2, "both ClientHellos were captured");
    let full = parse_client_hello(&hellos[0]).expect("first hello");
    let resumed = parse_client_hello(&hellos[1]).expect("second hello");

    // The GREASE values themselves are drawn per connection, as they are in
    // Chrome; what has to match is everything else and where GREASE sits.
    let real_ciphers = |hello: &ClientHello| -> Vec<u16> {
        hello
            .cipher_suites
            .iter()
            .copied()
            .filter(|suite| !is_grease(*suite))
            .collect()
    };
    let grease_positions = |hello: &ClientHello| -> Vec<usize> {
        hello
            .cipher_suites
            .iter()
            .enumerate()
            .filter(|(_, suite)| is_grease(**suite))
            .map(|(index, _)| index)
            .collect()
    };
    assert_eq!(
        real_ciphers(&resumed),
        real_ciphers(&full),
        "a resumption offers the same ciphers"
    );
    assert_eq!(
        grease_positions(&resumed),
        grease_positions(&full),
        "GREASE sits in the same place in both hellos"
    );
    // JA4_a counts extensions and JA4_c hashes them, so the added
    // `pre_shared_key` moves both by exactly one extension; everything the
    // profile controls — the cipher list and its hash — is untouched.
    let full_ja4 = ja4(&full);
    let resumed_ja4 = ja4(&resumed);
    assert_eq!(
        resumed_ja4.b, full_ja4.b,
        "the cipher hash of a resumed handshake is the profile's"
    );
    assert_eq!(
        resumed_ja4.raw_b, full_ja4.raw_b,
        "the cipher list of a resumed handshake is the profile's"
    );
    assert_eq!(
        resumed_ja4.a.len(),
        full_ja4.a.len(),
        "only the extension count changes: {} against {}",
        resumed_ja4.a,
        full_ja4.a
    );
    assert_eq!(
        &resumed_ja4.a[..7],
        &full_ja4.a[..7],
        "protocol, version, SNI and cipher count are unchanged"
    );

    let kinds = |hello: &ClientHello| -> Vec<u16> {
        hello
            .extensions
            .iter()
            .map(|extension| extension.kind)
            .filter(|kind| !is_grease(*kind))
            .collect()
    };
    let mut full_kinds = kinds(&full);
    let mut resumed_kinds = kinds(&resumed);
    assert!(
        resumed_kinds.contains(&ext::PRE_SHARED_KEY),
        "the resumption carries a pre_shared_key: {resumed_kinds:?}"
    );
    assert!(
        resumed_kinds.contains(&ext::PSK_KEY_EXCHANGE_MODES),
        "the resumption carries psk_key_exchange_modes: {resumed_kinds:?}"
    );
    assert_eq!(
        resumed_kinds.last(),
        Some(&ext::PRE_SHARED_KEY),
        "RFC 8446 §4.2.11: pre_shared_key is the last extension"
    );
    assert!(
        full.extensions
            .iter()
            .any(|extension| is_grease(extension.kind))
            && resumed
                .extensions
                .iter()
                .any(|extension| is_grease(extension.kind)),
        "both hellos still carry GREASE"
    );

    // Apart from the PSK pair the extension sets are the same.
    full_kinds.sort_unstable();
    resumed_kinds.retain(|kind| *kind != ext::PRE_SHARED_KEY);
    resumed_kinds.sort_unstable();
    let mut expected = full_kinds.clone();
    if !expected.contains(&ext::PSK_KEY_EXCHANGE_MODES) {
        expected.push(ext::PSK_KEY_EXCHANGE_MODES);
        expected.sort_unstable();
    }
    assert_eq!(
        resumed_kinds, expected,
        "the resumption offers the same extensions as the full handshake"
    );
}
