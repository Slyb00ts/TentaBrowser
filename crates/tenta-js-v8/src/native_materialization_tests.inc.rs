// Inspect native records inside an uninterrupted script before task-end GC.

fn event_records(engine: &Engine) -> usize {
    engine
        .dom
        .as_ref()
        .expect("DOM world")
        .ctx
        .events
        .borrow()
        .len()
}

#[test]
fn uninterrupted_event_self_cycles_bound_native_records_before_task_end() {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = Engine::with_document(document);
    let before = event_records(&engine);

    assert_eq!(
        run_pressure_without_task_end(
            &mut engine,
            "for(let i=0;i<12000;i++){\
               const event=new Event('pressure');event.self=event;} 'done'"
        ),
        "done"
    );
    // Native Event records are sampled inside the uninterrupted task, before
    // Engine::run_inner's final GC could hide missing admission checkpoints.
    assert!(
        event_records(&engine) < before + 4096,
        "Event records exceeded the hard materialization bound"
    );
    assert_eq!(engine.native_failure(), None);
    {
        let _entry = engine.entered();
        assert!(engine.graph_checkpoint());
        engine.reclaim_dead_hosts();
    }
    assert_eq!(event_records(&engine), before);
}

#[test]
fn uninterrupted_text_encoder_self_cycles_bound_native_records_before_task_end() {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = Engine::with_document(document);
    let before = engine.host_record_count();

    assert_eq!(
        run_pressure_without_task_end(
            &mut engine,
            "for(let i=0;i<12000;i++){\
               const encoder=new TextEncoder();encoder.self=encoder;} 'done'"
        ),
        "done"
    );
    assert!(
        engine.host_record_count() < before + 4096,
        "TextEncoder records exceeded the hard materialization bound"
    );
    assert_eq!(engine.native_failure(), None);
    {
        let _entry = engine.entered();
        assert!(engine.graph_checkpoint());
        engine.reclaim_dead_hosts();
    }
    assert_eq!(engine.host_record_count(), before);
}

#[test]
fn nested_event_constructors_refuse_before_publishing_native_records() {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = Engine::with_document(document);
    let before = event_records(&engine);
    let result = engine
        .evaluate(
            "const node=document.createTextNode('x');\
             globalThis.retainedEvents=[];let created=0,refused=0;\
             try{new StaticRange({startContainer:node,get startOffset(){\
               for(let i=0;i<10000;i++){try{\
                 const event=new Event('pressure');\
                 retainedEvents.push(event);created++;\
               }catch(error){if(error.name!=='RangeError')throw error;refused++;}}\
               return 0;},endContainer:node,endOffset:0});}\
             catch(error){if(error.name!=='RangeError')throw error;}\
             [created,refused].join('|')",
        )
        .expect("nested Event constructor admission script");
    let (created, refused) = result.split_once('|').expect("two counters");
    let created: usize = created.parse().expect("created count");
    let refused: usize = refused.parse().expect("refused count");
    assert!(created > 0 && created < 4096);
    assert!(refused > 0);
    assert_eq!(created + refused, 10000);
    assert_eq!(
        event_records(&engine),
        before + created,
        "a refused constructor left an orphan native Event record"
    );
    // Empty the array before dropping it: the getter above is hot enough for
    // a concurrent optimizing job, which embeds the array as a constant and
    // keeps it alive while it runs. A low-memory GC flushes compile queues
    // without waiting for a running job, so the array itself can outlive it.
    engine
        .evaluate("retainedEvents.length=0;retainedEvents=undefined")
        .expect("release the retained Event wrappers");
    engine.low_memory_gc();
    engine.reclaim_dead_hosts();
    assert_eq!(event_records(&engine), before);
    assert_eq!(engine.native_failure(), None);
}

#[test]
fn converted_custom_event_detail_is_released_when_nested_constructors_spend_budget() {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = Engine::with_document(document);
    let records_before = event_records(&engine);
    let values_before = engine.dom.as_ref().unwrap().state.borrow().value_count();
    let result = engine
        .evaluate(
            "globalThis.retainedEvents=[];let created=0,refused=0,outerRefused=false;\
             const marker={};\
             try{new CustomEvent('outer',{get detail(){\
               for(let i=0;i<10000;i++){try{const event=new Event('nested');\
                 retainedEvents.push(event);created++;}\
                 catch(error){if(error.name!=='RangeError')throw error;refused++;}}\
               return marker;}});}\
             catch(error){if(error.name!=='RangeError')throw error;outerRefused=true;}\
             [created,refused,outerRefused].join('|')",
        )
        .expect("constructor conversion budget script");
    let mut fields = result.split('|');
    let created: usize = fields.next().unwrap().parse().unwrap();
    let refused: usize = fields.next().unwrap().parse().unwrap();
    assert_eq!(fields.next(), Some("true"));
    assert!(created > 0 && created < 4096);
    assert_eq!(created + refused, 10000);
    assert_eq!(event_records(&engine), records_before + created);
    assert_eq!(
        engine.dom.as_ref().unwrap().state.borrow().value_count(),
        values_before,
        "a rejected CustomEvent retained its converted detail",
    );
    // Emptied first for the reason given in
    // `nested_event_constructors_refuse_before_publishing_native_records`.
    engine
        .evaluate("retainedEvents.length=0;retainedEvents=undefined")
        .unwrap();
    engine.low_memory_gc();
    engine.reclaim_dead_hosts();
    assert_eq!(event_records(&engine), records_before);
    assert_eq!(engine.native_failure(), None);
}

#[test]
fn nested_custom_events_transfer_each_reused_detail_reference() {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = Engine::with_document(document);
    let records_before = event_records(&engine);
    let values_before = engine.dom.as_ref().unwrap().state.borrow().value_count();
    assert_eq!(
        engine
            .evaluate(
                "globalThis.marker={note:'alive'};globalThis.retainedEvents=[];\
                 (()=>{const outer=new CustomEvent('outer',{get detail(){\
                   retainedEvents.push(new CustomEvent('inner',{detail:marker}));\
                   retainedEvents.push(new CustomEvent('inner',{detail:marker}));\
                   return marker;}});retainedEvents.push(outer);})();\
                 retainedEvents.map(event=>event.detail===marker).join('|')"
            )
            .unwrap(),
        "true|true|true"
    );
    assert_eq!(event_records(&engine), records_before + 3);
    assert_eq!(
        engine.dom.as_ref().unwrap().state.borrow().value_count(),
        values_before + 1,
        "three references to one marker must use one interned slot",
    );
    tenta_js::engine::PageEngine::run_microtasks(&mut engine);
    engine.low_memory_gc();
    engine.reclaim_dead_hosts();
    assert_eq!(
        engine
            .evaluate("retainedEvents.every(event=>event.detail===marker)")
            .unwrap(),
        "true"
    );
    engine
        .evaluate("retainedEvents.splice(0,2);marker=undefined;'released'")
        .unwrap();
    tenta_js::engine::PageEngine::run_microtasks(&mut engine);
    engine.low_memory_gc();
    engine.reclaim_dead_hosts();
    assert_eq!(event_records(&engine), records_before + 1);
    assert_eq!(
        engine.dom.as_ref().unwrap().state.borrow().value_count(),
        values_before + 1,
        "the surviving event must retain its share of the stored detail",
    );
    assert_eq!(
        engine.evaluate("retainedEvents[0].detail.note").unwrap(),
        "alive"
    );
    engine.evaluate("retainedEvents=[]").unwrap();
    tenta_js::engine::PageEngine::run_microtasks(&mut engine);
    engine.low_memory_gc();
    engine.reclaim_dead_hosts();
    assert_eq!(event_records(&engine), records_before);
    assert_eq!(
        engine.dom.as_ref().unwrap().state.borrow().value_count(),
        values_before
    );
    assert_eq!(engine.native_failure(), None);
}

#[test]
fn custom_detail_cycle_releases_its_native_event_record() {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = Engine::with_document(document);
    let before = event_records(&engine);
    engine
        .evaluate(
            "globalThis.heldCustom=(()=>{const detail={};const event=new CustomEvent('x',{detail});detail.event=event;return event})()",
        )
        .expect("create held CustomEvent cycle");
    for _ in 0..3 {
        engine.run_microtasks();
        tenta_js::engine::PageEngine::trim_memory(&mut engine);
    }
    assert_eq!(event_records(&engine), before + 1);
    engine.evaluate("globalThis.heldCustom=null").unwrap();
    for _ in 0..3 {
        engine.run_microtasks();
        tenta_js::engine::PageEngine::trim_memory(&mut engine);
    }
    assert_eq!(event_records(&engine), before);
    assert_eq!(engine.native_failure(), None);
}
