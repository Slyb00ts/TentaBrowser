//! Event propagation follows actual node and document parent relationships.

use tenta_js_v8::Engine;

fn new_engine(document: tenta_dom::SharedDocument) -> Engine {
    Engine::with_document(document)
}

include!("../../tenta-js/tests/support/event_propagation_path.rs");
