// Observable Event.target ownership, including edge replacement and collection.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use tenta_dom::{Document, SharedDocument};
use tenta_js::engine::PageEngine;

fn event_engine() -> (SharedDocument, Engine) {
    let document = Rc::new(RefCell::new(Document::new()));
    let engine = new_engine(document.clone());
    (document, engine)
}

fn event_eval(engine: &mut Engine, source: &str) -> String {
    PageEngine::evaluate(engine, source, Duration::from_secs(10)).expect("event ownership script")
}

fn event_collect(document: &SharedDocument, engine: &mut Engine) {
    for _ in 0..3 {
        engine.run_microtasks();
        engine.trim_memory();
        assert_eq!(engine.native_failure(), None);
        document.borrow_mut().sweep_detached();
    }
    engine.trim_memory();
    assert_eq!(engine.native_failure(), None);
}

#[test]
fn held_event_keeps_detached_target_wrapper_state() {
    let (document, mut engine) = event_engine();
    event_eval(
        &mut engine,
        "globalThis.savedEvent=(()=>{const node=document.createElement('div');\
         node.note='target';const proto=Object.create(Object.getPrototypeOf(node));\
         proto.fromProto=41;Object.setPrototypeOf(node,proto);\
         const event=new Event('probe');node.dispatchEvent(event);return event})();",
    );
    event_collect(&document, &mut engine);
    assert_eq!(
        document.borrow().len(),
        2,
        "a held event retains its target"
    );
    assert_eq!(
        event_eval(
            &mut engine,
            "[savedEvent.target.note,savedEvent.target.fromProto,\
             savedEvent.currentTarget===null,savedEvent.composedPath().length].join('|')"
        ),
        "target|41|true|0"
    );
}

#[test]
fn held_event_keeps_target_during_uninterrupted_allocation_pressure() {
    let (_document, mut engine) = event_engine();
    assert_eq!(
        event_eval(
            &mut engine,
            "globalThis.savedEvent=(()=>{const node=document.createElement('div');\
             node.note='target';const event=new Event('probe');\
             node.dispatchEvent(event);return event})();\
             for(let i=0;i<12000;i++)document.createElement('b');\
             [savedEvent.target.nodeName,savedEvent.target.note].join('|')"
        ),
        "DIV|target"
    );
    assert_eq!(engine.native_failure(), None);
}

#[test]
fn unreachable_event_target_expando_cycle_collects() {
    let (document, mut engine) = event_engine();
    event_eval(
        &mut engine,
        "(()=>{const node=document.createElement('div');const event=new Event('probe');\
         node.dispatchEvent(event);node.event=event;\
         globalThis.nodeRef=new WeakRef(node);globalThis.eventRef=new WeakRef(event)})();",
    );
    event_collect(&document, &mut engine);
    assert_eq!(document.borrow().len(), 1);
    assert_eq!(
        event_eval(
            &mut engine,
            "nodeRef.deref()===undefined&&eventRef.deref()===undefined"
        ),
        "true"
    );
}

#[test]
fn redispatch_replaces_target_ownership() {
    let (document, mut engine) = event_engine();
    event_eval(
        &mut engine,
        "globalThis.savedEvent=(()=>{const event=new Event('probe');\
         const first=document.createElement('div');first.dispatchEvent(event);\
         globalThis.oldRef=new WeakRef(first);\
         const second=document.createElement('span');second.note='new';\
         second.dispatchEvent(event);return event})();",
    );
    event_collect(&document, &mut engine);
    assert_eq!(document.borrow().len(), 2);
    assert_eq!(
        event_eval(
            &mut engine,
            "[oldRef.deref()===undefined,savedEvent.target.nodeName,savedEvent.target.note].join('|')"
        ),
        "true|SPAN|new"
    );
}

#[test]
fn init_event_releases_previous_target_ownership() {
    let (document, mut engine) = event_engine();
    event_eval(
        &mut engine,
        "globalThis.savedEvent=(()=>{const event=new Event('probe');\
         const node=document.createElement('div');node.dispatchEvent(event);\
         globalThis.oldRef=new WeakRef(node);return event})();\
         savedEvent.initEvent('reset',false,false);",
    );
    event_collect(&document, &mut engine);
    assert_eq!(document.borrow().len(), 1);
    assert_eq!(
        event_eval(
            &mut engine,
            "oldRef.deref()===undefined&&savedEvent.target===null"
        ),
        "true"
    );
}

#[test]
fn event_retains_target_from_another_document_during_pressure() {
    let (_document, mut engine) = event_engine();
    assert_eq!(
        event_eval(
            &mut engine,
            "globalThis.savedEvent=(()=>{\
             const other=document.implementation.createHTMLDocument('other');\
             const node=other.createElement('span');node.note=77;\
             const event=new Event('probe');node.dispatchEvent(event);return event})();\
             for(let i=0;i<12000;i++)document.createElement('b');\
             [savedEvent.target.nodeName,savedEvent.target.note,\
              savedEvent.target.ownerDocument===document].join('|')"
        ),
        "SPAN|77|false"
    );
    assert_eq!(engine.native_failure(), None);
}

#[test]
fn cross_document_dispatch_uses_target_listeners_and_the_original_event() {
    let (_document, mut engine) = event_engine();
    assert_eq!(
        event_eval(
            &mut engine,
            "const other=document.implementation.createHTMLDocument('other');\
             const parent=other.createElement('div'),node=other.createElement('span');\
             parent.appendChild(node);\
             const event=new Event('probe',{bubbles:true,cancelable:true});\
             const local=other.createEvent('Event');local.initEvent('untouched',false,false);\
             const calls=[];\
             parent.addEventListener('probe',function(e){\
                 calls.push('capture:'+String(e===event)+':'+String(this===parent)+':'+e.eventPhase);\
             },true);\
             node.addEventListener('probe',function(e){\
                 calls.push('target:'+String(e===event)+':'+String(this===node)+':'+e.eventPhase);\
                 e.preventDefault();e.stopImmediatePropagation();\
                 try{node.dispatchEvent(e)}catch(error){calls.push(error.name)}\
             });\
             node.addEventListener('probe',()=>calls.push('unexpected target'));\
             parent.addEventListener('probe',()=>calls.push('unexpected bubble'));\
             const accepted=node.dispatchEvent(event);\
             [accepted,event.defaultPrevented,event.currentTarget===null,event.composedPath().length,\
              local.type,local.target===null,calls.join(',')].join('|')"
        ),
        "false|true|true|0|untouched|true|capture:true:true:1,target:true:true:2,InvalidStateError"
    );
    assert_eq!(engine.native_failure(), None);
}

// Shared standalone EventTarget behavior and native ownership checks.

#[test]
fn standalone_targets_dispatch_independently_without_window_ancestors() {
    let (_document, mut engine) = event_engine();
    assert_eq!(
        event_eval(
            &mut engine,
            r#"
            const target=new EventTarget(), other=new EventTarget(), calls=[];
            window.addEventListener('standalone',()=>calls.push('window'));
            other.addEventListener('standalone',()=>calls.push('other'));
            function listener(event) {
                calls.push([this===target,event.target===target,
                    event.currentTarget===target,event.eventPhase,
                    event.composedPath().length].join(':'));
                event.preventDefault();
            }
            target.addEventListener('standalone', listener);
            target.addEventListener('standalone', listener);
            const event=new Event('standalone',{bubbles:true,cancelable:true});
            const first=target.dispatchEvent(event);
            target.removeEventListener('standalone',listener);
            const second=target.dispatchEvent(new Event('standalone'));
            [first,second,calls.join(','),event.currentTarget===null,
                event.composedPath().length].join('|')
        "#
        ),
        "false|true|true:true:true:2:1|true|0"
    );
}

#[test]
fn standalone_target_subclasses_keep_their_prototype_and_listener_brand() {
    let (_document, mut engine) = event_engine();
    assert_eq!(
        event_eval(
            &mut engine,
            r#"
        class Target extends EventTarget { answer() { return 42; } }
        const target=new Target();let calls=0;
        EventTarget.prototype.addEventListener.call(target,'x',()=>calls++);
        target.dispatchEvent(new Event('x'));
        let rejected=false;
        try { EventTarget.prototype.addEventListener.call(Object.create(Target.prototype),'x',()=>{}); }
        catch(error) { rejected=error instanceof TypeError; }
        [target instanceof Target,target instanceof EventTarget,target.answer(),calls,rejected].join('|')
    "#
        ),
        "true|true|42|1|true"
    );
}

#[test]
fn live_standalone_target_preserves_its_callback_across_collection() {
    let (document, mut engine) = event_engine();
    event_eval(
        &mut engine,
        r#"
        globalThis.target=new EventTarget();globalThis.calls=0;
        (()=>{const callback=()=>calls++;target.addEventListener('x',callback);
            globalThis.callbackRef=new WeakRef(callback)})();
    "#,
    );
    event_collect(&document, &mut engine);
    assert_eq!(
        event_eval(
            &mut engine,
            "target.dispatchEvent(new Event('x'));[calls,callbackRef.deref()!==undefined].join('|')"
        ),
        "1|true"
    );
}

#[test]
fn unreachable_standalone_target_callback_cycle_collects() {
    let (document, mut engine) = event_engine();
    event_eval(
        &mut engine,
        r#"
        (()=>{const target=new EventTarget();
            const callback=()=>target.note;
            target.addEventListener('x',callback);
            globalThis.targetRef=new WeakRef(target);globalThis.callbackRef=new WeakRef(callback);
        })();
    "#,
    );
    event_collect(&document, &mut engine);
    assert_eq!(
        event_eval(
            &mut engine,
            "targetRef.deref()===undefined&&callbackRef.deref()===undefined"
        ),
        "true"
    );
}

#[test]
fn standalone_callback_shared_with_a_rooted_target_stays_live_until_removed() {
    let (document, mut engine) = event_engine();
    event_eval(
        &mut engine,
        r#"
        globalThis.calls=0;
        (()=>{const target=new EventTarget();target.note=41;
            const callback=()=>{calls+=target.note};
            target.addEventListener('shared',callback);
            window.addEventListener('shared',callback);
            globalThis.targetRef=new WeakRef(target);globalThis.callbackRef=new WeakRef(callback);
        })();
    "#,
    );
    event_collect(&document, &mut engine);
    assert_eq!(
        event_eval(
            &mut engine,
            "window.dispatchEvent(new Event('shared'));[calls,targetRef.deref().note].join('|')"
        ),
        "41|41"
    );
    event_eval(
        &mut engine,
        "window.removeEventListener('shared',callbackRef.deref())",
    );
    event_collect(&document, &mut engine);
    assert_eq!(
        event_eval(
            &mut engine,
            "targetRef.deref()===undefined&&callbackRef.deref()===undefined"
        ),
        "true"
    );
}

#[test]
fn held_event_preserves_standalone_target_state_until_redispatch() {
    let (document, mut engine) = event_engine();
    event_eval(
        &mut engine,
        r#"
        globalThis.savedEvent=(()=>{const target=new EventTarget();
            target.note=41;const prototype=Object.create(EventTarget.prototype);
            prototype.marker=42;Object.setPrototypeOf(target,prototype);
            const event=new Event('x');target.dispatchEvent(event);
            globalThis.oldTargetRef=new WeakRef(target);return event;})();
    "#,
    );
    event_collect(&document, &mut engine);
    assert_eq!(
        event_eval(
            &mut engine,
            "[savedEvent.target===oldTargetRef.deref(),savedEvent.target.note,savedEvent.target.marker].join('|')"
        ),
        "true|41|42"
    );
    event_eval(&mut engine, "new EventTarget().dispatchEvent(savedEvent)");
    event_collect(&document, &mut engine);
    assert_eq!(
        event_eval(
            &mut engine,
            "oldTargetRef.deref()===undefined&&savedEvent.target instanceof EventTarget"
        ),
        "true"
    );
    event_eval(
        &mut engine,
        "globalThis.newTargetRef=new WeakRef(savedEvent.target);savedEvent.initEvent('reset',false,false)",
    );
    event_collect(&document, &mut engine);
    assert_eq!(
        event_eval(
            &mut engine,
            "newTargetRef.deref()===undefined&&savedEvent.target===null"
        ),
        "true"
    );
}

#[test]
fn unreachable_standalone_target_event_expando_cycle_collects() {
    let (document, mut engine) = event_engine();
    event_eval(
        &mut engine,
        r#"
        (()=>{const target=new EventTarget(),event=new Event('x');
            target.dispatchEvent(event);target.event=event;
            globalThis.targetRef=new WeakRef(target);globalThis.eventRef=new WeakRef(event);})();
    "#,
    );
    event_collect(&document, &mut engine);
    assert_eq!(
        event_eval(
            &mut engine,
            "targetRef.deref()===undefined&&eventRef.deref()===undefined"
        ),
        "true"
    );
}
