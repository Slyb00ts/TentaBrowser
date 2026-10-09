// Shared UIEvent/MouseEvent constructor and legacy-initializer behavior.
//
// UI Events: https://w3c.github.io/uievents/#interface-uievent
// Pinned WPT: dom/events/Event-subclasses-constructors.html and
// dom/events/Event-init-while-dispatching.html.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use tenta_dom::Document;
use tenta_js::engine::PageEngine;

fn event_eval(source: &str) -> String {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = new_engine(document);
    let result =
        PageEngine::evaluate(&mut engine, source, Duration::from_secs(10)).expect("UIEvent script");
    assert_eq!(engine.native_failure(), None);
    result
}

#[test]
fn constructors_expose_nullable_window_view_on_ui_and_mouse_events() {
    assert_eq!(
        event_eval(
            r#"const uiDefault=new UIEvent('resize');
               const ui=new UIEvent('resize',{view:window,detail:4});
               const mouse=new MouseEvent('click',{view:window,detail:2,
                   clientX:13,buttons:3});
               [uiDefault.view===null,uiDefault.detail,
                ui.view===window,ui.detail,
                mouse.view===window,mouse.detail,mouse.clientX,mouse.buttons,
                mouse instanceof UIEvent,mouse instanceof MouseEvent,
                Object.getPrototypeOf(mouse)===MouseEvent.prototype].join('|')"#
        ),
        "true|0|true|4|true|2|13|3|true|true|true"
    );
}

#[test]
fn synthetic_click_uses_the_target_documents_window_when_it_has_one() {
    assert_eq!(
        event_eval(
            r#"const pageButton=document.createElement('button');
               let pageView='missing';
               pageButton.addEventListener('click',event=>{
                   pageView=[event.view===window,
                       event.view===document.defaultView,event.detail].join(':');
               });
               pageButton.click();
               const other=document.implementation.createHTMLDocument('other');
               const otherButton=other.body.appendChild(other.createElement('button'));
               let otherView='missing';
               otherButton.addEventListener('click',event=>{
                   otherView=[event.view===null,other.defaultView===null,
                       event.detail].join(':');
               });
               otherButton.click();
               [pageView,otherView].join('|')"#
        ),
        "true:true:0|true:true:0"
    );
}

#[test]
fn view_accepts_only_the_actual_page_window_and_failed_conversion_does_not_reinit() {
    assert_eq!(
        event_eval(
            r#"function construct(view) {
                   try { new UIEvent('probe',{view}); return 'accepted'; }
                   catch (error) { return error.name; }
               }
               const event=new UIEvent('old',{view:window,detail:7});
               let initError='none';
               try { event.initUIEvent('new',false,false,Object.create(window),2); }
               catch (error) { initError=error.name; }
               [construct({}),construct(document),construct(Object.create(window)),
                construct(7),construct(null),construct(window),
                initError,event.type,event.view===window,event.detail].join('|')"#
        ),
        "TypeError|TypeError|TypeError|TypeError|accepted|accepted|TypeError|old|true|7"
    );
}

#[test]
fn init_ui_event_resets_base_state_without_replacing_the_event_wrapper() {
    assert_eq!(
        event_eval(
            r#"const event=document.createEvent('UIEvents');
               const same=event;
               event.initUIEvent('before',true,true,window,5);
               const before=[event.type,event.bubbles,event.cancelable,
                   event.view===window,event.detail].join(':');
               document.addEventListener('before',e=>e.preventDefault(),{once:true});
               document.dispatchEvent(event);
               const canceled=event.defaultPrevented&&event.target===document;
               event.initUIEvent('after');
               [before,canceled,event===same,event instanceof UIEvent,
                event.type,event.bubbles,event.cancelable,event.view===null,
                event.detail,event.target===null,event.defaultPrevented].join('|')"#
        ),
        "before:true:true:true:5|true|true|true|after|false|false|true|0|true|false"
    );
}

#[test]
fn init_mouse_event_sets_all_legacy_fields_then_restores_defaults() {
    assert_eq!(
        event_eval(
            r#"const related=document.createElement('div');
               const event=document.createEvent('MouseEvents');
               event.initMouseEvent('move',true,true,window,3,4,5,6,7,
                   true,false,true,false,2,related);
               const before=[event.type,event.bubbles,event.cancelable,
                   event.view===window,event.detail,event.screenX,event.screenY,
                   event.clientX,event.clientY,event.ctrlKey,event.altKey,
                   event.shiftKey,event.metaKey,event.button,event.buttons,
                   event.relatedTarget===related].join(':');
               event.initMouseEvent('again');
               [before,event instanceof MouseEvent,event.type,event.bubbles,
                event.cancelable,event.view===null,event.detail,event.screenX,
                event.screenY,event.clientX,event.clientY,event.ctrlKey,
                event.altKey,event.shiftKey,event.metaKey,event.button,
                event.buttons,event.relatedTarget===null].join('|')"#
        ),
        "move:true:true:true:3:4:5:6:7:true:false:true:false:2:0:true|true|again|false|false|true|0|0|0|0|0|false|false|false|false|0|0|true"
    );
}

#[test]
fn inherited_ui_initializer_changes_only_ui_fields_of_a_mouse_event() {
    assert_eq!(
        event_eval(
            r#"const event=new MouseEvent('old',{clientX:13,buttons:3});
               event.initUIEvent('new',true,false,window,8);
               [event instanceof MouseEvent,event.type,event.bubbles,
                event.view===window,event.detail,event.clientX,event.buttons].join('|')"#
        ),
        "true|new|true|true|8|13|3"
    );
}

#[test]
fn both_legacy_initializers_are_no_ops_while_dispatching() {
    assert_eq!(
        event_eval(
            r#"const event=new MouseEvent('probe',{
                   view:window,detail:5,clientX:13,buttons:3,cancelable:true});
               let during='missing';
               document.addEventListener('probe',e=>{
                   e.initUIEvent('changed',false,false,null,1);
                   e.initMouseEvent('changed-again');
                   during=[e.type,e.view===window,e.detail,e.clientX,
                       e.buttons,e.cancelable,e.target===document,
                       e.eventPhase===Event.AT_TARGET].join(':');
               },{once:true});
               document.dispatchEvent(event);
               [during,event.type,event.view===window,event.detail,
                event.clientX,event.buttons,event.target===document,
                event.eventPhase===Event.NONE].join('|')"#
        ),
        "probe:true:5:13:3:true:true:true|probe|true|5|13|3|true|true"
    );
}

#[test]
fn focus_and_wheel_constructors_match_wpt_inherited_dictionary_fields() {
    assert_eq!(
        event_eval(
            r#"const focus=new FocusEvent('focus',{bubbles:true,view:window,
                   detail:7,relatedTarget:document});
               const wheel=new WheelEvent('wheel',{bubbles:true,view:window,
                   detail:7,screenX:40,screenY:40,clientX:40,clientY:40,
                   ctrlKey:true,shiftKey:true,altKey:true,metaKey:true,
                   button:40,buttons:40,relatedTarget:document,
                   deltaX:3.1,deltaY:3.1,deltaZ:3.1,deltaMode:40});
               [new FocusEvent('focus').relatedTarget===null,
                focus instanceof UIEvent,focus.view===window,focus.detail,
                focus.relatedTarget===document,focus.bubbles,
                new WheelEvent('wheel').deltaMode,
                wheel instanceof MouseEvent,wheel instanceof UIEvent,
                wheel.view===window,wheel.detail,wheel.screenX,wheel.screenY,
                wheel.clientX,wheel.clientY,wheel.ctrlKey,wheel.shiftKey,
                wheel.altKey,wheel.metaKey,wheel.button,wheel.buttons,
                wheel.relatedTarget===document,wheel.deltaX,wheel.deltaY,
                wheel.deltaZ,wheel.deltaMode].join('|')"#
        ),
        "true|true|true|7|true|true|0|true|true|true|7|40|40|40|40|true|true|true|true|40|40|true|3.1|3.1|3.1|40"
    );
}

#[test]
fn mouse_modifier_state_uses_event_modifier_init_and_legacy_initializer_resets_it() {
    assert_eq!(
        event_eval(
            r#"const names=['Control','Shift','Alt','Meta','AltGraph','CapsLock'];
               const members=['ctrlKey','shiftKey','altKey','metaKey',
                   'modifierAltGraph','modifierCapsLock'];
               const isolated=members.every((member,index)=>{
                   const event=new MouseEvent('move',{[member]:true});
                   return names.every((name,i)=>event.getModifierState(name)===(i===index));
               });
               const all=Object.fromEntries(members.map(member=>[member,true]));
               const event=new MouseEvent('move',all);
               const complete=names.every(name=>event.getModifierState(name));
               event.initMouseEvent('reset');
               [isolated,complete,names.every(name=>!event.getModifierState(name)),
                !event.getModifierState('Unknown'),
                !new MouseEvent('move').getModifierState('Control')].join('|')"#
        ),
        "true|true|true|true|true"
    );
}

#[test]
fn create_event_exposes_only_mapped_supported_interfaces_uninitialized() {
    assert_eq!(
        event_eval(
            r#"const aliases=['FocusEvent','FOCUSEVENT','SVGEvents','svgevents'];
               const valid=aliases.map(name=>{
                   const event=document.createEvent(name);
                   let error='none';
                   try{document.dispatchEvent(event)}catch(e){error=e.name}
                   return [event.type,event instanceof (name.toLowerCase()==='focusevent'?FocusEvent:Event),error].join(':');
               });
               const unsupported=['BeforeUnloadEvent','DeviceMotionEvent',
                   'DeviceOrientationEvent','DragEvent','HashChangeEvent',
                   'MessageEvent','StorageEvent','TextEvent'];
               const rejected=unsupported.every(name=>{
                   try{document.createEvent(name);return false}
                   catch(e){return e.name==='NotSupportedError'}
               });
               [valid.join(','),rejected].join('|')"#
        ),
        ":true:InvalidStateError,:true:InvalidStateError,:true:InvalidStateError,:true:InvalidStateError|true"
    );
}

#[test]
fn wheel_deltas_use_webidl_double_conversion() {
    assert_eq!(
        event_eval(
            r#"const wheel=new WheelEvent('wheel',{
                   deltaX:-0,deltaY:-3.25,deltaZ:1.5,deltaMode:2});
               const coerced=new WheelEvent('wheel',{
                   deltaX:'2.5',deltaMode:'1'});
               const invalid=[NaN,Infinity,-Infinity].map(deltaX=>{
                   try{new WheelEvent('wheel',{deltaX});return 'accepted'}
                   catch(e){return e.name}
               });
               [Object.is(wheel.deltaX,-0),wheel.deltaY,wheel.deltaZ,
                wheel.deltaMode,WheelEvent.DOM_DELTA_PIXEL,
                WheelEvent.DOM_DELTA_LINE,WheelEvent.DOM_DELTA_PAGE,
                coerced.deltaX,coerced.deltaMode,invalid.join(',')].join('|')"#
        ),
        "true|-3.25|1.5|2|0|1|2|2.5|1|TypeError,TypeError,TypeError"
    );
}
