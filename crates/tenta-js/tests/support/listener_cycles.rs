// Shared behavioral tests included by the V8 and QuickJS engine adapters.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::{Document, Mutation, SharedDocument};
use tenta_js::engine::{PageEngine, ScriptKind};

fn engine_over_empty_document() -> (SharedDocument, Engine) {
    let document = Rc::new(RefCell::new(Document::new()));
    let engine = new_engine(document.clone());
    (document, engine)
}

fn collect_detached_nodes(document: &SharedDocument, engine: &mut Engine) -> usize {
    let mut freed = 0;
    // A collection releases wrappers; a native sweep then releases listener
    // ownership. Repeating the two explicit safe points requires no timing.
    for _ in 0..3 {
        engine.run_microtasks();
        engine.trim_memory();
        freed += document.borrow_mut().sweep_detached().nodes_freed;
        engine.trim_memory();
    }
    freed
}

#[test]
fn detached_node_with_noncapturing_listener_is_collected() {
    let (document, mut engine) = engine_over_empty_document();
    engine
        .run_script(
            "globalThis.emptyListener=()=>{}; \
             (()=>{const node=document.createElement('div'); \
                 node.addEventListener('probe',emptyListener);})();",
            ScriptKind::Classic,
        )
        .expect("register a listener without a reference to its node");
    assert_eq!(document.borrow().len(), 2);
    assert_eq!(collect_detached_nodes(&document, &mut engine), 1);
    assert_eq!(document.borrow().len(), 1);
}

#[test]
fn detached_node_captured_only_by_its_listener_is_collected() {
    let (document, mut engine) = engine_over_empty_document();
    engine
        .run_script(
            "(()=>{const node=document.createElement('div'); \
                 node.addEventListener('probe',()=>node.nodeName);})();",
            ScriptKind::Classic,
        )
        .expect("register a listener that closes over its detached node");
    assert_eq!(document.borrow().len(), 2);
    assert_eq!(
        collect_detached_nodes(&document, &mut engine),
        1,
        "the node and its listener have no root outside their ownership cycle"
    );
    assert_eq!(document.borrow().len(), 1);
}

#[test]
fn detached_nodes_captured_only_by_each_others_listeners_are_collected() {
    let (document, mut engine) = engine_over_empty_document();
    engine
        .run_script(
            "(()=>{const first=document.createElement('div'); \
                 const second=document.createElement('span'); \
                 first.addEventListener('probe',()=>second.nodeName); \
                 second.addEventListener('probe',()=>first.nodeName);})();",
            ScriptKind::Classic,
        )
        .expect("register listeners that close over different detached roots");
    assert_eq!(document.borrow().len(), 3);
    assert_eq!(
        collect_detached_nodes(&document, &mut engine),
        2,
        "neither listener cycle has an independent script or native root"
    );
    assert_eq!(document.borrow().len(), 1);
}

#[test]
fn removing_a_capturing_listener_allows_collection() {
    let (document, mut engine) = engine_over_empty_document();
    engine
        .run_script(
            "(()=>{const node=document.createElement('div'); \
                 const listener=()=>node.nodeName; \
                 node.addEventListener('probe',listener); \
                 node.removeEventListener('probe',listener);})();",
            ScriptKind::Classic,
        )
        .expect("remove the listener before dropping the node");
    assert_eq!(document.borrow().len(), 2);
    assert_eq!(collect_detached_nodes(&document, &mut engine), 1);
    assert_eq!(document.borrow().len(), 1);
}

#[test]
fn reachable_detached_node_keeps_its_capturing_listener() {
    let (document, mut engine) = engine_over_empty_document();
    engine
        .run_script(
            "(()=>{const node=document.createElement('div');globalThis.keep=node; \
                 node.addEventListener('probe',()=>{globalThis.fired=node.nodeName});})();",
            ScriptKind::Classic,
        )
        .expect("retain the node independently of its listener");
    assert_eq!(collect_detached_nodes(&document, &mut engine), 0);
    assert_eq!(document.borrow().len(), 2);
    engine
        .run_script(
            "keep.dispatchEvent(new Event('probe')); \
             if(globalThis.fired!=='DIV')throw Error('listener was released while its node was live');",
            ScriptKind::Classic,
        )
        .expect("the listener survives as long as its node is reachable");
}

#[test]
fn connected_node_keeps_its_capturing_listener_without_a_script_reference() {
    let (document, mut engine) = engine_over_empty_document();
    engine
        .run_script(
            "(()=>{const root=document.createElement('div');document.appendChild(root); \
                 const node=document.createElement('span');root.appendChild(node); \
                 node.addEventListener('probe',()=>{globalThis.fired=node.nodeName});})();",
            ScriptKind::Classic,
        )
        .expect("retain the target through the connected DOM tree");
    assert_eq!(collect_detached_nodes(&document, &mut engine), 0);
    assert_eq!(document.borrow().len(), 3);
    engine
        .run_script(
            "document.documentElement.firstChild.dispatchEvent(new Event('probe')); \
             if(globalThis.fired!=='SPAN')throw Error('connected listener was released');",
            ScriptKind::Classic,
        )
        .expect("the connected node still owns its working listener");
}

#[test]
fn reachable_detached_subtree_keeps_a_descendants_capturing_listener() {
    let (document, mut engine) = engine_over_empty_document();
    engine
        .run_script(
            "(()=>{const root=document.createElement('div');globalThis.keep=root; \
                 const node=document.createElement('span');root.appendChild(node); \
                 node.addEventListener('probe',()=>{globalThis.fired=node.nodeName});})();",
            ScriptKind::Classic,
        )
        .expect("retain only the detached ancestor directly from script");
    assert_eq!(collect_detached_nodes(&document, &mut engine), 0);
    assert_eq!(document.borrow().len(), 3);
    engine
        .run_script(
            "keep.firstChild.dispatchEvent(new Event('probe')); \
             if(globalThis.fired!=='SPAN')throw Error('descendant listener was released');",
            ScriptKind::Classic,
        )
        .expect("the reachable subtree retains its descendant's listener");
}

#[test]
fn native_handle_retains_a_capturing_listener_until_its_release() {
    let (document, mut engine) = engine_over_empty_document();
    engine
        .run_script(
            "(()=>{const node=document.createElement('div');document.appendChild(node); \
                 node.addEventListener('probe',()=>{globalThis.fired=node.nodeName});})();",
            ScriptKind::Classic,
        )
        .expect("register a listener before the native host takes its node");
    let node = document.borrow().document_element().expect("root element");
    let pin = document.borrow_mut().handle(node).expect("native node pin");
    document
        .borrow_mut()
        .mutate(Mutation::Remove { child: node })
        .expect("detach the natively held node");
    assert_eq!(collect_detached_nodes(&document, &mut engine), 0);
    assert_eq!(document.borrow().len(), 2);

    // Reattachment makes the natively retained target accessible to script
    // without introducing an additional script reference during collection.
    let parent = document.borrow().document_node();
    document
        .borrow_mut()
        .mutate(Mutation::Append {
            parent,
            child: node,
        })
        .expect("reattach the natively held node");
    engine
        .run_script(
            "document.documentElement.dispatchEvent(new Event('probe')); \
             if(globalThis.fired!=='DIV')throw Error('natively held listener was released');",
            ScriptKind::Classic,
        )
        .expect("native retention preserves the listener itself");
    document
        .borrow_mut()
        .mutate(Mutation::Remove { child: node })
        .expect("detach the target before releasing its final external root");
    document.borrow_mut().release(pin);
    assert_eq!(collect_detached_nodes(&document, &mut engine), 1);
    assert_eq!(document.borrow().len(), 1);
}

#[test]
fn a_live_collection_retains_its_subtrees_listener() {
    let (document, mut engine) = engine_over_empty_document();
    engine.run_script(
        "(()=>{const root=document.createElement('div');const child=document.createElement('span'); \
         root.appendChild(child);globalThis.keep=root.childNodes; \
         child.addEventListener('probe',()=>{globalThis.fired=child.nodeName});})();",
        ScriptKind::Classic,
    ).expect("retain only the live collection wrapper");
    assert_eq!(collect_detached_nodes(&document, &mut engine), 0);
    engine
        .run_script(
            "keep.item(0).dispatchEvent(new Event('probe')); \
         if(globalThis.fired!=='SPAN')throw Error('collection lost its listener'); \
         globalThis.keep=null;",
            ScriptKind::Classic,
        )
        .expect("a collection retains the actual descendant listener");
    assert_eq!(collect_detached_nodes(&document, &mut engine), 2);
}

#[test]
fn a_static_snapshot_retains_its_results_capturing_listener() {
    let (document, mut engine) = engine_over_empty_document();
    engine.run_script(
        "(()=>{const root=document.createElement('div');const child=document.createElement('span'); \
         root.appendChild(child);globalThis.keep=root.querySelectorAll('span'); \
         child.addEventListener('probe',()=>{globalThis.fired=child.nodeName});})();",
        ScriptKind::Classic,
    ).expect("a static snapshot owns native pins for its results");
    assert_eq!(collect_detached_nodes(&document, &mut engine), 0);
    engine.run_script(
        "keep.item(0).dispatchEvent(new Event('probe')); \
         if(globalThis.fired!=='SPAN')throw Error('snapshot lost its listener');globalThis.keep=null;",
        ScriptKind::Classic,
    ).expect("a live snapshot preserves the actual listener callback");
    assert_eq!(collect_detached_nodes(&document, &mut engine), 2);
}

#[test]
fn duplicate_callback_owners_are_counted_separately() {
    let (document, mut engine) = engine_over_empty_document();
    engine
        .run_script(
        "globalThis.fired=0;(()=>{const node=document.createElement('div');const listener=()=>{fired++;return node.nodeName}; \
         node.addEventListener('first',listener);node.addEventListener('second',listener); \
         node.addEventListener('third',listener);})();",
            ScriptKind::Classic,
        )
        .expect("three native registrations share one callback slot");
    assert_eq!(collect_detached_nodes(&document, &mut engine), 1);
    // Reusing arena and callback slots must not resurrect collected callbacks.
    engine
        .run_script(
        "const replacement=document.createElement('div');replacement.dispatchEvent(new Event('first')); \
         if(fired!==0)throw Error('stale event listener metadata');",
            ScriptKind::Classic,
        )
        .expect("released registrations cannot alias a replacement node");
}

#[test]
fn a_pending_timer_retains_a_shared_listener_until_it_fires() {
    let (document, mut engine) = engine_over_empty_document();
    engine
        .run_script(
            "globalThis.fired=0;(()=>{const node=document.createElement('div'); \
         const listener=()=>{globalThis.fired++;return node.nodeName}; \
         node.addEventListener('probe',listener);setTimeout(listener,1);})();",
            ScriptKind::Classic,
        )
        .expect("a timer independently owns the same callback");
    assert_eq!(collect_detached_nodes(&document, &mut engine), 0);
    engine.run_event_loop(10);
    engine
        .run_script(
            "if(fired!==1)throw Error('timer callback was collected')",
            ScriptKind::Classic,
        )
        .expect("the rooted timer executes exactly once");
    assert_eq!(collect_detached_nodes(&document, &mut engine), 1);
}

#[test]
fn a_native_event_detail_owner_prevents_listener_weakening() {
    let (document, mut engine) = engine_over_empty_document();
    engine
        .run_script(
            "(()=>{const node=document.createElement('div');const listener=()=>node.nodeName; \
         node.addEventListener('probe',listener); \
         globalThis.keep=new CustomEvent('detail',{detail:listener});})();",
            ScriptKind::Classic,
        )
        .expect("CustomEvent detail independently owns the same slab value");
    assert_eq!(collect_detached_nodes(&document, &mut engine), 0);
    engine.run_script(
        "if(keep.detail()!=='DIV')throw Error('native detail was collected');globalThis.keep=null;",
        ScriptKind::Classic,
    ).expect("the non-listener stored reference remains valid");
    assert_eq!(collect_detached_nodes(&document, &mut engine), 1);
}

#[test]
fn collector_uses_pristine_intrinsics_without_page_setters() {
    let (document, mut engine) = engine_over_empty_document();
    engine.run_script(
        "(()=>{const node=document.createElement('div');node.addEventListener('probe',()=>node.nodeName)})(); \
         globalThis.collectorPageCalls=0;const hostile=()=>{collectorPageCalls++;throw Error('page intrinsic')}; \
         WeakMap.prototype.set=hostile;WeakRef.prototype.deref=hostile; \
         globalThis.WeakMap=hostile;globalThis.WeakRef=hostile; \
         Object.defineProperty(Object.prototype,'0',{set:hostile,configurable:true});",
        ScriptKind::Classic,
    ).expect("replace page-visible collection intrinsics");
    assert_eq!(collect_detached_nodes(&document, &mut engine), 1);
    engine.run_script(
        "delete Object.prototype['0'];if(collectorPageCalls!==0)throw Error('collector invoked page code')",
        ScriptKind::Classic,
    ).expect("collection never calls page-owned constructors, methods, or setters");
}
