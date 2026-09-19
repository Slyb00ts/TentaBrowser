//! `defer` and `async`: the two external classic scripts whose fetch does not
//! hold the parse (the HTML standard's "the `script` element", P2 d-2b).
//!
//! The two timings differ in exactly one thing, and every test here is about
//! it: what the script waits for *after* its fetch. `defer` waits for the
//! document to be parsed — in document order, before `DOMContentLoaded` — while
//! `async` waits for nothing at all and runs the moment its text arrives. Both
//! are still part of the document's load chain, so the `load` event waits for
//! them, whatever order they run in.
//!
//! Ordering is only observable if the fetches can be made to finish in a known
//! order, so the tests that are about it use `delayed_route`: an origin that
//! answers one script 200 ms after the other has answered its own.

mod support;

use std::time::Duration;

use tenta_page::page::Goto;
use tenta_page::{Settle, SettleConfig, SettleState};

use support::{Harness, Recorder};

/// How long a fixture route the tests order against waits. Long enough that
/// the parse of a three-line document is over many times first, which is what
/// turns "this arrived after that" into a fact rather than a race.
const ANSWER_DELAY: Duration = Duration::from_millis(200);

fn body(inner: &str) -> String {
    format!("<!doctype html><html><head></head><body>{inner}</body></html>")
}

/// The scripts the engine ran, in order.
fn sources(record: &Recorder) -> Vec<String> {
    record.lock().unwrap().sources()
}

/// Whether the script at `index` could already see the markup that follows it.
fn saw_tail(record: &Recorder, index: usize) -> bool {
    record.lock().unwrap().scripts[index].tail_parsed
}

/// The text of the tail element, which is only there if the parse resumed.
fn tail(harness: &Harness) -> Option<String> {
    harness.document(|document| support::text_of(document, "#tail"))
}

// ------------------------------------------------------------------ `defer`

/// A deferred script waits for the document: the standard runs it once the
/// document has been parsed. `tail_parsed` is what tells that apart from
/// running at the parse point, where the markup after the script — this very
/// `<p>` — is not in the tree yet.
#[tokio::test(flavor = "current_thread")]
async fn a_deferred_script_runs_after_the_document_is_parsed() {
    let harness = Harness::builder(&body(
        "<script src=\"deferred.js\" defer></script><p id=tail>after</p>",
    ))
    .route("deferred.js", "deferred")
    .build();
    let navigation = harness.settle(Settle::Load).await.expect("navigation");

    assert_eq!(sources(&harness.record), vec!["deferred".to_owned()]);
    assert!(
        saw_tail(&harness.record, 0),
        "it ran once the parse was over, so the whole document was in the tree"
    );
    assert_eq!(tail(&harness), Some("after".to_owned()));
    assert_eq!(navigation.settle, SettleState::Settled { busy: false });
}

/// Deferred scripts run in *document* order, which is a promise about the
/// scripts and not about their fetches: the first one here answers a fifth of a
/// second after the second one does, and still runs first.
#[tokio::test(flavor = "current_thread")]
async fn a_deferred_script_runs_in_document_order_however_the_fetches_arrive() {
    let harness = Harness::builder(&body(
        "<script src=\"first.js\" defer></script><script src=\"second.js\" defer></script>\
         <p id=tail>after</p>",
    ))
    .delayed_route("first.js", "first", ANSWER_DELAY)
    .route("second.js", "second")
    .build();
    harness.settle(Settle::Load).await.expect("navigation");

    assert_eq!(
        sources(&harness.record),
        vec!["first".to_owned(), "second".to_owned()],
        "the second script's fetch arrived first and it still ran second"
    );
}

/// A deferred script runs before `DOMContentLoaded`, not merely at some point
/// after the parse: a settlement that waits only for `DOMContentLoaded` still
/// gets it run, because the parse does not hand the document over until its
/// deferred scripts have had their turn.
#[tokio::test(flavor = "current_thread")]
async fn a_deferred_script_runs_before_domcontentloaded() {
    let harness = Harness::builder(&body(
        "<script src=\"deferred.js\" defer></script><p id=tail>after</p>",
    ))
    .delayed_route("deferred.js", "deferred", ANSWER_DELAY)
    .build();
    let navigation = harness
        .settle(Settle::DomContent)
        .await
        .expect("navigation");

    assert_eq!(
        sources(&harness.record),
        vec!["deferred".to_owned()],
        "waiting for DOMContentLoaded was enough to observe the deferred script"
    );
    assert!(
        navigation.settle.is_settled(),
        "and the settle call ended by reaching it, not by timing out"
    );
}

/// A deferred script delays the document, not the parse: a parser-blocking
/// script written *after* it runs first, exactly as document order says — the
/// deferred one has not run and cannot run until the parse is over.
#[tokio::test(flavor = "current_thread")]
async fn a_deferred_script_does_not_delay_a_later_parser_blocking_one() {
    let harness = Harness::builder(&body(
        "<script src=\"deferred.js\" defer></script><script src=\"blocking.js\"></script>\
         <p id=tail>after</p>",
    ))
    .delayed_route("deferred.js", "deferred", ANSWER_DELAY)
    .route("blocking.js", "blocking")
    .build();
    harness.settle(Settle::Load).await.expect("navigation");

    assert_eq!(
        sources(&harness.record),
        vec!["blocking".to_owned(), "deferred".to_owned()],
        "the blocking script ran at its own pause, before the document was finished"
    );
}

// ------------------------------------------------------------------ `async`

/// An `async` script runs when its fetch arrives rather than when the document
/// is parsed, and the `load` event waits for it. The two halves are visible
/// here at once: the script ran, so `load` waited; and it saw the whole
/// document, so the parse did not.
#[tokio::test(flavor = "current_thread")]
async fn an_async_script_runs_when_its_fetch_completes_without_holding_the_parse() {
    let harness = Harness::builder(&body(
        "<script src=\"asynced.js\" async></script><p id=tail>after</p>",
    ))
    .delayed_route("asynced.js", "asynced", ANSWER_DELAY)
    .build();
    let navigation = harness.settle(Settle::Load).await.expect("navigation");

    assert_eq!(
        sources(&harness.record),
        vec!["asynced".to_owned()],
        "the load event waited for the script that was still in flight"
    );
    assert!(
        saw_tail(&harness.record, 0),
        "the parse did not wait for it: the document was complete when it arrived"
    );
    assert_eq!(navigation.settle, SettleState::Settled { busy: false });
}

/// The order `async` scripts run in is the order their fetches finish, which is
/// the whole difference from `defer`: reversed here against the document.
#[tokio::test(flavor = "current_thread")]
async fn an_async_script_runs_in_the_order_its_fetches_arrive() {
    let harness = Harness::builder(&body(
        "<script src=\"first.js\" async></script><script src=\"second.js\" async></script>\
         <p id=tail>after</p>",
    ))
    .delayed_route("first.js", "first", ANSWER_DELAY)
    .route("second.js", "second")
    .build();
    harness.settle(Settle::Load).await.expect("navigation");

    assert_eq!(
        sources(&harness.record),
        vec!["second".to_owned(), "first".to_owned()],
        "async scripts run as they arrive, not in document order"
    );
}

/// An external script carrying both attributes is `async`: `defer` is the
/// legacy fallback it was introduced as, not a second instruction. A settlement
/// that waits only for `DOMContentLoaded` is what makes the difference
/// visible, because a `defer` script would have run before the parse handed the
/// document over and an `async` one has run only if it arrived by then.
///
/// That second half is a known limitation rather than a rule: a navigation
/// lives for one settle call, and when that call ends the engine — and the
/// fetch task hanging off it — goes with it. Here the fetch has not even been
/// polled yet. Closing this means keeping the engine alive past the
/// settlement, which is P3-shaped.
#[tokio::test(flavor = "current_thread")]
async fn async_wins_over_defer_on_an_external_script() {
    let harness = Harness::builder(&body(
        "<script src=\"both.js\" async defer></script><p id=tail>after</p>",
    ))
    .delayed_route("both.js", "both", ANSWER_DELAY)
    .build();
    harness
        .settle(Settle::DomContent)
        .await
        .expect("navigation");

    assert!(
        sources(&harness.record).is_empty(),
        "`defer` would have made the parse wait for the script; `async` did not"
    );
    assert_eq!(
        tail(&harness),
        Some("after".to_owned()),
        "and it did not hold the parse the way a script with no timing attribute does"
    );
}

// ------------------------------------------------------- the load chain

/// A parallel fetch is a *parser* request, exactly as a blocking one is: the
/// parser inserted this script, so the settle machine never demotes it to
/// background while the document is still waiting for it. `busy` is where that
/// shows — a reclassified request leaves the settle `busy` even when everything
/// else finished — and the route is deliberately slower than the configured
/// `foreground_request_max` so the machine would have had the chance to.
#[tokio::test(flavor = "current_thread")]
async fn a_slow_parallel_fetch_is_never_reclassified_as_background() {
    let settle = SettleConfig {
        foreground_request_max: Duration::from_millis(50),
        ..SettleConfig::library()
    };
    let harness = Harness::builder(&body(
        "<script src=\"slow.js\" async></script><p id=tail>after</p>",
    ))
    .settle(settle)
    .delayed_route("slow.js", "slow", ANSWER_DELAY)
    .build();
    let navigation = harness
        .settle(Settle::NetworkIdle)
        .await
        .expect("navigation");

    assert_eq!(sources(&harness.record), vec!["slow".to_owned()]);
    assert_eq!(
        navigation.settle,
        SettleState::Settled { busy: false },
        "the fetch outlived foreground_request_max and was not demoted for it"
    );
}

/// `nomodule` is a gate and not a timing attribute: this engine has no module
/// support, so the gate is open and the script runs — and carrying neither
/// `async` nor `defer`, it runs where a classic script without them runs, which
/// is at the parse point.
#[tokio::test(flavor = "current_thread")]
async fn a_nomodule_script_without_a_timing_attribute_is_parser_blocking() {
    let harness = Harness::builder(&body(
        "<script src=\"nomodule.js\" nomodule></script><p id=tail>after</p>",
    ))
    .route("nomodule.js", "nomodule")
    .build();
    harness.settle(Settle::Load).await.expect("navigation");

    assert_eq!(sources(&harness.record), vec!["nomodule".to_owned()]);
    assert!(
        !saw_tail(&harness.record, 0),
        "the parse was paused on it, so the markup after it was not in the tree"
    );
    assert_eq!(tail(&harness), Some("after".to_owned()));
}

/// A failed parallel fetch is the script's own failure, and it must not leave
/// its claim on the settle behind: each of the two holds a count until its
/// fetch has resolved, and a count nobody releases is a settle that runs out
/// its deadline. `NetworkIdle` with a short deadline is what makes that
/// observable in seconds rather than in thirty.
#[tokio::test(flavor = "current_thread")]
async fn parallel_fetches_that_fail_do_not_hang_the_settle() {
    let harness = Harness::builder(&body(
        "<script src=\"gone-defer.js\" defer></script>\
         <script src=\"gone-async.js\" async></script>\
         <p id=tail>after</p>",
    ))
    .failing_route("gone-defer.js")
    .failing_route("gone-async.js")
    .build();
    let navigation = harness
        .goto(
            Goto::new()
                .wait_until(Settle::NetworkIdle)
                .timeout(Duration::from_secs(5)),
        )
        .await
        .expect("a failed script fetch does not fail the navigation");

    assert!(
        sources(&harness.record).is_empty(),
        "nothing ran, and nothing was invented to run"
    );
    assert_eq!(
        navigation.settle,
        SettleState::Settled { busy: false },
        "both counts were released by the failures, so the load event could fire"
    );
    assert_eq!(tail(&harness), Some("after".to_owned()));
}

/// The type is decided before the timing: a module is not a classic script
/// whatever else it carries, so `defer` does not make it one — and a module's
/// text must not be fetched as a classic script's (P2 d-2c).
#[tokio::test(flavor = "current_thread")]
async fn a_module_script_is_not_deferred_as_a_classic_one() {
    let harness = Harness::builder(&body(
        "<script src=\"app.js\" type=\"module\" defer></script><p id=tail>after</p>",
    ))
    .route("app.js", "app")
    .build();
    harness.settle(Settle::Load).await.expect("navigation");

    assert!(sources(&harness.record).is_empty(), "a module did not run");
    assert_eq!(
        harness.requests(),
        vec!["/".to_owned()],
        "and it was not fetched as a classic one"
    );
}
