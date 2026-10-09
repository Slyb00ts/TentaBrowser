//! Attr-as-Node behavior through real V8 generated bindings.

use std::cell::RefCell;
use std::rc::Rc;
use tenta_dom::Document;
use tenta_js_v8::Engine;

fn evaluate(source: &str) -> String {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = Engine::with_document(document);
    engine.evaluate(source).expect("Attr script")
}

fn new_engine(document: Rc<RefCell<Document>>) -> Engine {
    Engine::with_document(document)
}

include!("../../tenta-js/tests/support/attr_node.rs");

#[test]
fn indexed_named_node_map_access_reuses_the_attr_wrapper() {
    assert_eq!(
        evaluate(
            "const element = document.createElement('div'); \
             element.setAttribute('id', 'one'); \
             const attr = element.getAttributeNode('id'); \
             element.attributes[0] === attr",
        ),
        "true"
    );
}

#[test]
fn finalized_attr_and_child_list_can_be_rewrapped_before_native_reclamation() {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = Engine::with_document(document);
    engine
        .evaluate(
            "globalThis.element = document.createElement('div'); \
             element.setAttribute('id', 'held'); \
             element.getAttributeNode('id');",
        )
        .expect("mint an Attr without retaining its wrapper");
    engine.low_memory_gc();
    assert_eq!(
        engine
            .evaluate("element.getAttributeNode('id').value")
            .expect("rewrap a cached Attr after its old wrapper finalized"),
        "held"
    );

    engine
        .evaluate("globalThis.attr = element.getAttributeNode('id'); attr.childNodes;")
        .expect("mint an Attr childNodes list without retaining its wrapper");
    engine.low_memory_gc();
    assert_eq!(
        engine
            .evaluate("attr.childNodes.length")
            .expect("rewrap the cached childNodes snapshot"),
        "0"
    );
}
