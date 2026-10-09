//! A real DOM host panic through generated V8 glue poisons only that engine.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::Document;
use tenta_js::engine::PageEngine;
use tenta_js_v8::Engine;

#[test]
fn a_native_panic_is_uncatchable_and_the_engine_cannot_be_reused() {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = Engine::with_document(Rc::clone(&document));
    let mut neighbor = Engine::new();
    let error = {
        // Deliberately break the documented embedding borrow rule to inject a
        // real RefCell panic inside DocumentOps::body, after entering V8.
        let _borrow = document.borrow_mut();
        engine
            .evaluate("try { document.body; } catch (error) { 'caught'; }")
            .expect_err("a native panic cannot be caught by page JavaScript")
    };
    assert_eq!(engine.native_failure(), Some("get_document_body"));
    assert_eq!(PageEngine::native_failure(&engine), engine.native_failure());
    assert!(error.message.contains("get_document_body"));
    assert!(error.message.contains("must be discarded"));
    assert_eq!(
        engine.evaluate("40 + 2").unwrap_err().message,
        error.message,
        "the ordinary entry must not clear native-panic termination"
    );
    engine.run_microtasks();
    engine.pump_due_timers(10);
    assert_eq!(engine.evaluate("1").unwrap_err().message, error.message);
    assert_eq!(neighbor.evaluate("40 + 2").unwrap(), "42");
    drop(engine);
    assert_eq!(neighbor.evaluate("6 * 7").unwrap(), "42");
}

#[test]
fn native_panics_in_microtasks_and_timers_remain_sticky() {
    for queued in [
        "Promise.resolve().then(() => document.body)",
        "setTimeout(() => document.body, 0)",
    ] {
        let document = Rc::new(RefCell::new(Document::new()));
        let mut engine = Engine::with_document(Rc::clone(&document));
        engine.evaluate(queued).expect("queue callback");
        {
            let _borrow = document.borrow_mut();
            engine.run_microtasks();
            engine.pump_due_timers(10);
        }
        assert_eq!(engine.native_failure(), Some("get_document_body"));
        let error = engine.evaluate("42").expect_err("sticky native failure");
        assert!(error.message.contains("get_document_body"));
        engine.run_microtasks();
        engine.pump_due_timers(10);
        assert_eq!(engine.native_failure(), Some("get_document_body"));
    }
}

#[test]
fn a_handwritten_fetch_panic_is_uncatchable_and_local() {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = Engine::with_document(Rc::clone(&document));
    let mut neighbor = Engine::new();
    let error = {
        // `fetch` creates its promise before resolving the URL from the
        // document. This real borrow conflict injects a panic inside the
        // handwritten callback, after its host state has changed.
        let _borrow = document.borrow_mut();
        engine
            .evaluate("try { fetch('https://example.test/'); 'caught' } catch (_) { 'caught' }")
            .expect_err("a host panic must terminate, not reach the catch block")
    };
    assert_eq!(engine.native_failure(), Some("fetch"));
    assert!(error.message.contains("fetch"));
    assert!(error.message.contains("must be discarded"));
    assert_eq!(engine.evaluate("42").unwrap_err().message, error.message);
    assert_eq!(neighbor.evaluate("6 * 7").unwrap(), "42");
    drop(engine);
    assert_eq!(neighbor.evaluate("40 + 2").unwrap(), "42");
}
