//! Shared Web Storage behavior through the generated V8 bindings.

use tenta_js::event_loop::{Clock, MonoTime};
use tenta_js_v8::Engine;

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
}

include!("../../tenta-js/tests/support/web_storage.rs");
