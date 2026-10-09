use tenta_js::dom::events::TargetKey;

#[test]
fn listener_method_lease_preserves_exact_interned_callback_multiplicity() {
    let mut engine =
        crate::Engine::with_document(Rc::new(RefCell::new(tenta_dom::Document::new())));
    let (state, host) = {
        let dom = engine.dom.as_ref().expect("DOM world");
        (Rc::clone(&dom.state), dom.ctx.clone())
    };
    engine
        .evaluate(
            "globalThis.leaseCallback=()=>{};globalThis.leaseMarker={};\
             window.addEventListener('lease-private',leaseCallback);",
        )
        .unwrap();
    let bits = host
        .listeners
        .borrow()
        .snapshot_at(TargetKey::Window, "lease-private", false)[0]
        .callback
        .0;
    let refs = || {
        state
            .borrow()
            .values
            .get(bits as usize)
            .and_then(Option::as_ref)
            .map(|slot| slot.refs)
    };
    assert_eq!(refs(), Some(1));

    engine
        .evaluate("document.addEventListener('lease-private',leaseCallback)")
        .unwrap();
    let document_node = {
        let doc = host.doc.borrow();
        doc.node_ref(doc.document_node())
    };
    assert_eq!(
        host.listeners
            .borrow()
            .snapshot_at(TargetKey::Node(document_node), "lease-private", false)[0]
            .callback
            .0,
        bits,
    );
    assert_eq!(refs(), Some(2));
    for script in [
        "(()=>{try{document.addEventListener('lease-private',leaseCallback,\
         {get capture(){throw leaseMarker}})}catch(error){return error===leaseMarker}})()",
        "(()=>{try{window.removeEventListener('lease-private',leaseCallback,\
         {get capture(){throw leaseMarker}})}catch(error){return error===leaseMarker}})()",
    ] {
        assert_eq!(engine.evaluate(script).unwrap(), "true");
        assert_eq!(refs(), Some(2));
    }
    engine
        .evaluate("window.addEventListener('lease-private',leaseCallback)")
        .unwrap();
    assert_eq!(
        refs(),
        Some(2),
        "duplicate add releases its acquired reference"
    );
    engine
        .evaluate("document.removeEventListener('lease-private',leaseCallback)")
        .unwrap();
    assert_eq!(refs(), Some(1));
    engine
        .evaluate("window.removeEventListener('lease-private',leaseCallback)")
        .unwrap();
    assert_eq!(refs(), None);

    assert_eq!(
        engine
            .evaluate(
                "(()=>{try{document.addEventListener('lease-nested',leaseCallback,\
                 {get capture(){window.addEventListener('lease-nested',leaseCallback);\
                 throw leaseMarker}})}catch(error){return error===leaseMarker}})()"
            )
            .unwrap(),
        "true"
    );
    assert!(
        host.listeners
            .borrow()
            .snapshot_at(TargetKey::Node(document_node), "lease-nested", false)
            .is_empty()
    );
    let nested_bits = host
        .listeners
        .borrow()
        .snapshot_at(TargetKey::Window, "lease-nested", false)[0]
        .callback
        .0;
    assert_eq!(
        state.borrow().values[nested_bits as usize]
            .as_ref()
            .map(|slot| slot.refs),
        Some(1),
        "the inner call transfers one reference; the outer call releases its own"
    );
    engine
        .evaluate("window.removeEventListener('lease-nested',leaseCallback)")
        .unwrap();
    assert!(state.borrow().values[nested_bits as usize].is_none());
    assert_eq!(engine.native_failure(), None);
}

#[test]
fn stored_listener_value_cleanup_panic_is_sticky_and_retains_the_root() {
    let mut engine =
        crate::Engine::with_document(Rc::new(RefCell::new(tenta_dom::Document::new())));
    let state = Rc::clone(&engine.dom.as_ref().expect("DOM world").state);
    {
        let _entry = engine.entered();
        v8::scope!(let handle_scope, &mut engine.isolate);
        let context = v8::Local::new(handle_scope, &engine.context);
        let mut context_scope = v8::ContextScope::new(handle_scope, context);
        v8::scope!(let scope, &mut context_scope);
        let callback: v8::Local<v8::Value> = v8::Object::new(scope).into();
        tenta_js::bindings::v8_callback_boundary(scope, "listener value cleanup", (), |scope| {
            tenta_js::bindings::v8_begin_stored_value_lease(scope);
            let _bits = tenta_js::bindings::value_from_v8(scope, callback).unwrap();
            let _conflict = state.borrow_mut();
            tenta_js::bindings::v8_end_stored_value_lease(scope);
        });
    }
    assert_eq!(engine.native_failure(), Some("listener value cleanup"));
    assert_eq!(state.borrow().value_count(), 1);
    assert!(engine.evaluate("1 + 1").is_err());
    drop(engine);
    assert_eq!(state.borrow().value_count(), 0);
}
