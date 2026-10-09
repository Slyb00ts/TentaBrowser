//! A real navigation whose `<script>` elements V8 itself runs
//! (`docs/spec/05-javascript.md` §6, P2 d-1b-8).
//!
//! Everything else that tests the page runtime drives a stub engine; this file
//! is the first that goes through the browser assembly which installs the V8
//! backend as the engine factory, parses a served page and runs its scripts.

#![cfg(feature = "v8")]

mod support;

use std::time::Duration;

use tenta_browser::{
    ContextOptions, Document, Engine, Goto, PageOptions, Settle, SettleConfig, SettleState,
};
use tenta_core::{Budgets, Error, Tier};
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
    serve_all(&[("index.html", page)]).await
}

/// Serves each `(relative path, contents)` on loopback, so a page can fetch a
/// script from a path of its own.
async fn serve_all(files: &[(&str, &str)]) -> (FixtureServer, TempDir) {
    let temp = tenta_testutil::temp_dir("settle-js");
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

    let document = page.take_document().await.expect("document");
    assert_eq!(
        text_of(&document, "#marker"),
        "after",
        "the script's mutation survived the navigation"
    );
    assert!(has(&document, "#tail"), "the parse ran past the script");
    engine.close();
}

/// A timer callback that never returns must not hang the page thread.
///
/// `setTimeout(() => { while (true) {} }, 0)` runs as a timer callback, which is
/// an engine entry the engine does not bound itself: without the process-wide
/// watchdog the page thread would spin for ever inside `pump_due_timers` and
/// this navigation would never return. The callback mutates the tree before it
/// loops, so the marker proves it ran at all — a test that only asserted
/// "did not hang" would pass just as well if the script had never run.
///
/// The per-entry deadline is the page's `evaluate_time` budget, lowered here to
/// tens of milliseconds so the test is fast; `tokio::time::timeout` turns a
/// regression into a failure rather than a hung suite.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_runaway_timer_callback_does_not_hang_the_navigation() {
    let (server, _temp) = serve(
        "<!doctype html><html><head></head><body>\
         <script>\
         setTimeout(function () {\
           document.getElementById('tail').textContent = 'reached';\
           while (true) {}\
         }, 0);\
         </script>\
         <p id=\"tail\">the rest of the parse</p>\
         </body></html>",
    )
    .await;
    let engine = js_engine(SettleConfig::library());
    // A page's script deadline is its context's `evaluate_time`; lowering it
    // lowers the deadline the page arms around an engine entry.
    let context = engine
        .new_context(ContextOptions {
            allow_private_network: true,
            budgets: Some(Budgets::desktop().evaluate_time(Duration::from_millis(100))),
            ..ContextOptions::default()
        })
        .expect("context");
    let page = context
        .new_page(PageOptions::default())
        .await
        .expect("a JavaScript page");

    let navigation = tokio::time::timeout(
        Duration::from_secs(10),
        page.goto(&server.url("/index.html"), Goto::default()),
    )
    .await
    .expect("the navigation returned instead of hanging on the runaway callback")
    .expect("navigation");
    assert_eq!(navigation.status, 200);

    let document = page.take_document().await.expect("document");
    assert_eq!(
        text_of(&document, "#tail"),
        "reached",
        "the runaway callback ran and mutated the tree before it was terminated"
    );
    engine.close();
}

/// A lifecycle handler that never returns must not hang the page thread.
///
/// The two lifecycle dispatches are the engine entries the engine does not
/// bound itself: `tenta-page` routes each through `PageRuntime::bounded`, the
/// same arm-and-charge the timer pump gets, and this is the test that fails
/// without it. Each handler mutates the tree before it loops, so the two markers
/// are the proof the handlers ran at all rather than a hang that happened before
/// either of them.
///
/// Both halves are asserted, because they are bounded by two different call
/// sites and neither bounds the other. `DOMContentLoaded` is dispatched from
/// inside `navigate`, `load` from the settle loop — an entry armed in one place
/// would leave the other spinning. And the `load` marker is the second thing the
/// terminated `DOMContentLoaded` entry must not have taken with it: one entry's
/// deadline ending does not end the page, which is the same property the
/// runaway-timer test above pins for a single entry.
///
/// `tokio::time::timeout` turns a regression into a failure rather than a hung
/// suite, as it does in the sibling tests.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_runaway_lifecycle_handler_does_not_hang_the_navigation() {
    let (server, _temp) = serve(
        "<!doctype html><html><head></head><body>\
         <script>\
         document.addEventListener('DOMContentLoaded', function () {\
           document.getElementById('dcl').textContent = 'the parse ran to its end';\
           while (true) {}\
         });\
         window.addEventListener('load', function () {\
           document.getElementById('loaded').textContent = 'the load chain ran too';\
         });\
         </script>\
         <p id=\"dcl\">the parse has not ended</p>\
         <p id=\"loaded\">the load chain has not ended</p>\
         </body></html>",
    )
    .await;
    let engine = js_engine(SettleConfig::library());
    let context = engine
        .new_context(ContextOptions {
            allow_private_network: true,
            budgets: Some(Budgets::desktop().evaluate_time(Duration::from_millis(100))),
            ..ContextOptions::default()
        })
        .expect("context");
    let page = context
        .new_page(PageOptions::default())
        .await
        .expect("a JavaScript page");

    let navigation = tokio::time::timeout(
        Duration::from_secs(10),
        page.goto(&server.url("/index.html"), Goto::default()),
    )
    .await
    .expect("the navigation returned instead of hanging on the runaway handler")
    .expect("navigation");
    assert_eq!(navigation.status, 200);

    let document = page.take_document().await.expect("document");
    assert_eq!(
        text_of(&document, "#dcl"),
        "the parse ran to its end",
        "the `DOMContentLoaded` handler ran and mutated the tree before its \
         deadline terminated it"
    );
    assert_eq!(
        text_of(&document, "#loaded"),
        "the load chain ran too",
        "and the terminated entry did not take the rest of the navigation with \
         it: `load` still fired, under a deadline of its own"
    );
    engine.close();
}

/// The other half of the pair with the runaway-timer test above.
///
/// Under the default 60 s script budget a terminated runaway is survivable:
/// termination is how the page survives it (the test above). A page whose
/// *cumulative* script budget is reached has not made progress, and that is a
/// crash (`docs/spec/05-javascript.md` §11). Here `script_cpu` is set equal to
/// the per-entry `evaluate_time`, so the one `while (true) {}` that the engine's
/// own deadline terminates is itself the crossing — a terminated entry is not a
/// crash by itself; spending the budget is.
///
/// `tokio::time::timeout` turns a regression into a failure rather than a hung
/// suite, as it does in the sibling test.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_page_that_reaches_its_script_budget_is_crashed() {
    let (server, _temp) = serve(
        "<!doctype html><html><head></head><body>\
         <script>while (true) {}</script>\
         <script>while (true) {}</script>\
         <p id=\"tail\">the rest of the parse</p>\
         </body></html>",
    )
    .await;
    let engine = js_engine(SettleConfig::library());
    // An entry is terminated at half the window budget (50 ms here), so one
    // terminated entry is survivable and the second is the crossing.
    let context = engine
        .new_context(ContextOptions {
            allow_private_network: true,
            budgets: Some(
                Budgets::desktop()
                    .evaluate_time(Duration::from_millis(100))
                    .script_cpu(Duration::from_millis(100)),
            ),
            ..ContextOptions::default()
        })
        .expect("context");
    let page = context
        .new_page(PageOptions::default())
        .await
        .expect("a JavaScript page");

    let error = tokio::time::timeout(
        Duration::from_secs(10),
        page.goto(&server.url("/index.html"), Goto::default()),
    )
    .await
    .expect("the navigation returned instead of hanging on the runaway script")
    .expect_err("a page that spends its whole script budget is crashed");
    match &error {
        Error::Crashed { reason } => {
            assert!(
                reason.contains("script"),
                "the reason names the budget: {reason}"
            );
        }
        other => panic!("expected Crashed, got {other:?}"),
    }
    assert!(
        page.crash_reason().is_some(),
        "the page is marked crashed, not merely a failed navigation"
    );
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

    let document = page.take_document().await.expect("document");
    assert_eq!(text_of(&document, "#marker"), "before");
    assert!(has(&document, "#tail"), "the parse ran past the script");
    engine.close();
}

/// A parser-blocking external classic script is fetched while the parse is
/// paused on it and run before the parse resumes (`docs/spec/05-javascript.md`
/// §6, P2 d-2a).
///
/// Both halves of that are load-bearing and both are asserted: the two inline
/// scripts bracket the external one in document order, and the external script
/// observes that the markup *after* the element is not in the tree yet — which
/// it could only be if it had run once the parse was past it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_external_script_is_fetched_and_runs_while_the_parse_waits() {
    let (server, _temp) = serve_all(&[
        (
            "index.html",
            "<!doctype html><html><head><title>external</title></head><body>\
             <div id=\"order\">start</div>\
             <div id=\"probe\">unset</div>\
             <script>document.getElementById('order').textContent = 'first';</script>\
             <script src=\"middle.js\"></script>\
             <script>document.getElementById('order').textContent += ',last';</script>\
             <p id=\"tail\">the rest of the parse</p>\
             </body></html>",
        ),
        (
            "middle.js",
            "document.getElementById('order').textContent += ',middle';\
             document.getElementById('probe').textContent = \
             String(document.getElementById('tail') === null);",
        ),
    ])
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

    let document = page.take_document().await.expect("document");
    assert_eq!(
        text_of(&document, "#order"),
        "first,middle,last",
        "the fetched script ran between the two inline ones, in document order"
    );
    assert_eq!(
        text_of(&document, "#probe"),
        "true",
        "the fetched script ran while the parse was still paused on it"
    );
    assert!(has(&document, "#tail"), "the parse resumed past the script");
    engine.close();
}

/// A script the origin does not serve is a failed script fetch: the navigation
/// succeeds, the parse still reaches the end of the document, the error page is
/// not run as JavaScript, and the failed request does not keep the network
/// sources non-empty — a settle left waiting on it would run out its deadline
/// instead of settling.
///
/// The request count is what makes this test non-vacuous: a script that was
/// skipped rather than fetched would leave the server on one request, and the
/// same page would look identical from the document alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_script_the_origin_does_not_serve_still_lets_the_parse_finish() {
    let (server, _temp) = serve(
        "<!doctype html><html><head></head><body>\
         <div id=\"marker\">before</div>\
         <script src=\"gone.js\"></script>\
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
        .goto(
            &server.url("/index.html"),
            Goto::default()
                .wait_until(Settle::NetworkIdle)
                .timeout(Duration::from_secs(5)),
        )
        .await
        .expect("a failed script fetch does not fail the navigation");
    assert_eq!(navigation.status, 200);
    assert_eq!(
        navigation.settle,
        SettleState::Settled { busy: false },
        "the request the failed fetch left behind is out of the settle machine"
    );
    assert_eq!(
        server.requests_served(),
        2,
        "the document and the script, so the script's fetch was really issued"
    );

    let document = page.take_document().await.expect("document");
    assert_eq!(
        text_of(&document, "#marker"),
        "before",
        "the 404 body was not run in place of the script"
    );
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

    let document = page.take_document().await.expect("document");
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

    let document = page.take_document().await.expect("document");
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

    let document = page.take_document().await.expect("document");
    assert_eq!(
        text_of(&document, "#marker"),
        "before",
        "a Tier 1 page runs no script"
    );
    assert!(has(&document, "#tail"));
    engine.close();
}
