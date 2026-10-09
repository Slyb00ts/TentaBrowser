//! Block layout around inline formatting contexts: anonymous blocks, margin
//! collapsing across lines, empty inline content, and the corpus fixtures whose
//! block geometry does not depend on glyph advances.
//!
//! A test reports itself skipped while a layout step owned by a package that
//! has not landed yet is `todo!` (see `support::try_lay_out`).

// `FontSystem::new_embedded` exists only with the bundled fonts.
#![cfg(feature = "embedded-fonts")]

#[path = "block_inline/chrome_cases.rs"]
mod chrome_cases;
mod support;

use support::{Case, check_cases_when_ready, lay_out, rect_mismatch, try_lay_out};

/// Line height is pinned to 20px in these pages, so the expected values do not
/// depend on which font Chrome picked.
#[test]
fn rectangles_match_chrome() {
    check_cases_when_ready(chrome_cases::CASES);
}

/// The laid-out corpus fixture, or `None` while a layout step that the fixture
/// needs (`position: relative`, inline-blocks) is not implemented.
fn fixture(name: &str) -> Option<support::Laid> {
    let corpus = concat!(env!("CARGO_MANIFEST_DIR"), "/../../render-fixtures");
    let html = std::fs::read_to_string(format!("{corpus}/{name}/index.html"))
        .unwrap_or_else(|e| panic!("fixture {name}: {e}"));
    try_lay_out(&html, (900.0, 1000.0))
}

fn select(laid: &support::Laid, selector: &str) -> tenta_layout::Rect {
    let node = laid
        .doc
        .query_selector(laid.doc.document_node(), selector)
        .unwrap_or_else(|e| panic!("selector {selector}: {e:?}"))
        .unwrap_or_else(|| panic!("nothing matches {selector}"));
    let frag = *laid
        .frags
        .fragments_of(node)
        .first()
        .unwrap_or_else(|| panic!("{selector} has no fragment"));
    laid.frags.absolute_border_box(frag)
}

/// Gate entries `box-sizing` (content-box percent, border-box percent,
/// inherited reset). Only sizes and offsets inside the parent are asserted:
/// the paragraphs above them contain text whose height depends on the font.
#[test]
fn box_sizing_fixture_matches_the_corpus() {
    let Some(laid) = fixture("box-sizing") else {
        return;
    };
    let parent = select(&laid, ".parent");
    let content = select(&laid, ".half.content");
    let border = select(&laid, ".half.border");
    let reset = select(&laid, ".reset");
    let mut failures = Vec::new();
    for (what, actual, expected) in [
        (
            "content-box percent",
            content,
            [parent.x + 2.0, parent.y + 2.0, 230.0, 80.0],
        ),
        (
            "border-box percent",
            border,
            [parent.x + 2.0, parent.y + 82.0, 200.0, 50.0],
        ),
        ("parent", parent, [10.0, parent.y, 404.0, 134.0]),
        ("inherited reset", reset, [10.0, reset.y, 200.0, 70.0]),
    ] {
        if let Some(why) = rect_mismatch(actual, expected) {
            failures.push(format!("{what}: {why}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Gate entry `mixed-content-block` (full-width block child): a block between
/// inline runs gets the whole content width, however the text wraps.
#[test]
fn block_between_text_runs_spans_the_container() {
    let laid = lay_out(
        r#"<!doctype html><style>body{margin:0}.box{width:400px;border:2px solid;margin:8px}</style>
        <div class=box>leading inline text that should wrap across the full width of this container
        <div id=child style="background:#cdf">block child</div>
        trailing inline text after the block child</div>"#,
        (900.0, 1000.0),
    );
    let child = laid.rect("child");
    assert_eq!((child.x, child.w), (10.0, 400.0));
}

/// Gate entry `legacy-center` (auto width block remains fill available).
#[test]
fn legacy_center_auto_block_fills_its_host() {
    let Some(laid) = fixture("legacy-center") else {
        return;
    };
    let block = select(&laid, "#legacy > div:nth-child(4)");
    assert_eq!((block.x, block.w, block.h), (0.0, 400.0, 20.0));
}
