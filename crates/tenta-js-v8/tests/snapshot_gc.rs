//! Static collections retain detached results until their wrapper is collected.

use tenta_js_v8::Engine;

fn new_engine(document: tenta_dom::SharedDocument) -> Engine {
    Engine::with_document(document)
}

include!("../../tenta-js/tests/support/snapshot_gc.rs");
