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
    EngineClock, PageEngine, PageEngineConfig, PageEngineFactory, ScriptErrorKind, ScriptKind,
};
use tenta_js_v8::V8EngineFactory;

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
        let _ = tx.send((error.kind, error.message, started.elapsed()));
    });

    let (kind, message, elapsed) = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the watchdog must stop the script; a hang here is the bug");
    assert_eq!(kind, ScriptErrorKind::Terminated, "got {message:?}");
    assert!(
        elapsed < Duration::from_secs(1),
        "the watchdog stopped it early, not the 3 s deadline: {elapsed:?}"
    );
    runner.join().expect("the runner thread finished");
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
