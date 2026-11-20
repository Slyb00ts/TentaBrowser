//! The event interfaces a script defines over `Event`, `UIEvent` and
//! `MouseEvent` (`tenta_js::event_interfaces`), on the V8 host.
//!
//! Pages construct them (`new PointerEvent('pointerdown', …)` in a component
//! library's tests of its own gestures, `new InputEvent('input', …)` to tell a
//! framework a field changed), test for them (`'PointerEvent' in window`,
//! `event instanceof SubmitEvent`) and read their attributes off events the
//! engine fires. A missing name is a `ReferenceError` in each case.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::Document;
use tenta_js_v8::Engine;

fn engine() -> Engine {
    let mut document = Document::new();
    document.set_url(Some(
        "https://example.com/app/list?page=1"
            .parse()
            .expect("test URL"),
    ));
    let mut engine = Engine::with_document(Rc::new(RefCell::new(document)));
    engine.install_preludes();
    engine
}

fn eval(engine: &mut Engine, source: &str) -> String {
    engine
        .evaluate(source)
        .expect("test script should evaluate without throwing")
}

#[test]
fn every_interface_is_a_constructor_on_its_parents_chain() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "const parents={InputEvent:UIEvent,PointerEvent:MouseEvent,PopStateEvent:Event, \
               HashChangeEvent:Event,PageTransitionEvent:Event,TransitionEvent:Event, \
               AnimationEvent:Event,SubmitEvent:Event,CloseEvent:Event,StorageEvent:Event, \
               ClipboardEvent:Event,DragEvent:MouseEvent,ToggleEvent:Event, \
               MediaQueryListEvent:Event}; \
             Object.keys(parents).filter(name=>{ \
               const Interface=globalThis[name],made=new Interface('x'); \
               return !(typeof Interface==='function'&&Interface.name===name&& \
                 Interface.length===1&&Object.getPrototypeOf(Interface)===parents[name]&& \
                 made instanceof Interface&&made instanceof parents[name]&& \
                 made instanceof Event&&made.type==='x'&& \
                 Object.prototype.toString.call(made)==='[object '+name+']'&& \
                 made.constructor===Interface&&made.isTrusted===false&& \
                 !Object.keys(globalThis).includes(name)); \
             }).join(',')"
        ),
        ""
    );
}

#[test]
fn attributes_read_the_init_dictionary_and_default_without_one() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "const input=new InputEvent('input',{data:'a',inputType:'insertText', \
               isComposing:1,bubbles:true,detail:3,view:window}); \
             const plain=new InputEvent('input'); \
             const pointer=new PointerEvent('pointerdown',{pointerId:'7',pointerType:'pen', \
               isPrimary:true,pressure:'0.5',width:2.5,clientX:10,button:1,ctrlKey:true}); \
             const still=new PointerEvent('pointermove'); \
             [input.data,input.inputType,input.isComposing,input.bubbles,input.detail, \
              input.view===window,input.dataTransfer,input.getTargetRanges().length, \
              plain.data,plain.inputType,plain.isComposing, \
              pointer.pointerId,pointer.pointerType,pointer.isPrimary,pointer.pressure, \
              pointer.width,pointer.height,pointer.clientX,pointer.button,pointer.ctrlKey, \
              still.pointerId,still.pointerType,still.width,still.tiltX, \
              still.altitudeAngle===Math.PI/2,still.getCoalescedEvents().length].join('|')"
        ),
        "a|insertText|true|true|3|true||0|||false\
         |7|pen|true|0.5|2.5|1|10|1|true|0||1|0|true|0"
    );
}

#[test]
fn the_simple_interfaces_carry_their_members() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "const state={n:1},button=document.createElement('button'); \
             const pop=new PopStateEvent('popstate',{state}); \
             const hash=new HashChangeEvent('hashchange',{oldURL:'a',newURL:'b'}); \
             const page=new PageTransitionEvent('pageshow',{persisted:true}); \
             const transition=new TransitionEvent('transitionend', \
               {propertyName:'opacity',elapsedTime:0.25,pseudoElement:'::before'}); \
             const animation=new AnimationEvent('animationend',{animationName:'spin',elapsedTime:2}); \
             const submit=new SubmitEvent('submit',{submitter:button,cancelable:true}); \
             const close=new CloseEvent('close',{wasClean:true,code:1000,reason:'done'}); \
             const storage=new StorageEvent('storage',{key:'k',oldValue:null,newValue:'v',url:'u'}); \
             const toggle=new ToggleEvent('toggle',{oldState:'closed',newState:'open'}); \
             const media=new MediaQueryListEvent('change',{media:'(min-width: 1px)',matches:true}); \
             const data=new FormData();const form=new FormDataEvent('formdata',{formData:data}); \
             const drag=new DragEvent('dragstart',{clientX:4}); \
             [pop.state===state,new PopStateEvent('popstate').state,pop.hasUAVisualTransition, \
              hash.oldURL,hash.newURL,page.persisted,transition.propertyName, \
              transition.elapsedTime,transition.pseudoElement,animation.animationName, \
              animation.elapsedTime,submit.submitter===button,submit.cancelable, \
              new SubmitEvent('submit').submitter,close.wasClean,close.code,close.reason, \
              storage.key,storage.oldValue,storage.newValue,storage.url,storage.storageArea, \
              toggle.oldState,toggle.newState,media.media,media.matches,form.formData===data, \
              drag.clientX,drag.dataTransfer,new ClipboardEvent('copy').clipboardData].join('|')"
        ),
        "true||false|a|b|true|opacity|0.25|::before|spin|2|true|true||true|1000|done\
         |k||v|u||closed|open|(min-width: 1px)|true|true|4||"
    );
}

#[test]
fn a_constructed_event_is_dispatched_like_any_other() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "const target=document.createElement('input'),seen=[]; \
             document.documentElement?.appendChild(target); \
             target.addEventListener('input',event=>seen.push(event instanceof InputEvent, \
               event.data,event.target===target,event.eventPhase)); \
             target.oninput=event=>seen.push(event.inputType); \
             const event=new InputEvent('input',{data:'x',inputType:'insertText',cancelable:true}); \
             const kept=target.dispatchEvent(event); \
             target.addEventListener('pointerdown',event=>{event.preventDefault(); \
               seen.push(event.pointerType,event instanceof MouseEvent)}); \
             const refused=target.dispatchEvent( \
               new PointerEvent('pointerdown',{pointerType:'touch',cancelable:true})); \
             [seen.join(','),kept,refused,event.defaultPrevented].join('|')"
        ),
        "true,x,true,2,insertText,touch,true|true|false|false"
    );
}

#[test]
fn receivers_and_required_members_are_checked() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "const attempt=f=>{try{return String(f())}catch(e){return e.name}}; \
             const get=(Interface,name)=> \
               Object.getOwnPropertyDescriptor(Interface.prototype,name).get; \
             [attempt(()=>get(InputEvent,'data').call(new Event('x'))), \
              attempt(()=>get(InputEvent,'data').call(new PointerEvent('x'))), \
              attempt(()=>InputEvent('x')),attempt(()=>new InputEvent()), \
              attempt(()=>new FormDataEvent('formdata')), \
              attempt(()=>new FormDataEvent('formdata',{})), \
              attempt(()=>new BeforeUnloadEvent()), \
              typeof BeforeUnloadEvent,Object.getPrototypeOf(BeforeUnloadEvent)===Event, \
              'returnValue' in BeforeUnloadEvent.prototype, \
              Object.keys(InputEvent.prototype).join(','), \
              get(PopStateEvent,'state').name].join('|')"
        ),
        "TypeError|TypeError|TypeError|TypeError|TypeError|TypeError|TypeError\
         |function|true|true|data,isComposing,inputType,dataTransfer,getTargetRanges|get state"
    );
}

#[test]
fn a_subclass_keeps_the_members_of_its_interface() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "class Swipe extends PointerEvent{constructor(direction){ \
               super('swipe',{pointerType:'touch',bubbles:true});this.direction=direction}} \
             const swipe=new Swipe('left');let seen; \
             addEventListener('swipe',event=>{seen=[event instanceof Swipe,event.direction, \
               event.pointerType].join(',')}); \
             document.dispatchEvent(swipe); \
             [swipe instanceof PointerEvent,swipe.pointerType,seen].join('|')"
        ),
        "true|touch|true,left,touch"
    );
}

/// The two events of the session history are the engine's to fire, and they
/// are of their interfaces: with the script's own constructors, whatever the
/// page did to the globals.
#[test]
fn popstate_and_hashchange_are_fired_with_their_interfaces() {
    let mut engine = engine();
    eval(
        &mut engine,
        "globalThis.log=[];globalThis.Kept=[PopStateEvent,HashChangeEvent]; \
         globalThis.PopStateEvent=globalThis.HashChangeEvent=undefined; \
         addEventListener('popstate',event=>log.push(event instanceof Kept[0], \
           JSON.stringify(event.state),Object.keys(event).includes('state'))); \
         addEventListener('hashchange',event=>log.push(event instanceof Kept[1], \
           event.oldURL.split('#')[1]||'',event.newURL.split('#')[1])); \
         history.pushState({n:1},'','/one');history.pushState({n:2},'','/two'); \
         history.back();'queued'",
    );
    engine.run_event_loop(16);
    eval(&mut engine, "location.hash='#frag';'set'");
    engine.run_event_loop(16);
    assert_eq!(
        eval(&mut engine, "log.join('|')"),
        "true|{\"n\":1}|false|true||frag"
    );
}
