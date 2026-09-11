//! Node lifetime and the sweep (`docs/spec/04-dom-and-css.md` §1.1).
//!
//! The spec's tests (a), (b) and (d) are written against JavaScript wrappers;
//! Tier 1 has no bindings, so they are run against Rust `NodeHandle`s, which
//! the same rule protects: a detached subtree is freed only when no node in it
//! has a wrapper or a live handle.

mod support;

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation, NodeKind};

fn detached_element(document: &mut Document) -> tenta_core::NodeId {
    document.create_html_element(atoms::DIV)
}

#[test]
fn a_detached_elements_without_handles_are_reclaimed() {
    const COUNT: usize = 100_000;
    let mut document = Document::new();
    let mut handles = Vec::with_capacity(COUNT);
    for _ in 0..COUNT {
        let element = detached_element(&mut document);
        handles.push(document.handle(element).expect("live node"));
    }
    let slots_before = document.slots();
    for handle in handles {
        document.release(handle);
    }

    let stats = document.sweep_detached();
    println!(
        "sweep: {} roots examined, {} kept, {} nodes freed of {} slots",
        stats.roots_examined, stats.roots_kept, stats.nodes_freed, slots_before
    );
    let reclaimed = stats.nodes_freed as f64 / COUNT as f64;
    assert!(
        reclaimed >= 0.99,
        "reclaimed {reclaimed:.4} of the detached elements, below the 99% of 04 §8"
    );
    // The document node itself is the only thing left alive.
    assert_eq!(document.len(), 1);
    assert_eq!(document.detached_root_count(), 0);
}

#[test]
fn b_one_handle_keeps_a_thousand_node_subtree_alive() {
    const DEPTH: usize = 1_000;
    let mut document = Document::new();
    let root = detached_element(&mut document);
    let mut current = root;
    for _ in 1..DEPTH {
        let child = detached_element(&mut document);
        document
            .mutate(Mutation::Append {
                parent: current,
                child,
            })
            .expect("append");
        current = child;
    }
    // The handle is on the deepest node, so keeping the subtree cannot be a
    // side effect of holding its root.
    let handle = document.handle(current).expect("live node");
    let live_before = document.len();

    let stats = document.sweep_detached();
    assert_eq!(stats.roots_kept, 1, "the subtree must survive the sweep");
    assert_eq!(stats.nodes_freed, 0);
    assert_eq!(document.len(), live_before);

    // Every node is still reachable through the first_child chain.
    let mut walked = 1;
    let mut node = root;
    while document.first_child(node).is_some() {
        node = document.first_child(node);
        walked += 1;
    }
    assert_eq!(walked, DEPTH);
    assert_eq!(node, handle.id());
    assert!(document.resolve(handle.node_ref()).is_ok());
    document.release(handle);

    // With the handle gone the whole subtree is reclaimed.
    let stats = document.sweep_detached();
    assert_eq!(stats.nodes_freed, DEPTH);
    assert_eq!(document.len(), 1);
}

#[test]
fn d_a_handle_to_a_connected_node_survives_removing_its_grandparent() {
    let mut document = Document::new();
    let html = document.create_html_element(atoms::HTML);
    document
        .mutate(Mutation::Append {
            parent: document.document_node(),
            child: html,
        })
        .expect("append <html>");
    let grandparent = document.create_html_element(atoms::SECTION);
    let parent = document.create_html_element(atoms::DIV);
    let child = document.create_html_element(atoms::SPAN);
    document
        .mutate(Mutation::Append {
            parent: html,
            child: grandparent,
        })
        .expect("append");
    document
        .mutate(Mutation::Append {
            parent: grandparent,
            child: parent,
        })
        .expect("append");
    document
        .mutate(Mutation::Append { parent, child })
        .expect("append");

    let handle = document.handle(child).expect("live node");
    assert!(document.is_connected(child));

    document
        .mutate(Mutation::Remove { child: grandparent })
        .expect("remove");
    assert!(!document.is_connected(child));

    let stats = document.sweep_detached();
    assert_eq!(stats.roots_kept, 1);
    assert_eq!(stats.nodes_freed, 0);

    let resolved = document
        .resolve(handle.node_ref())
        .expect("handle resolves");
    assert_eq!(resolved, child);
    assert_eq!(document.parent(child), parent);
    assert_eq!(document.parent(parent), grandparent);
    assert!(!document.parent(grandparent).is_some());
    document.release(handle);
}

#[test]
fn wrappers_keep_a_subtree_and_clearing_one_releases_it() {
    let mut document = Document::new();
    let root = detached_element(&mut document);
    let child = detached_element(&mut document);
    document
        .mutate(Mutation::Append {
            parent: root,
            child,
        })
        .expect("append");
    document.set_has_wrapper(child, true);

    assert_eq!(document.sweep_detached().roots_kept, 1);
    // The second sweep uses the cached keep bit.
    let cached = document.sweep_detached();
    assert_eq!(cached.roots_cached, 1);
    assert_eq!(cached.roots_kept, 1);

    document.set_has_wrapper(child, false);
    let stats = document.sweep_detached();
    assert_eq!(
        stats.nodes_freed, 2,
        "clearing the wrapper invalidates the keep bit"
    );
}

#[test]
fn freed_slots_are_recycled_with_a_bumped_generation() {
    let mut document = Document::new();
    let element = detached_element(&mut document);
    let reference = document.node_ref(element);
    assert_eq!(document.resolve(reference).expect("live"), element);

    document.sweep_detached();
    let error = document.resolve(reference).expect_err("stale reference");
    assert_eq!(
        tenta_dom::error::dom_exception_name(&error),
        Some("InvalidStateError")
    );

    let recycled = detached_element(&mut document);
    assert_eq!(recycled, element, "the slot is reused");
    assert_ne!(
        document.node_ref(recycled).generation,
        reference.generation,
        "the generation must move so the old reference cannot alias"
    );
    assert!(document.resolve(reference).is_err());
}

#[test]
fn template_contents_are_not_swept_while_the_template_lives() {
    let mut document = Document::new();
    let template = document
        .create_element(
            tenta_html::QualName::new(atoms::NS_HTML, atoms::TEMPLATE),
            Vec::new(),
            tenta_html::ElementFlags {
                template: true,
                ..Default::default()
            },
        )
        .expect("create HTML element");
    let contents = document.template_contents(template).expect("contents");
    assert_eq!(document.kind(contents), NodeKind::Fragment);
    let inner = detached_element(&mut document);
    document
        .mutate(Mutation::Append {
            parent: contents,
            child: inner,
        })
        .expect("append into template contents");
    let handle = document.handle(template).expect("live node");

    let stats = document.sweep_detached();
    assert_eq!(stats.nodes_freed, 0);
    assert!(document.is_live(contents));
    assert!(document.is_live(inner));

    document.release(handle);
    let stats = document.sweep_detached();
    assert_eq!(
        stats.nodes_freed, 3,
        "template, contents fragment and the element inside it are freed together"
    );
}

#[test]
fn a_handle_inside_template_contents_keeps_the_template() {
    let mut document = Document::new();
    let template = document
        .create_element(
            tenta_html::QualName::new(atoms::NS_HTML, atoms::TEMPLATE),
            Vec::new(),
            tenta_html::ElementFlags {
                template: true,
                ..Default::default()
            },
        )
        .expect("create HTML element");
    let contents = document.template_contents(template).expect("contents");
    let inner = detached_element(&mut document);
    document
        .mutate(Mutation::Append {
            parent: contents,
            child: inner,
        })
        .expect("append");
    let handle = document.handle(inner).expect("live node");

    assert_eq!(document.sweep_detached().nodes_freed, 0);
    assert!(document.is_live(template));
    document.release(handle);
    assert_eq!(document.sweep_detached().nodes_freed, 3);
}

#[test]
fn cloned_handles_are_counted_independently() {
    let mut document = Document::new();
    let element = detached_element(&mut document);
    let first = document.handle(element).expect("live");
    let second = document.clone_handle(&first).expect("clone");
    assert_eq!(document.handle_count(element), 2);

    document.release(first);
    assert_eq!(document.handle_count(element), 1);
    assert_eq!(document.sweep_detached().nodes_freed, 0);

    document.release(second);
    assert_eq!(document.handle_count(element), 0);
    assert_eq!(document.sweep_detached().nodes_freed, 1);
}

#[test]
fn dirty_form_state_does_not_survive_the_slot_being_reused() {
    // A freed slot comes back under a fresh generation for the next element;
    // if the dirty value map still held the old key, the new control would
    // report the previous one's typed text (05 §9a).
    let mut document = Document::new();
    let input = document.create_html_element(atoms::INPUT);
    document.set_dirty_value(input, "typed".to_owned());
    document.set_dirty_checkedness(input, true);
    assert_eq!(document.dirty_value(input), Some("typed"));

    assert_eq!(document.sweep_detached().nodes_freed, 1);

    let reused = document.create_html_element(atoms::INPUT);
    assert_eq!(document.dirty_value(reused), None);
    assert_eq!(document.dirty_checkedness(reused), None);
}
