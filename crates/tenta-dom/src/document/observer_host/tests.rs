use super::*;
use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::atom::atoms;
use tenta_core::{NativeMemoryBudget, NativeMemoryComponent, NodeRef};
use tenta_html::QualName;

use crate::mutation::Mutation;

fn budget() -> NativeMemoryBudget {
    NativeMemoryBudget::with_limits(1024 * 1024, 1024 * 1024, 1024 * 1024, 0)
}

fn key(document: &Document, node: tenta_core::NodeId) -> ObserverNode {
    ObserverNode::new(document.document_id(), document.node_ref(node))
}

fn attached() -> (
    Rc<RefCell<Document>>,
    SharedObserverAgent,
    super::super::observer_registry::ObserverAttachment,
    NativeObserver,
) {
    let document = Rc::new(RefCell::new(Document::new()));
    let (agent, attachment) = document.borrow_mut().new_observer_world_owned(budget());
    let observer = NativeObserver::new(Arc::clone(&agent), ObserverId(0x1234)).unwrap();
    (document, agent, attachment, observer)
}

fn registration(agent: &SharedObserverAgent, document: &Document) -> Arc<ObserverRegistration> {
    agent
        .lock()
        .registration_for(document.document_id())
        .cloned()
        .unwrap()
}

#[test]
fn option_presence_and_exact_target_generation_are_validated_before_registration() {
    let (shared, agent, attachment, observer) = attached();
    let node = shared.borrow_mut().create_html_element(atoms::DIV);
    let target = key(&shared.borrow(), node);
    observer
        .observe(
            &shared.borrow(),
            target,
            ObserveOptionsInput {
                attribute_old_value: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(agent.lock().journal().has_registrations());
    assert!(matches!(
        observer.observe(
            &shared.borrow(),
            target,
            ObserveOptionsInput {
                attributes: Some(false),
                attribute_filter: Some(vec![atoms::ID]),
                ..Default::default()
            }
        ),
        Err(NativeObserveError::Options(
            OptionsError::NoMutationType | OptionsError::AttributesDisabled
        ))
    ));
    let stale = ObserverNode::new(
        target.document_id,
        NodeRef::new(target.node.id, target.node.generation + 1),
    );
    assert!(matches!(
        observer.observe(
            &shared.borrow(),
            stale,
            ObserveOptionsInput {
                child_list: true,
                ..Default::default()
            }
        ),
        Err(NativeObserveError::Native(_))
    ));
    let foreign = Document::new();
    let foreign_key = ObserverNode::new(foreign.document_id(), target.node);
    assert!(matches!(
        observer.observe(
            &shared.borrow(),
            foreign_key,
            ObserveOptionsInput {
                child_list: true,
                ..Default::default()
            }
        ),
        Err(NativeObserveError::Native(_))
    ));
    assert!(agent.lock().journal().has_registrations());
    drop(attachment);
}

#[test]
fn partial_publication_drops_only_pending_records_under_held_document_borrow() {
    let (shared, agent, attachment, observer) = attached();
    let parent = shared.borrow_mut().create_html_element(atoms::DIV);
    let first = shared.borrow_mut().create_html_element(atoms::SPAN);
    let second = shared.borrow_mut().create_html_element(atoms::B);
    let (parent_key, first_key, second_key) = {
        let document = shared.borrow();
        (
            key(&document, parent),
            key(&document, first),
            key(&document, second),
        )
    };
    observer
        .observe(
            &shared.borrow(),
            parent_key,
            ObserveOptionsInput {
                child_list: true,
                ..Default::default()
            },
        )
        .unwrap();
    shared
        .borrow_mut()
        .mutate(Mutation::Append {
            parent,
            child: first,
        })
        .unwrap();
    shared
        .borrow_mut()
        .mutate(Mutation::Append {
            parent,
            child: second,
        })
        .unwrap();

    let mut batch = observer.take_records().unwrap();
    assert_eq!(batch.len(), 2);
    assert_eq!(batch.front().unwrap().added_nodes, &[first_key]);
    let mut published = batch.commit_published_front().unwrap();
    assert_eq!(published.view().unwrap().record_type, RecordType::ChildList);
    let registration = registration(&agent, &shared.borrow());
    let held = shared.borrow_mut();
    drop(batch);
    assert_eq!(
        registration.with_pins_read(|pins| pins.owner_counts(parent_key)),
        Some((0, 1))
    );
    assert_eq!(
        registration.with_pins_read(|pins| pins.owner_counts(first_key)),
        Some((0, 1))
    );
    assert_eq!(
        registration.with_pins_read(|pins| pins.owner_counts(second_key)),
        Some((0, 0))
    );
    published.release().unwrap();
    drop(published);
    assert_eq!(held.handle_count(parent), 1);
    drop(held);
    shared.borrow_mut().sweep_detached();
    assert!(shared.borrow().resolve(parent_key.node).is_err());
    assert!(shared.borrow().resolve(first_key.node).is_err());
    assert!(shared.borrow().resolve(second_key.node).is_err());
    assert!(agent.lock().check_host_access().is_ok());
    drop(attachment);
}

#[test]
fn static_member_list_outlives_record_and_releases_under_refmut() {
    let (shared, agent, attachment, observer) = attached();
    let parent = shared.borrow_mut().create_html_element(atoms::DIV);
    let child = shared.borrow_mut().create_html_element(atoms::SPAN);
    let (parent_key, child_key) = {
        let document = shared.borrow();
        (key(&document, parent), key(&document, child))
    };
    observer
        .observe(
            &shared.borrow(),
            parent_key,
            ObserveOptionsInput {
                child_list: true,
                ..Default::default()
            },
        )
        .unwrap();
    shared
        .borrow_mut()
        .mutate(Mutation::Append { parent, child })
        .unwrap();
    let mut batch = observer.take_records().unwrap();
    let mut record = batch.commit_published_front().unwrap();
    drop(batch);
    let mut list = record.retain_list(ObserverListKind::Added).unwrap();
    assert_eq!(list.members().unwrap(), &[child_key]);
    let registration = registration(&agent, &shared.borrow());
    let held = shared.borrow_mut();
    record.release().unwrap();
    drop(record);
    assert_eq!(
        registration.with_pins_read(|pins| pins.owner_counts(parent_key)),
        Some((0, 0))
    );
    assert_eq!(
        registration.with_pins_read(|pins| pins.owner_counts(child_key)),
        Some((0, 1))
    );
    list.release().unwrap();
    drop(list);
    assert_eq!(
        registration.with_pins_read(|pins| pins.owner_counts(child_key)),
        Some((0, 0))
    );
    drop(held);
    shared.borrow_mut().sweep_detached();
    assert!(shared.borrow().resolve(child_key.node).is_err());
    drop(attachment);
}

fn late_drop_after_drain(direct_teardown: bool) {
    let (shared, agent, attachment, observer) = attached();
    let mut attachment = Some(attachment);
    let parent = shared.borrow_mut().create_html_element(atoms::DIV);
    let first = shared.borrow_mut().create_html_element(atoms::SPAN);
    let second = shared.borrow_mut().create_html_element(atoms::B);
    let target = key(&shared.borrow(), parent);
    let first_key = key(&shared.borrow(), first);
    let second_key = key(&shared.borrow(), second);
    observer
        .observe(
            &shared.borrow(),
            target,
            ObserveOptionsInput {
                child_list: true,
                ..Default::default()
            },
        )
        .unwrap();
    shared
        .borrow_mut()
        .mutate(Mutation::Append {
            parent,
            child: first,
        })
        .unwrap();
    shared
        .borrow_mut()
        .mutate(Mutation::Append {
            parent,
            child: second,
        })
        .unwrap();
    let mut batch = observer.take_records().unwrap();
    let mut published = batch.commit_published_front().unwrap();
    let mut list = published.retain_list(ObserverListKind::Added).unwrap();
    assert_eq!(list.members().unwrap(), &[first_key]);
    let registration = registration(&agent, &shared.borrow());
    if direct_teardown {
        let work = agent.lock().begin_teardown().unwrap();
        drop(work);
        agent
            .lock()
            .release_document_pins_on_teardown(&mut shared.borrow_mut())
            .unwrap();
    } else {
        drop(attachment.take());
    }
    shared.borrow_mut().sweep_detached();
    assert_eq!(registration.phase(), RegistrationPhase::Drained);
    assert!(agent.lock().check_native_health().is_ok());
    assert!(shared.borrow().resolve(target.node).is_err());
    assert!(shared.borrow().resolve(first_key.node).is_err());
    assert!(shared.borrow().resolve(second_key.node).is_err());
    let recycled = {
        let mut document = shared.borrow_mut();
        let old = [parent, first, second];
        let mut recycled = None;
        for _ in 0..8 {
            let fresh = document.create_html_element(atoms::P);
            if old.contains(&fresh) {
                recycled = Some(document.node_ref(fresh));
            }
        }
        recycled.expect("a freed native slot must be reused")
    };
    let held = shared.borrow_mut();
    drop(batch);
    published.release().unwrap();
    drop(published);
    list.release().unwrap();
    drop(list);
    assert_eq!(held.resolve(recycled).unwrap(), recycled.id);
    assert_eq!(held.handle_count(recycled.id), 0);
    drop(held);
    assert!(agent.lock().check_native_health().is_ok());
    if direct_teardown {
        agent.lock().finish_teardown().unwrap();
        drop(attachment.take());
    }
    shared.borrow_mut().sweep_detached();
    assert!(shared.borrow().resolve(target.node).is_err());
}

#[test]
fn batch_binding_quota_failure_keeps_the_journal_queue_and_exact_pin() {
    let (shared, agent, attachment, observer) = attached();
    let node = shared.borrow_mut().create_html_element(atoms::DIV);
    let target = key(&shared.borrow(), node);
    observer
        .observe(
            &shared.borrow(),
            target,
            ObserveOptionsInput {
                attributes: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
    shared
        .borrow_mut()
        .mutate(Mutation::SetAttribute {
            element: node,
            name: QualName::new(atoms::EMPTY, atoms::CLASS),
            value: "first".to_owned(),
        })
        .unwrap();
    let registration = registration(&agent, &shared.borrow());
    let budget = agent.lock().budget().clone();
    let mut pressure = budget.begin(NativeMemoryComponent::MutationObserver);
    assert!(pressure.charge(1024 * 1024 - budget.used(NativeMemoryComponent::MutationObserver)));
    assert!(matches!(
        observer.take_records(),
        Err(Error::BudgetExceeded { .. })
    ));
    assert!(matches!(
        observer.observe(
            &shared.borrow(),
            target,
            ObserveOptionsInput {
                attributes: Some(true),
                ..Default::default()
            }
        ),
        Err(NativeObserveError::Native(Error::BudgetExceeded { .. }))
    ));
    assert_eq!(agent.lock().journal().queued_count(observer.id()), 1);
    assert_eq!(
        registration.with_pins_read(|pins| pins.owner_counts(target)),
        Some((1, 0))
    );
    assert_eq!(shared.borrow().handle_count(node), 1);
    drop(pressure);
    assert!(budget.first_failure().is_some());
    drop(attachment);
    shared.borrow_mut().sweep_detached();
    assert!(shared.borrow().resolve(target.node).is_err());
}

#[test]
fn direct_teardown_drains_before_pending_record_and_list_finalizers() {
    late_drop_after_drain(true);
}

#[test]
fn retired_attachment_drains_before_pending_record_and_list_finalizers() {
    late_drop_after_drain(false);
}

#[test]
fn duplicate_logical_release_poison_blocks_later_host_calls_and_mutations() {
    let (shared, agent, attachment, observer) = attached();
    let node = shared.borrow_mut().create_html_element(atoms::DIV);
    let target = key(&shared.borrow(), node);
    observer
        .observe(
            &shared.borrow(),
            target,
            ObserveOptionsInput {
                attributes: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
    shared
        .borrow_mut()
        .mutate(Mutation::SetAttribute {
            element: node,
            name: QualName::new(atoms::EMPTY, atoms::CLASS),
            value: "first".to_owned(),
        })
        .unwrap();
    let mut batch = observer.take_records().unwrap();
    let registration = registration(&agent, &shared.borrow());
    registration
        .release_pending_record(batch.records.front_mut().unwrap())
        .unwrap();
    assert_eq!(
        registration.with_pins_read(|pins| pins.owner_counts(target)),
        Some((0, 0))
    );
    let held = shared.borrow();
    drop(batch);
    assert_eq!(held.handle_count(node), 1);
    drop(held);
    assert!(matches!(
        observer.take_records(),
        Err(Error::Crashed { .. })
    ));
    assert!(matches!(
        shared.borrow_mut().mutate(Mutation::SetAttribute {
            element: node,
            name: QualName::new(atoms::EMPTY, atoms::ID),
            value: "later".to_owned(),
        }),
        Err(Error::Crashed { .. })
    ));
    shared.borrow_mut().sweep_detached();
    assert_eq!(shared.borrow().handle_count(node), 0);
    assert!(agent.lock().check_native_health().is_err());
    assert_eq!(
        registration.with_pins_read(|pins| pins.owner_counts(target)),
        None
    );
    drop(attachment);
}

#[test]
fn failed_published_release_poisons_before_the_owner_is_dropped() {
    let (shared, agent, attachment, observer) = attached();
    let node = shared.borrow_mut().create_html_element(atoms::DIV);
    let target = key(&shared.borrow(), node);
    observer
        .observe(
            &shared.borrow(),
            target,
            ObserveOptionsInput {
                attributes: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
    shared
        .borrow_mut()
        .mutate(Mutation::SetAttribute {
            element: node,
            name: QualName::new(atoms::EMPTY, atoms::CLASS),
            value: "first".to_owned(),
        })
        .unwrap();
    let mut batch = observer.take_records().unwrap();
    let mut published = batch.commit_published_front().unwrap();
    drop(batch);
    let registration = registration(&agent, &shared.borrow());
    registration
        .release_published_record(published.record.as_mut().unwrap())
        .unwrap();
    let held = shared.borrow();
    assert!(published.release().is_err());
    assert!(published.view().is_ok());
    assert_eq!(held.handle_count(node), 1);
    assert!(matches!(
        observer.take_records(),
        Err(Error::Crashed { .. })
    ));
    assert!(matches!(
        published.retain_list(ObserverListKind::Added),
        Err(Error::Crashed { .. })
    ));
    drop(published);
    drop(held);
    shared.borrow_mut().sweep_detached();
    assert!(shared.borrow().resolve(target.node).is_err());
    assert_eq!(
        registration.with_pins_read(|pins| pins.owner_counts(target)),
        None
    );
    drop(attachment);
}

#[test]
fn disconnect_removes_registration_but_moved_record_keeps_its_pin() {
    let (shared, agent, attachment, observer) = attached();
    let parent = shared.borrow_mut().create_html_element(atoms::DIV);
    let child = shared.borrow_mut().create_html_element(atoms::SPAN);
    let target = key(&shared.borrow(), parent);
    let member = key(&shared.borrow(), child);
    observer
        .observe(
            &shared.borrow(),
            target,
            ObserveOptionsInput {
                child_list: true,
                ..Default::default()
            },
        )
        .unwrap();
    shared
        .borrow_mut()
        .mutate(Mutation::Append { parent, child })
        .unwrap();
    let batch = observer.disconnect().unwrap();
    assert!(!agent.lock().journal().has_registrations());
    assert_eq!(batch.front().unwrap().added_nodes, &[member]);
    assert_eq!(shared.borrow().handle_count(parent), 1);
    let held = shared.borrow();
    drop(batch);
    assert_eq!(held.handle_count(parent), 1);
    drop(held);
    shared.borrow_mut().sweep_detached();
    assert!(shared.borrow().resolve(target.node).is_err());
    assert!(observer.take_records().unwrap().is_empty());
    drop(attachment);
}

#[test]
fn retired_registration_refuses_new_publication_without_poisoning_late_cleanup() {
    let (shared, agent, attachment, observer) = attached();
    let parent = shared.borrow_mut().create_html_element(atoms::DIV);
    let first = shared.borrow_mut().create_html_element(atoms::SPAN);
    let second = shared.borrow_mut().create_html_element(atoms::B);
    let target = key(&shared.borrow(), parent);
    observer
        .observe(
            &shared.borrow(),
            target,
            ObserveOptionsInput {
                child_list: true,
                ..Default::default()
            },
        )
        .unwrap();
    shared
        .borrow_mut()
        .mutate(Mutation::Append {
            parent,
            child: first,
        })
        .unwrap();
    shared
        .borrow_mut()
        .mutate(Mutation::Append {
            parent,
            child: second,
        })
        .unwrap();
    let mut batch = observer.take_records().unwrap();
    let mut published = batch.commit_published_front().unwrap();
    let registration = registration(&agent, &shared.borrow());
    drop(attachment);
    assert_eq!(registration.phase(), RegistrationPhase::Retired);
    assert!(matches!(batch.commit_published_front(), Err(Error::Closed)));
    assert!(matches!(
        published.retain_list(ObserverListKind::Added),
        Err(Error::Closed)
    ));
    assert!(agent.lock().check_native_health().is_ok());
    let held = shared.borrow_mut();
    drop(batch);
    published.release().unwrap();
    drop(published);
    assert!(held.handle_count(parent) > 0);
    drop(held);
    shared.borrow_mut().sweep_detached();
    assert_eq!(registration.phase(), RegistrationPhase::Drained);
    assert!(shared.borrow().resolve(target.node).is_err());
    assert!(agent.lock().check_native_health().is_ok());
}

#[test]
fn inert_host_admission_normalizes_options_without_native_registration_or_pin() {
    let (shared, agent, attachment, observer) = attached();
    let node = shared.borrow_mut().create_html_element(atoms::DIV);
    let before_nodes = shared.borrow().len();
    let memory = agent.lock().budget().clone();
    let before_bytes = memory.used(NativeMemoryComponent::MutationObserver);
    assert!(matches!(
        observer.observe_inert(&shared.borrow(), ObserveOptionsInput::default()),
        Err(NativeObserveError::Options(OptionsError::NoMutationType))
    ));
    for options in [
        ObserveOptionsInput {
            attribute_old_value: Some(false),
            ..Default::default()
        },
        ObserveOptionsInput {
            character_data_old_value: Some(false),
            ..Default::default()
        },
        ObserveOptionsInput {
            attribute_filter: Some(vec![atoms::ID]),
            ..Default::default()
        },
    ] {
        observer.observe_inert(&shared.borrow(), options).unwrap();
    }
    assert!(matches!(
        observer.observe_inert(
            &shared.borrow(),
            ObserveOptionsInput {
                child_list: true,
                attributes: Some(false),
                attribute_filter: Some(vec![atoms::ID]),
                ..Default::default()
            }
        ),
        Err(NativeObserveError::Options(
            OptionsError::AttributesDisabled
        ))
    ));
    assert!(matches!(
        observer.observe_inert(
            &Document::new(),
            ObserveOptionsInput {
                child_list: true,
                ..Default::default()
            }
        ),
        Err(NativeObserveError::Native(_))
    ));
    assert!(!agent.lock().journal().has_registrations());
    assert_eq!(shared.borrow().handle_count(node), 0);
    assert_eq!(shared.borrow().len(), before_nodes);
    assert_eq!(
        memory.used(NativeMemoryComponent::MutationObserver),
        before_bytes
    );
    drop(attachment);
    assert!(matches!(
        observer.observe_inert(
            &shared.borrow(),
            ObserveOptionsInput {
                child_list: true,
                ..Default::default()
            }
        ),
        Err(NativeObserveError::Native(Error::Closed))
    ));
}

#[test]
fn native_graph_subtracts_only_current_active_conditional_generation() {
    let (shared, agent, attachment, observer) = attached();
    let (foreign_agent, foreign_attachment) =
        shared.borrow_mut().new_observer_world_owned(budget());
    let foreign_observer = NativeObserver::new(Arc::clone(&foreign_agent), observer.id()).unwrap();
    let node = shared.borrow_mut().create_html_element(atoms::DIV);
    let target = key(&shared.borrow(), node);
    let external = shared.borrow_mut().handle(node).unwrap();
    for observer in [&observer, &foreign_observer] {
        observer
            .observe(
                &shared.borrow(),
                target,
                ObserveOptionsInput {
                    attributes: Some(true),
                    ..Default::default()
                },
            )
            .unwrap();
    }
    shared
        .borrow_mut()
        .mutate(Mutation::SetAttribute {
            element: node,
            name: QualName::new(atoms::EMPTY, atoms::CLASS),
            value: "first".to_owned(),
        })
        .unwrap();
    let mut batch = observer.take_records().unwrap();
    assert_eq!(
        agent.lock().conditional_handle_subtraction(target).unwrap(),
        0
    );
    let published = batch.commit_published_front().unwrap();
    assert_eq!(shared.borrow().handle_count(node), 3);
    assert_eq!(
        agent.lock().conditional_handle_subtraction(target).unwrap(),
        1
    );
    assert_eq!(
        foreign_agent
            .lock()
            .conditional_handle_subtraction(target)
            .unwrap(),
        0
    );
    let stale = ObserverNode::new(
        target.document_id,
        NodeRef::new(target.node.id, target.node.generation + 1),
    );
    assert_eq!(
        agent.lock().conditional_handle_subtraction(stale).unwrap(),
        0
    );
    let other_document = Document::new();
    let foreign = ObserverNode::new(other_document.document_id(), target.node);
    assert_eq!(
        agent
            .lock()
            .conditional_handle_subtraction(foreign)
            .unwrap(),
        0
    );

    // Retirement keeps the same physical pin independently rooted until a
    // mutable Document seam drains it, even while its record still exists.
    drop(attachment);
    assert_eq!(shared.borrow().handle_count(node), 3);
    assert_eq!(
        agent.lock().conditional_handle_subtraction(target).unwrap(),
        0
    );
    let foreign_batch = foreign_observer.disconnect().unwrap();
    drop(foreign_batch);
    drop(batch);
    drop(published);
    shared.borrow_mut().sweep_detached();
    assert!(shared.borrow().resolve(target.node).is_ok());
    assert_eq!(shared.borrow().handle_count(node), 1);
    shared.borrow_mut().release(external);
    shared.borrow_mut().sweep_detached();
    assert!(shared.borrow().resolve(target.node).is_err());
    let fresh = shared.borrow_mut().create_html_element(atoms::P);
    assert_eq!(fresh, node);
    let fresh_key = key(&shared.borrow(), fresh);
    assert_ne!(fresh_key.node.generation, target.node.generation);
    assert_eq!(
        foreign_agent
            .lock()
            .conditional_handle_subtraction(fresh_key)
            .unwrap(),
        0
    );
    drop(foreign_attachment);
}

fn graph(agent: &SharedObserverAgent) -> Vec<super::super::ObserverGraphEdge> {
    let mut edges = Vec::new();
    agent
        .lock()
        .visit_gc_edges(|edge| {
            edges.push(edge);
            Ok(())
        })
        .unwrap();
    edges
}

#[test]
fn native_graph_preserves_transient_edges_and_pending_roots_until_notify() {
    use super::super::ObserverGraphEdge;
    let (shared, agent, attachment, observer) = attached();
    let parent = shared.borrow_mut().create_html_element(atoms::DIV);
    let child = shared.borrow_mut().create_html_element(atoms::SPAN);
    shared
        .borrow_mut()
        .mutate(Mutation::Append { parent, child })
        .unwrap();
    let parent_key = key(&shared.borrow(), parent);
    let child_key = key(&shared.borrow(), child);
    observer
        .observe(
            &shared.borrow(),
            parent_key,
            ObserveOptionsInput {
                attributes: Some(true),
                subtree: true,
                ..Default::default()
            },
        )
        .unwrap();
    shared
        .borrow_mut()
        .mutate(Mutation::Remove { child })
        .unwrap();
    let parent_edge = ObserverGraphEdge::Target {
        target: parent_key,
        observer: observer.id(),
    };
    let child_edge = ObserverGraphEdge::Target {
        target: child_key,
        observer: observer.id(),
    };
    let pending = ObserverGraphEdge::Pending(observer.id());
    let edges = graph(&agent);
    assert_eq!(edges.len(), 3);
    assert!(edges.contains(&parent_edge));
    assert!(edges.contains(&child_edge));
    assert!(edges.contains(&pending));
    assert!(observer.take_records().unwrap().is_empty());
    assert!(graph(&agent).contains(&pending));
    agent.lock().mark_notify_enqueued().unwrap();
    let notify = agent.lock().begin_notify().unwrap();
    assert_eq!(
        notify.iter().copied().collect::<Vec<_>>(),
        vec![observer.id()]
    );
    let edges = graph(&agent);
    assert!(!edges.contains(&pending));
    assert!(edges.contains(&child_edge));
    assert!(observer.drain_for_notify().unwrap().is_empty());
    assert_eq!(graph(&agent), vec![parent_edge]);
    drop(notify);
    drop(attachment);
    assert!(graph(&agent).is_empty());
}

#[test]
fn readonly_native_graph_failure_is_propagated_without_consuming_edges() {
    let (shared, agent, attachment, observer) = attached();
    let node = shared.borrow_mut().create_html_element(atoms::DIV);
    let target = key(&shared.borrow(), node);
    observer
        .observe(
            &shared.borrow(),
            target,
            ObserveOptionsInput {
                child_list: true,
                ..Default::default()
            },
        )
        .unwrap();
    let before = graph(&agent);
    let mut calls = 0;
    let outcome = agent.lock().visit_gc_edges(|_| {
        calls += 1;
        Err(Error::Closed)
    });
    assert!(matches!(outcome, Err(Error::Closed)));
    assert_eq!(calls, 1);
    assert_eq!(graph(&agent), before);
    assert_eq!(shared.borrow().handle_count(node), 0);
    agent.lock().poison_shared_effect();
    assert!(matches!(
        agent
            .lock()
            .visit_gc_edges(|_| panic!("poisoned graph visited")),
        Err(Error::Crashed { .. })
    ));
    assert!(matches!(
        agent.lock().conditional_handle_subtraction(target),
        Err(Error::Crashed { .. })
    ));
    assert!(matches!(
        observer.observe_inert(
            &shared.borrow(),
            ObserveOptionsInput {
                child_list: true,
                ..Default::default()
            }
        ),
        Err(NativeObserveError::Native(Error::Crashed { .. }))
    ));
    drop(attachment);
}

#[test]
fn inert_admission_and_graph_inspection_preserve_sticky_budget_failure() {
    let (shared, agent, attachment, observer) = attached();
    let memory = agent.lock().budget().clone();
    memory
        .begin(NativeMemoryComponent::MutationObserver)
        .fail_allocation();
    assert!(matches!(
        observer.observe_inert(&shared.borrow(), ObserveOptionsInput::default()),
        Err(NativeObserveError::Native(Error::BudgetExceeded { .. }))
    ));
    assert!(matches!(
        agent
            .lock()
            .visit_gc_edges(|_| panic!("failed graph visited")),
        Err(Error::BudgetExceeded { .. })
    ));
    let target = key(&shared.borrow(), shared.borrow().document_node());
    assert!(matches!(
        agent.lock().conditional_handle_subtraction(target),
        Err(Error::BudgetExceeded { .. })
    ));
    drop(attachment);
}

#[test]
fn host_ownership_poison_is_sticky_without_borrowing_the_document() {
    let (shared, agent, attachment, observer) = attached();
    let held = shared.borrow_mut();
    assert!(agent.lock().check_host_access().is_ok());
    agent.lock().poison_host_ownership();
    agent.lock().poison_host_ownership();
    assert!(matches!(
        agent.lock().check_host_access(),
        Err(Error::Crashed { .. })
    ));
    assert!(matches!(
        observer.take_records(),
        Err(Error::Crashed { .. })
    ));
    drop(held);
    drop(attachment);
    shared.borrow_mut().sweep_detached();
    assert!(matches!(
        agent.lock().check_host_access(),
        Err(Error::Crashed { .. })
    ));
}
