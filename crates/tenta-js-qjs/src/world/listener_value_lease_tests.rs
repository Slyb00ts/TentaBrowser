use tenta_js::dom::events::TargetKey;

#[test]
fn listener_method_lease_preserves_exact_interned_callback_multiplicity() {
    let mut engine = crate::Engine::new().unwrap();
    let world = engine.state.world.clone();
    let host = world.host.clone();
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
    let refs = || world.values.borrow().get(&bits).map(|slot| slot.refs);
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
        world
            .values
            .borrow()
            .get(&nested_bits)
            .map(|slot| slot.refs),
        Some(1),
        "the inner call transfers one reference; the outer call releases its own"
    );
    engine
        .evaluate("window.removeEventListener('lease-nested',leaseCallback)")
        .unwrap();
    assert!(!world.values.borrow().contains_key(&nested_bits));
    assert_eq!(engine.native_failure(), None);
}
