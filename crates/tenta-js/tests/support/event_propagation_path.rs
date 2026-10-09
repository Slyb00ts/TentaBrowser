// DOM get-the-parent rules: https://dom.spec.whatwg.org/#document
// Pinned WPT b89af32bc8f42d678f444eb0703bca015ddcf240:
// dom/events/Event-dispatch-bubbles-{true,false}.html.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use tenta_dom::Document;
use tenta_js::engine::PageEngine;

fn path_eval(source: &str) -> String {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = new_engine(document);
    let result = PageEngine::evaluate(&mut engine, source, Duration::from_secs(5))
        .expect("event path script");
    assert_eq!(engine.native_failure(), None);
    result
}

#[test]
fn detached_node_path_stops_at_its_actual_root() {
    assert_eq!(
        path_eval(
            "const parent=document.createElement('div'),child=document.createElement('span');\
             parent.appendChild(child);let path='',calls=[];\
             for(const [target,name] of [[window,'window'],[document,'document'],[parent,'parent'],[child,'child']]){\
                 target.addEventListener('probe',e=>{calls.push(name+':'+e.eventPhase);\
                 if(target===child)path=e.composedPath().map(n=>n===window?'window':n.nodeName).join(',')},true);\
             }child.dispatchEvent(new Event('probe',{bubbles:true}));[path,calls.join(',')].join('|')"
        ),
        "SPAN,DIV|parent:1,child:2"
    );
}

#[test]
fn auxiliary_document_path_has_no_window_even_when_its_target_is_connected() {
    assert_eq!(
        path_eval(
            "const other=document.implementation.createHTMLDocument('other');\
             const node=other.body.appendChild(other.createElement('i'));let observed='',calls=[];\
             window.addEventListener('probe',()=>calls.push('window'),true);\
             other.addEventListener('probe',()=>calls.push('other'),true);\
             node.addEventListener('probe',e=>{const path=e.composedPath();calls.push('node');\
                 observed=[path.includes(window),path[path.length-1]===other,other.defaultView===null].join('|')});\
             node.dispatchEvent(new Event('probe',{bubbles:true}));observed+'|'+calls.join(',')"
        ),
        "false|true|true|other,node"
    );
}

#[test]
fn document_load_stops_before_window_but_direct_window_dispatch_is_preserved() {
    assert_eq!(
        path_eval(
            "const root=document.appendChild(document.createElement('div'));\
             const node=root.appendChild(document.createElement('span'));let paths=[],calls=[];\
             window.addEventListener('probe',e=>calls.push('probe:'+e.eventPhase),true);\
             window.addEventListener('load',e=>calls.push('load:'+e.eventPhase),true);\
             function dispatch(target,type,label){target.addEventListener(type,e=>{const path=e.composedPath();\
                 paths.push(label+':'+path.length+':'+path.includes(window))},{once:true});\
                 target.dispatchEvent(new Event(type,{bubbles:true}));}\
             dispatch(node,'probe','node-probe');dispatch(node,'load','node-load');\
             dispatch(document,'load','document-load');dispatch(window,'load','window-load');\
             paths.join(',')+'|'+calls.join(',')"
        ),
        "node-probe:4:true,node-load:3:false,document-load:1:false,window-load:1:true|probe:1,load:2"
    );
}

// DOM dispatch preserves pre-set propagation flags and clears them only at exit.
// https://dom.spec.whatwg.org/#concept-event-dispatch
// Regression: WPT dom/events/Event-dispatch-bubble-canceled.html.
#[test]
fn propagation_stopped_before_dispatch_skips_listeners_and_resets_for_reuse() {
    for stop in [
        "event.cancelBubble=true",
        "event.cancelBubble=true;event.cancelBubble=false",
        "event.stopPropagation()",
        "event.stopImmediatePropagation()",
    ] {
        let source = format!(
            "const parent=document.createElement('div'),child=document.createElement('span');\
             parent.appendChild(child);let calls=[];\
             for(const [target,name] of [[parent,'parent'],[child,'child']]){{\
               target.addEventListener('probe',()=>calls.push(name+'-capture'),true);\
               target.addEventListener('probe',()=>calls.push(name+'-bubble'));\
             }}\
             const event=new Event('probe',{{bubbles:true}});{stop};\
             const first=child.dispatchEvent(event);\
             const stopped=[first,calls.length,event.cancelBubble,event.eventPhase,\
               event.currentTarget===null,event.target===child,event.composedPath().length].join('|');\
             const second=child.dispatchEvent(event);\
             stopped+'|'+second+'|'+calls.join(',')"
        );
        assert_eq!(
            path_eval(&source),
            "true|0|false|0|true|true|0|true|parent-capture,child-capture,child-bubble,parent-bubble",
            "pre-dispatch operation: {stop}"
        );
    }
}

#[test]
fn stopped_dispatch_preserves_cancellation_for_direct_window_and_foreign_targets() {
    for target in [
        "window",
        "document.implementation.createHTMLDocument('other').body",
    ] {
        let source = format!(
            "const target={target};let calls=0;\
             target.addEventListener('probe',()=>calls++);\
             const event=new Event('probe',{{cancelable:true}});\
             event.preventDefault();event.stopImmediatePropagation();\
             const first=target.dispatchEvent(event);\
             const stopped=[first,calls,event.defaultPrevented,event.cancelBubble,\
               event.currentTarget===null,event.target===target].join('|');\
             const second=target.dispatchEvent(event);\
             stopped+'|'+second+'|'+calls+'|'+event.defaultPrevented"
        );
        assert_eq!(
            path_eval(&source),
            "false|0|true|false|true|true|false|1|true",
            "dispatch target: {target}"
        );
    }
}
