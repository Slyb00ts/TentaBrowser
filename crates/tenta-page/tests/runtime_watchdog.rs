//! The page's per-entry deadline, armed on the process-wide watchdog thread
//! (`docs/spec/05-javascript.md` §11, P2 d-1b-7 stage (b)).
//!
//! These tests drive the stub engine (`support`), which is the only engine the
//! page layer may name: a backend must never appear under `tenta-page`
//! (`cargo xtask lint-deps`). The stub's `block` command is a callback that
//! does not return until it is terminated, so a page whose deadline is not
//! enforced stays blocked inside the entry — which is why every navigation here
//! runs on a thread of its own and is waited for with a timeout: a missing arm
//! must fail the test, not wedge the suite.

mod support;

use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use support::{Harness, URL};
use tenta_core::{Budgets, Error};
use tenta_page::PageEvent;
use tenta_page::page::{Goto, Navigation};
use tenta_page::settle::Settle;

/// How long a test waits for a navigation before declaring the page hung by a
/// callback that was never terminated.
const PATIENCE: Duration = Duration::from_secs(10);

/// The per-entry script deadline the fast tests configure. A blocked callback
/// costs exactly this, so it is small.
const BUDGET: Duration = Duration::from_millis(100);

/// The deadline for the two-page test, which needs a window wide enough to
/// start the second page while the first is still blocked.
const WIDE_BUDGET: Duration = Duration::from_millis(500);

/// A harness whose engine hands out a watchdog handle and whose page has a
/// script budget of `budget`.
fn harness_with(body: &str, budget: Duration) -> Harness {
    Harness::builder(body)
        .watchdog(true)
        .budgets(Budgets::default().evaluate_time(budget))
        .build()
}

/// Starts a navigation on a thread of its own and hands back the channel that
/// will carry its outcome.
///
/// The page is driven on its own current-thread runtime because `drive` runs the
/// page's tasks inline, so a page that blocks inside a callback blocks its own
/// thread — which is exactly the state under test, and must not be the test's.
fn start_goto(harness: &Harness) -> mpsc::Receiver<tenta_core::Result<Navigation>> {
    let page = harness.page.clone();
    let thread = harness.thread.clone();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a current-thread runtime for the page thread");
        let outcome = runtime.block_on(support::drive(&thread, async move {
            page.goto(URL, Goto::new().wait_until(Settle::Load)).await
        }));
        let _ = tx.send(outcome);
    });
    rx
}

/// Waits for one navigation, failing the test rather than hanging when the page
/// never returns.
fn await_goto(rx: &mpsc::Receiver<tenta_core::Result<Navigation>>) -> Navigation {
    match rx.recv_timeout(PATIENCE) {
        Ok(Ok(navigation)) => navigation,
        Ok(Err(error)) => panic!("the navigation failed: {error:?}"),
        Err(_) => panic!(
            "the page thread never returned: a callback that does not return was \
             never terminated"
        ),
    }
}

/// Waits for one navigation and returns whatever it produced — including the
/// error a crash leaves behind — rather than panicking on an error. Still fails
/// the test, rather than hanging, when the page never returns.
fn await_goto_result(
    rx: &mpsc::Receiver<tenta_core::Result<Navigation>>,
) -> tenta_core::Result<Navigation> {
    match rx.recv_timeout(PATIENCE) {
        Ok(result) => result,
        Err(_) => panic!(
            "the page thread never returned: a callback that does not return was \
             never terminated"
        ),
    }
}

/// A busy-wait on a `Condvar`-free flag, used only to sequence two threads in
/// the two-page test.
fn wait_until(what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(1));
    }
}

/// The core of the defect: `while (true) {}` inside a `setTimeout` callback.
/// The page arms the entry, the process-wide watchdog fires at the budget, and
/// the turn returns instead of hanging the page thread for ever.
#[test]
fn a_timer_that_never_returns_is_terminated_and_the_page_returns() {
    let harness = harness_with(
        "<!doctype html><html><body>\
         <script>setTimeout(block, 0)</script>\
         </body></html>",
        BUDGET,
    );

    let navigation = await_goto(&start_goto(&harness));

    assert!(
        navigation.error.is_none(),
        "a terminated callback is not a failed navigation: {navigation:?}"
    );
    let record = harness.record.lock().unwrap();
    assert!(
        record.blocked,
        "the callback reached the point of no return"
    );
    assert_eq!(
        record.terminations, 1,
        "the entry's deadline terminated the callback exactly once"
    );
}

/// The other half of the observation: a callback that returns on its own is
/// never cut short. Nothing about arming a deadline may fire on an entry that
/// finished inside it.
#[test]
fn a_normal_callback_is_never_terminated() {
    let harness = harness_with(
        "<!doctype html><html><body>\
         <script>setTimeout(mutate done, 0)</script>\
         </body></html>",
        BUDGET,
    );

    let navigation = await_goto(&start_goto(&harness));

    assert!(navigation.error.is_none(), "{navigation:?}");
    let record = harness.record.lock().unwrap();
    assert!(
        record.has_marker("done"),
        "the callback ran; the timer fired"
    );
    assert!(!record.blocked, "no callback blocked");
    assert_eq!(
        record.terminations, 0,
        "an entry that returned inside its deadline is never terminated"
    );
}

/// The property a single global slot would break.
///
/// Page A blocks in a callback, so its deadline is armed and live. Page B then
/// navigates and arms its own deadline while A is still blocked. With one slot
/// per process, B's arm replaces A's — A's handle and deadline are lost, and A
/// is never terminated, so `await_goto(A)` times out. With an entry per arm,
/// both deadlines fire independently.
#[test]
fn two_pages_arming_at_once_keep_their_own_deadlines() {
    let a = harness_with(
        "<!doctype html><html><body>\
         <script>setTimeout(block, 0)</script>\
         </body></html>",
        WIDE_BUDGET,
    );
    let b = harness_with(
        "<!doctype html><html><body>\
         <script>setTimeout(mutate b-done, 0)</script>\
         </body></html>",
        WIDE_BUDGET,
    );

    let a_rx = start_goto(&a);
    // A is inside the callback that never returns, so its arm is live and
    // cannot have fired yet (its deadline is a whole budget away).
    wait_until("page A to block in its callback", || {
        a.record.lock().unwrap().blocked
    });

    let b_navigation = await_goto(&start_goto(&b));
    assert!(b_navigation.error.is_none(), "{b_navigation:?}");

    // Only reached if A's own deadline was still armed after B armed its own.
    let a_navigation = await_goto(&a_rx);
    assert!(a_navigation.error.is_none(), "{a_navigation:?}");

    assert_eq!(
        a.record.lock().unwrap().terminations,
        1,
        "page A was terminated"
    );
    assert_eq!(
        b.record.lock().unwrap().terminations,
        0,
        "page B never blocked"
    );
}

/// An engine that reports no watchdog handle cannot be interrupted, so the page
/// arms nothing: a callback that returns still runs, and nothing is terminated.
/// The point is that the page must not panic on the missing handle.
#[test]
fn an_engine_without_a_watchdog_arms_nothing_and_does_not_panic() {
    // The default harness engine hands out no watchdog handle.
    let harness = Harness::builder(
        "<!doctype html><html><body>\
         <script>setTimeout(mutate no-handle, 0)</script>\
         </body></html>",
    )
    .build();

    let navigation = await_goto(&start_goto(&harness));

    assert!(navigation.error.is_none(), "{navigation:?}");
    let record = harness.record.lock().unwrap();
    assert!(record.has_marker("no-handle"), "the callback still ran");
    assert_eq!(
        record.terminations, 0,
        "there is no handle to arm, so no terminate can be asked for"
    );
}

/// A page whose cumulative script time reaches its budget is *crashed*, and the
/// navigation that crossed it fails with `Error::Crashed` rather than
/// succeeding or hanging.
///
/// The budget is set equal to the per-entry deadline, so one terminated entry —
/// a `block` callback cut short at `evaluate_time` — is itself the crossing.
/// Termination is not the crash; spending the budget is, which is why the same
/// terminated entry is survivable under the default 60 s budget
/// (`a_timer_that_never_returns_is_terminated_and_the_page_returns`).
#[test]
fn a_page_that_burns_its_script_budget_crashes() {
    let harness = Harness::builder(
        "<!doctype html><html><body>\
         <script>setTimeout(block, 0)</script>\
         </body></html>",
    )
    .watchdog(true)
    .budgets(Budgets::default().evaluate_time(BUDGET).script_cpu(BUDGET))
    .build();
    let mut events = harness.page.events();

    let outcome = await_goto_result(&start_goto(&harness));
    match outcome {
        Err(Error::Crashed { reason }) => {
            assert!(
                reason.contains("script"),
                "the reason names the budget: {reason}"
            );
        }
        other => panic!("expected the navigation to crash, got {other:?}"),
    }
    let reason = harness.page.crash_reason().expect("the page crashed");
    assert!(
        reason.contains("script"),
        "the reason names the budget: {reason}"
    );
    assert!(
        events
            .drain()
            .iter()
            .any(|event| matches!(event, PageEvent::Crashed { .. })),
        "the crash is reported as an event"
    );

    // A crashed page is terminal: a second `goto` is refused before it can post
    // a job, so it runs no script and reports the same crash.
    let scripts_before = harness.record.lock().unwrap().scripts.len();
    let again = await_goto_result(&start_goto(&harness));
    assert!(matches!(again, Err(Error::Crashed { .. })), "{again:?}");
    assert_eq!(
        harness.record.lock().unwrap().scripts.len(),
        scripts_before,
        "a crashed page runs no further script"
    );
}

/// A guard, not new coverage: a page that stays inside its budget across several
/// navigations and turns is never crashed, and its `crash_reason` stays `None`.
///
/// It is here so a botched crash check — one that fires on any charge, or on a
/// terminated-but-survivable entry — can never make every page crash. The
/// coverage it protects lives in the tests below and in
/// `crates/tenta-browser/tests/settle_js.rs`.
#[test]
fn a_page_inside_its_budget_is_not_crashed() {
    let harness = Harness::builder(
        "<!doctype html><html><body>\
         <script>setTimeout(mutate tick, 0)</script>\
         </body></html>",
    )
    .watchdog(true)
    .budgets(Budgets::default().script_cpu(Duration::from_secs(60)))
    .build();

    for _ in 0..3 {
        let navigation = await_goto(&start_goto(&harness));
        assert!(navigation.error.is_none(), "{navigation:?}");
    }
    assert_eq!(
        harness.page.crash_reason(),
        None,
        "a page inside its budget stays alive across navigations"
    );
}

/// The budget is the page's, not the navigation's.
///
/// Two navigations that each spend well under the budget together cross it, so
/// the second crashes. A per-navigation accumulator would restart the second
/// navigation at zero and never trip; this test is what catches that
/// regression. `spin` spends the time without ever needing the entry killed, so
/// nothing here depends on the watchdog thread's timing — the evidence is the
/// observable behaviour, not a terminate count.
#[test]
fn the_script_budget_spans_navigations() {
    let harness = Harness::builder(
        "<!doctype html><html><body>\
         <script>spin 100</script>\
         </body></html>",
    )
    // No watchdog: a `spin` always returns, so no entry needs terminating.
    .budgets(Budgets::default().script_cpu(Duration::from_millis(150)))
    .build();

    let first = await_goto(&start_goto(&harness));
    assert!(
        first.error.is_none(),
        "the first navigation is under the budget: {first:?}"
    );
    assert_eq!(
        harness.page.crash_reason(),
        None,
        "one navigation's worth of script is not the page's budget"
    );

    let second = await_goto_result(&start_goto(&harness));
    match second {
        Err(Error::Crashed { reason }) => {
            assert!(
                reason.contains("script"),
                "the reason names the budget: {reason}"
            );
        }
        other => panic!("expected the second navigation to crash, got {other:?}"),
    }
    let reason = harness.page.crash_reason().expect("the page crashed");
    let spent: u64 = reason
        .split(" ms spent")
        .next()
        .and_then(|head| head.rsplit(' ').next())
        .and_then(|millis| millis.parse().ok())
        .unwrap_or_else(|| panic!("the reason names the spent total: {reason}"));
    assert!(
        spent >= 150,
        "the accumulated total is past the 150 ms budget: {reason}"
    );
}
