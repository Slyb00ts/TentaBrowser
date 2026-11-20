//! Table, select, input, label and other form element members through
//! generated QuickJS bindings.

use tenta_js_qjs::Engine;

fn new_engine(document: tenta_dom::SharedDocument) -> Engine {
    Engine::with_document(document).expect("QuickJS runtime")
}

include!("../../tenta-js/tests/support/form_members.rs");
