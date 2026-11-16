// Include inside lib.rs::tests after css_graph_pressure_tests.inc.rs so the
// existing raw-task runner is available without a production test seam.

#[test]
fn standalone_target_listener_cycles_bound_native_records_inside_one_task() {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = Engine::with_document(document);
    let records_before = engine.host_record_count();
    let listeners_before = engine.listener_count();
    let values_before = engine.stored_value_count();
    let epoch_before = engine
        .gc_counter
        .epoch
        .load(std::sync::atomic::Ordering::Relaxed);

    assert_eq!(
        run_pressure_without_task_end(
            &mut engine,
            r#"for(let i=0;i<12000;i++){
                const target=new EventTarget();
                target.note=i;
                target.self=target;
                target.addEventListener('x',()=>target.note);
            }
            'done'"#
        ),
        "done"
    );
    assert!(
        engine
            .gc_counter
            .epoch
            .load(std::sync::atomic::Ordering::Relaxed)
            > epoch_before,
        "standalone listener cycles need an in-task pressure checkpoint"
    );
    assert!(
        engine.host_record_count() < records_before + crate::world::HARD_WRAPPER_MATERIALIZATIONS,
        "one standalone record per wrapper must remain bounded"
    );
    assert!(
        engine.listener_count() < listeners_before + crate::world::HARD_WRAPPER_MATERIALIZATIONS,
        "listener registrations must be drained with unreachable targets"
    );
    assert!(
        engine.stored_value_count() < values_before + crate::world::HARD_WRAPPER_MATERIALIZATIONS,
        "callback values must be released with unreachable targets"
    );
    assert_eq!(engine.native_failure(), None);

    {
        let _entry = engine.entered();
        assert!(engine.graph_checkpoint());
        engine.reclaim_dead_hosts();
    }
    assert_eq!(engine.host_record_count(), records_before);
    assert_eq!(engine.listener_count(), listeners_before);
    assert_eq!(engine.stored_value_count(), values_before);
}

#[test]
fn held_event_keeps_one_standalone_target_record_until_its_edge_is_reset() {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = Engine::with_document(document);
    let records_before = engine.host_record_count();

    assert_eq!(
        engine
            .evaluate(
                r#"globalThis.savedEvent=(()=>{
                    const target=new EventTarget();
                    target.note=41;
                    const event=new Event('x');
                    target.dispatchEvent(event);
                    return event;
                })();
                'ready'"#
            )
            .unwrap(),
        "ready"
    );
    engine.low_memory_gc();
    engine.reclaim_dead_hosts();
    assert_eq!(engine.host_record_count(), records_before + 1);
    assert_eq!(engine.evaluate("savedEvent.target.note").unwrap(), "41");

    engine
        .evaluate("savedEvent.initEvent('reset',false,false)")
        .unwrap();
    engine.low_memory_gc();
    engine.reclaim_dead_hosts();
    assert_eq!(engine.host_record_count(), records_before);
    assert_eq!(engine.native_failure(), None);
}

#[test]
fn refused_standalone_constructors_leave_exactly_the_published_records() {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = Engine::with_document(document);
    let records_before = engine.host_record_count();
    let result = engine
        .evaluate(
            r#"const node=document.createTextNode('x');
               globalThis.retainedTargets=[];
               let created=0,refused=0;
               try { new StaticRange({
                   startContainer:node,
                   get startOffset(){
                       for(let i=0;i<10000;i++){
                           try {
                               retainedTargets.push(new EventTarget());
                               created++;
                           } catch(error) {
                               if(error.name!=='RangeError') throw error;
                               refused++;
                           }
                       }
                       return 0;
                   },
                   endContainer:node,endOffset:0
               }); } catch(error) {
                   if(error.name!=='RangeError') throw error;
               }
               [created,refused].join('|')"#,
        )
        .expect("standalone constructor admission script");
    let (created, refused) = result.split_once('|').expect("two counters");
    let created: usize = created.parse().expect("created count");
    let refused: usize = refused.parse().expect("refused count");
    assert!(created > 0 && created < crate::world::HARD_WRAPPER_MATERIALIZATIONS);
    assert!(refused > 0);
    assert_eq!(created + refused, 10000);
    assert_eq!(
        engine.host_record_count(),
        records_before + created,
        "a refused EventTarget constructor left a native record"
    );

    // Emptied in place rather than replaced: a concurrent optimizing job for
    // the hot getter above can hold the array itself across a low-memory GC
    // (see `nested_event_constructors_refuse_before_publishing_native_records`).
    engine.evaluate("retainedTargets.length=0").unwrap();
    engine.low_memory_gc();
    engine.reclaim_dead_hosts();
    assert_eq!(engine.host_record_count(), records_before);
    assert_eq!(engine.native_failure(), None);
}
