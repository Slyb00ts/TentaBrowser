//! V8 Window storage accessors use the page's canonical storage area and the
//! same sticky native-failure boundary as other handwritten global getters.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::Document;
use tenta_js_v8::Engine;
use url::Url;

fn document_at(address: &str) -> Rc<RefCell<Document>> {
    Rc::new(RefCell::new(Document::with_url(
        Url::parse(address).expect("test URL"),
    )))
}

#[test]
fn window_storage_getters_return_canonical_wrappers() {
    let mut engine = Engine::with_document(document_at("https://example.test/page"));
    assert_eq!(
        engine
            .evaluate(
                "[window.localStorage === localStorage, \
                  localStorage === window.localStorage, \
                  sessionStorage === window.sessionStorage, \
                  localStorage !== sessionStorage, \
                  localStorage instanceof Storage, \
                  sessionStorage instanceof Storage].join('|')"
            )
            .expect("read storage attributes"),
        "true|true|true|true|true|true"
    );
}

#[test]
fn a_storage_getter_panic_terminates_only_its_own_engine() {
    let document = document_at("https://example.test/page");
    let mut engine = Engine::with_document(Rc::clone(&document));
    let mut neighbor = Engine::with_document(document_at("https://example.test/neighbor"));
    let error = {
        // The accessor resolves the final URL from this document. Holding its
        // exclusive borrow injects a real native panic after V8 enters the
        // handwritten getter; page JavaScript cannot catch that panic.
        let _borrow = document.borrow_mut();
        engine
            .evaluate("try { localStorage; 'continued' } catch (_) { 'caught' }")
            .expect_err("native getter panic must terminate the isolate")
    };
    assert_eq!(engine.native_failure(), Some("window_local_storage_getter"));
    assert!(error.message.contains("window_local_storage_getter"));
    assert_eq!(engine.evaluate("42").unwrap_err().message, error.message);
    assert_eq!(neighbor.evaluate("6 * 7").unwrap(), "42");
}
