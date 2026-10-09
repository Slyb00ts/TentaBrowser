// Shared behavioral controls for the DOM default passive value.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use tenta_dom::Document;
use tenta_js::engine::PageEngine;

fn passive_eval(engine: &mut Engine, source: &str) -> String {
    PageEngine::evaluate(engine, source, Duration::from_secs(10))
        .expect("default passive event script")
}

fn passive_engine() -> Engine {
    new_engine(Rc::new(RefCell::new(Document::new())))
}

#[test]
fn default_passive_applies_to_the_four_types_on_window_document_root_and_body() {
    let mut engine = passive_engine();
    assert_eq!(
        passive_eval(
            &mut engine,
            r#"const root=document.appendChild(document.createElement('html'));
               const body=root.appendChild(document.createElement('body'));
               const leaf=body.appendChild(document.createElement('div'));
               function outcome(target,type,options) {
                   let called=0;
                   const listener=event=>{called++;event.preventDefault()};
                   target.addEventListener(type,listener,options);
                   const event=new Event(type,{cancelable:true});
                   const dispatched=target.dispatchEvent(event);
                   target.removeEventListener(type,listener,options);
                   return [called,dispatched,event.defaultPrevented].join(':');
               }
               const types=['touchstart','touchmove','wheel','mousewheel'];
               const eligible=[window,document,root,body];
               const passive=eligible.every(target=>types.every(type=>
                   outcome(target,type)==='1:true:false'));
               const ordinary=[leaf,new AbortController().signal].every(target=>
                   types.every(type=>outcome(target,type)==='1:false:true'));
               const unrelated=eligible.every(target=>
                   outcome(target,'click')==='1:false:true');
               [passive,ordinary,unrelated].join('|')"#
        ),
        "true|true|true"
    );
    assert_eq!(engine.native_failure(), None);
}

#[test]
fn omitted_undefined_boolean_and_explicit_passive_options_remain_distinct() {
    let mut engine = passive_engine();
    assert_eq!(
        passive_eval(
            &mut engine,
            r#"const root=document.appendChild(document.createElement('html'));
               const body=root.appendChild(document.createElement('body'));
               function outcome(type,options) {
                   const listener=event=>event.preventDefault();
                   body.addEventListener(type,listener,options);
                   const event=new Event(type,{cancelable:true});
                   const dispatched=body.dispatchEvent(event);
                   body.removeEventListener(type,listener,options);
                   return [dispatched,event.defaultPrevented].join(':');
               }
               [outcome('wheel'),outcome('wheel',{}),
                outcome('wheel',{passive:undefined}),outcome('wheel',true),
                outcome('wheel',{passive:true}),outcome('wheel',{passive:false}),
                outcome('wheel',{passive:null}),outcome('Wheel')].join('|')"#
        ),
        "true:false|true:false|true:false|true:false|true:false|false:true|false:true|false:true"
    );
    assert_eq!(engine.native_failure(), None);
}

#[test]
fn passive_default_is_fixed_when_listener_is_registered() {
    let mut engine = passive_engine();
    assert_eq!(
        passive_eval(
            &mut engine,
            r#"const root=document.appendChild(document.createElement('html'));
               const oldBody=root.appendChild(document.createElement('body'));
               const newBody=document.createElement('body');
               oldBody.addEventListener('wheel',event=>event.preventDefault());
               newBody.addEventListener('wheel',event=>event.preventDefault());
               oldBody.remove();root.appendChild(newBody);
               function canceled(target) {
                   const event=new Event('wheel',{cancelable:true});
                   target.dispatchEvent(event);
                   return event.defaultPrevented;
               }
               [document.body===newBody,canceled(oldBody),canceled(newBody)].join('|')"#
        ),
        "true|false|true"
    );
    assert_eq!(engine.native_failure(), None);
}

#[test]
fn body_eligibility_follows_the_html_root_and_namespace() {
    let mut engine = passive_engine();
    assert_eq!(
        passive_eval(
            &mut engine,
            r#"const xml=new Document();
               const root=xml.createElementNS('http://www.w3.org/2000/svg','svg');
               xml.appendChild(root);
               const bodyLike=xml.createElementNS('http://www.w3.org/1999/xhtml','body');
               root.appendChild(bodyLike);
               const xhtml=new Document();
               const html=xhtml.createElementNS('http://www.w3.org/1999/xhtml','html');
               xhtml.appendChild(html);
               const body=xhtml.createElementNS('http://www.w3.org/1999/xhtml','body');
               html.appendChild(body);
               function canceled(target) {
                   const listener=event=>event.preventDefault();
                   target.addEventListener('wheel',listener);
                   const event=new Event('wheel',{cancelable:true});
                   target.dispatchEvent(event);
                   target.removeEventListener('wheel',listener);
                   return event.defaultPrevented;
               }
               [canceled(xml),canceled(root),canceled(bodyLike),
                canceled(xhtml),canceled(html),canceled(body)].join('|')"#
        ),
        "false|false|true|false|false|false"
    );
    assert_eq!(engine.native_failure(), None);
}

#[test]
fn html_frameset_body_uses_the_same_default_as_html_body() {
    let mut engine = passive_engine();
    assert_eq!(
        passive_eval(
            &mut engine,
            r#"const root=document.appendChild(document.createElement('html'));
               const frameset=root.appendChild(document.createElement('frameset'));
               const listener=event=>event.preventDefault();
               frameset.addEventListener('mousewheel',listener);
               const event=new Event('mousewheel',{cancelable:true});
               const dispatched=frameset.dispatchEvent(event);
               [document.body===frameset,dispatched,event.defaultPrevented].join('|')"#
        ),
        "true|true|false"
    );
    assert_eq!(engine.native_failure(), None);
}

#[test]
fn non_html_body_sibling_does_not_mask_the_actual_html_body() {
    let mut engine = passive_engine();
    assert_eq!(
        passive_eval(
            &mut engine,
            r#"const root=document.appendChild(document.createElement('html'));
               const svgBody=root.appendChild(document.createElementNS(
                   'http://www.w3.org/2000/svg','body'));
               const htmlBody=root.appendChild(document.createElement('body'));
               function canceled(target) {
                   const listener=event=>event.preventDefault();
                   target.addEventListener('wheel',listener);
                   const event=new Event('wheel',{cancelable:true});
                   target.dispatchEvent(event);
                   target.removeEventListener('wheel',listener);
                   return event.defaultPrevented;
               }
               [document.body===htmlBody,canceled(svgBody),canceled(htmlBody)].join('|')"#
        ),
        "true|true|false"
    );
    assert_eq!(engine.native_failure(), None);
}

// Integrate only after the standalone EventTarget constructor is available.

#[test]
fn standalone_event_target_does_not_inherit_root_default_passive_behavior() {
    let mut engine = passive_engine();
    assert_eq!(
        passive_eval(
            &mut engine,
            r#"const target=new EventTarget();
               const listener=event=>event.preventDefault();
               target.addEventListener('wheel',listener);
               const event=new Event('wheel',{cancelable:true});
               const dispatched=target.dispatchEvent(event);
               [dispatched,event.defaultPrevented].join('|')"#
        ),
        "false|true"
    );
    assert_eq!(engine.native_failure(), None);
}
