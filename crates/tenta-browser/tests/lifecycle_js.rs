//! `DOMContentLoaded` and `load` as events page script can hear
//! (`docs/spec/05-javascript.md` §3, P2 slice d-8).
//!
//! This is the end-to-end half. `crates/tenta-js-v8/tests/page_engine.rs`
//! covers the dispatch itself through the engine boundary against a hand-built
//! document; `crates/tenta-page/src/runtime.rs` and `tenta-js/tests/engine_trait.rs`
//! pin the bounding and the trait shape. What only a real navigation can show is
//! that the two events fire at the right moments in the parse-and-settle
//! sequence, in order, once each — and that a handler's own work is inside the
//! settle rather than after it.
//!
//! **Every navigation here waits for `Settle::NetworkIdle`.** A `load` handler
//! that queues a timer or a request is a deferred task the default
//! `Settle::Load` does not wait for, so a test written against it would settle
//! and tear the engine down before the handler's work ran — a failure with
//! nothing to do with this slice. `Settle::NetworkIdle` is also the only mode
//! that waits out the `load` event at all.

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

/// Serves one page at `/index.html` on loopback, for a test that needs a page of
/// its own shape rather than the shared fixture.
async fn serve_one(html: &str) -> (FixtureServer, TempDir) {
    let temp = tenta_testutil::temp_dir("lifecycle-js");
    temp.write("index.html", html);
    let server = FixtureServer::start(FixtureServerConfig::new(temp.path()))
        .await
        .expect("fixture server");
    (server, temp)
}

/// The text of the element `selector` matches, panicking when it is absent.
///
/// `Document::text_of` is the `--dump text` shape — one line per block element
/// — so even a one-word `<p>` comes back with a trailing newline.
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

/// The fixture page's own record of what each lifecycle event looked like.
///
/// `tests/fixtures/js-lifecycle/index.html` registers all three shapes a page
/// uses — `document.addEventListener('DOMContentLoaded')`,
/// `window.addEventListener('load')` and the `window.onload` handler attribute
/// — plus a `load` listener registered from inside the `DOMContentLoaded`
/// handler. The single text node it renders is therefore a compact statement of
/// four separate requirements at once.
///
/// Every field of every entry is load-bearing:
///
///  * `type` — the event the page was handed is the one HTML names;
///  * `target === document` — `DOMContentLoaded` is dispatched at the Document,
///    and `load` at the window is dispatched with DOM's *legacy target
///    override* flag, so `event.target` is the window's document while the
///    window's own listeners are the ones that run;
///  * `isTrusted` — the events are UA-fired, not script-constructed;
///  * `bubbles`/`cancelable` — HTML initialises `DOMContentLoaded`'s bubbles
///    attribute to true and says nothing about either flag for `load`, so the
///    pair is `true/false` and then `false/false`.
///
/// The order of the entries is asserted by the string itself: `dcl` before
/// `load`. The two window listeners come before `onload` because they were
/// registered before the handler was assigned, and `load-late` last because it
/// was registered inside the `DOMContentLoaded` handler — which it can only be,
/// and can only be reached, if the events are dispatched in order.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_fixture_page_hears_both_lifecycle_events_once_each_in_order() {
    let server = support::server().await;
    let engine = js_engine(SettleConfig::library());
    let context = support::context(&engine);
    let page = context
        .new_page(PageOptions::default())
        .await
        .expect("a JavaScript page");

    let navigation = goto_network_idle(&page, &server.url("/js-lifecycle/index.html")).await;
    assert_eq!(navigation.status, 200);
    assert_eq!(
        navigation.settle,
        SettleState::Settled { busy: false },
        "the loop judged the machine after both events had been dispatched"
    );

    let document = page.take_document().expect("document");
    assert_eq!(
        text_of(&document, "#status"),
        "lifecycle: \
         dcl=DOMContentLoaded/true/true/true/false|\
         load=load/true/true/false/false|\
         onload=load/true/true/false/false|\
         load-late=load/true/true/false/false",
        "both events reached every kind of listener the page registered, in order, \
         with the target, the trust flag and the flags HTML gives them"
    );
    assert_eq!(server.requests_served(), 1, "only the document");
    engine.close();
}

/// A `load` handler's own work is inside the settle.
///
/// This is the requirement the slice is most likely to get subtly wrong. The
/// settle machine decides the document is loaded the moment it *books* the
/// event, so a dispatch that ran after that decision — or outside the
/// `Origin::Tracked` scope that marks a task as belonging to the load chain —
/// would let the loop settle and tear the engine down with the handler's timer
/// still queued. The page would then read `pending` forever.
///
/// The snapshot is taken twice, at two delays, and both readings are asserted:
/// the second is what makes this a statement about the settle rather than about
/// a single turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_load_handler_that_queues_a_timer_is_inside_the_settle() {
    let (server, _temp) = serve_one(
        "<!doctype html><html><head></head><body>\
         <p id=\"status\">late: pending</p>\
         <script>\
         var seen = [];\
         window.addEventListener('load', function () {\
           seen.push('handler');\
           setTimeout(function () { seen.push('t0'); }, 0);\
           setTimeout(function () {\
             seen.push('t30');\
             document.getElementById('status').textContent = 'late: ' + seen.join(',');\
           }, 30);\
         });\
         </script>\
         </body></html>",
    )
    .await;
    let engine = js_engine(SettleConfig::library());
    let context = support::context(&engine);
    let page = context
        .new_page(PageOptions::default())
        .await
        .expect("a JavaScript page");

    let navigation = goto_network_idle(&page, &server.url("/index.html")).await;
    assert_eq!(
        navigation.settle,
        SettleState::Settled { busy: false },
        "the settle waited for the handler's timers, so it settled on the last one \
         rather than on a `load` that was booked but never ran"
    );

    let document = page.take_document().expect("document");
    assert_eq!(
        text_of(&document, "#status"),
        "late: handler,t0,t30",
        "the `load` handler ran, and both timers it queued ran before the engine \
         was torn down"
    );
    engine.close();
}

/// Both events fire exactly once, however many turns the settle loop runs.
///
/// The settle loop can iterate many times — it has to, for the timers above —
/// and the page layer's own state is what stops a second dispatch. The counters
/// are read from three timers at increasing delays, all armed inside the `load`
/// handler, so a loop that dispatched again on any later turn would be caught
/// by whichever reading came after it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn both_events_fire_once_each_across_many_settle_iterations() {
    let (server, _temp) = serve_one(
        "<!doctype html><html><head></head><body>\
         <p id=\"status\">counts: pending</p>\
         <script>\
         var dcl = 0, loads = 0;\
         var seen = [];\
         document.addEventListener('DOMContentLoaded', function () { dcl += 1; });\
         window.addEventListener('load', function () {\
           loads += 1;\
           function snap(label) {\
             return function () { seen.push(label + ':dcl=' + dcl + ',load=' + loads); };\
           }\
           setTimeout(snap('t0'), 0);\
           setTimeout(snap('t5'), 5);\
           setTimeout(function () {\
             snap('t20')();\
             document.getElementById('status').textContent = 'counts: ' + seen.join(' ');\
           }, 20);\
         });\
         </script>\
         </body></html>",
    )
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
        text_of(&document, "#status"),
        "counts: t0:dcl=1,load=1 t5:dcl=1,load=1 t20:dcl=1,load=1",
        "each event was dispatched once and only once, across every turn the \
         settle loop ran"
    );
    engine.close();
}

/// `DOMContentLoaded` fires after the whole document has been parsed.
///
/// HTML fires it "once the document has been completely parsed", which is a
/// statement about *when* rather than about *whether* — and it is the half a
/// dispatch driven by the settle machine could get wrong, because the machine
/// has other reasons to move on. The script that registers the listener is in
/// `<head>`, before the element it goes on to reach, so a dispatch that happened
/// while the parser was still running would run the handler against a tree
/// without `#late` in it and leave the marker at `pending`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn domcontentloaded_fires_after_the_whole_document_is_parsed() {
    let (server, _temp) = serve_one(
        "<!doctype html><html><head>\
         <script>\
         document.addEventListener('DOMContentLoaded', function () {\
           var el = document.getElementById('late');\
           if (el === null) { return; }\
           el.textContent = 'dcl: found the element';\
         });\
         </script>\
         </head><body>\
         <p id=\"late\">dcl: pending</p>\
         </body></html>",
    )
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
        text_of(&document, "#late"),
        "dcl: found the element",
        "the handler ran after the parser had put the later element in the tree, \
         so the dispatch is at the end of the parse and not during it"
    );
    engine.close();
}
