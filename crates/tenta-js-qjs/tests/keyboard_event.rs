use tenta_js_qjs::Engine;

fn new_engine(document: tenta_dom::SharedDocument) -> Engine {
    Engine::with_document(document).expect("QuickJS keyboard engine")
}

include!("../../tenta-js/tests/support/keyboard_event.rs");
