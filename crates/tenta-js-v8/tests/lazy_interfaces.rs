//! Interface objects are lazy data properties of the global, built on first
//! read or with the first wrapper of their interface, and they look and link
//! as the eagerly installed ones did.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::Document;
use tenta_js_v8::Engine;

fn eval(source: &str) -> String {
    let mut engine = Engine::with_document(Rc::new(RefCell::new(Document::new())));
    engine.evaluate(source).expect("interface script")
}

#[test]
fn an_interface_object_is_a_non_enumerable_data_property() {
    assert_eq!(
        eval(
            "var d = Object.getOwnPropertyDescriptor(globalThis, 'HTMLTableElement'); \
             [typeof d.value, d.writable, d.enumerable, d.configurable, \
              'HTMLMeterElement' in globalThis, Object.keys(globalThis).indexOf('Node'), \
              d.value === HTMLTableElement, Object.getPrototypeOf(HTMLTableElement) === HTMLElement, \
              Object.getPrototypeOf(HTMLElement) === Element, Object.getPrototypeOf(Node) === EventTarget, \
              Object.getPrototypeOf(EventTarget) === Function.prototype].join(',')"
        ),
        "function,true,false,true,true,-1,true,true,true,true,true"
    );
}

#[test]
fn a_wrapper_made_first_has_a_linked_constructor() {
    assert_eq!(
        eval(
            "var row = document.createElement('tr'); var ctor = row.constructor; \
             [ctor.name, Object.getPrototypeOf(ctor) === HTMLElement, ctor === HTMLTableRowElement, \
              row instanceof HTMLTableRowElement, \
              Object.getPrototypeOf(document.createElement('progress').constructor).name].join(',')"
        ),
        "HTMLTableRowElement,true,true,true,HTMLElement"
    );
}

#[test]
fn a_page_can_replace_or_delete_an_interface_it_never_read() {
    assert_eq!(
        eval(
            "HTMLMeterElement = 1; var deleted = delete globalThis.HTMLDetailsElement; \
             var details = document.createElement('details'); \
             [HTMLMeterElement, deleted, 'HTMLDetailsElement' in globalThis, \
              details.constructor.name, Object.getPrototypeOf(details.constructor) === HTMLElement, \
              typeof document.createElement('meter').constructor].join(',')"
        ),
        "1,true,false,HTMLDetailsElement,true,function"
    );
}
