use super::*;
use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::atom::atoms;

use super::super::observer_journal::{ObserverNode, RecordType, SemanticRecord};
use super::super::observer_ownership::OwnedObserverRecord;

fn budget() -> NativeMemoryBudget {
    NativeMemoryBudget::with_limits(1024 * 1024, 1024 * 1024, 1024 * 1024, 0)
}

fn attribute_record(document: &Document, element: tenta_core::NodeId) -> SemanticRecord {
    SemanticRecord {
        record_type: RecordType::Attributes,
        target: ObserverNode::new(document.document_id(), document.node_ref(element)),
        added_nodes: Vec::new(),
        removed_nodes: Vec::new(),
        previous_sibling: None,
        next_sibling: None,
        attribute_name: Some(atoms::CLASS),
        attribute_namespace: None,
        old_value: None,
    }
}

fn active_nodes(registry: &ObserverRegistry) -> Vec<Arc<ObserverRegistration>> {
    let mut nodes = Vec::new();
    let mut current = registry.active_head();
    while let Some(node) = current {
        current = ObserverRegistry::active_next(&node);
        if node.phase() == RegistrationPhase::Active {
            nodes.push(node);
        }
    }
    nodes
}

#[test]
fn last_host_alias_retires_without_borrowing_the_document() {
    let document = Rc::new(RefCell::new(Document::new()));
    let registry = ObserverRegistry::new(document.borrow().document_id());
    let agent = Arc::new(Mutex::new(ObserverAgent::new(budget())));
    let node = ObserverRegistration::root(budget());
    registry.attach(&node, Arc::downgrade(&agent)).unwrap();
    let owner = Arc::new(ObserverAttachment::new(
        Arc::clone(&registry),
        Arc::clone(&node),
    ));
    let alias = Arc::clone(&owner);
    drop(owner);
    assert_eq!(node.phase(), RegistrationPhase::Active);
    let held_document = document.borrow_mut();
    drop(alias);
    assert_eq!(node.phase(), RegistrationPhase::Retired);
    drop(held_document);
    assert_eq!(
        registry.drain_retired(&mut document.borrow_mut()).unwrap(),
        1
    );
    assert_eq!(node.phase(), RegistrationPhase::Drained);
}

#[test]
fn retiring_a_captured_head_and_middle_does_not_truncate_a_walk() {
    let mut document = Document::new();
    let budget = budget();
    let registry = ObserverRegistry::new(document.document_id());
    let agents = (0..3)
        .map(|_| Arc::new(Mutex::new(ObserverAgent::new(budget.clone()))))
        .collect::<Vec<_>>();
    let nodes = (0..3)
        .map(|_| ObserverRegistration::root(budget.clone()))
        .collect::<Vec<_>>();
    for index in (0..3).rev() {
        registry
            .attach(&nodes[index], Arc::downgrade(&agents[index]))
            .unwrap();
    }
    let captured_head = registry.active_head().unwrap();
    assert!(Arc::ptr_eq(&captured_head, &nodes[0]));
    assert!(registry.retire(&nodes[0]).unwrap());
    let captured_middle = ObserverRegistry::active_next(&captured_head).unwrap();
    assert!(Arc::ptr_eq(&captured_middle, &nodes[1]));
    assert!(registry.retire(&nodes[1]).unwrap());
    let captured_tail = ObserverRegistry::active_next(&captured_middle).unwrap();
    assert!(Arc::ptr_eq(&captured_tail, &nodes[2]));
    assert_eq!(captured_tail.phase(), RegistrationPhase::Active);
    assert!(ObserverRegistry::active_next(&captured_tail).is_none());
    assert_eq!(active_nodes(&registry).len(), 1);
    assert_eq!(registry.drain_retired(&mut document).unwrap(), 2);
    // A previously captured retired node still has the successor it observed.
    assert!(Arc::ptr_eq(
        &ObserverRegistry::active_next(&captured_middle).unwrap(),
        &nodes[2]
    ));
    registry.drain_all_on_document_drop(&mut document);
}

#[test]
fn consumer_walk_reaches_tail_when_callback_retires_the_middle_world() {
    let mut document = Document::new();
    let budget = budget();
    let registry = ObserverRegistry::new(document.document_id());
    let agents = (0..3)
        .map(|_| Arc::new(Mutex::new(ObserverAgent::new(budget.clone()))))
        .collect::<Vec<_>>();
    let nodes = (0..3)
        .map(|_| ObserverRegistration::root(budget.clone()))
        .collect::<Vec<_>>();
    for index in (0..3).rev() {
        registry
            .attach(&nodes[index], Arc::downgrade(&agents[index]))
            .unwrap();
    }
    let mut visited = Vec::new();
    registry
        .for_each_active(|node, _agent| {
            visited.push(Arc::clone(node));
            if Arc::ptr_eq(node, &nodes[0]) {
                registry.retire(&nodes[1])?;
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(visited.len(), 2);
    assert!(Arc::ptr_eq(&visited[0], &nodes[0]));
    assert!(Arc::ptr_eq(&visited[1], &nodes[2]));
    registry.drain_all_on_document_drop(&mut document);
}

#[test]
fn a_departed_middle_world_does_not_hide_later_registrations_or_release_pins_early() {
    let document = Rc::new(RefCell::new(Document::new()));
    let element = document.borrow_mut().create_html_element(atoms::DIV);
    let budget = budget();
    let registry = ObserverRegistry::new(document.borrow().document_id());
    let first_agent = Arc::new(Mutex::new(ObserverAgent::new(budget.clone())));
    let middle_agent = Arc::new(Mutex::new(ObserverAgent::new(budget.clone())));
    let last_agent = Arc::new(Mutex::new(ObserverAgent::new(budget.clone())));
    let first = ObserverRegistration::root(budget.clone());
    let middle = ObserverRegistration::root(budget.clone());
    let last = ObserverRegistration::root(budget.clone());
    registry.attach(&last, Arc::downgrade(&last_agent)).unwrap();
    registry
        .attach(&middle, Arc::downgrade(&middle_agent))
        .unwrap();
    registry
        .attach(&first, Arc::downgrade(&first_agent))
        .unwrap();
    assert!(
        registry
            .attach(&first, Arc::downgrade(&first_agent))
            .is_ok_and(|inserted| !inserted)
    );
    let before = active_nodes(&registry);
    assert_eq!(before.len(), 3);
    assert!(Arc::ptr_eq(&before[0], &first));
    assert!(Arc::ptr_eq(&before[1], &middle));
    assert!(Arc::ptr_eq(&before[2], &last));
    drop(before);

    {
        let mut doc = document.borrow_mut();
        let record = attribute_record(&doc, element);
        let lease = middle
            .with_active_pins(|pins| pins.acquire_pending_record(&mut doc, &record))
            .unwrap()
            .unwrap();
        assert!(lease.is_pending());
        assert_eq!(doc.handle_count(element), 1);
        assert_eq!(
            middle
                .with_active_pins_read(|pins| pins.owner_counts(record.target))
                .unwrap(),
            Some((1, 0))
        );
        assert_eq!(doc.sweep_detached().nodes_freed, 0);
    }

    // World death can happen while an immutable Document borrow is held.
    // The node remains in the header with its physical handle until a later
    // mutable Document seam, while the two surviving agents stay reachable.
    let held = document.borrow();
    drop(middle_agent);
    assert!(middle.agent().is_none());
    assert!(registry.retire(&middle).unwrap());
    assert!(!registry.retire(&middle).unwrap());
    assert!(first.agent().is_some());
    assert!(last.agent().is_some());
    let after = active_nodes(&registry);
    assert_eq!(after.len(), 2);
    assert!(Arc::ptr_eq(&after[0], &first));
    assert!(Arc::ptr_eq(&after[1], &last));
    drop(after);
    assert_eq!(held.handle_count(element), 1);
    drop(held);

    {
        let mut doc = document.borrow_mut();
        assert_eq!(registry.drain_retired(&mut doc).unwrap(), 1);
        assert_eq!(doc.handle_count(element), 0);
        assert_eq!(doc.sweep_detached().nodes_freed, 1);
    }
    assert_eq!(middle.phase(), RegistrationPhase::Drained);
    assert!(registry.retire(&first).unwrap());
    assert!(registry.retire(&last).unwrap());
    registry.drain_retired(&mut document.borrow_mut()).unwrap();
    drop(first_agent);
    drop(last_agent);
    drop(first);
    drop(middle);
    drop(last);
    drop(registry);
    drop(document);
    assert_eq!(budget.used(NativeMemoryComponent::MutationObserver), 0);
}

#[test]
fn root_shell_needs_no_quota_but_child_registration_refuses_before_publication() {
    let mut document = Document::new();
    let budget = NativeMemoryBudget::with_limits(0, 0, 0, 0);
    let registry = ObserverRegistry::new(document.document_id());
    let agent = Arc::new(Mutex::new(ObserverAgent::new(budget.clone())));
    let root = ObserverRegistration::root(budget.clone());
    assert!(budget.first_failure().is_none());
    assert!(registry.attach(&root, Arc::downgrade(&agent)).unwrap());
    assert!(budget.first_failure().is_none());
    assert!(matches!(
        ObserverRegistration::child(budget.clone()),
        Err(Error::BudgetExceeded { .. })
    ));
    assert_eq!(active_nodes(&registry).len(), 1);
    registry.drain_all_on_document_drop(&mut document);
    drop(root);
    drop(agent);
    drop(registry);
    assert_eq!(budget.used(NativeMemoryComponent::MutationObserver), 0);
}

#[test]
fn charged_child_node_keeps_its_reservation_through_retirement() {
    let mut document = Document::new();
    let budget = budget();
    let registry = ObserverRegistry::new(document.document_id());
    let agent = Arc::new(Mutex::new(ObserverAgent::new(budget.clone())));
    let child = ObserverRegistration::child(budget.clone()).unwrap();
    let charged = budget.used(NativeMemoryComponent::MutationObserver);
    assert!(charged >= std::mem::size_of::<ObserverRegistration>());
    registry.attach(&child, Arc::downgrade(&agent)).unwrap();
    assert!(registry.retire(&child).unwrap());
    drop(agent);
    assert_eq!(registry.drain_retired(&mut document).unwrap(), 1);
    assert_eq!(
        budget.used(NativeMemoryComponent::MutationObserver),
        charged
    );
    drop(child);
    assert_eq!(budget.used(NativeMemoryComponent::MutationObserver), 0);
}

#[test]
fn concurrent_headers_cannot_publish_one_registration_twice() {
    use std::sync::Barrier;

    let mut document = Document::new();
    let first = ObserverRegistry::new(document.document_id());
    let second = ObserverRegistry::new(document.document_id());
    let node = ObserverRegistration::root(budget());
    let agents = [
        Arc::new(Mutex::new(ObserverAgent::new(budget()))),
        Arc::new(Mutex::new(ObserverAgent::new(budget()))),
    ];
    let start = Arc::new(Barrier::new(3));
    let attempts = std::thread::scope(|scope| {
        let left = scope.spawn(|| {
            start.wait();
            first.attach(&node, Arc::downgrade(&agents[0]))
        });
        let right = scope.spawn(|| {
            start.wait();
            second.attach(&node, Arc::downgrade(&agents[1]))
        });
        start.wait();
        [left.join().unwrap(), right.join().unwrap()]
    });
    assert_eq!(
        attempts
            .iter()
            .filter(|result| matches!(result, Ok(true)))
            .count(),
        1
    );
    assert_eq!(
        usize::from(first.active_head().is_some()) + usize::from(second.active_head().is_some()),
        1
    );
    first.drain_all_on_document_drop(&mut document);
    second.drain_all_on_document_drop(&mut document);
}

#[test]
fn expired_world_cannot_publish_a_registration() {
    let mut document = Document::new();
    let registry = ObserverRegistry::new(document.document_id());
    let node = ObserverRegistration::root(budget());
    let agent = Arc::new(Mutex::new(ObserverAgent::new(budget())));
    let expired = Arc::downgrade(&agent);
    drop(agent);
    assert!(registry.attach(&node, expired).is_err());
    assert!(registry.active_head().is_none());
    registry.drain_all_on_document_drop(&mut document);
}

#[test]
fn final_captured_head_drops_a_deep_saved_successor_chain_iteratively() {
    let budget = budget();
    let mut head = ObserverRegistration::root(budget.clone());
    for _ in 0..8192 {
        let next = ObserverRegistration::root(budget.clone());
        next.state.lock().next = Some(head);
        head = next;
    }
    drop(head);
    assert_eq!(budget.used(NativeMemoryComponent::MutationObserver), 0);
}

#[test]
fn borrowed_document_defers_one_worlds_published_release_until_mutable_sweep() {
    let shared = Rc::new(RefCell::new(Document::new()));
    let target = shared.borrow_mut().create_html_element(atoms::DIV);
    let first_budget = budget();
    let second_budget = budget();
    let (first_agent, first_attachment) = shared
        .borrow_mut()
        .new_observer_world_owned(first_budget.clone());
    let (second_agent, second_attachment) = shared
        .borrow_mut()
        .new_observer_world_owned(second_budget.clone());
    let first = Arc::clone(&first_attachment.registration);
    let second = Arc::clone(&second_attachment.registration);
    let key = {
        let document = shared.borrow();
        ObserverNode::new(document.document_id(), document.node_ref(target))
    };
    let mut first_record = {
        let mut document = shared.borrow_mut();
        let facts = attribute_record(&document, target);
        first
            .with_pins(|pins| {
                OwnedObserverRecord::acquire_pending(&first_budget, pins, &mut document, facts)
            })
            .unwrap()
    };
    let mut second_record = {
        let mut document = shared.borrow_mut();
        let facts = attribute_record(&document, target);
        second
            .with_pins(|pins| {
                OwnedObserverRecord::acquire_pending(&second_budget, pins, &mut document, facts)
            })
            .unwrap()
    };
    first.with_pins(|pins| first_record.publish(pins).unwrap());
    second.with_pins(|pins| second_record.publish(pins).unwrap());
    assert_eq!(shared.borrow().handle_count(target), 2);

    let held = shared.borrow_mut();
    first
        .with_pins(|pins| first_record.release_published(pins))
        .unwrap();
    assert_eq!(held.handle_count(target), 2);
    assert_eq!(
        first.with_pins_read(|pins| pins.owner_counts(key)),
        Some((0, 0))
    );
    assert_eq!(
        second.with_pins_read(|pins| pins.owner_counts(key)),
        Some((0, 1))
    );
    drop(first_record);
    assert!(first_budget.used(NativeMemoryComponent::MutationObserver) > 0);
    drop(held);

    shared.borrow_mut().sweep_detached();
    assert_eq!(shared.borrow().handle_count(target), 1);
    assert_eq!(first.with_pins_read(|pins| pins.owner_counts(key)), None);
    assert_eq!(
        second.with_pins_read(|pins| pins.owner_counts(key)),
        Some((0, 1))
    );
    let held = shared.borrow();
    second
        .with_pins(|pins| second_record.release_published(pins))
        .unwrap();
    assert_eq!(held.handle_count(target), 1);
    drop(second_record);
    drop(held);
    shared.borrow_mut().sweep_detached();
    assert!(shared.borrow().resolve(key.node).is_err());
    assert_eq!(second.with_pins_read(|pins| pins.owner_counts(key)), None);
    assert!(first_budget.first_failure().is_none());
    assert!(second_budget.first_failure().is_none());
    drop(first_attachment);
    drop(second_attachment);
    drop(first_agent);
    drop(second_agent);
    shared.borrow_mut().sweep_detached();
}
