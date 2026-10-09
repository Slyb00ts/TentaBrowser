// CompositionEvent constructor, inheritance, and legacy initializer behavior.
// UI Events: https://w3c.github.io/uievents/#interface-compositionevent
// Pinned WPT: dom/events/Event-subclasses-constructors.html and
// uievents/legacy/Event-subclasses-init.html.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use tenta_dom::Document;
use tenta_js::engine::PageEngine;

fn composition_eval(source: &str) -> String {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = new_engine(document);
    let result = PageEngine::evaluate(&mut engine, source, Duration::from_secs(10))
        .expect("CompositionEvent script");
    assert_eq!(engine.native_failure(), None);
    result
}

#[test]
fn constructor_defaults_and_inherited_properties_match_ui_events() {
    assert_eq!(
        composition_eval(
            r#"const values=[undefined,null,{}].map(init=>new CompositionEvent('x',init));
               values.unshift(new CompositionEvent('x'));
               [CompositionEvent.length,
                values.every(e=>e.data===''&&e.view===null&&e.detail===0),
                values.every(e=>e instanceof CompositionEvent&&e instanceof UIEvent&&e instanceof Event),
                values.every(e=>e.type==='x'&&!e.bubbles&&!e.cancelable&&!e.composed&&!e.isTrusted)]
                   .join('|')"#
        ),
        "1|true|true|true"
    );
}

#[test]
fn constructor_converts_data_and_all_inherited_dictionary_members() {
    assert_eq!(
        composition_eval(
            r#"const event=new CompositionEvent('start',{
                   data:'Ω',view:window,detail:7,bubbles:true,cancelable:true,composed:true
               });
               const nil=new CompositionEvent('x',{data:null});
               const missing=new CompositionEvent('x',{data:undefined});
               const surrogate=new CompositionEvent('x',{data:'\uD800'});
               const pair=new CompositionEvent('x',{data:'\uD83D\uDE00'});
               [event.data,event.view===window,event.detail,event.bubbles,
                event.cancelable,event.composed,nil.data,missing.data,
                surrogate.data==='\uFFFD',pair.data==='\uD83D\uDE00'].join('|')"#
        ),
        "Ω|true|7|true|true|true|null||true|true"
    );
}

#[test]
fn data_to_string_throw_preserves_the_page_exception() {
    assert_eq!(
        composition_eval(
            r#"const marker=new RangeError('data conversion');
               let received=false;
               try {
                   new CompositionEvent('x',{data:{toString(){throw marker}}});
               } catch (error) { received=error===marker; }
               [received,new CompositionEvent('ok').data].join('|')"#
        ),
        "true|"
    );
}

#[test]
fn dictionary_getter_error_precedes_event_publication() {
    assert_eq!(
        composition_eval(
            r#"const marker=new RangeError('data failed');
               let saw='none';
               try { new CompositionEvent('x',{get data(){throw marker}}); }
               catch(error) { saw=error===marker?'same':error.name; }
               [saw,new CompositionEvent('ok').data].join('|')"#
        ),
        "same|"
    );
}

#[test]
fn legacy_initializer_resets_base_and_composition_state() {
    assert_eq!(
        composition_eval(
            r#"const event=document.createEvent('CompositionEvent');
               const same=event;
               event.initCompositionEvent('before',true,true,window,'first');
               const before=[event.type,event.bubbles,event.cancelable,
                   event.view===window,event.detail,event.data].join(':');
               document.addEventListener('before',e=>e.preventDefault(),{once:true});
               document.dispatchEvent(event);
               const canceled=event.defaultPrevented&&event.target===document;
               event.initCompositionEvent('after');
               [before,canceled,event===same,event instanceof CompositionEvent,
                event.type,event.bubbles,event.cancelable,event.view===null,
                event.detail,event.data,event.target===null,event.defaultPrevented].join('|')"#
        ),
        "before:true:true:true:0:first|true|true|true|after|false|false|true|0||true|false"
    );
}

#[test]
fn inherited_ui_initializer_preserves_composition_data() {
    assert_eq!(
        composition_eval(
            r#"const event=new CompositionEvent('old',{data:'kept',detail:4});
               event.initUIEvent('new',true,false,window,8);
               [event.type,event.bubbles,event.view===window,event.detail,event.data].join('|')"#
        ),
        "new|true|true|8|kept"
    );
}

#[test]
fn legacy_initializer_rejects_missing_type_and_is_ignored_while_dispatching() {
    assert_eq!(
        composition_eval(
            r#"const event=new CompositionEvent('probe',{
                   data:'original',view:window,detail:5,cancelable:true});
               let missing='none';
               try { event.initCompositionEvent(); }
               catch(error) { missing=error.name; }
               let during='missing';
               document.addEventListener('probe',e=>{
                   e.initCompositionEvent('changed',false,false,null,'changed');
                   during=[e.type,e.data,e.view===window,e.detail,e.cancelable].join(':');
               },{once:true});
               document.dispatchEvent(event);
               [missing,during,event.type,event.data,event.view===window,
                event.detail,event.target===document].join('|')"#
        ),
        "TypeError|probe:original:true:5:true|probe|original|true|5|true"
    );
}
