use tenta_js_qjs::Engine;

fn new_engine(document: tenta_dom::SharedDocument) -> Engine {
    Engine::with_document(document).expect("QuickJS composition engine")
}

include!("../../tenta-js/tests/support/composition_event.rs");
