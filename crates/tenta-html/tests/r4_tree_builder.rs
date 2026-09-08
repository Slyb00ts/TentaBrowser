//! Round-4 tree builder coverage: bounded stack walks on unmatched, formatting
//! and foreign end tags (M-3), the open flag on active-formatting entries
//! (M-4), the document node budget (M-5), table scoping when the depth cap
//! retargets an insertion (m-1) and template contents in the depth helper
//! (m-4).

mod common;

use std::time::{Duration, Instant};

use common::tree::{NodeData, Tree};
use tenta_html::{MAX_DOM_DEPTH, Parser, ParserOptions};

fn parse(html: &str) -> Tree {
    parse_with(html, ParserOptions::default())
}

fn parse_with(html: &str, options: ParserOptions) -> Tree {
    let mut parser = Parser::new(Tree::new(), options);
    parser.feed_str(html);
    parser.finish()
}

fn element_named(tree: &Tree, node: usize) -> Option<&str> {
    match &tree.nodes[node].data {
        NodeData::Element { name, .. } => Some(name.local.as_str()),
        _ => None,
    }
}

fn find_by_id(tree: &Tree, id: &str) -> Option<usize> {
    tree.nodes.iter().position(|node| match &node.data {
        NodeData::Element { attrs, .. } => attrs
            .iter()
            .any(|attr| attr.name.local.as_str() == "id" && attr.value == id),
        _ => false,
    })
}

fn repeat(piece: &str, times: usize) -> String {
    let mut out = String::with_capacity(piece.len() * times + 32);
    for _ in 0..times {
        out.push_str(piece);
    }
    out
}

// ---------------------------------------------------------------- m-1 / m-4

#[test]
fn retargeting_past_the_cap_never_lands_on_a_table_section() {
    let mut html = String::from("<!doctype html><body>");
    html.push_str(&repeat("<table><div><template><div>", 140));
    html.push_str("<p id=last>x</p>");
    let tree = parse(&html);
    let last = find_by_id(&tree, "last").expect("#last is in the tree");
    let parent = tree.nodes[last].parent.expect("#last has a parent");
    let parent_name = element_named(&tree, parent).expect("parent is an element");
    assert!(
        !matches!(parent_name, "table" | "tbody" | "thead" | "tfoot" | "tr"),
        "#last was retargeted under <{parent_name}>"
    );
    assert!(tree.depth_of_deepest_element() <= MAX_DOM_DEPTH);
}

#[test]
fn the_depth_helper_counts_template_contents() {
    // 500 open <div>s under <html><body>, a <template> at depth 503 and 50
    // <div>s inside it: without the template contents the helper would
    // report 503 and miss any overflow built inside the fragment.
    let mut html = String::from("<!doctype html><body>");
    html.push_str(&repeat("<div>", 500));
    html.push_str("<template>");
    html.push_str(&repeat("<div>", 50));
    let tree = parse(&html);
    assert_eq!(tree.depth_of_deepest_element(), MAX_DOM_DEPTH);
}

// ---------------------------------------------------------------------- M-5

fn formatting_amplifier(paragraphs: usize) -> String {
    // 512 distinct <b id> entries defeat Noah's Ark; each later <p> then
    // reconstructs all 512 of them.
    let mut html = String::from("<!doctype html><body><p>");
    for index in 0..512 {
        html.push_str(&format!("<b id=b{index}>"));
    }
    html.push_str("</p>");
    html.push_str(&repeat("<p>y</p>", paragraphs));
    html
}

#[test]
fn the_node_budget_bounds_formatting_amplification() {
    let budget = 20_000;
    let options = ParserOptions {
        max_dom_nodes: budget,
        ..ParserOptions::default()
    };
    let tree = parse_with(&formatting_amplifier(4_000), options);
    let (elements, nodes) = tree.reachable_counts();
    assert!(
        elements <= budget + 8,
        "{elements} elements in the tree against a budget of {budget}"
    );
    assert!(
        nodes <= budget + 4_100,
        "{nodes} nodes in the tree against a budget of {budget}"
    );
    // Elements created past the budget stay out of the tree but on the stack,
    // so the parse keeps its invariants: one detached element per dropped
    // start tag, never more.
    assert!(
        tree.element_count() <= elements + 4_000 + 8,
        "{} elements created against {elements} in the tree",
        tree.element_count()
    );
    assert!(
        tree.errors.iter().any(|error| error.contains("budget")),
        "dropping elements past the budget is a parse error"
    );
    let unbounded = parse(&formatting_amplifier(40));
    assert!(
        unbounded.element_count() > 512 * 40,
        "below the budget every reconstruction still happens"
    );
}

#[test]
fn the_default_budget_leaves_ordinary_large_documents_alone() {
    let mut html = String::from("<!doctype html><body>");
    html.push_str(&repeat("<a><div>", 100_000));
    let tree = parse(&html);
    assert!(tree.element_count() >= 200_000);
    assert!(!tree.errors.iter().any(|error| error.contains("budget")));
}

#[test]
fn a_pre_drops_exactly_one_leading_newline() {
    let tree = parse("<!doctype html><body><pre>\n\nx</pre>");
    let text = tree
        .nodes
        .iter()
        .find_map(|node| match &node.data {
            NodeData::Text(text) if text.contains('x') => Some(text.clone()),
            _ => None,
        })
        .expect("text");
    assert_eq!(text, "\nx");
}

// ------------------------------------------------------------- M-3 / M-4

/// Best-of-three timing of one parse.
fn best_of_three(html: &str) -> Duration {
    (0..3)
        .map(|_| {
            let start = Instant::now();
            std::hint::black_box(parse(html));
            start.elapsed()
        })
        .min()
        .expect("three rounds")
}

/// Asserts a 4x input costs at most `slack` × 4 the small one (linear = 1x4,
/// quadratic = 16x4), and reports ms/MB for the log.
fn assert_linear(name: &str, small: &str, large: &str, slack: f64) {
    let small_time = best_of_three(small);
    let large_time = best_of_three(large);
    let ms_per_mb = large_time.as_secs_f64() * 1e3 / (large.len() as f64 / 1e6);
    println!(
        "{name}: {small_time:?} for {} B, {large_time:?} for {} B ({ms_per_mb:.0} ms/MB)",
        small.len(),
        large.len()
    );
    let allowed = small_time.mul_f64(4.0 * slack) + Duration::from_millis(5);
    assert!(
        large_time <= allowed,
        "{name}: 4x input took {large_time:?} against {small_time:?} (allowed {allowed:?})"
    );
}

fn unmatched_end_tags(n: usize, end_tag: &str) -> String {
    let mut html = String::from("<!doctype html><body>");
    html.push_str(&repeat("<span>", n));
    html.push_str(&repeat(end_tag, n));
    html
}

#[test]
#[ignore = "budget measurement"]
fn unmatched_end_tags_on_a_deep_stack_parse_linearly() {
    assert_linear(
        "</zzz> flood",
        &unmatched_end_tags(25_000, "</zzz>"),
        &unmatched_end_tags(100_000, "</zzz>"),
        2.5,
    );
}

#[test]
#[ignore = "budget measurement"]
fn formatting_end_tags_on_a_deep_stack_parse_linearly() {
    assert_linear(
        "</b> flood",
        &unmatched_end_tags(25_000, "</b>"),
        &unmatched_end_tags(100_000, "</b>"),
        2.5,
    );
}

#[test]
#[ignore = "budget measurement"]
fn misnested_anchors_on_a_deep_stack_parse_linearly() {
    let shape = |n: usize| {
        let mut html = String::from("<!doctype html><body>");
        html.push_str(&repeat("<span>", n));
        html.push_str(&repeat("<a>", n));
        html
    };
    assert_linear("misnested <a>", &shape(25_000), &shape(100_000), 2.5);
}

#[test]
#[ignore = "budget measurement"]
fn foreign_end_tags_on_a_deep_stack_parse_linearly() {
    let shape = |n: usize| {
        let mut html = String::from("<!doctype html><body><svg>");
        html.push_str(&repeat("<g>", n));
        html.push_str(&repeat("</zzz>", n));
        html
    };
    assert_linear("foreign </zzz>", &shape(25_000), &shape(100_000), 2.5);
}

#[test]
#[ignore = "budget measurement"]
fn formatting_reconstruction_on_a_deep_stack_parses_linearly() {
    // 512 open formatting entries, then text after a `</div>` at every one
    // of `n` depths: the "still open" check must not scan the stack.
    let shape = |depth: usize| {
        let mut html = String::from("<!doctype html><body>");
        for index in 0..512 {
            html.push_str(&format!("<b id=b{index}>"));
        }
        html.push_str(&repeat("<div>", depth));
        html.push_str(&repeat("</div>y", depth));
        html
    };
    assert_linear("reconstruction at depth", &shape(2_000), &shape(8_000), 2.5);
}
