// Shared real-engine snapshot lifetime tests included by both backends.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use tenta_dom::{Document, SharedDocument};
use tenta_js::engine::{PageEngine, ScriptKind};

fn collect_detached(document: &SharedDocument, engine: &mut Engine) -> usize {
    let mut freed = 0;
    for _ in 0..3 {
        engine.run_microtasks();
        assert!(
            engine.native_failure().is_none(),
            "microtask native failure"
        );
        engine.trim_memory();
        assert!(engine.native_failure().is_none(), "GC native failure");
        freed += document.borrow_mut().sweep_detached().nodes_freed;
        engine.trim_memory();
        assert!(engine.native_failure().is_none(), "drain native failure");
    }
    freed
}

fn snapshot_alone_keeps_a_detached_subtree(query: &str) {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = new_engine(document.clone());
    engine
        .run_script(
            &format!(
                "(()=>{{const root=document.createElement('div'); \
                 const child=document.createElement('span');root.appendChild(child); \
                 globalThis.heldList=root.{query};}})();"
            ),
            ScriptKind::Classic,
        )
        .expect("create only a static collection root");
    assert_eq!(document.borrow().len(), 3);
    assert_eq!(collect_detached(&document, &mut engine), 0);
    assert_eq!(document.borrow().len(), 3);
    assert_eq!(
        PageEngine::evaluate(
            &mut engine,
            "heldList.item(0).parentNode.nodeName",
            Duration::from_secs(5),
        )
        .expect("resolve the pinned result and its detached parent"),
        "DIV"
    );
    engine
        .run_script("globalThis.heldList = null;", ScriptKind::Classic)
        .expect("release the only remaining script root");
    assert_eq!(collect_detached(&document, &mut engine), 2);
    assert_eq!(document.borrow().len(), 1);
}

#[test]
fn query_selector_all_snapshot_pins_detached_results_until_list_gc() {
    snapshot_alone_keeps_a_detached_subtree("querySelectorAll('span')");
}

#[test]
fn static_html_collection_snapshot_pins_detached_results_until_list_gc() {
    snapshot_alone_keeps_a_detached_subtree("getElementsByTagName('span')");
}
