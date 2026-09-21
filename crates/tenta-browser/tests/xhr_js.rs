//! `XMLHttpRequest` over a real socket, from `send()` to the events the page
//! sees (`docs/spec/05-javascript.md` §3, P2 slice b-6).
//!
//! `crates/tenta-page/src/fetch.rs` covers the mappings against hand-built
//! requests and responses; `crates/tenta-js-v8/tests/dom_xhr.rs` covers the
//! engine half against a recording driver. This file is the end-to-end half and
//! is the only one where all three meet: V8 runs the page's script, the
//! `tenta-net` driver sends the request over loopback, and the page loop's drain
//! dispatches the answer at the object that made it.
//!
//! **Every navigation here waits for `Settle::NetworkIdle`.** Under the default
//! `Settle::Load` the settle machine returns the moment `load` fires and ignores
//! requests that are still open, so settling tears the engine and the driver
//! down, `Drop` aborts the request, and it can never complete. That is a property
//! of the settle machine rather than of the driver, and a test written against
//! `Settle::Load` would fail for a reason that has nothing to do with this
//! slice.
//!
//! The first test runs the *benchmark fixture itself*
//! (`tests/fixtures/js-xhr/index.html`), which is what makes the `S-js-xhr`
//! scenario's subject measurable; the others build their own pages, because a
//! gate needs a second origin that a self-contained fixture cannot have.

#![cfg(feature = "v8")]

mod support;

use std::time::Duration;

use tenta_browser::{Document, Engine, Goto, PageOptions, Settle, SettleConfig, SettleState};
use tenta_core::Tier;
use tenta_testutil::{FixtureServer, FixtureServerConfig, TempDir};

/// An engine whose ceiling reaches Tier 2, so a page may ask for JavaScript.
fn js_engine(settle: SettleConfig) -> Engine {
    Engine::builder()
        .io_threads(1)
        .page_threads(1)
        .max_tier(Tier::Js)
        .settle(settle)
        .build()
        .expect("engine")
}

/// Serves each `(relative path, contents)` on loopback, so a page can be asked
/// for a path of its own.
///
/// The built-in `/api/*` routes are served whatever the root is, which is what
/// lets these pages use `/api/echo` and `/api/redirect` beside their own files.
async fn serve_all(files: &[(&str, &str)]) -> (FixtureServer, TempDir) {
    let temp = tenta_testutil::temp_dir("xhr-js");
    for (relative, contents) in files {
        temp.write(relative, contents);
    }
    let server = FixtureServer::start(FixtureServerConfig::new(temp.path()))
        .await
        .expect("fixture server");
    (server, temp)
}

/// The text of the element `selector` matches, panicking when it is absent.
///
/// `Document::text_of` is the `--dump text` shape — one line per block element
/// — so even a one-word `<div>` comes back with a trailing newline.
fn text_of(document: &Document, selector: &str) -> String {
    let node = document
        .query_selector(selector)
        .expect("selector parses")
        .unwrap_or_else(|| panic!("{selector} is in the document"));
    document.text_of(node).trim().to_owned()
}

/// A navigation that waited for the network, under a deadline that fails the
/// test rather than hanging it.
///
/// The timeout bounds a genuine hang — a driver that neither pushed nor
/// completed its request leaves a foreground request holding the settle open
/// until the page's own `Goto` deadline.
async fn goto_network_idle(page: &tenta_browser::Page, url: &str) -> tenta_browser::Navigation {
    tokio::time::timeout(
        Duration::from_secs(10),
        page.goto(url, Goto::default().wait_until(Settle::NetworkIdle)),
    )
    .await
    .expect("the navigation settled instead of hanging")
    .expect("navigation")
}

// ---------------------------------------------------------------------------
// The fixture the benchmark scenario runs
// ---------------------------------------------------------------------------

/// The benchmark fixture's own page: one `XMLHttpRequest` and one `fetch()`,
/// both against the server's JSON echo, and the content they put in the DOM.
///
/// The default root is `tests/fixtures/`, so this is the file the benchmark
/// scenario serves — not a copy of its shape — and it is the only test that runs
/// it.
///
/// **The content assertions are the point, not the marker.** The fixture's
/// `xhr.onerror` calls `done()` too, and the scenario's
/// `expect_stdout_contains = "js-xhr: done (2 requests)"` therefore passes even
/// when the request failed — a driver that failed every request would settle the
/// page just as fast. Each slot is checked against something only a *successful*
/// round trip can produce: `#xhr` is the server's own `query` field for
/// `/api/echo?item=alpha&via=xhr`, and `#fetch` is `reply.json.item` — a field
/// the server only adds when the body it received parses as JSON — joined with
/// `reply.json.via` from that same parsed body.
///
/// **Why the marker is not the proof, kept from the slice this one completes.**
/// This test was written with the `fetch` half `#[ignore]`d: `fetch(url, init)`
/// used to be refused by `unsupported_init`
/// (`crates/tenta-js-v8/src/host.rs`) with a *synchronous* `TypeError`, so the
/// chain's `.catch` never ran and `done()` was never called for the second
/// request. A test that only grepped for the marker would have been unable to
/// tell that from a working page, because `xhr.onerror` calls `done()` as well —
/// which is exactly why the slots are asserted here and the marker only in the
/// scenario.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_fixture_page_settles_its_marker_once_the_fetch_half_works() {
    let server = support::server().await;
    let engine = js_engine(SettleConfig::library());
    let context = support::context(&engine);
    let page = context
        .new_page(PageOptions::default())
        .await
        .expect("a JavaScript page");

    let navigation = goto_network_idle(&page, &server.url("/js-xhr/index.html")).await;
    assert_eq!(navigation.status, 200);
    assert_eq!(
        navigation.settle,
        SettleState::Settled { busy: false },
        "the loop judged the machine after the request was out of it, with nothing reclassified"
    );

    let document = page.take_document().expect("document");
    assert_eq!(
        text_of(&document, "#xhr"),
        "xhr: item=alpha&via=xhr",
        "the response's body reached the page's `onload` and was parsed as JSON"
    );
    assert_eq!(
        text_of(&document, "#fetch"),
        "fetch: beta via fetch",
        "the POST's JSON body reached the server and its `json` field came back"
    );
    assert_eq!(
        text_of(&document, "#status"),
        "js-xhr: done (2 requests)",
        "both halves settled, which is the string the scenario greps for"
    );
    assert_eq!(
        server.requests_served(),
        3,
        "the document, the XHR and the `fetch` POST: every request the page makes \
         provably went over the wire"
    );
    engine.close();
}

// ---------------------------------------------------------------------------
// The two cross-origin gates, end to end
// ---------------------------------------------------------------------------

/// Gate 1: a URL that is not same-origin with the document is refused before
/// anything is sent, and the page sees the standard's *request error steps*.
///
/// The two fixture servers are two origins because they are two ports — RFC 6454
/// counts the port — so this needs no DNS and no host name. The request count on
/// the second server is the non-vacuous half: a page that never called `send()`
/// would also show `status === 0`, so `requests_served() == 0` there is what
/// proves the refusal happened before the wire.
///
/// The refusal is *synchronous*, and that turn is this engine's rather than the
/// standard's: the standard runs `send()`'s fetch in parallel, so a network
/// error reaches the page after `send()` returns. `XhrDriver`'s doc records the
/// deviation; this test is what pins it. The page records the order — the
/// handler's line runs before the statement after `send()` — because a driver
/// that deferred the refusal would reach the same final state on a later turn,
/// and only the ordering tells the two apart.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cross_origin_xhr_is_refused_before_it_reaches_the_other_origin() {
    let (target, _target_temp) = serve_all(&[("data.json", "{\"ok\":true}")]).await;
    let url = target.url("/data.json");
    let (server, _temp) = serve_all(&[(
        "index.html",
        &format!(
            "<!doctype html><html><head></head><body>\
             <div id=\"result\">unset</div>\
             <script>\
             var order = [];\
             var xhr = new XMLHttpRequest();\
             xhr.open('GET', '{url}');\
             xhr.onload = function () {{ order.push('load'); }};\
             xhr.onerror = function () {{ order.push('error'); }};\
             xhr.send();\
             order.push('after send');\
             document.getElementById('result').textContent =\
               'error@' + xhr.readyState + '@' + xhr.status + '@' + order.join(',');\
             </script>\
             </body></html>"
        ),
    )])
    .await;
    let engine = js_engine(SettleConfig::library());
    let context = support::context(&engine);
    let page = context
        .new_page(PageOptions::default())
        .await
        .expect("a JavaScript page");

    let navigation = goto_network_idle(&page, &server.url("/index.html")).await;
    assert_eq!(navigation.settle, SettleState::Settled { busy: false });

    let document = page.take_document().expect("document");
    assert_eq!(
        text_of(&document, "#result"),
        "error@4@0@error,after send",
        "the request error steps ran inside `send()` — DONE, the network error's \
         status, the `error` event, and only then the statement after the call"
    );
    assert_eq!(
        target.requests_served(),
        0,
        "the other origin was never asked for anything"
    );
    assert_eq!(server.requests_served(), 1, "only the document");
    engine.close();
}

/// Gate 2: a same-origin URL that redirects off-origin is a failure, and the
/// cross-origin body never reaches the page.
///
/// `Client::send` follows redirects internally, so this is the case gate 1
/// cannot see: the URL the page named *is* same-origin, and the transport is what
/// leaves the origin. Dropping gate 2 would hand the page the other origin's body
/// and header list behind a refusal that looked complete.
///
/// Both halves of the assertion matter. `#result` holding `error@0` rather than
/// the body is the leak assertion — the page's `onload` would have put that body
/// in the same slot. And `requests_served() == 1` on the other origin is what
/// proves the gate closed a *real* leak path: the redirected request really was
/// made, and its answer was discarded after the fact rather than never asked
/// for.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_same_origin_xhr_that_redirects_off_origin_is_a_failure() {
    let (target, _target_temp) =
        serve_all(&[("leak.json", "{\"secret\":\"the other origin's\"}")]).await;
    let url = target.url("/leak.json");
    let (server, _temp) = serve_all(&[(
        "index.html",
        &format!(
            "<!doctype html><html><head></head><body>\
             <div id=\"result\">unset</div>\
             <script>\
             var xhr = new XMLHttpRequest();\
             xhr.open('GET', '/api/redirect?to={url}');\
             xhr.onload = function () {{ document.getElementById('result').textContent = 'load:' + xhr.responseText; }};\
             xhr.onerror = function () {{\
               document.getElementById('result').textContent = 'error@' + xhr.readyState + '@' + xhr.status;\
             }};\
             xhr.send();\
             </script>\
             </body></html>"
        ),
    )])
    .await;
    let engine = js_engine(SettleConfig::library());
    let context = support::context(&engine);
    let page = context
        .new_page(PageOptions::default())
        .await
        .expect("a JavaScript page");

    let navigation = goto_network_idle(&page, &server.url("/index.html")).await;
    assert_eq!(navigation.settle, SettleState::Settled { busy: false });

    let document = page.take_document().expect("document");
    assert_eq!(
        text_of(&document, "#result"),
        "error@4@0",
        "the redirected response was discarded rather than delivered"
    );
    assert!(
        !document.text().contains("the other origin's"),
        "and the cross-origin body is nowhere in the document"
    );
    assert_eq!(
        target.requests_served(),
        1,
        "the other origin was reached, so the gate closed a real leak rather than \
         a request nothing made"
    );
    assert_eq!(
        server.requests_served(),
        2,
        "the document and the same-origin URL that redirected"
    );
    engine.close();
}
