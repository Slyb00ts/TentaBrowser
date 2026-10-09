//! Real V8 wrapper-graph admission and bounded collection controls.

use tenta_js_v8::Engine;

fn new_engine(document: tenta_dom::SharedDocument) -> Engine {
    Engine::with_document(document)
}

include!("../../tenta-js/tests/support/wrapper_pressure.rs");

#[test]
fn composed_path_batch_publication_refuses_without_losing_the_original_error() {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Duration;
    use tenta_core::atom::atoms;
    use tenta_dom::Mutation;
    use tenta_html::QualName;
    use tenta_js::engine::PageEngine;

    let document = Rc::new(RefCell::new(tenta_dom::Document::new()));
    // Build natively so the ancestors do not have existing JS wrappers. The
    // one composedPath() result must publish the whole interface sequence in
    // a single callback frame and hit the hard admission bound partway through.
    {
        let mut doc = document.borrow_mut();
        let mut parent = doc.document_node();
        for _ in 0..4300 {
            let child = doc.create_html_element(atoms::DIV);
            doc.mutate(Mutation::Append { parent, child })
                .expect("build connected event path");
            parent = child;
        }
        doc.mutate(Mutation::SetAttribute {
            element: parent,
            name: QualName::new(atoms::EMPTY, atoms::ID),
            value: "tail".to_owned(),
        })
        .expect("index event target");
    }
    let mut engine = Engine::with_document(document.clone());
    let result = PageEngine::evaluate(
        &mut engine,
        "globalThis.tail=document.getElementById('tail');\
         globalThis.observed='not-run';\
         tail.addEventListener('x',e=>{try{e.composedPath();observed='no-error'}\
         catch(error){observed=error.name+'|'+error.message}});\
         tail.dispatchEvent(new Event('x'));observed",
        Duration::from_secs(30),
    )
    .expect("caught outgoing interface sequence refusal");
    assert_eq!(
        result,
        "RangeError|native wrapper allocation limit exceeded"
    );
    assert_eq!(document.borrow().len(), 4301);
    assert_eq!(engine.native_failure(), None);
    assert_eq!(
        PageEngine::evaluate(
            &mut engine,
            "document.createElement('i').nodeName",
            Duration::from_secs(5),
        )
        .expect("the next task can allocate after graph recovery"),
        "I"
    );
}
