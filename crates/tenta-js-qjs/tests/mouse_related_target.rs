//! QuickJS MouseEvent.relatedTarget ownership through the shared Event graph.

use tenta_js_qjs::Engine;

fn new_engine(document: tenta_dom::SharedDocument) -> Engine {
    Engine::with_document(document).expect("QuickJS runtime")
}

include!("../../tenta-js/tests/support/mouse_related_target.rs");
