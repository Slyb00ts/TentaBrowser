//! Shared Web Storage behavior through the generated QuickJS bindings.

use tenta_js::event_loop::{Clock, MonoTime};
use tenta_js_qjs::Engine;

fn new_engine_with_storage(
    document: tenta_dom::SharedDocument,
    page: tenta_js::storage::PageStorage,
) -> Engine {
    Engine::with_document_on_clock_and_storage(
        document,
        Clock::virtual_at(MonoTime::ZERO),
        None,
        tenta_net::CookieJar::new(),
        page,
    )
    .expect("QuickJS runtime")
}

include!("../../tenta-js/tests/support/web_storage.rs");
