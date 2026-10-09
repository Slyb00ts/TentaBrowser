//! Native DOM reachability must preserve observable JavaScript wrapper state.

use tenta_js_v8::Engine;

fn new_engine(document: tenta_dom::SharedDocument) -> Engine {
    Engine::with_document(document)
}

include!("../../tenta-js/tests/support/wrapper_state.rs");
