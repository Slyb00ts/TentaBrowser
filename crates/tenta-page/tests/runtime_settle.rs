//! What running a page's scripts does to the settle call that follows
//! (`docs/spec/02-architecture.md` §5.1, P2 d-1b-4).
//!
//! The engine is a real `tenta_js` event loop, so the timer provenance, the
//! nesting clamp and the task counting the settle machine reacts to are the
//! production ones; only the script interpreter is a stub.
//!
//! One thing the stub has to do for itself, because it is standing in for a
//! page's engine: run a handed-out script under `Origin::Tracked`. That is the
//! attribution every deferral below rests on — a `setTimeout` can only be a
//! deferral if the script that scheduled it was part of the load chain — and it
//! is the real backend, not this harness, that owes it
//! (`crates/tenta-js-v8/tests/page_engine_provenance.rs` pins it there).

mod support;

use std::time::Duration;

use tenta_core::Origin;
use tenta_js::engine::LifecycleEvent;
use tenta_page::{Settle, SettleConfig, SettleState};

use support::Harness;

fn body(inner: &str) -> String {
    format!("<!doctype html><html><head></head><body>{inner}</body></html>")
}

fn settled(busy: bool) -> SettleState {
    SettleState::Settled { busy }
}

/// A settle configuration whose timer rule the page must apply to the engine.
fn rule(tracked_timer_max: Duration) -> SettleConfig {
    SettleConfig {
        tracked_timer_max,
        ..SettleConfig::library()
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_static_page_settles_at_load_without_entering_an_engine() {
    let harness = Harness::new(&body("<p id=tail>nothing scripted here</p>"));
    let navigation = harness
        .settle(Settle::NetworkIdle)
        .await
        .expect("navigation");

    assert_eq!(navigation.settle, settled(false));
    assert_eq!(harness.page.settle_state(), Some(settled(false)));
    assert!(
        harness.record.lock().unwrap().scripts.is_empty(),
        "a page with no script never creates an engine"
    );
    assert!(harness.has("#tail"));
}

/// A `setInterval` is activity, not a deferral: the page keeps running while
/// the settle call ends, exactly as a live tab's animation loop does.
#[tokio::test(flavor = "current_thread")]
async fn a_set_interval_that_mutates_does_not_hold_the_settle_open() {
    let harness = Harness::new(&body(
        "<script>setInterval(mutate js,30)</script><p id=tail>after</p>",
    ));
    let navigation = harness
        .settle(Settle::NetworkIdle)
        .await
        .expect("navigation");

    assert_eq!(navigation.settle, settled(false));
    assert!(!harness.record.lock().unwrap().has_marker("js"));
    assert!(harness.has("#tail"));
}

/// A page driving itself with `requestAnimationFrame` is background too: the
/// settle call ends with the callback still registered.
#[tokio::test(flavor = "current_thread")]
async fn an_animation_frame_loop_does_not_hold_the_settle_open() {
    let harness = Harness::new(&body("<script>raf-loop</script><p id=tail>after</p>"));
    let navigation = harness
        .settle(Settle::NetworkIdle)
        .await
        .expect("navigation");

    assert_eq!(navigation.settle, settled(false));
    assert_eq!(
        harness.record.lock().unwrap().frames_requested,
        1,
        "the script did register a frame callback"
    );
}

/// A short `setTimeout` from an inline script is a deferral of the load chain:
/// the settle call waits for it, and the page it left behind shows it ran.
///
/// This is also what proves the page applies *its* timer rule to the engine:
/// under the engine's own default (32 ms) a 100 ms timer would be background,
/// the settle would end at once, and the marker would never be appended.
#[tokio::test(flavor = "current_thread")]
async fn a_tracked_deferral_from_inline_script_holds_the_settle_until_it_runs() {
    let harness = Harness::builder(&body(
        "<script>setTimeout(mutate timer,100)</script><p id=tail>after</p>",
    ))
    .settle(rule(Duration::from_millis(200)))
    .build();
    let navigation = harness
        .settle(Settle::NetworkIdle)
        .await
        .expect("navigation");

    assert_eq!(navigation.settle, settled(false));
    assert!(
        harness.record.lock().unwrap().has_marker("timer"),
        "the settle call ended only after the deferral it was holding for ran"
    );
    assert!(
        harness
            .record
            .lock()
            .unwrap()
            .queued_origins()
            .contains(&Origin::Tracked),
        "the engine reported the script's timer as tracked"
    );
    assert!(harness.has("#tail"));
}

/// The page hands its engine the two user-agent lifecycle events, once each and
/// in the order HTML fires them (d-8).
///
/// The stub has no script to serve one to, so what this asserts is the page's
/// own decision — which is the half the engine cannot make: `DOMContentLoaded`
/// belongs at the end of the parse and `load` at the end of the load chain, and
/// it is the page that knows where those moments are.
///
/// The page carries a tracked deferral, so the settle loop runs more than one
/// turn. That is what makes the "once each" part a statement about the loop and
/// not about a single iteration: a machine that re-fired on every evaluation
/// would show the events repeated here.
#[tokio::test(flavor = "current_thread")]
async fn both_lifecycle_events_reach_the_engine_once_each_in_order() {
    let harness = Harness::new(&body(
        "<script>setTimeout(mutate timer,0)</script><p id=tail>after</p>",
    ));
    let navigation = harness
        .settle(Settle::NetworkIdle)
        .await
        .expect("navigation");

    assert_eq!(navigation.settle, settled(false));
    assert!(
        harness.record.lock().unwrap().has_marker("timer"),
        "the deferral ran, so the loop really did run more than one turn"
    );
    assert_eq!(
        harness.record.lock().unwrap().lifecycle,
        vec![LifecycleEvent::DomContentLoaded, LifecycleEvent::Load],
        "the parse's end came before the load chain's, and each fired exactly once"
    );
}

/// The same page under a rule that makes the timer activity: it is the page's
/// rule the engine applies, not the engine's own default.
#[tokio::test(flavor = "current_thread")]
async fn a_long_timer_under_a_short_rule_is_background() {
    let harness = Harness::builder(&body(
        "<script>setTimeout(mutate timer,100)</script><p id=tail>after</p>",
    ))
    .settle(rule(Duration::from_millis(16)))
    .build();
    let navigation = harness
        .settle(Settle::NetworkIdle)
        .await
        .expect("navigation");

    assert_eq!(navigation.settle, settled(false));
    assert!(
        !harness.record.lock().unwrap().has_marker("timer"),
        "a 100 ms timer is activity under a 16 ms rule, so the settle ends before it fires"
    );
    assert!(
        !harness
            .record
            .lock()
            .unwrap()
            .queued_origins()
            .contains(&Origin::Tracked),
        "nothing about that timer is a deferral"
    );
}

/// One turn of the page loop runs the rendering opportunity the engine owes as
/// well as the timers: a frame that comes due while a deferral is pending runs
/// before the settle call ends.
#[tokio::test(flavor = "current_thread")]
async fn an_animation_frame_due_while_a_deferral_is_pending_runs() {
    let harness = Harness::builder(&body(
        "<script>raf(mutate raf)\nsetTimeout(mutate timer,40)</script><p id=tail>after</p>",
    ))
    .settle(rule(Duration::from_millis(200)))
    .build();
    let navigation = harness
        .settle(Settle::NetworkIdle)
        .await
        .expect("navigation");

    assert_eq!(navigation.settle, settled(false));
    let record = harness.record.lock().unwrap();
    assert!(record.has_marker("raf"), "the frame callback ran");
    assert!(record.has_marker("timer"), "so did the deferral");
    assert!(record.frames_run >= 1);
}

/// A chain that reschedules itself forever finally *is* the page's work, so
/// the settle call ends at the busy cap instead of waiting for it.
///
/// The depth the chain is allowed to keep inheriting `Tracked` comes from the
/// page's `SettleConfig` too: under the library depth of 8 the chain would
/// become background after eight hops and the settle would end by itself.
#[tokio::test(flavor = "current_thread")]
async fn a_tracked_chain_that_never_ends_settles_busy_at_the_cap() {
    let settle = SettleConfig {
        busy_cap: Duration::from_millis(200),
        tracked_timer_max: Duration::from_millis(200),
        tracked_timer_depth: 64,
        ..SettleConfig::library()
    };
    let harness = Harness::builder(&body(
        "<script>loop-forever hop 20</script><p id=tail>after</p>",
    ))
    .settle(settle)
    .build();
    let navigation = harness
        .settle(Settle::NetworkIdle)
        .await
        .expect("navigation");

    assert_eq!(
        navigation.settle,
        settled(true),
        "the load chain never ended by itself, so the settle call gave up on it"
    );
    let record = harness.record.lock().unwrap();
    assert!(
        record.hops >= 2,
        "the engine was pumped during the settle: {} hops",
        record.hops
    );
}

/// The settle call reads a page whose parse is over and whose scripts have
/// run: the tree is complete and the deferral an inline script made has been
/// honoured by the time the call returns.
///
/// The pause-time snapshot the tracker keeps (`PendingSources::parser_running`
/// and `pending_scripts`) is fed by the page but is not observable from here —
/// `PendingSources` only leaves the page through `SettleState::Timeout`, and
/// the parse is always over before the settle loop looks at the page.
#[tokio::test(flavor = "current_thread")]
async fn the_settle_call_looks_at_a_page_whose_parse_is_over() {
    let harness = Harness::builder(&body(
        "<script>setTimeout(mutate timer,5)</script><p id=tail>after</p>",
    ))
    .settle(rule(Duration::from_millis(200)))
    .build();
    let navigation = harness
        .settle(Settle::NetworkIdle)
        .await
        .expect("navigation");

    assert_eq!(navigation.settle, settled(false));
    assert_eq!(
        harness.document(|document| support::text_of(document, "#tail")),
        Some("after".to_owned()),
        "the parse resumed past the script before the settle call looked at the page"
    );
    let record = harness.record.lock().unwrap();
    assert!(record.has_marker("timer"));
    assert_eq!(
        record.finished_of(Origin::Tracked),
        record.queued_of(Origin::Tracked),
        "every unit of tracked work the engine reported was released again"
    );
}
