//! Tests for the `AbortController` and `AbortSignal` bindings (P2 package (c)
//! slice 3).
//!
//! `AbortSignal` is the first event target that is neither a node nor the
//! window, which is the whole reason this slice adds a `TargetKey` variant: the
//! signal is a JS-owned host object (it shares the one `TAG_HOST` handle tag with
//! `URL`, `URLSearchParams` and `Headers`), so its listener entries cannot be
//! keyed by the tag and must not be keyed by the interface name — two signals are
//! two targets.
//!
//! The DOM Standard's abort algorithm is what the tests below pin: the abort
//! state *is* the abort reason, the `abort` event fires synchronously inside
//! `abort()`, a second `abort()` is a no-op, and the reason is one value that
//! `reason` reads back and `throwIfAborted()` re-throws by identity.
//!
//! They live here rather than under `crates/tenta-page/tests/` because they need
//! the V8 glue, and no V8 test may live there.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_js_v8::Engine;

/// A document with `<html><body></body></html>`; the abort tests need a
/// browsing context but no markup.
fn document() -> Rc<RefCell<Document>> {
    let mut d = Document::new();
    let html = d.create_html_element(atoms::HTML);
    let body = d.create_html_element(atoms::BODY);
    d.mutate(Mutation::Append {
        parent: d.document_node(),
        child: html,
    })
    .unwrap();
    d.mutate(Mutation::Append {
        parent: html,
        child: body,
    })
    .unwrap();
    Rc::new(RefCell::new(d))
}

fn eval(source: &str) -> String {
    let mut engine = Engine::with_document(document());
    engine
        .evaluate(source)
        .expect("script should evaluate without throwing")
}

// ---------------------------------------------------------------------------
// Installation and the constructor
// ---------------------------------------------------------------------------

#[test]
fn abort_controller_is_installed_as_a_global_interface() {
    assert_eq!(
        eval(
            r#"
            [
                typeof AbortController, typeof AbortSignal,
                new AbortController() instanceof AbortController,
                new AbortController().signal instanceof AbortSignal,
                new AbortController().signal instanceof EventTarget,
                Object.getPrototypeOf(AbortSignal.prototype) === EventTarget.prototype,
                Object.prototype.toString.call(new AbortController().signal),
            ].join('|');
        "#
        ),
        "function|function|true|true|true|true|[object AbortSignal]"
    );
}

#[test]
fn a_controller_signal_is_the_same_object_on_every_read() {
    // `[SameObject]`: the constructor makes the signal and every read answers
    // with the object it made, rather than with a fresh one over the same state.
    assert_eq!(
        eval(
            r#"
            const c = new AbortController();
            const a = c.signal;
            c.abort();
            const b = c.signal;
            String(a === b);
        "#
        ),
        "true"
    );
}

#[test]
fn an_abort_signal_declares_no_usable_constructor() {
    // `AbortSignal` has no `constructor` in the DOM Standard, so `new` throws.
    assert_eq!(
        eval(
            r#"
            try { new AbortSignal(); 'made'; } catch (e) { e.constructor.name; }
        "#
        ),
        "TypeError"
    );
}

// ---------------------------------------------------------------------------
// The abort algorithm
// ---------------------------------------------------------------------------

#[test]
fn abort_flips_aborted_and_installs_a_default_reason() {
    assert_eq!(
        eval(
            r#"
            const c = new AbortController();
            const s = c.signal;
            const before = [s.aborted, String(s.reason)];
            c.abort();
            [
                before.join(','),
                s.aborted,
                s.reason.name, s.reason.code,
                s.reason instanceof DOMException,
                s.reason === s.reason,
                typeof s.reason.message,
                s.reason.message.length > 0,
            ].join('|');
        "#
        ),
        // `undefined` before the abort, an `AbortError` after it.
        "false,undefined|true|AbortError|20|true|true|string|true"
    );
}

/// The `abort` event fires **synchronously**, inside the `abort()` call: a
/// listener registered before the call runs before the statement after it, and
/// the signal it is dispatched at is already aborted while it runs.
#[test]
fn aborting_fires_an_abort_event_synchronously() {
    assert_eq!(
        eval(
            r#"
            const c = new AbortController();
            const order = [];
            let seen = null;
            c.signal.addEventListener('abort', (e) => {
                order.push('listener');
                seen = [
                    e.type,
                    e.target === c.signal,
                    e.currentTarget === c.signal,
                    e.isTrusted,
                    e.bubbles,
                    e.cancelable,
                    c.signal.aborted,
                ].join(',');
            });
            order.push('before');
            c.abort();
            order.push('after');
            `${order.join(',')}#${seen}#${c.signal.aborted}`;
        "#
        ),
        // A deferred dispatch would put `listener` after `after`.
        "before,listener,after#abort,true,true,true,false,false,true#true"
    );
}

#[test]
fn a_second_abort_is_a_no_op() {
    assert_eq!(
        eval(
            r#"
            const c = new AbortController();
            const hits = [];
            c.signal.addEventListener('abort', () => hits.push(1));
            c.abort('first');
            const reason = c.signal.reason;
            c.abort('second');
            [
                hits.length,
                c.signal.reason === reason,
                c.signal.reason,
                c.signal.aborted,
            ].join('|');
        "#
        ),
        "1|true|first|true"
    );
}

#[test]
fn an_explicit_reason_is_kept_by_identity() {
    // A reason is *the value the page passed*, not a copy: identity survives, so
    // `abort(reason)` followed by `signal.reason` is `===` the argument. The
    // object case is the one that proves the value is stored and interned rather
    // than re-created from its bits.
    assert_eq!(
        eval(
            r#"
            const c = new AbortController();
            const object = { tag: 'reason' };
            const text = 'plain text';
            const other = new AbortController();
            c.abort(object);
            other.abort(text);
            [
                c.signal.reason === object,
                c.signal.reason.tag,
                other.signal.reason === text,
                other.signal.reason,
                // An arbitrary object is not the `AbortError`, so `name` is not
                // synthesised for it.
                String(c.signal.reason.name),
            ].join('|');
        "#
        ),
        "true|reason|true|plain text|undefined"
    );
}

#[test]
fn abort_null_and_undefined_take_their_two_paths() {
    // `abort(null)` stores `null` — a value that is not `undefined`, so the
    // signal is aborted. `abort(undefined)` passes no reason at all, so the
    // default `AbortError` is installed.
    assert_eq!(
        eval(
            r#"
            const withNull = new AbortController();
            const withUndefined = new AbortController();
            const withNoArgument = new AbortController();
            withNull.abort(null);
            withUndefined.abort(undefined);
            withNoArgument.abort();
            [
                withNull.signal.aborted, withNull.signal.reason === null,
                withUndefined.signal.reason.name,
                withNoArgument.signal.reason.name,
            ].join('|');
        "#
        ),
        "true|true|AbortError|AbortError"
    );
}

/// Step 1 of the abort algorithm returns *before* step 2 builds the default
/// `AbortError`, so the `DOMException` constructor runs once for a signal that is
/// aborted twice without a reason — building a reason the second call would
/// immediately discard is exactly what returning first avoids.
#[test]
fn a_second_abort_builds_no_second_default_reason() {
    assert_eq!(
        eval(
            r#"
            const real = DOMException;
            let built = 0;
            globalThis.DOMException = function (message, name) {
                built += 1;
                return new real(message, name);
            };
            const c = new AbortController();
            c.abort();
            const afterFirst = built;
            c.abort();
            globalThis.DOMException = real;
            `${afterFirst}#${built}#${c.signal.reason.name}`;
        "#
        ),
        "1#1#AbortError"
    );
}

#[test]
fn throw_if_aborted_throws_the_stored_reason() {
    assert_eq!(
        eval(
            r#"
            const quiet = new AbortController();
            const silent = (() => {
                try { quiet.signal.throwIfAborted(); return 'no throw'; }
                catch (e) { return 'threw ' + e; }
            })();

            const loud = new AbortController();
            const object = { tag: 'boom' };
            loud.abort(object);
            let caught = null;
            try { loud.signal.throwIfAborted(); } catch (e) { caught = e; }

            const defaulted = new AbortController();
            defaulted.abort();
            let defaultCaught = null;
            try { defaulted.signal.throwIfAborted(); } catch (e) { defaultCaught = e; }
            [
                silent,
                caught === object,
                caught === loud.signal.reason,
                defaultCaught === defaulted.signal.reason,
                defaultCaught.name,
            ].join('|');
        "#
        ),
        "no throw|true|true|true|AbortError"
    );
}

// ---------------------------------------------------------------------------
// Listeners on a signal
// ---------------------------------------------------------------------------

#[test]
fn remove_event_listener_suppresses_the_abort_event() {
    assert_eq!(
        eval(
            r#"
            const c = new AbortController();
            const hits = [];
            const listener = () => hits.push('removed');
            c.signal.addEventListener('abort', listener);
            c.signal.addEventListener('abort', () => hits.push('kept'));
            c.signal.removeEventListener('abort', listener);
            c.abort();
            hits.join(',');
        "#
        ),
        "kept"
    );
}

/// Two signals are two event targets. This is the test that fails if listener
/// entries are keyed by the handle *tag* or by the interface name: every
/// JS-owned host object shares `TAG_HOST`, and every signal has the same
/// interface name, so either key would put both signals' listeners in one bucket
/// and `b.abort()` would run `a`'s listener.
#[test]
fn two_signals_do_not_share_a_listener_list() {
    assert_eq!(
        eval(
            r#"
            const a = new AbortController();
            const b = new AbortController();
            const hits = [];
            a.signal.addEventListener('abort', () => hits.push('a'));
            b.signal.addEventListener('abort', () => hits.push('b'));
            b.abort();
            const afterB = hits.join(',');
            a.abort();
            `${afterB}#${hits.join(',')}`;
        "#
        ),
        "b#b,a"
    );
}

#[test]
fn dispatch_event_on_a_signal_reaches_only_its_own_listeners() {
    // The `dispatchEvent` path resolves the same key `addEventListener` stored,
    // so a manual event on one signal is isolated in the same way.
    assert_eq!(
        eval(
            r#"
            const a = new AbortController();
            const b = new AbortController();
            const hits = [];
            a.signal.addEventListener('ping', () => hits.push('a'));
            b.signal.addEventListener('ping', () => hits.push('b'));
            const event = new Event('ping');
            b.signal.dispatchEvent(event);
            `${hits.join(',')}#${event.target === b.signal}#${event.isTrusted}`;
        "#
        ),
        "b#true#false"
    );
}

#[test]
fn a_listener_on_a_signal_is_not_a_listener_on_the_window() {
    // An `AbortSignal` is an event target but not in the tree, so its abort event
    // has no propagation path: the window above the document must not see it.
    assert_eq!(
        eval(
            r#"
            const seen = [];
            addEventListener('abort', () => seen.push('window'));
            const c = new AbortController();
            c.signal.addEventListener('abort', () => seen.push('signal'));
            c.abort();
            seen.join(',');
        "#
        ),
        "signal"
    );
}

// ---------------------------------------------------------------------------
// What is deliberately not implemented
// ---------------------------------------------------------------------------

/// The members this slice defers, pinned so the gap is visible rather than
/// silently assumed. Each is a separate decision recorded in the WebIDL
/// provenance headers (`idl/dom/AbortSignal.webidl`).
#[test]
fn the_deferred_members_are_absent() {
    assert_eq!(
        eval(
            r#"
            const s = new AbortController().signal;
            [
                typeof AbortSignal.abort,
                typeof AbortSignal.timeout,
                typeof AbortSignal.any,
                typeof s.onabort,
            ].join('|');
        "#
        ),
        "undefined|undefined|undefined|undefined"
    );
}

// ---------------------------------------------------------------------------
// Reclamation
// ---------------------------------------------------------------------------

#[test]
fn a_collected_signal_releases_its_listeners_and_reason() {
    // A signal record owns two things beyond its own slot: the listener entries
    // registered on it (keyed under its handle) and the interned abort reason.
    // Neither is the host slab's to free, so this exercises the drain that
    // `reclaim_dead_hosts` performs when the record goes.
    let mut engine = Engine::with_document(document());
    // Warm up the one world-wide interned `undefined`, which the first read of an
    // un-aborted `reason` creates and which nothing ever releases, so the
    // baseline below is the steady state.
    engine
        .evaluate("String(new AbortController().signal.reason);")
        .expect("evaluate the warm-up");
    for _ in 0..5 {
        engine.low_memory_gc();
        engine.reclaim_dead_hosts();
        engine.reclaim_dead_events();
    }
    let host_baseline = engine.host_record_count();
    let listener_baseline = engine.listener_count();
    let value_baseline = engine.stored_value_count();

    engine
        .evaluate(
            r#"
            for (let i = 0; i < 25; i += 1) {
                const c = new AbortController();
                c.signal.addEventListener('abort', () => {});
                c.abort({ tag: String(i) });
            }
        "#,
        )
        .expect("evaluate the allocation loop");

    // Two records and one listener are created per iteration, plus one stored
    // reason; collect and drain until the finalizers have all run.
    for _ in 0..8 {
        engine.low_memory_gc();
        engine.reclaim_dead_hosts();
        engine.reclaim_dead_events();
    }

    assert_eq!(
        engine.host_record_count(),
        host_baseline,
        "a collected controller/signal pair should free both records"
    );
    assert_eq!(
        engine.listener_count(),
        listener_baseline,
        "a collected signal's listeners should leave the listener table with it"
    );
    assert_eq!(
        engine.stored_value_count(),
        value_baseline,
        "a collected signal's abort reason should be released"
    );
}

#[test]
fn a_live_signal_keeps_its_listeners_and_reason() {
    let mut engine = Engine::with_document(document());
    let host_baseline = engine.host_record_count();
    let listener_baseline = engine.listener_count();
    engine
        .evaluate(
            r#"
            globalThis.controller = new AbortController();
            globalThis.controller.signal.addEventListener('abort', () => {
                globalThis.hits = (globalThis.hits || 0) + 1;
            });
            globalThis.controller.abort('kept');
        "#,
        )
        .expect("evaluate the kept pair");
    for _ in 0..5 {
        engine.low_memory_gc();
        engine.reclaim_dead_hosts();
        engine.reclaim_dead_events();
    }
    assert_eq!(engine.host_record_count(), host_baseline + 2);
    assert_eq!(engine.listener_count(), listener_baseline + 1);
    assert_eq!(
        engine
            .evaluate(
                "globalThis.controller.signal.reason + ':' + \
                 globalThis.controller.signal.aborted + ':' + globalThis.hits"
            )
            .expect("read the kept pair"),
        "kept:true:1"
    );
}

/// When only the signal's own wrapper dies, the record has to survive for as
/// long as the controller's does: `[SameObject]` means the next read of
/// `controller.signal` must answer with the *state* the path holds, and a signal
/// rebuilt from scratch would come back un-aborted.
#[test]
fn a_signal_whose_controller_survives_stays_aborted() {
    let mut engine = Engine::with_document(document());
    let baseline = engine.host_record_count();
    engine
        .evaluate(
            r#"
            globalThis.controller = (function () {
                const c = new AbortController();
                c.signal.addEventListener('abort', () => {});
                c.abort('boom');
                return c;
            })();
        "#,
        )
        .expect("evaluate the factory");
    // The signal wrapper built inside the IIFE is unreachable from the moment it
    // returns, so the collection below reclaims it while the controller lives on.
    for _ in 0..8 {
        engine.low_memory_gc();
        engine.reclaim_dead_hosts();
        engine.reclaim_dead_events();
    }
    assert_eq!(
        engine.host_record_count(),
        baseline + 2,
        "the controller's record keeps the signal's alive"
    );
    assert_eq!(
        engine
            .evaluate(
                "globalThis.controller.signal.aborted + ':' + \
                 globalThis.controller.signal.reason"
            )
            .expect("read the re-created signal wrapper"),
        "true:boom"
    );
}

#[test]
fn signals_survive_many_round_trips() {
    // Interleaves controller construction, listener registration and aborts many
    // times over, so a record freed while a live wrapper still needs it — or a
    // listener bucket reused behind one — shows up as a wrong value rather than
    // as a crash.
    let mut engine = Engine::with_document(document());
    let baseline = engine.host_record_count();
    assert_eq!(
        engine
            .evaluate(
                r#"
                let fired = 0;
                let reasons = '';
                for (let i = 0; i < 50; i += 1) {
                    const c = new AbortController();
                    c.signal.addEventListener('abort', () => { fired += 1; });
                    c.abort(i);
                    reasons += String(c.signal.reason) + ',';
                    // A second abort must neither re-fire nor replace.
                    c.abort(i + 1000);
                }
                `${fired}#${reasons.slice(0, -1).split(',').length}#${reasons.startsWith('0,1,2,')}`;
            "#
            )
            .expect("evaluate the round-trip loop"),
        "50#50#true"
    );
    for _ in 0..5 {
        engine.low_memory_gc();
        engine.reclaim_dead_hosts();
        engine.reclaim_dead_events();
    }
    assert_eq!(engine.host_record_count(), baseline);
}
