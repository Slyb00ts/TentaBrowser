//! Constructed stylesheet behavior through generated V8 bindings.

use tenta_js_v8::Engine;

fn new_engine(document: tenta_dom::SharedDocument) -> Engine {
    Engine::with_document(document)
}

include!("../../tenta-js/tests/support/cssom_sheet.rs");

/// `replace()` is V8's alone for now: it is defined natively beside the
/// generated interface, and QuickJS has no counterpart.
#[test]
fn replace_reports_through_a_promise_what_replace_sync_does() {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = new_engine(document);
    engine.install_preludes();
    engine
        .evaluate(
            "globalThis.out=[];const sheet=new CSSStyleSheet(); \
             const promise=sheet.replace('@import url(x.css); a{color:red} b{color:blue}'); \
             out.push(promise instanceof Promise,sheet.cssRules.length); \
             promise.then(value=>out.push('fulfilled',value===sheet)); \
             CSSStyleSheet.prototype.replace.call({}, 'a{}').then( \
               ()=>out.push('wrong'),error=>out.push('receiver',error.name)); \
             sheet.replace().then(()=>out.push('wrong'),error=>out.push('arity',error.name)); \
             const root=document.appendChild(document.createElement('html')); \
             const style=root.appendChild(document.createElement('style')); \
             style.textContent='a{color:red}';const element=style.sheet; \
             element.replace('b{}').then(()=>out.push('wrong'), \
               error=>out.push('element',error.name,element.cssRules.length)); \
             let sync='none';try{element.replaceSync('b{}')}catch(error){sync=error.name} \
             out.push(sync,typeof CSSStyleSheet.prototype.replace, \
               CSSStyleSheet.prototype.replace.length,CSSStyleSheet.prototype.replace.name);",
        )
        .expect("script");
    engine.run_event_loop(16);
    assert_eq!(
        engine.evaluate("out.join('|')").expect("result"),
        "true|2|NotAllowedError|function|1|replace|fulfilled|true|receiver|TypeError|\
         arity|TypeError|element|NotAllowedError|1"
    );
}
