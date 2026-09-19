//! Reclaiming a swept node's listeners (P2 d-1b-6 part B).
//!
//! A node that no wrapper and no Rust handle keeps alive is freed by
//! `Document::sweep_detached`, but the listener entries registered on it live
//! in the engine's world, not in the document, and `CallbackRef` is not
//! self-releasing. The document records the nodes it freed — reference and
//! generation — and the engine drains them by that exact key at its next
//! `trim_memory`, releasing the V8 value each entry owned.
//!
//! Without the drain the entries stay in the table for the life of the world
//! and keep a stored callback alive with them — a silent leak on a hot path
//! (`element.innerHTML = …` detaches a subtree on every assignment).

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::NodeId;
use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation, SharedDocument};
use tenta_js::engine::{PageEngine, ScriptKind};
use tenta_js_v8::Engine;

/// `<html><body><div id="target"></div><div id="other"></div></body></html>`,
/// with the ids of the two `<div>`s so a test can detach one from Rust.
fn test_document() -> (SharedDocument, NodeId, NodeId) {
    let mut document = Document::new();
    let html = document.create_html_element(atoms::HTML);
    let body = document.create_html_element(atoms::BODY);
    let target = document.create_html_element(atoms::DIV);
    let other = document.create_html_element(atoms::DIV);

    for (parent, child) in [
        (document.document_node(), html),
        (html, body),
        (body, target),
        (body, other),
    ] {
        document
            .mutate(Mutation::Append { parent, child })
            .expect("append");
    }
    for (element, id) in [(target, "target"), (other, "other")] {
        document
            .mutate(Mutation::SetAttribute {
                element,
                name: tenta_html::QualName::new(atoms::EMPTY, atoms::ID),
                value: id.to_owned(),
            })
            .expect("set id attribute");
    }
    (Rc::new(RefCell::new(document)), target, other)
}

fn engine_over(document: &SharedDocument) -> Engine {
    Engine::with_document(document.clone())
}

/// The whole point of the work package: a listener registered on a node a
/// sweep freed is gone from the table, and the stored callback it owned has
/// been released rather than pinned for the life of the world.
#[test]
fn a_swept_nodes_listener_is_drained_and_its_stored_callback_released() {
    let (document, _target, other) = test_document();
    let mut engine = engine_over(&document);

    engine
        .run_script(
            "document.querySelector('#other').addEventListener('x', () => {});",
            ScriptKind::Classic,
        )
        .expect("the registering script runs");
    assert_eq!(engine.listener_count(), 1, "the listener is registered");
    assert_eq!(
        engine.stored_value_count(),
        1,
        "the listener owns one stored callback"
    );

    // Detach the node: it is now a detached root with no wrapper script still
    // holds. The forced collection clears the wrapper bookkeeping, so the sweep
    // can free it.
    document
        .borrow_mut()
        .mutate(Mutation::Remove { child: other })
        .expect("detach #other");
    engine.trim_memory();
    assert!(
        !document.borrow().has_wrapper(other),
        "the collected wrapper's node must be sweepable"
    );

    // The node's reference, taken while it is still live: the sweep records
    // the generation it lived under, and freeing the slot bumps past it.
    let other_ref = document.borrow().node_ref(other);

    let stats = document.borrow_mut().sweep_detached();
    assert_eq!(stats.nodes_freed, 1, "the detached node is freed");
    assert_eq!(
        document.borrow().sweep_freed_nodes(),
        &[other_ref],
        "the sweep records the reference the engine has to drain"
    );
    assert_eq!(
        engine.listener_count(),
        1,
        "the entry is still there until the drain runs; that is the leak"
    );

    // The next trim drains the recorded reference through the installed
    // `ScriptHost`.
    engine.trim_memory();
    assert_eq!(
        engine.listener_count(),
        0,
        "the freed node's listener entry is drained"
    );
    assert_eq!(
        engine.stored_value_count(),
        0,
        "and the stored callback it owned is released, not pinned"
    );
}

/// The drain is addressed by the ids the sweep recorded, so a live node's
/// listeners are untouched by it and still fire.
#[test]
fn a_live_nodes_listener_survives_a_drain() {
    let (document, target, other) = test_document();
    let mut engine = engine_over(&document);

    engine
        .run_script(
            "globalThis.keep = document.querySelector('#target'); \
             keep.addEventListener('x', () => { globalThis.fired = true; }); \
             globalThis.fired = false; \
             document.querySelector('#other').addEventListener('x', () => {});",
            ScriptKind::Classic,
        )
        .expect("the registering script runs");
    assert_eq!(engine.listener_count(), 2);

    document
        .borrow_mut()
        .mutate(Mutation::Remove { child: other })
        .expect("detach #other");
    engine.trim_memory();
    let stats = document.borrow_mut().sweep_detached();
    assert_eq!(stats.nodes_freed, 1, "only the unreferenced node is freed");
    assert!(
        document.borrow().has_wrapper(target),
        "the node script still holds keeps its wrapper, so it is not sweepable"
    );

    engine.trim_memory();
    assert_eq!(
        engine.listener_count(),
        1,
        "only the freed node's entry was drained"
    );

    engine
        .run_script(
            "keep.dispatchEvent(new Event('x')); \
             if (!globalThis.fired) throw new Error('the live listener did not fire');",
            ScriptKind::Classic,
        )
        .expect("the surviving listener still fires on its own node");
    assert_eq!(
        engine.listener_count(),
        1,
        "dispatching does not disturb the entry either"
    );
}

/// The defect the exact-key drain exists to close. The drain runs at one
/// `trim_memory` and consumes the record of the *previous* sweep; the page runs
/// script in between, and a script allocation can take the freed slot. A drain
/// that matched on the id alone would then remove the live node's listener —
/// generation-blind — and it would silently stop firing.
#[test]
fn a_listener_on_a_node_that_reuses_a_freed_slot_survives_the_drain() {
    let (document, _target, other) = test_document();
    let mut engine = engine_over(&document);

    engine
        .run_script(
            "document.querySelector('#other').addEventListener('x', () => { \
                 globalThis.freedFired = true; \
             });",
            ScriptKind::Classic,
        )
        .expect("the registering script runs");
    assert_eq!(engine.listener_count(), 1);
    assert_eq!(engine.stored_value_count(), 1);

    // Detach the node and clear the wrapper bookkeeping, then sweep: #other's
    // slot is freed and its reference recorded for the next drain.
    document
        .borrow_mut()
        .mutate(Mutation::Remove { child: other })
        .expect("detach #other");
    engine.trim_memory();
    let other_ref = document.borrow().node_ref(other);
    assert_eq!(
        document.borrow_mut().sweep_detached().nodes_freed,
        1,
        "the detached node is freed"
    );
    assert_eq!(
        document.borrow().sweep_freed_nodes(),
        &[other_ref],
        "the sweep records the freed node's reference, generation included"
    );

    // Script runs before the next trim and allocates a node into the freed
    // slot, registering a listener on it.
    engine
        .run_script(
            "globalThis.reused = document.createElement('span'); \
             reused.addEventListener('x', () => { globalThis.reusedFired = true; });",
            ScriptKind::Classic,
        )
        .expect("the reusing script runs");

    // The slot really was reused, under a bumped generation. Assert that before
    // the drain, so the test cannot pass by the allocation landing elsewhere.
    {
        let doc = document.borrow();
        assert!(doc.is_live(other), "the arena hands the freed slot back");
        assert_ne!(
            doc.node_ref(other).generation,
            other_ref.generation,
            "the reused slot carries a bumped generation"
        );
    }
    assert_eq!(
        engine.listener_count(),
        2,
        "the freed node's entry and the reused node's are both registered"
    );

    // The drain consumes the freed node's reference. It must take only that
    // entry: a drain on the id alone would also remove the live listener.
    engine.trim_memory();
    assert_eq!(
        engine.listener_count(),
        1,
        "only the freed node's entry was drained, not the reused slot's"
    );
    assert_eq!(
        engine.stored_value_count(),
        1,
        "the freed node's stored callback was released; the live one was not"
    );

    engine
        .run_script(
            "reused.dispatchEvent(new Event('x')); \
             if (!globalThis.reusedFired) throw new Error('the reused listener did not fire');",
            ScriptKind::Classic,
        )
        .expect("the surviving listener still fires on the recycled node");
    assert_eq!(
        engine.listener_count(),
        1,
        "dispatching does not disturb the entry either"
    );
}
