//! List markers: bullet boxes against Chrome, counter strings by structure.
//!
//! The bullet rectangles in `block_complete/bullet_cases.rs` were measured in
//! Chrome 153 screenshots at a list item whose border box starts at x = 100.

// Marker text needs the bundled fonts.
#![cfg(feature = "embedded-fonts")]

#[path = "block_complete/bullet_cases.rs"]
mod bullet_cases;
mod support;

use tenta_layout::{FragId, FragmentKind, MarkerKind, Rect};

use support::{Laid, lay_out, rect_mismatch};

fn marker_of(laid: &Laid, item: &str) -> FragId {
    let item_frag = laid.frag(item);
    *laid
        .frags
        .get(item_frag)
        .children
        .iter()
        .find(|&&child| matches!(laid.frags.get(child).kind, FragmentKind::Marker(_)))
        .expect("the list item has a marker fragment")
}

fn page(style: &str, list: &str) -> String {
    format!(
        "<!doctype html><style>body{{margin:0;font:16px LS}} ul,ol{{margin:0;padding-left:100px}} {style}</style>{list}"
    )
}

#[test]
fn bullets_match_chrome() {
    let mut failures = Vec::new();
    for (name, html, expected) in bullet_cases::CASES {
        let laid = lay_out(html, (400.0, 300.0));
        let marker = marker_of(&laid, "a");
        let rect = laid.frags.absolute_border_box(marker);
        if let Some(why) = rect_mismatch(rect, *expected) {
            failures.push(format!("{name}: {why}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_bullet_belongs_to_no_node_so_the_item_keeps_its_own_box() {
    let laid = lay_out(&page("", "<ul><li id=a>Hello</li></ul>"), (400.0, 300.0));
    let item = laid.frag("a");
    assert_eq!(laid.frags.get(item).kind, FragmentKind::Box);
    assert_eq!(laid.rect("a"), Rect::new(100.0, 0.0, 300.0, 18.0));
}

#[test]
fn a_counter_marker_is_a_text_fragment_ending_at_the_item() {
    let laid = lay_out(&page("", "<ol><li id=a>Hello</li></ol>"), (400.0, 300.0));
    let marker = marker_of(&laid, "a");
    let fragment = laid.frags.get(marker);
    assert_eq!(fragment.kind, FragmentKind::Marker(MarkerKind::Text));
    // "1. " is 8 + 4 + 4 px wide in Liberation Serif at 16px, and the trailing
    // space is the gap to the item.
    let rect = laid.frags.absolute_border_box(marker);
    assert_eq!((rect.x, rect.w), (84.0, 16.0));
    assert_eq!(fragment.children.len(), 1);
    let text = laid.frags.get(fragment.children[0]);
    assert!(matches!(&text.kind, FragmentKind::Text(t) if !t.runs.is_empty()));
}

#[test]
fn a_counter_marker_shares_the_baseline_of_the_first_line() {
    let html = page(
        "li{line-height:40px;padding-top:6px}",
        "<ol start=10><li id=a>Hello</li></ol>",
    );
    let laid = lay_out(&html, (400.0, 300.0));
    let marker = marker_of(&laid, "a");
    let marker_text = laid.frags.get(marker).children[0];
    let marker_baseline = baseline_of(&laid, marker_text);
    let item = laid.frags.get(laid.frag("a"));
    let line = item
        .children
        .iter()
        .copied()
        .find(|&c| laid.frags.get(c).kind == FragmentKind::Line)
        .expect("the item has a line");
    let text = laid.frags.get(line).children[0];
    assert_eq!(marker_baseline, baseline_of(&laid, text));
    // "10. " is 8 + 8 + 4 + 4 px wide.
    assert_eq!(laid.frags.get(marker).rect.w, 24.0);
}

fn baseline_of(laid: &Laid, frag: FragId) -> f32 {
    let FragmentKind::Text(text) = &laid.frags.get(frag).kind else {
        panic!("not a text fragment");
    };
    laid.frags.absolute_border_box(frag).y + text.baseline
}

#[test]
fn a_string_marker_is_text_ending_at_the_item() {
    let laid = lay_out(
        &page("li{list-style:'->'}", "<ul><li id=a>Hello</li></ul>"),
        (400.0, 300.0),
    );
    let marker = marker_of(&laid, "a");
    let rect = laid.frags.absolute_border_box(marker);
    assert_eq!(rect.right(), 100.0);
    assert!(rect.w > 0.0);
}

#[test]
fn list_style_none_creates_no_marker() {
    let laid = lay_out(
        &page("li{list-style:none}", "<ul><li id=a>Hello</li></ul>"),
        (400.0, 300.0),
    );
    let item = laid.frags.get(laid.frag("a"));
    assert!(
        item.children
            .iter()
            .all(|&c| !matches!(laid.frags.get(c).kind, FragmentKind::Marker(_)))
    );
}

/// Needs inline atomics, so it reports itself skipped until they land. Chrome
/// 153 puts the bullet 7px below the top of a marker whose baseline is the
/// bottom of the 50px canvas.
#[test]
fn a_tall_atomic_on_the_first_line_moves_the_bullet_down() {
    let html = page(
        "ul{list-style:disc}",
        "<ul><li id=a><canvas style='width:50px;height:50px'></canvas></li></ul>",
    );
    let Some(laid) = support::try_lay_out(&html, (400.0, 300.0)) else {
        return;
    };
    let rect = laid.frags.absolute_border_box(marker_of(&laid, "a"));
    assert_eq!(rect_mismatch(rect, [84.0, 43.0, 5.0, 5.0]), None);
}

/// Chrome 153 screenshot: with `line-height: 40px` and `padding-top: 6px` the
/// bottom of the digit `1`, in the marker and in the item's own text alike, is
/// pixel row 31 (row 20 with `line-height: normal`).
#[test]
fn a_counter_marker_baseline_matches_chrome() {
    for (line_height, baseline) in [("40px", 31.0), ("normal", 20.0)] {
        let html = page(
            &format!("li{{line-height:{line_height};padding-top:6px}}"),
            "<ol><li id=a>1</li></ol>",
        );
        let laid = lay_out(&html, (400.0, 300.0));
        let marker = marker_of(&laid, "a");
        let text = laid.frags.get(marker).children[0];
        assert_eq!(
            baseline_of(&laid, text),
            baseline,
            "line-height {line_height}"
        );
    }
}
