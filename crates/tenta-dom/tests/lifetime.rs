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
    let wrapped = document.node_ref(child);
    document.retain_wrapper(wrapped).expect("live node wrapper");

    assert_eq!(document.sweep_detached().roots_kept, 1);
    // The second sweep uses the cached keep bit.
    let cached = document.sweep_detached();
    assert_eq!(cached.roots_cached, 1);
    assert_eq!(cached.roots_kept, 1);

    assert!(document.release_wrapper(wrapped));
    let stats = document.sweep_detached();
    assert_eq!(
        stats.nodes_freed, 2,
        "clearing the wrapper invalidates the keep bit"
    );
}

#[test]
fn independent_wrapper_claims_release_only_their_own_ownership() {
    let mut document = Document::new();
    let root = detached_element(&mut document);
    let child = detached_element(&mut document);
    document
        .mutate(Mutation::Append {
            parent: root,
            child,
        })
        .unwrap();
    let node = document.node_ref(child);
    document.retain_wrapper(node).unwrap();
    let single = document.memory_report();
    for _ in 0..3 {
        document.retain_wrapper(node).unwrap();
    }
    assert_eq!(document.wrapper_count(child), 4);
    assert!(document.memory_report().indexes > single.indexes);
    assert_eq!(
        document.handle_count(child),
        0,
        "JS claims are not Rust roots"
    );
    assert_eq!(document.subtree_retention(root), (false, 4));
    assert_eq!(document.sweep_detached().nodes_freed, 0);
    for expected in (1..4).rev() {
        assert!(document.release_wrapper(node));
        assert_eq!(document.wrapper_count(child), expected);
        assert_eq!(document.sweep_detached().nodes_freed, 0);
    }
    assert!(document.release_wrapper(node));
    assert_eq!(document.sweep_detached().nodes_freed, 2);
    assert!(!document.release_wrapper(node));
}

#[test]
fn stale_wrapper_claims_cannot_acquire_or_release_a_reused_slot() {
    let mut document = Document::new();
    let old = detached_element(&mut document);
    let stale = document.node_ref(old);
    document.retain_wrapper(stale).unwrap();
    document.release_wrapper(stale);
    assert_eq!(document.sweep_detached().nodes_freed, 1);
    let new = detached_element(&mut document);
    assert_eq!(old, new);
    let current = document.node_ref(new);
    assert_ne!(stale.generation, current.generation);
    document.retain_wrapper(current).unwrap();
    assert!(document.retain_wrapper(stale).is_err());
    assert!(!document.release_wrapper(stale));
    assert_eq!(document.wrapper_count(new), 1);
    assert_eq!(document.sweep_detached().nodes_freed, 0);
    document.release_wrapper(current);
    assert_eq!(document.sweep_detached().nodes_freed, 1);
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

/// The sweep records which nodes it freed so the engine can release the
/// per-world state that was keyed by them (a freed node's event listeners,
/// 04 §1.1). A listener can be registered on any node of a freed subtree, not
/// only on the freed root, so the record has to cover the whole subtree; and it
/// records the reference the node lived under, generation included, because
/// that is what the listener table is keyed by.
#[test]
fn a_sweep_records_every_node_it_freed_subtree_included() {
    let mut document = Document::new();
    let root = detached_element(&mut document);
    let child = detached_element(&mut document);
    let grandchild = detached_element(&mut document);
    document
        .mutate(Mutation::Append {
            parent: root,
            child,
        })
        .expect("append");
    document
        .mutate(Mutation::Append {
            parent: child,
            child: grandchild,
        })
        .expect("append");

    // Taken while the nodes are live: the recorded generation is the one they
    // lived under, which the arena will bump past as it frees each slot.
    let expected: Vec<_> = [root, child, grandchild]
        .into_iter()
        .map(|id| document.node_ref(id))
        .collect();

    assert_eq!(document.sweep_detached().nodes_freed, 3);

    let mut recorded = document.sweep_freed_nodes().to_vec();
    assert_eq!(
        recorded.len(),
        3,
        "every freed node is recorded, not only the freed root"
    );
    recorded.sort_unstable_by_key(|node| node.id);
    let mut expected = expected;
    expected.sort_unstable_by_key(|node| node.id);
    assert_eq!(recorded, expected, "the record is the freed nodes exactly");

    // The sweep freed them, so the recorded reference no longer resolves: the
    // node is gone even though the id may name a live node again afterwards.
    for node in &recorded {
        assert!(
            document.resolve(*node).is_err(),
            "{node:?} is recorded but no longer resolves"
        );
    }

    // The arena hands a freed slot back under a bumped generation. That
    // difference is what makes an exact-key drain correct: the freed node's
    // listener entries stay keyed under the generation it lived under, while a
    // listener on the node that took the slot is keyed under the new one.
    let reused = document.create_html_element(atoms::SPAN);
    let recorded_reused = recorded
        .iter()
        .find(|node| node.id == reused)
        .expect("a freed slot is reused");
    assert_ne!(
        document.node_ref(reused).generation,
        recorded_reused.generation,
        "the reused slot carries a bumped generation, so the freed node's \
         recorded key cannot match the node that took its slot"
    );
}

/// Most sweeps free nothing, so the record must cost nothing then: it is a
/// buffer reused across sweeps, not one allocated per sweep (the reported
/// bookkeeping bytes do not move across a no-op sweep).
#[test]
fn a_sweep_that_frees_nothing_records_nothing_and_reuses_its_buffer() {
    let mut document = Document::new();
    document.sweep_detached();
    assert!(
        document.sweep_freed_nodes().is_empty(),
        "nothing has been freed yet"
    );
    let baseline = document.memory_report().indexes;
    document.sweep_detached();
    assert!(document.sweep_freed_nodes().is_empty());
    assert_eq!(
        document.memory_report().indexes,
        baseline,
        "a sweep that frees nothing must not commit any memory"
    );

    // Free a subtree: the buffer grows to hold its nodes and stays holding that
    // capacity for the next sweep.
    let root = detached_element(&mut document);
    let child = detached_element(&mut document);
    document
        .mutate(Mutation::Append {
            parent: root,
            child,
        })
        .expect("append");
    assert_eq!(document.sweep_detached().nodes_freed, 2);
    assert_eq!(document.sweep_freed_nodes().len(), 2);
    let committed = document.memory_report().indexes;
    assert!(
        committed > baseline,
        "recording the freed nodes commits memory: {baseline} -> {committed}"
    );

    // A later sweep that frees nothing empties the accessor without
    // reallocating: an implementation that built a fresh buffer per sweep would
    // report zero capacity here and the byte count would drop.
    assert_eq!(document.sweep_detached().nodes_freed, 0);
    assert!(document.sweep_freed_nodes().is_empty());
    assert_eq!(
        document.memory_report().indexes,
        committed,
        "the freed-node buffer is reused, not reallocated"
    );
}
