// KeyboardEvent constructor, modifier, and legacy initializer behavior.
// UI Events: https://w3c.github.io/uievents/#interface-keyboardevent
// Pinned WPT: dom/events/Event-subclasses-constructors.html.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use tenta_dom::Document;
use tenta_js::engine::PageEngine;

fn keyboard_eval(source: &str) -> String {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = new_engine(document);
    let result = PageEngine::evaluate(&mut engine, source, Duration::from_secs(10))
        .expect("KeyboardEvent script");
    assert_eq!(engine.native_failure(), None);
    result
}

#[test]
fn constructor_defaults_and_location_constants() {
    assert_eq!(
        keyboard_eval(
            r#"const e=new KeyboardEvent('keydown');
               [KeyboardEvent.length,e instanceof KeyboardEvent,e instanceof UIEvent,
                e instanceof Event,e.type,e.key,e.code,e.location,e.repeat,e.isComposing,
                e.ctrlKey,e.shiftKey,e.altKey,e.metaKey,e.charCode,e.keyCode,e.which,
                e.view===null,e.detail,e.bubbles,e.cancelable,e.composed,
                KeyboardEvent.DOM_KEY_LOCATION_STANDARD,
                KeyboardEvent.DOM_KEY_LOCATION_LEFT,
                KeyboardEvent.DOM_KEY_LOCATION_RIGHT,
                KeyboardEvent.DOM_KEY_LOCATION_NUMPAD].join('|')"#
        ),
        "1|true|true|true|keydown|||0|false|false|false|false|false|false|0|0|0|true|0|false|false|false|0|1|2|3"
    );
}

#[test]
fn constructor_initializes_keys_inherited_fields_and_legacy_codes() {
    assert_eq!(
        keyboard_eval(
            r#"const e=new KeyboardEvent('keydown',{
                 key:'Ω',code:'KeyO',location:2,repeat:true,isComposing:true,
                 ctrlKey:true,shiftKey:true,altKey:true,metaKey:true,
                 charCode:937,keyCode:79,which:7,view:window,detail:3,
                 bubbles:true,cancelable:true,composed:true});
               [e.key,e.code,e.location,e.repeat,e.isComposing,
                e.ctrlKey,e.shiftKey,e.altKey,e.metaKey,
                e.charCode,e.keyCode,e.which,e.view===window,e.detail,
                e.bubbles,e.cancelable,e.composed].join('|')"#
        ),
        "Ω|KeyO|2|true|true|true|true|true|true|937|79|7|true|3|true|true|true"
    );
}

#[test]
fn modifier_state_is_case_sensitive_and_covers_extended_initializer() {
    assert_eq!(
        keyboard_eval(
            r#"const names=['Control','Shift','Alt','Meta','AltGraph','CapsLock',
                  'Fn','FnLock','Hyper','NumLock','ScrollLock','Super','Symbol','SymbolLock'];
               const init={ctrlKey:true,shiftKey:true,altKey:true,metaKey:true,
                  modifierAltGraph:true,modifierCapsLock:true,modifierFn:true,
                  modifierFnLock:true,modifierHyper:true,modifierNumLock:true,
                  modifierScrollLock:true,modifierSuper:true,modifierSymbol:true,
                  modifierSymbolLock:true};
               const all=new KeyboardEvent('x',init);
               const none=new KeyboardEvent('x');
               [names.every(name=>all.getModifierState(name)),
                names.every(name=>!none.getModifierState(name)),
                !all.getModifierState('control'),!all.getModifierState('Unknown'),
                all.getModifierState(null)===false].join('|')"#
        ),
        "true|true|true|true|true"
    );
}

#[test]
fn dictionary_abrupt_completion_prevents_event_publication() {
    assert_eq!(
        keyboard_eval(
            r#"const marker=new RangeError('modifier getter');
               let received=false;
               try { new KeyboardEvent('x',{get modifierCapsLock(){throw marker}}); }
               catch (error) { received=error===marker; }
               [received,new KeyboardEvent('ok').key===''].join('|')"#
        ),
        "true|true"
    );
}

#[test]
fn legacy_initializer_resets_owned_fields_and_preserves_identity() {
    assert_eq!(
        keyboard_eval(
            r#"const e=document.createEvent('KeyboardEvent');
               const same=e;
               e.initKeyboardEvent('keydown',true,true,window,'Z',1,true,true,false,true);
               const before=[e.type,e.bubbles,e.cancelable,e.view===window,e.key,
                  e.location,e.ctrlKey,e.altKey,e.shiftKey,e.metaKey,e.code,
                  e.repeat,e.charCode,e.keyCode,e.which,e.detail].join(':');
               e.initKeyboardEvent('reset');
               [before,e===same,e instanceof KeyboardEvent,e.type,e.bubbles,
                e.cancelable,e.view===null,e.key,e.location,e.ctrlKey,e.altKey,
                e.shiftKey,e.metaKey,e.detail].join('|')"#
        ),
        "keydown:true:true:true:Z:1:true:true:false:true::false:0:0:0:0|true|true|reset|false|false|true||0|false|false|false|false|0"
    );
}

#[test]
fn inherited_ui_initializer_updates_base_without_discarding_keyboard_state() {
    assert_eq!(
        keyboard_eval(
            r#"const e=new KeyboardEvent('old',{key:'kept',code:'KeyK',ctrlKey:true,detail:4});
               e.initUIEvent('new',true,false,window,8);
               [e.type,e.bubbles,e.view===window,e.detail,e.key,e.code,
                e.ctrlKey,e.getModifierState('Control')].join('|')"#
        ),
        "new|true|true|8|kept|KeyK|true|true"
    );
}

#[test]
fn initializer_requires_type_and_is_ignored_while_dispatching() {
    assert_eq!(
        keyboard_eval(
            r#"const e=new KeyboardEvent('probe',{key:'A',ctrlKey:true,detail:5});
               let missing='none';
               try { e.initKeyboardEvent(); } catch(error) { missing=error.name; }
               let during='none';
               document.addEventListener('probe',event=>{
                   event.initKeyboardEvent('changed');
                   during=[event.type,event.key,event.ctrlKey,event.detail].join(':');
               },{once:true});
               document.dispatchEvent(e);
               [missing,during,e.type,e.key,e.ctrlKey,e.detail].join('|')"#
        ),
        "TypeError|probe:A:true:5|probe|A|true|5"
    );
}
