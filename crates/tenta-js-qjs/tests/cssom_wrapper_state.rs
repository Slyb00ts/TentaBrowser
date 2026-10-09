//! Constructed CSSOM wrapper ownership through QuickJS.

use tenta_js_qjs::Engine;

fn new_engine(document: tenta_dom::SharedDocument) -> Engine {
    Engine::with_document(document).expect("QuickJS runtime")
}

include!("../../tenta-js/tests/support/cssom_wrapper_state.rs");
