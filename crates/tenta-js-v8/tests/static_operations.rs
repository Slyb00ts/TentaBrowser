//! Static operations through generated V8 bindings, and the ones a script
//! defines over them (`tenta_js::conveniences`).

use std::time::Duration;

use tenta_js::engine::PageEngine;
use tenta_js_v8::Engine;

fn new_engine(document: tenta_dom::SharedDocument) -> Engine {
    Engine::with_document(document)
}

include!("../../tenta-js/tests/support/static_operations.rs");

fn page() -> Engine {
    let mut document = Document::new();
    document.set_url(Some(
        "https://example.com/dir/page.html"
            .parse()
            .expect("test URL"),
    ));
    let mut engine = new_engine(Rc::new(RefCell::new(document)));
    engine.install_preludes();
    engine
}

fn run(engine: &mut Engine, source: &str) -> String {
    PageEngine::evaluate(engine, source, Duration::from_secs(10)).expect("page script")
}

#[test]
fn abort_signal_timeout_aborts_with_a_timeout_error_when_its_timer_fires() {
    let mut engine = page();
    assert_eq!(
        run(
            &mut engine,
            "globalThis.signal=AbortSignal.timeout(40);globalThis.fired=[]; \
             signal.addEventListener('abort',e=>fired.push(e.type,e.target===signal)); \
             const attempt=f=>{try{return f()}catch(e){return e.name}}; \
             [signal instanceof AbortSignal,signal.aborted,AbortSignal.timeout.length, \
              AbortSignal.timeout.name,attempt(()=>AbortSignal.timeout()), \
              attempt(()=>AbortSignal.timeout(-1)),attempt(()=>AbortSignal.timeout(NaN)), \
              AbortSignal.timeout('7') instanceof AbortSignal].join('|')"
        ),
        "true|false|1|timeout|TypeError|TypeError|TypeError|true"
    );
    engine.run_event_loop(64);
    assert_eq!(
        run(
            &mut engine,
            "[signal.aborted,signal.reason instanceof DOMException,signal.reason.name, \
              fired.join(',')].join('|')"
        ),
        "true|true|TimeoutError|abort,true"
    );
}

#[test]
fn abort_signal_any_follows_the_first_source_to_abort() {
    let mut engine = page();
    assert_eq!(
        run(
            &mut engine,
            "const attempt=f=>{try{return f()}catch(e){return e.name}}; \
             const a=new AbortController(),b=new AbortController(); \
             const any=AbortSignal.any([a.signal,b.signal]);let fired=0; \
             any.addEventListener('abort',()=>fired++); \
             const before=any.aborted;b.abort('second');a.abort('first'); \
             const done=AbortSignal.abort('gone'); \
             const already=AbortSignal.any(new Set([a.signal,done])); \
             const empty=AbortSignal.any([]); \
             [any instanceof AbortSignal,before,any.aborted,any.reason,fired, \
              already.aborted,already.reason,empty.aborted,AbortSignal.any.length, \
              attempt(()=>AbortSignal.any([{}])),attempt(()=>AbortSignal.any(1)), \
              attempt(()=>AbortSignal.any())].join('|')"
        ),
        "true|false|true|second|1|true|first|false|1|TypeError|TypeError|TypeError"
    );
}

#[test]
fn response_json_serializes_its_data_and_names_the_type() {
    let mut engine = page();
    assert_eq!(
        run(
            &mut engine,
            "const attempt=f=>{try{return f()}catch(e){return e.name}}; \
             globalThis.texts=[]; \
             const plain=Response.json({a:[1,'x'],toJSON(){return {b:2}}}); \
             const made=Response.json([1],{status:201,statusText:'Made', \
               headers:{'Content-Type':'application/problem+json','X-A':'1'}}); \
             plain.text().then(t=>texts.push(t));made.json().then(v=>texts.push(v[0])); \
             [plain instanceof Response,plain.status,plain.headers.get('content-type'), \
              made.status,made.statusText,made.headers.get('content-type'),made.headers.get('x-a'), \
              Response.json.length,attempt(()=>Response.json(undefined)), \
              attempt(()=>Response.json(()=>1)),attempt(()=>Response.json(1n)), \
              attempt(()=>Response.json()),attempt(()=>Response.json(1,{status:99}))].join('|')"
        ),
        "true|200|application/json|201|Made|application/problem+json|1|1\
         |TypeError|TypeError|TypeError|TypeError|RangeError"
    );
    engine.run_event_loop(8);
    assert_eq!(run(&mut engine, "texts.join('|')"), "{\"b\":2}|1");
}
