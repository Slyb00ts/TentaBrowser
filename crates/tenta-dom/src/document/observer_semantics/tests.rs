use super::*;
use tenta_core::atom::atoms;

fn node(document: &Document, id: NodeId) -> ObserverNode {
    ObserverNode::new(document.document_id(), document.node_ref(id))
}

fn append(document: &mut Document, parent: NodeId, child: NodeId) {
    document.mutate(Mutation::Append { parent, child }).unwrap();
}

#[test]
fn uninterested_mutation_skips_semantic_capture_even_with_other_registrations() {
    let mut document = Document::new();
    let parent = document.create_html_element(atoms::DIV);
    let text = document.create_text("same");
    append(&mut document, parent, text);
    let mut journal = ObserverJournal::default();
    journal
        .observe(
            super::super::observer_journal::ObserverId(1),
            node(&document, parent),
            super::super::observer_journal::ObserveOptions::normalize(
                super::super::observer_journal::ObserveOptionsInput {
                    child_list: true,
                    ..Default::default()
                },
            )
            .unwrap(),
        )
        .unwrap();

    let write = Mutation::SetText {
        node: text,
        text: "same".to_owned(),
    };
    assert!(!may_observe_mutation(&document, &journal, &write));
    assert!(may_observe_mutation(
        &document,
        &journal,
        &Mutation::Remove { child: text }
    ));
    journal
        .observe(
            super::super::observer_journal::ObserverId(2),
            node(&document, parent),
            super::super::observer_journal::ObserveOptions::normalize(
                super::super::observer_journal::ObserveOptionsInput {
                    character_data: Some(true),
                    subtree: true,
                    ..Default::default()
                },
            )
            .unwrap(),
        )
        .unwrap();
    assert!(may_observe_mutation(&document, &journal, &write));
}

#[test]
fn attributes_only_subtree_observer_requires_transients_on_removal() {
    let mut document = Document::new();
    let ancestor = document.create_html_element(atoms::DIV);
    let child = document.create_html_element(atoms::SPAN);
    append(&mut document, ancestor, child);
    let mut journal = ObserverJournal::default();
    journal
        .observe(
            super::super::observer_journal::ObserverId(3),
            node(&document, ancestor),
            super::super::observer_journal::ObserveOptions::normalize(
                super::super::observer_journal::ObserveOptionsInput {
                    attributes: Some(true),
                    subtree: true,
                    ..Default::default()
                },
            )
            .unwrap(),
        )
        .unwrap();

    assert!(may_observe_mutation(
        &document,
        &journal,
        &Mutation::Remove { child }
    ));
    let plan = capture_validated(&document, &Mutation::Remove { child }).unwrap();
    assert_eq!(plan.transient_removals.len(), 1);
    assert_eq!(
        plan.transient_removals[0].removed_root,
        node(&document, child)
    );
    assert!(journal.prepare(&plan.records[0]).unwrap().is_empty());
}

#[test]
fn fragment_insertion_has_one_fragment_removal_then_one_parent_addition() {
    let mut document = Document::new();
    let parent = document.create_html_element(atoms::DIV);
    let fragment = document.create_fragment();
    let a = document.create_html_element(atoms::A);
    let b = document.create_html_element(atoms::B);
    append(&mut document, fragment, a);
    append(&mut document, fragment, b);

    let plan = capture_validated(
        &document,
        &Mutation::Append {
            parent,
            child: fragment,
        },
    )
    .unwrap();
    assert_eq!(plan.records.len(), 2);
    assert_eq!(plan.records[0].target, node(&document, fragment));
    assert_eq!(
        plan.records[0].removed_nodes,
        vec![node(&document, a), node(&document, b)]
    );
    assert_eq!(plan.records[1].target, node(&document, parent));
    assert_eq!(
        plan.records[1].added_nodes,
        vec![node(&document, a), node(&document, b)]
    );
    assert_eq!(plan.transient_removals.len(), 2);
    assert_eq!(
        plan.transient_removals[0].old_parent_ancestors,
        vec![node(&document, fragment)]
    );

    document
        .mutate(Mutation::Append {
            parent,
            child: fragment,
        })
        .unwrap();
    assert!(document.children(fragment).next().is_none());
    assert_eq!(document.children(parent).collect::<Vec<_>>(), vec![a, b]);
}

#[test]
fn replacement_snapshots_old_parent_and_combined_target_record() {
    let mut document = Document::new();
    let source = document.create_html_element(atoms::DIV);
    let parent = document.create_html_element(atoms::DIV);
    let old = document.create_html_element(atoms::A);
    let next = document.create_html_element(atoms::B);
    let replacement = document.create_html_element(atoms::SPAN);
    append(&mut document, parent, old);
    append(&mut document, parent, next);
    append(&mut document, source, replacement);

    let mutation = Mutation::ReplaceChild {
        parent,
        new_child: replacement,
        old_child: old,
    };
    let plan = capture_validated(&document, &mutation).unwrap();
    assert_eq!(plan.records.len(), 2);
    assert_eq!(plan.records[0].target, node(&document, source));
    assert_eq!(
        plan.records[0].removed_nodes,
        vec![node(&document, replacement)]
    );
    assert_eq!(plan.records[1].target, node(&document, parent));
    assert_eq!(
        plan.records[1].added_nodes,
        vec![node(&document, replacement)]
    );
    assert_eq!(plan.records[1].removed_nodes, vec![node(&document, old)]);
    assert_eq!(plan.records[1].next_sibling, Some(node(&document, next)));
    assert_eq!(plan.transient_removals.len(), 2);

    document.mutate(mutation).unwrap();
    assert_eq!(
        document.children(parent).collect::<Vec<_>>(),
        vec![replacement, next]
    );
}

#[test]
fn fragment_replacement_prepares_transients_in_native_unlink_order() {
    let mut document = Document::new();
    let parent = document.create_html_element(atoms::DIV);
    let old = document.create_html_element(atoms::A);
    let fragment = document.create_fragment();
    let first = document.create_html_element(atoms::SPAN);
    let second = document.create_html_element(atoms::B);
    append(&mut document, parent, old);
    append(&mut document, fragment, first);
    append(&mut document, fragment, second);

    let plan = capture_validated(
        &document,
        &Mutation::ReplaceChild {
            parent,
            new_child: fragment,
            old_child: old,
        },
    )
    .unwrap();
    assert_eq!(
        plan.transient_removals
            .iter()
            .map(|removal| removal.removed_root)
            .collect::<Vec<_>>(),
        vec![
            node(&document, old),
            node(&document, first),
            node(&document, second)
        ]
    );
}

#[test]
fn replacing_a_child_with_itself_queues_adoption_and_combined_addition() {
    let mut document = Document::new();
    let parent = document.create_html_element(atoms::DIV);
    let child = document.create_html_element(atoms::A);
    append(&mut document, parent, child);

    let mutation = Mutation::ReplaceChild {
        parent,
        new_child: child,
        old_child: child,
    };
    let plan = capture_validated(&document, &mutation).unwrap();
    assert_eq!(plan.records.len(), 2);
    assert_eq!(plan.records[0].removed_nodes, vec![node(&document, child)]);
    assert_eq!(plan.records[1].added_nodes, vec![node(&document, child)]);
    assert!(plan.records[1].removed_nodes.is_empty());
    document.mutate(mutation).unwrap();
    assert_eq!(document.parent(child), parent);
}

#[test]
fn moving_a_sibling_captures_pre_adoption_previous_sibling() {
    let mut document = Document::new();
    let parent = document.create_html_element(atoms::DIV);
    let a = document.create_html_element(atoms::A);
    let b = document.create_html_element(atoms::B);
    let c = document.create_html_element(atoms::SPAN);
    for child in [a, b, c] {
        append(&mut document, parent, child);
    }
    let plan = capture_validated(
        &document,
        &Mutation::InsertBefore {
            parent,
            child: b,
            before: c,
        },
    )
    .unwrap();
    assert_eq!(plan.records.len(), 2);
    assert_eq!(plan.records[0].previous_sibling, Some(node(&document, a)));
    assert_eq!(plan.records[1].previous_sibling, Some(node(&document, b)));
    assert_eq!(plan.records[1].next_sibling, Some(node(&document, c)));
}

#[test]
fn empty_fragment_and_detached_removal_have_no_semantic_records() {
    let mut document = Document::new();
    let parent = document.create_html_element(atoms::DIV);
    let fragment = document.create_fragment();
    let child = document.create_html_element(atoms::A);
    assert!(
        capture_validated(
            &document,
            &Mutation::Append {
                parent,
                child: fragment
            }
        )
        .unwrap()
        .records
        .is_empty()
    );
    assert!(
        capture_validated(&document, &Mutation::Remove { child })
            .unwrap()
            .records
            .is_empty()
    );
}

#[test]
fn equal_character_data_write_still_captures_a_record_and_old_value() {
    let mut document = Document::new();
    let text = document.create_text("same");
    let mutation = Mutation::SetText {
        node: text,
        text: "same".to_owned(),
    };
    let plan = capture_validated(&document, &mutation).unwrap();
    assert_eq!(plan.records.len(), 1);
    assert_eq!(
        plan.records[0].record_type,
        super::super::observer_journal::RecordType::CharacterData
    );
    assert_eq!(plan.records[0].old_value.as_deref(), Some("same"));
    document.mutate(mutation).unwrap();
    assert_eq!(document.text_data(text), Some("same"));
}

#[test]
fn parser_reparent_children_captures_each_unlink_then_append_in_native_order() {
    let mut document = Document::new();
    let from = document.create_html_element(atoms::DIV);
    let to = document.create_html_element(atoms::DIV);
    let previous = document.create_html_element(atoms::SPAN);
    let first = document.create_html_element(atoms::A);
    let second = document.create_html_element(atoms::B);
    append(&mut document, to, previous);
    append(&mut document, from, first);
    append(&mut document, from, second);

    let mutation = Mutation::ReparentChildren { from, to };
    let plan = capture_validated(&document, &mutation).unwrap();
    assert_eq!(plan.records.len(), 4);
    assert_eq!(plan.transient_removals.len(), 2);
    assert_eq!(plan.records[0].target, node(&document, from));
    assert_eq!(plan.records[0].removed_nodes, vec![node(&document, first)]);
    assert_eq!(plan.records[0].next_sibling, Some(node(&document, second)));
    assert_eq!(plan.records[1].target, node(&document, to));
    assert_eq!(plan.records[1].added_nodes, vec![node(&document, first)]);
    assert_eq!(
        plan.records[1].previous_sibling,
        Some(node(&document, previous))
    );
    assert_eq!(plan.records[2].target, node(&document, from));
    assert_eq!(plan.records[2].removed_nodes, vec![node(&document, second)]);
    assert_eq!(plan.records[2].previous_sibling, None);
    assert_eq!(plan.records[3].target, node(&document, to));
    assert_eq!(plan.records[3].added_nodes, vec![node(&document, second)]);
    assert_eq!(
        plan.records[3].previous_sibling,
        Some(node(&document, first))
    );

    document.mutate(mutation).unwrap();
    assert!(document.children(from).next().is_none());
    assert_eq!(
        document.children(to).collect::<Vec<_>>(),
        vec![previous, first, second]
    );
}

#[test]
fn same_parent_reparent_tracks_last_removal_after_earlier_children_rotate() {
    let mut document = Document::new();
    let parent = document.create_html_element(atoms::DIV);
    let a = document.create_html_element(atoms::A);
    let b = document.create_html_element(atoms::B);
    let c = document.create_html_element(atoms::SPAN);
    for child in [a, b, c] {
        append(&mut document, parent, child);
    }

    let mutation = Mutation::ReparentChildren {
        from: parent,
        to: parent,
    };
    let plan = capture_validated(&document, &mutation).unwrap();
    assert_eq!(plan.records.len(), 6);
    assert_eq!(plan.records[0].next_sibling, Some(node(&document, b)));
    assert_eq!(plan.records[2].next_sibling, Some(node(&document, c)));
    assert_eq!(plan.records[4].next_sibling, Some(node(&document, a)));
    assert_eq!(plan.records[1].previous_sibling, Some(node(&document, c)));
    assert_eq!(plan.records[3].previous_sibling, Some(node(&document, a)));
    assert_eq!(plan.records[5].previous_sibling, Some(node(&document, b)));

    document.mutate(mutation).unwrap();
    assert_eq!(document.children(parent).collect::<Vec<_>>(), vec![a, b, c]);
}

#[test]
fn replace_all_captures_one_grouped_parent_record_after_source_removal() {
    let mut document = Document::new();
    let parent = document.create_html_element(atoms::DIV);
    let old_a = document.create_html_element(atoms::A);
    let old_b = document.create_html_element(atoms::B);
    let source = document.create_fragment();
    let new_a = document.create_html_element(atoms::SPAN);
    let new_b = document.create_text("new");
    for child in [old_a, old_b] {
        append(&mut document, parent, child);
    }
    for child in [new_a, new_b] {
        append(&mut document, source, child);
    }

    let mutation = Mutation::ReplaceAll {
        parent,
        replacement: ReplaceAllWith::ChildrenOf(source),
    };
    let plan = capture_validated(&document, &mutation).unwrap();
    assert!(plan.request_notification);
    assert_eq!(plan.transient_removals.len(), 4);
    assert_eq!(plan.records.len(), 2);
    assert_eq!(plan.records[0].target, node(&document, source));
    assert!(plan.records[0].added_nodes.is_empty());
    assert_eq!(
        plan.records[0].removed_nodes,
        vec![node(&document, new_a), node(&document, new_b)]
    );
    assert_eq!(plan.records[1].target, node(&document, parent));
    assert_eq!(
        plan.records[1].removed_nodes,
        vec![node(&document, old_a), node(&document, old_b)]
    );
    assert_eq!(
        plan.records[1].added_nodes,
        vec![node(&document, new_a), node(&document, new_b)]
    );
    assert_eq!(plan.records[1].previous_sibling, None);
    assert_eq!(plan.records[1].next_sibling, None);
    assert_eq!(plan.records[1].old_value, None);
    assert_eq!(
        plan.steps,
        vec![
            SemanticStep::InstallTransient(node(&document, old_a)),
            SemanticStep::InstallTransient(node(&document, old_b)),
            SemanticStep::InstallTransient(node(&document, new_a)),
            SemanticStep::InstallTransient(node(&document, new_b)),
            SemanticStep::QueueRecord(node(&document, source)),
            SemanticStep::QueueRecord(node(&document, parent)),
        ]
    );
}

#[test]
fn empty_replace_all_does_not_request_or_capture_a_record() {
    let mut document = Document::new();
    let parent = document.create_html_element(atoms::DIV);
    let mutation = Mutation::ReplaceAll {
        parent,
        replacement: ReplaceAllWith::Empty,
    };
    let plan = capture_validated(&document, &mutation).unwrap();
    assert!(!plan.request_notification);
    assert!(plan.records.is_empty());
    assert!(plan.transient_removals.is_empty());
    assert!(plan.steps.is_empty());
    document.mutate(mutation).unwrap();
    assert_eq!(document.first_child(parent), NodeId::NONE);
}
