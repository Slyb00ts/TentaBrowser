//! Attr-as-Node behavior through real QuickJS generated bindings.

use std::cell::RefCell;
use std::rc::Rc;
use tenta_dom::Document;
use tenta_js_qjs::Engine;

fn evaluate(source: &str) -> String {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = Engine::with_document(document).expect("QuickJS runtime");
    engine.evaluate(source).expect("Attr script")
}

fn new_engine(document: Rc<RefCell<Document>>) -> Engine {
    Engine::with_document(document).expect("QuickJS runtime")
}

include!("../../tenta-js/tests/support/attr_node.rs");
