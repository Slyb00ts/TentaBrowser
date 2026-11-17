//! `PromiseRejectionEvent` and the `unhandledrejection` event.
//!
//! The interface and the function that dispatches the event are a script
//! (`crates/tenta-js/src/rejection.rs`); the engine calls it when a task ends
//! with a rejected promise nobody handled. Whether a cancelled event keeps the
//! rejection out of the console is tested with a page, in
//! `crates/tenta-browser/tests/error_events_js.rs`.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::Document;
use tenta_js_v8::Engine;

/// An engine with the preludes a page gets.
fn engine() -> Engine {
    let mut document = Document::new();
    document.set_url(Some("https://example.com/".parse().expect("test URL")));
    let mut engine = Engine::with_document(Rc::new(RefCell::new(document)));
    engine.install_preludes();
    engine
}

fn eval(engine: &mut Engine, source: &str) -> String {
    engine
        .evaluate(source)
        .unwrap_or_else(|error| panic!("`{source}` should evaluate: {error}"))
}

/// Runs `source`, ends the task, then reads `after`.
fn after_task(source: &str, after: &str) -> String {
    let mut engine = engine();
    eval(&mut engine, source);
    engine.run_microtasks();
    eval(&mut engine, after)
}

#[test]
fn promise_rejection_event_is_an_event_with_a_promise_and_a_reason() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            r#"
            const attempt = f => { try { return String(f()); } catch (e) { return e.constructor.name; } };
            const promise = Promise.resolve(), reason = { why: 1 };
            const event = new PromiseRejectionEvent('unhandledrejection',
                { promise, reason, cancelable: true });
            [
                event instanceof PromiseRejectionEvent, event instanceof Event, String(event),
                event.type, event.promise === promise, event.reason === reason, event.cancelable,
                new PromiseRejectionEvent('x', { promise }).reason === undefined,
                Object.keys(PromiseRejectionEvent.prototype).join(','),
                attempt(() => new PromiseRejectionEvent('x')),
                attempt(() => new PromiseRejectionEvent('x', {})),
                attempt(() => new PromiseRejectionEvent('x', { promise: 1 })),
                attempt(() => Object.getOwnPropertyDescriptor(
                    PromiseRejectionEvent.prototype, 'promise').get.call(new Event('x'))),
            ].join('|');
            "#
        ),
        "true|true|[object PromiseRejectionEvent]|unhandledrejection|true|true|true|true|\
         promise,reason|TypeError|TypeError|TypeError|TypeError"
    );
}

#[test]
fn a_rejection_nobody_handles_fires_unhandledrejection_at_the_window() {
    assert_eq!(
        after_task(
            r#"
            globalThis.seen = [];
            globalThis.reason = new Error('lost');
            addEventListener('unhandledrejection', event => {
                seen.push([event instanceof PromiseRejectionEvent, event.type, event.cancelable,
                           event.target === window, event.reason === reason,
                           event.promise === rejected].join(','));
            });
            globalThis.rejected = Promise.reject(reason);
            // A handler attached in the same task is a handled rejection.
            Promise.reject(new Error('caught')).catch(() => {});
            seen.push('before the task ended: ' + seen.length);
            "#,
            "seen.join('|')"
        ),
        "before the task ended: 0|true,unhandledrejection,true,true,true,true"
    );
}

#[test]
fn the_handler_attribute_hears_it_and_a_listeners_microtask_runs() {
    assert_eq!(
        after_task(
            r#"
            globalThis.seen = [];
            window.onunhandledrejection = function (event) {
                seen.push('handler:' + (this === window) + ':' + event.reason);
                Promise.resolve().then(() => seen.push('microtask'));
            };
            Promise.reject('one');
            Promise.reject('two');
            "#,
            "seen.join('|')"
        ),
        "handler:true:one|handler:true:two|microtask|microtask"
    );
}

#[test]
fn a_rejection_with_nobody_listening_is_dropped_and_a_later_listener_is_not_told() {
    let mut engine = engine();
    eval(
        &mut engine,
        "Promise.reject(new Error('unheard')); 'rejected'",
    );
    engine.run_microtasks();
    eval(
        &mut engine,
        "globalThis.seen = []; addEventListener('unhandledrejection', e => seen.push(1)); 'on'",
    );
    engine.run_microtasks();
    assert_eq!(eval(&mut engine, "seen.length"), "0");
}

#[test]
fn a_page_that_replaces_dispatch_event_does_not_change_the_dispatch() {
    assert_eq!(
        after_task(
            r#"
            globalThis.seen = [];
            addEventListener('unhandledrejection', event => seen.push(String(event.reason)));
            EventTarget.prototype.dispatchEvent = () => { seen.push('replaced'); return true; };
            globalThis.PromiseRejectionEvent = function () { seen.push('replaced'); };
            Promise.reject('still heard');
            "#,
            "seen.join('|')"
        ),
        "still heard"
    );
}
