//! `ParentNode` / `ChildNode` convenience mutations: the DOM Standard's
//! algorithms, their exception names and the records they leave behind.

use tenta_core::NodeId;
use tenta_core::atom::atoms;
use tenta_dom::error::dom_exception_name;
use tenta_dom::{Document, Mutation, MutationRecord, NodeKind, NodeOrText, RecordingObserverSink};

fn text(data: &str) -> NodeOrText {
    NodeOrText::Text(data.to_owned())
}

fn element(document: &mut Document) -> NodeId {
    document.create_html_element(atoms::DIV)
}

fn parent_with(document: &mut Document, count: usize) -> (NodeId, Vec<NodeId>) {
    let parent = element(document);
    let children: Vec<NodeId> = (0..count).map(|_| element(document)).collect();
    document
        .append_nodes(
            parent,
            children.iter().map(|c| NodeOrText::Node(*c)).collect(),
        )
        .expect("append children");
    (parent, children)
}

fn children(document: &Document, parent: NodeId) -> Vec<NodeId> {
    document.children(parent).collect()
}

fn texts(document: &Document, parent: NodeId) -> Vec<String> {
    document
        .children(parent)
        .map(|child| document.text_data(child).unwrap_or("<node>").to_owned())
        .collect()
}

#[test]
fn append_keeps_each_string_as_its_own_unmerged_text_node() {
    let mut document = Document::new();
    let parent = element(&mut document);
    document
        .append_nodes(parent, vec![text("a"), text("b"), text("")])
        .expect("append");
    assert_eq!(texts(&document, parent), ["a", "b", ""]);
}

#[test]
fn prepend_inserts_before_the_first_child_in_argument_order() {
    let mut document = Document::new();
    let (parent, kids) = parent_with(&mut document, 2);
    document
        .prepend_nodes(parent, vec![text("x"), NodeOrText::Node(kids[1])])
        .expect("prepend");
    let after = children(&document, parent);
    assert_eq!(after.len(), 3);
    assert_eq!(document.text_data(after[0]), Some("x"));
    assert_eq!(&after[1..], &[kids[1], kids[0]]);
}

#[test]
fn before_skips_preceding_siblings_that_are_arguments() {
    let mut document = Document::new();
    let (parent, kids) = parent_with(&mut document, 3);
    document
        .before_nodes(
            kids[2],
            vec![NodeOrText::Node(kids[1]), NodeOrText::Node(kids[0])],
        )
        .expect("before");
    assert_eq!(children(&document, parent), [kids[1], kids[0], kids[2]]);
}

#[test]
fn after_skips_following_siblings_that_are_arguments() {
    let mut document = Document::new();
    let (parent, kids) = parent_with(&mut document, 3);
    document
        .after_nodes(kids[0], vec![NodeOrText::Node(kids[1]), text("t")])
        .expect("after");
    let after = children(&document, parent);
    assert_eq!(after.len(), 4);
    assert_eq!(&after[..3], &[kids[0], kids[1], after[2]]);
    assert_eq!(document.text_data(after[2]), Some("t"));
    assert_eq!(after[3], kids[2]);
}

#[test]
fn replace_with_may_name_the_context_object_among_its_arguments() {
    let mut document = Document::new();
    let (parent, kids) = parent_with(&mut document, 2);
    document
        .replace_with_nodes(
            kids[0],
            vec![NodeOrText::Node(kids[1]), NodeOrText::Node(kids[0])],
        )
        .expect("replaceWith");
    assert_eq!(children(&document, parent), [kids[1], kids[0]]);
}

#[test]
fn replace_with_nothing_removes_the_node() {
    let mut document = Document::new();
    let (parent, kids) = parent_with(&mut document, 2);
    document
        .replace_with_nodes(kids[0], Vec::new())
        .expect("replaceWith");
    assert_eq!(children(&document, parent), [kids[1]]);
    assert!(!document.parent(kids[0]).is_some());
}

#[test]
fn child_node_methods_ignore_a_node_without_a_parent() {
    let mut document = Document::new();
    let lone = element(&mut document);
    let other = element(&mut document);
    document
        .before_nodes(lone, vec![NodeOrText::Node(other)])
        .expect("before");
    document
        .after_nodes(lone, vec![NodeOrText::Node(other)])
        .expect("after");
    document
        .replace_with_nodes(lone, vec![NodeOrText::Node(other)])
        .expect("replaceWith");
    assert!(!document.parent(other).is_some());
}

#[test]
fn replace_children_swaps_every_child_for_the_converted_nodes() {
    let mut document = Document::new();
    let (parent, kids) = parent_with(&mut document, 2);
    document
        .replace_children(parent, vec![text("x"), NodeOrText::Node(kids[1])])
        .expect("replaceChildren");
    let after = children(&document, parent);
    assert_eq!(after.len(), 2);
    assert_eq!(document.text_data(after[0]), Some("x"));
    assert_eq!(after[1], kids[1]);
    assert!(!document.parent(kids[0]).is_some());

    document
        .replace_children(parent, Vec::new())
        .expect("replaceChildren()");
    assert!(children(&document, parent).is_empty());
}

#[test]
fn replace_children_with_a_child_of_the_parent_keeps_only_that_child() {
    let mut document = Document::new();
    let (parent, kids) = parent_with(&mut document, 3);
    document
        .replace_children(parent, vec![NodeOrText::Node(kids[1])])
        .expect("replaceChildren");
    assert_eq!(children(&document, parent), [kids[1]]);
}

// The children being removed are ignored by the hierarchy check: WPT
// `dom/nodes/ParentNode-replaceChildren.html` lines 126-150 (and the skips at
// `pre-insertion-validation-hierarchy.js` lines 54-88) expect a document that
// holds a doctype and an element to accept a new element or doctype.
#[test]
fn document_replace_children_ignores_the_children_it_removes() {
    let mut document = Document::new();
    let root = document.document_node();
    let doctype = document.create_doctype("html", "", "");
    let old_root = element(&mut document);
    for child in [doctype, old_root] {
        document
            .mutate(Mutation::Append {
                parent: root,
                child,
            })
            .expect("doctype and root");
    }

    let new_root = element(&mut document);
    document
        .replace_children(root, vec![NodeOrText::Node(new_root)])
        .expect("replace with an element");
    assert_eq!(children(&document, root), [new_root]);

    document
        .replace_children(root, vec![NodeOrText::Node(doctype)])
        .expect("replace with a doctype");
    assert_eq!(children(&document, root), [doctype]);
    assert_eq!(document.kind(doctype), NodeKind::DocType);
}

#[test]
fn hierarchy_violations_are_named_and_leave_the_tree_unchanged() {
    let mut document = Document::new();
    let root = document.document_node();
    let existing = element(&mut document);
    document
        .append_nodes(root, vec![NodeOrText::Node(existing)])
        .expect("root element");
    let (outer, kids) = parent_with(&mut document, 1);
    let first = element(&mut document);
    let second = element(&mut document);

    let named = |result: tenta_core::Result<()>| {
        dom_exception_name(&result.expect_err("must fail")).map(str::to_owned)
    };
    let hierarchy = Some("HierarchyRequestError".to_owned());

    assert_eq!(
        named(document.replace_children(root, vec![text("t")])),
        hierarchy
    );
    assert_eq!(
        named(document.replace_children(
            root,
            vec![NodeOrText::Node(first), NodeOrText::Node(second)]
        )),
        hierarchy
    );
    assert_eq!(
        named(document.append_nodes(root, vec![text("t")])),
        hierarchy
    );
    assert_eq!(
        named(document.after_nodes(existing, vec![NodeOrText::Node(first)])),
        hierarchy
    );
    assert_eq!(
        named(document.append_nodes(kids[0], vec![NodeOrText::Node(outer)])),
        hierarchy
    );
    assert_eq!(
        named(document.replace_children(outer, vec![NodeOrText::Node(outer)])),
        hierarchy
    );
    assert_eq!(
        named(document.replace_children(kids[0], vec![NodeOrText::Node(outer)])),
        hierarchy
    );
    assert_eq!(children(&document, root), [existing]);
    assert_eq!(children(&document, outer), kids);
    assert_eq!(document.parent(outer), NodeId::NONE);
}

#[test]
fn moving_a_node_reports_its_removal_before_its_insertion() {
    let mut document = Document::new();
    let (old_parent, old_kids) = parent_with(&mut document, 1);
    let (parent, kids) = parent_with(&mut document, 1);
    let sink = RecordingObserverSink::new();
    document.set_observer_sink(Box::new(sink.clone()));

    document
        .after_nodes(kids[0], vec![NodeOrText::Node(old_kids[0])])
        .expect("after");

    let records = sink.take();
    assert_eq!(
        records,
        [
            MutationRecord::Removed {
                parent: old_parent,
                node: old_kids[0],
            },
            MutationRecord::Inserted {
                parent,
                node: old_kids[0],
                previous_sibling: kids[0],
            },
        ]
    );
}

#[test]
fn replace_children_with_a_child_of_the_parent_adopts_it_then_replaces_all() {
    let mut document = Document::new();
    let (parent, kids) = parent_with(&mut document, 3);
    let sink = RecordingObserverSink::new();
    document.set_observer_sink(Box::new(sink.clone()));

    document
        .replace_children(parent, vec![NodeOrText::Node(kids[1])])
        .expect("replaceChildren");

    // Adopting the node removes it from `parent` (its own record), then the
    // replace-all removes what is left and inserts the node.
    let on_parent: Vec<MutationRecord> = sink
        .take()
        .into_iter()
        .filter(|record| match record {
            MutationRecord::Removed { parent: p, .. }
            | MutationRecord::Inserted { parent: p, .. } => *p == parent,
            _ => false,
        })
        .collect();
    assert_eq!(
        on_parent,
        [
            MutationRecord::Removed {
                parent,
                node: kids[1],
            },
            MutationRecord::Removed {
                parent,
                node: kids[0],
            },
            MutationRecord::Removed {
                parent,
                node: kids[2],
            },
            MutationRecord::Inserted {
                parent,
                node: kids[1],
                previous_sibling: NodeId::NONE,
            },
        ]
    );
    assert_eq!(children(&document, parent), [kids[1]]);
}

#[test]
fn replace_children_leaves_one_removal_per_old_child_and_one_insertion_per_new_one() {
    let mut document = Document::new();
    let (parent, kids) = parent_with(&mut document, 2);
    let incoming = element(&mut document);
    let sink = RecordingObserverSink::new();
    document.set_observer_sink(Box::new(sink.clone()));

    document
        .replace_children(parent, vec![NodeOrText::Node(incoming)])
        .expect("replaceChildren");

    let records = sink.take();
    let removed: Vec<NodeId> = records
        .iter()
        .filter_map(|record| match record {
            MutationRecord::Removed { parent: p, node } if *p == parent => Some(*node),
            _ => None,
        })
        .collect();
    assert_eq!(removed, kids);
    assert!(records.iter().any(|record| matches!(
        record,
        MutationRecord::Inserted { parent: p, node, .. } if *p == parent && *node == incoming
    )));
    assert_eq!(children(&document, parent), [incoming]);
}

#[test]
fn replace_all_refuses_a_text_node_for_a_document_parent() {
    let mut document = Document::new();
    let root = document.document_node();
    let text_node = document.create_text("t");
    let error = document
        .mutate(Mutation::ReplaceAll {
            parent: root,
            replacement: tenta_dom::ReplaceAllWith::Detached(text_node),
        })
        .expect_err("a document cannot hold text");
    assert_eq!(dom_exception_name(&error), Some("HierarchyRequestError"));
}

#[test]
fn a_node_named_as_its_own_argument_stays_where_it_is() {
    let mut document = Document::new();
    let (parent, kids) = parent_with(&mut document, 3);
    let x = kids[1];
    document
        .replace_with_nodes(x, vec![NodeOrText::Node(x)])
        .expect("x.replaceWith(x)");
    document
        .before_nodes(x, vec![NodeOrText::Node(x)])
        .expect("x.before(x)");
    document
        .after_nodes(x, vec![NodeOrText::Node(x)])
        .expect("x.after(x)");
    assert_eq!(children(&document, parent), kids);
}

#[test]
fn before_after_and_replace_with_record_the_move_and_the_insertion() {
    let mut document = Document::new();
    let (parent, kids) = parent_with(&mut document, 2);
    let extra = element(&mut document);
    let sink = RecordingObserverSink::new();
    document.set_observer_sink(Box::new(sink.clone()));

    document
        .before_nodes(kids[1], vec![NodeOrText::Node(extra)])
        .expect("before");
    assert_eq!(
        sink.take(),
        [MutationRecord::Inserted {
            parent,
            node: extra,
            previous_sibling: kids[0],
        }]
    );

    document
        .after_nodes(kids[1], vec![NodeOrText::Node(kids[0])])
        .expect("after");
    assert_eq!(
        sink.take(),
        [
            MutationRecord::Removed {
                parent,
                node: kids[0],
            },
            MutationRecord::Inserted {
                parent,
                node: kids[0],
                previous_sibling: kids[1],
            },
        ]
    );

    document
        .replace_with_nodes(extra, vec![NodeOrText::Node(kids[1])])
        .expect("replaceWith");
    assert_eq!(
        sink.take(),
        [
            MutationRecord::Removed {
                parent,
                node: kids[1],
            },
            MutationRecord::Removed {
                parent,
                node: extra,
            },
            MutationRecord::Inserted {
                parent,
                node: kids[1],
                previous_sibling: NodeId::NONE,
            },
        ]
    );
}
