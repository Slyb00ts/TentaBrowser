//! Several V8 isolates hosted by one thread (`crate::IsolateEntry`).
//!
//! An engine method assumes its own isolate is the thread's current one, which
//! only held while `Isolate::new` left it entered at construction. Two engines
//! on one thread break that: the second engine's entries run while the *first*
//! isolate is current, and `ContextScope::new` panics with "…and Context do not
//! belong to the same Isolate". These tests are the differential test the
//! `unsafe` rules require for the RAII entry guard — every shape here panics
//! before it, on one thread, with no page pool and no `catch_unwind` between
//! the engine and the test.

use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

use tenta_core::Origin;
use tenta_js::engine::PageEngine;
use tenta_js::event_loop::ProvenanceSink;
use tenta_js_v8::Engine;

/// Two engines on one thread, each given a global of its own and read back.
///
/// The two globals are the non-vacuity of "two isolates": a single shared
/// context would answer with whichever value was written last, so the
/// assertions would catch a backend that quietly reused one isolate.
fn two_isolates(run_older_first: bool, drop_older_first: bool) {
    let mut older = Engine::new();
    let mut newer = Engine::new();

    // The shape that panicked before the guard: an entry on the engine whose
    // isolate is *not* the thread's current one. `Isolate::new` left the newer
    // isolate entered, so this order is the one that reaches `ContextScope`
    // with two live isolates.
    let (first, first_name) = if run_older_first {
        (&mut older, "older")
    } else {
        (&mut newer, "newer")
    };
    first
        .evaluate(&format!("globalThis.tag = {first_name:?}; 1 + 1"))
        .unwrap_or_else(|error| panic!("an entry on the {first_name} engine failed: {error}"));

    let (second, second_name) = if run_older_first {
        (&mut newer, "newer")
    } else {
        (&mut older, "older")
    };
    second
        .evaluate(&format!("globalThis.tag = {second_name:?}; globalThis.tag"))
        .unwrap_or_else(|error| panic!("an entry on the {second_name} engine failed: {error}"));

    assert_eq!(
        older.evaluate("globalThis.tag").expect("a second entry"),
        "older",
        "the older engine kept its own global"
    );
    assert_eq!(
        newer.evaluate("globalThis.tag").expect("a second entry"),
        "newer",
        "the newer engine kept its own global"
    );

    // Both drop orders are load-bearing. `OwnedIsolate::drop` asserts the
    // isolate being disposed is the thread's current one, and disposal happens
    // between entries, so the engine dropped first is the one that has to enter
    // its own isolate rather than inherit the other's.
    if drop_older_first {
        drop(older);
        drop(newer);
    } else {
        drop(newer);
        drop(older);
    }
}

#[test]
fn two_engines_run_oldest_first_and_drop_oldest_first() {
    two_isolates(true, true);
}

#[test]
fn two_engines_run_oldest_first_and_drop_newest_first() {
    two_isolates(true, false);
}

#[test]
fn two_engines_run_newest_first_and_drop_oldest_first() {
    two_isolates(false, true);
}

#[test]
fn two_engines_run_newest_first_and_drop_newest_first() {
    two_isolates(false, false);
}

/// A [`ProvenanceSink`] that panics while it is armed.
///
/// The sink is the only public way to run *Rust* code inside an engine entry:
/// the loop calls it from `run_event_loop`'s own frame, not from a V8 callback,
/// so a panic here unwinds through the entry's `IsolateEntry` guard instead of
/// crossing the `extern "C"` boundary every binding callback would abort on.
///
/// Armed only around the one entry, because the same sink is consulted from
/// inside `setTimeout`'s binding — where a panic would abort the process rather
/// than unwind — and from the engine's teardown.
struct Panicked {
    armed: Cell<bool>,
}

impl Panicked {
    fn arm(&self) {
        self.armed.set(true);
    }

    fn disarm(&self) {
        self.armed.set(false);
    }

    fn check(&self, what: &str) {
        assert!(
            !self.armed.get(),
            "the provenance sink was asked to record a {what} task while armed"
        );
    }
}

impl ProvenanceSink for Panicked {
    fn task_queued(&self, _origin: Origin) {
        self.check("queued");
    }

    fn task_finished(&self, _origin: Origin) {
        self.check("finished");
    }
}

/// A panic that unwinds out of a guarded entry must leave the guard's exit
/// having run.
///
/// The guard is RAII precisely because the engine boundary is wrapped in
/// `catch_unwind` and `panic = "unwind"` is pinned: a bare `enter()`/`exit()`
/// pair would leave the panic's own isolate current, and the *next* entry on
/// this thread — any engine's — would build its scopes on the wrong isolate.
///
/// Three facts are asserted, and the third is the one a missing exit trips:
///
/// 1. the panic escaped the entry rather than being swallowed by it;
/// 2. the next entry works, on the engine that panicked and on a second one;
/// 3. both engines can still be dropped — `Engine::drop` asserts that the
///    engine's isolate is not entered on this thread, which a guard that never
///    ran its `Drop` (or never popped) leaves true.
#[test]
fn a_panic_inside_an_entry_leaves_the_guard_exited() {
    let sink = Rc::new(Panicked {
        armed: Cell::new(false),
    });
    let mut panicking = Engine::new();
    // A second engine, so the thread really does host two isolates and the
    // unwind cannot be confused with "only one isolate ever existed".
    let mut bystander = Engine::new();
    panicking.set_provenance_sink(sink.clone());
    bystander.set_provenance_sink(sink.clone());

    // A background timer, queued while the sink is quiet: `evaluate` is a
    // top-level entry, so the loop's provenance is `Background` and the timer's
    // task is queued through the sink.
    panicking
        .evaluate("globalThis.ran = 0; setTimeout(() => { globalThis.ran = 1; }, 1);")
        .expect("scheduling a timer");

    // The entry under test. `run_event_loop` reaches the sink from Rust — the
    // timer is queued as a task before any callback is invoked — so the panic
    // unwinds through `run_event_loop`'s guard.
    sink.arm();
    let panicked = catch_unwind(AssertUnwindSafe(|| panicking.run_event_loop(10)));
    sink.disarm();
    assert!(
        panicked.is_err(),
        "the sink panicked inside the entry, so the entry must have unwound"
    );

    // The next entry on the same thread works, on both engines. A guard that
    // left the isolate entered would not fail here on its own — it would fail
    // at teardown — so this is the "and the thread is still usable" half.
    assert_eq!(
        panicking.evaluate("1 + 1").expect("a later entry"),
        "2",
        "the engine that panicked is still usable"
    );
    assert_eq!(
        bystander.evaluate("2 + 2").expect("a later entry"),
        "4",
        "the other engine on the thread is still usable"
    );

    // Disposal: `Engine::drop` asserts no entry of this engine's isolate is
    // live on this thread.
    drop(panicking);
    drop(bystander);
}
