use super::*;
use tenta_core::atom::atoms;
use tenta_html::{Attribute, NodeOrText, QualName, TreeSink};

use crate::RangeBoundary;
use crate::document::observer_agent::NotifyDemand;
use crate::document::observer_journal::{
    ObserveOptions, ObserveOptionsInput, ObserverId, RecordType, SemanticRecord,
};

fn attached_document() -> (Document, SharedObserverAgent) {
    let mut document = Document::new();
    let budget = NativeMemoryBudget::with_limits(1024 * 1024, 0, 1024 * 1024, 0);
    let agent = Arc::new(Mutex::new(ObserverAgent::new(budget)));
    document.attach_root_observer_agent(agent.clone()).unwrap();
    (document, agent)
}

fn observe(
    document: &Document,
    agent: &SharedObserverAgent,
    id: u64,
    node: NodeId,
    child_list: bool,
) {
    let options = ObserveOptions::normalize(ObserveOptionsInput {
        child_list,
        attributes: (!child_list).then_some(true),
        attribute_old_value: (!child_list).then_some(true),
        ..Default::default()
    })
    .unwrap();
    agent
        .lock()
        .journal_mut()
        .observe(
            ObserverId(id),
            ObservedMutation::key(document, node),
            options,
        )
        .unwrap();
}

fn release_on_teardown(document: &mut Document, agent: &SharedObserverAgent) {
    let work = agent.lock().begin_teardown().unwrap();
    drop(work);
    agent
        .lock()
        .release_document_pins_on_teardown(document)
        .unwrap();
    agent.lock().finish_teardown().unwrap();
}

fn take_records_and_release(
    document: &mut Document,
    agent: &SharedObserverAgent,
    observer: ObserverId,
) -> Vec<SemanticRecord> {
    let mut state = agent.lock();
    let records = state.journal_mut().take_records(observer);
    let snapshots = records
        .iter()
        .map(|record| record.record().clone())
        .collect();
    for mut record in records {
        state
            .with_pins_for(document.document_id(), |pins| {
                record.release(pins, document)
            })
            .expect("observer has its exact Document pin ledger")
            .expect("a delivered native record releases its exact pins");
    }
    snapshots
}

#[test]
fn parser_text_merge_queues_character_data_old_value_without_replacing_rope() {
    let (mut document, agent) = attached_document();
    let parent = document.create_html_element(atoms::DIV);
    let text = document.create_text("first");
    document
        .mutate(Mutation::Append {
            parent,
            child: text,
        })
        .unwrap();
    let options = ObserveOptions::normalize(ObserveOptionsInput {
        character_data: Some(true),
        character_data_old_value: Some(true),
        ..Default::default()
    })
    .unwrap();
    agent
        .lock()
        .journal_mut()
        .observe(
            ObserverId(81),
            ObservedMutation::key(&document, text),
            options,
        )
        .unwrap();

    TreeSink::append(&mut document, &parent, NodeOrText::Text(" second"));
    assert_eq!(document.text_data(text), Some("first second"));
    assert_eq!(document.children(parent).collect::<Vec<_>>(), vec![text]);
    let records = take_records_and_release(&mut document, &agent, ObserverId(81));
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].record_type, RecordType::CharacterData);
    assert_eq!(records[0].target, ObservedMutation::key(&document, text));
    assert_eq!(records[0].old_value.as_deref(), Some("first"));
    release_on_teardown(&mut document, &agent);
}

#[test]
fn parser_first_text_insertion_queues_child_list_record() {
    let (mut document, agent) = attached_document();
    let parent = document.create_html_element(atoms::DIV);
    observe(&document, &agent, 82, parent, true);

    TreeSink::append(&mut document, &parent, NodeOrText::Text("first"));
    let text = document.first_child(parent);
    assert_eq!(document.text_data(text), Some("first"));
    let records = take_records_and_release(&mut document, &agent, ObserverId(82));
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].record_type, RecordType::ChildList);
    assert_eq!(records[0].target, ObservedMutation::key(&document, parent));
    assert_eq!(
        records[0].added_nodes,
        vec![ObservedMutation::key(&document, text)]
    );
    assert!(records[0].removed_nodes.is_empty());
    release_on_teardown(&mut document, &agent);
}

#[test]
fn parser_bulk_missing_attributes_keep_one_pass_and_queue_each_added_name() {
    let (mut document, agent) = attached_document();
    let element = document.create_html_element(atoms::BODY);
    observe(&document, &agent, 83, element, false);
    let name = |local| QualName::new(atoms::EMPTY, local);
    TreeSink::add_attrs_if_missing(
        &mut document,
        &element,
        vec![
            Attribute {
                name: name(atoms::ID),
                value: "kept".to_owned(),
            },
            Attribute {
                name: name(atoms::CLASS),
                value: "one".to_owned(),
            },
            Attribute {
                name: name(atoms::ID),
                value: "ignored".to_owned(),
            },
        ],
    );
    assert_eq!(
        document.attribute_ns(element, atoms::EMPTY, atoms::ID),
        Some("kept")
    );
    assert_eq!(
        document.attribute_ns(element, atoms::EMPTY, atoms::CLASS),
        Some("one")
    );
    let records = take_records_and_release(&mut document, &agent, ObserverId(83));
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].attribute_name, Some(atoms::ID));
    assert_eq!(records[1].attribute_name, Some(atoms::CLASS));
    assert!(
        records.iter().all(
            |record| record.record_type == RecordType::Attributes && record.old_value.is_none()
        )
    );
    release_on_teardown(&mut document, &agent);
}

#[test]
fn parser_text_before_sibling_merges_into_previous_text_and_queues_old_value() {
    let (mut document, agent) = attached_document();
    let parent = document.create_html_element(atoms::DIV);
    let text = document.create_text("left");
    let sibling = document.create_html_element(atoms::SPAN);
    document
        .mutate(Mutation::Append {
            parent,
            child: text,
        })
        .unwrap();
    document
        .mutate(Mutation::Append {
            parent,
            child: sibling,
        })
        .unwrap();
    let options = ObserveOptions::normalize(ObserveOptionsInput {
        character_data: Some(true),
        character_data_old_value: Some(true),
        ..Default::default()
    })
    .unwrap();
    agent
        .lock()
        .journal_mut()
        .observe(
            ObserverId(84),
            ObservedMutation::key(&document, text),
            options,
        )
        .unwrap();

    TreeSink::append_before_sibling(&mut document, &sibling, NodeOrText::Text(" right"));
    assert_eq!(document.text_data(text), Some("left right"));
    assert_eq!(document.next_sibling(text), sibling);
    let records = take_records_and_release(&mut document, &agent, ObserverId(84));
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].record_type, RecordType::CharacterData);
    assert_eq!(records[0].old_value.as_deref(), Some("left"));
    release_on_teardown(&mut document, &agent);
}

#[test]
fn sweep_releases_retired_world_pins_without_an_intervening_mutation() {
    use std::cell::RefCell;
    use std::rc::Rc;

    let shared = Rc::new(RefCell::new(Document::new()));
    let budget = NativeMemoryBudget::with_limits(1024 * 1024, 0, 1024 * 1024, 0);
    let (agent, attachment, target, reference) = {
        let mut document = shared.borrow_mut();
        let (agent, attachment) = document.new_observer_world_owned(budget.clone());
        let target = document.create_html_element(atoms::DIV);
        observe(&document, &agent, 85, target, false);
        document
            .mutate(Mutation::SetAttribute {
                element: target,
                name: QualName::new(atoms::EMPTY, atoms::CLASS),
                value: "retained".to_owned(),
            })
            .unwrap();
        assert_eq!(document.handle_count(target), 1);
        let reference = document.node_ref(target);
        (agent, attachment, target, reference)
    };
    let work = agent.lock().begin_teardown().unwrap();
    drop(work);
    let held = shared.borrow();
    drop(attachment);
    assert_eq!(held.handle_count(target), 1);
    drop(held);

    assert_eq!(shared.borrow_mut().sweep_detached().nodes_freed, 1);
    assert!(shared.borrow().resolve(reference).is_err());
    agent.lock().finish_teardown().unwrap();
    drop(agent);
    drop(shared);
    assert_eq!(
        budget.used(tenta_core::NativeMemoryComponent::MutationObserver),
        0
    );
}

#[test]
fn two_independent_worlds_queue_and_pin_the_same_document_separately() {
    let mut document = Document::new();
    let parent = document.create_html_element(atoms::DIV);
    let child = document.create_html_element(atoms::SPAN);
    let first_budget = NativeMemoryBudget::with_limits(1024 * 1024, 0, 1024 * 1024, 0);
    let second_budget = NativeMemoryBudget::with_limits(1024 * 1024, 0, 1024 * 1024, 0);
    let first = Arc::new(Mutex::new(ObserverAgent::new(first_budget.clone())));
    let second = Arc::new(Mutex::new(ObserverAgent::new(second_budget.clone())));
    document.attach_root_observer_agent(first.clone()).unwrap();
    document.attach_root_observer_agent(second.clone()).unwrap();
    observe(&document, &first, 21, parent, true);
    observe(&document, &second, 22, parent, true);

    document.mutate(Mutation::Append { parent, child }).unwrap();
    assert_eq!(document.parent(child), parent);
    assert_eq!(first.lock().journal().queued_count(ObserverId(21)), 1);
    assert_eq!(second.lock().journal().queued_count(ObserverId(22)), 1);
    let target = ObservedMutation::key(&document, parent);
    for agent in [&first, &second] {
        let registration = Arc::clone(
            agent
                .lock()
                .registration_for(document.document_id())
                .unwrap(),
        );
        assert_eq!(
            registration
                .with_active_pins_read(|pins| pins.owner_counts(target))
                .unwrap(),
            Some((1, 0))
        );
    }
    assert_eq!(document.handle_count(parent), 2);
    release_on_teardown(&mut document, &first);
    release_on_teardown(&mut document, &second);
    assert_eq!(document.handle_count(parent), 0);
    drop(document);
    drop(first);
    drop(second);
    assert_eq!(
        first_budget.used(tenta_core::NativeMemoryComponent::MutationObserver),
        0
    );
    assert_eq!(
        second_budget.used(tenta_core::NativeMemoryComponent::MutationObserver),
        0
    );
}

#[test]
fn selected_content_stops_on_typed_observer_failure_without_losing_existing_child() {
    let mut document = Document::new();
    let select = document.create_html_element(atoms::SELECT);
    let target = document.create_html_element(atoms::SELECTEDCONTENT);
    let option = document.create_html_element(atoms::OPTION);
    let retained = document.create_text("retained");
    let source = document.create_text("source");
    for (parent, child) in [
        (select, target),
        (select, option),
        (target, retained),
        (option, source),
    ] {
        document.mutate(Mutation::Append { parent, child }).unwrap();
    }
    document
        .mutate(Mutation::SetAttribute {
            element: option,
            name: QualName::new(atoms::EMPTY, atoms::SELECTED),
            value: String::new(),
        })
        .unwrap();
    let budget = NativeMemoryBudget::with_limits(1024 * 1024, 0, 1024 * 1024, 0);
    let agent = Arc::new(Mutex::new(ObserverAgent::new(budget.clone())));
    document.attach_root_observer_agent(agent.clone()).unwrap();
    observe(&document, &agent, 86, target, true);
    budget
        .begin(tenta_core::NativeMemoryComponent::MutationObserver)
        .fail_allocation();

    assert!(matches!(
        document.clone_option_into_selectedcontent(option),
        Err(Error::BudgetExceeded { .. })
    ));
    assert_eq!(document.first_child(target), retained);
    assert_eq!(document.next_sibling(retained), NodeId::NONE);
    TreeSink::maybe_clone_option_into_selectedcontent(&mut document, &option);
    assert!(
        document
            .parse_errors()
            .iter()
            .any(|message| message.contains("selected-content cloning refused"))
    );
    assert_eq!(
        TreeSink::pop(&mut document, &option),
        tenta_html::ParseControl::Stop
    );
    assert!(matches!(
        document.check_observer_health(),
        Err(Error::BudgetExceeded { .. })
    ));
    release_on_teardown(&mut document, &agent);
}

#[test]
fn later_world_admission_failure_rolls_back_earlier_preparation_without_a_dom_effect() {
    let mut document = Document::new();
    let parent = document.create_html_element(atoms::DIV);
    let child = document.create_html_element(atoms::SPAN);
    let good_budget = NativeMemoryBudget::with_limits(1024 * 1024, 0, 1024 * 1024, 0);
    let bad_budget = NativeMemoryBudget::with_limits(1024 * 1024, 0, 1024 * 1024, 0);
    let good = Arc::new(Mutex::new(ObserverAgent::new(good_budget.clone())));
    let bad = Arc::new(Mutex::new(ObserverAgent::new(bad_budget.clone())));
    // The registry visits its newest attachment first. The good world has
    // therefore already reserved and pinned before the bad world's refusal.
    document.attach_root_observer_agent(bad.clone()).unwrap();
    document.attach_root_observer_agent(good.clone()).unwrap();
    observe(&document, &good, 31, parent, true);
    observe(&document, &bad, 32, parent, true);
    bad_budget
        .begin(tenta_core::NativeMemoryComponent::MutationObserver)
        .fail_allocation();

    assert!(matches!(
        document.mutate(Mutation::Append { parent, child }),
        Err(Error::BudgetExceeded { .. })
    ));
    assert_eq!(document.parent(child), NodeId::NONE);
    assert_eq!(document.handle_count(parent), 0);
    assert_eq!(good.lock().journal().queued_count(ObserverId(31)), 0);
    assert_eq!(bad.lock().journal().queued_count(ObserverId(32)), 0);
    release_on_teardown(&mut document, &good);
    release_on_teardown(&mut document, &bad);
    drop(document);
    drop(good);
    drop(bad);
    assert_eq!(
        good_budget.used(tenta_core::NativeMemoryComponent::MutationObserver),
        0
    );
    assert_eq!(
        bad_budget.used(tenta_core::NativeMemoryComponent::MutationObserver),
        0
    );
}

#[test]
fn post_effect_failure_between_world_hooks_poison_both_and_retain_unconsumed_pins() {
    let mut document = Document::new();
    let parent = document.create_html_element(atoms::DIV);
    let child = document.create_html_element(atoms::SPAN);
    let budget = NativeMemoryBudget::with_limits(1024 * 1024, 0, 1024 * 1024, 0);
    let first = Arc::new(Mutex::new(ObserverAgent::new(budget.clone())));
    let second = Arc::new(Mutex::new(ObserverAgent::new(budget.clone())));
    document.attach_root_observer_agent(second.clone()).unwrap();
    document.attach_root_observer_agent(first.clone()).unwrap();
    observe(&document, &first, 41, parent, true);
    observe(&document, &second, 42, parent, true);

    let mutation = Mutation::Append { parent, child };
    let mut observed = document
        .prepare_observed_mutation(&mutation)
        .unwrap()
        .unwrap();
    observed.fail_after_world_hook = Some(1);
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        document
            .apply_observed_mutation(mutation, &mut observed)
            .unwrap();
    }));
    assert!(failure.is_err());
    observed.retain_after_panic();
    assert_eq!(document.parent(child), parent);
    assert_eq!(first.lock().journal().queued_count(ObserverId(41)), 1);
    assert_eq!(second.lock().journal().queued_count(ObserverId(42)), 0);
    for agent in [&first, &second] {
        assert!(matches!(
            agent.lock().check_native_health(),
            Err(Error::Crashed { .. })
        ));
    }
    let second_node = Arc::clone(
        second
            .lock()
            .registration_for(document.document_id())
            .unwrap(),
    );
    assert!(second_node.with_prepared(|_| ()).is_ok());
    assert!(document.handle_count(parent) >= 1);
    let later = document.create_html_element(atoms::A);
    assert!(matches!(
        document.mutate(Mutation::Append {
            parent,
            child: later
        }),
        Err(Error::Crashed { .. })
    ));
    assert_eq!(document.parent(later), NodeId::NONE);
    release_on_teardown(&mut document, &first);
    release_on_teardown(&mut document, &second);
    assert_eq!(document.handle_count(parent), 0);
    drop(observed);
    drop(document);
    drop(first);
    drop(second);
    drop(second_node);
    assert_eq!(
        budget.used(tenta_core::NativeMemoryComponent::MutationObserver),
        0
    );
}

#[test]
fn panic_between_world_failure_cleanups_poison_all_and_retains_second_owner() {
    let mut document = Document::new();
    let parent = document.create_html_element(atoms::DIV);
    let child = document.create_html_element(atoms::SPAN);
    let budget = NativeMemoryBudget::with_limits(1024 * 1024, 0, 1024 * 1024, 0);
    let first = Arc::new(Mutex::new(ObserverAgent::new(budget.clone())));
    let second = Arc::new(Mutex::new(ObserverAgent::new(budget.clone())));
    document.attach_root_observer_agent(second.clone()).unwrap();
    document.attach_root_observer_agent(first.clone()).unwrap();
    observe(&document, &first, 51, parent, true);
    observe(&document, &second, 52, parent, true);

    let mut observed = document
        .prepare_observed_mutation(&Mutation::Append { parent, child })
        .unwrap()
        .unwrap();
    document.link(parent, child, NodeId::NONE);
    observed.note_effect();
    observed.fail_after_world_cleanup = Some(1);
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = document.finish_observed_error(
            &mut observed,
            Error::invalid("injected later native step failure"),
        );
    }));
    assert!(failure.is_err());
    assert_eq!(document.parent(child), parent);
    for agent in [&first, &second] {
        assert!(matches!(
            agent.lock().check_native_health(),
            Err(Error::Crashed { .. })
        ));
    }
    let second_node = Arc::clone(
        second
            .lock()
            .registration_for(document.document_id())
            .unwrap(),
    );
    assert!(second_node.with_prepared(|_| ()).is_ok());
    assert!(document.handle_count(parent) >= 1);
    let later = document.create_html_element(atoms::A);
    assert!(matches!(
        document.mutate(Mutation::Append {
            parent,
            child: later
        }),
        Err(Error::Crashed { .. })
    ));
    assert_eq!(document.parent(later), NodeId::NONE);
    release_on_teardown(&mut document, &first);
    release_on_teardown(&mut document, &second);
    assert_eq!(document.handle_count(parent), 0);
    drop(observed);
    drop(document);
    drop(first);
    drop(second);
    drop(second_node);
    assert_eq!(
        budget.used(tenta_core::NativeMemoryComponent::MutationObserver),
        0
    );
}

#[test]
fn moving_between_parents_activates_source_then_destination() {
    let mut document = Document::new();
    let source = document.create_html_element(atoms::DIV);
    let destination = document.create_html_element(atoms::DIV);
    let child = document.create_html_element(atoms::SPAN);
    document
        .mutate(Mutation::Append {
            parent: source,
            child,
        })
        .unwrap();
    let budget = NativeMemoryBudget::with_limits(1024 * 1024, 0, 1024 * 1024, 0);
    let agent = Arc::new(Mutex::new(ObserverAgent::new(budget)));
    document.attach_root_observer_agent(agent.clone()).unwrap();
    // Reverse construction/registration order must not sort notifications.
    observe(&document, &agent, 2, destination, true);
    observe(&document, &agent, 1, source, true);

    document
        .mutate(Mutation::Append {
            parent: destination,
            child,
        })
        .unwrap();
    assert_eq!(document.parent(child), destination);
    assert_eq!(agent.lock().notify_demand(), NotifyDemand::Requested);
    let mut state = agent.lock();
    assert_eq!(state.journal().queued_count(ObserverId(1)), 1);
    assert_eq!(state.journal().queued_count(ObserverId(2)), 1);
    state.mark_notify_enqueued().unwrap();
    let pending = state.begin_notify().unwrap();
    assert_eq!(
        pending.iter().copied().collect::<Vec<_>>(),
        [ObserverId(1), ObserverId(2)]
    );
    drop(state);
    drop(pending);
    release_on_teardown(&mut document, &agent);
}

#[test]
fn same_attribute_value_still_queues_one_old_value_record() {
    let (mut document, agent) = attached_document();
    let element = document.create_html_element(atoms::DIV);
    let name = QualName::new(atoms::EMPTY, atoms::ID);
    document
        .mutate(Mutation::SetAttribute {
            element,
            name,
            value: "same".to_owned(),
        })
        .unwrap();
    observe(&document, &agent, 3, element, false);
    document
        .mutate(Mutation::SetAttribute {
            element,
            name,
            value: "same".to_owned(),
        })
        .unwrap();
    let mut state = agent.lock();
    assert_eq!(state.journal().queued_count(ObserverId(3)), 1);
    let records = state.journal_mut().take_records(ObserverId(3));
    let record = records[0].record();
    assert_eq!(record.record_type, RecordType::Attributes);
    assert_eq!(record.old_value.as_deref(), Some("same"));
    drop(state);
    release_on_teardown(&mut document, &agent);
    drop(records);
}

#[test]
fn replacing_with_empty_fragment_still_queues_combined_target_record() {
    let mut document = Document::new();
    let parent = document.create_html_element(atoms::DIV);
    let old = document.create_html_element(atoms::SPAN);
    let fragment = document.create_fragment();
    document
        .mutate(Mutation::Append { parent, child: old })
        .unwrap();
    let budget = NativeMemoryBudget::with_limits(1024 * 1024, 0, 1024 * 1024, 0);
    let agent = Arc::new(Mutex::new(ObserverAgent::new(budget)));
    document.attach_root_observer_agent(agent.clone()).unwrap();
    observe(&document, &agent, 4, parent, true);

    document
        .mutate(Mutation::ReplaceChild {
            parent,
            new_child: fragment,
            old_child: old,
        })
        .unwrap();
    assert_eq!(document.parent(old), NodeId::NONE);
    assert_eq!(agent.lock().journal().queued_count(ObserverId(4)), 1);
    release_on_teardown(&mut document, &agent);
}

#[test]
fn post_link_step_mismatch_retains_prepared_record_for_poison_teardown() {
    let (mut document, agent) = attached_document();
    let parent = document.create_html_element(atoms::DIV);
    let child = document.create_html_element(atoms::SPAN);
    let later = document.create_html_element(atoms::SPAN);
    observe(&document, &agent, 5, parent, true);
    let mutation = Mutation::Append { parent, child };
    let mut observed = document
        .prepare_observed_mutation(&mutation)
        .unwrap()
        .unwrap();
    assert!(document.handle_count(parent) > 0);
    let registration = agent
        .lock()
        .registration_for(document.document_id())
        .cloned()
        .unwrap();
    registration
        .with_prepared(|owner| owner.operation.discard_next_step_for_test())
        .unwrap();

    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        document
            .apply_observed_mutation(mutation, &mut observed)
            .unwrap();
    }));
    assert!(failure.is_err());
    observed.retain_after_panic();
    assert_eq!(document.parent(child), parent);
    assert!(matches!(
        agent.lock().check_native_health(),
        Err(Error::Crashed { .. })
    ));
    assert!(registration.with_prepared(|_| ()).is_ok());
    assert!(document.handle_count(parent) > 0);
    let error = document
        .mutate(Mutation::Append {
            parent,
            child: later,
        })
        .unwrap_err();
    assert!(matches!(error, Error::Crashed { .. }));
    assert_eq!(document.parent(later), NodeId::NONE);
    release_on_teardown(&mut document, &agent);
    assert_eq!(document.handle_count(parent), 0);
}

#[test]
fn incomplete_commit_retains_unpublished_record_before_panicking() {
    let (mut document, agent) = attached_document();
    let parent = document.create_html_element(atoms::DIV);
    let child = document.create_html_element(atoms::SPAN);
    observe(&document, &agent, 6, parent, true);
    let mut observed = document
        .prepare_observed_mutation(&Mutation::Append { parent, child })
        .unwrap()
        .unwrap();
    assert!(document.handle_count(parent) > 0);

    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| observed.commit()));
    assert!(failure.is_err());
    observed.retain_after_panic();
    assert_eq!(document.parent(child), NodeId::NONE);
    assert!(matches!(
        agent.lock().check_native_health(),
        Err(Error::Crashed { .. })
    ));
    assert!(document.handle_count(parent) > 0);
    release_on_teardown(&mut document, &agent);
    assert_eq!(document.handle_count(parent), 0);
}

#[test]
fn replace_all_queues_one_parent_record_for_same_content_and_empty() {
    let (mut document, agent) = attached_document();
    let parent = document.create_html_element(atoms::DIV);
    let old = document.create_text("same");
    document
        .mutate(Mutation::Append { parent, child: old })
        .unwrap();
    observe(&document, &agent, 91, parent, true);

    let replacement = document.create_text("same");
    document
        .mutate(Mutation::ReplaceAll {
            parent,
            replacement: ReplaceAllWith::Text(replacement),
        })
        .unwrap();
    assert_eq!(document.first_child(parent), replacement);
    assert_eq!(document.text_data(replacement), Some("same"));
    let records = take_records_and_release(&mut document, &agent, ObserverId(91));
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].record_type, RecordType::ChildList);
    assert_eq!(
        records[0].removed_nodes,
        vec![ObservedMutation::key(&document, old)]
    );
    assert_eq!(
        records[0].added_nodes,
        vec![ObservedMutation::key(&document, replacement)]
    );
    assert_eq!(records[0].previous_sibling, None);
    assert_eq!(records[0].next_sibling, None);
    assert_eq!(records[0].old_value, None);

    document
        .mutate(Mutation::ReplaceAll {
            parent,
            replacement: ReplaceAllWith::Empty,
        })
        .unwrap();
    let records = take_records_and_release(&mut document, &agent, ObserverId(91));
    assert_eq!(records.len(), 1);
    assert!(records[0].added_nodes.is_empty());
    assert_eq!(
        records[0].removed_nodes,
        vec![ObservedMutation::key(&document, replacement)]
    );
    assert_eq!(document.first_child(parent), NodeId::NONE);

    document
        .mutate(Mutation::ReplaceAll {
            parent,
            replacement: ReplaceAllWith::Empty,
        })
        .unwrap();
    assert_eq!(agent.lock().journal().queued_count(ObserverId(91)), 0);
    release_on_teardown(&mut document, &agent);
}

#[test]
fn replace_all_fragment_queues_source_then_one_grouped_parent_record() {
    let (mut document, agent) = attached_document();
    let parent = document.create_html_element(atoms::DIV);
    let old_a = document.create_html_element(atoms::A);
    let old_b = document.create_html_element(atoms::B);
    let source = document.create_fragment();
    let new_a = document.create_html_element(atoms::SPAN);
    let new_b = document.create_text("new");
    for child in [old_a, old_b] {
        document.mutate(Mutation::Append { parent, child }).unwrap();
    }
    for child in [new_a, new_b] {
        document
            .mutate(Mutation::Append {
                parent: source,
                child,
            })
            .unwrap();
    }
    observe(&document, &agent, 92, source, true);
    observe(&document, &agent, 93, parent, true);

    document
        .mutate(Mutation::ReplaceAll {
            parent,
            replacement: ReplaceAllWith::ChildrenOf(source),
        })
        .unwrap();
    assert_eq!(
        document.children(parent).collect::<Vec<_>>(),
        vec![new_a, new_b]
    );
    assert_eq!(document.first_child(source), NodeId::NONE);
    let source_records = take_records_and_release(&mut document, &agent, ObserverId(92));
    let parent_records = take_records_and_release(&mut document, &agent, ObserverId(93));
    assert_eq!(source_records.len(), 1);
    assert_eq!(parent_records.len(), 1);
    assert_eq!(source_records[0].removed_nodes.len(), 2);
    assert!(source_records[0].added_nodes.is_empty());
    assert_eq!(parent_records[0].removed_nodes.len(), 2);
    assert_eq!(parent_records[0].added_nodes.len(), 2);
    assert_eq!(parent_records[0].previous_sibling, None);
    assert_eq!(parent_records[0].next_sibling, None);
    assert_eq!(parent_records[0].old_value, None);
    release_on_teardown(&mut document, &agent);
}

#[test]
fn replace_all_refuses_second_world_budget_before_unlinking_any_child() {
    let mut document = Document::new();
    let parent = document.create_html_element(atoms::DIV);
    let old = document.create_html_element(atoms::A);
    let source = document.create_fragment();
    let new = document.create_html_element(atoms::SPAN);
    document
        .mutate(Mutation::Append { parent, child: old })
        .unwrap();
    document
        .mutate(Mutation::Append {
            parent: source,
            child: new,
        })
        .unwrap();
    let range_point = RangeBoundary {
        node: document.node_ref(old),
        offset: 0,
    };
    let range = document
        .register_live_range(range_point, range_point)
        .unwrap();
    let good_budget = NativeMemoryBudget::with_limits(1024 * 1024, 0, 1024 * 1024, 0);
    let bad_budget = NativeMemoryBudget::with_limits(1024 * 1024, 0, 1024 * 1024, 0);
    let good = Arc::new(Mutex::new(ObserverAgent::new(good_budget.clone())));
    let bad = Arc::new(Mutex::new(ObserverAgent::new(bad_budget.clone())));
    document.attach_root_observer_agent(bad.clone()).unwrap();
    document.attach_root_observer_agent(good.clone()).unwrap();
    observe(&document, &good, 94, parent, true);
    observe(&document, &bad, 95, parent, true);
    let used = bad_budget.used(tenta_core::NativeMemoryComponent::MutationObserver);
    let mut hold = bad_budget.begin(tenta_core::NativeMemoryComponent::MutationObserver);
    assert!(hold.charge(1024 * 1024 - used - 16));

    assert!(matches!(
        document.mutate(Mutation::ReplaceAll {
            parent,
            replacement: ReplaceAllWith::ChildrenOf(source),
        }),
        Err(Error::BudgetExceeded { .. })
    ));
    assert_eq!(document.children(parent).collect::<Vec<_>>(), vec![old]);
    assert_eq!(document.children(source).collect::<Vec<_>>(), vec![new]);
    assert_eq!(document.live_range_points(range), Some([range_point; 2]));
    assert_eq!(good.lock().journal().queued_count(ObserverId(94)), 0);
    assert_eq!(bad.lock().journal().queued_count(ObserverId(95)), 0);
    assert_eq!(document.handle_count(parent), 0);
    document.release_live_range(range);
    release_on_teardown(&mut document, &good);
    release_on_teardown(&mut document, &bad);
    drop(hold);
    drop(document);
    drop(good);
    drop(bad);
    assert_eq!(
        good_budget.used(tenta_core::NativeMemoryComponent::MutationObserver),
        0
    );
    assert_eq!(
        bad_budget.used(tenta_core::NativeMemoryComponent::MutationObserver),
        0
    );
}

#[test]
fn set_inner_html_uses_one_grouped_parent_record() {
    let (mut document, agent) = attached_document();
    let parent = document.create_html_element(atoms::DIV);
    let old = document.create_text("old");
    document
        .mutate(Mutation::Append { parent, child: old })
        .unwrap();
    observe(&document, &agent, 96, parent, true);
    document.set_inner_html(parent, "new").unwrap();
    assert_eq!(
        document.text_data(document.first_child(parent)),
        Some("new")
    );
    let records = take_records_and_release(&mut document, &agent, ObserverId(96));
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].removed_nodes,
        vec![ObservedMutation::key(&document, old)]
    );
    assert_eq!(records[0].added_nodes.len(), 1);
    assert_eq!(records[0].old_value, None);
    assert_eq!(records[0].previous_sibling, None);
    assert_eq!(records[0].next_sibling, None);
    release_on_teardown(&mut document, &agent);
}

#[test]
fn replace_all_rejects_invalid_source_before_removing_existing_children() {
    let mut document = Document::new();
    let parent = document.create_html_element(atoms::DIV);
    let old = document.create_html_element(atoms::SPAN);
    document
        .mutate(Mutation::Append { parent, child: old })
        .unwrap();
    let source = document.create_fragment();
    let doctype = document.create_doctype("html", "", "");
    document
        .mutate(Mutation::Append {
            parent: source,
            child: doctype,
        })
        .unwrap_err();
    // A source cannot be the target itself, and a non-container cannot supply children.
    assert!(
        document
            .mutate(Mutation::ReplaceAll {
                parent,
                replacement: ReplaceAllWith::ChildrenOf(parent),
            })
            .is_err()
    );
    assert!(
        document
            .mutate(Mutation::ReplaceAll {
                parent,
                replacement: ReplaceAllWith::ChildrenOf(old),
            })
            .is_err()
    );
    assert_eq!(document.children(parent).collect::<Vec<_>>(), vec![old]);
}

#[test]
fn replace_all_rechecks_range_capacity_after_observer_pins_before_any_effect() {
    let (mut document, agent) = attached_document();
    let parent = document.create_html_element(atoms::DIV);
    let old = document.create_text("old");
    document
        .mutate(Mutation::Append { parent, child: old })
        .unwrap();
    let point = RangeBoundary {
        node: document.node_ref(old),
        offset: 0,
    };
    let range = document.register_live_range(point, point).unwrap();
    observe(&document, &agent, 97, parent, true);
    let pins: Vec<_> = (0..u16::MAX - 2)
        .map(|_| document.handle(parent).unwrap())
        .collect();

    let error = document
        .mutate(Mutation::ReplaceAll {
            parent,
            replacement: ReplaceAllWith::Empty,
        })
        .unwrap_err();
    assert!(error.to_string().contains("live range fixup"), "{error}");
    assert_eq!(document.first_child(parent), old);
    assert_eq!(document.live_range_points(range), Some([point; 2]));
    assert_eq!(agent.lock().journal().queued_count(ObserverId(97)), 0);
    assert_eq!(document.handle_count(parent), u16::MAX - 2);
    for pin in pins {
        document.release(pin);
    }
    document.release_live_range(range);
    release_on_teardown(&mut document, &agent);
}

#[test]
fn native_document_can_attach_after_parser_mutations_and_a_refused_first_world() {
    let mut document = Document::new();
    let parent = document.create_html_element(atoms::DIV);
    TreeSink::append(&mut document, &parent, NodeOrText::Text("before"));
    TreeSink::append(&mut document, &parent, NodeOrText::Text(" attach"));
    assert_eq!(
        document.text_data(document.first_child(parent)),
        Some("before attach")
    );
    assert!(document.observer_registry.is_none());
    document.check_observer_health().unwrap();

    let (other, foreign_agent) = attached_document();
    assert!(document.attach_root_observer_agent(foreign_agent).is_err());
    assert!(document.observer_registry.is_none());
    drop(other);

    let (agent, attachment) = document.new_observer_world_owned(NativeMemoryBudget::with_limits(
        1024 * 1024,
        0,
        1024 * 1024,
        0,
    ));
    observe(&document, &agent, 101, parent, true);
    let child = document.create_html_element(atoms::SPAN);
    document.mutate(Mutation::Append { parent, child }).unwrap();
    let records = take_records_and_release(&mut document, &agent, ObserverId(101));
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].added_nodes,
        vec![ObservedMutation::key(&document, child)]
    );
    drop(attachment);
    document.sweep_detached();
    assert_eq!(document.handle_count(parent), 0);
}

#[test]
fn empty_world_keeps_notification_demand_across_noop_failure_and_delivery() {
    let (mut document, agent) = attached_document();
    let parent = document.create_html_element(atoms::DIV);
    let child = document.create_html_element(atoms::SPAN);
    let name = QualName::new(atoms::EMPTY, atoms::ID);
    document.mutate(Mutation::Remove { child }).unwrap();
    document
        .mutate(Mutation::RemoveAttribute {
            element: parent,
            name,
        })
        .unwrap();
    assert_eq!(agent.lock().notify_demand(), NotifyDemand::Idle);
    document.mutate(Mutation::Append { parent, child }).unwrap();
    assert_eq!(agent.lock().notify_demand(), NotifyDemand::Requested);
    agent.lock().mark_notify_enqueued().unwrap();
    assert!(
        document
            .mutate(Mutation::Append {
                parent: child,
                child: parent
            })
            .is_err()
    );
    document
        .mutate(Mutation::RemoveAttribute {
            element: parent,
            name,
        })
        .unwrap();
    assert_eq!(agent.lock().notify_demand(), NotifyDemand::Enqueued);
    assert!(agent.lock().begin_notify().unwrap().is_empty());
    assert_eq!(agent.lock().notify_demand(), NotifyDemand::Idle);
    document.mutate(Mutation::Remove { child }).unwrap();
    assert_eq!(agent.lock().notify_demand(), NotifyDemand::Requested);
    assert_eq!(
        agent
            .lock()
            .budget()
            .used(tenta_core::NativeMemoryComponent::MutationObserver),
        0
    );
}

#[test]
fn empty_world_preserves_sticky_budget_before_effect_and_native_poison_before_validation() {
    let (mut document, agent) = attached_document();
    let text = document.create_text("before");
    let budget = agent.lock().budget().clone();
    budget
        .begin(tenta_core::NativeMemoryComponent::MutationObserver)
        .fail_allocation();
    assert!(matches!(
        document.mutate(Mutation::SetText {
            node: text,
            text: "after".to_owned()
        }),
        Err(Error::BudgetExceeded { .. })
    ));
    assert_eq!(document.text_data(text), Some("before"));
    assert_eq!(agent.lock().notify_demand(), NotifyDemand::Idle);
    agent.lock().poison_shared_effect();
    assert!(matches!(
        document.mutate(Mutation::Append {
            parent: text,
            child: text
        }),
        Err(Error::Crashed { .. })
    ));
}

#[test]
fn empty_world_late_budget_failure_stops_before_effect_and_preserves_the_error() {
    let (mut document, agent) = attached_document();
    let text = document.create_text("before");
    let registry = Arc::clone(document.observer_registry.as_ref().unwrap());
    let registration = agent
        .lock()
        .registration_for(document.document_id())
        .cloned()
        .unwrap();
    let mut observed =
        ObservedMutation::without_records(registry, registration, agent.clone(), true);
    let budget = agent.lock().budget().clone();
    budget
        .begin(tenta_core::NativeMemoryComponent::MutationObserver)
        .fail_allocation();

    let error = observed
        .check_pre_effect()
        .expect_err("the outer effect gate must see a late failure");
    assert!(matches!(&error, Error::BudgetExceeded { .. }));
    let error = document
        .finish_observed_error(&mut observed, error)
        .expect_err("cleanup must preserve the original budget error");
    assert!(matches!(error, Error::BudgetExceeded { .. }));
    let failure = budget.first_failure().unwrap();
    assert_eq!(
        failure.component,
        tenta_core::NativeMemoryComponent::MutationObserver
    );
    assert_eq!(
        failure.cause,
        tenta_core::NativeMemoryFailureCause::Allocation
    );
    assert_eq!(document.text_data(text), Some("before"));
    assert_eq!(agent.lock().notify_demand(), NotifyDemand::Idle);
    assert_eq!(
        budget.used(tenta_core::NativeMemoryComponent::MutationObserver),
        0
    );
    assert!(agent.lock().check_native_health().is_ok());
}

#[test]
fn empty_world_drains_released_pins_before_the_next_effect() {
    let (mut document, agent) = attached_document();
    let element = document.create_html_element(atoms::DIV);
    observe(&document, &agent, 102, element, false);
    let mutation = || Mutation::SetAttribute {
        element,
        name: QualName::new(atoms::EMPTY, atoms::ID),
        value: "changed".to_owned(),
    };
    document.mutate(mutation()).unwrap();
    let records = agent.lock().journal_mut().disconnect(ObserverId(102));
    for mut record in records {
        agent
            .lock()
            .with_pins_for(document.document_id(), |pins| {
                record.publish(pins).unwrap();
                record.release_published(pins).unwrap();
            })
            .unwrap();
    }
    assert!(!agent.lock().journal().has_registrations());
    assert_eq!(document.handle_count(element), 1);
    document.mutate(mutation()).unwrap();
    assert_eq!(document.handle_count(element), 0);
    assert!(agent.lock().check_native_health().is_ok());
}

struct FailEmptyWorldOnRemoval {
    budget: NativeMemoryBudget,
}

impl crate::mutation::MutationObserverSink for FailEmptyWorldOnRemoval {
    fn record(&mut self, record: crate::mutation::MutationRecord) {
        if matches!(record, crate::mutation::MutationRecord::Removed { .. }) {
            self.budget
                .begin(tenta_core::NativeMemoryComponent::MutationObserver)
                .fail_allocation();
        }
    }
}

#[test]
fn empty_world_partial_effect_retains_notification_when_next_effect_exceeds_budget() {
    let (mut document, agent) = attached_document();
    let from = document.create_html_element(atoms::DIV);
    let to = document.create_html_element(atoms::DIV);
    let child = document.create_html_element(atoms::SPAN);
    document
        .mutate(Mutation::Append {
            parent: from,
            child,
        })
        .unwrap();
    agent.lock().mark_notify_enqueued().unwrap();
    assert!(agent.lock().begin_notify().unwrap().is_empty());
    document.set_observer_sink(Box::new(FailEmptyWorldOnRemoval {
        budget: agent.lock().budget().clone(),
    }));
    assert!(matches!(
        document.mutate(Mutation::ReparentChildren { from, to }),
        Err(Error::BudgetExceeded { .. })
    ));
    assert_eq!(document.parent(child), NodeId::NONE);
    assert_eq!(document.first_child(to), NodeId::NONE);
    assert_eq!(agent.lock().notify_demand(), NotifyDemand::Requested);
    assert!(agent.lock().check_native_health().is_ok());
}

struct RetireEmptyWorldOnRemoval {
    attachment: ObserverAttachment,
}

impl crate::mutation::MutationObserverSink for RetireEmptyWorldOnRemoval {
    fn record(&mut self, record: crate::mutation::MutationRecord) {
        if matches!(record, crate::mutation::MutationRecord::Removed { .. }) {
            self.attachment.retire();
        }
    }
}

#[test]
fn retained_empty_agent_does_not_resume_a_retired_world_between_effects() {
    let mut document = Document::new();
    let (agent, attachment) = document.new_observer_world_owned(NativeMemoryBudget::with_limits(
        1024 * 1024,
        0,
        1024 * 1024,
        0,
    ));
    let from = document.create_html_element(atoms::DIV);
    let to = document.create_html_element(atoms::DIV);
    let child = document.create_html_element(atoms::SPAN);
    document
        .mutate(Mutation::Append {
            parent: from,
            child,
        })
        .unwrap();
    agent.lock().mark_notify_enqueued().unwrap();
    assert!(agent.lock().begin_notify().unwrap().is_empty());
    document.set_observer_sink(Box::new(RetireEmptyWorldOnRemoval { attachment }));
    assert!(matches!(
        document.mutate(Mutation::ReparentChildren { from, to }),
        Err(Error::Closed)
    ));
    assert_eq!(document.parent(child), NodeId::NONE);
    assert_eq!(document.first_child(to), NodeId::NONE);
    assert_eq!(agent.lock().notify_demand(), NotifyDemand::Idle);
    assert!(agent.lock().check_native_health().is_ok());
    document.sweep_detached();
    assert_eq!(
        agent
            .lock()
            .registration_for(document.document_id())
            .unwrap()
            .phase(),
        RegistrationPhase::Drained
    );
}

struct PanicEmptyWorldSink;

impl crate::mutation::MutationObserverSink for PanicEmptyWorldSink {
    fn record(&mut self, _: crate::mutation::MutationRecord) {
        panic!("injected native diagnostic failure with an empty observer world");
    }
}

#[test]
fn empty_world_native_panic_poison_is_sticky_after_a_real_effect() {
    let (mut document, agent) = attached_document();
    let text = document.create_text("before");
    document.set_observer_sink(Box::new(PanicEmptyWorldSink));
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = document.mutate(Mutation::SetText {
            node: text,
            text: "after".to_owned(),
        });
    }));
    assert!(failure.is_err());
    assert_eq!(document.text_data(text), Some("after"));
    assert!(matches!(
        agent.lock().check_native_health(),
        Err(Error::Crashed { .. })
    ));
    document.set_observer_sink(Box::new(crate::mutation::NoopObserverSink));
    assert!(matches!(
        document.mutate(Mutation::SetText {
            node: text,
            text: "later".to_owned()
        }),
        Err(Error::Crashed { .. })
    ));
    assert_eq!(document.text_data(text), Some("after"));
}

#[test]
fn a_second_empty_world_still_participates_in_all_world_budget_admission() {
    let (mut document, first) = attached_document();
    let second = Arc::new(Mutex::new(ObserverAgent::new(
        NativeMemoryBudget::with_limits(1024 * 1024, 0, 1024 * 1024, 0),
    )));
    document
        .attach_root_observer_agent(Arc::clone(&second))
        .unwrap();
    let text = document.create_text("before");
    first
        .lock()
        .budget()
        .begin(tenta_core::NativeMemoryComponent::MutationObserver)
        .fail_allocation();
    assert!(matches!(
        document.mutate(Mutation::SetText {
            node: text,
            text: "after".to_owned()
        }),
        Err(Error::BudgetExceeded { .. })
    ));
    assert_eq!(document.text_data(text), Some("before"));
    assert_eq!(first.lock().notify_demand(), NotifyDemand::Idle);
    assert_eq!(second.lock().notify_demand(), NotifyDemand::Idle);
    assert_eq!(document.handle_count(text), 0);
}

#[test]
fn empty_world_attribute_removal_requests_only_after_a_live_namespace_match() {
    let mut document = Document::new();
    let element = document.create_html_element(atoms::DIV);
    let plain = QualName::new(atoms::EMPTY, atoms::ID);
    let namespaced = QualName::new(tenta_core::Atom::new("urn:observer-removal"), atoms::ID);
    for (name, value) in [(plain, "plain"), (namespaced, "namespaced")] {
        document
            .mutate(Mutation::SetAttribute {
                element,
                name,
                value: value.to_owned(),
            })
            .unwrap();
    }
    let (agent, _attachment) = document.new_observer_world_owned(NativeMemoryBudget::with_limits(
        1024 * 1024,
        0,
        1024 * 1024,
        0,
    ));
    let diagnostics = crate::mutation::RecordingObserverSink::new();
    document.set_observer_sink(Box::new(diagnostics.clone()));
    let before = document.mutation_generation();
    document
        .mutate(Mutation::RemoveAttribute {
            element,
            name: plain,
        })
        .unwrap();
    assert_eq!(agent.lock().notify_demand(), NotifyDemand::Requested);
    assert_eq!(document.attribute(element, atoms::ID), None);
    assert_eq!(
        document.attribute_ns(element, namespaced.ns, atoms::ID),
        Some("namespaced")
    );
    assert_eq!(document.mutation_generation(), before + 1);
    assert_eq!(
        diagnostics.take(),
        vec![crate::mutation::MutationRecord::Attribute {
            element,
            name: atoms::ID,
            namespace: atoms::EMPTY,
            old_value: Some("plain".to_owned()),
        }]
    );
    agent.lock().mark_notify_enqueued().unwrap();
    assert!(agent.lock().begin_notify().unwrap().is_empty());

    // The removed slot is still a tombstone. A same-local-name attribute in
    // another namespace cannot turn this no-op into notification demand.
    document
        .mutate(Mutation::RemoveAttribute {
            element,
            name: plain,
        })
        .unwrap();
    assert_eq!(agent.lock().notify_demand(), NotifyDemand::Idle);
    assert_eq!(document.mutation_generation(), before + 1);
    assert!(diagnostics.is_empty());
    document
        .mutate(Mutation::RemoveAttribute {
            element,
            name: namespaced,
        })
        .unwrap();
    assert_eq!(agent.lock().notify_demand(), NotifyDemand::Requested);
    assert_eq!(document.mutation_generation(), before + 2);
    assert_eq!(
        diagnostics.take(),
        vec![crate::mutation::MutationRecord::Attribute {
            element,
            name: atoms::ID,
            namespace: namespaced.ns,
            old_value: Some("namespaced".to_owned()),
        }]
    );
    assert_eq!(document.handle_count(element), 0);
}

#[test]
fn attribute_removal_outcome_publishes_one_old_value_to_each_observing_world() {
    let (mut document, first) = attached_document();
    let (second, _attachment) = document.new_observer_world_owned(NativeMemoryBudget::with_limits(
        1024 * 1024,
        0,
        1024 * 1024,
        0,
    ));
    let element = document.create_html_element(atoms::DIV);
    let name = QualName::new(atoms::EMPTY, atoms::CLASS);
    document
        .mutate(Mutation::SetAttribute {
            element,
            name,
            value: "before".to_owned(),
        })
        .unwrap();
    observe(&document, &first, 121, element, false);
    observe(&document, &second, 122, element, false);
    document
        .mutate(Mutation::RemoveAttribute { element, name })
        .unwrap();
    document
        .mutate(Mutation::RemoveAttribute { element, name })
        .unwrap();
    for (agent, observer) in [(&first, ObserverId(121)), (&second, ObserverId(122))] {
        let records = take_records_and_release(&mut document, agent, observer);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record_type, RecordType::Attributes);
        assert_eq!(records[0].target, ObservedMutation::key(&document, element));
        assert_eq!(records[0].attribute_name, Some(atoms::CLASS));
        assert_eq!(records[0].old_value.as_deref(), Some("before"));
    }
    assert_eq!(document.handle_count(element), 0);
    release_on_teardown(&mut document, &first);
    release_on_teardown(&mut document, &second);
}

#[test]
fn unattached_borrowed_parser_merges_keep_diagnostic_old_values_and_generations() {
    let mut document = Document::new();
    let parent = document.create_html_element(atoms::DIV);
    document.append_text(parent, "leading").unwrap();
    let leading = document.first_child(parent);
    let middle = document.create_html_element(atoms::SPAN);
    document
        .mutate(Mutation::Append {
            parent,
            child: middle,
        })
        .unwrap();
    document.append_text(parent, "trailing").unwrap();
    let trailing = document.last_child(parent);
    let diagnostics = crate::mutation::RecordingObserverSink::new();
    document.set_observer_sink(Box::new(diagnostics.clone()));
    let before = document.mutation_generation();

    document.append_text(parent, " suffix").unwrap();
    document.insert_text_before(middle, " prefix").unwrap();
    document.append_text(parent, "").unwrap();
    assert!(document.observer_registry.is_none());
    assert_eq!(
        document.children(parent).collect::<Vec<_>>(),
        vec![leading, middle, trailing]
    );
    assert_eq!(document.text_data(leading), Some("leading prefix"));
    assert_eq!(document.text_data(trailing), Some("trailing suffix"));
    assert_eq!(document.mutation_generation(), before + 3);
    assert_eq!(
        diagnostics.take(),
        vec![
            crate::mutation::MutationRecord::CharacterData {
                node: trailing,
                old_value: "trailing".to_owned()
            },
            crate::mutation::MutationRecord::CharacterData {
                node: leading,
                old_value: "leading".to_owned()
            },
        ]
    );
}
