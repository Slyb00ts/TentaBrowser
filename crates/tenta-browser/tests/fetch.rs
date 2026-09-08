//! Tier 1 fetches against the loopback fixture server
//! (`docs/spec/15-testing-and-benchmarks.md` §1).

mod support;

use std::time::Duration;

use tenta_browser::{
    ContextOptions, Engine, Goto, PageEvent, PageOptions, PreloadKind, Settle, SettleState,
};
use tenta_core::{BudgetKind, Budgets, Error};
use tenta_page::error_page::ERROR_URL;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_tiny_fixture_extracts_its_expected_text() {
    let server = support::server().await;
    let engine = support::engine(1);
    let context = support::context(&engine);

    let document = context
        .fetch(&server.url("/tiny/index.html"))
        .await
        .expect("document");

    let expected = std::fs::read_to_string(support::fixture_dir("tiny").join("expected.txt"))
        .expect("expected text");
    assert_eq!(document.text(), expected);
    assert_eq!(document.status(), 200);
    assert!(document.ok());
    assert_eq!(document.encoding(), "UTF-8");
    assert_eq!(document.title().as_deref(), Some("Tiny"));
    assert_eq!(
        document.content_type(),
        Some("text/html; charset=utf-8"),
        "the response's content type is kept verbatim"
    );
    assert_eq!(
        document.final_url().as_str(),
        server.url("/tiny/index.html")
    );
    assert!(document.redirects().is_empty());
    assert!(document.html().starts_with("<html lang=\"en\">"));
    assert_eq!(server.requests_served(), 1);
    engine.close();
}

/// The 1 MB article fixture, compared with `expected.txt` the way
/// `crates/tenta-dom/tests/extract_fixture.rs` compares it: the two agree line
/// for line from the article heading on.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_static_article_text_matches_the_reference_output() {
    let server = support::server().await;
    let engine = support::engine(1);
    let context = support::context(&engine);

    let document = context
        .fetch(&server.url("/static-article/index.html"))
        .await
        .expect("document");
    let text = document.text();

    let expected =
        std::fs::read_to_string(support::fixture_dir("static-article").join("expected.txt"))
            .expect("expected text");
    let anchor = "The Long Read: A Field Guide to Headless Engines";
    let expected_body = expected.split_once(anchor).expect("anchor in expected").1;
    let actual_body = text.split_once(anchor).expect("anchor in extracted text").1;
    let expected_lines: Vec<&str> = expected_body.lines().collect();
    let actual_lines: Vec<&str> = actual_body.lines().collect();
    assert_eq!(
        expected_lines.len(),
        actual_lines.len(),
        "block count differs; first difference at {:?}",
        expected_lines
            .iter()
            .zip(actual_lines.iter())
            .position(|(a, b)| a != b)
    );
    assert_eq!(expected_lines, actual_lines);
    assert!(text.contains("END-OF-ARTICLE-7F3A"));

    let main = document.main_content().expect("the article has content");
    assert!(main.link_density < 0.2);
    assert_eq!(
        document.metadata().title.as_deref(),
        Some("The Long Read: A Field Guide to Headless Engines")
    );
    engine.close();
}

/// `<meta charset="windows-1250">` inside the prescan window decides the
/// encoding when the response carries none.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_meta_charset_decodes_a_windows_1250_page() {
    let server = support::server().await;
    let engine = support::engine(1);
    let context = support::context(&engine);

    let document = context
        .fetch(&server.url("/encoding-cp1250/index.html?content_type=text/html"))
        .await
        .expect("document");

    assert_eq!(document.encoding(), "windows-1250");
    let expected =
        std::fs::read_to_string(support::fixture_dir("encoding-cp1250").join("expected.txt"))
            .expect("expected text");
    assert_eq!(document.text(), expected);
    assert_eq!(document.title().as_deref(), Some("Zażółć gęślą jaźń"));
    engine.close();
}

/// A `charset` on the response wins over the document's own declaration
/// (WHATWG: the transport charset is `Certain`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_transport_charset_overrides_the_meta_declaration() {
    let server = support::server().await;
    let engine = support::engine(1);
    let context = support::context(&engine);

    // The file declares `<meta charset="utf-8">`; the response says otherwise.
    let document = context
        .fetch(&server.url("/tiny/index.html?content_type=text/html;charset=windows-1250"))
        .await
        .expect("document");

    assert_eq!(document.encoding(), "windows-1250");
    assert_eq!(
        document.content_type(),
        Some("text/html;charset=windows-1250")
    );
    // The fixture is ASCII, so the text is the same either way: what the test
    // asserts is which declaration decided.
    assert!(document.text().contains("Tiny fixture"));
    engine.close();
}

/// A 404 is a navigation that succeeded with a status, not a failed
/// navigation: Chromium replaces the document only when the network failed
/// (`docs/spec/20-platform-behaviour.md` §1), and every Playwright client
/// depends on `goto` resolving here.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_404_is_a_navigation_with_a_status_and_a_body() {
    let server = support::server().await;
    let engine = support::engine(1);
    let context = support::context(&engine);

    let document = context
        .fetch(&server.url("/no/such/page.html"))
        .await
        .expect("a 404 still produces a document");

    assert_eq!(document.status(), 404);
    assert!(!document.ok());
    assert!(document.text().contains("not found"));
    assert!(document.navigation().error.is_none());
    engine.close();
}

/// A refused connection is a failed navigation: the caller gets
/// `Error::Net` with Chromium's name and the page holds the error document.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_connection_fails_and_leaves_an_error_document() {
    let server = support::server().await;
    let port = server.port();
    // Take the port out of service: nothing listens on it once the server is
    // dropped, and the OS refuses the connection.
    server.shutdown();

    let engine = support::engine(1);
    let context = support::context(&engine);
    let page = context.new_page(PageOptions::tier1()).await.expect("page");
    let mut events = page.events();

    let url = format!("http://127.0.0.1:{port}/tiny/index.html");
    let error = page
        .goto(&url, Goto::default().timeout(Duration::from_secs(5)))
        .await
        .expect_err("the connection is refused");
    match &error {
        Error::Net { code, .. } => assert_eq!(code, "net::ERR_CONNECTION_REFUSED"),
        other => panic!("expected Error::Net, got {other:?}"),
    }
    assert!(error.retryable());

    let document = page.document().expect("the error document replaced it");
    assert_eq!(document.url().map(|url| url.as_str()), Some(ERROR_URL));
    assert!(document.text().contains("net::ERR_CONNECTION_REFUSED"));
    drop(document);
    assert_eq!(
        page.navigation().and_then(|nav| nav.error).as_deref(),
        Some("net::ERR_CONNECTION_REFUSED")
    );

    let seen = events.drain();
    assert!(
        seen.iter()
            .any(|event| matches!(event, PageEvent::RequestFailed { .. })),
        "the failure is reported as an event: {seen:?}"
    );
    page.close().await.expect("close");
    engine.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_body_over_the_budget_is_refused() {
    let server = support::server().await;
    let engine = Engine::builder()
        .page_threads(1)
        .budgets(Budgets::desktop())
        .build()
        .expect("engine");
    // Lower the response budget below the size of the 1 MB fixture.
    let mut budgets = Budgets::desktop();
    budgets.network_bytes = 32 * 1024;
    let context = engine
        .new_context(ContextOptions {
            allow_private_network: true,
            budgets: Some(budgets),
            ..ContextOptions::default()
        })
        .expect("context");

    let error = context
        .fetch(&server.url("/static-article/index.html"))
        .await
        .expect_err("the body is far over 32 KiB");
    match error {
        Error::BudgetExceeded { kind, component } => {
            assert_eq!(kind, BudgetKind::NetworkBytes);
            assert!(component.is_some());
        }
        other => panic!("expected BudgetExceeded, got {other:?}"),
    }
    engine.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_navigation_past_its_deadline_times_out() {
    let server = support::server().await;
    let engine = support::engine(1);
    let context = support::context(&engine);
    let page = context.new_page(PageOptions::tier1()).await.expect("page");

    // Pre-head timeout: deadline expires before the response head arrives.
    // The page should get an error document with ERR_TIMED_OUT.
    let error = page
        .goto(
            &server.url("/api/never"),
            Goto::default().timeout(Duration::from_millis(150)),
        )
        .await
        .expect_err("the endpoint never answers");
    match error {
        Error::Timeout { millis, .. } => assert_eq!(millis, 150),
        other => panic!("expected Timeout, got {other:?}"),
    }
    let doc = page.document().expect("error document installed");
    assert!(
        doc.text().contains("ERR_TIMED_OUT"),
        "pre-head timeout installs error document with ERR_TIMED_OUT"
    );
    drop(doc);
    let nav = page.navigation().expect("navigation recorded");
    assert_eq!(nav.error.as_deref(), Some("net::ERR_TIMED_OUT"));

    // Mid-body timeout: deadline expires while the response body is being read.
    // The page should have a partial document with settle: Timeout, and return error.
    let error = page
        .goto(
            &server.url("/api/sse"),
            Goto::default().timeout(Duration::from_millis(200)),
        )
        .await
        .expect_err("the sse endpoint never finishes");
    match error {
        Error::Timeout { millis, .. } => assert_eq!(millis, 200),
        other => panic!("expected Timeout, got {other:?}"),
    }
    let doc = page.document().expect("partial document installed");
    // SSE sends events with no HTML, so the partial document should be mostly empty
    let text = doc.text();
    assert!(
        text.is_empty() || text.contains("data:"),
        "mid-body timeout installs partial document"
    );
    drop(doc);
    let nav = page.navigation().expect("navigation recorded");
    assert!(matches!(nav.settle, SettleState::Timeout { .. }));
    assert_eq!(nav.error, None, "mid-body timeout is not a net error");

    page.close().await.expect("close");
    engine.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_redirect_chain_is_recorded_and_the_final_url_wins() {
    let server = support::server().await;
    let engine = support::engine(1);
    let context = support::context(&engine);

    let start = server.url("/api/redirect?to=/tiny/index.html&n=3");
    let document = context.fetch(&start).await.expect("document");

    assert_eq!(document.status(), 200);
    assert_eq!(document.url().as_str(), start, "the requested URL is kept");
    assert_eq!(
        document.final_url().as_str(),
        server.url("/tiny/index.html")
    );
    assert_eq!(
        document.redirects().len(),
        3,
        "three hops before the document: {:?}",
        document.redirects()
    );
    assert!(document.text().contains("Tiny fixture"));
    engine.close();
}

/// Tier 1 records every subresource hint and issues none
/// (`docs/spec/02-architecture.md` §1.2).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subresource_hints_are_recorded_and_never_fetched() {
    let server = support::server().await;
    let engine = support::engine(1);
    let context = support::context(&engine);

    let document = context
        .fetch(&server.url("/static-article/index.html"))
        .await
        .expect("document");

    let hints = document.recorded_hints();
    assert!(
        !hints.is_empty(),
        "the article has images and stylesheets to record"
    );
    assert!(
        hints.iter().all(|hint| hint.is_recorded_only()),
        "no hint may be issued at Tier 1"
    );
    assert!(
        hints.iter().any(|hint| hint.kind == PreloadKind::Image),
        "the article's images are among the hints"
    );
    assert_eq!(
        server.requests_served(),
        1,
        "the document is the only request that reached the server"
    );
    engine.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_navigation_reports_its_events_in_order() {
    let server = support::server().await;
    let engine = support::engine(1);
    let context = support::context(&engine);
    let page = context.new_page(PageOptions::tier1()).await.expect("page");
    let mut events = page.events();

    let navigation = page
        .goto(
            &server.url("/tiny/index.html"),
            Goto::default().wait_until(Settle::NetworkIdle),
        )
        .await
        .expect("navigation");
    assert_eq!(navigation.settle, SettleState::Settled { busy: false });
    assert_eq!(navigation.status, 200);
    assert!(navigation.bytes > 0);

    let seen = events.drain();
    let kinds: Vec<&str> = seen
        .iter()
        .map(|event| match event {
            PageEvent::Request { .. } => "request",
            PageEvent::Response { .. } => "response",
            PageEvent::RequestFailed { .. } => "failed",
            PageEvent::FrameNavigated { .. } => "navigated",
            PageEvent::Crashed { .. } => "crashed",
            PageEvent::TierChanged { .. } => "tier",
            _ => "other",
        })
        .collect();
    assert_eq!(
        kinds,
        ["request", "response", "navigated", "tier"],
        "events were {seen:?}"
    );
    page.close().await.expect("close");
    engine.close();
}

/// `Settle::Text` waits for content that is already there and reports the
/// settle state; a text that never appears ends in a `Timeout` outcome that
/// keeps the parsed document installed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_text_wait_settles_on_the_parsed_document() {
    let server = support::server().await;
    let engine = support::engine(1);
    let context = support::context(&engine);
    let page = context.new_page(PageOptions::tier1()).await.expect("page");

    let navigation = page
        .goto(
            &server.url("/tiny/index.html"),
            Goto::default().wait_until(Settle::Text("One paragraph".to_owned())),
        )
        .await
        .expect("the text is in the document");
    assert_eq!(navigation.settle, SettleState::Settled { busy: false });

    let navigation = page
        .goto(
            &server.url("/tiny/index.html"),
            Goto::default()
                .wait_until(Settle::Text("never appears".to_owned()))
                .timeout(Duration::from_millis(200)),
        )
        .await
        .expect("a settle deadline is an outcome, not a failure");
    assert!(
        matches!(navigation.settle, SettleState::Timeout { .. }),
        "{navigation:?}"
    );
    assert_eq!(
        navigation.error, None,
        "a settle timeout is not a net error"
    );
    let document = page
        .document()
        .expect("the parsed document stays installed");
    assert!(document.text().contains("One paragraph"));
    drop(document);
    page.close().await.expect("close");
    engine.close();
}

/// Per-hop events are emitted for each redirect in the chain (N-R3-6).
/// The event sequence should be: Request(initial) -> Response(hop1, 3xx) ->
/// Request(final) -> Response(final, 200) -> FrameNavigated -> TierChanged.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn redirect_emits_per_hop_events() {
    let server = support::server().await;
    let engine = support::engine(1);
    let context = support::context(&engine);
    let page = context.new_page(PageOptions::tier1()).await.expect("page");

    // Navigate to a URL that redirects to another URL
    let mut events = page.events();
    let redirect_url = server.url("/api/redirect?to=/tiny/index.html");
    let navigation = page
        .goto(&redirect_url, Goto::default())
        .await
        .expect("navigation completes");

    // Verify the navigation succeeded
    assert_eq!(navigation.status, 200);
    assert_eq!(
        navigation.final_url.as_str(),
        server.url("/tiny/index.html").as_str()
    );
    assert!(!navigation.redirects.is_empty(), "navigation has redirects");

    // Collect events and verify the per-hop sequence
    let mut collected_events = Vec::new();
    while let Some(event) = events.next().await {
        collected_events.push(event.clone());
        // Stop after TierChanged
        if matches!(event, PageEvent::TierChanged { .. }) {
            break;
        }
    }

    // Verify the event sequence:
    // 1. Request for the initial redirect URL
    // 2. Response for the redirect hop (3xx)
    // 3. Request for the final URL
    // 4. Response for the final URL (200)
    // 5. FrameNavigated
    // 6. TierChanged
    assert!(
        collected_events.len() >= 6,
        "expected at least 6 events (Request, Response(3xx), Request, Response(200), FrameNavigated, TierChanged), got {} events: {:#?}",
        collected_events.len(),
        collected_events
    );

    // Check first Request event for initial URL
    assert!(
        matches!(
            &collected_events[0],
            PageEvent::Request { url, .. } if url.path() == "/api/redirect"
        ),
        "first event should be Request for the initial URL, got: {:?}",
        collected_events[0]
    );

    // Check first Response event for redirect hop (should be 3xx status)
    assert!(
        matches!(
            &collected_events[1],
            PageEvent::Response { url, status, .. } if url.path() == "/api/redirect" && *status >= 300 && *status < 400
        ),
        "second event should be Response for the redirect hop with 3xx status, got: {:?}",
        collected_events[1]
    );

    // Extract the redirect status for validation
    let redirect_status = if let PageEvent::Response { status, .. } = &collected_events[1] {
        *status
    } else {
        unreachable!()
    };

    // Check second Request event for final URL
    assert!(
        matches!(
            &collected_events[2],
            PageEvent::Request { url, .. } if url.path() == "/tiny/index.html"
        ),
        "third event should be Request for the final URL, got: {:?}",
        collected_events[2]
    );

    // Check final Response event (should be 200)
    assert!(
        matches!(
            &collected_events[3],
            PageEvent::Response { url, status, .. } if url.path() == "/tiny/index.html" && *status == 200
        ),
        "fourth event should be Response for the final URL with 200 status, got: {:?}",
        collected_events[3]
    );

    // Verify FrameNavigated is present
    let frame_navigated = collected_events
        .iter()
        .any(|e| matches!(e, PageEvent::FrameNavigated { .. }));
    assert!(frame_navigated, "FrameNavigated event should be present");

    // Verify that navigation.redirects has the hop with correct status
    assert_eq!(
        navigation.redirects.len(),
        1,
        "navigation should have one redirect hop"
    );
    assert_eq!(
        navigation.redirects[0].url.path(),
        "/api/redirect",
        "redirect hop URL should be the initial URL"
    );
    assert_eq!(
        navigation.redirects[0].status, redirect_status,
        "redirect hop status should match the Response event status"
    );

    page.close().await.expect("close");
    engine.close();
}
