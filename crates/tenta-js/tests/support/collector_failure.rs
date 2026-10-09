// Included by backend-private unit-test adapters; no production fault hooks.

use tenta_js::engine::{LifecycleEvent, PageEngine};

#[test]
fn failed_post_gc_listener_restoration_poisons_the_engine() {
    let mut engine = new_cycle_engine();
    engine
        .evaluate(
            "(()=>{const node=document.createElement('div');document.appendChild(node); \
         node.addEventListener('probe',()=>node.nodeName);})(); \
         document.addEventListener('DOMContentLoaded',()=>document.createElement('b')); \
         setTimeout(()=>document.createElement('i'),0); \
         Promise.resolve().then(()=>document.createElement('u'));",
        )
        .expect("queue independent listener, timer, and microtask effects");
    let context = cycle_context(&engine);
    let node = context.doc.borrow().document_element().unwrap();
    context
        .doc
        .borrow_mut()
        .mutate(tenta_dom::Mutation::Remove { child: node })
        .unwrap();
    let count = context.doc.borrow().len();
    // Preparation reads listeners/documents; restoration of a dead listener
    // must also remove handler metadata. This real conflicting native borrow
    // makes that step panic after GC, without injecting a success/failure flag.
    let held = context.handlers.borrow_mut();
    engine.trim_memory();
    assert_eq!(engine.native_failure(), Some(expected_cycle_boundary()));
    assert!(
        retains_failed_cycle(&engine),
        "recovery graph remains owned after failure"
    );
    drop(held);

    engine.run_microtasks();
    assert_eq!(engine.pump_due_timers(10), 0);
    engine.dispatch_lifecycle(LifecycleEvent::DomContentLoaded);
    assert!(engine.evaluate("document.createElement('em')").is_err());
    assert_eq!(
        context.doc.borrow().len(),
        count,
        "no queued script runs after poison"
    );
    assert!(retains_failed_cycle(&engine));
    drop(engine);
}

#[test]
fn collected_node_handler_releases_its_metadata_before_slot_reuse() {
    use tenta_js::dom::events::{EventHandlerName, TargetKey};

    let mut engine = new_cycle_engine();
    engine
        .evaluate(
            "(()=>{const node=document.createElement('div');document.appendChild(node); \
         node.addEventListener('probe',()=>node.nodeName);})();",
        )
        .expect("create a real stored callback");
    let context = cycle_context(&engine);
    let node = context.doc.borrow().document_element().unwrap();
    let reference = context.doc.borrow().node_ref(node);
    context
        .doc
        .borrow_mut()
        .mutate(tenta_dom::Mutation::Remove { child: node })
        .unwrap();
    // Transfer one owned reference from addEventListener to a native handler.
    // HTMLElement's GlobalEventHandlers IDL is a separate unfinished surface.
    let mut listeners = context.listeners.borrow_mut();
    let callback = listeners.snapshot(reference, "probe", false)[0].callback;
    assert_eq!(
        listeners.remove(reference, "probe", callback, false),
        Some(callback)
    );
    let target = TargetKey::Node(reference);
    let id = listeners.add_handler_at(target, "click".into(), callback);
    drop(listeners);
    context
        .handlers
        .borrow_mut()
        .insert((target, EventHandlerName::new("onclick")), id);
    engine.trim_memory();
    assert_eq!(engine.native_failure(), None);
    assert!(context.handlers.borrow().is_empty());
    assert!(context.listeners.borrow().is_empty());
    assert_eq!(context.doc.borrow_mut().sweep_detached().nodes_freed, 1);
}
