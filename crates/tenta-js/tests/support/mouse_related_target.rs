// Shared real-engine ownership checks for MouseEvent.relatedTarget.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use tenta_dom::{Document, SharedDocument};
use tenta_js::engine::PageEngine;

fn related_engine() -> (SharedDocument, Engine) {
    let document = Rc::new(RefCell::new(Document::new()));
    let engine = new_engine(document.clone());
    (document, engine)
}

fn related_eval(engine: &mut Engine, source: &str) -> String {
    PageEngine::evaluate(engine, source, Duration::from_secs(10))
        .expect("relatedTarget ownership script")
}

fn related_collect(document: &SharedDocument, engine: &mut Engine) {
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
fn held_mouse_event_preserves_detached_related_wrapper_state_then_releases_it() {
    let (document, mut engine) = related_engine();
    related_eval(
        &mut engine,
        r#"globalThis.heldMouse=(()=>{const target=document.createElement('div');
            target.note='held';const proto=Object.create(Object.getPrototypeOf(target));
            proto.inherited=41;Object.setPrototypeOf(target,proto);
            globalThis.relatedRef=new WeakRef(target);
            return new MouseEvent('mouseout',{relatedTarget:target})})()"#,
    );
    related_collect(&document, &mut engine);
    assert_eq!(document.borrow().len(), 2);
    assert_eq!(
        related_eval(
            &mut engine,
            "[heldMouse.relatedTarget===relatedRef.deref(),heldMouse.relatedTarget.note,heldMouse.relatedTarget.inherited].join('|')"
        ),
        "true|held|41"
    );
    related_eval(&mut engine, "globalThis.heldMouse=null");
    related_collect(&document, &mut engine);
    assert_eq!(document.borrow().len(), 1);
    assert_eq!(
        related_eval(&mut engine, "relatedRef.deref()===undefined"),
        "true"
    );
}

#[test]
fn dispatch_target_and_related_target_are_independent_outgoing_edges() {
    let (document, mut engine) = related_engine();
    related_eval(
        &mut engine,
        r#"globalThis.heldMouse=(()=>{const first=document.createElement('div');
            first.note='first';globalThis.firstRef=new WeakRef(first);
            const event=new MouseEvent('move',{relatedTarget:first});
            first.dispatchEvent(event);return event})()"#,
    );
    related_collect(&document, &mut engine);
    assert_eq!(
        related_eval(
            &mut engine,
            "[heldMouse.target===heldMouse.relatedTarget,heldMouse.target.note].join('|')"
        ),
        "true|first"
    );
    related_eval(
        &mut engine,
        r#"(()=>{const second=document.createElement('span');second.note='second';
            globalThis.secondRef=new WeakRef(second);second.dispatchEvent(heldMouse)})()"#,
    );
    related_collect(&document, &mut engine);
    assert_eq!(document.borrow().len(), 3);
    assert_eq!(
        related_eval(
            &mut engine,
            "[heldMouse.target.note,heldMouse.relatedTarget.note,heldMouse.target===heldMouse.relatedTarget].join('|')"
        ),
        "second|first|false"
    );
    related_eval(
        &mut engine,
        "heldMouse.initMouseEvent('reset',false,false,null,0,0,0,0,0,false,false,false,false,0,null)",
    );
    related_collect(&document, &mut engine);
    assert_eq!(document.borrow().len(), 1);
    assert_eq!(
        related_eval(
            &mut engine,
            "[heldMouse.target===null,heldMouse.relatedTarget===null,firstRef.deref()===undefined,secondRef.deref()===undefined].join('|')"
        ),
        "true|true|true|true"
    );
}

#[test]
fn legacy_initializer_replaces_owned_related_target_and_ignores_dispatch_reentry() {
    let (document, mut engine) = related_engine();
    related_eval(
        &mut engine,
        r#"globalThis.heldMouse=(()=>{const first=document.createElement('div');
            first.note='first';globalThis.firstRef=new WeakRef(first);
            return new MouseEvent('move',{relatedTarget:first})})()"#,
    );
    related_eval(
        &mut engine,
        r#"(()=>{const second=document.createElement('span');
            second.note='second';globalThis.secondRef=new WeakRef(second);
            heldMouse.initMouseEvent('move',false,false,null,0,0,0,0,0,
                false,false,false,false,0,second);
            second.addEventListener('move',()=>{
                heldMouse.initMouseEvent('ignored',false,false,null,0,0,0,0,0,
                    false,false,false,false,0,null)
            });
            second.dispatchEvent(heldMouse)})()"#,
    );
    related_collect(&document, &mut engine);
    assert_eq!(document.borrow().len(), 2);
    assert_eq!(
        related_eval(
            &mut engine,
            "[firstRef.deref()===undefined,heldMouse.type,heldMouse.target===heldMouse.relatedTarget,heldMouse.relatedTarget.note].join('|')"
        ),
        "true|move|true|second"
    );
    related_eval(
        &mut engine,
        "heldMouse.initMouseEvent('reset',false,false,null,0,0,0,0,0,false,false,false,false,0,null)",
    );
    related_collect(&document, &mut engine);
    assert_eq!(document.borrow().len(), 1);
    assert_eq!(
        related_eval(
            &mut engine,
            "[heldMouse.target===null,heldMouse.relatedTarget===null,secondRef.deref()===undefined].join('|')"
        ),
        "true|true|true"
    );
}

#[test]
fn unreachable_related_target_event_callback_cycle_collects() {
    let (document, mut engine) = related_engine();
    related_eval(
        &mut engine,
        r#"(()=>{const target=document.createElement('div');
            const event=new MouseEvent('mouseout',{relatedTarget:target});
            const callback=()=>event.relatedTarget;
            target.addEventListener('probe',callback);
            globalThis.relatedRef=new WeakRef(target);
            globalThis.mouseRef=new WeakRef(event);
            globalThis.callbackRef=new WeakRef(callback)})()"#,
    );
    related_collect(&document, &mut engine);
    assert_eq!(document.borrow().len(), 1);
    assert_eq!(
        related_eval(
            &mut engine,
            "[relatedRef.deref(),mouseRef.deref(),callbackRef.deref()].every(value=>value===undefined)"
        ),
        "true"
    );
}

#[test]
fn each_event_owned_reference_keeps_a_shared_target_until_its_own_release() {
    let (document, mut engine) = related_engine();
    related_eval(
        &mut engine,
        r#"(()=>{const target=document.createElement('div');target.note='shared';
            globalThis.relatedRef=new WeakRef(target);
            globalThis.firstMouse=new MouseEvent('x',{relatedTarget:target});
            globalThis.secondMouse=new MouseEvent('x',{relatedTarget:target});
            globalThis.detailEvent=new CustomEvent('x',{detail:target})})()"#,
    );
    related_collect(&document, &mut engine);
    assert_eq!(
        related_eval(
            &mut engine,
            "[firstMouse.relatedTarget===secondMouse.relatedTarget,detailEvent.detail===firstMouse.relatedTarget,detailEvent.detail.note].join('|')"
        ),
        "true|true|shared"
    );
    related_eval(
        &mut engine,
        "globalThis.firstMouse=null;globalThis.secondMouse=null",
    );
    related_collect(&document, &mut engine);
    assert_eq!(document.borrow().len(), 2);
    assert_eq!(
        related_eval(&mut engine, "detailEvent.detail.note"),
        "shared"
    );
    related_eval(&mut engine, "globalThis.detailEvent=null");
    related_collect(&document, &mut engine);
    assert_eq!(document.borrow().len(), 1);
    assert_eq!(
        related_eval(&mut engine, "relatedRef.deref()===undefined"),
        "true"
    );
}

#[test]
fn converted_related_target_survives_later_dictionary_getter_reentry() {
    let (document, mut engine) = related_engine();
    assert_eq!(
        related_eval(
            &mut engine,
            r#"globalThis.reads=[];
            globalThis.heldMouse=new MouseEvent('x',{
                get relatedTarget(){const node=document.createElement('div');
                    node.note='converted';globalThis.relatedRef=new WeakRef(node);
                    reads.push('related');return node},
                get screenX(){reads.push('later');
                    for(let i=0;i<1500;i++)document.createElement('i');
                    return 7}
            });
            [reads.join(','),heldMouse.relatedTarget===relatedRef.deref(),heldMouse.relatedTarget.note,heldMouse.screenX].join('|')"#,
        ),
        "related,later|true|converted|7"
    );
    related_collect(&document, &mut engine);
    assert_eq!(
        related_eval(&mut engine, "heldMouse.relatedTarget.note"),
        "converted"
    );
}

#[test]
fn primitive_custom_detail_remains_readable_while_object_edges_collect() {
    let (document, mut engine) = related_engine();
    related_eval(
        &mut engine,
        "globalThis.detailEvent=new CustomEvent('x',{detail:7})",
    );
    related_collect(&document, &mut engine);
    assert_eq!(related_eval(&mut engine, "detailEvent.detail"), "7");
    assert_eq!(engine.native_failure(), None);
}

#[test]
fn window_and_standalone_event_target_keep_exact_related_identity() {
    let (document, mut engine) = related_engine();
    assert_eq!(
        related_eval(
            &mut engine,
            "new MouseEvent('x',{relatedTarget:window}).relatedTarget===globalThis"
        ),
        "true"
    );
    related_eval(
        &mut engine,
        r#"globalThis.heldMouse=(()=>{const target=new EventTarget();
            target.note=37;globalThis.relatedRef=new WeakRef(target);
            return new MouseEvent('x',{relatedTarget:target})})()"#,
    );
    related_collect(&document, &mut engine);
    assert_eq!(
        related_eval(
            &mut engine,
            "[heldMouse.relatedTarget===relatedRef.deref(),heldMouse.relatedTarget.note].join('|')"
        ),
        "true|37"
    );
    related_eval(&mut engine, "globalThis.heldMouse=null");
    related_collect(&document, &mut engine);
    assert_eq!(
        related_eval(&mut engine, "relatedRef.deref()===undefined"),
        "true"
    );
}

#[test]
fn custom_detail_object_event_cycle_preserves_held_state_then_collects() {
    let (document, mut engine) = related_engine();
    related_eval(
        &mut engine,
        r#"globalThis.heldCustom=(()=>{const detail={note:'alive'};
            const event=new CustomEvent('cycle',{detail});detail.event=event;
            globalThis.customRef=new WeakRef(event);
            globalThis.detailRef=new WeakRef(detail);return event})()"#,
    );
    related_collect(&document, &mut engine);
    assert_eq!(
        related_eval(
            &mut engine,
            "[heldCustom===customRef.deref(),heldCustom.detail===detailRef.deref(),heldCustom.detail.event===heldCustom,heldCustom.detail.note].join('|')"
        ),
        "true|true|true|alive"
    );
    related_eval(&mut engine, "globalThis.heldCustom=null");
    related_collect(&document, &mut engine);
    assert_eq!(
        related_eval(
            &mut engine,
            "[customRef.deref(),detailRef.deref()].every(value=>value===undefined)"
        ),
        "true"
    );
}

#[test]
fn held_focus_event_preserves_detached_related_wrapper_then_releases_it() {
    let (document, mut engine) = related_engine();
    related_eval(
        &mut engine,
        r#"globalThis.heldFocus=(()=>{const target=document.createElement('div');
            target.note='focus';globalThis.focusRelatedRef=new WeakRef(target);
            return new FocusEvent('focus',{relatedTarget:target})})()"#,
    );
    related_collect(&document, &mut engine);
    assert_eq!(document.borrow().len(), 2);
    assert_eq!(
        related_eval(
            &mut engine,
            "[heldFocus.relatedTarget===focusRelatedRef.deref(),heldFocus.relatedTarget.note].join('|')"
        ),
        "true|focus"
    );
    related_eval(&mut engine, "globalThis.heldFocus=null");
    related_collect(&document, &mut engine);
    assert_eq!(document.borrow().len(), 1);
    assert_eq!(
        related_eval(&mut engine, "focusRelatedRef.deref()===undefined"),
        "true"
    );
}

#[test]
fn wheel_dictionary_error_after_related_target_does_not_create_an_event_edge() {
    let (document, mut engine) = related_engine();
    assert_eq!(
        related_eval(
            &mut engine,
            r#"(()=>{const target=document.createElement('div');
                globalThis.failedWheelRef=new WeakRef(target);
                const marker=new Error('delta getter');
                try{new WheelEvent('wheel',{relatedTarget:target,
                    get deltaX(){throw marker}});return 'accepted'}
                catch(e){return e===marker}})()"#
        ),
        "true"
    );
    related_collect(&document, &mut engine);
    assert_eq!(document.borrow().len(), 1);
    assert_eq!(
        related_eval(&mut engine, "failedWheelRef.deref()===undefined"),
        "true"
    );
}
