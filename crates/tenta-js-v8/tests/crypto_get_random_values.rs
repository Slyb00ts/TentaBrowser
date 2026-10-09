//! Window Crypto behavior through the generated V8 binding.

use std::cell::RefCell;
use std::rc::Rc;
use tenta_js_v8::Engine;

fn new_page_engine() -> Engine {
    Engine::with_document(Rc::new(RefCell::new(tenta_dom::Document::new())))
}

fn eval(source: &str) -> String {
    new_page_engine().evaluate(source).expect("Crypto script")
}

fn new_engine_with_budget(
    document: tenta_dom::SharedDocument,
    budget: tenta_core::NativeMemoryBudget,
) -> Engine {
    Engine::with_document_on_clock_storage_and_memory(
        document,
        tenta_js::event_loop::Clock::virtual_at(tenta_js::event_loop::MonoTime::ZERO),
        None,
        tenta_net::CookieJar::new(),
        tenta_js::storage::PageStorage::isolated(),
        budget,
    )
}

include!("../../tenta-js/tests/support/crypto_get_random_values.rs");

#[test]
fn crypto_wrapper_retains_expando_across_full_gc() {
    let mut engine = new_page_engine();
    assert_eq!(
        engine
            .evaluate("crypto.marker = 73; crypto === window.crypto")
            .expect("first Crypto wrapper"),
        "true"
    );
    // No page global or local holds Crypto except Window's own wrapper root.
    engine.low_memory_gc();
    assert_eq!(
        engine
            .evaluate("window.crypto.marker")
            .expect("retained expando"),
        "73"
    );
    assert_eq!(
        engine
            .evaluate("crypto === window.crypto")
            .expect("same canonical Crypto wrapper"),
        "true"
    );
}
