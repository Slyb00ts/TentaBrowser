//! Splitting a `Text` node and the tree surgery of a range
//! (`Document::split_text`, `delete_range_contents`, `extract_range_contents`,
//! `clone_range_contents`, `insert_node_at_range`).
//!
//! The expected trees are worked out by hand from the DOM Standard's
//! algorithms (<https://dom.spec.whatwg.org/#interface-range>), one case per
//! shape a range can have below its common ancestor: inside one text node, a
//! spine on both sides, a spine on one side only, and none.

use tenta_core::NodeId;
use tenta_core::atom::atoms;
use tenta_dom::{Document, LiveRangeId, Mutation, RangeBoundary, RangePoint};
use tenta_html::{ParseStatus, Parser, ParserOptions};

fn parse(html: &str) -> Document {
    let document = Document::with_url("https://example.com/".parse().expect("the test URL"));
    let mut parser = Parser::new(document, ParserOptions::default());
    assert_eq!(parser.feed(html.as_bytes()), ParseStatus::Continue);
    parser.finish()
}

/// `<div id=root>` holding two paragraphs with text on both sides of an
/// inline element each.
const TWO_PARAGRAPHS: &str = "<div id=root><p id=a>one<b id=b>two</b>three</p>\
                              <p id=c>four<i id=i>five</i>six</p></div>";

fn by_id(document: &Document, id: &str) -> NodeId {
    document.get_element_by_id(id).expect("the element")
}

/// The child of `#id` at `index`.
fn child(document: &Document, id: &str, index: usize) -> NodeId {
    document
        .children(by_id(document, id))
        .nth(index)
        .expect("the child")
}

fn root_html(document: &Document) -> String {
    document.inner_html(by_id(document, "root"))
}

fn live(document: &mut Document, start: RangePoint, end: RangePoint) -> LiveRangeId {
    let start = RangeBoundary {
        node: document.node_ref(start.0),
        offset: start.1,
    };
    let end = RangeBoundary {
        node: document.node_ref(end.0),
        offset: end.1,
    };
    document.register_live_range(start, end).expect("a range")
}

fn points(document: &Document, range: LiveRangeId) -> [RangePoint; 2] {
    document
        .live_range_points(range)
        .expect("registered")
        .map(|point| (point.node.id, point.offset))
}

fn error_name(error: &tenta_core::Error) -> &str {
    tenta_dom::error::dom_exception_name(error).unwrap_or("?")
}

#[test]
fn a_range_inside_one_text_node_takes_a_slice_of_its_data() {
    let mut document = parse(TWO_PARAGRAPHS);
    let one = child(&document, "a", 0);
    let copy = document.clone_range_contents((one, 1), (one, 2)).unwrap();
    assert_eq!(document.inner_html(copy), "n");
    assert_eq!(document.text_data(one), Some("one"));

    let (fragment, collapsed) = document.extract_range_contents((one, 1), (one, 2)).unwrap();
    assert_eq!(document.inner_html(fragment), "n");
    assert_eq!(document.text_data(one), Some("oe"));
    assert_eq!(collapsed, (one, 1));

    assert_eq!(
        document.delete_range_contents((one, 0), (one, 1)).unwrap(),
        (one, 0)
    );
    assert_eq!(document.text_data(one), Some("e"));

    // A collapsed range has no contents.
    let (empty, at) = document.extract_range_contents((one, 1), (one, 1)).unwrap();
    assert_eq!(document.inner_html(empty), "");
    assert_eq!(at, (one, 1));
    assert_eq!(document.text_data(one), Some("e"));
}

/// The range starts in the text `one` and ends in the text `five`: both nodes
/// hang below a child of the common ancestor, so both sides are cloned down
/// to the text and nothing between them is contained.
const EXTRACTED: &str = "<p id=\"a\">ne<b id=\"b\">two</b>three</p>\
                         <p id=\"c\">four<i id=\"i\">fi</i></p>";
const LEFT_BEHIND: &str = "<p id=\"a\">o</p><p id=\"c\"><i id=\"i\">ve</i>six</p>";

#[test]
fn extraction_clones_both_sides_and_moves_what_lies_between() {
    let mut document = parse(TWO_PARAGRAPHS);
    let root = by_id(&document, "root");
    let start = (child(&document, "a", 0), 1);
    let end = (child(&document, "i", 0), 2);
    let moved_bold = by_id(&document, "b");

    let (fragment, collapsed) = document.extract_range_contents(start, end).unwrap();
    assert_eq!(document.inner_html(fragment), EXTRACTED);
    assert_eq!(root_html(&document), LEFT_BEHIND);
    // After the child that held the start node.
    assert_eq!(collapsed, (root, 1));
    // What was contained whole moved, it was not copied: the `<b>` in the
    // fragment is the node that was in the tree.
    assert!(document.is_inclusive_ancestor(fragment, moved_bold));
}

#[test]
fn cloning_gives_the_same_fragment_and_leaves_the_tree() {
    let mut document = parse(TWO_PARAGRAPHS);
    let before = root_html(&document);
    let start = (child(&document, "a", 0), 1);
    let end = (child(&document, "i", 0), 2);
    let bold = by_id(&document, "b");

    let fragment = document.clone_range_contents(start, end).unwrap();
    assert_eq!(document.inner_html(fragment), EXTRACTED);
    assert_eq!(root_html(&document), before);
    assert!(!document.is_inclusive_ancestor(fragment, bold));
}

#[test]
fn deletion_leaves_what_extraction_leaves() {
    let mut document = parse(TWO_PARAGRAPHS);
    let root = by_id(&document, "root");
    let start = (child(&document, "a", 0), 1);
    let end = (child(&document, "i", 0), 2);

    assert_eq!(
        document.delete_range_contents(start, end).unwrap(),
        (root, 1)
    );
    assert_eq!(root_html(&document), LEFT_BEHIND);
}

#[test]
fn boundaries_in_the_common_ancestor_contain_whole_children() {
    let mut document = parse(TWO_PARAGRAPHS);
    let root = by_id(&document, "root");
    let (fragment, collapsed) = document
        .extract_range_contents((root, 0), (root, 1))
        .unwrap();
    assert_eq!(
        document.inner_html(fragment),
        "<p id=\"a\">one<b id=\"b\">two</b>three</p>"
    );
    assert_eq!(
        root_html(&document),
        "<p id=\"c\">four<i id=\"i\">five</i>six</p>"
    );
    assert_eq!(collapsed, (root, 0));
}

#[test]
fn boundaries_in_elements_take_the_children_on_their_inner_sides() {
    let mut document = parse(TWO_PARAGRAPHS);
    let root = by_id(&document, "root");
    // From before `<b>` in the first paragraph to after `four` in the second.
    let start = (by_id(&document, "a"), 1);
    let end = (by_id(&document, "c"), 1);
    let copy = document.clone_range_contents(start, end).unwrap();
    let expected = "<p id=\"a\"><b id=\"b\">two</b>three</p><p id=\"c\">four</p>";
    assert_eq!(document.inner_html(copy), expected);

    let (fragment, collapsed) = document.extract_range_contents(start, end).unwrap();
    assert_eq!(document.inner_html(fragment), expected);
    assert_eq!(
        root_html(&document),
        "<p id=\"a\">one</p><p id=\"c\"><i id=\"i\">five</i>six</p>"
    );
    assert_eq!(collapsed, (root, 1));
}

#[test]
fn a_start_in_the_common_ancestor_has_a_spine_on_the_end_side_only() {
    let mut document = parse(TWO_PARAGRAPHS);
    let root = by_id(&document, "root");
    let end = (child(&document, "i", 0), 2);
    let copy = document.clone_range_contents((root, 0), end).unwrap();
    let expected = "<p id=\"a\">one<b id=\"b\">two</b>three</p>\
                    <p id=\"c\">four<i id=\"i\">fi</i></p>";
    assert_eq!(document.inner_html(copy), expected);

    let (fragment, collapsed) = document.extract_range_contents((root, 0), end).unwrap();
    assert_eq!(document.inner_html(fragment), expected);
    assert_eq!(
        root_html(&document),
        "<p id=\"c\"><i id=\"i\">ve</i>six</p>"
    );
    // The start node holds the end node, so the range collapses to its start.
    assert_eq!(collapsed, (root, 0));
}

#[test]
fn an_end_in_the_common_ancestor_has_a_spine_on_the_start_side_only() {
    let mut document = parse(TWO_PARAGRAPHS);
    let root = by_id(&document, "root");
    let start = (child(&document, "b", 0), 1);
    let copy = document.clone_range_contents(start, (root, 2)).unwrap();
    let expected = "<p id=\"a\"><b id=\"b\">wo</b>three</p>\
                    <p id=\"c\">four<i id=\"i\">five</i>six</p>";
    assert_eq!(document.inner_html(copy), expected);

    let mut deleted = parse(TWO_PARAGRAPHS);
    let deleted_root = by_id(&deleted, "root");
    let deleted_start = (child(&deleted, "b", 0), 1);
    assert_eq!(
        deleted
            .delete_range_contents(deleted_start, (deleted_root, 2))
            .unwrap(),
        (deleted_root, 1)
    );
    let left = "<p id=\"a\">one<b id=\"b\">t</b></p>";
    assert_eq!(root_html(&deleted), left);

    let (fragment, collapsed) = document.extract_range_contents(start, (root, 2)).unwrap();
    assert_eq!(document.inner_html(fragment), expected);
    assert_eq!(root_html(&document), left);
    assert_eq!(collapsed, (root, 1));
}

#[test]
fn comments_are_sliced_as_text_is() {
    let mut document = parse("<div id=root><!--comment-->text</div>");
    let comment = child(&document, "root", 0);
    let text = child(&document, "root", 1);
    let (fragment, collapsed) = document
        .extract_range_contents((comment, 2), (text, 2))
        .unwrap();
    assert_eq!(document.inner_html(fragment), "<!--mment-->te");
    assert_eq!(root_html(&document), "<!--co-->xt");
    assert_eq!(collapsed, (by_id(&document, "root"), 1));
}

#[test]
fn a_range_that_contains_the_doctype_is_refused_before_anything_moves() {
    let mut document = parse("<!doctype html><html><body><p id=root>x</p></body></html>");
    let node = document.document_node();
    let before = document.inner_html(node);
    for result in [
        document
            .extract_range_contents((node, 0), (node, 2))
            .map(|_| ()),
        document
            .clone_range_contents((node, 0), (node, 2))
            .map(|_| ()),
    ] {
        assert_eq!(
            result.as_ref().map_err(error_name),
            Err("HierarchyRequestError")
        );
    }
    assert_eq!(document.inner_html(node), before);
    // Deleting has no such rule: the doctype and the document element go.
    document
        .delete_range_contents((node, 0), (node, 2))
        .unwrap();
    assert_eq!(document.inner_html(node), "");
}

#[test]
fn other_ranges_follow_the_nodes_and_data_that_move() {
    let mut document = parse(TWO_PARAGRAPHS);
    let root = by_id(&document, "root");
    let one = child(&document, "a", 0);
    let three = child(&document, "a", 2);
    let six = child(&document, "c", 2);
    // In data that is cut away; in a node that moves into the fragment; and
    // in a node that stays.
    let cut = live(&mut document, (one, 2), (one, 3));
    let moved = live(&mut document, (three, 1), (three, 4));
    let kept = live(&mut document, (six, 1), (six, 2));

    document
        .extract_range_contents((one, 1), (child(&document, "i", 0), 2))
        .unwrap();
    // The data from offset 1 on is gone, and the boundaries come to rest there.
    assert_eq!(points(&document, cut), [(one, 1), (one, 1)]);
    // A boundary inside a removed node moves to where the node was.
    assert_eq!(points(&document, moved), [(by_id(&document, "a"), 1); 2]);
    assert_eq!(points(&document, kept), [(six, 1), (six, 2)]);
    assert_eq!(root_html(&document), LEFT_BEHIND);
    let _ = root;
}

#[test]
fn splitting_a_text_node_moves_data_and_the_boundaries_in_it() {
    let mut document = parse("<div id=root><p id=p>hello<b></b></p></div>");
    let paragraph = by_id(&document, "p");
    let text = child(&document, "p", 0);
    let across = live(&mut document, (text, 1), (text, 4));
    let at_split = live(&mut document, (text, 2), (text, 2));
    let after_text = live(&mut document, (paragraph, 1), (paragraph, 1));
    let after_bold = live(&mut document, (paragraph, 2), (paragraph, 2));

    let tail = document.split_text(text, 2).unwrap();
    assert_eq!(document.text_data(text), Some("he"));
    assert_eq!(document.text_data(tail), Some("llo"));
    assert_eq!(document.next_sibling(text), tail);
    assert_eq!(root_html(&document), "<p id=\"p\">hello<b></b></p>");
    assert_eq!(document.children(paragraph).count(), 3);

    // A boundary past the split follows its data; one at the split stays.
    assert_eq!(points(&document, across), [(text, 1), (tail, 2)]);
    assert_eq!(points(&document, at_split), [(text, 2), (text, 2)]);
    // A boundary just after the text stays after everything the text became.
    assert_eq!(points(&document, after_text), [(paragraph, 2); 2]);
    assert_eq!(points(&document, after_bold), [(paragraph, 3); 2]);
}

#[test]
fn splitting_at_the_ends_and_without_a_parent() {
    let mut document = parse("<div id=root><p id=p>hello</p></div>");
    let text = child(&document, "p", 0);
    assert_eq!(
        document.split_text(text, 6).as_ref().map_err(error_name),
        Err("IndexSizeError")
    );
    let empty = document.split_text(text, 5).unwrap();
    assert_eq!(document.text_data(empty), Some(""));
    assert_eq!(document.text_data(text), Some("hello"));
    assert_eq!(document.next_sibling(text), empty);

    let detached = document.create_text("abcd");
    let tail = document.split_text(detached, 1).unwrap();
    assert_eq!(document.text_data(detached), Some("a"));
    assert_eq!(document.text_data(tail), Some("bcd"));
    assert!(!document.parent(tail).is_some());

    let element = by_id(&document, "p");
    assert_eq!(
        document.split_text(element, 0).as_ref().map_err(error_name),
        Err("HierarchyRequestError")
    );
    // Offsets are UTF-16 code units: the astral character counts two.
    let astral = document.create_text("a\u{1F600}b");
    let rest = document.split_text(astral, 3).unwrap();
    assert_eq!(document.text_data(astral), Some("a\u{1F600}"));
    assert_eq!(document.text_data(rest), Some("b"));
}

#[test]
fn a_node_is_inserted_at_the_start_of_a_range() {
    let mut document = parse("<div id=root><p id=p>hello<b></b></p></div>");
    let paragraph = by_id(&document, "p");
    let text = child(&document, "p", 0);

    // Into a text node: the text is split and the node goes between.
    let inserted = document.create_html_element(atoms::I);
    assert_eq!(
        document.insert_node_at_range((text, 2), inserted).unwrap(),
        (paragraph, 2)
    );
    assert_eq!(root_html(&document), "<p id=\"p\">he<i></i>llo<b></b></p>");

    // At a place between children.
    let span = document.create_html_element(atoms::SPAN);
    assert_eq!(
        document.insert_node_at_range((paragraph, 0), span).unwrap(),
        (paragraph, 1)
    );
    assert_eq!(
        root_html(&document),
        "<p id=\"p\"><span></span>he<i></i>llo<b></b></p>"
    );

    // A fragment counts as many nodes as it holds.
    let fragment = document.create_fragment();
    for data in ["x", "y"] {
        let child = document.create_text(data);
        document
            .mutate(Mutation::Append {
                parent: fragment,
                child,
            })
            .unwrap();
    }
    assert_eq!(
        document
            .insert_node_at_range((paragraph, 5), fragment)
            .unwrap(),
        (paragraph, 7)
    );
    assert_eq!(
        root_html(&document),
        "<p id=\"p\"><span></span>he<i></i>llo<b></b>xy</p>"
    );

    // The node that is already at the range's start stays where it is.
    assert_eq!(
        document.insert_node_at_range((paragraph, 0), span).unwrap(),
        (paragraph, 1)
    );
    assert_eq!(document.first_child(paragraph), span);
    // A node from elsewhere in the tree moves.
    assert_eq!(
        document.insert_node_at_range((paragraph, 7), span).unwrap(),
        (paragraph, 7)
    );
    assert_eq!(document.last_child(paragraph), span);
}

#[test]
fn an_insertion_that_cannot_happen_leaves_the_text_whole() {
    let mut document = parse("<div id=root><p id=p>hello<!--c--></p></div>");
    let root = by_id(&document, "root");
    let paragraph = by_id(&document, "p");
    let text = child(&document, "p", 0);
    let comment = child(&document, "p", 1);
    let other = document.create_html_element(atoms::I);

    for (start, node) in [
        // A comment cannot be split around a node.
        ((comment, 1), other),
        // The node is the start node itself.
        ((text, 1), text),
        // An ancestor of the place it is to go.
        ((text, 2), root),
    ] {
        assert_eq!(
            document
                .insert_node_at_range(start, node)
                .as_ref()
                .map_err(error_name),
            Err("HierarchyRequestError")
        );
    }
    assert_eq!(document.children(paragraph).count(), 2);
    assert_eq!(document.text_data(text), Some("hello"));

    // A text node without a parent has nowhere to put a sibling.
    let detached = document.create_text("abcd");
    assert_eq!(
        document
            .insert_node_at_range((detached, 1), other)
            .as_ref()
            .map_err(error_name),
        Err("HierarchyRequestError")
    );
    assert_eq!(document.text_data(detached), Some("abcd"));
}

#[test]
fn only_text_may_be_cut_through_by_a_range_that_is_surrounded() {
    let document = parse(TWO_PARAGRAPHS);
    let one = child(&document, "a", 0);
    let three = child(&document, "a", 2);
    let five = child(&document, "i", 0);
    // Both ends in text nodes that are children of the common ancestor.
    assert!(!document.range_splits_a_non_text_node((one, 1), (three, 2)));
    assert!(!document.range_splits_a_non_text_node((one, 1), (one, 2)));
    // The end is inside `<i>`, which the range enters without leaving.
    assert!(document.range_splits_a_non_text_node((one, 1), (five, 2)));
    let root = by_id(&document, "root");
    assert!(!document.range_splits_a_non_text_node((root, 0), (root, 2)));
    assert!(document.range_splits_a_non_text_node((root, 0), (by_id(&document, "c"), 1)));
}

#[test]
fn a_contextual_fragment_is_parsed_as_the_children_of_the_start_element() {
    let mut document = parse(
        "<!doctype html><html><body><div id=root><table id=t><tbody id=b></tbody></table>\
         <p id=p>text</p></div></body></html>",
    );
    let rows = "<tr><td>cell</td></tr>";
    // In a table section the rows are rows; in a paragraph the table markup
    // is dropped and the text is left.
    let in_body = document
        .create_contextual_fragment(by_id(&document, "b"), rows)
        .unwrap();
    assert_eq!(document.inner_html(in_body), "<tr><td>cell</td></tr>");
    let in_paragraph = document
        .create_contextual_fragment(by_id(&document, "p"), rows)
        .unwrap();
    assert_eq!(document.inner_html(in_paragraph), "cell");
    // A start in text is a start in the text's parent.
    let text = child(&document, "p", 0);
    let in_text = document.create_contextual_fragment(text, rows).unwrap();
    assert_eq!(document.inner_html(in_text), "cell");
    // The document, a detached text node and the `<html>` element have no
    // element to parse in: the context is a `<body>`.
    let html_element = document.parent(document.parent(by_id(&document, "root")));
    let detached = document.create_text("free");
    for start in [document.document_node(), detached, html_element] {
        let fragment = document
            .create_contextual_fragment(start, "<title>t</title><p>x</p>")
            .unwrap();
        assert_eq!(document.inner_html(fragment), "<title>t</title><p>x</p>");
    }
    // Nothing was inserted anywhere.
    assert_eq!(
        root_html(&document),
        "<table id=\"t\"><tbody id=\"b\"></tbody></table><p id=\"p\">text</p>"
    );
}

#[test]
fn a_script_in_a_contextual_fragment_has_not_started() {
    let mut document = parse("<div id=root><p id=p></p></div>");
    let paragraph = by_id(&document, "p");
    let markup = "<b><script>one()</script></b><script src=two.js></script>";

    // Markup set through innerHTML is parsed with its scripts spent.
    document.set_inner_html(paragraph, markup).unwrap();
    let spent: Vec<bool> = document
        .descendants(paragraph)
        .filter(|node| document.tag(*node) == Some(atoms::SCRIPT))
        .map(|node| document.script_started(node))
        .collect();
    assert_eq!(spent, [true, true]);

    let fragment = document
        .create_contextual_fragment(paragraph, markup)
        .unwrap();
    let fresh: Vec<bool> = document
        .descendants(fragment)
        .filter(|node| document.tag(*node) == Some(atoms::SCRIPT))
        .map(|node| document.script_started(node))
        .collect();
    assert_eq!(fresh, [false, false]);
}

/// A range whose ends are a hundred thousand levels apart is walked as a
/// path, on a stack too small to recurse that far.
#[test]
fn a_range_a_hundred_thousand_levels_deep_is_walked_without_the_stack() {
    const DEPTH: usize = 100_000;
    let body = || {
        let mut document = Document::new();
        let top = document.create_html_element(atoms::DIV);
        document
            .mutate(Mutation::Append {
                parent: document.document_node(),
                child: top,
            })
            .unwrap();
        let mut lowest = top;
        for _ in 0..DEPTH {
            let child = document.create_html_element(atoms::DIV);
            document
                .mutate(Mutation::Append {
                    parent: lowest,
                    child,
                })
                .unwrap();
            lowest = child;
        }
        let text = document.create_text("deep");
        document
            .mutate(Mutation::Append {
                parent: lowest,
                child: text,
            })
            .unwrap();
        let before = document.create_text("before");
        document
            .mutate(Mutation::InsertBefore {
                parent: top,
                child: before,
                before: document.first_child(top),
            })
            .unwrap();
        let after = document.create_text("after");
        document
            .mutate(Mutation::Append {
                parent: top,
                child: after,
            })
            .unwrap();

        // How deep the first-child chain below `node` goes, and its last text.
        let chain = |document: &Document, mut node: NodeId| {
            let mut depth = 0;
            while document.first_child(node).is_some() {
                node = document.first_child(node);
                depth += 1;
            }
            (depth, document.text_data(node).map(str::to_owned))
        };

        // Deep at its end and shallow at its start, then the other way round.
        let mirrored = document
            .clone_range_contents((before, 2), (text, 2))
            .unwrap();
        assert_eq!(document.children(mirrored).count(), 2);
        assert_eq!(
            chain(&document, document.last_child(mirrored)),
            (DEPTH, Some("de".to_owned()))
        );
        let copy = document
            .clone_range_contents((text, 2), (after, 2))
            .unwrap();
        assert_eq!(document.children(copy).count(), 2);
        assert_eq!(
            chain(&document, document.first_child(copy)),
            (DEPTH, Some("ep".to_owned()))
        );

        let (fragment, collapsed) = document
            .extract_range_contents((text, 2), (after, 2))
            .unwrap();
        assert_eq!(collapsed, (top, 2));
        assert_eq!(document.text_data(text), Some("de"));
        assert_eq!(document.text_data(after), Some("ter"));
        assert_eq!(
            chain(&document, document.first_child(fragment)),
            (DEPTH, Some("ep".to_owned()))
        );
        assert_eq!(
            document
                .delete_range_contents((before, 1), (text, 1))
                .unwrap(),
            (top, 1)
        );
        assert_eq!(document.text_data(before), Some("b"));
        assert_eq!(document.text_data(text), Some("e"));
    };
    std::thread::Builder::new()
        .stack_size(512 * 1024)
        .spawn(body)
        .expect("spawn")
        .join()
        .expect("a deep range must not overflow a 512 KB stack");
}
