// Include inside world.rs::tests after the generic materialization tests.
// These controls observe native records before Engine::entry's task-end GC.

#[test]
fn standalone_listener_cycles_bound_native_records_inside_one_task() {
    let mut engine = crate::Engine::new().unwrap();
    engine.evaluate("0").unwrap();
    engine.runtime.set_gc_threshold(usize::MAX);
    let world = engine.state.world.clone();
    let records_before = world.host.host_records.borrow().len();
    let listeners_before = world.host.listeners.borrow().len();
    let values_before = world.values.borrow().len();
    let epoch_before = world.gc_epoch.get();

    let observed = engine.context.with(|ctx| {
        let _active = crate::host::ActiveContext::new(&world, &ctx);
        ctx.eval::<(), _>(
            r#"for(let i=0;i<12000;i++){
                const target=new EventTarget();
                target.note=i;
                target.self=target;
                target.addEventListener('x',()=>target.note);
            }
            void 0;"#,
        )
        .unwrap();
        (
            world.host.host_records.borrow().len(),
            world.host.listeners.borrow().len(),
            world.values.borrow().len(),
        )
    });
    assert_eq!(engine.native_failure(), None);
    assert!(
        world.gc_epoch.get() > epoch_before,
        "standalone listener cycles need an in-task pressure checkpoint"
    );
    assert!(
        observed.0 < records_before + WRAPPER_DEFERRED_LIMIT,
        "one standalone record per wrapper must remain bounded"
    );
    assert!(
        observed.1 < listeners_before + WRAPPER_DEFERRED_LIMIT,
        "listener registrations must be drained with unreachable targets"
    );
    assert!(
        observed.2 < values_before + WRAPPER_DEFERRED_LIMIT,
        "callback values must be released with unreachable targets"
    );

    engine.trim_memory();
    assert_eq!(world.host.host_records.borrow().len(), records_before);
    assert_eq!(world.host.listeners.borrow().len(), listeners_before);
    assert_eq!(world.values.borrow().len(), values_before);
}

#[test]
fn held_event_keeps_one_standalone_target_record_until_its_edge_is_reset() {
    let mut engine = crate::Engine::new().unwrap();
    let world = engine.state.world.clone();
    let records_before = world.host.host_records.borrow().len();

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
    engine.trim_memory();
    assert_eq!(world.host.host_records.borrow().len(), records_before + 1);
    assert_eq!(engine.evaluate("savedEvent.target.note").unwrap(), "41");

    engine
        .evaluate("savedEvent.initEvent('reset',false,false)")
        .unwrap();
    engine.trim_memory();
    assert_eq!(world.host.host_records.borrow().len(), records_before);
    assert_eq!(engine.native_failure(), None);
}

#[test]
fn refused_standalone_constructors_leave_exactly_the_published_records() {
    let mut engine = crate::Engine::new().unwrap();
    let world = engine.state.world.clone();
    let records_before = world.host.host_records.borrow().len();
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
    assert!(created > 0 && created < WRAPPER_DEFERRED_LIMIT);
    assert!(refused > 0);
    assert_eq!(created + refused, 10000);
    assert_eq!(
        world.host.host_records.borrow().len(),
        records_before + created,
        "a refused EventTarget constructor left a native record"
    );

    engine.evaluate("retainedTargets=[]").unwrap();
    engine.trim_memory();
    assert_eq!(world.host.host_records.borrow().len(), records_before);
    assert_eq!(engine.native_failure(), None);
}

#[test]
fn failed_standalone_target_adoption_reclaims_its_exact_host_record() {
    use tenta_js::bindings::EventTargetConstructor;

    let engine = crate::Engine::new().unwrap();
    let world = engine.state.world.clone();
    let root = world.host.clone();
    let child = root
        .new_child(Rc::new(RefCell::new(tenta_dom::Document::new())))
        .expect("child context");
    let realm: Realm = child.clone();
    let host = <HostObject as EventTargetConstructor>::construct(&realm).unwrap();
    let key = decode(host.handle_bits()).host_key();
    assert_eq!(child.host_records.borrow().len(), 1);
    assert!(child.host_records.borrow().get(key).is_some());

    root.documents.clear_minted();
    engine.context.with(|ctx| {
        let _active = crate::host::ActiveContext::new(&world, &ctx);
        assert!(world.adopt(&ctx, "EventTarget", Box::new(host)).is_none());
    });
    assert!(child.host_records.borrow().get(key).is_none());
    assert_eq!(child.host_records.borrow().len(), 0);
    assert_eq!(world.native_failure.get(), None);
}
