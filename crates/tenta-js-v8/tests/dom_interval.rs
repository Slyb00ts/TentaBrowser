//! Tests for Phase 4a-2c: `setInterval` / `clearInterval`. Frameworks and app
//! code poll on intervals; the timer is re-armed by the loop under a stable id
//! so the handle `clearInterval` receives keeps working across firings.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
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

#[test]
fn an_uncleared_interval_keeps_firing_up_to_the_task_bound() {
    // A never-cleared interval would repeat forever; `run_event_loop`'s task
    // bound is what stops the drive, proving the loop re-arms the same id.
    let mut engine = Engine::with_document(test_document());
    engine
        .evaluate("globalThis.n = 0; setInterval(() => { globalThis.n++; }, 5);")
        .expect("install interval");

    let ran = engine.run_event_loop(8);
    assert_eq!(ran, 8);
    assert_eq!(engine.evaluate("String(globalThis.n)").unwrap(), "8");
}

#[test]
fn clear_interval_from_inside_the_callback_stops_the_repeat() {
    // The handle returned by `setInterval` stays valid across firings, so a
    // callback can cancel its own interval once a condition is met.
    let mut engine = Engine::with_document(test_document());
    engine
        .evaluate(
            r#"
            globalThis.ticks = 0;
            globalThis.id = setInterval(() => {
                globalThis.ticks++;
                if (globalThis.ticks === 3) clearInterval(globalThis.id);
            }, 10);
        "#,
        )
        .expect("install self-clearing interval");

    // Bounded well above three; the callback's own clearInterval is what stops it.
    let ran = engine.run_event_loop(50);
    assert_eq!(ran, 3);
    assert_eq!(engine.evaluate("String(globalThis.ticks)").unwrap(), "3");
}

#[test]
fn clear_interval_before_the_first_tick_prevents_any_firing() {
    let mut engine = Engine::with_document(test_document());
    engine
        .evaluate(
            r#"
            globalThis.fired = false;
            const id = setInterval(() => { globalThis.fired = true; }, 10);
            clearInterval(id);
        "#,
        )
        .expect("install then immediately clear interval");

    let ran = engine.run_event_loop(10);
    assert_eq!(ran, 0);
    assert_eq!(
        engine.evaluate("String(globalThis.fired)").unwrap(),
        "false"
    );
}

#[test]
fn run_event_loop_requeues_handlers_beyond_the_task_bound() {
    // Five timers all come due together; a bound of two must run two now and
    // leave the other three for the next drive rather than dropping them.
    let mut engine = Engine::with_document(test_document());
    engine
        .evaluate(
            r#"
            globalThis.hits = 0;
            for (let i = 0; i < 5; i++) setTimeout(() => { globalThis.hits++; }, 1);
        "#,
        )
        .expect("schedule five timers");

    let first = engine.run_event_loop(2);
    assert_eq!(first, 2);
    assert_eq!(engine.evaluate("String(globalThis.hits)").unwrap(), "2");

    let second = engine.run_event_loop(10);
    assert_eq!(second, 3);
    assert_eq!(engine.evaluate("String(globalThis.hits)").unwrap(), "5");
}

#[test]
fn dropping_an_engine_with_a_live_interval_is_clean() {
    // A never-cleared interval keeps a v8::Global handle in the timer map; the
    // engine must release it while the isolate is alive, so this drop neither
    // leaks nor faults.
    let mut engine = Engine::with_document(test_document());
    engine
        .evaluate("setInterval(() => {}, 10);")
        .expect("install interval");
    engine.tick_frame();
    drop(engine);
}
