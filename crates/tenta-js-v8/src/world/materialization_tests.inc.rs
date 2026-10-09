use crate::Engine;
use tenta_js::bindings::AbortControllerConstructor;
use tenta_js::bindings::host::{encode_event, encode_node};
use tenta_js::dom::events::{EventKind, EventRecord};

fn materialization_engine() -> Engine {
    Engine::with_document(Rc::new(RefCell::new(tenta_dom::Document::new())))
}

#[test]
fn missing_template_does_not_publish_event_linked_host_or_node_wrapper_flags() {
    let mut engine = materialization_engine();
    let (state, ctx) = {
        let dom = engine.dom.as_ref().unwrap();
        (Rc::clone(&dom.state), dom.ctx.clone())
    };
    let event_key = ctx.events.borrow_mut().insert(EventRecord::new(
        "transient".to_owned(),
        EventKind::Plain,
        0.0,
    ));
    let realm: Realm = ctx.clone();
    let controller = <HostObject as AbortControllerConstructor>::construct(&realm).unwrap();
    let controller_key = decode(controller.handle_bits()).host_key();
    let signal_key = match ctx.host_records.borrow().get(controller_key) {
        Some(HostRecord::Controller(record)) => record.signal,
        _ => panic!("constructed controller has a signal"),
    };
    let node_id = ctx.doc.borrow_mut().create_html_element(atoms::DIV);
    let node = ctx.doc.borrow().node_ref(node_id);
    let event_bits = encode_event(ctx.slot, event_key);
    let signal_bits = encode_host(ctx.slot, signal_key);
    let node_bits = encode_node(ctx.slot, node);
    let world = V8World::new(state.clone(), ctx.clone());
    {
        let _entry = engine.entered();
        v8::scope!(let handle_scope, &mut engine.isolate);
        let context = v8::Local::new(handle_scope, &engine.context);
        let mut context_scope = v8::ContextScope::new(handle_scope, context);
        v8::scope!(let scope, &mut context_scope);
        let templates = scope
            .remove_slot::<tenta_js::bindings::TemplateCache>()
            .expect("installed Web IDL templates");
        let event = world.wrapper(scope, "Event", event_bits);
        let signal = world.wrapper(scope, "AbortSignal", signal_bits);
        let element = world.wrapper(scope, "HTMLElement", node_bits);
        scope.set_slot(templates);
        assert!(event.unwrap().is_none());
        assert!(signal.unwrap().is_none());
        assert!(element.unwrap().is_none());
    }

    // There was no published Weak wrapper to enqueue dead bookkeeping. A
    // transient event can be removed immediately, a controller can release
    // its signal, and the detached node can be swept.
    assert!(!ctx.events.borrow().get(event_key).unwrap().has_wrapper);
    assert!(ctx.events.borrow_mut().remove(event_key).is_some());
    assert!(matches!(
        ctx.host_records.borrow().get(signal_key),
        Some(HostRecord::Signal(record)) if !record.has_wrapper
    ));
    drop(controller);
    let _ = ctx.reclaim_host(controller_key);
    assert!(ctx.host_records.borrow().get(signal_key).is_none());
    assert!(!ctx.doc.borrow().has_wrapper(node_id));
    ctx.doc.borrow_mut().sweep_detached();
    assert!(!ctx.doc.borrow().is_live(node_id));
    for bits in [event_bits, signal_bits, node_bits] {
        assert!(!state.borrow().wrappers.contains_key(&bits));
        assert!(!state.borrow().hosts.contains_key(&bits));
    }
    assert_eq!(engine.native_failure(), None);
}

#[test]
fn missing_template_preserves_another_worlds_node_claim() {
    let document = Rc::new(RefCell::new(tenta_dom::Document::new()));
    let mut engine = Engine::with_document(document.clone());
    let mut other = Engine::with_document(document.clone());
    other
        .evaluate(
            "globalThis.kept = document.appendChild(document.createElement('div')); \
             kept.note = 'foreign'",
        )
        .unwrap();
    let node = {
        let doc = document.borrow();
        doc.node_ref(doc.document_element().unwrap())
    };
    let (state, ctx) = {
        let dom = engine.dom.as_ref().unwrap();
        (dom.state.clone(), dom.ctx.clone())
    };
    let bits = encode_node(ctx.slot, node);
    let world = V8World::new(state, ctx);
    {
        let _entry = engine.entered();
        v8::scope!(let handle_scope, &mut engine.isolate);
        let context = v8::Local::new(handle_scope, &engine.context);
        let mut context_scope = v8::ContextScope::new(handle_scope, context);
        v8::scope!(let scope, &mut context_scope);
        let templates = scope
            .remove_slot::<tenta_js::bindings::TemplateCache>()
            .unwrap();
        assert!(world.wrapper(scope, "HTMLElement", bits).unwrap().is_none());
        scope.set_slot(templates);
    }
    assert_eq!(document.borrow().wrapper_count(node.id), 1);
    other.evaluate("document.removeChild(kept)").unwrap();
    drop(engine);
    assert_eq!(document.borrow_mut().sweep_detached().nodes_freed, 0);
    assert_eq!(other.evaluate("kept.note").unwrap(), "foreign");
    drop(other);
    assert_eq!(document.borrow_mut().sweep_detached().nodes_freed, 1);
}

#[test]
fn failed_event_adoption_uses_the_constructed_owner_and_releases_detail() {
    let mut engine = materialization_engine();
    let (state, root) = {
        let dom = engine.dom.as_ref().unwrap();
        (Rc::clone(&dom.state), dom.ctx.clone())
    };
    let child = root
        .new_child(Rc::new(RefCell::new(tenta_dom::Document::new())))
        .expect("child context");
    {
        let _entry = engine.entered();
        v8::scope!(let handle_scope, &mut engine.isolate);
        let context = v8::Local::new(handle_scope, &engine.context);
        let mut context_scope = v8::ContextScope::new(handle_scope, context);
        v8::scope!(let scope, &mut context_scope);
        let marker = v8::Object::new(scope);
        let detail = state.borrow_mut().store_value(scope, marker.into());
        let key = child.events.borrow_mut().insert(EventRecord::new(
            "made".to_owned(),
            EventKind::Custom {
                detail: Some(detail),
            },
            0.0,
        ));
        assert_eq!(state.borrow().value_count(), 1);
        // The record's owner survives, but its lookup slot is no longer published.
        // The failed adopt must reclaim through HostObject's owner, not through
        // DocumentSet::get, and must release the detail's V8 root once.
        root.documents.clear_minted();
        let world = V8World::new(Rc::clone(&state), root);
        let this = v8::Object::new(scope);
        let result = world.adopt(
            scope,
            "CustomEvent",
            this,
            Box::new(HostObject::event(child.clone(), key)),
        );
        assert!(result.is_err());
        assert!(child.events.borrow().get(key).is_none());
        assert_eq!(state.borrow().value_count(), 0);
    }
    assert_eq!(engine.native_failure(), None);
}

#[test]
fn failed_mouse_adoption_releases_only_its_related_target_reference() {
    let mut engine = materialization_engine();
    let (state, root) = {
        let dom = engine.dom.as_ref().unwrap();
        (Rc::clone(&dom.state), dom.ctx.clone())
    };
    let child = root
        .new_child(Rc::new(RefCell::new(tenta_dom::Document::new())))
        .expect("child context");
    {
        let _entry = engine.entered();
        v8::scope!(let handle_scope, &mut engine.isolate);
        let context = v8::Local::new(handle_scope, &engine.context);
        let mut context_scope = v8::ContextScope::new(handle_scope, context);
        v8::scope!(let scope, &mut context_scope);
        let marker = v8::Object::new(scope);
        let related_value = state.borrow_mut().store_value(scope, marker.into());
        assert_eq!(
            state.borrow_mut().store_value(scope, marker.into()),
            related_value
        );
        let key = child.events.borrow_mut().insert(EventRecord::new(
            "move".to_owned(),
            EventKind::Mouse {
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
        assert_eq!(
            state.borrow().values[related_value as usize]
                .as_ref()
                .unwrap()
                .refs,
            2
        );
        root.documents.clear_minted();
        let world = V8World::new(Rc::clone(&state), root);
        let this = v8::Object::new(scope);
        assert!(
            world
                .adopt(
                    scope,
                    "MouseEvent",
                    this,
                    Box::new(HostObject::event(child.clone(), key))
                )
                .is_err()
        );
        assert!(child.events.borrow().get(key).is_none());
        assert_eq!(
            state.borrow().values[related_value as usize]
                .as_ref()
                .unwrap()
                .refs,
            1
        );
        state.borrow_mut().release_value(related_value);
        assert_eq!(state.borrow().value_count(), 0);
    }
    assert_eq!(engine.native_failure(), None);
}

#[test]
fn failed_host_adoption_reclaims_its_linked_signal_and_value() {
    let mut engine = materialization_engine();
    let (state, root) = {
        let dom = engine.dom.as_ref().unwrap();
        (Rc::clone(&dom.state), dom.ctx.clone())
    };
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
    {
        let _entry = engine.entered();
        v8::scope!(let handle_scope, &mut engine.isolate);
        let context = v8::Local::new(handle_scope, &engine.context);
        let mut context_scope = v8::ContextScope::new(handle_scope, context);
        v8::scope!(let scope, &mut context_scope);
        let marker = v8::Object::new(scope);
        let reason = state.borrow_mut().store_value(scope, marker.into());
        if let Some(HostRecord::Signal(record)) = child.host_records.borrow_mut().get_mut(signal) {
            record.reason = Some(reason);
        }
        assert_eq!(child.host_records.borrow().len(), 2);
        assert_eq!(state.borrow().value_count(), 1);
        root.documents.clear_minted();
        let world = V8World::new(Rc::clone(&state), root);
        let this = v8::Object::new(scope);
        assert!(
            world
                .adopt(scope, "AbortController", this, Box::new(host))
                .is_err()
        );
        assert_eq!(child.host_records.borrow().len(), 0);
        assert_eq!(state.borrow().value_count(), 0);
    }
    assert_eq!(engine.native_failure(), None);
}

#[test]
fn refused_cached_record_rewrap_preserves_its_native_record() {
    let mut engine = materialization_engine();
    let (state, ctx) = {
        let dom = engine.dom.as_ref().unwrap();
        (Rc::clone(&dom.state), dom.ctx.clone())
    };
    let key = ctx
        .host_records
        .borrow_mut()
        .insert(HostRecord::TextEncoder);
    let bits = encode_host(ctx.slot, key);
    let world = V8World::new(Rc::clone(&state), ctx.clone());
    state.borrow_mut().new_wrapper_materializations = HARD_WRAPPER_MATERIALIZATIONS;
    {
        let _entry = engine.entered();
        v8::scope!(let handle_scope, &mut engine.isolate);
        let context = v8::Local::new(handle_scope, &engine.context);
        let mut context_scope = v8::ContextScope::new(handle_scope, context);
        v8::scope!(let scope, &mut context_scope);
        assert!(world.wrapper(scope, "TextEncoder", bits).is_err());
        assert!(ctx.host_records.borrow().get(key).is_some());
        state.borrow_mut().new_wrapper_materializations = 0;
        assert!(world.wrapper(scope, "TextEncoder", bits).unwrap().is_some());
        assert!(ctx.host_records.borrow().get(key).is_some());
    }
    assert_eq!(engine.native_failure(), None);
}

#[test]
fn failed_node_adoption_does_not_reclaim_the_document_arena() {
    let mut engine = materialization_engine();
    let (state, root) = {
        let dom = engine.dom.as_ref().unwrap();
        (Rc::clone(&dom.state), dom.ctx.clone())
    };
    let child = root
        .new_child(Rc::new(RefCell::new(tenta_dom::Document::new())))
        .expect("child context");
    let id = child.doc.borrow().document_node();
    let node = child.doc.borrow().node_ref(id);
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
                .adopt(
                    scope,
                    "Document",
                    this,
                    Box::new(HostObject::node(child.clone(), node)),
                )
                .is_err()
        );
        assert!(child.doc.borrow().is_live(id));
    }
    assert_eq!(engine.native_failure(), None);
}
