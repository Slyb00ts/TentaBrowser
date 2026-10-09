// Sample native records before the Engine entry epilogue can collect them.
#[test]
fn uninterrupted_event_materialization_bounds_native_records() {
    let mut engine = crate::Engine::new().unwrap();
    engine.evaluate("0").unwrap();
    engine.runtime.set_gc_threshold(usize::MAX);
    let world = engine.state.world.clone();
    let before = world.host.events.borrow().len();
    let epoch_before = world.gc_epoch.get();
    let observed = engine.context.with(|ctx| {
        let _active = crate::host::ActiveContext::new(&world, &ctx);
        ctx.eval::<(), _>(
            "for(let i=0;i<12000;i++){const event=new Event('pressure');event.self=event;}void 0;",
        )
        .unwrap();
        world.host.events.borrow().len()
    });
    assert_eq!(engine.native_failure(), None);
    assert!(
        observed < before + WRAPPER_DEFERRED_LIMIT,
        "uninterrupted Event construction retained {observed} native records"
    );
    assert!(
        world.gc_epoch.get() > epoch_before,
        "Event-only cycles must reach a pressure checkpoint inside the task"
    );
    engine.trim_memory();
    assert_eq!(world.host.events.borrow().len(), before);
}

#[test]
fn uninterrupted_non_css_materialization_bounds_native_host_records() {
    let mut engine = crate::Engine::new().unwrap();
    engine.evaluate("0").unwrap();
    engine.runtime.set_gc_threshold(usize::MAX);
    let world = engine.state.world.clone();
    let before = world.host.host_records.borrow().len();
    let observed = engine.context.with(|ctx| {
        let _active = crate::host::ActiveContext::new(&world, &ctx);
        ctx.eval::<(), _>(
            "for(let i=0;i<12000;i++){const encoder=new TextEncoder();encoder.self=encoder;}void 0;",
        )
        .unwrap();
        world.host.host_records.borrow().len()
    });
    assert_eq!(engine.native_failure(), None);
    assert!(
        observed < before + WRAPPER_DEFERRED_LIMIT,
        "uninterrupted TextEncoder construction retained {observed} native host records"
    );
    engine.trim_memory();
    assert_eq!(world.host.host_records.borrow().len(), before);
}

#[test]
fn nested_event_constructors_refuse_before_publishing_native_records() {
    let mut engine = crate::Engine::new().unwrap();
    let world = engine.state.world.clone();
    let before = world.host.events.borrow().len();
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
    assert!(created > 0 && created < WRAPPER_DEFERRED_LIMIT);
    assert!(refused > 0);
    assert_eq!(created + refused, 10000);
    assert_eq!(world.host.events.borrow().len(), before + created);
    engine.evaluate("retainedEvents=undefined").unwrap();
    engine.trim_memory();
    assert_eq!(world.host.events.borrow().len(), before);
    assert_eq!(engine.native_failure(), None);
}

#[test]
fn rejected_custom_event_releases_converted_detail_reference() {
    let mut engine = crate::Engine::new().unwrap();
    let world = engine.state.world.clone();
    let events_before = world.host.events.borrow().len();
    let values_before = world.values.borrow().len();
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
        .expect("CustomEvent conversion admission script");
    let mut fields = result.split('|');
    let created: usize = fields.next().unwrap().parse().unwrap();
    let refused: usize = fields.next().unwrap().parse().unwrap();
    assert_eq!(fields.next(), Some("true"));
    assert!(created > 0 && created < WRAPPER_DEFERRED_LIMIT);
    assert_eq!(created + refused, 10000);
    assert_eq!(world.host.events.borrow().len(), events_before + created);
    assert_eq!(world.values.borrow().len(), values_before);
    engine.evaluate("retainedEvents=undefined").unwrap();
    engine.trim_memory();
    assert_eq!(world.host.events.borrow().len(), events_before);
    assert_eq!(engine.native_failure(), None);
}

#[test]
fn nested_custom_events_transfer_each_reused_detail_reference() {
    let mut engine = crate::Engine::new().unwrap();
    let world = engine.state.world.clone();
    let events_before = world.host.events.borrow().len();
    let values_before = world.values.borrow().len();
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
    assert_eq!(world.host.events.borrow().len(), events_before + 3);
    assert_eq!(world.values.borrow().len(), values_before + 1);
    engine
        .evaluate("retainedEvents.splice(0,2);marker=undefined")
        .unwrap();
    engine.trim_memory();
    assert_eq!(world.host.events.borrow().len(), events_before + 1);
    assert_eq!(world.values.borrow().len(), values_before + 1);
    assert_eq!(
        engine.evaluate("retainedEvents[0].detail.note").unwrap(),
        "alive"
    );
    engine.evaluate("retainedEvents=[]").unwrap();
    engine.trim_memory();
    assert_eq!(world.host.events.borrow().len(), events_before);
    assert_eq!(world.values.borrow().len(), values_before);
    assert_eq!(engine.native_failure(), None);
}

#[test]
fn failed_event_adoption_uses_constructed_owner_and_releases_detail() {
    let engine = crate::Engine::new().unwrap();
    let world = engine.state.world.clone();
    let root = world.host.clone();
    let child = root
        .new_child(Rc::new(RefCell::new(tenta_dom::Document::new())))
        .expect("child context");
    engine.context.with(|ctx| {
        let _active = crate::host::ActiveContext::new(&world, &ctx);
        let marker = Object::new(ctx.clone()).unwrap();
        let detail = world.store_value(&ctx, marker.into_value());
        let key = child
            .events
            .borrow_mut()
            .insert(tenta_js::dom::events::EventRecord::new(
                "made".to_owned(),
                tenta_js::dom::events::EventKind::Custom {
                    detail: Some(detail),
                },
                0.0,
            ));
        assert_eq!(world.values.borrow().len(), 1);
        root.documents.clear_minted();
        assert!(
            world
                .adopt(
                    &ctx,
                    "CustomEvent",
                    Box::new(HostObject::event(child.clone(), key))
                )
                .is_none()
        );
        assert!(child.events.borrow().get(key).is_none());
        assert_eq!(world.values.borrow().len(), 0);
        assert_eq!(world.native_failure.get(), None);
    });
}

#[test]
fn failed_mouse_adoption_releases_only_its_related_target_reference() {
    let engine = crate::Engine::new().unwrap();
    let world = engine.state.world.clone();
    let root = world.host.clone();
    let child = root
        .new_child(Rc::new(RefCell::new(tenta_dom::Document::new())))
        .expect("child context");
    engine.context.with(|ctx| {
        let _active = crate::host::ActiveContext::new(&world, &ctx);
        let marker = Object::new(ctx.clone()).unwrap();
        let related_value = world.store_value(&ctx, marker.clone().into_value());
        assert_eq!(world.store_value(&ctx, marker.into_value()), related_value);
        let key = child
            .events
            .borrow_mut()
            .insert(tenta_js::dom::events::EventRecord::new(
                "move".to_owned(),
                tenta_js::dom::events::EventKind::Mouse {
                    view: None,
                    detail: 0,
                    which: 0,
                    screen_x: 0,
                    screen_y: 0,
                    client_x: 0,
                    client_y: 0,
                    modifiers: 0,
                    button: 0,
                    buttons: 0,
                    related_target: None,
                    related_value: Some(related_value),
                    wheel: None,
                },
                0.0,
            ));
        assert_eq!(world.values.borrow().get(&related_value).unwrap().refs, 2);
        root.documents.clear_minted();
        assert!(
            world
                .adopt(
                    &ctx,
                    "MouseEvent",
                    Box::new(HostObject::event(child.clone(), key))
                )
                .is_none()
        );
        assert!(child.events.borrow().get(key).is_none());
        assert_eq!(world.values.borrow().get(&related_value).unwrap().refs, 1);
        world.release_stored_reference(related_value).unwrap();
        assert!(world.values.borrow().is_empty());
        assert_eq!(world.native_failure.get(), None);
    });
}

#[test]
fn failed_host_adoption_reclaims_linked_signal_and_value() {
    let engine = crate::Engine::new().unwrap();
    let world = engine.state.world.clone();
    let root = world.host.clone();
    let child = root
        .new_child(Rc::new(RefCell::new(tenta_dom::Document::new())))
        .expect("child context");
    let realm: Realm = child.clone();
    let host = <HostObject as AbortControllerConstructor>::construct(&realm).unwrap();
    let controller = decode(host.handle_bits()).host_key();
    let signal = match child.host_records.borrow().get(controller) {
        Some(HostRecord::Controller(record)) => record.signal,
        _ => panic!("constructed controller has a signal"),
    };
    engine.context.with(|ctx| {
        let _active = crate::host::ActiveContext::new(&world, &ctx);
        let marker = Object::new(ctx.clone()).unwrap();
        let reason = world.store_value(&ctx, marker.into_value());
        if let Some(HostRecord::Signal(record)) = child.host_records.borrow_mut().get_mut(signal) {
            record.reason = Some(reason);
        }
        assert_eq!(child.host_records.borrow().len(), 2);
        assert_eq!(world.values.borrow().len(), 1);
        root.documents.clear_minted();
        assert!(
            world
                .adopt(&ctx, "AbortController", Box::new(host))
                .is_none()
        );
        assert_eq!(child.host_records.borrow().len(), 0);
        assert_eq!(world.values.borrow().len(), 0);
        assert_eq!(world.native_failure.get(), None);
    });
}

#[test]
fn refused_existing_record_rewrap_preserves_native_record() {
    let engine = crate::Engine::new().unwrap();
    let world = engine.state.world.clone();
    let key = world
        .host
        .host_records
        .borrow_mut()
        .insert(HostRecord::TextEncoder);
    let bits = encode_host(world.host.slot, key);
    world
        .new_wrapper_materializations
        .set(WRAPPER_DEFERRED_LIMIT);
    engine.context.with(|ctx| {
        let _active = crate::host::ActiveContext::new(&world, &ctx);
        assert!(world.wrapper(&ctx, "TextEncoder", bits).is_none());
        assert!(world.host.host_records.borrow().get(key).is_some());
        // The failed reservation raised RangeError; clear that pending JS
        // exception before proving the same native record can be reminted.
        let _ = ctx.catch();
        world.new_wrapper_materializations.set(0);
        assert!(world.wrapper(&ctx, "TextEncoder", bits).is_some());
        assert!(world.host.host_records.borrow().get(key).is_some());
    });
    assert_eq!(world.native_failure.get(), None);
}

#[test]
fn failed_document_adoption_does_not_reclaim_arena() {
    let engine = crate::Engine::new().unwrap();
    let world = engine.state.world.clone();
    let root = world.host.clone();
    let child = root
        .new_child(Rc::new(RefCell::new(tenta_dom::Document::new())))
        .expect("child context");
    let id = child.doc.borrow().document_node();
    let node = child.doc.borrow().node_ref(id);
    root.documents.clear_minted();
    engine.context.with(|ctx| {
        let _active = crate::host::ActiveContext::new(&world, &ctx);
        assert!(
            world
                .adopt(
                    &ctx,
                    "Document",
                    Box::new(HostObject::node(child.clone(), node))
                )
                .is_none()
        );
    });
    assert!(child.doc.borrow().is_live(id));
    assert_eq!(world.native_failure.get(), None);
}

#[test]
fn stored_value_lease_cleanup_failure_retains_remaining_roots_until_teardown() {
    struct KeptHost(Rc<Cell<bool>>);
    impl Drop for KeptHost {
        fn drop(&mut self) {
            self.0.set(true);
        }
    }
    struct PanickingHost;
    impl Drop for PanickingHost {
        fn drop(&mut self) {
            panic!("stored value finalizer failure");
        }
    }
    let dropped = Rc::new(Cell::new(false));
    let mut engine = crate::Engine::new().unwrap();
    let world = engine.state.world.clone();
    let error = engine
        .entry::<()>(None, |ctx| {
            quickjs_callback_boundary(ctx, "stored value cleanup", || {
                world.begin_stored_value_lease(ctx)?;
                for host in [
                    Rc::new(KeptHost(dropped.clone())) as Rc<dyn std::any::Any>,
                    Rc::new(PanickingHost),
                ] {
                    let instance = Class::instance(
                        ctx.clone(),
                        QuickJsHost {
                            host: Some(host),
                            handle: u64::MAX,
                            generation: 0,
                            interfaces: &[],
                            dead: world.dead.clone(),
                            native_failure: world.native_failure.clone(),
                        },
                    )?;
                    let _ = value_from_quickjs(ctx, instance.into_value())?;
                }
                Ok(())
            })
        })
        .unwrap_err();
    assert_eq!(error.kind, tenta_js::engine::ScriptErrorKind::Terminated);
    assert_eq!(engine.native_failure(), Some("native host finalizer"));
    assert!(!dropped.get(), "the earlier value root must remain owned");
    assert_eq!(world.callback_roots.borrow().len(), 1);
    assert_eq!(
        engine.evaluate("1 + 1").unwrap_err().kind,
        tenta_js::engine::ScriptErrorKind::Terminated,
    );
    drop(engine);
    assert!(dropped.get(), "teardown must release retained value roots");
    assert!(world.callback_roots.borrow().is_empty());
}

#[test]
fn custom_detail_cycle_releases_its_native_event_record() {
    let mut engine = crate::Engine::new().unwrap();
    let world = engine.state.world.clone();
    let before = world.host.events.borrow().len();
    engine
        .evaluate(
            "globalThis.heldCustom=(()=>{const detail={};const event=new CustomEvent('x',{detail});detail.event=event;return event})()",
        )
        .expect("create held CustomEvent cycle");
    for _ in 0..3 {
        engine.run_microtasks();
        engine.trim_memory();
    }
    assert_eq!(world.host.events.borrow().len(), before + 1);
    engine.evaluate("globalThis.heldCustom=null").unwrap();
    for _ in 0..3 {
        engine.run_microtasks();
        engine.trim_memory();
    }
    assert_eq!(world.host.events.borrow().len(), before);
    assert_eq!(engine.native_failure(), None);
}
