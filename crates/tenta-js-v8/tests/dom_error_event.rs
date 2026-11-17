//! `ErrorEvent`, the `error` event the engine fires at the window for an
//! exception nothing caught, the window's `onerror` handler, and
//! `reportError()`.
//!
//! The exception in these tests is thrown by an event listener, which is one of
//! the places the engine reports an uncaught exception from
//! (`inspector::report_caught`); the event is fired when the task ends, which
//! `Engine::run_microtasks` stands for here as it does in the other suites.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_js_v8::Engine;

fn document() -> Rc<RefCell<Document>> {
    let mut d = Document::new();
    let html = d.create_html_element(atoms::HTML);
    let body = d.create_html_element(atoms::BODY);
    for (parent, child) in [(d.document_node(), html), (html, body)] {
        d.mutate(Mutation::Append { parent, child })
            .expect("append");
    }
    Rc::new(RefCell::new(d))
}

fn engine() -> Engine {
    Engine::with_document(document())
}

fn eval(engine: &mut Engine, source: &str) -> String {
    engine
        .evaluate(source)
        .unwrap_or_else(|error| panic!("`{source}` should evaluate: {error}"))
}

/// Runs `source`, ends the task, then reads `after`.
fn after_task(engine: &mut Engine, source: &str, after: &str) -> String {
    eval(engine, source);
    engine.run_microtasks();
    eval(engine, after)
}

/// Script that makes a listener throw `expression` during a dispatch, which the
/// engine reports as an exception nothing caught.
fn throwing(expression: &str) -> String {
    format!(
        "document.addEventListener('probe', () => {{ throw {expression}; }}, {{ once: true }});
         document.dispatchEvent(new Event('probe'));"
    )
}

// ---------------------------------------------------------------------------
// The interface
// ---------------------------------------------------------------------------

#[test]
fn error_event_is_an_event_with_five_members() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            r#"
            const reason = { why: 'because' };
            const made = new ErrorEvent('error', {
                message: 'm', filename: 'http://x.test/a.js', lineno: 3, colno: 7, error: reason,
                cancelable: true,
            });
            const bare = new ErrorEvent('x');
            [
                made instanceof ErrorEvent, made instanceof Event, made.type, made.message,
                made.filename, made.lineno, made.colno, made.error === reason, made.cancelable,
                made.isTrusted,
                '[' + bare.message + ']', '[' + bare.filename + ']', bare.lineno, bare.colno,
                bare.error === undefined,
                Object.keys(ErrorEvent.prototype).join(','), ErrorEvent.length,
            ].join('|');
            "#
        ),
        "true|true|error|m|http://x.test/a.js|3|7|true|true|false|[]|[]|0|0|true|\
         message,filename,lineno,colno,error|1"
    );
}

// ---------------------------------------------------------------------------
// An exception nothing caught
// ---------------------------------------------------------------------------

#[test]
fn an_uncaught_exception_fires_error_at_the_window() {
    let mut engine = engine();
    eval(
        &mut engine,
        r#"
        globalThis.seen = [];
        globalThis.thrown = new RangeError('boom');
        addEventListener('error', event => {
            seen.push([
                event instanceof ErrorEvent, event.type, event.isTrusted, event.cancelable,
                event.bubbles, event.target === window, event.message, event.error === thrown,
                event.lineno > 0, event.colno > 0,
            ].join(','));
        });
        'listening'
        "#,
    );
    assert_eq!(
        eval(&mut engine, "seen.length"),
        "0",
        "nothing has thrown yet"
    );
    eval(&mut engine, &throwing("thrown"));
    assert_eq!(
        eval(&mut engine, "seen.length"),
        "0",
        "the event is fired when the task ends, not inside the dispatch that threw"
    );
    engine.run_microtasks();
    assert_eq!(
        eval(&mut engine, "seen.join('|')"),
        "true,error,true,true,false,true,Uncaught RangeError: boom,true,true,true"
    );
}

#[test]
fn every_uncaught_exception_of_a_task_gets_an_event_in_order() {
    let mut engine = engine();
    assert_eq!(
        after_task(
            &mut engine,
            &format!(
                "globalThis.seen = [];
                 addEventListener('error', event => seen.push(String(event.error)));
                 {} {} {}",
                throwing("'first'"),
                throwing("2"),
                throwing("null")
            ),
            "seen.join('|')"
        ),
        "first|2|null",
        "a thrown value that is not an Error is the event's error as it was thrown"
    );
}

#[test]
fn the_onerror_handler_takes_five_arguments_and_cancels_by_returning_true() {
    let mut engine = engine();
    assert_eq!(
        after_task(
            &mut engine,
            &format!(
                "globalThis.seen = [];
                 globalThis.thrown = new Error('boom');
                 window.onerror = function (message, source, lineno, colno, error) {{
                     seen.push([arguments.length, this === window, message, typeof source,
                                lineno > 0, colno > 0, error === thrown].join(','));
                     return true;
                 }};
                 addEventListener('error', event => seen.push('prevented:' + event.defaultPrevented));
                 {}",
                throwing("thrown")
            ),
            "seen.join('|')"
        ),
        "5,true,Uncaught Error: boom,string,true,true,true|prevented:true"
    );
}

#[test]
fn an_onerror_handler_that_returns_anything_else_does_not_cancel() {
    let mut engine = engine();
    assert_eq!(
        after_task(
            &mut engine,
            &format!(
                "globalThis.seen = [];
                 window.onerror = () => false;
                 addEventListener('error', event => seen.push(event.defaultPrevented));
                 {}",
                throwing("new Error('x')")
            ),
            "seen.join('|')"
        ),
        "false",
        "`false` cancels for every other handler and not for this one"
    );
}

#[test]
fn an_event_that_is_not_an_error_event_reaches_onerror_as_an_event() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            r#"
            globalThis.seen = [];
            window.onerror = function () {
                seen.push(arguments.length + ':' + (arguments[0] instanceof Event));
            };
            window.dispatchEvent(new Event('error'));
            window.dispatchEvent(new ErrorEvent('error', { message: 'made', lineno: 4 }));
            window.onerror = function (message, source, lineno) { seen.push(message + '@' + lineno); };
            window.dispatchEvent(new ErrorEvent('error', { message: 'made', lineno: 4 }));
            seen.join('|');
            "#
        ),
        "1:true|5:false|made@4"
    );
}

#[test]
fn an_exception_thrown_by_an_error_listener_is_not_an_error_event() {
    let mut engine = engine();
    eval(
        &mut engine,
        &format!(
            "globalThis.count = 0;
             addEventListener('error', () => {{ count += 1; throw new Error('again'); }});
             {}",
            throwing("new Error('first')")
        ),
    );
    for _ in 0..3 {
        engine.run_microtasks();
    }
    assert_eq!(
        eval(&mut engine, "count"),
        "1",
        "the listener's own exception is reported and does not come back as an event"
    );
}

#[test]
fn a_microtask_an_error_listener_queues_runs_with_the_event() {
    let mut engine = engine();
    assert_eq!(
        after_task(
            &mut engine,
            &format!(
                "globalThis.seen = [];
                 addEventListener('error', () => {{
                     seen.push('event');
                     Promise.resolve().then(() => seen.push('microtask'));
                 }});
                 {}",
                throwing("new Error('x')")
            ),
            "seen.join('|')"
        ),
        "event|microtask"
    );
}

#[test]
fn an_uncaught_exception_with_nobody_listening_leaves_nothing_behind() {
    let mut engine = engine();
    eval(&mut engine, &throwing("new Error('unheard')"));
    engine.run_microtasks();
    // A listener added afterwards does not hear about the earlier exception.
    assert_eq!(
        after_task(
            &mut engine,
            "globalThis.seen = []; addEventListener('error', e => seen.push(e.message)); 'on'",
            "seen.length"
        ),
        "0"
    );
}

// ---------------------------------------------------------------------------
// reportError()
// ---------------------------------------------------------------------------

#[test]
fn report_error_reports_a_value_as_an_uncaught_exception() {
    let mut engine = engine();
    assert_eq!(
        after_task(
            &mut engine,
            r#"
            globalThis.seen = [];
            globalThis.value = { not: 'an error' };
            addEventListener('error', event => seen.push(event.error === value));
            const returned = reportError(value);
            seen.push('returned:' + returned);
            try { reportError(); seen.push('no throw'); }
            catch (error) { seen.push(error.constructor.name); }
            "#,
            "seen.join('|')"
        ),
        "returned:undefined|TypeError|true",
        "the call returns normally, and the event follows at the end of the task"
    );
}

// ---------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------

#[test]
fn an_error_event_a_page_keeps_stays_readable_and_is_freed_with_it() {
    let mut engine = engine();
    eval(
        &mut engine,
        &format!(
            "addEventListener('error', event => {{ globalThis.kept = event; }});
             {}",
            throwing("new Error('kept')")
        ),
    );
    engine.run_microtasks();
    for _ in 0..3 {
        engine.low_memory_gc();
        engine.reclaim_dead_hosts();
    }
    assert_eq!(
        eval(
            &mut engine,
            "[kept.message, kept.error.message, kept.error instanceof Error].join('|')"
        ),
        "Uncaught Error: kept|kept|true",
        "the event keeps the value that was thrown"
    );
}
