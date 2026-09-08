//! Behavioural tests for the loopback fixture server. Every timing assertion
//! is a lower bound derived from the configured delay (a simulated latency
//! must at least be waited out) or a generous upper bound on a loopback
//! request; nothing sleeps for its own sake.

use std::time::Duration;

use tenta_testutil::client::{get, request};
use tenta_testutil::{FixtureServer, FixtureServerConfig, fixtures_root, temp_dir};

async fn start(config: FixtureServerConfig) -> FixtureServer {
    FixtureServer::start(config).await.expect("server starts")
}

#[tokio::test]
async fn serves_a_repository_fixture_with_its_content_type() {
    let server = start(FixtureServerConfig::new(fixtures_root())).await;
    let response = get(server.addr(), "/tiny/index.html").await.unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(
        response.header("content-type"),
        Some("text/html; charset=utf-8")
    );
    assert!(response.body_text().contains("<h1>Tiny fixture</h1>"));
    assert_eq!(server.requests_served(), 1);
}

#[tokio::test]
async fn directory_paths_serve_index_html_and_extensions_map_types() {
    let root = temp_dir("fixtures");
    root.write("site/index.html", "<p>index</p>");
    root.write("site/style.css", "p{}");
    root.write("site/img/a.png", [0x89, b'P', b'N', b'G']);
    root.write("site/app.mjs", "export {}");
    root.write("site/blob.bin", "x");
    let server = start(FixtureServerConfig::new(root.path())).await;

    let index = get(server.addr(), "/site/").await.unwrap();
    assert_eq!(index.status, 200);
    assert_eq!(index.body_text(), "<p>index</p>");

    for (path, expected) in [
        ("/site/style.css", "text/css; charset=utf-8"),
        ("/site/img/a.png", "image/png"),
        ("/site/app.mjs", "text/javascript; charset=utf-8"),
        ("/site/blob.bin", "application/octet-stream"),
    ] {
        let response = get(server.addr(), path).await.unwrap();
        assert_eq!(response.status, 200, "{path}");
        assert_eq!(response.header("content-type"), Some(expected), "{path}");
    }
}

#[tokio::test]
async fn missing_files_are_404_and_traversal_is_refused() {
    let root = temp_dir("fixtures");
    root.write("public/index.html", "ok");
    root.write("secret.txt", "do not serve");
    let served_root = root.path().join("public");
    let server = start(FixtureServerConfig::new(&served_root)).await;

    assert_eq!(get(server.addr(), "/nope.html").await.unwrap().status, 404);
    assert_eq!(get(server.addr(), "/api/nope").await.unwrap().status, 404);
    for path in [
        "/../secret.txt",
        "/%2e%2e/secret.txt",
        "/a/../../secret.txt",
    ] {
        let response = get(server.addr(), path).await.unwrap();
        assert_ne!(response.status, 200, "{path} must not be served");
        assert!(
            !response.body_text().contains("do not serve"),
            "{path} leaked the file"
        );
    }
}

#[tokio::test]
async fn echo_returns_method_query_headers_and_parsed_json() {
    let server = start(FixtureServerConfig::new(fixtures_root())).await;
    let response = request(
        server.addr(),
        "POST",
        "/api/echo?item=alpha&n=2",
        &[("Content-Type", "application/json"), ("X-Test", "yes")],
        br#"{"a":1,"b":[true]}"#,
    )
    .await
    .unwrap()
    .finish()
    .await
    .unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.header("content-type"), Some("application/json"));
    let echoed: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
    assert_eq!(echoed["method"], "POST");
    assert_eq!(echoed["path"], "/api/echo");
    assert_eq!(echoed["query"], "item=alpha&n=2");
    assert_eq!(echoed["headers"]["x-test"], "yes");
    assert_eq!(echoed["json"]["a"], 1);
    assert_eq!(echoed["json"]["b"][0], true);
    assert_eq!(echoed["body"], r#"{"a":1,"b":[true]}"#);
}

#[tokio::test]
async fn slow_endpoint_waits_the_requested_time() {
    let server = start(FixtureServerConfig::new(fixtures_root())).await;
    let response = get(server.addr(), "/api/slow?ms=120").await.unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.body_text(), r#"{"slept_ms":120}"#);
    let elapsed = response.body_at - response.sent_at;
    assert!(
        elapsed >= Duration::from_millis(108),
        "slow endpoint answered after {elapsed:?}, expected >= ~120 ms"
    );
}

#[tokio::test]
async fn never_endpoint_does_not_answer() {
    let server = start(FixtureServerConfig::new(fixtures_root())).await;
    let outcome =
        tokio::time::timeout(Duration::from_millis(300), get(server.addr(), "/api/never")).await;
    assert!(
        outcome.is_err(),
        "/api/never must not respond, got {outcome:?}"
    );
    // The server is still healthy for other requests.
    assert_eq!(
        get(server.addr(), "/tiny/index.html").await.unwrap().status,
        200
    );
}

#[tokio::test]
async fn sse_streams_events_every_interval() {
    let server = start(FixtureServerConfig::new(fixtures_root())).await;
    let mut streaming = request(server.addr(), "GET", "/api/sse", &[], &[])
        .await
        .unwrap();
    assert_eq!(streaming.status, 200);
    assert!(
        streaming
            .headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("content-type") && v == "text/event-stream")
    );
    let mut events = 0;
    let mut seen = 0;
    while events < 3 {
        let body = streaming.read_body_until(seen + 1).await.unwrap();
        seen = body.len();
        events = String::from_utf8_lossy(body).matches("\n\n").count();
    }
    let elapsed = streaming.head_at.elapsed();
    let body = String::from_utf8_lossy(&streaming.buffered);
    assert!(
        body.contains("data: {\"n\":1,"),
        "first event missing: {body}"
    );
    assert!(
        body.contains("data: {\"n\":3,"),
        "third event missing: {body}"
    );
    assert!(
        elapsed >= Duration::from_millis(250),
        "three events at 100 ms spacing arrived after only {elapsed:?}"
    );
}

#[tokio::test]
async fn rtt_is_split_between_head_and_body() {
    let rtt = Duration::from_millis(200);
    let server = start(FixtureServerConfig::new(fixtures_root()).rtt(rtt)).await;
    let response = get(server.addr(), "/tiny/index.html").await.unwrap();
    assert_eq!(response.status, 200);
    let to_head = response.head_at - response.sent_at;
    let to_body = response.body_at - response.sent_at;
    let head_to_body = response.body_at - response.head_at;
    assert!(
        to_head >= Duration::from_millis(80),
        "head arrived after {to_head:?}, expected >= ~100 ms"
    );
    assert!(
        head_to_body >= Duration::from_millis(60),
        "body followed the head after only {head_to_body:?}, expected >= ~100 ms"
    );
    assert!(
        to_body >= Duration::from_millis(180),
        "whole response took {to_body:?}, expected >= ~200 ms"
    );
}

#[tokio::test]
async fn zero_rtt_is_fast_and_delay_path_only_affects_matches() {
    let root = temp_dir("fixtures");
    root.write("slow/a.js", "1");
    root.write("fast.js", "2");
    let server = start(
        FixtureServerConfig::new(root.path()).delay_path("/slow/*.js", Duration::from_millis(150)),
    )
    .await;

    let fast = get(server.addr(), "/fast.js").await.unwrap();
    let slow = get(server.addr(), "/slow/a.js").await.unwrap();
    assert_eq!((fast.status, slow.status), (200, 200));
    let fast_elapsed = fast.body_at - fast.sent_at;
    let slow_elapsed = slow.body_at - slow.sent_at;
    assert!(
        slow_elapsed >= Duration::from_millis(135),
        "delayed path answered after {slow_elapsed:?}, expected >= ~150 ms"
    );
    assert!(
        fast_elapsed < Duration::from_millis(100),
        "undelayed loopback request took {fast_elapsed:?}"
    );
}

#[tokio::test]
async fn dropping_the_server_frees_the_port() {
    let server = start(FixtureServerConfig::new(fixtures_root())).await;
    let addr = server.addr();
    drop(server);
    // Rebinding the same port must succeed once the accept loop is gone.
    let rebound = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(listener) = tokio::net::TcpListener::bind(addr).await {
                return listener;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(rebound.is_ok(), "port {addr} still held after shutdown");
}

#[tokio::test]
async fn status_endpoint_answers_with_the_code_and_retry_after_it_was_given() {
    let server = start(FixtureServerConfig::new(fixtures_root())).await;

    let response = get(server.addr(), "/api/status?code=503&retry_after=1")
        .await
        .unwrap();
    assert_eq!(response.status, 503);
    assert_eq!(response.header("retry-after"), Some("1"));
    assert!(response.body_text().contains("HTTP 503"));

    // An HTTP-date is the other form RFC 9110 §10.2.3 allows.
    let dated = get(
        server.addr(),
        "/api/status?code=429&retry_after=Wed%2C%2021%20Oct%202026%2007%3A28%3A00%20GMT",
    )
    .await
    .unwrap();
    assert_eq!(dated.status, 429);
    assert_eq!(
        dated.header("retry-after"),
        Some("Wed, 21 Oct 2026 07:28:00 GMT")
    );

    // No parameters at all is a plain 200 with no Retry-After.
    let plain = get(server.addr(), "/api/status").await.unwrap();
    assert_eq!(plain.status, 200);
    assert_eq!(plain.header("retry-after"), None);
}
