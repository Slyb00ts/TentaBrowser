//! Shapes of markup whose cost used to grow quadratically: many attributes on
//! one tag, deeply nested formatting elements, and deeply nested XML.
//!
//! Each test times one run over a 4x input against four runs over the 1x one.
//! Both sides do the same total work, so a linear implementation makes the
//! ratio 1 and a quadratic one makes it 4, and the comparison does not depend
//! on how fast the machine is. They run in the normal suite because the
//! assertion is about the shape of the curve, not about an absolute budget.

mod common;

use std::time::{Duration, Instant};

use common::tree::Tree;
use tenta_html::xml::{Parser as XmlParser, XmlOptions};
use tenta_html::{Parser, ParserOptions};

/// Cost of the two workloads, measured alternately, best of three rounds.
///
/// The control does the small workload `ratio` times, so it performs the same
/// total work as one run of the large one. Comparing those two rather than
/// comparing one small run scaled by `ratio` makes the assertion a ratio of
/// two measurements taken in the same window on the same machine: whatever
/// slows one slows the other, which matters because these run in the normal
/// suite alongside every other test binary. Taking the best of three rounds
/// removes the rest of the interference; a quadratic path is slower on its
/// best round too.
fn best_pair(ratio: u32, mut small: impl FnMut(), mut large: impl FnMut()) -> (Duration, Duration) {
    let (mut control, mut scaled) = (Duration::MAX, Duration::MAX);
    for _ in 0..3 {
        let start = Instant::now();
        for _ in 0..ratio {
            small();
        }
        control = control.min(start.elapsed());
        let start = Instant::now();
        large();
        scaled = scaled.min(start.elapsed());
    }
    (control, scaled)
}

/// Asserts that one run over the `ratio`-times-larger input costs no more than
/// `slack` times `ratio` runs over the small one.
///
/// A linear algorithm lands at 1.0; a quadratic one at `ratio`, so `slack` has
/// room to absorb cache effects without ever reaching the quadratic answer.
fn assert_scales_linearly(name: &str, control: Duration, scaled: Duration, ratio: u32, slack: f64) {
    let allowed = control.mul_f64(slack) + Duration::from_millis(5);
    println!(
        "{name}: {ratio} x small {control:?}, 1 x {ratio}-size {scaled:?} \
         (ratio {:.2}, allowed {allowed:?}, quadratic would be {ratio}.00)",
        scaled.as_secs_f64() / control.as_secs_f64()
    );
    assert!(
        scaled <= allowed,
        "{name} took {scaled:?} for one {ratio}x input against {control:?} for {ratio} small \
         ones, which is not linear (allowed {allowed:?})"
    );
}

fn parse_html(html: &str) -> usize {
    let mut parser = Parser::new(Tree::new(), ParserOptions::default());
    parser.feed_str(html);
    parser.finish().nodes.len()
}

fn tag_with_attributes(count: usize) -> String {
    let mut html = String::with_capacity(count * 12 + 32);
    html.push_str("<!doctype html><p");
    for index in 0..count {
        html.push_str(&format!(" a{index}=v"));
    }
    html.push_str(">text</p>");
    html
}

#[test]
fn thousands_of_attributes_on_one_tag_parse_correctly() {
    // Structural test: verify the tree builds correctly with many attributes.
    let html = tag_with_attributes(1_000);
    let nodes = parse_html(&html);
    assert!(nodes > 3, "tree should build with many attributes");
}

#[test]
#[ignore = "budget measurement"]
fn thousands_of_attributes_on_one_tag_parse_linearly() {
    let small = tag_with_attributes(4_000);
    let large = tag_with_attributes(16_000);
    // Warm the atom table so interning the names is not counted twice.
    parse_html(&small);
    parse_html(&large);

    let (short, long) = best_pair(
        4,
        || {
            std::hint::black_box(parse_html(&small));
        },
        || {
            std::hint::black_box(parse_html(&large));
        },
    );
    assert_scales_linearly("16 000 attributes", short, long, 4, 2.5);
}

#[test]
fn duplicate_attributes_are_still_dropped_past_the_set_threshold() {
    // The hash-set path takes over above 16 attributes; the first value of a
    // duplicated name still wins, as the tokenizer spec requires.
    let mut html = String::from("<p");
    for index in 0..40 {
        html.push_str(&format!(" a{index}=first"));
    }
    for index in 0..40 {
        html.push_str(&format!(" a{index}=second"));
    }
    html.push('>');

    let mut parser = Parser::new(Tree::new(), ParserOptions::default());
    parser.feed_str(&html);
    let tree = parser.finish();
    let attrs = tree
        .nodes
        .iter()
        .find_map(|node| match &node.data {
            common::tree::NodeData::Element { name, attrs, .. }
                if name.local == tenta_core::atom::atoms::P =>
            {
                Some(attrs.clone())
            }
            _ => None,
        })
        .expect("the <p> element");
    assert_eq!(attrs.len(), 40, "duplicates must not be stored");
    assert!(
        attrs.iter().all(|attr| attr.value == "first"),
        "the first value of a duplicated attribute wins"
    );
}

fn nested_formatting(count: usize) -> String {
    let mut html = String::with_capacity(count * 16 + 32);
    html.push_str("<!doctype html><body>");
    for index in 0..count {
        html.push_str(&format!("<b id=x{index}>t"));
    }
    html
}

#[test]
fn deeply_nested_formatting_elements_build_linear_node_counts() {
    // Structural test: verify that deeply nested formatting elements don't
    // amplify node counts quadratically.
    let small = nested_formatting(4_000);
    let large = nested_formatting(16_000);

    let small_nodes = parse_html(&small);
    let large_nodes = parse_html(&large);

    // After the fix, nodes should grow linearly: 4x tags should not
    // produce 4x quadratic amplification. Allow 4x slack for tree overhead.
    let tag_ratio = 16_000.0 / 4_000.0;
    let node_ratio = large_nodes as f64 / small_nodes as f64;
    assert!(
        node_ratio <= tag_ratio * 4.0,
        "formatting elements amplified: {} tags -> {} nodes, ratio {:.1}",
        16_000,
        large_nodes,
        node_ratio
    );
}

#[test]
#[ignore = "budget measurement"]
fn deeply_nested_formatting_elements_parse_linearly() {
    let small = nested_formatting(4_000);
    let large = nested_formatting(16_000);
    parse_html(&small);
    parse_html(&large);

    let (short, long) = best_pair(
        4,
        || {
            std::hint::black_box(parse_html(&small));
        },
        || {
            std::hint::black_box(parse_html(&large));
        },
    );
    assert_scales_linearly("16 000 nested <b>", short, long, 4, 2.5);
}

fn adoption_agency_pairs(count: usize) -> String {
    let mut html = String::with_capacity(count * 8 + 32);
    html.push_str("<!doctype html><body>");
    for _ in 0..count {
        html.push_str("<a><div>");
    }
    html
}

/// Every `<a>` runs the adoption agency against the previous `<div>`; once
/// the tree is at the depth cap, each run also checks the depth bookkeeping.
#[test]
#[ignore = "budget measurement"]
fn adoption_agency_at_the_depth_cap_parses_linearly() {
    let small = adoption_agency_pairs(2_500);
    let large = adoption_agency_pairs(10_000);
    parse_html(&small);
    parse_html(&large);

    let (short, long) = best_pair(
        4,
        || {
            std::hint::black_box(parse_html(&small));
        },
        || {
            std::hint::black_box(parse_html(&large));
        },
    );
    assert_scales_linearly("10 000 <a><div> pairs", short, long, 4, 2.5);
}

fn nested_xml(count: usize) -> String {
    let mut xml = String::with_capacity(count * 8 + 32);
    xml.push_str("<r>");
    for _ in 0..count {
        xml.push_str("<a>");
    }
    for _ in 0..count {
        xml.push_str("</a>");
    }
    xml.push_str("</r>");
    xml
}

fn parse_xml(text: &str) -> Result<Tree, tenta_html::xml::XmlError> {
    let mut parser = XmlParser::new(Tree::new(), XmlOptions::default());
    parser.feed_str(text);
    parser.finish()
}

#[test]
fn xml_nesting_is_capped() {
    let deep = nested_xml(100_000);
    let error = parse_xml(&deep).expect_err("100 000 levels is past the cap");
    assert!(
        error.message.contains("nesting deeper than"),
        "unexpected error {error:?}"
    );

    // The cap is reported at the element that crosses it, so the parse stops
    // there rather than walking the rest of the document.
    let small = nested_xml(125);
    let large = nested_xml(500);
    parse_xml(&small).expect("125 levels is under the cap");
    parse_xml(&large).expect("500 levels is under the cap");
}

#[test]
#[ignore = "budget measurement"]
fn xml_nesting_costs_stay_linear() {
    // Both sides stay under `MAX_XML_DEPTH`, so the cap itself is not what is
    // being timed.
    let small = nested_xml(125);
    let large = nested_xml(500);
    parse_xml(&small).expect("125 levels is under the cap");
    parse_xml(&large).expect("500 levels is under the cap");
    let (short, long) = best_pair(
        4,
        || {
            std::hint::black_box(parse_xml(&small).is_ok());
        },
        || {
            std::hint::black_box(parse_xml(&large).is_ok());
        },
    );
    assert_scales_linearly("500 nested XML elements", short, long, 4, 2.5);
}

#[test]
fn xml_prefix_lookup_parses_correctly_with_many_prefixes() {
    // Every level declares its own prefix; verify parsing succeeds.
    fn document(levels: usize) -> String {
        let mut xml = String::new();
        for index in 0..levels {
            xml.push_str(&format!(
                "<p{index}:e xmlns:p{index}=\"urn:{index}\" p{index}:a=\"1\">"
            ));
        }
        for index in (0..levels).rev() {
            xml.push_str(&format!("</p{index}:e>"));
        }
        xml
    }
    let small = document(100);
    parse_xml(&small).expect("well formed with 100 prefixes");
}

#[test]
#[ignore = "budget measurement"]
fn xml_prefix_lookup_does_not_degrade_with_declaration_depth() {
    // Every level declares its own prefix; verify resolution stays linear.
    fn document(levels: usize) -> String {
        let mut xml = String::new();
        for index in 0..levels {
            xml.push_str(&format!(
                "<p{index}:e xmlns:p{index}=\"urn:{index}\" p{index}:a=\"1\">"
            ));
        }
        for index in (0..levels).rev() {
            xml.push_str(&format!("</p{index}:e>"));
        }
        xml
    }
    let small = document(100);
    let large = document(400);
    parse_xml(&small).expect("well formed");
    parse_xml(&large).expect("well formed");
    let (short, long) = best_pair(
        4,
        || {
            std::hint::black_box(parse_xml(&small).is_ok());
        },
        || {
            std::hint::black_box(parse_xml(&large).is_ok());
        },
    );
    assert_scales_linearly("400 nested prefix declarations", short, long, 4, 2.5);
}
