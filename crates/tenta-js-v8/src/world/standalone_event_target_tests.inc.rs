// Include inside world.rs::tests. A post-host publication failure may reclaim
// only this constructor's exact EventTarget record, never a reminted record.

#[test]
fn failed_standalone_target_adoption_reclaims_its_exact_host_record() {
    use tenta_js::bindings::EventTargetConstructor;

    let mut engine = materialization_engine();
    let (state, root) = {
        let dom = engine.dom.as_ref().unwrap();
        (Rc::clone(&dom.state), dom.ctx.clone())
    };
    let child = root
        .new_child(Rc::new(RefCell::new(tenta_dom::Document::new())))
        .expect("child context");
    let realm: Realm = child.clone();
    let host = <HostObject as EventTargetConstructor>::construct(&realm).unwrap();
    let key = decode(host.handle_bits()).host_key();
    assert_eq!(child.host_records.borrow().len(), 1);
    assert!(child.host_records.borrow().get(key).is_some());

    // The constructed owner remains valid, but its document slot cannot be
    // resolved when the generated constructor adopts the new wrapper.
    root.documents.clear_minted();
    let world = V8World::new(state, root);
    {
        let _entry = engine.entered();
        v8::scope!(let handle_scope, &mut engine.isolate);
        let context = v8::Local::new(handle_scope, &engine.context);
        let mut context_scope = v8::ContextScope::new(handle_scope, context);
        v8::scope!(let scope, &mut context_scope);
        let this = v8::Object::new(scope);
        assert!(
            world
                .adopt(scope, "EventTarget", this, Box::new(host))
                .is_err()
        );
    }
    assert!(child.host_records.borrow().get(key).is_none());
    assert_eq!(child.host_records.borrow().len(), 0);
    assert_eq!(engine.native_failure(), None);
}
