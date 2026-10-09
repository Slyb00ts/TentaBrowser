use super::*;
use tenta_core::atom::atoms;
use tenta_html::QualName;

#[test]
fn captured_facts_hold_their_page_charge_until_after_native_use() {
    let mut document = Document::new();
    let parent = document.create_html_element(atoms::DIV);
    let child = document.create_html_element(atoms::SPAN);
    document.mutate(Mutation::Append { parent, child }).unwrap();
    let mutation = Mutation::Remove { child };
    let budget = NativeMemoryBudget::with_limits(1024 * 1024, 0, 1024 * 1024, 0);
    let captured = capture_validated_charged(&document, &mutation, &budget).unwrap();
    assert_eq!(captured.plan.steps.len(), 2);
    assert!(budget.used(NativeMemoryComponent::MutationObserver) > 0);
    assert_eq!(document.parent(child), parent);
    drop(captured);
    assert_eq!(budget.used(NativeMemoryComponent::MutationObserver), 0);
}

#[test]
fn semantic_fact_limit_is_sticky_before_native_tree_change() {
    let mut document = Document::new();
    let parent = document.create_html_element(atoms::DIV);
    let child = document.create_html_element(atoms::SPAN);
    document.mutate(Mutation::Append { parent, child }).unwrap();
    let mutation = Mutation::Remove { child };
    let budget = NativeMemoryBudget::with_limits(0, 0, 0, 0);
    let error = capture_validated_charged(&document, &mutation, &budget).unwrap_err();
    assert!(matches!(
        error,
        Error::BudgetExceeded {
            component: Some(component),
            ..
        } if component == "mutation observer"
    ));
    assert_eq!(document.parent(child), parent);
    assert_eq!(budget.used(NativeMemoryComponent::MutationObserver), 0);
    assert_eq!(
        budget.first_failure().map(|failure| failure.component),
        Some(NativeMemoryComponent::MutationObserver)
    );
}

#[test]
fn attribute_old_value_and_fragment_growth_remain_charged_with_the_facts() {
    let mut document = Document::new();
    let parent = document.create_html_element(atoms::DIV);
    let name = QualName::new(atoms::EMPTY, atoms::ID);
    let old_value = "old-attribute-value".repeat(16);
    document
        .mutate(Mutation::SetAttribute {
            element: parent,
            name,
            value: old_value.clone(),
        })
        .unwrap();
    let budget = NativeMemoryBudget::with_limits(1024 * 1024, 0, 1024 * 1024, 0);
    let attribute = capture_validated_charged(
        &document,
        &Mutation::SetAttribute {
            element: parent,
            name,
            value: "new".to_owned(),
        },
        &budget,
    )
    .unwrap();
    assert_eq!(
        attribute.plan.records[0].old_value.as_deref(),
        Some(old_value.as_str())
    );
    let attribute_bytes = budget.used(NativeMemoryComponent::MutationObserver);
    assert!(attribute_bytes >= old_value.len());

    let fragment = document.create_fragment();
    for _ in 0..17 {
        let child = document.create_html_element(atoms::SPAN);
        document
            .mutate(Mutation::Append {
                parent: fragment,
                child,
            })
            .unwrap();
    }
    let insertion = capture_validated_charged(
        &document,
        &Mutation::Append {
            parent,
            child: fragment,
        },
        &budget,
    )
    .unwrap();
    assert_eq!(insertion.plan.transient_removals.len(), 17);
    assert_eq!(insertion.plan.records[0].removed_nodes.len(), 17);
    assert!(budget.used(NativeMemoryComponent::MutationObserver) > attribute_bytes);
    drop(insertion);
    assert_eq!(
        budget.used(NativeMemoryComponent::MutationObserver),
        attribute_bytes
    );
    drop(attribute);
    assert_eq!(budget.used(NativeMemoryComponent::MutationObserver), 0);
}

#[test]
fn preexisting_sticky_failure_keeps_its_original_component() {
    let mut document = Document::new();
    let parent = document.create_html_element(atoms::DIV);
    let child = document.create_html_element(atoms::SPAN);
    document.mutate(Mutation::Append { parent, child }).unwrap();
    let budget = NativeMemoryBudget::with_limits(0, 0, 0, 0);
    let mut clone_charge = budget.begin(NativeMemoryComponent::StructuredClone);
    assert!(!clone_charge.charge(1));
    let original = budget.first_failure().unwrap();
    let error =
        capture_validated_charged(&document, &Mutation::Remove { child }, &budget).unwrap_err();
    assert!(matches!(
        error,
        Error::BudgetExceeded {
            component: Some(component),
            ..
        } if component == "structured clone"
    ));
    assert_eq!(budget.first_failure(), Some(original));
    assert_eq!(document.parent(child), parent);
    assert_eq!(budget.total_used(), 0);
}
