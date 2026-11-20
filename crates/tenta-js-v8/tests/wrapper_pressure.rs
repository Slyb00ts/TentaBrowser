//! Real V8 wrapper-graph admission and bounded collection controls.

use tenta_js_v8::Engine;

fn new_engine(document: tenta_dom::SharedDocument) -> Engine {
    Engine::with_document(document)
}

include!("../../tenta-js/tests/support/wrapper_pressure.rs");

#[test]
fn composed_path_batch_publication_refuses_without_losing_the_original_error() {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Duration;
    use tenta_core::atom::atoms;
    use tenta_dom::Mutation;
    use tenta_html::QualName;
    use tenta_js::engine::PageEngine;

    let document = Rc::new(RefCell::new(tenta_dom::Document::new()));
    // Build natively so the ancestors do not have existing JS wrappers. The
    // one composedPath() result must publish the whole interface sequence in
    // a single callback frame and hit the hard admission bound partway through.
    {
        let mut doc = document.borrow_mut();
        let mut parent = doc.document_node();
        for _ in 0..4300 {
            let child = doc.create_html_element(atoms::DIV);
            doc.mutate(Mutation::Append { parent, child })
                .expect("build connected event path");
            parent = child;
        }
        doc.mutate(Mutation::SetAttribute {
            element: parent,
            name: QualName::new(atoms::EMPTY, atoms::ID),
            value: "tail".to_owned(),
        })
        .expect("index event target");
    }
    let mut engine = Engine::with_document(document.clone());
    let result = PageEngine::evaluate(
        &mut engine,
        "globalThis.tail=document.getElementById('tail');\
         globalThis.observed='not-run';\
         tail.addEventListener('x',e=>{try{e.composedPath();observed='no-error'}\
         catch(error){observed=error.name+'|'+error.message}});\
         tail.dispatchEvent(new Event('x'));observed",
        Duration::from_secs(30),
    )
    .expect("caught outgoing interface sequence refusal");
    assert_eq!(
        result,
        "RangeError|native wrapper allocation limit exceeded"
    );
    assert_eq!(document.borrow().len(), 4301);
    assert_eq!(engine.native_failure(), None);
    assert_eq!(
        PageEngine::evaluate(
            &mut engine,
            "document.createElement('i').nodeName",
            Duration::from_secs(5),
        )
        .expect("the next task can allocate after graph recovery"),
        "I"
    );
}

fn body_document() -> tenta_dom::SharedDocument {
    use tenta_core::atom::atoms;
    use tenta_dom::Mutation;

    let document = std::rc::Rc::new(std::cell::RefCell::new(tenta_dom::Document::new()));
    {
        let mut doc = document.borrow_mut();
        let root = doc.document_node();
        let html = doc.create_html_element(atoms::HTML);
        let body = doc.create_html_element(atoms::BODY);
        doc.mutate(Mutation::Append {
            parent: root,
            child: html,
        })
        .expect("append <html>");
        doc.mutate(Mutation::Append {
            parent: html,
            child: body,
        })
        .expect("append <body>");
    }
    document
}

/// Creates, connects, reads and removes `count` elements, one new wrapper each.
const WORK: &str = "const work=count=>{let n=0;for(let i=0;i<count;i++){ \
     const e=document.createElement('i');document.body.appendChild(e); \
     n+=e.parentNode===document.body?1:0;e.remove()}return n}; \
     const attempt=f=>{try{return String(f())}catch(e){return e.name}};";

/// A listener called by `dispatchEvent()` or `click()` runs inside that
/// binding call, where nothing can be collected. It is a frame of its own for
/// the wrapper bound, so a handler that builds a long list works, also when
/// it was itself dispatched by a listener, and the debt it leaves is collected
/// by the next call from the top.
#[test]
fn a_listener_called_from_script_may_touch_many_nodes() {
    use std::time::Duration;
    use tenta_js::engine::PageEngine;

    let mut engine = Engine::with_document(body_document());
    let result = PageEngine::evaluate(
        &mut engine,
        &format!(
            "{WORK} \
             const target=document.createElement('button');document.body.appendChild(target); \
             let dispatched,clicked,nested,second; \
             target.addEventListener('x',()=>{{dispatched=attempt(()=>work(20000))}}); \
             target.addEventListener('x',()=>{{second=attempt(()=>work(5000))}}); \
             target.dispatchEvent(new Event('x')); \
             target.onclick=()=>{{clicked=attempt(()=>work(20000))}}; \
             target.click(); \
             const inner=document.createElement('i'); \
             inner.addEventListener('y',()=>{{nested=attempt(()=>work(6000))}}); \
             target.addEventListener('z',()=>{{inner.dispatchEvent(new Event('y')); \
               nested+='+'+attempt(()=>work(6000))}}); \
             target.dispatchEvent(new Event('z')); \
             [dispatched,second,clicked,nested,attempt(()=>work(20000))].join('|')"
        ),
        Duration::from_secs(120),
    )
    .expect("the listeners ran");
    assert_eq!(result, "20000|5000|20000|6000+6000|20000");
    assert_eq!(engine.native_failure(), None);
}

/// The callbacks native code calls from a task of its own are frames too:
/// an idle callback and a port's message handler run inside the native
/// function that is the task, and a `forEach` callback inside `forEach`.
#[test]
fn callbacks_called_by_native_tasks_may_touch_many_nodes() {
    use std::time::Duration;
    use tenta_js::engine::PageEngine;

    let mut engine = Engine::with_document(body_document());
    let first = PageEngine::evaluate(
        &mut engine,
        &format!(
            "{WORK} \
             globalThis.seen={{}}; \
             requestIdleCallback(()=>{{seen.idle=attempt(()=>work(20000))}}); \
             const channel=new MessageChannel(); \
             channel.port1.onmessage=()=>{{seen.port=attempt(()=>work(20000))}}; \
             channel.port2.postMessage(1); \
             const other=new MessageChannel(); \
             other.port1.addEventListener('message',()=>{{seen.listener=attempt(()=>work(20000))}}); \
             other.port1.start();other.port2.postMessage(1); \
             class Big extends HTMLElement{{ \
               constructor(){{super();seen.constructed=attempt(()=>work(6000))}} \
               connectedCallback(){{seen.connected=attempt(()=>work(6000))}}}} \
             customElements.define('x-big',Big); \
             const host=document.createElement('div');document.body.appendChild(host); \
             host.addEventListener('x',()=>{{host.appendChild(document.createElement('x-big'))}}); \
             host.dispatchEvent(new Event('x')); \
             const data=new FormData();for(let i=0;i<6000;i++)data.append('k'+i,'v'); \
             let each=0; \
             seen.forEach=attempt(()=>{{data.forEach(()=>{{document.createElement('i');each++}});return each}}); \
             [seen.constructed,seen.connected,seen.forEach].join('|')"
        ),
        Duration::from_secs(120),
    )
    .expect("the page script ran");
    assert_eq!(first, "6000|6000|6000");
    engine.run_event_loop(16);
    assert_eq!(
        PageEngine::evaluate(
            &mut engine,
            "[seen.idle,seen.port,seen.listener].join('|')",
            Duration::from_secs(5),
        )
        .expect("the tasks ran"),
        "20000|20000|20000"
    );
    assert_eq!(engine.native_failure(), None);
}

/// The frame of a listener does not lift the bound on one binding call: an
/// argument conversion that runs script inside a listener's call still spends
/// that call's allowance and no more.
#[test]
fn a_conversion_inside_a_listener_keeps_the_bound_of_its_call() {
    use std::time::Duration;
    use tenta_js::engine::PageEngine;

    let mut engine = Engine::with_document(body_document());
    let result = PageEngine::evaluate(
        &mut engine,
        &format!(
            "{WORK} \
             const target=document.createElement('i');const node=document.createTextNode('x'); \
             let created=0,refused=0,after; \
             target.addEventListener('x',()=>{{ \
               work(3000); \
               try{{new StaticRange({{startContainer:node,get startOffset(){{ \
                 for(let i=0;i<10000;i++){{try{{document.createElement('b');created++}} \
                   catch(e){{if(e.name!=='RangeError')throw e;refused++}}}}return 0}}, \
                 endContainer:node,endOffset:0}})}}catch(e){{if(e.name!=='RangeError')throw e}} \
               after=attempt(()=>work(3000))}}); \
             target.dispatchEvent(new Event('x')); \
             [created>0,created<=4096,created+refused,after].join('|')"
        ),
        Duration::from_secs(120),
    )
    .expect("the listener ran");
    assert_eq!(result, "true|true|10000|3000");
    assert_eq!(engine.native_failure(), None);
}

/// The bound on a dispatch is still a bound: a listener that never stops is
/// refused its next wrapper, and the page goes on after the call returns.
#[test]
fn a_listener_that_never_stops_is_still_refused() {
    use std::time::Duration;
    use tenta_js::engine::PageEngine;

    let mut engine = Engine::with_document(body_document());
    let result = PageEngine::evaluate(
        &mut engine,
        "const target=document.createElement('i');let outcome='none',made=0; \
         target.addEventListener('x',()=>{try{for(;;){document.createElement('i');made++}} \
           catch(e){outcome=e.name+'|'+e.message}}); \
         target.dispatchEvent(new Event('x')); \
         [outcome,made>4096,made<=262144].join('|')",
        Duration::from_secs(300),
    )
    .expect("the refusal was caught");
    assert_eq!(
        result,
        "RangeError|native wrapper allocation limit exceeded|true|true"
    );
    assert_eq!(
        PageEngine::evaluate(
            &mut engine,
            "document.createElement('i').nodeName",
            Duration::from_secs(5),
        )
        .expect("the next task can allocate after the graph is collected"),
        "I"
    );
    assert_eq!(engine.native_failure(), None);
}
