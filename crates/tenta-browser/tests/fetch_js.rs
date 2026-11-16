//! `fetch()` over a real socket, drained back into script by the page loop
//! (`docs/spec/05-javascript.md` §3, P2 slice 4a-1b).
//!
//! `crates/tenta-page/src/fetch.rs` covers the mappings against a hand-built
//! `tenta-net::Response`; `crates/tenta-js-v8/tests/dom_fetch.rs` covers the
//! engine half against a recording driver. This file is the end-to-end half and
//! is the only one where all three meet: V8 runs the page's script, the
//! `tenta-net` driver fetches over loopback, and the page loop's drain settles
//! the promise.
//!
//! **Every navigation here waits for `Settle::NetworkIdle`.** Under the default
//! `Settle::Load` the settle machine returns the moment `load` fires and
//! ignores requests that are still open, so settling tears the engine and the
//! driver down, `Drop` aborts the fetch, and it can never complete. That is a
//! property of the settle machine rather than of the driver, and a test written
//! against `Settle::Load` would fail for a reason that has nothing to do with
//! this slice.

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
    let temp = tenta_testutil::temp_dir("fetch-js");
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

/// The whole chain in one turn of the settle loop: the driver pushes the
/// completion, wakes the loop, and the drain settles the promise before the
/// settle can be judged finished.
///
/// Three assertions, and each one rules out a different vacuous pass. The
/// element's text proves the response reached script with the status and the
/// final URL the driver reported; the request count proves the fetch went over
/// the wire rather than being answered from anywhere else; and the settle
/// *state* proves the loop read the machine after the fetch was out of it.
///
/// `Settled { busy: false }` is the assertion a lost wake fails, and the way it
/// fails is worth stating exactly. `network_idle_machine` reports `Draining`
/// for as long as `network_sources_empty()` is false, and the wakeup a
/// foreground request schedules is at `record.started +
/// foreground_request_max` — the settle machine's *reclassification* bound. So
/// a completion nothing woke the loop for would leave the request registered
/// until then, reclassify it as background, and report `Settled { busy: true }`
/// rather than hanging. `busy: false` is what tells the two apart.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fetch_on_a_real_page_resolves_and_the_loop_waits_for_it() {
    let (server, _temp) = serve_all(&[
        (
            "index.html",
            "<!doctype html><html><head><title>fetch</title></head><body>\
             <div id=\"result\">unset</div>\
             <script>\
             fetch('/data.json').then(function (r) {\
               document.getElementById('result').textContent = r.status + ':' + r.url;\
             });\
             </script>\
             <p id=\"tail\">the rest of the parse</p>\
             </body></html>",
        ),
        ("data.json", "{\"ok\":true}"),
    ])
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
        "the loop judged the machine after the fetch was out of it, with nothing reclassified"
    );

    let document = page.take_document().await.expect("document");
    assert_eq!(
        text_of(&document, "#result"),
        format!("200:{}", server.url("/data.json")),
        "the promise resolved with a Response carrying the status and the final URL"
    );
    assert_eq!(
        server.requests_served(),
        2,
        "the document and the fetch, so the fetch provably went over the wire"
    );
    engine.close();
}

/// A response's headers and body reach script too, and both do so in the one
/// checkpoint the drain owes.
///
/// Two links of the promise chain, so the second one only runs because the
/// first turn's microtask checkpoint ran `then` and read the body. This is what
/// pins the drain's position in `turn`: before `pump_due_timers`, whose leading
/// checkpoint is what pays the debt the drain just created.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_response_header_and_body_reach_script() {
    let (server, _temp) = serve_all(&[
        (
            "index.html",
            "<!doctype html><html><head></head><body>\
             <div id=\"ctype\">unset</div>\
             <div id=\"body\">unset</div>\
             <script>\
             fetch('/data.json').then(function (r) {\
               document.getElementById('ctype').textContent = r.headers.get('content-type');\
               return r.text();\
             }).then(function (body) {\
               document.getElementById('body').textContent = body;\
             });\
             </script>\
             </body></html>",
        ),
        ("data.json", "{\"ok\":true}"),
    ])
    .await;
    let engine = js_engine(SettleConfig::library());
    let context = support::context(&engine);
    let page = context
        .new_page(PageOptions::default())
        .await
        .expect("a JavaScript page");

    let navigation = goto_network_idle(&page, &server.url("/index.html")).await;
    assert_eq!(navigation.settle, SettleState::Settled { busy: false });

    let document = page.take_document().await.expect("document");
    assert_eq!(
        text_of(&document, "#ctype"),
        "application/json",
        "the response's header list reached script"
    );
    assert_eq!(
        text_of(&document, "#body"),
        "{\"ok\":true}",
        "and the body did, through text()"
    );
    engine.close();
}

/// A `fetch()` to an origin that does not share its response rejects with a
/// `TypeError`, and the body never reaches the page. `cors_js.rs` covers the
/// protocol; this pins the rejection shape — a rejected promise rather than a
/// synchronous throw — because a `catch` block is the only thing that can
/// observe it.
///
/// The two fixture servers are two origins because they are two ports — RFC
/// 6454 counts the port — so this needs no DNS and no host name.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cross_origin_fetch_without_consent_rejects() {
    let (target, _target_temp) =
        serve_all(&[("data.json", "{\"secret\":\"the other origin's\"}")]).await;
    let url = target.url("/data.json");
    let (server, _temp) = serve_all(&[(
        "index.html",
        &format!(
            "<!doctype html><html><head></head><body>\
             <div id=\"result\">unset</div>\
             <script>\
             fetch('{url}').then(function (r) {{ return r.text(); }}).then(function (text) {{\
               document.getElementById('result').textContent = 'leaked ' + text;\
             }}, function (e) {{\
               document.getElementById('result').textContent = e.constructor.name;\
             }});\
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

    let document = page.take_document().await.expect("document");
    assert_eq!(
        text_of(&document, "#result"),
        "TypeError",
        "the failure reached script as a rejection, not a throw"
    );
    assert!(!document.text().contains("the other origin's"));
    assert_eq!(
        target.requests_served(),
        1,
        "the request was made, as a browser makes it; its answer was discarded"
    );
    engine.close();
}

/// A status that is not ok resolves rather than rejecting: a 503 is a
/// *response*, and `Response.ok` is what says otherwise.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_non_ok_status_resolves_rather_than_rejecting() {
    let (server, _temp) = serve_all(&[(
        "index.html",
        "<!doctype html><html><head></head><body>\
         <div id=\"result\">unset</div>\
         <script>\
         fetch('/api/status?code=503').then(function (r) {\
           document.getElementById('result').textContent = r.status + ':' + r.ok;\
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
    assert_eq!(navigation.settle, SettleState::Settled { busy: false });

    let document = page.take_document().await.expect("document");
    assert_eq!(
        text_of(&document, "#result"),
        "503:false",
        "a failed status is a response with ok === false, not a rejection"
    );
    assert_eq!(
        server.requests_served(),
        2,
        "the document and the 503 the fetch asked for"
    );
    engine.close();
}

/// A `POST` with a JSON body and a `Content-Type` over a real socket, asserted
/// against what the **server** says it received.
///
/// This is the slice that makes `fetch()`'s second argument mean anything, so the
/// assertions are the server's own echo rather than the page's opinion of it:
/// `reply.method`, `reply.headers['content-type']` and `reply.body` are the
/// fields `tenta-testutil`'s `/api/echo` fills from the *request* it was handed,
/// and `reply.json.item` is that body parsed back. A page that sent a `GET`, no
/// header, or no body could not produce this text.
///
/// `content-length` is asserted too, and it is the one field here that is the
/// *transport's* rather than the page's: the caller never sets it
/// (`crates/tenta-page/src/fetch.rs`), so the value the server saw is what
/// `tenta-net`'s `headers::build` derived from the body — the number the framing
/// of the request depends on.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_post_body_and_content_type_reach_the_server() {
    let (server, _temp) = serve_all(&[(
        "index.html",
        "<!doctype html><html><head></head><body>\
         <div id=\"method\">unset</div>\
         <div id=\"ctype\">unset</div>\
         <div id=\"length\">unset</div>\
         <div id=\"body\">unset</div>\
         <div id=\"item\">unset</div>\
         <script>\
         const sent = JSON.stringify({ item: 'beta', via: 'fetch' });\
         fetch('/api/echo', {\
           method: 'POST',\
           headers: { 'Content-Type': 'application/json' },\
           body: sent\
         }).then(function (response) { return response.json(); }).then(function (reply) {\
           const put = function (id, value) {\
             document.getElementById(id).textContent = value;\
           };\
           put('method', reply.method);\
           put('ctype', reply.headers['content-type']);\
           put('length', reply.headers['content-length']);\
           put('body', reply.body);\
           put('item', reply.json.item + ' via ' + reply.json.via);\
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
        "the loop judged the machine after the request was out of it"
    );

    // The body the page's own `JSON.stringify` produced, written out here so the
    // framing assertion below is derived from it rather than from a magic
    // number that a change to the page would silently invalidate.
    let sent = r#"{"item":"beta","via":"fetch"}"#;

    let document = page.take_document().await.expect("document");
    assert_eq!(
        text_of(&document, "#method"),
        "POST",
        "the server saw the method the initializer named"
    );
    assert_eq!(
        text_of(&document, "#ctype"),
        "application/json",
        "and the `Content-Type` the page set, which nothing appended a second of"
    );
    assert_eq!(
        text_of(&document, "#length"),
        sent.len().to_string(),
        "the transport framed the body with its own length"
    );
    assert_eq!(
        text_of(&document, "#body"),
        sent,
        "and the body itself arrived byte for byte"
    );
    assert_eq!(
        text_of(&document, "#item"),
        "beta via fetch",
        "the server parsed the body as JSON, so the POST's payload is what it read"
    );
    assert_eq!(
        server.requests_served(),
        2,
        "the document and the POST, so it provably went over the wire"
    );
    engine.close();
}

/// A string body with no `Content-Type` of its own gets the type the standard's
/// *extract a body with type* infers for the scalar-value-string arm —
/// `text/plain;charset=UTF-8` — and the server is what reports it.
///
/// The `method` is `POST` because a `GET` may not carry a body at all, which the
/// same standard states and `crates/tenta-js`'s `request_init` tests assert.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_string_body_infers_the_standards_content_type() {
    let (server, _temp) = serve_all(&[(
        "index.html",
        "<!doctype html><html><head></head><body>\
         <div id=\"ctype\">unset</div>\
         <div id=\"body\">unset</div>\
         <script>\
         fetch('/api/echo', { method: 'POST', body: 'plain text' })\
           .then(function (response) { return response.json(); })\
           .then(function (reply) {\
             document.getElementById('ctype').textContent = reply.headers['content-type'];\
             document.getElementById('body').textContent = reply.body;\
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
    assert_eq!(navigation.settle, SettleState::Settled { busy: false });

    let document = page.take_document().await.expect("document");
    assert_eq!(text_of(&document, "#ctype"), "text/plain;charset=UTF-8");
    assert_eq!(text_of(&document, "#body"), "plain text");
    engine.close();
}
