use super::*;
use tenta_core::NodeRef;
use tenta_core::atom::atoms;

use super::super::observer_journal::{ObserverNode, RecordType};

fn record(target: ObserverNode) -> SemanticRecord {
    SemanticRecord {
        record_type: RecordType::Attributes,
        target,
        added_nodes: Vec::new(),
        removed_nodes: Vec::new(),
        previous_sibling: None,
        next_sibling: None,
        attribute_name: Some(atoms::ID),
        attribute_namespace: None,
        old_value: Some("previous".to_owned()),
    }
}

fn target() -> (Document, tenta_core::NodeId, ObserverNode) {
    let mut document = Document::new();
    let node = document.create_html_element(atoms::DIV);
    let key = ObserverNode::new(document.document_id(), document.node_ref(node));
    (document, node, key)
}

#[test]
fn one_record_owns_budget_and_exact_pin_through_publication() {
    let budget = NativeMemoryBudget::with_limits(1_000_000, 1_000_000, 1_000_000, 0);
    let (mut document, node, key) = target();
    let mut pins = ObserverPins::new(budget.clone());
    let mut owned =
        OwnedObserverRecord::acquire_pending(&budget, &mut pins, &mut document, record(key))
            .unwrap();
    assert!(budget.used(NativeMemoryComponent::MutationObserver) > 0);
    assert_eq!(document.handle_count(node), 1);
    owned.publish(&mut pins).unwrap();
    assert_eq!(pins.owner_counts(key), Some((0, 1)));
    owned.release(&mut pins, &mut document).unwrap();
    assert_eq!(document.handle_count(node), 0);
    assert!(budget.used(NativeMemoryComponent::MutationObserver) > 0);
    drop(document);
    drop(owned);
    drop(pins);
    assert_eq!(budget.used(NativeMemoryComponent::MutationObserver), 0);
}

#[test]
fn hard_budget_refusal_occurs_before_any_native_pin() {
    let budget = NativeMemoryBudget::with_limits(1, 1, 1, 0);
    let (mut document, node, key) = target();
    let mut pins = ObserverPins::new(budget.clone());
    assert!(
        OwnedObserverRecord::acquire_pending(&budget, &mut pins, &mut document, record(key))
            .is_err()
    );
    assert_eq!(document.handle_count(node), 0);
    assert!(pins.is_empty());
    assert!(budget.first_failure().is_some());
}

#[test]
fn earlier_page_budget_failure_keeps_its_original_component() {
    let budget = NativeMemoryBudget::with_limits(100, 0, 100, 0);
    assert!(
        !budget
            .begin(NativeMemoryComponent::StructuredClone)
            .charge(1)
    );
    let (mut document, node, key) = target();
    let mut pins = ObserverPins::new(budget.clone());
    let error =
        OwnedObserverRecord::acquire_pending(&budget, &mut pins, &mut document, record(key))
            .unwrap_err();
    assert!(matches!(
        error,
        tenta_core::Error::BudgetExceeded {
            component: Some(component),
            ..
        } if component == "structured clone"
    ));
    assert_eq!(document.handle_count(node), 0);
    assert!(pins.is_empty());
}

#[test]
fn invalid_generation_does_not_misreport_an_allocation_failure() {
    let budget = NativeMemoryBudget::with_limits(1_000_000, 1_000_000, 1_000_000, 0);
    let (mut document, node, key) = target();
    let stale = ObserverNode::new(
        key.document_id,
        NodeRef::new(key.node.id, key.node.generation + 1),
    );
    let mut pins = ObserverPins::new(budget.clone());
    assert!(
        OwnedObserverRecord::acquire_pending(&budget, &mut pins, &mut document, record(stale))
            .is_err()
    );
    assert_eq!(document.handle_count(node), 0);
    assert!(pins.is_empty());
    assert!(budget.first_failure().is_none());
    assert_eq!(budget.used(NativeMemoryComponent::MutationObserver), 0);
}

#[test]
fn record_and_physical_pin_must_use_the_same_page_ledger() {
    let first = NativeMemoryBudget::with_limits(1_000_000, 0, 1_000_000, 0);
    let second = NativeMemoryBudget::with_limits(1_000_000, 0, 1_000_000, 0);
    let (mut document, node, key) = target();
    let mut pins = ObserverPins::new(second.clone());
    assert!(
        OwnedObserverRecord::acquire_pending(&first, &mut pins, &mut document, record(key))
            .is_err()
    );
    assert_eq!(document.handle_count(node), 0);
    assert_eq!(first.total_used(), 0);
    assert_eq!(second.total_used(), 0);
    assert!(first.first_failure().is_none());
    assert!(second.first_failure().is_none());
}

#[test]
fn published_list_outlives_record_and_releases_only_its_members() {
    let budget = NativeMemoryBudget::with_limits(1_000_000, 1_000_000, 1_000_000, 0);
    let (mut document, target_id, target) = target();
    let member_id = document.create_html_element(atoms::SPAN);
    let sibling_id = document.create_html_element(atoms::B);
    let member = ObserverNode::new(document.document_id(), document.node_ref(member_id));
    let sibling = ObserverNode::new(document.document_id(), document.node_ref(sibling_id));
    let mut facts = record(target);
    facts.record_type = RecordType::ChildList;
    facts.removed_nodes.push(member);
    facts.previous_sibling = Some(sibling);
    let mut pins = ObserverPins::new(budget.clone());
    let mut owned =
        OwnedObserverRecord::acquire_pending(&budget, &mut pins, &mut document, facts).unwrap();
    owned.publish(&mut pins).unwrap();
    let mut list = OwnedObserverList::retain(&mut pins, &[member]).unwrap();
    owned.release_published(&mut pins).unwrap();
    drop(owned);
    assert_eq!(pins.owner_counts(target), Some((0, 0)));
    assert_eq!(pins.owner_counts(sibling), Some((0, 0)));
    assert_eq!(pins.owner_counts(member), Some((0, 1)));
    assert_eq!(pins.drain_released(&mut document).unwrap(), 2);
    assert_eq!(document.handle_count(target_id), 0);
    assert_eq!(document.handle_count(sibling_id), 0);
    assert_eq!(document.handle_count(member_id), 1);
    list.release_published(&mut pins).unwrap();
    assert!(list.release_published(&mut pins).is_err());
    assert_eq!(pins.owner_counts(member), Some((0, 0)));
    assert_eq!(document.handle_count(member_id), 1);
    let charged_with_list = budget.used(NativeMemoryComponent::MutationObserver);
    drop(list);
    let charged_with_physical_pin = budget.used(NativeMemoryComponent::MutationObserver);
    assert!(charged_with_physical_pin < charged_with_list);
    assert!(charged_with_physical_pin > 0);
    assert_eq!(pins.drain_released(&mut document).unwrap(), 1);
    assert_eq!(document.handle_count(member_id), 0);
    assert!(budget.first_failure().is_none());
}
