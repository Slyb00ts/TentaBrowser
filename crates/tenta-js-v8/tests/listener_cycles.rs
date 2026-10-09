//! Detached DOM nodes must not stay alive solely through their own listeners.

use tenta_js_v8::Engine;

fn new_engine(document: tenta_dom::SharedDocument) -> Engine {
    Engine::with_document(document)
}

include!("../../tenta-js/tests/support/listener_cycles.rs");
