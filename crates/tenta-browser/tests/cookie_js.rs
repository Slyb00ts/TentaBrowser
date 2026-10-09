//! `document.cookie` over a real page (`docs/spec/05-javascript.md` §3, P2 slice
//! d-4b b-1).
//!
//! `crates/tenta-js-v8/tests/dom_cookie.rs` covers the binding against a jar only
//! Rust can see. This file is the end-to-end half, and it exists for one reason:
//! the jar `document.cookie` writes into must be **the same jar `tenta-net` sends
//! from**, not a per-engine copy. A cookie a page sets here has to appear in the
//! `Cookie` header of a later request, which is a fact no engine-only test can
//! observe.
//!
//! **Every test here with a `fetch` in flight waits for `Settle::NetworkIdle`.**
//! Under the default `Settle::Load` the settle machine returns the moment `load`
//! fires and ignores requests that are still open, so settling tears the engine
//! and the driver down, `Drop` aborts the fetch, and it can never complete. That
//! is a property of the settle machine rather than of this slice; the tests
//! without a fetch say why they may use `Settle::Load`.

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

/// Serves each `(relative path, contents)` on loopback, so a page can fetch a
/// resource from a path of its own.
async fn serve_all(files: &[(&str, &str)]) -> (FixtureServer, TempDir) {
    let temp = tenta_testutil::temp_dir("cookie-js");
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
async fn goto_network_idle(page: &tenta_browser::Page, url: &str) -> tenta_browser::Navigation {
    tokio::time::timeout(
        Duration::from_secs(10),
        page.goto(url, Goto::default().wait_until(Settle::NetworkIdle)),
    )
    .await
    .expect("the navigation settled instead of hanging")
    .expect("navigation")
}

/// A cookie script writes is in the jar the *client* sends from.
///
/// The page sets the cookie and then fetches `/api/cookie`, which echoes the
/// `Cookie` header the request carried as `{"cookie": "..."}`. The echoed header
/// containing `fromjs=1` is what proves the write reached the shared jar: a
/// per-engine copy would leave the header empty, and the request count proves the
/// fetch actually went over the wire rather than being answered from anywhere
/// else — a page that never fetched would also show an empty header.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cookie_script_set_is_sent_on_a_later_request() {
    let (server, _temp) = serve_all(&[(
        "index.html",
        "<!doctype html><html><head><title>cookie</title></head><body>\
         <div id=\"result\">unset</div>\
         <script>\
         document.cookie = 'fromjs=1; Path=/';\
         fetch('/api/cookie').then(function (r) {\
           return r.json();\
         }).then(function (json) {\
           document.getElementById('result').textContent = json.cookie;\
         });\
         </script>\
         </body></html>",
    )])
    .await;
    let engine = js_engine(SettleConfig::library());
    let context = support::context(&engine);
    let page = context
        .new_page(PageOptions::default())
        .await
        .expect("a JavaScript page");

    let navigation = goto_network_idle(&page, &server.url("/index.html")).await;
    assert_eq!(navigation.status, 200);
    assert_eq!(
        navigation.settle,
        SettleState::Settled { busy: false },
        "the loop judged the machine after the fetch was out of it"
    );

    let document = page.take_document().await.expect("document");
    let echoed = text_of(&document, "#result");
    assert!(
        echoed.contains("fromjs=1"),
        "the cookie script wrote was sent on the fetch's request: {echoed:?}"
    );
    assert_eq!(
        server.requests_served(),
        2,
        "the document and the fetch, so the fetch provably went over the wire"
    );
    engine.close();
}

/// The jar is the context's, so a cookie survives a navigation.
///
/// Page one sets `persist=1`; page two, a second navigation of the *same* page to
/// the same origin, reads `document.cookie` back. A per-engine (rather than
/// per-context) jar would lose it at the first navigation, because a navigation
/// builds a fresh engine. No fetch is in flight, so `Settle::Load` is enough
/// here — nothing needs the settle machine to wait on a request.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cookie_survives_a_navigation() {
    let (server, _temp) = serve_all(&[
        (
            "index.html",
            "<!doctype html><html><head></head><body>\
             <script>document.cookie = 'persist=1';</script>\
             </body></html>",
        ),
        (
            "read.html",
            "<!doctype html><html><head></head><body>\
             <div id=\"result\">unset</div>\
             <script>\
             document.getElementById('result').textContent = document.cookie;\
             </script>\
             </body></html>",
        ),
    ])
    .await;
    let engine = js_engine(SettleConfig::library());
    let context = support::context(&engine);
    let page = context
        .new_page(PageOptions::default())
        .await
        .expect("a JavaScript page");

    let first = page
        .goto(&server.url("/index.html"), Goto::default())
        .await
        .expect("navigation");
    assert_eq!(first.settle, SettleState::Settled { busy: false });

    let second = page
        .goto(&server.url("/read.html"), Goto::default())
        .await
        .expect("navigation");
    assert_eq!(second.settle, SettleState::Settled { busy: false });

    let document = page.take_document().await.expect("document");
    assert!(
        text_of(&document, "#result").contains("persist=1"),
        "the cookie set before the navigation is still in the context's jar"
    );
    engine.close();
}

/// A script cannot set an `HttpOnly` cookie, and cannot see one.
///
/// The assignment is refused by the jar's non-HTTP API rule — not by a check in
/// the binding — so this test also pins that the binding passes `is_http = false`
/// on the setter. The normal cookie is set and read back in the same script, so
/// the negative assertion is not satisfied vacuously by an empty string.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_httponly_assignment_from_script_is_refused() {
    let (server, _temp) = serve_all(&[(
        "index.html",
        "<!doctype html><html><head></head><body>\
         <div id=\"result\">unset</div>\
         <script>\
         document.cookie = 'visible=1';\
         document.cookie = 'h=1; HttpOnly';\
         document.getElementById('result').textContent = document.cookie;\
         </script>\
         </body></html>",
    )])
    .await;
    let engine = js_engine(SettleConfig::library());
    let context = support::context(&engine);
    let page = context
        .new_page(PageOptions::default())
        .await
        .expect("a JavaScript page");

    let navigation = page
        .goto(&server.url("/index.html"), Goto::default())
        .await
        .expect("navigation");
    assert_eq!(navigation.settle, SettleState::Settled { busy: false });

    let document = page.take_document().await.expect("document");
    let got = text_of(&document, "#result");
    assert!(
        got.contains("visible=1"),
        "a normal cookie is stored and read back: {got:?}"
    );
    assert!(
        !got.contains("h=1"),
        "a script assignment marked HttpOnly is refused by the jar: {got:?}"
    );
    engine.close();
}
