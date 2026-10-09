// Shared real-engine regressions for observable JavaScript wrapper state.
// A native-reachable object must retain the state of its JavaScript wrapper.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use tenta_dom::{Document, Mutation, SharedDocument};
use tenta_js::engine::{PageEngine, ScriptKind};

fn document_and_engine() -> (SharedDocument, Engine) {
    let document = Rc::new(RefCell::new(Document::new()));
    let engine = new_engine(document.clone());
    (document, engine)
}

fn evaluate(engine: &mut Engine, source: &str) -> String {
    PageEngine::evaluate(engine, source, Duration::from_secs(5))
        .expect("read observable wrapper state")
}

fn collect(document: &SharedDocument, engine: &mut Engine) -> usize {
    let mut freed = 0;
    for _ in 0..3 {
        engine.run_microtasks();
        engine.trim_memory();
        assert!(
            engine.native_failure().is_none(),
            "native collection failure"
        );
        freed += document.borrow_mut().sweep_detached().nodes_freed;
        engine.trim_memory();
        assert!(engine.native_failure().is_none(), "native drain failure");
    }
    freed
}

fn retains_state(setup: &str, query: &str, expected: &str, nodes: usize) {
    let (document, mut engine) = document_and_engine();
    engine
        .run_script(setup, ScriptKind::Classic)
        .expect("create wrapper state");
    let before = evaluate(&mut engine, query);
    let freed = collect(&document, &mut engine);
    let after = evaluate(&mut engine, query);
    assert_eq!(freed, 0, "reachable native subtree must survive");
    assert_eq!(document.borrow().len(), nodes);
    assert_eq!(
        (before.as_str(), after.as_str()),
        (expected, expected),
        "wrapper state must survive both ordinary access and explicit collection"
    );
}

#[test]
fn connected_node_keeps_expando_when_reacquired_from_native_tree() {
    retains_state(
        "(()=>{const root=document.createElement('div');document.appendChild(root); \
         const child=root.appendChild(document.createElement('span'));child.id='target'; \
         child[Symbol.for('state')]=41;child.note='connected';})();",
        "[document.getElementById('target')[Symbol.for('state')], \
         document.documentElement.firstChild.note].join('|')",
        "41|connected",
        3,
    );
}

#[test]
fn held_owner_keeps_style_wrapper_state() {
    retains_state(
        "globalThis.owner=document.createElement('div'); \
         owner.style[Symbol.for('state')]=42;owner.style.note='style';",
        "[owner.style[Symbol.for('state')],owner.style.note].join('|')",
        "42|style",
        2,
    );
}

#[test]
fn held_owner_keeps_attributes_wrapper_state() {
    retains_state(
        "globalThis.owner=document.createElement('div'); \
         owner.attributes[Symbol.for('state')]=43;owner.attributes.note='attributes';",
        "[owner.attributes[Symbol.for('state')],owner.attributes.note].join('|')",
        "43|attributes",
        2,
    );
}

#[test]
fn held_owner_keeps_class_list_wrapper_state() {
    retains_state(
        "globalThis.owner=document.createElement('div'); \
         owner.classList[Symbol.for('state')]=44;owner.classList.note='tokens';",
        "[owner.classList[Symbol.for('state')],owner.classList.note].join('|')",
        "44|tokens",
        2,
    );
}

#[test]
fn held_detached_ancestor_keeps_descendant_wrapper_state() {
    retains_state(
        "globalThis.root=document.createElement('section'); \
         (()=>{const child=root.appendChild(document.createElement('b')); \
         child[Symbol.for('state')]=45;child.note='descendant';})();",
        "[root.firstChild[Symbol.for('state')],root.firstChild.note].join('|')",
        "45|descendant",
        3,
    );
}

#[test]
fn held_detached_descendant_keeps_ancestor_wrapper_state() {
    retains_state(
        "globalThis.leaf=(()=>{const root=document.createElement('section'); \
         root[Symbol.for('state')]=46;root.note='ancestor'; \
         return root.appendChild(document.createElement('b'));})();",
        "[leaf.parentNode[Symbol.for('state')],leaf.parentNode.note].join('|')",
        "46|ancestor",
        3,
    );
}

#[test]
fn a_script_held_node_keeps_its_own_wrapper_state() {
    retains_state(
        "globalThis.owner=document.createElement('div'); \
         owner[Symbol.for('state')]=47;owner.note='direct';",
        "[owner[Symbol.for('state')],owner.note].join('|')",
        "47|direct",
        2,
    );
}

#[test]
fn an_unreachable_group_with_wrapper_cycles_is_collectible() {
    let (document, mut engine) = document_and_engine();
    engine
        .run_script(
            "(()=>{const root=document.createElement('section'); \
         const child=root.appendChild(document.createElement('b')); \
         root.peer=child;child.peer=root;root.style.owner=root; \
         root.attributes[Symbol('owner')]=root;root.classList.owner=child;})();",
            ScriptKind::Classic,
        )
        .expect("create unreachable cycles through nodes and owner wrappers");
    assert_eq!(document.borrow().len(), 3);
    assert_eq!(collect(&document, &mut engine), 2);
    assert_eq!(document.borrow().len(), 1);
}

#[test]
fn native_node_handle_keeps_wrapper_state_until_its_release() {
    let (document, mut engine) = document_and_engine();
    engine
        .run_script(
            "globalThis.temporary=document.appendChild(document.createElement('div')); \
         temporary[Symbol.for('state')]=48;temporary.note='native';",
            ScriptKind::Classic,
        )
        .expect("retain a wrapper while the native host acquires its node");
    let node = document.borrow().document_element().expect("native root");
    let pin = document.borrow_mut().handle(node).expect("native node pin");
    document
        .borrow_mut()
        .mutate(Mutation::Remove { child: node })
        .unwrap();
    engine
        .run_script("globalThis.temporary=null", ScriptKind::Classic)
        .unwrap();
    assert_eq!(collect(&document, &mut engine), 0);
    let parent = document.borrow().document_node();
    document
        .borrow_mut()
        .mutate(Mutation::Append {
            parent,
            child: node,
        })
        .unwrap();
    let state = evaluate(
        &mut engine,
        "[document.documentElement[Symbol.for('state')], \
         document.documentElement.note].join('|')",
    );
    document
        .borrow_mut()
        .mutate(Mutation::Remove { child: node })
        .unwrap();
    document.borrow_mut().release(pin);
    assert_eq!(collect(&document, &mut engine), 1);
    assert_eq!(state, "48|native");
}

#[test]
fn retained_subtree_keeps_prototype_and_accessor_state() {
    retains_state(
        "globalThis.root=document.createElement('div'); \
         (()=>{const child=root.appendChild(document.createElement('b')); \
         const prototype=Object.create(Object.getPrototypeOf(child)); \
         prototype.inherited='prototype';Object.setPrototypeOf(child,prototype); \
         Object.defineProperty(child,'readBack',{get(){return this.nodeName+'!'}});})();",
        "[root.firstChild.inherited,root.firstChild.readBack].join('|')",
        "prototype|B!",
        3,
    );
}

#[test]
fn reparenting_moves_wrapper_state_to_the_new_retained_subtree() {
    let (document, mut engine) = document_and_engine();
    engine
        .run_script(
            "globalThis.source=document.createElement('div'); \
         globalThis.destination=document.createElement('section'); \
         destination.note='destination'; \
         (()=>{const child=source.appendChild(document.createElement('b')); \
         child.note='child';})(); \
         destination.appendChild(source.firstChild); \
         globalThis.kept=destination.firstChild;source=null;destination=null;",
            ScriptKind::Classic,
        )
        .expect("reparent before releasing both old direct parent references");
    let before = evaluate(&mut engine, "[kept.note,kept.parentNode.note].join('|')");
    assert_eq!(
        collect(&document, &mut engine),
        1,
        "old empty tree must collect"
    );
    assert_eq!(document.borrow().len(), 3);
    let after = evaluate(&mut engine, "[kept.note,kept.parentNode.note].join('|')");
    engine
        .run_script("globalThis.kept=null", ScriptKind::Classic)
        .unwrap();
    assert_eq!(
        collect(&document, &mut engine),
        2,
        "new group must collect after release"
    );
    assert_eq!(
        (before.as_str(), after.as_str()),
        ("child|destination", "child|destination")
    );
}

#[test]
fn dictionary_interface_arguments_keep_state_through_later_getters() {
    retains_state(
        "globalThis.convertedRange=new StaticRange({ \
         get startContainer(){const n=document.createElement('b'); \
           n[Symbol.for('argument-state')]='start';return n}, \
         get startOffset(){document.nodeType;return 0}, \
         get endContainer(){const n=document.createElement('i'); \
           n[Symbol.for('argument-state')]='end';return n}, \
         get endOffset(){document.nodeType;return 0}});",
        "[convertedRange.startContainer[Symbol.for('argument-state')], \
         convertedRange.endContainer[Symbol.for('argument-state')]].join('|')",
        "start|end",
        3,
    );
}

#[test]
fn sequence_interface_arguments_survive_later_iterator_reentry() {
    let (_, mut engine) = document_and_engine();
    let size = evaluate(
        &mut engine,
        "(()=>{const parts={[Symbol.iterator](){let index=0;return{next(){ \
         if(index++===0)return{value:new Blob(['payload']),done:false}; \
         document.nodeType;return{done:true}}}}};return new Blob(parts).size})()",
    );
    assert_eq!(size, "7");
}

#[test]
fn converted_dictionary_argument_keeps_state_for_native_lookup() {
    let (document, mut engine) = document_and_engine();
    engine
        .run_script(
            "globalThis.lookupArgumentSeen='unvisited'; \
         new StaticRange({endContainer:document,endOffset:0, \
         get startContainer(){const n=document.createElement('b'); \
           n.id='temporary-argument';n.note='original';document.appendChild(n);return n}, \
         get startOffset(){lookupArgumentSeen=String( \
           document.getElementById('temporary-argument').note); \
           document.removeChild(document.documentElement);return 0}})",
            ScriptKind::Classic,
        )
        .expect("convert a temporary endpoint exposed by the native document");
    let seen = evaluate(&mut engine, "lookupArgumentSeen");
    let freed = collect(&document, &mut engine);
    assert_eq!(seen, "original");
    assert_eq!(freed, 1, "temporary endpoint must collect after conversion");
}

#[test]
fn converted_dictionary_arguments_are_live_during_later_getters() {
    let (document, mut engine) = document_and_engine();
    engine
        .run_script(
            "globalThis.argumentSeen=[];globalThis.argumentWeak=[]; \
         (()=>{let start,end;new StaticRange({ \
         get startContainer(){const n=document.createElement('b');n.note='start'; \
           start=new WeakRef(n);argumentWeak.push(start);return n}, \
         get startOffset(){document.nodeType; \
           argumentSeen.push(start.deref()?.note??'missing');return 0}, \
         get endContainer(){const n=document.createElement('i');n.note='end'; \
           end=new WeakRef(n);argumentWeak.push(end);return n}, \
         get endOffset(){document.nodeType; \
           argumentSeen.push(end.deref()?.note??'missing');return 0}})})();",
            ScriptKind::Classic,
        )
        .expect("complete real StaticRange dictionary conversion");
    // Member ordering is independent of this lifetime assertion.
    let seen = evaluate(&mut engine, "argumentSeen.slice().sort().join('|')");
    let freed = collect(&document, &mut engine);
    let released = evaluate(
        &mut engine,
        "argumentWeak.every(reference=>reference.deref()===undefined)",
    );
    assert_eq!(seen, "end|start");
    assert_eq!(freed, 2, "completed arguments must not remain rooted");
    assert_eq!(released, "true");
}

#[test]
fn failed_dictionary_conversion_releases_earlier_interface_arguments() {
    let (document, mut engine) = document_and_engine();
    engine
        .run_script(
            "globalThis.failedSeen='unvisited';globalThis.failedError=''; \
         try{new StaticRange({endContainer:document,endOffset:0, \
         get startContainer(){const n=document.createElement('b');n.note='live'; \
           globalThis.failedWeak=new WeakRef(n);return n}, \
         get startOffset(){failedSeen=failedWeak.deref()?.note??'missing'; \
           throw new RangeError('conversion stopped')}})} \
         catch(error){failedError=error.name}",
            ScriptKind::Classic,
        )
        .expect("catch the page getter's conversion error");
    let seen = evaluate(&mut engine, "[failedSeen,failedError].join('|')");
    let freed = collect(&document, &mut engine);
    let released = evaluate(&mut engine, "failedWeak.deref()===undefined");
    assert_eq!(seen, "live|RangeError");
    assert_eq!(freed, 1, "failed arguments must not remain rooted");
    assert_eq!(released, "true");
}

#[test]
fn converted_sequence_arguments_are_live_during_later_iterator_steps() {
    let (document, mut engine) = document_and_engine();
    let seen = evaluate(
        &mut engine,
        "(()=>{let seen='unvisited';const parts={[Symbol.iterator](){let index=0; \
         return{next(){if(index++===0){const blob=new Blob(['payload']); \
           globalThis.partWeak=new WeakRef(blob);return{value:blob,done:false}} \
         document.nodeType;seen=String(partWeak.deref()?.size??'missing'); \
         return{done:true}}}}};const result=new Blob(parts);return result.size+':'+seen})()",
    );
    collect(&document, &mut engine);
    let released = evaluate(&mut engine, "partWeak.deref()===undefined");
    assert_eq!(seen, "7:7");
    assert_eq!(
        released, "true",
        "sequence arguments must not remain rooted"
    );
}
