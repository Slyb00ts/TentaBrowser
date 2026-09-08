//! The iterative-traversal rule (`docs/spec/02-architecture.md` §3): DOM APIs
//! can build trees far deeper than any native stack, so nothing in this crate
//! may recurse over the tree.
//!
//! Every case here runs on a thread with a 512 KB stack. A recursive walk of a
//! million-deep chain would overflow it, and a native stack overflow aborts
//! the process rather than raising an error (R-12), so a regression fails
//! loudly instead of silently passing.

mod support;

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};

const DEPTH: usize = 1_000_000;
const SMALL_STACK: usize = 512 * 1024;

fn on_small_stack<T: Send + 'static>(body: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(SMALL_STACK)
        .spawn(body)
        .expect("spawn")
        .join()
        .expect("the traversal must not overflow a 512 KB stack")
}

fn deep_chain(depth: usize) -> (Document, tenta_core::NodeId) {
    let mut document = Document::new();
    let root = document.create_html_element(atoms::DIV);
    document
        .mutate(Mutation::Append {
            parent: document.document_node(),
            child: root,
        })
        .expect("append root");
    let mut current = root;
    for _ in 1..depth {
        let child = document.create_html_element(atoms::DIV);
        document
            .mutate(Mutation::Append {
                parent: current,
                child,
            })
            .expect("append");
        current = child;
    }
    (document, root)
}

#[test]
fn a_million_deep_chain_is_built_walked_and_dropped() {
    on_small_stack(|| {
        let (document, root) = deep_chain(DEPTH);
        assert_eq!(document.len(), DEPTH + 1);

        // Pre-order walk with no stack at all.
        let walked = document.descendants_inclusive(root).count();
        assert_eq!(walked, DEPTH);

        // Ancestor walk from the deepest node.
        let deepest = document
            .descendants_inclusive(root)
            .last()
            .expect("a deepest node");
        assert_eq!(document.ancestors(deepest).count(), DEPTH);
        assert_eq!(document.root_of(deepest), document.document_node());

        // The cycle guard walks ancestors too, and refuses this insertion.
        assert!(document.is_inclusive_ancestor(root, deepest));

        drop(document);
    });
}

#[test]
fn a_million_deep_chain_is_removed_and_swept() {
    on_small_stack(|| {
        let (mut document, root) = deep_chain(DEPTH);
        document
            .mutate(Mutation::Remove { child: root })
            .expect("remove the whole chain");
        assert!(!document.is_connected(root));

        let stats = document.sweep_detached();
        assert_eq!(stats.nodes_freed, DEPTH);
        assert_eq!(document.len(), 1);
    });
}

#[test]
fn extraction_and_serialisation_truncate_instead_of_overflowing() {
    on_small_stack(|| {
        let (document, root) = deep_chain(DEPTH);
        let html = document.outer_html(root);
        assert!(
            html.contains(tenta_dom::TRUNCATION_MARKER),
            "serialisation past the depth cap must be marked"
        );
        let text = document.text();
        assert!(text.contains(tenta_dom::TRUNCATION_MARKER));
        let markdown = tenta_dom::extract::markdown(&document, root);
        assert!(markdown.contains(tenta_dom::TRUNCATION_MARKER));
        let snapshot = tenta_dom::extract::snapshot(&document, root);
        // The snapshot stops at the depth cap, so its own depth is bounded.
        let mut depth = 0usize;
        let mut node = &snapshot;
        while let Some(child) = node.children.first() {
            node = child;
            depth += 1;
        }
        assert!(
            depth <= tenta_dom::MAX_TRAVERSAL_DEPTH,
            "snapshot depth {depth}"
        );
        drop(document);
    });
}
