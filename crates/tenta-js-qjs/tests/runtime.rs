//! Real-engine behavior at the QuickJS page boundary.
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use tenta_js::engine::{LifecycleEvent, PageEngine, ScriptErrorKind, ScriptKind};
use tenta_js_qjs::Engine;

fn page() -> Engine {
    let mut engine = Engine::new().expect("QuickJS runtime");
    engine.evaluate("const root=document.createElement('html');document.appendChild(root);root.innerHTML='<head></head><body></body>'").expect("document tree");
    engine
}
fn assert_js(engine: &mut Engine, source: &str, expected: &str) {
    assert_eq!(
        engine
            .evaluate(source)
            .unwrap_or_else(|e| panic!("{source}: {e:?}")),
        expected
    );
}

#[test]
fn persistent_context_and_native_prototypes() {
    let mut engine = page();
    assert_js(&mut engine, "globalThis.answer=40;answer+2", "42");
    assert_js(
        &mut engine,
        "[document instanceof Document,document.body instanceof HTMLElement,document.body instanceof Node,document.body.ownerDocument===document,document.body===document.body].join(',')",
        "true,true,true,true,true",
    );
    assert_js(
        &mut engine,
        "Object.prototype.toString.call(document.body)",
        "[object HTMLElement]",
    );
}
#[test]
fn inherited_methods_and_brand_checks() {
    let mut engine = page();
    assert_js(
        &mut engine,
        "const x=document.createElement('div');x.id='x';document.body.appendChild(x);[x.parentNode===document.body,x.nodeName,document.getElementById('x')===x].join(',')",
        "true,DIV,true",
    );
    assert_js(
        &mut engine,
        "try{Node.prototype.appendChild.call({},document.createElement('div'));'bad'}catch(e){e.name}",
        "TypeError",
    );
    assert_js(
        &mut engine,
        "try{Object.getOwnPropertyDescriptor(Node.prototype,'nodeName').get.call(Object.create(Node.prototype));'bad'}catch(e){e.name}",
        "TypeError",
    );
}
#[test]
fn callbacks_dispatch_reentrantly_and_preserve_identity() {
    let mut engine = page();
    assert_js(
        &mut engine,
        "const b=document.createElement('button');document.body.appendChild(b);let n=0;function listen(e){n++;if(e.target!==b)throw Error('target')}b.addEventListener('click',listen);b.click();b.removeEventListener('click',listen);b.click();n",
        "1",
    );
}
#[test]
fn javascript_exceptions_leave_the_runtime_usable() {
    let mut engine = page();
    let syntax = engine.evaluate("function {").unwrap_err();
    assert_eq!(syntax.kind, ScriptErrorKind::Syntax);
    let thrown = engine.evaluate("throw Error('expected')").unwrap_err();
    assert!(thrown.message.contains("expected"));
    assert_js(&mut engine, "6*7", "42");
    let hostile = engine
        .evaluate("throw {get name(){throw Error('name')},toString(){throw Error('string')}}")
        .unwrap_err();
    assert_eq!(hostile.kind, ScriptErrorKind::Uncaught);
    assert_js(&mut engine, "document.createElement('div').nodeName", "DIV");
}
#[test]
fn promises_run_only_at_checkpoints() {
    let mut engine = page();
    assert_js(
        &mut engine,
        "globalThis.order=[];Promise.resolve().then(()=>order.push('promise'));queueMicrotask(()=>order.push('microtask'));order.push('sync');order.join(',')",
        "sync",
    );
    engine.run_microtasks();
    assert_js(&mut engine, "order.join(',')", "sync,promise,microtask");
}

#[test]
fn page_timer_pump_runs_microtasks_without_a_due_timer() {
    let mut engine = page();
    engine
        .evaluate("globalThis.result='pending';new Response('ready').text().then(v=>result=v)")
        .unwrap();
    assert_eq!(engine.pump_due_timers(10), 0);
    assert_js(&mut engine, "result", "ready");
}

#[test]
fn buffer_sources_use_native_brands_offsets_and_saved_intrinsics() {
    let mut engine = page();
    assert_js(
        &mut engine,
        "const decoder=new TextDecoder();const bytes=new Uint8Array([65,66,67,68]);const view=new DataView(bytes.buffer,1,2);const NativeUint8Array=Uint8Array;Object.defineProperty(view,'buffer',{get(){throw Error('shadowed')}});Object.defineProperty(DataView.prototype,'byteOffset',{get(){throw Error('replaced')}});globalThis.Uint8Array=function(){throw Error('replaced')};[decoder.decode(bytes.buffer),decoder.decode(view),decoder.decode(bytes.subarray(2)),new TextEncoder().encode('ok') instanceof NativeUint8Array].join(':')",
        "ABCD:BC:CD:true",
    );
}

#[test]
fn css_namespace_overloads_and_inline_styles_use_stylo() {
    let mut engine = page();
    assert_js(
        &mut engine,
        r#"const style=document.body.style;style.cssText='content: "a;b:c"; color: blue !important';[CSS.supports('display','grid'),CSS.supports('(display: grid)'),CSS.supports('color','not-a-color'),style.getPropertyValue('content'),style.getPropertyValue('color'),style.getPropertyPriority('color')].join('|')"#,
        "true|true|false|\"a;b:c\"|blue|important",
    );
}
#[test]
fn timers_carry_arguments_and_allow_cancellation() {
    let mut engine = page();
    assert_js(
        &mut engine,
        "globalThis.order=[];const id=setTimeout(()=>order.push('cancelled'),0);clearTimeout(id);setTimeout((a,b)=>{order.push(a+b);Promise.resolve().then(()=>order.push('micro'))},2,20,22);order.length",
        "0",
    );
    engine.advance_time(Duration::from_millis(2));
    assert_eq!(engine.pump_due_timers(10), 1);
    assert_js(&mut engine, "order.join(',')", "42,micro");
}
#[test]
fn script_deadline_interrupts_and_recovers() {
    let mut engine = page();
    let error = engine
        .evaluate_with_timeout("for(;;){}", Some(Duration::from_millis(10)))
        .unwrap_err();
    assert_eq!(error.kind, ScriptErrorKind::Terminated);
    assert_js(&mut engine, "21*2", "42");
}
#[test]
fn deadline_also_bounds_completion_coercion() {
    let mut engine = page();
    let error = engine
        .evaluate_with_timeout("({toString(){for(;;){}}})", Some(Duration::from_millis(10)))
        .unwrap_err();
    assert_eq!(error.kind, ScriptErrorKind::Terminated);
}
#[test]
fn recursive_microtasks_are_bounded() {
    let mut engine = page();
    engine.set_script_timeout(Some(Duration::from_millis(10)));
    engine
        .evaluate("Promise.resolve().then(function again(){Promise.resolve().then(again)})")
        .unwrap();
    engine.run_microtasks();
    assert_js(&mut engine, "3+4", "7");
}
#[test]
fn native_panics_are_uncatchable_and_poison_only_one_runtime() {
    let document = Rc::new(RefCell::new(tenta_dom::Document::new()));
    let mut engine = Engine::with_document(document.clone()).unwrap();
    let borrow = document.borrow_mut();
    let error = engine
        .evaluate("try{document.createElement('div')}catch(e){globalThis.caught=true}")
        .unwrap_err();
    assert_eq!(error.kind, ScriptErrorKind::Terminated);
    assert!(engine.native_failure().is_some());
    drop(borrow);
    assert!(engine.evaluate("1").is_err());
    let mut other = page();
    assert_js(&mut other, "40+2", "42");
}
#[test]
fn lifecycle_handlers_and_trusted_target() {
    let mut engine = page();
    PageEngine::run_script(&mut engine,"globalThis.events=[];document.addEventListener('DOMContentLoaded',e=>events.push(e.type+':'+e.isTrusted));window.onload=e=>events.push(e.type+':'+(e.target===document));",ScriptKind::Classic).unwrap();
    PageEngine::dispatch_lifecycle(&mut engine, LifecycleEvent::DomContentLoaded);
    PageEngine::dispatch_lifecycle(&mut engine, LifecycleEvent::Load);
    assert_js(
        &mut engine,
        "events.join(',')",
        "DOMContentLoaded:true,load:true",
    );
}
#[test]
fn url_and_iterable_headers_use_native_host_records() {
    let mut engine = page();
    assert_js(
        &mut engine,
        "const u=new URL('/x?q=1','https://example.test/');u.searchParams.append('q','2');u.href",
        "https://example.test/x?q=1&q=2",
    );
    assert_js(
        &mut engine,
        "const h=new Headers({alpha:1});h.append('beta','two');[h.get('alpha'),...h.keys()].join(',')",
        "1,alpha,beta",
    );
}
#[test]
fn response_consumption_and_json_promises() {
    let mut engine = page();
    engine
        .evaluate(
            "globalThis.result='pending';new Response('{\"ok\":42}').json().then(v=>result=v.ok)",
        )
        .unwrap();
    engine.run_microtasks();
    assert_js(&mut engine, "result", "42");
}
#[test]
fn abort_throws_the_exact_reason() {
    let mut engine = page();
    assert_js(
        &mut engine,
        "const c=new AbortController();const reason={value:42};c.abort(reason);try{c.signal.throwIfAborted();'bad'}catch(e){e===reason}",
        "true",
    );
}
#[test]
fn range_tracks_shared_document_mutation() {
    let mut engine = page();
    assert_js(
        &mut engine,
        "document.body.textContent='abcdef';const r=document.createRange();r.setStart(document.body.firstChild,1);r.setEnd(document.body.firstChild,4);document.body.firstChild.deleteData(1,3);[document.body.textContent,r.startOffset,r.endOffset].join(':')",
        "aef:1:1",
    );
}

#[test]
fn host_errors_use_domexception_even_after_global_replacement() {
    let mut engine = page();
    assert_js(
        &mut engine,
        "const NativeDOMException=DOMException;globalThis.DOMException=function(){throw Error('tampered')};try{document.appendChild(document);'bad'}catch(e){[e instanceof NativeDOMException,e.name,e.code].join(':')}",
        "true:HierarchyRequestError:3",
    );
}
#[test]
fn two_runtimes_interleave_without_sharing_globals() {
    let mut one = page();
    let mut two = page();
    assert_js(&mut one, "globalThis.marker=11", "11");
    assert_js(&mut two, "typeof marker", "undefined");
    assert_js(&mut two, "globalThis.marker=22", "22");
    drop(one);
    assert_js(&mut two, "marker", "22");
}
