//! The single mutation path (`docs/spec/04-dom-and-css.md` §1): links,
//! connectivity, indexes, guards, text merging and the observer log.

mod support;

use tenta_core::atom::atoms;
use tenta_core::{Atom, NodeId};
use tenta_dom::error::dom_exception_name;
use tenta_dom::{Document, Mutation, MutationRecord, NodeKind, RecordingObserverSink};
use tenta_html::{Attribute, ElementFlags, QualName};

fn attr(name: Atom, value: &str) -> Attribute {
    Attribute {
        name: QualName::new(atoms::EMPTY, name),
        value: value.to_owned(),
    }
}

fn document_with_body() -> (Document, NodeId) {
    let mut document = Document::new();
    let html = document.create_html_element(atoms::HTML);
    document
        .mutate(Mutation::Append {
            parent: document.document_node(),
            child: html,
        })
        .expect("append <html>");
    let body = document.create_html_element(atoms::BODY);
    document
        .mutate(Mutation::Append {
            parent: html,
            child: body,
        })
        .expect("append <body>");
    (document, body)
}

#[test]
fn append_and_insert_before_maintain_the_sibling_links() {
    let (mut document, body) = document_with_body();
    let a = document.create_html_element(atoms::P);
    let b = document.create_html_element(atoms::P);
    let c = document.create_html_element(atoms::P);
    for child in [a, b] {
        document
            .mutate(Mutation::Append {
                parent: body,
                child,
            })
            .expect("append");
    }
    document
        .mutate(Mutation::InsertBefore {
            parent: body,
            child: c,
            before: b,
        })
        .expect("insert before");

    assert_eq!(document.children(body).collect::<Vec<_>>(), vec![a, c, b]);
    assert_eq!(document.first_child(body), a);
    assert_eq!(document.last_child(body), b);
    assert_eq!(document.next_sibling(a), c);
    assert_eq!(document.previous_sibling(b), c);
    assert!(!document.previous_sibling(a).is_some());
    assert!(!document.next_sibling(b).is_some());
    assert!([a, b, c].iter().all(|node| document.is_connected(*node)));
}

#[test]
fn removing_a_subtree_clears_connectivity_and_creates_a_detached_root() {
    let (mut document, body) = document_with_body();
    let section = document.create_html_element(atoms::SECTION);
    let child = document.create_html_element(atoms::SPAN);
    document
        .mutate(Mutation::Append {
            parent: body,
            child: section,
        })
        .expect("append");
    document
        .mutate(Mutation::Append {
            parent: section,
            child,
        })
        .expect("append");
    assert!(document.is_connected(child));
    assert_eq!(document.detached_root_count(), 0);

    document
        .mutate(Mutation::Remove { child: section })
        .expect("remove");
    assert!(!document.is_connected(section));
    assert!(!document.is_connected(child));
    assert_eq!(document.detached_root_count(), 1);

    document
        .mutate(Mutation::Append {
            parent: body,
            child: section,
        })
        .expect("re-append");
    assert!(
        document.is_connected(child),
        "connectivity propagates back down"
    );
    assert_eq!(document.detached_root_count(), 0);
}

#[test]
fn cycles_and_invalid_kinds_are_refused() {
    let (mut document, body) = document_with_body();
    let outer = document.create_html_element(atoms::DIV);
    let inner = document.create_html_element(atoms::DIV);
    document
        .mutate(Mutation::Append {
            parent: body,
            child: outer,
        })
        .expect("append");
    document
        .mutate(Mutation::Append {
            parent: outer,
            child: inner,
        })
        .expect("append");

    let error = document
        .mutate(Mutation::Append {
            parent: inner,
            child: outer,
        })
        .expect_err("a node cannot contain itself");
    assert_eq!(dom_exception_name(&error), Some("HierarchyRequestError"));

    let error = document
        .mutate(Mutation::Append {
            parent: outer,
            child: outer,
        })
        .expect_err("a node cannot be its own child");
    assert_eq!(dom_exception_name(&error), Some("HierarchyRequestError"));

    let text = document.create_text("no children here");
    let error = document
        .mutate(Mutation::Append {
            parent: text,
            child: inner,
        })
        .expect_err("text nodes have no children");
    assert_eq!(dom_exception_name(&error), Some("HierarchyRequestError"));

    let error = document
        .mutate(Mutation::Append {
            parent: body,
            child: document.document_node(),
        })
        .expect_err("the document node cannot be inserted");
    assert_eq!(dom_exception_name(&error), Some("HierarchyRequestError"));

    // The tree is unchanged after every refusal.
    assert_eq!(document.parent(outer), body);
    assert_eq!(document.parent(inner), outer);
}

#[test]
fn insert_before_a_node_of_another_parent_is_a_not_found_error() {
    let (mut document, body) = document_with_body();
    let other = document.create_html_element(atoms::DIV);
    let child = document.create_html_element(atoms::P);
    let error = document
        .mutate(Mutation::InsertBefore {
            parent: body,
            child,
            before: other,
        })
        .expect_err("reference node is not a child");
    assert_eq!(dom_exception_name(&error), Some("NotFoundError"));
}

#[test]
fn replace_child_swaps_in_place() {
    let (mut document, body) = document_with_body();
    let (a, b, c) = (
        document.create_html_element(atoms::P),
        document.create_html_element(atoms::P),
        document.create_html_element(atoms::SPAN),
    );
    for child in [a, b] {
        document
            .mutate(Mutation::Append {
                parent: body,
                child,
            })
            .expect("append");
    }
    document
        .mutate(Mutation::ReplaceChild {
            parent: body,
            new_child: c,
            old_child: a,
        })
        .expect("replace");
    assert_eq!(document.children(body).collect::<Vec<_>>(), vec![c, b]);
    assert!(!document.is_connected(a));
}

#[test]
fn inserting_a_fragment_moves_its_children() {
    let (mut document, body) = document_with_body();
    let fragment = document.create_fragment();
    let a = document.create_html_element(atoms::P);
    let b = document.create_html_element(atoms::P);
    for child in [a, b] {
        document
            .mutate(Mutation::Append {
                parent: fragment,
                child,
            })
            .expect("append into fragment");
    }
    document
        .mutate(Mutation::Append {
            parent: body,
            child: fragment,
        })
        .expect("append fragment");
    assert_eq!(document.children(body).collect::<Vec<_>>(), vec![a, b]);
    assert_eq!(document.children(fragment).count(), 0);
    assert!(document.is_connected(a) && document.is_connected(b));
}

#[test]
fn reparent_children_moves_them_in_order_and_refuses_cycles() {
    let (mut document, body) = document_with_body();
    let from = document.create_html_element(atoms::DIV);
    let to = document.create_html_element(atoms::DIV);
    document
        .mutate(Mutation::Append {
            parent: body,
            child: from,
        })
        .expect("append");
    document
        .mutate(Mutation::Append {
            parent: body,
            child: to,
        })
        .expect("append");
    let children: Vec<NodeId> = (0..3)
        .map(|_| document.create_html_element(atoms::SPAN))
        .collect();
    for child in &children {
        document
            .mutate(Mutation::Append {
                parent: from,
                child: *child,
            })
            .expect("append");
    }
    document
        .mutate(Mutation::ReparentChildren { from, to })
        .expect("reparent");
    assert_eq!(document.children(to).collect::<Vec<_>>(), children);
    assert_eq!(document.children(from).count(), 0);

    let nested = document.create_html_element(atoms::DIV);
    document
        .mutate(Mutation::Append {
            parent: to,
            child: nested,
        })
        .expect("append");
    let error = document
        .mutate(Mutation::ReparentChildren {
            from: to,
            to: nested,
        })
        .expect_err("children cannot move into their own subtree");
    assert_eq!(dom_exception_name(&error), Some("HierarchyRequestError"));
}

#[test]
fn attributes_update_the_id_and_class_indexes() {
    let (mut document, body) = document_with_body();
    let element = document
        .create_element(
            QualName::new(atoms::NS_HTML, atoms::DIV),
            vec![attr(atoms::ID, "one"), attr(atoms::CLASS, "a b")],
            ElementFlags::default(),
        )
        .expect("create HTML element");
    document
        .mutate(Mutation::Append {
            parent: body,
            child: element,
        })
        .expect("append");

    assert_eq!(document.get_element_by_id("one"), Some(element));
    assert_eq!(document.get_elements_by_class_name("a"), vec![element]);
    assert_eq!(document.get_elements_by_class_name("b"), vec![element]);
    assert_eq!(document.class_count(element), 2);

    document
        .mutate(Mutation::SetAttribute {
            element,
            name: QualName::new(atoms::EMPTY, atoms::ID),
            value: "two".to_owned(),
        })
        .expect("set id");
    assert_eq!(document.get_element_by_id("one"), None);
    assert_eq!(document.get_element_by_id("two"), Some(element));

    document
        .mutate(Mutation::SetAttribute {
            element,
            name: QualName::new(atoms::EMPTY, atoms::CLASS),
            value: "c".to_owned(),
        })
        .expect("set class");
    assert!(document.get_elements_by_class_name("a").is_empty());
    assert_eq!(document.get_elements_by_class_name("c"), vec![element]);

    document
        .mutate(Mutation::RemoveAttribute {
            element,
            name: QualName::new(atoms::EMPTY, atoms::ID),
        })
        .expect("remove id");
    assert_eq!(document.get_element_by_id("two"), None);
    assert_eq!(document.attribute(element, atoms::ID), None);
    assert_eq!(document.element_id(element), None);
}

#[test]
fn repeated_attribute_writes_stay_bounded() {
    let (mut document, body) = document_with_body();
    let element = document.create_html_element(atoms::DIV);
    document
        .mutate(Mutation::Append {
            parent: body,
            child: element,
        })
        .expect("append");
    for index in 0..20_000 {
        document
            .mutate(Mutation::SetAttribute {
                element,
                name: QualName::new(atoms::EMPTY, atoms::VALUE),
                value: format!("value-number-{index}"),
            })
            .expect("set attribute");
    }
    assert_eq!(
        document.attribute(element, atoms::VALUE),
        Some("value-number-19999")
    );
    let report = document.memory_report();
    println!(
        "after 20 000 attribute writes: {} attribute records, {} value bytes",
        report.attributes, report.attribute_values
    );
    assert!(
        report.attribute_values < 64 * 1024,
        "the shared value buffer must be compacted, got {} bytes",
        report.attribute_values
    );
}

#[test]
fn adjacent_text_runs_merge_into_one_node() {
    let (mut document, body) = document_with_body();
    let paragraph = document.create_html_element(atoms::P);
    document
        .mutate(Mutation::Append {
            parent: body,
            child: paragraph,
        })
        .expect("append");

    document.append_text(paragraph, "Hello, ").expect("text");
    document.append_text(paragraph, "world").expect("text");
    document.append_text(paragraph, "!").expect("text");

    let children: Vec<NodeId> = document.children(paragraph).collect();
    assert_eq!(children.len(), 1, "the tree builder merges adjacent text");
    assert_eq!(document.kind(children[0]), NodeKind::Text);
    assert_eq!(document.text_data(children[0]), Some("Hello, world!"));
}

#[test]
fn setting_text_is_copy_on_write_and_keeps_old_chunks_valid() {
    let (mut document, body) = document_with_body();
    let text = document.create_text("original text long enough to be interesting");
    document
        .mutate(Mutation::Append {
            parent: body,
            child: text,
        })
        .expect("append");
    // Sealing hands out an Arc, exactly as an external JavaScript string does.
    let chunk = document.text_chunk(text).expect("a text chunk");
    let before = chunk.as_str().to_owned();

    document
        .mutate(Mutation::SetText {
            node: text,
            text: "replacement".to_owned(),
        })
        .expect("set text");
    assert_eq!(document.text_data(text), Some("replacement"));
    assert_eq!(
        chunk.as_str(),
        before,
        "the chunk an external string holds must never change"
    );
}

#[test]
fn text_chunks_seal_at_the_rope_budget_and_report_latin1() {
    let (mut document, body) = document_with_body();
    for index in 0..500 {
        let paragraph = document.create_html_element(atoms::P);
        document
            .mutate(Mutation::Append {
                parent: body,
                child: paragraph,
            })
            .expect("append");
        document
            .append_text(
                paragraph,
                &format!("paragraph number {index} of the rope test"),
            )
            .expect("text");
    }
    let (chunks, sealed, open) = document.rope_stats();
    println!("rope: {chunks} sealed chunks, {sealed} sealed bytes, {open} open");
    assert!(chunks > 1, "the rope must seal chunks as it fills");
    assert!(sealed / chunks >= tenta_dom::CHUNK_TARGET);
    assert!(sealed / chunks <= tenta_dom::CHUNK_MAX);

    let first = document.first_child(document.first_child(body));
    assert_eq!(
        document.text_data(first),
        Some("paragraph number 0 of the rope test")
    );
    let chunk = document.text_chunk(first).expect("chunk");
    assert!(chunk.is_latin1(), "ASCII text yields a one-byte chunk");
}

#[test]
fn the_observer_sink_sees_every_change_in_order() {
    let (mut document, body) = document_with_body();
    let sink = RecordingObserverSink::new();
    document.set_observer_sink(Box::new(sink.clone()));

    let element = document.create_html_element(atoms::DIV);
    document
        .mutate(Mutation::Append {
            parent: body,
            child: element,
        })
        .expect("append");
    document
        .mutate(Mutation::SetAttribute {
            element,
            name: QualName::new(atoms::EMPTY, atoms::ID),
            value: "x".to_owned(),
        })
        .expect("set");
    document
        .mutate(Mutation::Remove { child: element })
        .expect("remove");

    let records = sink.take();
    assert_eq!(records.len(), 3, "got {records:?}");
    assert!(matches!(
        records[0],
        MutationRecord::Inserted { parent, node, .. } if parent == body && node == element
    ));
    assert!(matches!(
        &records[1],
        MutationRecord::Attribute { element: e, old_value: None, .. } if *e == element
    ));
    assert!(matches!(
        records[2],
        MutationRecord::Removed { parent, node } if parent == body && node == element
    ));
}

#[test]
fn the_mutation_generation_moves_on_every_change() {
    let (mut document, body) = document_with_body();
    let before = document.mutation_generation();
    let element = document.create_html_element(atoms::DIV);
    assert_eq!(
        document.mutation_generation(),
        before,
        "creating a detached node is not a tree mutation"
    );
    document
        .mutate(Mutation::Append {
            parent: body,
            child: element,
        })
        .expect("append");
    assert!(document.mutation_generation() > before);
    let after_append = document.mutation_generation();
    document
        .mutate(Mutation::SetAttribute {
            element,
            name: QualName::new(atoms::EMPTY, atoms::CLASS),
            value: "y".to_owned(),
        })
        .expect("set");
    assert!(document.mutation_generation() > after_append);
}

#[test]
fn base_href_changes_the_resolution_base() {
    let mut document = Document::with_url("https://example.test/a/b/page.html".parse().unwrap());
    let html = document.create_html_element(atoms::HTML);
    document
        .mutate(Mutation::Append {
            parent: document.document_node(),
            child: html,
        })
        .expect("append");
    assert_eq!(
        document.resolve_url("c.html").map(|u| u.to_string()),
        Some("https://example.test/a/b/c.html".to_owned())
    );

    let base = document
        .create_element(
            QualName::new(atoms::NS_HTML, Atom::new("base")),
            vec![attr(atoms::HREF, "/root/")],
            ElementFlags::default(),
        )
        .expect("create HTML element");
    document
        .mutate(Mutation::Append {
            parent: html,
            child: base,
        })
        .expect("append <base>");
    assert_eq!(
        document.base_url().map(|u| u.to_string()),
        Some("https://example.test/root/".to_owned())
    );
    assert_eq!(
        document.resolve_url("c.html").map(|u| u.to_string()),
        Some("https://example.test/root/c.html".to_owned())
    );
}
