//! A real navigation whose `<script>` elements V8 itself runs
//! (`docs/spec/05-javascript.md` §6, P2 d-1b-8).
//!
//! Everything else that tests the page runtime drives a stub engine; this file
//! is the first that goes through the browser assembly which installs the V8
//! backend as the engine factory, parses a served page and runs its scripts.

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

/// Serves `page` at `/index.html` on loopback.
///
/// The temp directory is returned so it outlives the server, which reads its
/// files from it.
async fn serve(page: &str) -> (FixtureServer, TempDir) {
    let temp = tenta_testutil::temp_dir("settle-js");
    temp.write("index.html", page);
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

fn has(document: &Document, selector: &str) -> bool {
    document
        .query_selector(selector)
        .expect("selector parses")
        .is_some()
}

/// An inline script runs while the parse is paused on it, and the mutation it
/// leaves behind is what the caller reads after `goto` returns.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_inline_script_runs_and_mutates_the_document() {
    let (server, _temp) = serve(
        "<!doctype html><html><head><title>inline</title></head><body>\
         <div id=\"marker\">before</div>\
         <script>document.getElementById('marker').textContent = 'after';</script>\
         <p id=\"tail\">the rest of the parse</p>\
         </body></html>",
    )
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
    assert_eq!(navigation.status, 200);

    let document = page.take_document().expect("document");
    assert_eq!(
        text_of(&document, "#marker"),
        "after",
        "the script's mutation survived the navigation"
    );
    assert!(has(&document, "#tail"), "the parse ran past the script");
    engine.close();
}

/// A script that throws is a page-level fact, not an engine failure: the
/// navigation still succeeds, the rest of the parse runs, and the tree keeps
/// what the parse wrote before the script.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_throwing_script_does_not_fail_the_navigation() {
    let (server, _temp) = serve(
        "<!doctype html><html><head></head><body>\
         <div id=\"marker\">before</div>\
         <script>throw new Error('boom');</script>\
         <p id=\"tail\">the rest of the parse</p>\
         </body></html>",
    )
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
        .expect("a throwing script does not fail the navigation");
    assert_eq!(navigation.status, 200);

    let document = page.take_document().expect("document");
    assert_eq!(text_of(&document, "#marker"), "before");
    assert!(has(&document, "#tail"), "the parse ran past the script");
    engine.close();
}

/// A tracked `setTimeout` from an inline script holds the settle open until it
/// runs, so the page it leaves behind shows the deferral.
///
/// The engine applies the page's own timer rule, exactly as the stub-engine
/// test `crates/tenta-page/tests/runtime_settle.rs` proves: under the library
/// rule a 100 ms timer would be background, the settle would end at once, and
/// the callback would never run.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tracked_deferral_holds_the_settle_until_it_runs() {
    let (server, _temp) = serve(
        "<!doctype html><html><head></head><body>\
         <div id=\"marker\">before</div>\
         <script>setTimeout(function () { document.getElementById('marker').textContent = 'timer-ran'; }, 100);</script>\
         <p id=\"tail\">the rest of the parse</p>\
         </body></html>",
    )
    .await;
    let engine = js_engine(SettleConfig {
        tracked_timer_max: Duration::from_millis(200),
        ..SettleConfig::library()
    });
    let context = support::context(&engine);
    let page = context
        .new_page(PageOptions::default())
        .await
        .expect("a JavaScript page");

    let navigation = page
        .goto(
            &server.url("/index.html"),
            Goto::default().wait_until(Settle::NetworkIdle),
        )
        .await
        .expect("navigation");
    assert_eq!(navigation.settle, SettleState::Settled { busy: false });

    let document = page.take_document().expect("document");
    assert_eq!(
        text_of(&document, "#marker"),
        "timer-ran",
        "the settle ended only after the deferral it was holding for ran"
    );
    assert!(has(&document, "#tail"));
    engine.close();
}

/// The same page under a rule that makes the timer activity: it is the page's
/// rule the engine applies, so the settle ends before the callback fires and
/// the mutation never happens.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_deferral_under_a_short_rule_lets_the_settle_end_first() {
    let (server, _temp) = serve(
        "<!doctype html><html><head></head><body>\
         <div id=\"marker\">before</div>\
         <script>setTimeout(function () { document.getElementById('marker').textContent = 'timer-ran'; }, 100);</script>\
         <p id=\"tail\">the rest of the parse</p>\
         </body></html>",
    )
    .await;
    let engine = js_engine(SettleConfig {
        tracked_timer_max: Duration::from_millis(16),
        ..SettleConfig::library()
    });
    let context = support::context(&engine);
    let page = context
        .new_page(PageOptions::default())
        .await
        .expect("a JavaScript page");

    let navigation = page
        .goto(
            &server.url("/index.html"),
            Goto::default().wait_until(Settle::NetworkIdle),
        )
        .await
        .expect("navigation");
    assert_eq!(navigation.settle, SettleState::Settled { busy: false });

    let document = page.take_document().expect("document");
    assert_eq!(
        text_of(&document, "#marker"),
        "before",
        "a 100 ms timer is activity under a 16 ms rule, so the settle ends before it fires"
    );
    assert!(has(&document, "#tail"));
    engine.close();
}

/// A page's group reports the engine the page was given, which is how the
/// diagnostics tell a Tier 2 group from a Tier 1 one
/// (`docs/spec/02-architecture.md` §3).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pages_group_reports_the_engine_it_was_given() {
    let engine = js_engine(SettleConfig::library());
    let context = support::context(&engine);

    let scripted = context
        .new_page(PageOptions::default())
        .await
        .expect("a JavaScript page");
    assert!(
        context
            .group_threads()
            .iter()
            .all(|(_, _, has_engine)| *has_engine),
        "a page given an engine is in a group that has one: {:?}",
        context.group_threads()
    );

    // The same context distinguishes a page that was given none.
    let tier1 = context
        .new_page(PageOptions::tier1())
        .await
        .expect("a Tier 1 page");
    assert_eq!(
        context
            .group_threads()
            .iter()
            .filter(|(_, _, has_engine)| !has_engine)
            .count(),
        1,
        "only the Tier 1 page's group is engineless: {:?}",
        context.group_threads()
    );

    scripted.close().await.expect("close");
    tier1.close().await.expect("close");
    context.close();
    engine.close();
}

/// A Tier 1 page is given no engine even though the context could give it one,
/// so its script never runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tier1_page_builds_no_engine() {
    let (server, _temp) = serve(
        "<!doctype html><html><head></head><body>\
         <div id=\"marker\">before</div>\
         <script>document.getElementById('marker').textContent = 'ran';</script>\
         <p id=\"tail\">the rest of the parse</p>\
         </body></html>",
    )
    .await;
    let engine = js_engine(SettleConfig::library());
    let context = support::context(&engine);
    let page = context
        .new_page(PageOptions::tier1())
        .await
        .expect("a Tier 1 page");

    let navigation = page
        .goto(&server.url("/index.html"), Goto::default())
        .await
        .expect("navigation");
    assert_eq!(navigation.status, 200);

    let document = page.take_document().expect("document");
    assert_eq!(
        text_of(&document, "#marker"),
        "before",
        "a Tier 1 page runs no script"
    );
    assert!(has(&document, "#tail"));
    engine.close();
}
