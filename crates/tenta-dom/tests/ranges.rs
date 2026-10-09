//! Range lifetime and mutation behavior through the shared DOM entry point.

use tenta_core::{NodeId, atom::atoms};
use tenta_dom::{Document, LiveRangeId, Mutation, RangeBoundary};

fn range(document: &mut Document, node: NodeId, start: u32, end: u32) -> LiveRangeId {
    let node = document.node_ref(node);
    document
        .register_live_range(
            RangeBoundary {
                node,
                offset: start,
            },
            RangeBoundary { node, offset: end },
        )
        .unwrap()
}

fn offsets(document: &Document, range: LiveRangeId) -> [u32; 2] {
    document
        .live_range_points(range)
        .unwrap()
        .map(|point| point.offset)
}

#[test]
fn detached_endpoints_are_retained_and_release_is_idempotent() {
    let mut document = Document::new();
    let text = document.create_text("retained");
    let first = range(&mut document, text, 0, 5);
    let second = range(&mut document, text, 1, 4);
    assert_eq!(document.handle_count(text), 4);
    assert_eq!(document.sweep_detached().nodes_freed, 0);
    document.release_live_range(first);
    document.release_live_range(first);
    assert_eq!(document.handle_count(text), 2);
    assert_eq!(document.sweep_detached().nodes_freed, 0);
    document.release_live_range(second);
    assert_eq!(document.sweep_detached().nodes_freed, 1);
}

#[test]
fn fragment_insertion_shifts_only_offsets_after_the_insertion_point() {
    let mut document = Document::new();
    let parent = document.create_html_element(atoms::DIV);
    let first = document.create_text("first");
    let last = document.create_text("last");
    for child in [first, last] {
        document.mutate(Mutation::Append { parent, child }).unwrap();
    }
    let range = range(&mut document, parent, 1, 2);
    let fragment = document.create_fragment();
    for text in ["a", "b", "c"] {
        let child = document.create_text(text);
        document
            .mutate(Mutation::Append {
                parent: fragment,
                child,
            })
            .unwrap();
    }
    document
        .mutate(Mutation::InsertBefore {
            parent,
            child: fragment,
            before: last,
        })
        .unwrap();
    assert_eq!(offsets(&document, range), [1, 5]);
    document.mutate(Mutation::Remove { child: first }).unwrap();
    assert_eq!(offsets(&document, range), [0, 4]);
    document.release_live_range(range);
}

#[test]
fn replacement_updates_every_offset_at_and_around_the_changed_span() {
    let mut document = Document::new();
    let text = document.create_text("abcdef");
    let ranges: Vec<_> = (0..=6)
        .map(|offset| range(&mut document, text, offset, offset))
        .collect();
    document.replace_data(text, 2, 2, "XYZ").unwrap();
    let actual: Vec<_> = ranges
        .iter()
        .map(|range| offsets(&document, *range)[0])
        .collect();
    assert_eq!(actual, [0, 1, 2, 2, 2, 6, 7]);
    document
        .mutate(Mutation::SetText {
            node: text,
            text: "new".into(),
        })
        .unwrap();
    assert!(
        ranges
            .iter()
            .all(|range| offsets(&document, *range) == [0, 0])
    );
    for range in ranges {
        document.release_live_range(range);
    }
    let text = document.create_text("abc");
    let live = range(&mut document, text, 2, 3);
    document.replace_data(text, 1, u32::MAX, "x").unwrap();
    assert_eq!(document.text_data(text), Some("ax"));
    assert_eq!(offsets(&document, live), [1, 1]);
    document.release_live_range(live);
}

#[test]
fn handle_capacity_failures_leave_ranges_and_tree_unchanged() {
    let mut document = Document::new();
    let parent = document.create_html_element(atoms::DIV);
    let text = document.create_text("child");
    document
        .mutate(Mutation::Append {
            parent,
            child: text,
        })
        .unwrap();
    let live = range(&mut document, text, 1, 4);
    let pins: Vec<_> = (0..u16::MAX - 1)
        .map(|_| document.handle(parent).unwrap())
        .collect();
    let point = RangeBoundary {
        node: document.node_ref(parent),
        offset: 0,
    };
    assert!(document.register_live_range(point, point).is_err());
    assert_eq!(document.handle_count(parent), u16::MAX - 1);
    assert!(document.mutate(Mutation::Remove { child: text }).is_err());
    assert_eq!(document.parent(text), parent);
    assert_eq!(offsets(&document, live), [1, 4]);
    assert_eq!(document.handle_count(parent), u16::MAX - 1);
    for pin in pins {
        document.release(pin);
    }
    document.mutate(Mutation::Remove { child: text }).unwrap();
    assert_eq!(offsets(&document, live), [0, 0]);
    assert_eq!(document.handle_count(parent), 2);
    assert_eq!(document.handle_count(text), 0);
    document.release_live_range(live);
    assert_eq!(document.sweep_detached().nodes_freed, 2);
}

#[test]
fn compound_moves_preflight_all_range_handles_before_removing_any_child() {
    let mut document = Document::new();
    let parent = document.create_html_element(atoms::DIV);
    let existing = document.create_text("existing");
    document
        .mutate(Mutation::Append {
            parent,
            child: existing,
        })
        .unwrap();
    let fragment = document.create_fragment();
    let first = document.create_text("first");
    let last = document.create_text("last");
    for child in [first, last] {
        document
            .mutate(Mutation::Append {
                parent: fragment,
                child,
            })
            .unwrap();
    }
    let first_range = range(&mut document, first, 1, 4);
    let last_range = range(&mut document, last, 1, 3);
    let pins: Vec<_> = (0..u16::MAX - 2)
        .map(|_| document.handle(fragment).unwrap())
        .collect();
    for mutation in [
        Mutation::Append {
            parent,
            child: fragment,
        },
        Mutation::ReplaceChild {
            parent,
            new_child: fragment,
            old_child: existing,
        },
        Mutation::ReparentChildren {
            from: fragment,
            to: parent,
        },
    ] {
        assert!(document.mutate(mutation).is_err());
        assert_eq!(document.children(parent).collect::<Vec<_>>(), [existing]);
        assert_eq!(
            document.children(fragment).collect::<Vec<_>>(),
            [first, last]
        );
        assert_eq!(offsets(&document, first_range), [1, 4]);
        assert_eq!(offsets(&document, last_range), [1, 3]);
        assert_eq!(document.handle_count(fragment), u16::MAX - 2);
    }
    for pin in pins {
        document.release(pin);
    }
    document
        .mutate(Mutation::ReplaceChild {
            parent,
            new_child: fragment,
            old_child: existing,
        })
        .unwrap();
    assert_eq!(document.children(parent).collect::<Vec<_>>(), [first, last]);
    assert_eq!(offsets(&document, first_range), [0, 0]);
    assert_eq!(offsets(&document, last_range), [0, 0]);
    document.release_live_range(first_range);
    document.release_live_range(last_range);
}

#[test]
fn replacing_markup_preflights_every_removed_boundary_handle() {
    let mut document = Document::new();
    let parent = document.create_html_element(atoms::DIV);
    let first = document.create_html_element(atoms::P);
    let first_text = document.create_text("first");
    document
        .mutate(Mutation::Append {
            parent: first,
            child: first_text,
        })
        .unwrap();
    let last = document.create_text("last");
    for child in [first, last] {
        document.mutate(Mutation::Append { parent, child }).unwrap();
    }
    let first_range = range(&mut document, first_text, 1, 4);
    let last_range = range(&mut document, last, 1, 3);
    let pins: Vec<_> = (0..u16::MAX - 2)
        .map(|_| document.handle(parent).unwrap())
        .collect();
    for markup in ["", "<p>replacement</p>"] {
        assert!(document.set_inner_html(parent, markup).is_err());
        assert_eq!(document.children(parent).collect::<Vec<_>>(), [first, last]);
        assert_eq!(offsets(&document, first_range), [1, 4]);
        assert_eq!(offsets(&document, last_range), [1, 3]);
        assert_eq!(document.handle_count(parent), u16::MAX - 2);
    }
    let extra_pin = document.handle(parent).unwrap();
    assert!(
        document
            .set_outer_html(first, "<p>replacement</p>")
            .is_err()
    );
    assert_eq!(document.children(parent).collect::<Vec<_>>(), [first, last]);
    assert_eq!(offsets(&document, first_range), [1, 4]);
    assert_eq!(offsets(&document, last_range), [1, 3]);
    assert_eq!(document.handle_count(parent), u16::MAX - 1);
    document.release(extra_pin);
    for pin in pins {
        document.release(pin);
    }
    document
        .set_inner_html(parent, "<p>replacement</p>")
        .unwrap();
    assert_eq!(document.inner_html(parent), "<p>replacement</p>");
    assert_eq!(offsets(&document, first_range), [0, 0]);
    assert_eq!(offsets(&document, last_range), [0, 0]);
    assert_eq!(document.handle_count(parent), 4);
    document.release_live_range(first_range);
    document.release_live_range(last_range);
}
