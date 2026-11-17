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
fn classic_scripts_are_sloppy_unless_they_opt_into_strict_mode() {
    let mut engine = page();
    // An undeclared assignment creates a global, and `this` of a plain call is
    // the global object: both throw or differ under strict mode.
    assert_js(
        &mut engine,
        "undeclared = 5; [typeof undeclared, (function () { return this === window; })()].join(',')",
        "number,true",
    );
    engine
        .run_script("fromPageScript = 7", ScriptKind::Classic)
        .expect("a sloppy page script");
    assert_js(&mut engine, "String(fromPageScript)", "7");
    assert_js(
        &mut engine,
        "(function () { 'use strict'; try { alsoUndeclared = 1; return 'sloppy'; } \
         catch (e) { return e.name; } })()",
        "ReferenceError",
    );
}
#[test]
fn media_canvas_and_svg_elements_have_their_interfaces() {
    let mut engine = page();
    assert_js(
        &mut engine,
        "var ns = 'http://www.w3.org/2000/svg'; \
         var v = document.createElement('video'), c = document.createElement('canvas'); \
         var svg = document.createElementNS(ns, 'svg'), a = document.createElementNS(ns, 'a'); \
         v.src = '/clip.mp4'; v.muted = true; \
         [v instanceof HTMLVideoElement, v instanceof HTMLMediaElement, \
          document.createElement('audio') instanceof HTMLAudioElement, \
          c instanceof HTMLCanvasElement, svg instanceof SVGSVGElement, \
          a instanceof SVGAElement, a instanceof SVGGraphicsElement, \
          v.paused, v.readyState, v.canPlayType('video/mp4'), v.hasAttribute('muted'), \
          c.width + 'x' + c.height, c.getContext('2d'), c.toDataURL(), \
          v.play() instanceof Promise].join('|')",
        "true|true|true|true|true|true|true|true|0||true|300x150||data:,|true",
    );
}

#[test]
fn meta_source_and_dialog_interfaces_and_random_uuids() {
    let mut engine = page();
    assert_js(
        &mut engine,
        "var dialog = document.createElement('dialog'); dialog.showModal(); \
         [document.createElement('meta') instanceof HTMLMetaElement, \
          document.createElement('source') instanceof HTMLSourceElement, \
          dialog instanceof HTMLDialogElement, dialog.open, \
          /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(crypto.randomUUID()), \
          typeof document.domain].join('|')",
        "true|true|true|true|true|string",
    );
}

/// Shadow trees are in the shared bindings, so QuickJS has them too: the
/// root, its separate tree, slots, and an event leaving through the host.
#[test]
fn shadow_trees_work_on_quickjs() {
    let mut engine = page();
    assert_js(
        &mut engine,
        "var host = document.body.appendChild(document.createElement('div')); \
         host.innerHTML = '<b slot=title>T</b>'; \
         var shadow = host.attachShadow({mode: 'open'}); \
         shadow.innerHTML = '<slot name=title></slot><button id=go>go</button>'; \
         var go = shadow.getElementById('go'), seen = []; \
         host.addEventListener('click', function (e) { seen.push(e.target === host); }); \
         go.click(); \
         var threw = ''; \
         try { host.attachShadow({mode: 'open'}); } catch (e) { threw = e.name; } \
         [shadow instanceof ShadowRoot, shadow instanceof DocumentFragment, host.shadowRoot === shadow, \
          shadow.host === host, shadow.mode, String(document.getElementById('go')), go.isConnected, \
          go.getRootNode() === shadow, shadow.firstChild instanceof HTMLSlotElement, \
          shadow.firstChild.assignedNodes().length, host.firstChild.assignedSlot === shadow.firstChild, \
          seen.join(), threw].join('|')",
        "true|true|true|true|open|null|true|true|true|1|true|true|NotSupportedError",
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
fn a_body_is_read_as_a_buffer_bytes_or_a_blob_and_a_response_is_cloned() {
    let mut engine = page();
    engine
        .evaluate(
            "globalThis.out=[];\
             const r=new Response('hi',{status:201,headers:{'content-type':'text/x-test'}});\
             const c=r.clone();out.push(c.status,c!==r,c.headers.get('content-type'));\
             r.arrayBuffer().then(b=>{out.push(b instanceof ArrayBuffer,Array.from(new Uint8Array(b)).join(','));return c.blob()})\
              .then(b=>{out.push(b instanceof Blob,b.size,b.type);return b.bytes()})\
              .then(v=>{out.push(v instanceof Uint8Array,v.length);return new Blob(['abc']).arrayBuffer()})\
              .then(b=>out.push(b.byteLength));\
             try{r.clone();out.push('cloned')}catch(e){out.push(e instanceof TypeError)}",
        )
        .unwrap();
    engine.run_microtasks();
    assert_js(
        &mut engine,
        "out.join('|')",
        "201|true|text/x-test|true|true|104,105|true|2|text/x-test|true|2|3",
    );
}
#[test]
fn a_request_is_constructed_copied_cloned_and_read() {
    let mut engine = page();
    engine
        .evaluate(
            "globalThis.out=[];\
             const r=new Request('http://fixture.test/a?x=1',{method:'post',headers:{'X-A':'1'},body:'one',credentials:'include'});\
             out.push(r.method,r.url,r.headers.get('x-a'),r.credentials,r.mode,r.bodyUsed,r.signal instanceof AbortSignal,r.signal===r.signal);\
             const c=r.clone();const d=new Request(r,{method:'PUT'});\
             out.push(c.method,d.method,r.bodyUsed,d.headers.get('x-a'));\
             try{new Request(r);out.push('copied')}catch(e){out.push(e instanceof TypeError)}\
             try{new Request('http://fixture.test/a',{body:'x'});out.push('made')}catch(e){out.push(e instanceof TypeError)}\
             const s=new AbortController();const g=new Request('http://fixture.test/b',{signal:s.signal});s.abort();\
             out.push(g.signal===s.signal,g.signal.aborted);\
             c.text().then(t=>{out.push(t);return d.text()}).then(t=>out.push(t));",
        )
        .unwrap();
    engine.run_microtasks();
    assert_js(
        &mut engine,
        "out.join('|')",
        "POST|http://fixture.test/a?x=1|1|include|cors|false|true|true|POST|PUT|true|1|true|true|true|true|one|one",
    );
}
#[test]
fn an_error_event_carries_its_five_members_to_a_listener() {
    // The window of this backend has no `onerror` handler attribute, so the
    // handler's own five-argument call is not reachable here; the interface
    // and a listener are.
    let mut engine = page();
    assert_js(
        &mut engine,
        "globalThis.out=[];const reason={};\
         const e=new ErrorEvent('error',{message:'m',filename:'f.js',lineno:3,colno:7,error:reason,cancelable:true});\
         out.push(e instanceof Event,new ErrorEvent('x').error===undefined);\
         window.addEventListener('error',function(event){out.push(event===e,event.message,event.filename,event.lineno,event.colno,event.error===reason);event.preventDefault()});\
         window.dispatchEvent(e);out.push(e.defaultPrevented);\
         out.join('|')",
        "true|true|true|m|f.js|3|7|true|true",
    );
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

/// `EventTarget`'s operations take the window as their receiver on QuickJS as
/// well: called on it through the prototype, and bare.
#[test]
fn event_target_operations_take_the_window_as_receiver() {
    let mut engine = page();
    assert_js(
        &mut engine,
        "var seen = []; var proto = EventTarget.prototype; \
         proto.addEventListener.call(window, 'ping', function () { seen.push('called'); }); \
         var bare = proto.addEventListener; bare('ping', function () { seen.push('bare'); }); \
         proto.dispatchEvent.call(window, new Event('ping')); \
         seen.join(',')",
        "called,bare",
    );
}
