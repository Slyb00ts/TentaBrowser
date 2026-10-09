// Real-engine admission controls shared by the two graph implementations.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use tenta_dom::{Document, SharedDocument};
use tenta_js::engine::PageEngine;

fn pressure_engine() -> (SharedDocument, Engine) {
    let document = Rc::new(RefCell::new(Document::new()));
    let engine = new_engine(document.clone());
    (document, engine)
}

fn run_pressure(engine: &mut Engine, source: &str) -> String {
    PageEngine::evaluate(engine, source, Duration::from_secs(30)).expect("bounded native workload")
}

#[test]
fn uninterrupted_detached_node_creation_reclaims_before_task_end() {
    let (document, mut engine) = pressure_engine();
    let epoch = engine.gc_epoch();
    assert_eq!(
        run_pressure(
            &mut engine,
            "for(let i=0;i<12000;i++){const node=document.createElement('b');node.note=i;} 'done'"
        ),
        "done"
    );
    assert!(
        engine.gc_epoch() > epoch + 1,
        "pressure must collect within this single task"
    );
    assert!(
        document.borrow().len() < 4096,
        "uninterrupted allocation must not retain the whole loop"
    );
    engine.trim_memory();
    document.borrow_mut().sweep_detached();
    assert_eq!(document.borrow().len(), 1);
    assert_eq!(engine.native_failure(), None);
}

#[test]
fn uninterrupted_owner_wrapper_creation_stays_bounded() {
    let (document, mut engine) = pressure_engine();
    assert_eq!(
        run_pressure(
            &mut engine,
            "for(let i=0;i<6000;i++){const owner=document.createElement('div'); \
         owner.style.note=i;owner.attributes.note=i;owner.classList.note=i;} 'done'"
        ),
        "done"
    );
    assert!(
        document.borrow().len() < 2048,
        "owner wrappers must participate in pressure collection"
    );
    engine.trim_memory();
    document.borrow_mut().sweep_detached();
    assert_eq!(document.borrow().len(), 1);
    assert_eq!(engine.native_failure(), None);
}

#[test]
fn connected_wrappers_above_the_deferred_limit_remain_admissible() {
    let (document, mut engine) = pressure_engine();
    assert_eq!(
        run_pressure(
            &mut engine,
            "globalThis.root=document.appendChild(document.createElement('div')); \
         for(let i=0;i<5000;i++){const node=root.appendChild(document.createElement('b'));node.note=i;} \
         [root.firstChild.note,root.lastChild.note,root.childNodes.length].join('|')"
        ),
        "0|4999|5000"
    );
    assert_eq!(document.borrow().len(), 5002);
    assert_eq!(
        run_pressure(
            &mut engine,
            "root.appendChild(document.createElement('i')).note='later'; \
         [root.childNodes.length,root.lastChild.note,root.firstChild.note].join('|')"
        ),
        "5001|later|0"
    );
    assert_eq!(engine.native_failure(), None);
}

#[test]
fn deep_connected_tree_retains_state_across_pressure_checkpoints() {
    let (document, mut engine) = pressure_engine();
    assert_eq!(
        run_pressure(
            &mut engine,
            "globalThis.root=document.appendChild(document.createElement('div')); \
         let tail=root;for(let i=0;i<1200;i++){tail=tail.appendChild(document.createElement('b')); \
         tail.note=i;tail.style.note=i;} \
         [root.firstChild.note,tail.note,tail.style.note].join('|')"
        ),
        "0|1199|1199"
    );
    assert_eq!(document.borrow().len(), 1202);
    engine.trim_memory();
    assert_eq!(
        run_pressure(
            &mut engine,
            "let node=root;while(node.lastChild)node=node.lastChild;[node.note,node.style.note].join('|')"
        ),
        "1199|1199"
    );
    assert_eq!(engine.native_failure(), None);
}

#[test]
fn nested_conversion_refuses_before_host_allocation_and_recovers() {
    let (document, mut engine) = pressure_engine();
    // An engine may also reject the outer constructor after its getter
    // exhausts the budget; the inner allocation and recovery checks remain.
    let result = run_pressure(
        &mut engine,
        "globalThis.kept=document.createTextNode('text');kept.note='argument'; \
         let created=0,refused=0;try{new StaticRange({startContainer:kept,get startOffset(){ \
         for(let i=0;i<10000;i++){try{document.createElement('b');created++;}catch(e){ \
         if(e.name!=='RangeError')throw e;refused++;}}return 0;},endContainer:kept,endOffset:0});} \
         catch(e){if(e.name!=='RangeError')throw e;} \
         [created,refused,kept.note].join('|')",
    );
    let fields: Vec<_> = result.split('|').collect();
    let created: usize = fields[0].parse().unwrap();
    let refused: usize = fields[1].parse().unwrap();
    assert!(created > 0 && refused > 0);
    assert_eq!(created + refused, 10000);
    assert_eq!(fields[2], "argument");
    assert_eq!(
        document.borrow().len(),
        created + 2,
        "refused callbacks must not create unwrapped native nodes"
    );
    assert!(created < 4096);
    assert_eq!(
        run_pressure(
            &mut engine,
            "const next=document.createElement('i');[next.nodeName,kept.note].join('|')"
        ),
        "I|argument"
    );
    assert_eq!(engine.native_failure(), None);
}

#[test]
fn a_paused_parser_retains_detached_wrapper_state_during_pressure() {
    use tenta_dom::{Mutation, SharedSink};
    use tenta_html::{ParseStatus, Parser, ParserOptions};
    let (sink, document) = SharedSink::with_new_document();
    let mut parser = Parser::new(
        sink,
        ParserOptions {
            pause_on_script: true,
            ..Default::default()
        },
    );
    assert_eq!(
        parser.feed_str("<body><div id=parsing><script>s</script><b>after</b></div>"),
        ParseStatus::Script
    );
    let script = parser.take_pending_script().unwrap();
    let parent = document.borrow().parent(script);
    let reference = document.borrow().node_ref(parent);
    let mut engine = new_engine(document.clone());
    assert_eq!(
        run_pressure(
            &mut engine,
            "(()=>{const node=document.getElementById('parsing');node.note='parser';node.remove()})(); \
         for(let i=0;i<12000;i++)document.createElement('i');'done'"
        ),
        "done"
    );
    assert!(
        document.borrow().resolve(reference).is_ok(),
        "pressure must not recycle the parser's raw handle"
    );
    assert!(
        document.borrow().len() < 4096,
        "unrelated script-created nodes must still be reclaimed"
    );
    assert_eq!(parser.resume(), ParseStatus::Continue);
    assert!(
        document
            .borrow()
            .inner_html(parent)
            .contains("<b>after</b>")
    );
    parser.finish();
    let body = document.borrow().body().unwrap();
    document
        .borrow_mut()
        .mutate(Mutation::Append {
            parent: body,
            child: parent,
        })
        .unwrap();
    assert_eq!(
        run_pressure(
            &mut engine,
            "const parsed=document.getElementById('parsing');[parsed.note,parsed.lastChild.textContent].join('|')"
        ),
        "parser|after"
    );
    assert_eq!(engine.native_failure(), None);
}
