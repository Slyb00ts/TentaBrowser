//! Exact native identities must not repeat when a packed generation is exhausted.
//!
//! These are native-host regressions: no engine timing, forced generation field,
//! or fabricated future key is used. Every reuse goes through the real slabs.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::Document;
use tenta_js::bindings::host::{
    HostContext, HostObject, HostRecord, SharedContext, decode, encode_node, resolve_node_handle,
};
use tenta_js::bindings::{DocumentOps, NodeListOps, NodeOps};
use tenta_js::dom::events::{EventKind, EventRecord, EventSlab};
use tenta_js::event_loop::{Clock, MonoTime, SharedClock};
use tenta_js::storage::PageStorage;

fn context() -> SharedContext {
    HostContext::new(
        Rc::new(RefCell::new(Document::new())),
        SharedClock::new(Clock::virtual_at(MonoTime::ZERO)),
        tenta_net::CookieJar::new(),
        PageStorage::isolated(),
    )
}

fn document_host(context: &SharedContext) -> HostObject {
    let reference = {
        let document = context.doc.borrow();
        document.node_ref(document.document_node())
    };
    HostObject::node(context.clone(), reference)
}

#[test]
fn implementation_cache_cannot_resolve_to_an_unrelated_host_after_generation_exhaustion() {
    let context = context();
    let document = document_host(&context);
    let first = decode(document.implementation().unwrap().bits()).host_key();
    assert!(context.reclaim_host(first).keys.contains(&first));

    let mut exhausted = first;
    for _ in 1..=u16::MAX {
        let key = context
            .host_records
            .borrow_mut()
            .insert(HostRecord::TextEncoder);
        assert!(context.reclaim_host(key).keys.contains(&key));
        exhausted = key;
    }
    assert!(context.reclaim_host(exhausted).keys.is_empty());
    let unrelated = context
        .host_records
        .borrow_mut()
        .insert(HostRecord::TextEncoder);
    let replacement = decode(document.implementation().unwrap().bits()).host_key();
    assert!(
        matches!(
            context.host_records.borrow().get(replacement),
            Some(HostRecord::Implementation)
        ),
        "document.implementation must never adopt a different host class through its stale cache"
    );
    assert_ne!(replacement, unrelated);
    assert!(matches!(
        context.host_records.borrow().get(unrelated),
        Some(HostRecord::TextEncoder)
    ));
}

#[test]
fn attr_empty_child_list_cache_cannot_adopt_an_unrelated_nonempty_snapshot() {
    let context = context();
    let document = document_host(&context);
    let attr_key = decode(document.create_attribute("probe").unwrap().bits()).host_key();
    let attr = HostObject::host_record(context.clone(), attr_key);
    let first = decode(attr.child_nodes().unwrap().bits()).snapshot_key();
    context.snapshots.borrow_mut().remove(first);
    let mut exhausted = first;
    for _ in 1..=u16::MAX {
        let key = context.snapshots.borrow_mut().insert(Vec::new()).unwrap();
        context.snapshots.borrow_mut().remove(key);
        exhausted = key;
    }
    context.snapshots.borrow_mut().remove(exhausted);
    let document_reference = {
        let document = context.doc.borrow();
        document.node_ref(document.document_node())
    };
    let unrelated = context
        .snapshots
        .borrow_mut()
        .insert(vec![document_reference])
        .unwrap();
    let replacement = decode(attr.child_nodes().unwrap().bits()).snapshot_key();
    let list = HostObject::static_node_list(context.clone(), replacement);
    assert_eq!(
        NodeListOps::length(&list),
        0,
        "an Attr has no children even after its cached snapshot slot is exhausted"
    );
    assert_ne!(replacement, unrelated);
    assert_eq!(
        context.snapshots.borrow().get(unrelated),
        Some([document_reference].as_slice())
    );
}

#[test]
fn old_event_key_cannot_read_or_remove_a_later_event_after_generation_exhaustion() {
    let mut events = EventSlab::new();
    let first = events.insert(EventRecord::new("first".to_owned(), EventKind::Plain, 0.0));
    assert!(events.remove(first).is_some());
    let mut exhausted = first;
    for _ in 1..=u16::MAX {
        let key = events.insert(EventRecord::new("reuse".to_owned(), EventKind::Plain, 0.0));
        assert!(events.remove(key).is_some());
        exhausted = key;
    }
    assert!(events.remove(exhausted).is_none());
    assert!(events.is_empty());
    let current = events.insert(EventRecord::new(
        "current".to_owned(),
        EventKind::Plain,
        0.0,
    ));
    assert!(
        events.get(first).is_none(),
        "an old Event key must stay stale"
    );
    assert!(events.get_mut(first).is_none());
    assert!(events.remove(first).is_none());
    assert_eq!(events.len(), 1, "retired tombstones are not live Events");
    assert!(events.get(current).is_some());
    assert!(events.remove(current).is_some());
    assert!(events.remove(current).is_none());
    assert!(events.is_empty());
}

#[test]
fn old_packed_node_handle_stays_stale_when_its_sixteen_bit_generation_is_exhausted() {
    let context = context();
    let first = {
        let mut document = context.doc.borrow_mut();
        let node = document.create_text("first");
        let reference = document.node_ref(node);
        assert_eq!(document.sweep_detached().nodes_freed, 1);
        reference
    };
    let old_bits = encode_node(context.slot, first);
    for _ in 1..=u16::MAX {
        let mut document = context.doc.borrow_mut();
        document.create_text("reuse");
        assert_eq!(document.sweep_detached().nodes_freed, 1);
    }
    assert_eq!(context.doc.borrow_mut().sweep_detached().nodes_freed, 0);
    assert!(!context.doc.borrow().is_live(first.id));
    let current = {
        let mut document = context.doc.borrow_mut();
        let node = document.create_text("current");
        document.node_ref(node)
    };
    assert!(context.doc.borrow().resolve(first).is_err());
    assert!(
        resolve_node_handle(&context, old_bits).is_err(),
        "packed handles must reject the same stale generation as native NodeRef"
    );
    assert!(resolve_node_handle(&context, encode_node(context.slot, current)).is_ok());
}
