//! Qualified element names through the real v8 bindings.

fn new_engine() -> tenta_js_v8::Engine {
    let document = std::rc::Rc::new(std::cell::RefCell::new(tenta_dom::Document::new()));
    tenta_js_v8::Engine::with_document(document)
}

include!("../../tenta-js/tests/support/dom_names.rs");
