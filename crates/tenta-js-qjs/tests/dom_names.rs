//! Qualified element names through the real qjs bindings.

fn new_engine() -> tenta_js_qjs::Engine {
    let document = std::rc::Rc::new(std::cell::RefCell::new(tenta_dom::Document::new()));
    tenta_js_qjs::Engine::with_document(document).expect("QuickJS runtime")
}

include!("../../tenta-js/tests/support/dom_names.rs");
