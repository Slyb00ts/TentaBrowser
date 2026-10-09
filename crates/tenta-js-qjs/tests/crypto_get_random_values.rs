//! Window Crypto behavior through the generated QuickJS binding.

use tenta_js_qjs::Engine;

fn eval(source: &str) -> String {
    Engine::new()
        .expect("QuickJS runtime")
        .evaluate(source)
        .expect("Crypto script")
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
    .expect("bounded QuickJS engine")
}

include!("../../tenta-js/tests/support/crypto_get_random_values.rs");
