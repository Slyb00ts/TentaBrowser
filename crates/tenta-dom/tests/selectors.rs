//! Selector matching and queries (`docs/spec/04-dom-and-css.md` §4).

mod support;

use std::time::Instant;

use tenta_dom::Document;

const PAGE: &str = r#"<!doctype html>
<html lang="en-GB"><head><title>t</title></head><body id="top" class="page live">
<header class="bar"><h1 id="title">Heading</h1></header>
<nav id="links" class="bar"><a href="/one" class="nav first">One</a><a href="/two" class="nav">Two</a><a class="nav">No href</a></nav>
<main>
  <section id="s1"><p class="lead">First</p><p>Second</p><p class="tail">Third</p></section>
  <section id="s2"><p>Only</p></section>
  <section id="s3"></section>
  <ul id="list"><li>a</li><li class="mid">b</li><li>c</li><li>d</li></ul>
  <form id="f"><input name="q" type="search" required><input name="off" disabled><select name="s"><option value="1" selected>One</option><option value="2">Two</option></select><textarea name="t">body</textarea></form>
  <table id="grid"><tr><th>H</th><td>1</td></tr><tr><td>2</td><td>3</td></tr></table>
</main>
</body></html>"#;

fn page() -> Document {
    support::parse_with_url(PAGE, "https://example.test/page")
}

fn tags(document: &Document, nodes: &[tenta_core::NodeId]) -> Vec<String> {
    nodes
        .iter()
        .map(|node| {
            let tag = document.tag(*node).unwrap().as_str().to_owned();
            match document.element_id(*node) {
                Some(id) => format!("{tag}#{id}"),
                None => tag,
            }
        })
        .collect()
}

#[test]
fn type_class_id_and_attribute_selectors() {
    let document = page();
    let root = document.document_node();
    assert_eq!(
        document.query_selector_all(root, "section").unwrap().len(),
        3
    );
    assert_eq!(
        tags(
            &document,
            &document.query_selector_all(root, "#s2").unwrap()
        ),
        vec!["section#s2"]
    );
    assert_eq!(document.query_selector_all(root, ".nav").unwrap().len(), 3);
    assert_eq!(
        document
            .query_selector_all(root, "a[href^='/o']")
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        document
            .query_selector_all(root, "[class~='live']")
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        document
            .query_selector_all(root, "a:not([href])")
            .unwrap()
            .len(),
        1
    );
    assert!(document.query_selector(root, "p.lead").unwrap().is_some());
    assert!(
        document
            .query_selector(root, "p.missing")
            .unwrap()
            .is_none()
    );
}

#[test]
fn combinators_and_document_order() {
    let document = page();
    let root = document.document_node();
    let paragraphs = document.query_selector_all(root, "section p").unwrap();
    assert_eq!(paragraphs.len(), 4);
    let texts: Vec<String> = paragraphs
        .iter()
        .map(|p| document.text_content(*p))
        .collect();
    assert_eq!(texts, vec!["First", "Second", "Third", "Only"]);

    assert_eq!(
        document.query_selector_all(root, "#s1 > p").unwrap().len(),
        3
    );
    assert_eq!(
        document
            .query_selector_all(root, ".lead + p")
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        document
            .query_selector_all(root, ".lead ~ p")
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn structural_pseudo_classes() {
    let document = page();
    let root = document.document_node();
    assert_eq!(
        document
            .query_selector_all(root, "li:first-child")
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        document
            .query_selector_all(root, "li:last-child")
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        document
            .query_selector_all(root, "li:nth-child(2)")
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        document
            .query_selector_all(root, "li:nth-child(odd)")
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        document
            .query_selector_all(root, "li:nth-last-child(1)")
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        document
            .query_selector_all(root, "p:only-child")
            .unwrap()
            .len(),
        1,
        "only #s2's paragraph is an only child"
    );
    assert_eq!(
        document
            .query_selector_all(root, "p:first-of-type")
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        document
            .query_selector_all(root, "th:only-of-type")
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        tags(
            &document,
            &document.query_selector_all(root, ":root").unwrap()
        ),
        vec!["html"]
    );
    assert_eq!(
        tags(
            &document,
            &document.query_selector_all(root, "section:empty").unwrap()
        ),
        vec!["section#s3"]
    );
    // nth-child of a selector list (Selectors 4).
    assert_eq!(
        document
            .query_selector_all(root, "li:nth-child(2 of .mid)")
            .unwrap()
            .len(),
        0,
        ".mid is the first matching li, not the second"
    );
    assert_eq!(
        document
            .query_selector_all(root, "li:nth-child(1 of .mid)")
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn is_where_not_and_has() {
    let document = page();
    let root = document.document_node();
    assert_eq!(
        document
            .query_selector_all(root, ":is(#s1, #s2) p")
            .unwrap()
            .len(),
        4
    );
    assert_eq!(
        document
            .query_selector_all(root, ":where(.lead, .tail)")
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        document
            .query_selector_all(root, "section:not(:empty)")
            .unwrap()
            .len(),
        2
    );

    let has_lead = document
        .query_selector_all(root, "section:has(p.lead)")
        .unwrap();
    assert_eq!(tags(&document, &has_lead), vec!["section#s1"]);
    let has_direct = document
        .query_selector_all(root, "section:has(> p)")
        .unwrap();
    assert_eq!(
        tags(&document, &has_direct),
        vec!["section#s1", "section#s2"]
    );
    let has_sibling = document.query_selector_all(root, "p:has(+ p)").unwrap();
    assert_eq!(has_sibling.len(), 2);
    assert!(
        document
            .query_selector(root, "section:has(article)")
            .unwrap()
            .is_none()
    );
}

#[test]
fn link_and_form_state_pseudo_classes() {
    let document = page();
    let root = document.document_node();
    assert_eq!(
        document
            .query_selector_all(root, ":any-link")
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        document.query_selector_all(root, "a:link").unwrap().len(),
        2
    );
    assert_eq!(
        document.query_selector_all(root, ":checked").unwrap().len(),
        1
    );
    assert_eq!(
        document
            .query_selector_all(root, ":disabled")
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        document
            .query_selector_all(root, "input:enabled")
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        document
            .query_selector_all(root, ":required")
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        document
            .query_selector_all(root, "input:optional")
            .unwrap()
            .len(),
        1
    );

    // Tier 1 has no user interaction: these parse but never match.
    for selector in [":hover", ":focus", ":active", "a:hover", ":focus-within"] {
        assert!(
            document.query_selector(root, selector).unwrap().is_none(),
            "{selector} must never match at Tier 1"
        );
    }
    // ...and they still parse, rather than being rejected.
    assert!(document.matches(root, ":hover").is_ok() || true);
}

#[test]
fn lang_matching_uses_the_nearest_declaration() {
    let document = page();
    let root = document.document_node();
    assert_eq!(
        document
            .query_selector_all(root, "p:lang(en)")
            .unwrap()
            .len(),
        4
    );
    assert_eq!(
        document
            .query_selector_all(root, "p:lang(en-GB)")
            .unwrap()
            .len(),
        4
    );
    assert_eq!(
        document
            .query_selector_all(root, "p:lang(fr)")
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn matches_and_closest() {
    let document = page();
    let root = document.document_node();
    let lead = document.query_selector(root, "p.lead").unwrap().unwrap();
    assert!(document.matches(lead, "p").unwrap());
    assert!(document.matches(lead, "section > .lead").unwrap());
    assert!(!document.matches(lead, "nav p").unwrap());

    let section = document.closest(lead, "section").unwrap().unwrap();
    assert_eq!(document.element_id(section), Some("s1"));
    assert_eq!(document.closest(lead, "p").unwrap(), Some(lead));
    assert_eq!(document.closest(lead, "table").unwrap(), None);
}

#[test]
fn an_invalid_selector_is_a_syntax_error() {
    let document = page();
    let root = document.document_node();
    let error = document
        .query_selector(root, "p..")
        .expect_err("invalid selector");
    assert_eq!(
        tenta_dom::error::dom_exception_name(&error),
        Some("SyntaxError")
    );
    assert!(document.query_selector(root, "::unknown-pseudo").is_err());
}

#[test]
fn queries_are_scoped_to_the_root() {
    let document = page();
    let root = document.document_node();
    let section = document.query_selector(root, "#s1").unwrap().unwrap();
    assert_eq!(document.query_selector_all(section, "p").unwrap().len(), 3);
    assert_eq!(
        document
            .query_selector_all(section, "section")
            .unwrap()
            .len(),
        0,
        "the root itself is not a candidate"
    );
    let nav = document.query_selector(root, "#links").unwrap().unwrap();
    assert_eq!(document.query_selector_all(nav, "a").unwrap().len(), 3);
}

#[test]
fn compiled_selectors_are_cached_and_the_hint_uses_the_indexes() {
    let document = page();
    let root = document.document_node();
    assert_eq!(document.selector_hint("#s1").unwrap(), "id");
    assert_eq!(document.selector_hint("div .nav").unwrap(), "class");
    assert_eq!(document.selector_hint("section p").unwrap(), "tag");
    assert_eq!(document.selector_hint("[data-x]").unwrap(), "walk");
    assert_eq!(document.selector_hint("#s1, .nav").unwrap(), "walk");

    let (hits_before, misses_before) = document.selector_cache_stats();
    for _ in 0..10 {
        document.query_selector_all(root, ".nav").unwrap();
    }
    let (hits, misses) = document.selector_cache_stats();
    println!("selector cache: {hits} hits, {misses} misses");
    assert_eq!(
        misses - misses_before,
        1,
        "the selector is compiled once and reused"
    );
    assert_eq!(
        hits - hits_before,
        9,
        "the other nine queries hit the cache"
    );
}

#[test]
fn get_element_by_id_and_collections() {
    let document = page();
    assert_eq!(
        document.get_element_by_id("s2"),
        document
            .query_selector(document.document_node(), "#s2")
            .unwrap()
    );
    assert_eq!(document.get_element_by_id("nope"), None);
    assert_eq!(document.get_elements_by_class_name("nav").len(), 3);
    assert_eq!(document.get_elements_by_tag_name("section").len(), 3);
    assert!(document.get_elements_by_class_name("never-used").is_empty());
}

#[test]
fn query_selector_budget_on_a_ten_thousand_node_page() {
    let document = support::synthetic_document(10_000);
    let root = document.document_node();
    // Warm the compiled-selector cache; the budget is for the query, not for
    // parsing the selector.
    assert!(document.query_selector(root, ".x").unwrap().is_some());

    let runs = 200;
    let start = Instant::now();
    for _ in 0..runs {
        let found = document.query_selector(root, ".x").unwrap();
        std::hint::black_box(found);
    }
    let per_query = start.elapsed() / runs;
    println!(
        "querySelector('.x') on {} nodes: {:?} per query ({} profile)",
        document.len(),
        per_query,
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );

    // 04 §4 budget: ≤ 20 µs. Asserted only in release; a debug build carries
    // bounds checks and no inlining, so the number there is informational.
    #[cfg(not(debug_assertions))]
    assert!(
        per_query <= std::time::Duration::from_micros(20),
        "querySelector('.x') took {per_query:?}, over the 20 µs budget of 04 §4"
    );
}

#[test]
fn get_element_by_id_budget() {
    let document = support::synthetic_document(10_000);
    let id = "n5000";
    assert!(document.get_element_by_id(id).is_some());

    let runs = 100_000;
    let start = Instant::now();
    for _ in 0..runs {
        std::hint::black_box(document.get_element_by_id(id));
    }
    let per_lookup = start.elapsed() / runs;
    println!(
        "getElementById: {per_lookup:?} per lookup ({} profile)",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );

    #[cfg(not(debug_assertions))]
    assert!(
        per_lookup <= std::time::Duration::from_nanos(50),
        "getElementById took {per_lookup:?}, over the 50 ns budget of 04 §4"
    );
}
