use std::cell::RefCell;
use std::rc::Rc;
use tenta_js_v8::Engine;

fn new_engine() -> Engine {
    Engine::with_document(Rc::new(RefCell::new(tenta_dom::Document::new())))
}

include!("../../tenta-js/tests/support/listener_conversion_lease.rs");
