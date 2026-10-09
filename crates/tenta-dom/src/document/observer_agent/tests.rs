use super::*;
use tenta_core::atom::atoms;

use super::super::observer_journal::{
    MutationFacts, ObserveOptions, ObserveOptionsInput, ObserverId, ObserverNode, RecordType,
};
use super::super::observer_semantics::capture_validated;
use super::super::observer_semantics::{SemanticStep, TransientRemoval};

fn node(document: &Document, id: tenta_core::NodeId) -> ObserverNode {
    ObserverNode::new(document.document_id(), document.node_ref(id))
}

fn attribute_options(subtree: bool) -> ObserveOptions {
    ObserveOptions::normalize(ObserveOptionsInput {
        attributes: Some(true),
        subtree,
        ..ObserveOptionsInput::default()
    })
    .unwrap()
}

fn child_list_options() -> ObserveOptions {
    ObserveOptions::normalize(ObserveOptionsInput {
        child_list: true,
        ..ObserveOptionsInput::default()
    })
    .unwrap()
}

fn attribute_plan(target: ObserverNode) -> SemanticPlan {
    SemanticPlan {
        records: vec![MutationFacts::attributes(
            target,
            vec![target],
            atoms::CLASS,
            None,
            None,
        )],
        transient_removals: vec![],
        steps: vec![SemanticStep::QueueRecord(target)],
        request_notification: true,
    }
}

#[test]
fn root_registration_reuses_an_alias_without_a_dynamic_charge() {
    let document = Document::new();
    let child = Document::new();
    let budget = NativeMemoryBudget::with_limits(0, 0, 0, 0);
    let mut agent = ObserverAgent::new(budget.clone());
    assert!(matches!(
        agent.attach_document(&child),
        Err(Error::InvalidArgument { reason }) if reason == "observer root Document is not bound"
    ));
    assert!(budget.first_failure().is_none());
    agent.bind_root_document(&document).unwrap();
    agent.attach_document(&document).unwrap();
    let root = Arc::clone(agent.registration_for(document.document_id()).unwrap());
    agent.attach_document(&document).unwrap();
    assert!(Arc::ptr_eq(
        &root,
        agent.registration_for(document.document_id()).unwrap()
    ));
    assert_eq!(budget.used(NativeMemoryComponent::MutationObserver), 0);

    assert!(matches!(
        agent.attach_document(&child),
        Err(Error::BudgetExceeded { .. })
    ));
    assert!(agent.registration_for(child.document_id()).is_none());
    assert!(Arc::ptr_eq(
        &root,
        agent.registration_for(document.document_id()).unwrap()
    ));
    drop(root);
    drop(agent);
    assert_eq!(budget.used(NativeMemoryComponent::MutationObserver), 0);
}

#[test]
fn child_registration_is_distinct_and_releases_its_charge() {
    let mut root_document = Document::new();
    let mut child_document = Document::new();
    let budget = NativeMemoryBudget::with_limits(1024 * 1024, 1024 * 1024, 1024 * 1024, 0);
    let mut agent = ObserverAgent::new(budget.clone());
    agent.bind_root_document(&root_document).unwrap();
    agent.attach_document(&root_document).unwrap();
    agent.attach_document(&child_document).unwrap();
    let root = agent.registration_for(root_document.document_id()).unwrap();
    let child = agent
        .registration_for(child_document.document_id())
        .unwrap();
    assert!(!Arc::ptr_eq(root, child));
    assert!(budget.used(NativeMemoryComponent::MutationObserver) > 0);

    assert!(matches!(
        agent.release_document_pins_on_teardown(&mut root_document),
        Err(Error::InvalidArgument { reason }) if reason == "observer teardown has not started"
    ));
    drop(agent.begin_teardown().unwrap());
    agent
        .release_document_pins_on_teardown(&mut root_document)
        .unwrap();
    agent
        .release_document_pins_on_teardown(&mut child_document)
        .unwrap();
    agent.finish_teardown().unwrap();
    drop(agent);
    assert_eq!(budget.used(NativeMemoryComponent::MutationObserver), 0);
}

#[test]
fn unpublished_native_registration_drains_only_its_exact_document() {
    let mut root_document = Document::new();
    let mut child_document = Document::new();
    let element = root_document.create_html_element(atoms::DIV);
    let target = node(&root_document, element);
    let budget = NativeMemoryBudget::with_limits(1024 * 1024, 1024 * 1024, 1024 * 1024, 0);
    let mut agent = ObserverAgent::new(budget.clone());
    agent.bind_root_document(&root_document).unwrap();
    agent.attach_document(&root_document).unwrap();
    agent.attach_document(&child_document).unwrap();
    let registration = Arc::clone(agent.registration_for(root_document.document_id()).unwrap());
    assert_eq!(registration.phase(), RegistrationPhase::Unbound);
    assert_eq!(
        registration.document_id(),
        Some(root_document.document_id())
    );

    let observer = ObserverId(9);
    agent
        .journal_mut()
        .observe(observer, target, attribute_options(false))
        .unwrap();
    let mut operation = agent
        .prepare(&mut root_document, &attribute_plan(target))
        .unwrap();
    agent.queue_next_record(&mut operation, target).unwrap();
    assert!(agent.commit(operation));
    assert_eq!(root_document.handle_count(element), 1);

    drop(agent.begin_teardown().unwrap());
    assert!(matches!(
        registration.drain_on_teardown(&mut child_document),
        Err(Error::InvalidArgument { reason }) if reason == "observer teardown used another Document"
    ));
    assert_eq!(root_document.handle_count(element), 1);
    agent
        .release_document_pins_on_teardown(&mut root_document)
        .unwrap();
    assert_eq!(root_document.handle_count(element), 0);
    agent
        .release_document_pins_on_teardown(&mut root_document)
        .unwrap();
    agent
        .release_document_pins_on_teardown(&mut child_document)
        .unwrap();
    agent.finish_teardown().unwrap();
    drop(registration);
    drop(agent);
    assert_eq!(budget.used(NativeMemoryComponent::MutationObserver), 0);
}

#[test]
fn prepared_attribute_record_commits_with_exact_pin_then_releases() {
    let mut document = Document::new();
    let element = document.create_html_element(atoms::DIV);
    let target = node(&document, element);
    let budget = NativeMemoryBudget::with_limits(1024 * 1024, 1024 * 1024, 1024 * 1024, 0);
    let mut agent = ObserverAgent::new(budget.clone());
    agent.bind_root_document(&document).unwrap();
    agent.attach_document(&document).unwrap();
    let observer = ObserverId(1);
    agent
        .journal_mut()
        .observe(observer, target, attribute_options(false))
        .unwrap();
    let plan = attribute_plan(target);
    let mut operation = agent.prepare(&mut document, &plan).unwrap();
    assert_eq!(
        agent
            .with_pins_for_read(document.document_id(), |pins| pins.owner_counts(target))
            .unwrap(),
        Some((1, 0))
    );
    agent.queue_next_record(&mut operation, target).unwrap();
    assert!(agent.commit(operation));
    assert_eq!(agent.journal().queued_count(observer), 1);
    let records = agent.journal_mut().take_records(observer);
    for mut record in records {
        agent
            .with_pins_for(document.document_id(), |pins| {
                record.release(pins, &mut document)
            })
            .unwrap()
            .unwrap();
    }
    assert!(
        agent
            .with_pins_for_read(document.document_id(), ObserverPins::is_empty)
            .unwrap()
    );
    drop(agent.begin_teardown().unwrap());
    agent
        .release_document_pins_on_teardown(&mut document)
        .unwrap();
    agent.finish_teardown().unwrap();
    drop(agent);
    drop(document);
    assert_eq!(budget.used(NativeMemoryComponent::MutationObserver), 0);
}

#[test]
fn teardown_drops_undelivered_and_failed_work_before_releasing_a_retained_document() {
    use std::cell::RefCell;
    use std::rc::Rc;

    let shared = Rc::new(RefCell::new(Document::new()));
    let budget = NativeMemoryBudget::with_limits(1024 * 1024, 1024 * 1024, 1024 * 1024, 0);
    let mut agent = ObserverAgent::new(budget.clone());
    let (element, target) = {
        let mut document = shared.borrow_mut();
        let element = document.create_html_element(atoms::DIV);
        let target = node(&document, element);
        agent.bind_root_document(&document).unwrap();
        agent.attach_document(&document).unwrap();
        agent
            .journal_mut()
            .observe(ObserverId(40), target, attribute_options(false))
            .unwrap();

        let mut queued = agent
            .prepare(&mut document, &attribute_plan(target))
            .unwrap();
        agent.queue_next_record(&mut queued, target).unwrap();
        assert!(agent.commit(queued));
        let failed = agent
            .prepare(&mut document, &attribute_plan(target))
            .unwrap();
        agent.retain_panicked_operation(failed);
        assert_eq!(
            agent
                .with_pins_for_read(document.document_id(), |pins| pins.owner_counts(target))
                .unwrap(),
            Some((2, 0))
        );
        assert_eq!(document.handle_count(element), 1);
        assert_eq!(document.sweep_detached().nodes_freed, 0);
        (element, target)
    };

    // The world first disables entry, then moves owners out under its short
    // agent lock. Their buffers and charges are dropped after that lock and
    // every Document borrow have ended.
    let work = agent.begin_teardown().unwrap();
    assert!(matches!(
        agent.check_document(&shared.borrow()),
        Err(Error::Crashed { .. })
    ));
    assert_eq!(agent.journal().queued_count(ObserverId(40)), 0);
    drop(work);
    assert!(matches!(
        agent.finish_teardown(),
        Err(Error::Crashed { .. })
    ));

    {
        let mut document = shared.borrow_mut();
        assert_eq!(
            agent
                .with_pins_for_read(document.document_id(), |pins| pins.owner_counts(target))
                .unwrap(),
            Some((2, 0))
        );
        agent
            .release_document_pins_on_teardown(&mut document)
            .unwrap();
        assert_eq!(document.handle_count(element), 0);
        assert_eq!(document.sweep_detached().nodes_freed, 1);
        let next = document.create_html_element(atoms::SPAN);
        assert!(document.resolve(document.node_ref(next)).is_ok());
    }
    agent.finish_teardown().unwrap();
    drop(agent);
    drop(shared);
    assert_eq!(budget.used(NativeMemoryComponent::MutationObserver), 0);
}

#[test]
fn aborted_unlink_removes_transients_and_keeps_registration() {
    let mut document = Document::new();
    let ancestor = document.create_html_element(atoms::DIV);
    let child = document.create_html_element(atoms::SPAN);
    let ancestor = node(&document, ancestor);
    let child = node(&document, child);
    let budget = NativeMemoryBudget::with_limits(1024 * 1024, 1024 * 1024, 1024 * 1024, 0);
    let mut agent = ObserverAgent::new(budget.clone());
    agent.bind_root_document(&document).unwrap();
    agent.attach_document(&document).unwrap();
    agent
        .journal_mut()
        .observe(ObserverId(2), ancestor, attribute_options(true))
        .unwrap();
    let plan = SemanticPlan {
        records: vec![],
        transient_removals: vec![TransientRemoval {
            removed_root: child,
            old_parent_ancestors: vec![ancestor],
        }],
        steps: vec![SemanticStep::InstallTransient(child)],
        request_notification: true,
    };
    let mut operation = agent.prepare(&mut document, &plan).unwrap();
    agent.install_next_transient(&mut operation, child).unwrap();
    agent.abort(&mut document, operation).unwrap();
    assert_eq!(agent.journal().queued_count(ObserverId(2)), 0);
    assert!(
        agent
            .with_pins_for_read(document.document_id(), ObserverPins::is_empty)
            .unwrap()
    );
    drop(agent.begin_teardown().unwrap());
    agent
        .release_document_pins_on_teardown(&mut document)
        .unwrap();
    agent.finish_teardown().unwrap();
    drop(agent);
    drop(document);
    assert_eq!(budget.used(NativeMemoryComponent::MutationObserver), 0);
}

#[test]
fn one_agent_orders_observers_across_two_documents_by_first_record() {
    let mut first = Document::new();
    let mut second = Document::new();
    let first_element = first.create_html_element(atoms::DIV);
    let second_element = second.create_html_element(atoms::SPAN);
    let first_target = node(&first, first_element);
    let second_target = node(&second, second_element);
    let budget = NativeMemoryBudget::with_limits(1024 * 1024, 1024 * 1024, 1024 * 1024, 0);
    let mut agent = ObserverAgent::new(budget.clone());
    agent.bind_root_document(&first).unwrap();
    agent.attach_document(&first).unwrap();
    agent.attach_document(&second).unwrap();
    let observer_a = ObserverId(10);
    let observer_b = ObserverId(11);
    agent
        .journal_mut()
        .observe(observer_a, first_target, attribute_options(false))
        .unwrap();
    agent
        .journal_mut()
        .observe(observer_b, second_target, attribute_options(false))
        .unwrap();

    for (document, target) in [(&mut second, second_target), (&mut first, first_target)] {
        let plan = attribute_plan(target);
        let mut operation = agent.prepare(document, &plan).unwrap();
        agent.queue_next_record(&mut operation, target).unwrap();
        assert!(agent.commit(operation));
    }
    assert_eq!(agent.notify_demand(), NotifyDemand::Requested);
    agent.mark_notify_enqueued().unwrap();
    let pending: Vec<_> = agent.begin_notify().unwrap().into_iter().collect();
    assert_eq!(pending, vec![observer_b, observer_a]);
    assert_eq!(agent.notify_demand(), NotifyDemand::Idle);

    for (observer, document) in [(observer_b, &mut second), (observer_a, &mut first)] {
        for mut record in agent.journal_mut().drain_for_notify(observer) {
            agent
                .with_pins_for(document.document_id(), |pins| {
                    record.release(pins, document)
                })
                .unwrap()
                .unwrap();
        }
    }
    drop(agent.begin_teardown().unwrap());
    agent.release_document_pins_on_teardown(&mut first).unwrap();
    agent
        .release_document_pins_on_teardown(&mut second)
        .unwrap();
    agent.finish_teardown().unwrap();
    drop(agent);
    drop(first);
    drop(second);
    assert_eq!(budget.used(NativeMemoryComponent::MutationObserver), 0);
}

#[test]
fn a_semantic_queue_request_survives_empty_selection_and_take_records() {
    let mut document = Document::new();
    let element = document.create_html_element(atoms::DIV);
    let target = node(&document, element);
    let budget = NativeMemoryBudget::with_limits(1024 * 1024, 1024 * 1024, 1024 * 1024, 0);
    let mut agent = ObserverAgent::new(budget.clone());
    agent.bind_root_document(&document).unwrap();
    agent.attach_document(&document).unwrap();
    let no_match = SemanticPlan::notification_only(true);
    let operation = agent.prepare(&mut document, &no_match).unwrap();
    assert!(agent.commit(operation));
    assert_eq!(agent.notify_demand(), NotifyDemand::Requested);
    agent.mark_notify_enqueued().unwrap();
    assert!(agent.begin_notify().unwrap().is_empty());

    let observer = ObserverId(12);
    agent
        .journal_mut()
        .observe(observer, target, attribute_options(false))
        .unwrap();
    let plan = attribute_plan(target);
    let mut operation = agent.prepare(&mut document, &plan).unwrap();
    agent.queue_next_record(&mut operation, target).unwrap();
    assert!(agent.commit(operation));
    for mut record in agent.journal_mut().take_records(observer) {
        agent
            .with_pins_for(document.document_id(), |pins| {
                record.release(pins, &mut document)
            })
            .unwrap()
            .unwrap();
    }
    assert_eq!(agent.notify_demand(), NotifyDemand::Requested);
    agent.mark_notify_enqueued().unwrap();
    assert_eq!(
        agent
            .begin_notify()
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>(),
        vec![observer]
    );
    assert!(agent.journal_mut().drain_for_notify(observer).is_empty());
    drop(agent.begin_teardown().unwrap());
    agent
        .release_document_pins_on_teardown(&mut document)
        .unwrap();
    agent.finish_teardown().unwrap();
    drop(agent);
    drop(document);
    assert_eq!(budget.used(NativeMemoryComponent::MutationObserver), 0);
}

#[test]
fn transients_install_at_each_exact_unlink_and_abort_reverses_them() {
    let mut document = Document::new();
    let ancestor = document.create_html_element(atoms::DIV);
    let first = document.create_html_element(atoms::SPAN);
    let second = document.create_html_element(atoms::SPAN);
    let ancestor = node(&document, ancestor);
    let first = node(&document, first);
    let second = node(&document, second);
    let budget = NativeMemoryBudget::with_limits(1024 * 1024, 1024 * 1024, 1024 * 1024, 0);
    let mut agent = ObserverAgent::new(budget.clone());
    agent.bind_root_document(&document).unwrap();
    agent.attach_document(&document).unwrap();
    agent
        .journal_mut()
        .observe(ObserverId(13), ancestor, attribute_options(true))
        .unwrap();
    let plan = SemanticPlan {
        records: vec![],
        transient_removals: vec![
            TransientRemoval {
                removed_root: first,
                old_parent_ancestors: vec![ancestor],
            },
            TransientRemoval {
                removed_root: second,
                old_parent_ancestors: vec![ancestor],
            },
        ],
        steps: vec![
            SemanticStep::InstallTransient(first),
            SemanticStep::InstallTransient(second),
        ],
        request_notification: true,
    };
    let mut operation = agent.prepare(&mut document, &plan).unwrap();
    assert!(
        agent
            .install_next_transient(&mut operation, second)
            .is_err()
    );
    agent.install_next_transient(&mut operation, first).unwrap();
    assert!(agent.journal().may_observe_path(
        [first],
        RecordType::Attributes,
        Some(atoms::CLASS),
        None,
    ));
    assert!(!agent.journal().may_observe_path(
        [second],
        RecordType::Attributes,
        Some(atoms::CLASS),
        None,
    ));
    agent.abort(&mut document, operation).unwrap();
    assert!(!agent.journal().may_observe_path(
        [first],
        RecordType::Attributes,
        Some(atoms::CLASS),
        None,
    ));
    assert!(!agent.journal().may_observe_path(
        [second],
        RecordType::Attributes,
        Some(atoms::CLASS),
        None,
    ));
    drop(agent.begin_teardown().unwrap());
    agent
        .release_document_pins_on_teardown(&mut document)
        .unwrap();
    agent.finish_teardown().unwrap();
    drop(agent);
    drop(document);
    assert_eq!(budget.used(NativeMemoryComponent::MutationObserver), 0);
}

#[test]
fn record_before_later_transient_keeps_pending_activation_order() {
    let mut document = Document::new();
    let source = document.create_html_element(atoms::DIV);
    let destination = document.create_html_element(atoms::DIV);
    let moving = document.create_html_element(atoms::SPAN);
    let removed = document.create_html_element(atoms::A);
    document
        .mutate(crate::mutation::Mutation::Append {
            parent: source,
            child: moving,
        })
        .unwrap();
    document
        .mutate(crate::mutation::Mutation::Append {
            parent: destination,
            child: removed,
        })
        .unwrap();
    let source_key = node(&document, source);
    let destination_key = node(&document, destination);
    let moving_key = node(&document, moving);
    let removed_key = node(&document, removed);
    let budget = NativeMemoryBudget::with_limits(1024 * 1024, 1024 * 1024, 1024 * 1024, 0);
    let mut agent = ObserverAgent::new(budget.clone());
    agent.bind_root_document(&document).unwrap();
    agent.attach_document(&document).unwrap();
    let first = ObserverId(20);
    let second = ObserverId(21);
    agent
        .journal_mut()
        .observe(first, source_key, child_list_options())
        .unwrap();
    agent
        .journal_mut()
        .observe(second, destination_key, attribute_options(true))
        .unwrap();
    let plan = SemanticPlan {
        records: vec![MutationFacts::child_list(
            source_key,
            vec![source_key],
            Vec::new(),
            vec![moving_key],
            None,
            None,
        )],
        transient_removals: vec![TransientRemoval {
            removed_root: removed_key,
            old_parent_ancestors: vec![destination_key],
        }],
        steps: vec![
            SemanticStep::QueueRecord(source_key),
            SemanticStep::InstallTransient(removed_key),
        ],
        request_notification: true,
    };
    let mut operation = agent.prepare(&mut document, &plan).unwrap();
    document
        .mutate(crate::mutation::Mutation::Remove { child: moving })
        .unwrap();
    agent.queue_next_record(&mut operation, source_key).unwrap();
    agent
        .install_next_transient(&mut operation, removed_key)
        .unwrap();
    document
        .mutate(crate::mutation::Mutation::Remove { child: removed })
        .unwrap();
    agent.confirm_unlink(&mut operation, removed_key).unwrap();
    assert!(agent.commit(operation));
    agent.mark_notify_enqueued().unwrap();
    assert_eq!(
        agent
            .begin_notify()
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>(),
        vec![first, second]
    );
    for mut record in agent.journal_mut().drain_for_notify(first) {
        agent
            .with_pins_for(document.document_id(), |pins| {
                record.release(pins, &mut document)
            })
            .unwrap()
            .unwrap();
    }
    assert!(agent.journal_mut().drain_for_notify(second).is_empty());
    drop(agent.begin_teardown().unwrap());
    agent
        .release_document_pins_on_teardown(&mut document)
        .unwrap();
    agent.finish_teardown().unwrap();
    drop(agent);
    drop(document);
    assert_eq!(budget.used(NativeMemoryComponent::MutationObserver), 0);
}

#[test]
fn cross_parent_replacement_activates_source_record_before_target_transient() {
    let mut document = Document::new();
    let source = document.create_html_element(atoms::DIV);
    let target = document.create_html_element(atoms::DIV);
    let replacement = document.create_html_element(atoms::SPAN);
    let old = document.create_html_element(atoms::A);
    document
        .mutate(crate::mutation::Mutation::Append {
            parent: source,
            child: replacement,
        })
        .unwrap();
    document
        .mutate(crate::mutation::Mutation::Append {
            parent: target,
            child: old,
        })
        .unwrap();
    let source_key = node(&document, source);
    let target_key = node(&document, target);
    let replacement_key = node(&document, replacement);
    let old_key = node(&document, old);
    let budget = NativeMemoryBudget::with_limits(1024 * 1024, 1024 * 1024, 1024 * 1024, 0);
    let mut agent = ObserverAgent::new(budget.clone());
    agent.bind_root_document(&document).unwrap();
    agent.attach_document(&document).unwrap();
    // B is created/registered first; the source record must activate A first.
    let observer_b = ObserverId(30);
    let observer_a = ObserverId(31);
    agent
        .journal_mut()
        .observe(observer_b, target_key, attribute_options(true))
        .unwrap();
    agent
        .journal_mut()
        .observe(observer_a, source_key, child_list_options())
        .unwrap();
    let plan = capture_validated(
        &document,
        &crate::mutation::Mutation::ReplaceChild {
            parent: target,
            new_child: replacement,
            old_child: old,
        },
    )
    .unwrap();
    assert_eq!(
        plan.steps,
        vec![
            SemanticStep::InstallTransient(replacement_key),
            SemanticStep::QueueRecord(source_key),
            SemanticStep::InstallTransient(old_key),
            SemanticStep::QueueRecord(target_key),
        ]
    );
    let mut operation = agent.prepare(&mut document, &plan).unwrap();
    agent
        .install_next_transient(&mut operation, replacement_key)
        .unwrap();
    document
        .mutate(crate::mutation::Mutation::Remove { child: replacement })
        .unwrap();
    agent
        .confirm_unlink(&mut operation, replacement_key)
        .unwrap();
    agent.queue_next_record(&mut operation, source_key).unwrap();
    agent
        .install_next_transient(&mut operation, old_key)
        .unwrap();
    document
        .mutate(crate::mutation::Mutation::Remove { child: old })
        .unwrap();
    agent.confirm_unlink(&mut operation, old_key).unwrap();
    document
        .mutate(crate::mutation::Mutation::Append {
            parent: target,
            child: replacement,
        })
        .unwrap();
    agent.queue_next_record(&mut operation, target_key).unwrap();
    assert!(agent.commit(operation));
    agent.mark_notify_enqueued().unwrap();
    assert_eq!(
        agent
            .begin_notify()
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>(),
        vec![observer_a, observer_b]
    );
    for mut record in agent.journal_mut().drain_for_notify(observer_a) {
        agent
            .with_pins_for(document.document_id(), |pins| {
                record.release(pins, &mut document)
            })
            .unwrap()
            .unwrap();
    }
    assert!(agent.journal_mut().drain_for_notify(observer_b).is_empty());
    drop(agent.begin_teardown().unwrap());
    agent
        .release_document_pins_on_teardown(&mut document)
        .unwrap();
    agent.finish_teardown().unwrap();
    drop(agent);
    drop(document);
    assert_eq!(budget.used(NativeMemoryComponent::MutationObserver), 0);
}

#[test]
fn failed_second_unlink_keeps_first_record_and_releases_unperformed_suffix() {
    let mut document = Document::new();
    let source = document.create_html_element(atoms::DIV);
    let target = document.create_html_element(atoms::DIV);
    let replacement = document.create_html_element(atoms::SPAN);
    let old = document.create_html_element(atoms::A);
    document
        .mutate(crate::mutation::Mutation::Append {
            parent: source,
            child: replacement,
        })
        .unwrap();
    document
        .mutate(crate::mutation::Mutation::Append {
            parent: target,
            child: old,
        })
        .unwrap();
    let source_key = node(&document, source);
    let target_key = node(&document, target);
    let replacement_key = node(&document, replacement);
    let old_key = node(&document, old);
    let budget = NativeMemoryBudget::with_limits(1024 * 1024, 1024 * 1024, 1024 * 1024, 0);
    let mut agent = ObserverAgent::new(budget.clone());
    agent.bind_root_document(&document).unwrap();
    agent.attach_document(&document).unwrap();
    let source_observer = ObserverId(40);
    let transient_observer = ObserverId(41);
    let target_observer = ObserverId(42);
    agent
        .journal_mut()
        .observe(source_observer, source_key, child_list_options())
        .unwrap();
    agent
        .journal_mut()
        .observe(transient_observer, target_key, attribute_options(true))
        .unwrap();
    agent
        .journal_mut()
        .observe(target_observer, target_key, child_list_options())
        .unwrap();
    let plan = capture_validated(
        &document,
        &crate::mutation::Mutation::ReplaceChild {
            parent: target,
            new_child: replacement,
            old_child: old,
        },
    )
    .unwrap();
    let mut operation = agent.prepare(&mut document, &plan).unwrap();
    agent
        .install_next_transient(&mut operation, replacement_key)
        .unwrap();
    document
        .mutate(crate::mutation::Mutation::Remove { child: replacement })
        .unwrap();
    agent
        .confirm_unlink(&mut operation, replacement_key)
        .unwrap();
    agent.queue_next_record(&mut operation, source_key).unwrap();
    agent
        .install_next_transient(&mut operation, old_key)
        .unwrap();
    // Model a range-fixup refusal before the second native unlink changes
    // links. The first source removal and its record are already observable.
    agent
        .finish_partial_failure(&mut document, operation)
        .unwrap();
    assert_eq!(document.parent(old), target);
    assert_eq!(agent.journal().queued_count(source_observer), 1);
    assert_eq!(agent.journal().queued_count(target_observer), 0);
    assert_eq!(
        agent
            .with_pins_for_read(document.document_id(), |pins| pins.owner_counts(old_key))
            .unwrap(),
        None
    );
    assert_eq!(agent.notify_demand(), NotifyDemand::Requested);
    agent.mark_notify_enqueued().unwrap();
    assert_eq!(
        agent
            .begin_notify()
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>(),
        vec![source_observer]
    );
    for mut record in agent.journal_mut().drain_for_notify(source_observer) {
        agent
            .with_pins_for(document.document_id(), |pins| {
                record.release(pins, &mut document)
            })
            .unwrap()
            .unwrap();
    }
    drop(agent.begin_teardown().unwrap());
    agent
        .release_document_pins_on_teardown(&mut document)
        .unwrap();
    agent.finish_teardown().unwrap();
    drop(agent);
    drop(document);
    assert_eq!(budget.used(NativeMemoryComponent::MutationObserver), 0);
}

#[test]
fn compound_range_preflight_runs_after_observer_pins_are_acquired() {
    let mut document = Document::new();
    let source = document.create_html_element(atoms::DIV);
    let target = document.create_html_element(atoms::DIV);
    let replacement = document.create_html_element(atoms::SPAN);
    let old = document.create_text("old");
    document
        .mutate(crate::mutation::Mutation::Append {
            parent: source,
            child: replacement,
        })
        .unwrap();
    document
        .mutate(crate::mutation::Mutation::Append {
            parent: target,
            child: old,
        })
        .unwrap();
    let point = range_point(&document, old);
    let live = document.register_live_range(point, point).unwrap();
    // Without the observer's new target pin, two replacement range handles
    // still fit. After observer preflight, only one slot remains.
    let external: Vec<_> = (0..u16::MAX - 2)
        .map(|_| document.handle(target).unwrap())
        .collect();
    let target_key = node(&document, target);
    let budget = NativeMemoryBudget::with_limits(1024 * 1024, 0, 1024 * 1024, 0);
    let mut agent = ObserverAgent::new(budget.clone());
    agent.bind_root_document(&document).unwrap();
    agent.attach_document(&document).unwrap();
    let observer = ObserverId(43);
    agent
        .journal_mut()
        .observe(observer, target_key, child_list_options())
        .unwrap();
    let mutation = crate::mutation::Mutation::ReplaceChild {
        parent: target,
        new_child: replacement,
        old_child: old,
    };
    let plan = capture_validated(&document, &mutation).unwrap();
    let operation = agent.prepare(&mut document, &plan).unwrap();
    assert_eq!(document.handle_count(target), u16::MAX - 1);
    let points_before = document.live_range_points(live);
    let error = document.mutate(mutation).unwrap_err();
    assert!(matches!(
        error,
        tenta_core::Error::Other { code, message }
            if code == "InvalidStateError"
                && message == "live range fixup exceeds node handle capacity"
    ));
    agent.abort(&mut document, operation).unwrap();
    assert_eq!(document.children(source).collect::<Vec<_>>(), [replacement]);
    assert_eq!(document.children(target).collect::<Vec<_>>(), [old]);
    assert_eq!(document.live_range_points(live), points_before);
    assert_eq!(agent.journal().queued_count(observer), 0);
    assert_eq!(document.handle_count(target), u16::MAX - 2);
    for handle in external {
        document.release(handle);
    }
    document.release_live_range(live);
    drop(agent.begin_teardown().unwrap());
    agent
        .release_document_pins_on_teardown(&mut document)
        .unwrap();
    agent.finish_teardown().unwrap();
    drop(agent);
    drop(document);
    assert_eq!(budget.used(NativeMemoryComponent::MutationObserver), 0);
}

fn range_point(document: &Document, node: tenta_core::NodeId) -> crate::RangeBoundary {
    crate::RangeBoundary {
        node: document.node_ref(node),
        offset: 0,
    }
}
