use super::super::observer_ownership::OwnedObserverList;
use super::*;
use crate::mutation::Mutation;
use tenta_core::NodeRef;
use tenta_core::atom::atoms;

fn document_with_target() -> (Document, ObserverNode) {
    let mut document = Document::new();
    let target = document.create_html_element(atoms::DIV);
    let reference = ObserverNode::new(document.document_id(), document.node_ref(target));
    (document, reference)
}

fn attribute_record(target: ObserverNode) -> SemanticRecord {
    SemanticRecord {
        record_type: super::super::observer_journal::RecordType::Attributes,
        target,
        added_nodes: Vec::new(),
        removed_nodes: Vec::new(),
        previous_sibling: None,
        next_sibling: None,
        attribute_name: Some(atoms::CLASS),
        attribute_namespace: None,
        old_value: None,
    }
}

#[test]
fn one_physical_handle_survives_more_than_u16_max_pending_records() {
    let (mut document, target) = document_with_target();
    let record = attribute_record(target);
    let mut pins = ObserverPins::default();
    let mut leases = Vec::new();
    for _ in 0..=usize::from(u16::MAX) {
        leases.push(pins.acquire_pending_record(&mut document, &record).unwrap());
    }
    assert_eq!(
        pins.owner_counts(target),
        Some((usize::from(u16::MAX) + 1, 0))
    );
    assert_eq!(pins.conditional_handle_subtraction(target), 0);
    for mut lease in leases {
        pins.release(&mut document, &mut lease).unwrap();
    }
    assert!(pins.is_empty());
}

#[test]
fn mixed_pending_and_published_owners_keep_pin_unconditional_until_last_pending() {
    let (mut document, target) = document_with_target();
    let record = attribute_record(target);
    let mut pins = ObserverPins::default();
    let mut published = pins.acquire_pending_record(&mut document, &record).unwrap();
    let mut pending = pins.acquire_pending_record(&mut document, &record).unwrap();
    pins.publish_record(&mut published).unwrap();
    assert_eq!(pins.owner_counts(target), Some((1, 1)));
    assert_eq!(pins.conditional_handle_subtraction(target), 0);
    pins.release(&mut document, &mut pending).unwrap();
    assert_eq!(pins.owner_counts(target), Some((0, 1)));
    assert_eq!(pins.conditional_handle_subtraction(target), 1);
    pins.release(&mut document, &mut published).unwrap();
    assert!(pins.is_empty());
}

#[test]
fn unpublished_wrapper_rollback_restores_pending_owner_without_repinning() {
    let (mut document, target) = document_with_target();
    let record = attribute_record(target);
    let mut pins = ObserverPins::default();
    let mut lease = pins.acquire_pending_record(&mut document, &record).unwrap();
    pins.publish_record(&mut lease).unwrap();
    assert_eq!(pins.owner_counts(target), Some((0, 1)));
    pins.revert_publication(&mut lease).unwrap();
    assert_eq!(pins.owner_counts(target), Some((1, 0)));
    assert_eq!(pins.conditional_handle_subtraction(target), 0);
    pins.release(&mut document, &mut lease).unwrap();
    assert!(pins.is_empty());
}

#[test]
fn failed_release_retains_the_exact_active_lease_for_owner_teardown() {
    let (mut document, target) = document_with_target();
    let mut other_document = Document::new();
    let mut pins = ObserverPins::default();
    let mut lease = pins
        .acquire_pending_record(&mut document, &attribute_record(target))
        .unwrap();
    assert!(pins.release(&mut other_document, &mut lease).is_err());
    assert!(lease.is_pending());
    assert_eq!(pins.owner_counts(target), Some((1, 0)));
    pins.release(&mut document, &mut lease).unwrap();
    assert!(pins.is_empty());
}

#[test]
fn later_record_preflight_failure_releases_every_earlier_batch_pin() {
    let (mut document, target) = document_with_target();
    let mut pins = ObserverPins::default();
    let foreign = ObserverNode::new(target.document_id + 1, target.node);
    let records = [attribute_record(target), attribute_record(foreign)];
    let original = document.resolve(target.node).unwrap();
    let handles_before = document.handle_count(original);

    assert!(
        pins.acquire_pending_batch(&mut document, records.iter())
            .is_err()
    );
    assert!(pins.is_empty());
    assert_eq!(document.handle_count(original), handles_before);
}

#[test]
fn stale_generation_cannot_acquire_or_alias_a_reused_node() {
    let (mut document, target) = document_with_target();
    let mut pins = ObserverPins::default();
    let stale = ObserverNode::new(
        target.document_id,
        NodeRef::new(target.node.id, target.node.generation + 1),
    );
    assert!(
        pins.acquire_pending_record(&mut document, &attribute_record(stale))
            .is_err()
    );
    assert!(pins.is_empty());
    let foreign = ObserverNode::new(target.document_id + 1, target.node);
    assert!(
        pins.acquire_pending_record(&mut document, &attribute_record(foreign))
            .is_err()
    );
    assert!(pins.is_empty());
    let mut lease = pins
        .acquire_pending_record(&mut document, &attribute_record(target))
        .unwrap();
    lease.nodes[0] = stale;
    assert!(pins.publish_record(&mut lease).is_err());
    assert_eq!(pins.owner_counts(target), Some((1, 0)));
    lease.nodes[0] = target;
    pins.release(&mut document, &mut lease).unwrap();
}

#[test]
fn duplicate_fields_count_one_owner_and_release_without_underflow() {
    let (mut document, target) = document_with_target();
    let mut record = attribute_record(target);
    record.record_type = super::super::observer_journal::RecordType::ChildList;
    record.added_nodes.push(target);
    record.previous_sibling = Some(target);
    let mut pins = ObserverPins::default();
    let mut lease = pins.acquire_pending_record(&mut document, &record).unwrap();
    assert_eq!(pins.owner_counts(target), Some((1, 0)));
    pins.publish_record(&mut lease).unwrap();
    assert_eq!(pins.owner_counts(target), Some((0, 1)));
    pins.release(&mut document, &mut lease).unwrap();
    assert!(pins.is_empty());
}

#[test]
fn independent_native_handle_does_not_become_a_conditional_observer_pin() {
    let (mut document, target) = document_with_target();
    let external = document.handle(target.node.id).unwrap();
    let record = attribute_record(target);
    let mut pins = ObserverPins::default();
    let mut lease = pins.acquire_pending_record(&mut document, &record).unwrap();
    pins.publish_record(&mut lease).unwrap();
    assert_eq!(pins.conditional_handle_subtraction(target), 1);
    pins.release(&mut document, &mut lease).unwrap();
    assert!(pins.is_empty());
    document.release(external);
}

#[test]
fn empty_list_keeps_no_nodes_and_removed_list_keeps_only_its_members() {
    let (mut document, target) = document_with_target();
    let member = document.create_html_element(atoms::SPAN);
    let sibling = document.create_html_element(atoms::B);
    let member_ref = ObserverNode::new(document.document_id(), document.node_ref(member));
    let sibling_ref = ObserverNode::new(document.document_id(), document.node_ref(sibling));
    let mut record = attribute_record(target);
    record.record_type = super::super::observer_journal::RecordType::ChildList;
    record.removed_nodes.push(member_ref);
    record.previous_sibling = Some(sibling_ref);
    let mut pins = ObserverPins::default();
    let mut record_lease = pins.acquire_pending_record(&mut document, &record).unwrap();
    pins.publish_record(&mut record_lease).unwrap();
    let before_list = pins.budget().used(NativeMemoryComponent::MutationObserver);
    let mut empty_list = OwnedObserverList::retain(&mut pins, &[]).unwrap();
    assert_eq!(
        pins.budget().used(NativeMemoryComponent::MutationObserver),
        before_list
    );
    let mut removed_list = OwnedObserverList::retain(&mut pins, &record.removed_nodes).unwrap();
    assert!(pins.budget().used(NativeMemoryComponent::MutationObserver) > before_list);
    pins.release(&mut document, &mut record_lease).unwrap();
    assert_eq!(pins.owner_counts(target), None);
    assert_eq!(pins.owner_counts(sibling_ref), None);
    assert_eq!(pins.owner_counts(member_ref), Some((0, 1)));
    empty_list.release_published(&mut pins).unwrap();
    removed_list.release_published(&mut pins).unwrap();
    assert_eq!(pins.drain_released(&mut document).unwrap(), 1);
    assert!(pins.is_empty());
    drop(empty_list);
    drop(removed_list);
    assert_eq!(
        pins.budget().used(NativeMemoryComponent::MutationObserver),
        before_list
    );
}

#[test]
fn native_document_teardown_releases_all_observer_pins() {
    let (mut document, target) = document_with_target();
    let record = attribute_record(target);
    let mut pins = ObserverPins::default();
    let _lease = pins.acquire_pending_record(&mut document, &record).unwrap();
    document
        .mutate(Mutation::Remove {
            child: target.node.id,
        })
        .unwrap();
    assert_eq!(document.sweep_detached().nodes_freed, 0);
    pins.release_all(&mut document);
    assert!(pins.is_empty());
    assert!(document.sweep_detached().nodes_freed >= 1);
}

#[test]
fn published_release_is_exactly_once_and_does_not_relabel_errors_as_budget_failures() {
    let (mut document, target) = document_with_target();
    let budget = NativeMemoryBudget::with_limits(1_000_000, 1_000_000, 1_000_000, 0);
    let mut pins = ObserverPins::new(budget.clone());
    let mut lease = pins
        .acquire_pending_record(&mut document, &attribute_record(target))
        .unwrap();
    assert!(pins.release_published(&mut lease).is_err());
    assert!(lease.is_pending());
    assert_eq!(pins.owner_counts(target), Some((1, 0)));
    pins.publish_record(&mut lease).unwrap();
    pins.release_published(&mut lease).unwrap();
    assert_eq!(pins.owner_counts(target), Some((0, 0)));
    assert_eq!(pins.conditional_handle_subtraction(target), 0);
    assert!(pins.release_published(&mut lease).is_err());
    assert_eq!(pins.owner_counts(target), Some((0, 0)));
    assert!(budget.first_failure().is_none());
    assert_eq!(document.handle_count(target.node.id), 1);
    let mut foreign_document = Document::new();
    assert!(pins.drain_released(&mut foreign_document).is_err());
    assert_eq!(pins.owner_counts(target), Some((0, 0)));
    assert_eq!(document.handle_count(target.node.id), 1);
    assert!(budget.first_failure().is_none());
    assert_eq!(pins.drain_released(&mut document).unwrap(), 1);
    assert_eq!(document.handle_count(target.node.id), 0);
    assert_eq!(pins.drain_released(&mut document).unwrap(), 0);
}

#[test]
fn new_pending_owner_reuses_a_deferred_physical_handle_before_drain() {
    let (mut document, target) = document_with_target();
    let mut pins = ObserverPins::default();
    let record = attribute_record(target);
    let mut published = pins.acquire_pending_record(&mut document, &record).unwrap();
    pins.publish_record(&mut published).unwrap();
    pins.release_published(&mut published).unwrap();
    assert_eq!(pins.owner_counts(target), Some((0, 0)));
    assert_eq!(document.handle_count(target.node.id), 1);

    let mut pending = pins.acquire_pending_record(&mut document, &record).unwrap();
    assert_eq!(pins.owner_counts(target), Some((1, 0)));
    assert_eq!(document.handle_count(target.node.id), 1);
    assert_eq!(pins.drain_released(&mut document).unwrap(), 0);
    pins.release(&mut document, &mut pending).unwrap();
    assert!(pins.is_empty());
    assert_eq!(document.handle_count(target.node.id), 0);
}

#[test]
fn mixed_fresh_and_deferred_nodes_acquire_atomically() {
    let (mut document, target) = document_with_target();
    let fresh_id = document.create_html_element(atoms::SPAN);
    let fresh = ObserverNode::new(document.document_id(), document.node_ref(fresh_id));
    let mut pins = ObserverPins::default();
    let mut original = pins
        .acquire_pending_record(&mut document, &attribute_record(target))
        .unwrap();
    pins.publish_record(&mut original).unwrap();
    pins.release_published(&mut original).unwrap();
    assert_eq!(pins.owner_counts(target), Some((0, 0)));
    assert_eq!(document.handle_count(target.node.id), 1);

    let mut mixed = attribute_record(target);
    mixed.record_type = super::super::observer_journal::RecordType::ChildList;
    mixed.attribute_name = None;
    mixed.added_nodes.push(fresh);
    let stale = ObserverNode::new(
        fresh.document_id,
        NodeRef::new(fresh.node.id, fresh.node.generation + 1),
    );
    mixed.added_nodes.push(stale);
    assert!(pins.acquire_pending_record(&mut document, &mixed).is_err());
    assert_eq!(pins.owner_counts(target), Some((0, 0)));
    assert_eq!(pins.owner_counts(fresh), None);
    assert_eq!(document.handle_count(target.node.id), 1);
    assert_eq!(document.handle_count(fresh_id), 0);

    mixed.added_nodes.truncate(1);
    let mut pending = pins.acquire_pending_record(&mut document, &mixed).unwrap();
    assert_eq!(pins.owner_counts(target), Some((1, 0)));
    assert_eq!(pins.owner_counts(fresh), Some((1, 0)));
    assert_eq!(document.handle_count(target.node.id), 1);
    assert_eq!(document.handle_count(fresh_id), 1);
    assert_eq!(pins.drain_released(&mut document).unwrap(), 0);
    pins.release(&mut document, &mut pending).unwrap();
    assert!(pins.is_empty());
    assert_eq!(document.handle_count(target.node.id), 0);
    assert_eq!(document.handle_count(fresh_id), 0);
}
