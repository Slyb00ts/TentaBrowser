//! `matches` and `query_selector_all` must agree
//! (`docs/spec/04-dom-and-css.md` §4).
//!
//! `query_selector_all` narrows the candidate set through the id, class and
//! tag indexes before matching, so a wrong hint silently drops elements that
//! `matches` accepts. This test asserts the invariant directly: for every
//! element of the document and every selector in the table,
//! `matches(element, selector)` is true exactly when the element appears in
//! `query_selector_all(root, selector)`.

mod support;

use tenta_core::NodeId;
use tenta_dom::Document;
use tenta_html::QuirksMode;

/// Ids, classes and tag names that stress the case rules: a camelCase SVG
/// element name (case-sensitive in SVG, matched case-insensitively in HTML),
/// ids and classes differing only in case, and an id that is also a class.
const PAGE: &str = r#"<!doctype html>
<html><body id="Top" class="Page live">
<div id="box" class="Row row"><span class="row">a</span></div>
<div id="Box" class="ROW"><span class="Row">b</span></div>
<p class="box">paragraph whose class is another element's id</p>
<section><a href="/x" class="link">x</a><a class="link">no href</a></section>
<ul><li>1</li><li class="mid">2</li><li>3</li><li class="mid">4</li></ul>
<svg viewBox="0 0 1 1"><clipPath id="cp"><rect/></clipPath><linearGradient id="lg"/></svg>
<table><tr><th>H</th><td class="cell">1</td></tr><tr><td class="cell">2</td><td>3</td></tr></table>
<template><i id="inert">not in the light tree</i></template>
</body></html>"#;

const SELECTORS: &[&str] = &[
    "*",
    "div",
    "DIV",
    "span",
    "p",
    "#box",
    "#Box",
    "#Top",
    "#cp",
    ".row",
    ".Row",
    ".ROW",
    ".box",
    ".link",
    ".mid",
    "div.row",
    "div#box.Row",
    "#box .row",
    "#box > span",
    "div span",
    "div + div",
    "div ~ p",
    "a[href]",
    "a[href^='/x']",
    "a[class~='link']",
    "[id]",
    "[id='box']",
    "[viewBox]",
    "clipPath",
    "clippath",
    "linearGradient",
    "svg clipPath",
    "svg > *",
    "rect",
    "li:nth-child(2)",
    "li:nth-child(odd)",
    "li:nth-last-child(1)",
    "li:first-child",
    "li:last-child",
    "section :only-of-type",
    "section:empty",
    ":root",
    ":is(div, p).box",
    ":is(#box, #Box)",
    ":not(.row)",
    "div:not(.row)",
    ":has(> span)",
    "div:has(.Row)",
    "td.cell",
    "tr th",
    "#box, .link",
    "div, span, p",
    "#missing",
    ".missing",
    "missingtag",
];

fn all_elements(document: &Document) -> Vec<NodeId> {
    document
        .descendants_inclusive(document.document_node())
        .filter(|node| document.is_element(*node))
        .collect()
}

fn check(document: &Document, label: &str) {
    let root = document.document_node();
    let elements = all_elements(document);
    assert!(
        elements.len() > 10,
        "{label}: the fixture must have elements"
    );

    for selector in SELECTORS {
        let found = document
            .query_selector_all(root, selector)
            .unwrap_or_else(|error| panic!("{label}: {selector} failed to compile: {error}"));

        // The query result must be in document order and free of duplicates.
        let mut ordered = found.clone();
        document.sort_tree_order(&mut ordered);
        assert_eq!(found, ordered, "{label}: {selector} is not in tree order");
        let mut unique = found.clone();
        unique.dedup();
        assert_eq!(
            unique.len(),
            found.len(),
            "{label}: {selector} has duplicates"
        );

        for element in &elements {
            let matched = document
                .matches(*element, selector)
                .unwrap_or_else(|error| panic!("{label}: {selector} match failed: {error}"));
            let listed = found.contains(element);
            assert_eq!(
                matched,
                listed,
                "{label}: {selector} on <{}{}>: matches() said {matched}, \
                 query_selector_all() said {listed}",
                document
                    .tag(*element)
                    .map(|t| t.to_string())
                    .unwrap_or_default(),
                document
                    .element_id(*element)
                    .map(|id| format!(" id={id}"))
                    .unwrap_or_default(),
            );
        }

        // The first match of the list is what query_selector must return.
        assert_eq!(
            document.query_selector(root, selector).unwrap(),
            found.first().copied(),
            "{label}: query_selector({selector}) disagrees with the list"
        );
    }
}

#[test]
fn matching_and_querying_agree_in_standards_mode() {
    let document = support::parse(PAGE);
    assert_eq!(document.quirks_mode(), QuirksMode::NoQuirks);
    check(&document, "standards");
}

#[test]
fn matching_and_querying_agree_in_quirks_mode() {
    // Without a doctype the document is in quirks mode, where class and id
    // selectors match ASCII case-insensitively; the index hints must be
    // dropped rather than used with the wrong case.
    let document = support::parse(PAGE.trim_start_matches("<!doctype html>\n"));
    assert_eq!(document.quirks_mode(), QuirksMode::Quirks);
    check(&document, "quirks");
}

#[test]
fn quirks_mode_matches_ids_and_classes_case_insensitively() {
    let document = support::parse(PAGE.trim_start_matches("<!doctype html>\n"));
    let root = document.document_node();
    assert_eq!(
        document.query_selector_all(root, "#BOX").unwrap().len(),
        2,
        "#box and #Box both match in quirks mode"
    );
    assert_eq!(
        document.query_selector_all(root, ".row").unwrap().len(),
        4,
        "Row, row and ROW all match in quirks mode"
    );

    let standards = support::parse(PAGE);
    assert_eq!(
        standards
            .query_selector_all(standards.document_node(), "#BOX")
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        standards
            .query_selector_all(standards.document_node(), ".row")
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn camel_case_svg_local_names_are_case_sensitive() {
    let document = support::parse(PAGE);
    let root = document.document_node();
    assert_eq!(
        document.query_selector_all(root, "clipPath").unwrap().len(),
        1,
        "the SVG element name matches with its own case"
    );
    assert_eq!(
        document.query_selector_all(root, "clippath").unwrap().len(),
        0,
        "SVG local names are case-sensitive, unlike HTML ones"
    );
    assert_eq!(
        document.query_selector_all(root, "DIV").unwrap().len(),
        document.query_selector_all(root, "div").unwrap().len(),
        "HTML local names are matched case-insensitively"
    );
}

#[test]
fn get_element_by_id_is_case_sensitive_even_in_quirks_mode() {
    let quirks = support::parse(PAGE.trim_start_matches("<!doctype html>\n"));
    assert_eq!(quirks.quirks_mode(), QuirksMode::Quirks);

    // querySelector matches case-insensitively in quirks mode
    assert_eq!(
        quirks
            .query_selector_all(quirks.document_node(), "#BOX")
            .unwrap()
            .len(),
        2,
        "querySelector('.box') matches both #box and #Box case-insensitively in quirks mode"
    );

    // But getElementById is case-SENSITIVE per DOM spec, even in quirks mode
    assert!(
        quirks.get_element_by_id("box").is_some(),
        "getElementById('box') finds #box"
    );
    assert!(
        quirks.get_element_by_id("Box").is_some(),
        "getElementById('Box') finds #Box"
    );
    assert!(
        quirks.get_element_by_id("BOX").is_none(),
        "getElementById('BOX') finds nothing (case-sensitive)"
    );
}
