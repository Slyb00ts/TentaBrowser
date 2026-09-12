//! Tests for P2 d-0d: driving an engine on wall time instead of a virtual
//! clock, and the single time base that `performance.now()`,
//! `performance.timeOrigin` and `Event.timeStamp` all read.
//!
//! A headless run keeps the virtual clock so timer ordering is reproducible; a
//! live page passes [`Clock::real`] and is driven by `next_wakeup`/
//! `pump_due_timers` instead of `run_event_loop`, which moves the clock itself
//! and therefore only advances a virtual one (`docs/spec/05-javascript.md` §5).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_js::event_loop::Clock;
use tenta_js_v8::Engine;

fn test_document() -> Rc<RefCell<Document>> {
    let mut document = Document::new();
    let html = document.create_html_element(atoms::HTML);
    let body = document.create_html_element(atoms::BODY);
    document
        .mutate(Mutation::Append {
            parent: document.document_node(),
            child: html,
        })
        .expect("append <html>");
    document
        .mutate(Mutation::Append {
            parent: html,
            child: body,
        })
        .expect("append <body>");
    Rc::new(RefCell::new(document))
}

fn real_engine() -> Engine {
    Engine::with_document_on_clock(test_document(), Clock::real(), None)
}

/// A delay comfortably longer than an `evaluate` on a loaded machine, so a
/// "not due yet" assertion is not a race against V8 compiling the next script.
const NOT_YET: Duration = Duration::from_millis(500);

/// Evaluates `source` and pays the microtask checkpoint the script entry owes,
/// so a following `next_wakeup()` reports pending *work* rather than the
/// checkpoint this call just incurred.
fn eval(engine: &mut Engine, source: &str) -> String {
    let value = engine
        .evaluate(source)
        .unwrap_or_else(|error| panic!("evaluate `{source}`: {error}"));
    engine.run_microtasks();
    value
}

fn number(engine: &mut Engine, source: &str) -> f64 {
    eval(engine, source)
        .parse()
        .unwrap_or_else(|_| panic!("`{source}` returns a number"))
}

#[test]
fn a_real_clock_advances_performance_now_without_anyone_stepping_it() {
    let mut engine = real_engine();
    let before = number(&mut engine, "performance.now()");
    std::thread::sleep(Duration::from_millis(20));
    let after = number(&mut engine, "performance.now()");
    assert!(
        after >= before + 15.0,
        "wall time must move the clock on its own: {before} -> {after}"
    );
}

#[test]
fn a_real_clock_anchors_time_origin_to_the_wall_clock() {
    let mut engine = real_engine();
    // `timeOrigin + performance.now()` is the page's wall-clock reading, which
    // is how script correlates a performance timestamp with `Date.now()`.
    let drift = number(
        &mut engine,
        "Date.now() - (performance.timeOrigin + performance.now())",
    );
    assert!(
        drift.abs() < 1000.0,
        "timeOrigin + now() should track Date.now(), off by {drift} ms"
    );
}

#[test]
fn an_event_timestamps_on_the_same_clock_as_performance_now() {
    // The virtual clock makes the equality exact and observable: both readings
    // are the loop's clock, so a page computing `performance.now() -
    // evt.timeStamp` as a dispatch latency gets 0, not the gap between two
    // unrelated origins.
    let mut engine = Engine::with_document(test_document());
    engine.tick_frame();
    assert_eq!(
        engine
            .evaluate("new Event('x').timeStamp === performance.now()")
            .expect("compare an event timestamp with performance.now()"),
        "true"
    );
    let stamp = number(&mut engine, "new Event('x').timeStamp");
    assert!(
        stamp > 16.0 && stamp < 17.0,
        "the timestamp follows the frame clock, got {stamp}"
    );
}

#[test]
fn a_real_clock_page_parks_on_next_wakeup_and_pumps_what_came_due() {
    let mut engine = real_engine();
    // Nothing scheduled: no timer reason to wake at all.
    assert_eq!(engine.next_wakeup(), None);

    eval(
        &mut engine,
        &format!(
            "globalThis.fired = 0; setTimeout(() => {{ globalThis.fired++; }}, {});",
            NOT_YET.as_millis()
        ),
    );

    let wakeup = engine.next_wakeup().expect("a timer is pending");
    assert!(
        wakeup > Duration::ZERO && wakeup <= NOT_YET,
        "unexpected park interval {wakeup:?}"
    );
    // Not due yet, so a pump before the deadline must run nothing.
    assert_eq!(engine.pump_due_timers(16), 0);
    assert_eq!(number(&mut engine, "globalThis.fired"), 0.0);

    std::thread::sleep(NOT_YET);
    assert_eq!(engine.pump_due_timers(16), 1);
    assert_eq!(number(&mut engine, "globalThis.fired"), 1.0);
    // The one-shot is gone, so the thread has nothing left to park for.
    assert_eq!(engine.next_wakeup(), None);
}

#[test]
fn run_event_loop_cannot_drive_a_real_clock() {
    // It advances the clock to each deadline, which a real clock ignores. This
    // is why a live page uses `pump_due_timers`; the assertion pins the reason
    // so the two drive paths are not quietly swapped.
    let mut engine = real_engine();
    eval(
        &mut engine,
        "globalThis.fired = 0; setTimeout(() => { globalThis.fired++; }, 3600000);",
    );
    assert_eq!(engine.run_event_loop(16), 0);
    assert_eq!(number(&mut engine, "globalThis.fired"), 0.0);
    assert!(engine.next_wakeup().is_some());
}

#[test]
fn a_pump_bounded_by_max_tasks_keeps_the_rest_for_the_next_pump() {
    let mut engine = real_engine();
    eval(
        &mut engine,
        r#"
            globalThis.fired = 0;
            for (let i = 0; i < 3; i++) {
                setTimeout(() => { globalThis.fired++; }, 1);
            }
        "#,
    );
    std::thread::sleep(Duration::from_millis(10));

    assert_eq!(engine.pump_due_timers(2), 2);
    assert_eq!(number(&mut engine, "globalThis.fired"), 2.0);
    // The timer heap is empty now — it handed all three over when they fired —
    // so only the ready queue knows work remains. A page that parked on the
    // heap alone would sleep through it, so the wakeup has to be zero.
    assert_eq!(engine.next_wakeup(), Some(Duration::ZERO));

    // The handler the bound held back is still queued, ahead of later work.
    assert_eq!(engine.pump_due_timers(2), 1);
    assert_eq!(number(&mut engine, "globalThis.fired"), 3.0);
    assert_eq!(engine.next_wakeup(), None);
}

#[test]
fn a_zero_delay_timer_chain_cannot_spin_inside_one_pump() {
    // Each pump sweeps the heap once. A handler that re-arms at zero delay is
    // due immediately, so a pump that re-read the heap would never return and
    // would starve the rest of the page thread.
    let mut engine = real_engine();
    eval(
        &mut engine,
        r#"
            globalThis.fired = 0;
            function again() { globalThis.fired++; setTimeout(again, 0); }
            setTimeout(again, 0);
        "#,
    );
    std::thread::sleep(Duration::from_millis(5));

    assert_eq!(engine.pump_due_timers(64), 1);
    assert_eq!(number(&mut engine, "globalThis.fired"), 1.0);
    // The replacement is already due, so the thread comes straight back.
    assert_eq!(engine.next_wakeup(), Some(Duration::ZERO));
    assert_eq!(engine.pump_due_timers(64), 1);
    assert_eq!(number(&mut engine, "globalThis.fired"), 2.0);
}

#[test]
fn clear_timeout_aborts_a_timer_that_fired_but_has_not_been_called() {
    // The canceller has the earlier deadline, so it is queued ahead of its
    // victim and both come due in one sweep: `A` runs, cancels `b`, and `b`'s
    // handler is still sitting in the ready queue uncalled. HTML checks the map
    // of active timers when the timer's task runs, so `B` must be dropped.
    //
    // (Equal delays would NOT abort: `b` would be queued first and run before
    // the cancel, exactly as in every browser.)
    let mut engine = real_engine();
    eval(
        &mut engine,
        r#"
            globalThis.ran = '';
            const b = setTimeout(() => { globalThis.ran += 'B'; }, 40);
            setTimeout(() => { globalThis.ran += 'A'; clearTimeout(b); }, 1);
        "#,
    );
    std::thread::sleep(Duration::from_millis(60));

    // One callback ran; the cancelled one was dropped, not counted.
    assert_eq!(engine.pump_due_timers(16), 1);
    assert_eq!(eval(&mut engine, "globalThis.ran"), "A");
    assert_eq!(engine.next_wakeup(), None);
}

#[test]
fn an_equal_delay_cancel_does_not_reach_back_and_abort_an_earlier_task() {
    // The other side of the window, and the behaviour browsers actually have:
    // `b` was queued first, so it runs before the cancel ever happens.
    let mut engine = real_engine();
    eval(
        &mut engine,
        r#"
            globalThis.ran = '';
            const b = setTimeout(() => { globalThis.ran += 'B'; }, 1);
            setTimeout(() => { globalThis.ran += 'A'; clearTimeout(b); }, 1);
        "#,
    );
    std::thread::sleep(Duration::from_millis(20));

    assert_eq!(engine.pump_due_timers(16), 2);
    assert_eq!(eval(&mut engine, "globalThis.ran"), "BA");
}

#[test]
fn clear_interval_aborts_a_firing_that_is_already_queued() {
    // Same window, the repeating case: the interval re-arms in the heap when it
    // fires, so cancelling it must stop both the queued call and the next one.
    // The canceller is given the earlier deadline so it genuinely gets there
    // first.
    let mut engine = real_engine();
    eval(
        &mut engine,
        r#"
            globalThis.ticks = 0;
            globalThis.id = setInterval(() => { globalThis.ticks++; }, 40);
            setTimeout(() => { clearInterval(globalThis.id); }, 1);
        "#,
    );
    std::thread::sleep(Duration::from_millis(60));

    // Only the canceller ran; the interval's queued firing was aborted.
    assert_eq!(engine.pump_due_timers(16), 1);
    assert_eq!(number(&mut engine, "globalThis.ticks"), 0.0);

    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(engine.pump_due_timers(16), 0);
    assert_eq!(number(&mut engine, "globalThis.ticks"), 0.0);
}

#[test]
fn two_queued_firings_of_one_interval_both_run() {
    // An interval re-arms under the SAME id, so the deferral window has to
    // count outstanding firings rather than merely flag the id. With a flag the
    // second firing is silently swallowed: it would be declined as "cancelled".
    let mut engine = real_engine();
    eval(
        &mut engine,
        r#"
            globalThis.ticks = 0;
            setInterval(() => { globalThis.ticks++; }, 1);
        "#,
    );
    std::thread::sleep(Duration::from_millis(10));

    // Fire it once, but invoke nothing: the firing stays deferred.
    assert_eq!(engine.pump_due_timers(0), 0);
    std::thread::sleep(Duration::from_millis(10));
    // A second firing of the same id is now outstanding alongside the first.
    assert_eq!(engine.pump_due_timers(16), 2);
    assert_eq!(number(&mut engine, "globalThis.ticks"), 2.0);
}

#[test]
fn a_timer_cancelled_before_it_fires_is_still_a_plain_no_op() {
    // The ordinary path must keep working: the abort window is an addition, not
    // a replacement for cancelling a timer that is still in the heap.
    let mut engine = real_engine();
    eval(
        &mut engine,
        r#"
            globalThis.ran = 0;
            const id = setTimeout(() => { globalThis.ran++; }, 1);
            clearTimeout(id);
            clearTimeout(id);
        "#,
    );
    std::thread::sleep(Duration::from_millis(10));

    assert_eq!(engine.pump_due_timers(16), 0);
    assert_eq!(number(&mut engine, "globalThis.ran"), 0.0);
    assert_eq!(engine.next_wakeup(), None);
}

#[test]
fn a_page_never_parks_with_a_promise_reaction_still_owed() {
    // V8's microtask queue is not introspectable and the timer heap knows
    // nothing about it, so a driver that consulted only the heap would park on
    // `None` and leave every `.then` after a script entry unrun for as long as
    // the page happened to have no timer.
    let mut engine = real_engine();
    engine
        .evaluate("globalThis.x = 0; Promise.resolve().then(() => { globalThis.x = 1; });")
        .expect("queue a promise reaction");
    assert_eq!(engine.next_wakeup(), Some(Duration::ZERO));

    engine.run_microtasks();
    assert_eq!(number(&mut engine, "globalThis.x"), 1.0);
    // Paid off, and nothing else pending.
    assert_eq!(engine.next_wakeup(), None);
}

#[test]
fn a_real_clock_page_is_told_when_a_rendering_opportunity_is_owed() {
    // A `requestAnimationFrame` callback carries no deadline, and on a real
    // clock nothing produces a frame cadence on its own (`tick_frame`'s advance
    // is a no-op there), so without a paced wakeup the page would park forever.
    // The exact pacing arithmetic is pinned deterministically on the virtual
    // clock in `tenta_js::event_loop::driver`; this checks the engine wires the
    // two halves of the contract together.
    let mut engine = real_engine();
    eval(&mut engine, "globalThis.frames = 0;");
    assert_eq!(engine.next_wakeup(), None);
    assert!(!engine.frame_due());

    eval(
        &mut engine,
        r#"
            globalThis.again = () => {
                globalThis.frames++;
                requestAnimationFrame(globalThis.again);
            };
            requestAnimationFrame(globalThis.again);
        "#,
    );

    // An opportunity is owed within one frame of the loop origin. Whether it is
    // due *yet* depends only on how long the engine took to get here, so park
    // for whatever it reports rather than assuming.
    for expected in 1..=2 {
        let wakeup = engine
            .next_wakeup()
            .expect("a rendering opportunity is owed");
        assert!(
            wakeup <= Duration::from_millis(17),
            "a frame should be paced at ~16.67 ms, got {wakeup:?}"
        );
        std::thread::sleep(wakeup);
        assert!(engine.frame_due(), "the wakeup elapsed but no frame is due");

        engine.run_animation_frame();
        assert_eq!(
            number(&mut engine, "globalThis.frames"),
            f64::from(expected)
        );
    }
}
