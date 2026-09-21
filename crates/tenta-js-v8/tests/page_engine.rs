//! The V8 engine as a page drives it: through the engine boundary
//! ([`tenta_js::engine`]) alone.
//!
//! Everything here goes through a `Box<dyn PageEngine>`, so what is under test
//! is the contract the page layer depends on rather than the inherent `Engine`
//! methods underneath it. The engine-independent half of that contract is
//! pinned by `tenta-js/tests/engine_trait.rs` against a stub; this is the V8
//! backend's end of it.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use tenta_core::NodeId;
use tenta_core::TimerRule;
use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_html::QualName;
use tenta_js::bindings::host::SharedDocument;
use tenta_js::engine::{
    EngineClock, LifecycleEvent, PageEngine, PageEngineConfig, PageEngineFactory, ScriptErrorKind,
    ScriptKind,
};
use tenta_js_v8::V8EngineFactory;
use tenta_net::CookieJar;

/// `<html><body><div id="target"></div></body></html>`, and the id of that
/// `<div>` so a test can assert on the tree from the Rust side.
fn test_document() -> (SharedDocument, NodeId) {
    let mut document = Document::new();
    let html = document.create_html_element(atoms::HTML);
    let body = document.create_html_element(atoms::BODY);
    let div = document.create_html_element(atoms::DIV);

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
    document
        .mutate(Mutation::Append {
            parent: body,
            child: div,
        })
        .expect("append <div>");
    document
        .mutate(Mutation::SetAttribute {
            element: div,
            name: QualName::new(atoms::EMPTY, atoms::ID),
            value: "target".to_owned(),
        })
        .expect("set id attribute");

    (Rc::new(RefCell::new(document)), div)
}

/// The configuration a deterministic harness asks for: a virtual clock it steps
/// itself, no heap cap, no script deadline.
fn config() -> PageEngineConfig {
    PageEngineConfig {
        timer_rule: TimerRule::default(),
        clock: EngineClock::Virtual,
        max_heap_bytes: None,
        script_timeout: None,
        locale: "en-US".to_owned(),
        timezone: None,
        cookies: CookieJar::new(),
    }
}

/// Builds an engine over `document` the way the assembly crate would, and hands
/// it back as the trait object a page holds.
fn build_engine(document: &SharedDocument, config: &PageEngineConfig) -> Box<dyn PageEngine> {
    V8EngineFactory
        .create(document.clone(), config)
        .expect("the V8 factory builds an engine over a document")
}

/// As [`build_engine`], with the deterministic harness configuration.
fn engine_over(document: &SharedDocument) -> Box<dyn PageEngine> {
    build_engine(document, &config())
}

/// The harness configuration with a script deadline on it — the only way a page
/// sets one, since `PageEngine` takes a timeout from its creation configuration
/// and not per call.
fn config_with_deadline(deadline: Duration) -> PageEngineConfig {
    PageEngineConfig {
        script_timeout: Some(deadline),
        ..config()
    }
}

#[test]
fn the_engine_and_the_page_share_one_document() {
    // The page's whole reason for holding a `SharedDocument` is that the
    // parser and running script write into the same tree; the handle the engine
    // reports has to be that one, not a copy of it.
    let (document, _target) = test_document();
    let engine = engine_over(&document);
    assert!(
        Rc::ptr_eq(&engine.document(), &document),
        "the engine reports the page's document, not a fresh one"
    );
}

#[test]
fn a_script_run_through_the_boundary_mutates_the_shared_document() {
    let (document, _target) = test_document();
    let mut engine = engine_over(&document);

    engine
        .run_script("globalThis.marker = 'kept';", ScriptKind::Classic)
        .expect("the first script runs");
    engine
        .run_script(
            "const p = document.createElement('p'); \
             p.setAttribute('id', marker); \
             document.body.appendChild(p);",
            ScriptKind::Classic,
        )
        .expect("the second script runs");

    let doc = document.borrow();
    let body = doc.body().expect("<body> exists");
    let appended = doc
        .element_children(body)
        .last()
        .expect("the script appended an element to <body>");
    assert_eq!(
        doc.tag(appended),
        Some(atoms::P),
        "the script's element is in the tree"
    );
    assert_eq!(
        doc.element_id(appended),
        Some("kept"),
        "the second run saw the global the first one set"
    );
}

#[test]
fn each_way_a_script_fails_earns_its_own_kind() {
    let (document, _target) = test_document();
    // The deadline comes from the creation configuration, so this one engine
    // carries it through every run below — including the ones expected to
    // finish long before it.
    let mut engine = build_engine(&document, &config_with_deadline(Duration::from_millis(200)));

    // Source that never compiled.
    let error = engine
        .run_script("function (", ScriptKind::Classic)
        .expect_err("malformed source does not run");
    assert_eq!(error.kind, ScriptErrorKind::Syntax, "got {error:?}");

    // Source that compiled and then threw.
    let error = engine
        .run_script("throw new Error('boom');", ScriptKind::Classic)
        .expect_err("a thrown error does not run to completion");
    assert_eq!(error.kind, ScriptErrorKind::Uncaught, "got {error:?}");
    assert!(error.message.contains("boom"), "got {error:?}");

    // Source the engine itself stopped, because the page set a deadline.
    let error = engine
        .run_script("while (true) {}", ScriptKind::Classic)
        .expect_err("a runaway script is stopped");
    assert_eq!(error.kind, ScriptErrorKind::Terminated, "got {error:?}");
    assert!(error.message.contains("deadline"), "got {error:?}");

    // The isolate survives a stopped run: prove it by making it change the tree
    // the test can see.
    engine
        .run_script(
            "document.body.classList.add('still-usable');",
            ScriptKind::Classic,
        )
        .expect("the engine is usable after a terminated run");
    let doc = document.borrow();
    let body = doc.body().expect("<body> exists");
    assert!(doc.has_class(body, "still-usable"));
}

#[test]
fn a_script_timer_waits_for_the_driver_to_pump_it() {
    let (document, target) = test_document();
    let mut engine = engine_over(&document);

    engine
        .run_script(
            "const el = document.querySelector('#target'); \
             setTimeout(() => { el.setAttribute('id', 'fired'); }, 0);",
            ScriptKind::Classic,
        )
        .expect("the scheduling script runs");

    assert_eq!(
        document.borrow().element_id(target),
        Some("target"),
        "nothing runs until the loop is pumped"
    );
    assert_eq!(
        engine.next_wakeup(),
        Some(Duration::ZERO),
        "a due timer is work owed now"
    );

    assert_eq!(engine.pump_due_timers(10), 1, "one timer callback ran");
    assert_eq!(document.borrow().element_id(target), Some("fired"));
    assert_eq!(
        engine.next_wakeup(),
        None,
        "nothing is owed once the timer has run"
    );
}

#[test]
fn trim_memory_advances_the_gc_epoch_and_clears_a_collected_wrapper() {
    let (document, target) = test_document();
    let mut engine = engine_over(&document);

    engine
        .run_script(
            "globalThis.__wrapped = document.querySelector('#target');",
            ScriptKind::Classic,
        )
        .expect("the wrapping script runs");
    assert!(
        document.borrow().has_wrapper(target),
        "creating the wrapper marks the node"
    );

    // Drop the only reference, and do it from a second script so no frame of
    // the script that created the wrapper is still on the stack when the
    // collection runs.
    engine
        .run_script("globalThis.__wrapped = undefined;", ScriptKind::Classic)
        .expect("the dropping script runs");

    let before = engine.gc_epoch();
    engine.trim_memory();
    let after = engine.gc_epoch();
    assert!(
        after > before,
        "a forced collection must advance the epoch: {before} -> {after}"
    );
    assert!(
        !document.borrow().has_wrapper(target),
        "the collected wrapper's node must be sweepable"
    );
}

/// A loop V8 reaches an interrupt check inside many times while it runs.
///
/// This is what makes the stray-termination tests below real: a terminate that
/// is pending when the script starts aborts it at one of those checks, and a
/// terminate issued *during* it does the same. A trivial `1 + 1` proves nothing
/// — V8 can complete it without ever looking at the termination request, so a
/// test built on it would pass with the bug still in place. The loop is short
/// enough to finish in milliseconds when nothing terminates it.
const INTERRUPTIBLE_SCRIPT: &str =
    "let sum = 0; for (let i = 0; i < 10_000_000; i++) { sum += i; } sum;";

/// Terminates the engine from another thread while nothing is running on it,
/// and returns only once the request has landed.
///
/// This is the "between scripts" window `EngineWatchdog::terminate` documents
/// ("best effort and idempotent: the engine may be between scripts, or already
/// stopped"): the request is a real one, through the engine's own watchdog, but
/// no running script is there to consume it, so it sits on the isolate until
/// the next entry — the state the entry-start clear exists for. The terminate
/// still has to be issued *before* the entry under test starts, so it cannot
/// arrive late and kill the wrong thing: `IsolateHandle::terminate_execution`
/// is synchronous, and joining the issuing thread makes that ordering explicit
/// rather than timing-dependent.
fn terminate_while_idle(engine: &mut Box<dyn PageEngine>) {
    let watchdog = engine.watchdog().expect("a V8 isolate can be interrupted");
    std::thread::spawn(move || watchdog.terminate())
        .join()
        .expect("the terminating thread finished");
}

/// Whether `class` is on the document's `<body>`.
fn body_has_class(document: &SharedDocument, class: &str) -> bool {
    let doc = document.borrow();
    let body = doc.body().expect("<body> exists");
    doc.has_class(body, class)
}

/// Runs `body` on a thread of its own and returns what it produced, failing
/// rather than hanging if it does not finish.
///
/// The engine is not `Send`, so a test that terminates from another thread has
/// to build its document and engine on a worker; the bounded receive is the
/// outer timeout, so an engine that spins instead of returning fails the suite
/// rather than blocking it.
fn run_bounded<T: Send + 'static>(body: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    let runner = std::thread::spawn(move || {
        let _ = tx.send(body());
    });
    let result = rx
        .recv_timeout(Duration::from_secs(30))
        .expect("the engine work must finish; a hang here is the bug");
    runner.join().expect("the runner thread finished");
    result
}

#[test]
fn the_watchdog_stops_a_runaway_script_from_another_thread() {
    // The engine is not `Send`, so the run happens on a thread that builds its
    // own document and engine, and this thread waits with a bound: a watchdog
    // that never fires must fail the test rather than hang the suite.
    let (tx, rx) = std::sync::mpsc::channel();
    let runner = std::thread::spawn(move || {
        let (document, _target) = test_document();
        // A deadline far longer than the watchdog's, so the terminate below is
        // what stops this script. It is also the safety net: without it a
        // broken watchdog would spin here forever.
        let mut engine = build_engine(&document, &config_with_deadline(Duration::from_secs(3)));
        let watchdog = engine.watchdog().expect("a V8 isolate can be interrupted");
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            watchdog.terminate();
        });

        let started = Instant::now();
        let error = engine
            .run_script("while (true) {}", ScriptKind::Classic)
            .expect_err("the watchdog stopped the script");
        // The kill must leave the isolate usable, and the entry-start clear must
        // not have blunted it: this script was never terminated and must run.
        let follow_up = engine
            .run_script(INTERRUPTIBLE_SCRIPT, ScriptKind::Classic)
            .is_ok();
        let _ = tx.send((error.kind, error.message, started.elapsed(), follow_up));
    });

    let (kind, message, elapsed, follow_up) = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the watchdog must stop the script; a hang here is the bug");
    assert_eq!(kind, ScriptErrorKind::Terminated, "got {message:?}");
    assert!(
        elapsed < Duration::from_secs(1),
        "the watchdog stopped it early, not the 3 s deadline: {elapsed:?}"
    );
    assert!(follow_up, "the isolate is usable after a terminated run");
    runner.join().expect("the runner thread finished");
}

// The four tests below pin the entry-start clear one way in at a time. Each
// arranges a real terminate through the engine's watchdog from another thread
// while nothing runs on the isolate, then requires the next script — the one
// the terminate was never aimed at — to run. Without the clear they all fail:
// V8's termination is a pending request on the isolate, invisible to
// `is_execution_terminating()` until running code handles it, so the next entry
// inherits it and its script dies.

#[test]
fn a_stray_termination_does_not_abort_the_next_script() {
    // The `run_inner` entry: a top-level script through the page boundary.
    let ran = run_bounded(|| {
        let (document, _target) = test_document();
        let mut engine = engine_over(&document);
        terminate_while_idle(&mut engine);
        engine
            .run_script(INTERRUPTIBLE_SCRIPT, ScriptKind::Classic)
            .map_err(|error| (error.kind, error.message))
    });
    ran.expect("a termination issued while the engine was idle must not abort the next script");
}

#[test]
fn a_stray_termination_does_not_abort_the_next_microtask() {
    // The `checkpoint` entry, on its own: `run_microtasks` drives no timers and
    // no callbacks, so the clear it needs is the one in the checkpoint.
    let ran = run_bounded(|| {
        let (document, _target) = test_document();
        let mut engine = engine_over(&document);
        engine
            .run_script(
                "queueMicrotask(() => { document.body.classList.add('microtask-ran'); });",
                ScriptKind::Classic,
            )
            .expect("the queueing script runs");
        terminate_while_idle(&mut engine);

        engine.run_microtasks();
        body_has_class(&document, "microtask-ran")
    });
    assert!(ran, "a stray termination must not abort the next microtask");
}

#[test]
fn a_stray_termination_does_not_abort_the_next_timer_callback() {
    // The `pump_due_timers` entry. Note what makes this *not* the same test as
    // "terminate during a timer callback": V8 consumes a terminate that a
    // running script reaches an interrupt check for, so a callback that is
    // already spinning when the terminate arrives is killed by it and leaves
    // nothing pending. The state that poisons a later entry is a request no
    // running script consumed, which is what `terminate_while_idle` arranges.
    let ran = run_bounded(|| {
        let (document, _target) = test_document();
        let mut engine = engine_over(&document);
        engine
            .run_script(
                "setTimeout(() => { document.body.classList.add('timer-ran'); }, 0);",
                ScriptKind::Classic,
            )
            .expect("the scheduling script runs");
        terminate_while_idle(&mut engine);

        let pumped = engine.pump_due_timers(10);
        (pumped, body_has_class(&document, "timer-ran"))
    });
    assert_eq!(ran.0, 1, "the pump ran the due callback");
    assert!(
        ran.1,
        "a stray termination must not abort the next timer callback"
    );
}

#[test]
fn a_stray_termination_does_not_abort_the_next_animation_frame_callback() {
    // The `invoke_handler` entry with nothing in front of it: unlike
    // `pump_due_timers`, `run_animation_frame` reaches its first callback
    // before any microtask checkpoint, so this one is covered by the clear in
    // `invoke_handler` and by nothing else.
    let ran = run_bounded(|| {
        let (document, _target) = test_document();
        let mut engine = engine_over(&document);
        engine
            .run_script(
                "requestAnimationFrame(() => { document.body.classList.add('frame-ran'); });",
                ScriptKind::Classic,
            )
            .expect("the registering script runs");
        terminate_while_idle(&mut engine);

        engine.run_animation_frame();
        body_has_class(&document, "frame-ran")
    });
    assert!(
        ran,
        "a stray termination must not abort the next animation-frame callback"
    );
}

#[test]
fn a_terminate_during_a_timer_callback_stops_that_callback_and_no_later_script() {
    // The real kill on the `invoke_handler` path, and the check that the
    // entry-start clear does not blunt it: the terminate arrives *after* the
    // callback started, so nothing has cleared it away.
    //
    // It is deliberately not one of the regression tests above. V8 consumes a
    // terminate the running callback reaches an interrupt check for, clearing
    // both the request and the exception, so the isolate is already clean by
    // the time the pump returns and this passes with or without the clear. What
    // it pins is the other half: the clear must still leave a terminate issued
    // inside an entry alone.
    let (ran, completed, follow_up) = run_bounded(|| {
        let (document, _target) = test_document();
        let mut engine = engine_over(&document);

        // The loop runs for seconds on its own, so the 50 ms terminate always
        // lands inside it — and it is finite, so a watchdog that never fires
        // ends the callback rather than hanging the suite. The class is added
        // only if the callback reaches its end, which makes its absence proof
        // that the terminate did stop the callback.
        engine
            .run_script(
                "setTimeout(() => { \
                     let sum = 0; \
                     for (let i = 0; i < 2_000_000_000; i++) { sum += i; } \
                     document.body.classList.add('callback-completed'); \
                 }, 0);",
                ScriptKind::Classic,
            )
            .expect("the scheduling script runs");

        let watchdog = engine.watchdog().expect("a V8 isolate can be interrupted");
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            watchdog.terminate();
        });

        // The callback starts spinning here, and the terminate lands inside it.
        let pumped = engine.pump_due_timers(10);
        let completed = body_has_class(&document, "callback-completed");
        let follow_up = engine
            .run_script(INTERRUPTIBLE_SCRIPT, ScriptKind::Classic)
            .is_ok();
        (pumped, completed, follow_up)
    });

    assert_eq!(
        ran, 1,
        "the pump entered the terminated callback exactly once"
    );
    assert!(
        !completed,
        "the terminate must have stopped the callback before its last statement"
    );
    assert!(follow_up, "a stopped callback leaves the isolate usable");
}

#[test]
fn a_terminated_timer_callback_ends_the_pump_before_the_next_runaway() {
    // The hole this pins: a page arms one watchdog deadline over one
    // `pump_due_timers` entry, and the pump used to run every callback that came
    // due in that sweep under it. The first callback's termination was consumed
    // by the call itself, the second callback's own start-of-entry clear wiped
    // the spent request, and the deadline that was meant to stop it had already
    // fired and could not fire again — so the second runaway was unkillable.
    //
    // Both callbacks therefore run a loop that is finite and seconds long rather
    // than `while (1)`: with the fix absent the second one still *returns*, and
    // the assertions below report it, instead of leaving a leaked worker thread
    // spinning where only `run_bounded`'s 30 s bound could notice.
    let (
        pumped,
        elapsed,
        first_entered,
        second_entered,
        second_completed,
        pumped2,
        second_entered_later,
        second_completed_later,
    ) = run_bounded(|| {
        let (document, _target) = test_document();
        let mut engine = engine_over(&document);
        engine
            .run_script(
                "setTimeout(() => { \
                     document.body.classList.add('first-entered'); \
                     let s = 0; \
                     for (let i = 0; i < 2_000_000_000; i++) { s += i; } \
                 }, 0); \
                 setTimeout(() => { \
                     document.body.classList.add('second-entered'); \
                     let s = 0; \
                     for (let i = 0; i < 2_000_000_000; i++) { s += i; } \
                     document.body.classList.add('second-completed'); \
                 }, 0);",
                ScriptKind::Classic,
            )
            .expect("the scheduling script runs");

        // The deadline both callbacks of the sweep were meant to share.
        let watchdog = engine.watchdog().expect("a V8 isolate can be interrupted");
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            watchdog.terminate();
        });

        let started = Instant::now();
        let pumped = engine.pump_due_timers(10);
        let elapsed = started.elapsed();
        let first_entered = body_has_class(&document, "first-entered");
        let second_entered = body_has_class(&document, "second-entered");
        let second_completed = body_has_class(&document, "second-completed");

        // Phase two: the terminated callback ended the pump, so the second one
        // is still queued, and a fresh arm and a fresh pump reach it. Under the
        // old behaviour there was nothing left to reach — the callback ran in
        // phase one, past the only deadline there was.
        let watchdog = engine.watchdog().expect("a V8 isolate can be interrupted");
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            watchdog.terminate();
        });
        let pumped2 = engine.pump_due_timers(10);
        (
            pumped,
            elapsed,
            first_entered,
            second_entered,
            second_completed,
            pumped2,
            body_has_class(&document, "second-entered"),
            body_has_class(&document, "second-completed"),
        )
    });

    assert_eq!(
        pumped, 1,
        "the pump entered the terminated callback once and stopped: \
         elapsed={elapsed:?}, first_entered={first_entered}, \
         second_entered={second_entered}, second_completed={second_completed}"
    );
    assert!(
        elapsed < Duration::from_secs(1),
        "the pump stopped at the deadline, not after the second runaway finished: {elapsed:?}"
    );
    assert!(
        first_entered,
        "the terminated callback did enter script before the deadline stopped it"
    );
    assert!(
        !second_entered,
        "the second callback of the same pump was not entered"
    );
    assert!(
        !second_completed,
        "the second callback of the same pump did not reach its end"
    );
    assert_eq!(
        pumped2, 1,
        "the deferred callback stayed queued and the next pump reached it"
    );
    assert!(
        second_entered_later,
        "the second callback was deferred, not dropped"
    );
    assert!(
        !second_completed_later,
        "and that pump's fresh deadline stopped it too"
    );
}

#[test]
fn a_terminated_frame_callback_ends_the_opportunity_before_the_next_runaway() {
    // The same hole on the rendering-opportunity side: `run_animation_frame` is
    // the entry a page arms, both callbacks registered before the frame come due
    // in that one call, and only the first of them was covered by the arm.
    let (
        elapsed,
        first_entered,
        second_entered,
        second_completed,
        second_entered_later,
        second_completed_later,
    ) = run_bounded(|| {
        let (document, _target) = test_document();
        let mut engine = engine_over(&document);
        engine
            .run_script(
                "requestAnimationFrame(() => { \
                     document.body.classList.add('first-frame-entered'); \
                     let s = 0; \
                     for (let i = 0; i < 2_000_000_000; i++) { s += i; } \
                 }); \
                 requestAnimationFrame(() => { \
                     document.body.classList.add('second-frame-entered'); \
                     let s = 0; \
                     for (let i = 0; i < 2_000_000_000; i++) { s += i; } \
                     document.body.classList.add('second-frame-completed'); \
                 });",
                ScriptKind::Classic,
            )
            .expect("the registering script runs");

        let watchdog = engine.watchdog().expect("a V8 isolate can be interrupted");
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            watchdog.terminate();
        });

        let started = Instant::now();
        engine.run_animation_frame();
        let elapsed = started.elapsed();
        let first_entered = body_has_class(&document, "first-frame-entered");
        let second_entered = body_has_class(&document, "second-frame-entered");
        let second_completed = body_has_class(&document, "second-frame-completed");

        // The callback the terminated one displaced stays on the ready-frame
        // queue, and the next opportunity runs it — under an arm of its own.
        let watchdog = engine.watchdog().expect("a V8 isolate can be interrupted");
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            watchdog.terminate();
        });
        engine.run_animation_frame();
        (
            elapsed,
            first_entered,
            second_entered,
            second_completed,
            body_has_class(&document, "second-frame-entered"),
            body_has_class(&document, "second-frame-completed"),
        )
    });

    assert!(
        elapsed < Duration::from_secs(1),
        "the opportunity stopped at the deadline, not after the second runaway finished: \
         elapsed={elapsed:?}, first_entered={first_entered}, \
         second_entered={second_entered}, second_completed={second_completed}"
    );
    assert!(
        first_entered,
        "the terminated frame callback did enter script before the deadline stopped it"
    );
    assert!(
        !second_entered,
        "the second callback of the same frame was not entered"
    );
    assert!(
        !second_completed,
        "the second callback of the same frame did not reach its end"
    );
    assert!(
        second_entered_later,
        "the second callback was deferred to the next frame, not dropped"
    );
    assert!(
        !second_completed_later,
        "and that frame's fresh deadline stopped it too"
    );
}

/// Records, for whichever lifecycle events the page sees, `type | target===document
/// | isTrusted | bubbles | cancelable` into an attribute on `#target`.
///
/// The page can only be asked what it saw one value at a time — there is no
/// `evaluate`, so nothing crosses back — and the shared document is what the
/// Rust side can read.
const RECORD_LIFECYCLE: &str = "\
const el = document.querySelector('#target'); \
const see = (name) => (e) => el.setAttribute(name, \
    [e.type, e.target === document, e.isTrusted, e.bubbles, e.cancelable].join('|')); \
document.addEventListener('DOMContentLoaded', see('data-dcl')); \
window.addEventListener('load', see('data-load'));";

#[test]
fn dispatch_lifecycle_fires_both_events_at_their_targets_with_their_flags() {
    let (document, target) = test_document();
    let mut engine = engine_over(&document);
    engine
        .run_script(RECORD_LIFECYCLE, ScriptKind::Classic)
        .expect("the registering script runs");

    engine.dispatch_lifecycle(LifecycleEvent::DomContentLoaded);
    engine.dispatch_lifecycle(LifecycleEvent::Load);

    let doc = document.borrow();
    // HTML "the end": `DOMContentLoaded` is fired at the Document with its
    // bubbles attribute initialised to true, and nothing makes it cancelable.
    assert_eq!(
        doc.attribute_str(target, "data-dcl"),
        Some("DOMContentLoaded|true|true|true|false"),
        "DOMContentLoaded is a trusted, bubbling event at the document"
    );
    // HTML "the load event": fired at the Window with the legacy target
    // override flag set, which makes `target` the window's document while the
    // listener table stays the window's — hence the window listener above.
    assert_eq!(
        doc.attribute_str(target, "data-load"),
        Some("load|true|true|false|false"),
        "load is trusted, does not bubble, and reports the document as its target"
    );
}

#[test]
fn a_lifecycle_dispatch_leaves_no_work_owed() {
    // The page layer's settle loop fires the events and evaluates its own state
    // in the same turn, so a dispatch that left work behind would be read as a
    // page that is still loading.
    let (document, _target) = test_document();
    let mut engine = engine_over(&document);
    engine
        .run_script(RECORD_LIFECYCLE, ScriptKind::Classic)
        .expect("the registering script runs");

    engine.dispatch_lifecycle(LifecycleEvent::DomContentLoaded);
    engine.dispatch_lifecycle(LifecycleEvent::Load);

    // Every script entry leaves a microtask checkpoint owed, dispatching
    // included, so drain it before asking what the dispatch left behind — the
    // question is whether the dispatch queued work of its own.
    engine.run_microtasks();
    assert_eq!(
        engine.pump_due_timers(10),
        0,
        "the dispatch queued no timer"
    );
    assert_eq!(engine.next_wakeup(), None, "nothing is owed afterwards");
}

#[test]
fn a_load_listener_added_by_a_domcontentloaded_handler_still_runs() {
    // The order the page layer fires in is what makes this work: a handler that
    // registers for the next event is how a real page does post-parse work, and
    // it only sees `load` if `DOMContentLoaded` completed first.
    let (document, target) = test_document();
    let mut engine = engine_over(&document);
    engine
        .run_script(
            "const el = document.querySelector('#target'); \
             document.addEventListener('DOMContentLoaded', () => { \
                 window.addEventListener('load', () => el.setAttribute('data-late', 'ran')); \
             });",
            ScriptKind::Classic,
        )
        .expect("the registering script runs");

    engine.dispatch_lifecycle(LifecycleEvent::DomContentLoaded);
    assert_eq!(
        document.borrow().attribute_str(target, "data-late"),
        None,
        "the late listener has not run yet"
    );

    engine.dispatch_lifecycle(LifecycleEvent::Load);
    assert_eq!(
        document.borrow().attribute_str(target, "data-late"),
        Some("ran"),
        "a listener registered during DOMContentLoaded still receives load"
    );
}

#[test]
fn the_factory_refuses_a_timezone_it_cannot_apply() {
    // A per-page timezone needs process-global ICU configuration V8 only
    // accepts before the platform is initialised, so the honest answer is an
    // error the host turns into an unsupported-feature report — never an engine
    // that quietly reports UTC.
    let (document, _target) = test_document();
    let refused = PageEngineConfig {
        timezone: Some("Europe/Warsaw".to_owned()),
        ..config()
    };
    let error = V8EngineFactory
        .create(document, &refused)
        .err()
        .expect("the V8 backend must not accept a timezone it cannot apply");
    assert!(error.contains("Europe/Warsaw"), "got {error}");
}

#[test]
fn the_factory_is_shareable_across_page_threads() {
    // It is stored in browser-wide configuration, which page threads share,
    // while the engines it builds stay on one thread each.
    fn assert_send_sync<T: Send + Sync + 'static>() {}
    assert_send_sync::<V8EngineFactory>();
    assert_send_sync::<Box<dyn PageEngineFactory>>();
}
