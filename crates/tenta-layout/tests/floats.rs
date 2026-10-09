//! CSS floats and `clear` against Chrome, plus placement invariants.
//!
//! The expectations in `floats/chrome_cases.rs` are `getBoundingClientRect`
//! values recorded from headless Chrome 154 for exactly the HTML next to them,
//! with the bundled Liberation Sans declared through `@font-face`. The cases are
//! grouped by name prefix: `fl_` placement, `tx_` line shortening, `cl_` clear,
//! `bfc_` formatting-context roots, `in_` floats inside inline content, `sh_`
//! and `im_` shrink-to-fit and replaced floats, `rt_` direction, `fx_` flex and
//! grid, `ps_` positioned boxes, `ns_` nesting and whole-page shapes.

// `FontSystem::new_embedded` exists only with the bundled fonts.
#![cfg(feature = "embedded-fonts")]

#[path = "floats/chrome_cases.rs"]
mod chrome_cases;
mod support;

use support::{Case, Laid, lay_out, rect_mismatch};
use tenta_layout::{FragFlags, FragId, FragmentKind, Rect};

/// What `getBoundingClientRect` reports: the union of every fragment of the
/// element, so an inline box split over several lines covers all of them.
fn client_rect(laid: &Laid, id: &str) -> Rect {
    let node = laid.node(id);
    let mut union: Option<Rect> = None;
    // Generated boxes share the element's node; the element's own fragments
    // are the outermost ones.
    let outermost = |mut frag: FragId| {
        while let Some(parent) = laid.frags.parent(frag) {
            if laid.frags.get(parent).node == node {
                return false;
            }
            frag = parent;
        }
        true
    };
    for &frag in laid.frags.fragments_of(node) {
        if !outermost(frag) {
            continue;
        }
        let rect = laid.frags.absolute_border_box(frag);
        union = Some(union.map_or(rect, |u| {
            let (x, y) = (u.x.min(rect.x), u.y.min(rect.y));
            let (r, b) = (u.right().max(rect.right()), u.bottom().max(rect.bottom()));
            Rect::new(x, y, r - x, b - y)
        }));
    }
    union.unwrap_or_default()
}

/// Lays out every recorded case whose name starts with `prefix` and fails with
/// one line per mismatching rectangle.
fn check_prefix(prefix: &str) {
    let cases: Vec<&Case> = chrome_cases::CASES
        .iter()
        .filter(|case| case.name.starts_with(prefix))
        .collect();
    assert!(!cases.is_empty(), "no case starts with {prefix}");
    let mut failures = Vec::new();
    for case in cases {
        let laid = lay_out(case.html, case.viewport);
        for (id, expected) in case.expect {
            if let Some(why) = rect_mismatch(client_rect(&laid, id), *expected) {
                failures.push(format!("{} / #{id}: {why}", case.name));
            }
        }
        let extent = laid.frags.scroll_extent;
        if (extent.w, extent.h) != case.scroll {
            failures.push(format!(
                "{} / scroll extent: got {:?}, Chrome {:?}",
                case.name,
                (extent.w, extent.h),
                case.scroll
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn float_placement_matches_chrome() {
    check_prefix("fl_");
}

#[test]
fn lines_beside_floats_match_chrome() {
    check_prefix("tx_");
}

#[test]
fn clear_and_clearance_match_chrome() {
    check_prefix("cl_");
}

#[test]
fn formatting_context_roots_match_chrome() {
    check_prefix("bfc_");
}

#[test]
fn floats_in_inline_content_match_chrome() {
    check_prefix("in_");
}

#[test]
fn shrink_to_fit_floats_match_chrome() {
    check_prefix("sh_");
}

#[test]
fn replaced_floats_match_chrome() {
    check_prefix("im_");
}

#[test]
fn right_to_left_and_logical_floats_match_chrome() {
    check_prefix("rt_");
}

#[test]
fn flex_and_grid_with_floats_match_chrome() {
    check_prefix("fx_");
}

#[test]
fn positioned_boxes_and_floats_match_chrome() {
    check_prefix("ps_");
}

#[test]
fn nested_floats_and_page_shapes_match_chrome() {
    check_prefix("ns_");
}

// ---------------------------------------------------------------- invariants

/// A small deterministic generator, so a failing scene can be replayed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len() as u64) as usize]
    }
}

const WORDS: &[&str] = &[
    "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta", "iota", "kappa",
    "lambda", "mu", "nu", "xi", "omicron", "pi", "rho", "sigma", "tau", "upsilon",
];

fn sentence(rng: &mut Rng, words: u64) -> String {
    (0..words)
        .map(|_| rng.pick(WORDS))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A random page of floats, paragraphs, clearing blocks and context roots.
fn scene(seed: u64) -> (String, f32) {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let width = 260 + rng.below(400);
    let mut body = String::new();
    for _ in 0..(6 + rng.below(14)) {
        match rng.below(8) {
            0..=2 => {
                let side = rng.pick(&["left", "right"]);
                let clear = rng.pick(&["none", "none", "none", "left", "right", "both"]);
                let (w, h) = (20 + rng.below(width / 2 - 20), 8 + rng.below(70));
                let (mv, mh) = (rng.below(8), rng.below(12));
                body.push_str(&format!(
                    "<div style='float:{side};clear:{clear};width:{w}px;height:{h}px;margin:{mv}px {mh}px'></div>"
                ));
            }
            3..=4 => {
                let words = 4 + rng.below(30);
                let text = sentence(&mut rng, words);
                let margin = rng.below(14);
                body.push_str(&format!("<p style='margin:{margin}px 0'>{text}</p>"));
            }
            5 => {
                let clear = rng.pick(&["left", "right", "both"]);
                let (h, m) = (rng.below(12), rng.below(20));
                body.push_str(&format!(
                    "<div style='clear:{clear};height:{h}px;margin:{m}px 0'></div>"
                ));
            }
            6 => {
                let words = 3 + rng.below(12);
                let text = sentence(&mut rng, words);
                let margin = rng.below(10);
                body.push_str(&format!(
                    "<div style='overflow:hidden;margin:{margin}px 0'>{text}</div>"
                ));
            }
            _ => {
                let side = rng.pick(&["left", "right"]);
                let words = 6 + rng.below(20);
                let text = sentence(&mut rng, words);
                let lead = sentence(&mut rng, 3);
                let (w, h) = (20 + rng.below(80), 8 + rng.below(40));
                body.push_str(&format!(
                    "<p>{lead} <span style='float:{side};width:{w}px;height:{h}px'></span>{text}</p>"
                ));
            }
        }
    }
    let html = format!(
        "<!doctype html><html><head><style>html,body{{margin:0}}body{{font:16px 'Liberation Sans'}}</style></head><body><div id=p style='width:{width}px'>{body}</div></body></html>"
    );
    (html, width as f32)
}

fn inside_float(laid: &Laid, mut frag: FragId) -> bool {
    while let Some(parent) = laid.frags.parent(frag) {
        if laid.frags.get(parent).flags.contains(FragFlags::FLOAT) {
            return true;
        }
        frag = parent;
    }
    false
}

/// True inside a formatting context root that the floats left almost no room:
/// its words overflow it one per line, and where they end up is not a layout
/// rule.
fn squeezed(laid: &Laid, mut frag: FragId) -> bool {
    loop {
        let f = laid.frags.get(frag);
        if f.flags.contains(FragFlags::ESTABLISHES_BFC) && f.rect.w < 70.0 {
            return true;
        }
        match laid.frags.parent(frag) {
            Some(parent) => frag = parent,
            None => return false,
        }
    }
}

fn margin_box(laid: &Laid, frag: FragId) -> Rect {
    let b = laid.frags.absolute_border_box(frag);
    let m = laid.frags.get(frag).margin;
    Rect::new(
        b.x - m.left,
        b.y - m.top,
        b.w + m.left + m.right,
        b.h + m.top + m.bottom,
    )
}

fn overlap(a: &Rect, b: &Rect) -> bool {
    const SLACK: f32 = 0.02;
    a.w > 0.0
        && a.h > 0.0
        && b.w > 0.0
        && b.h > 0.0
        && a.x < b.right() - SLACK
        && b.x < a.right() - SLACK
        && a.y < b.bottom() - SLACK
        && b.y < a.bottom() - SLACK
}

fn all_frags(laid: &Laid) -> impl Iterator<Item = FragId> {
    (0..laid.frags.len() as u32).map(FragId)
}

#[test]
fn floats_never_overlap_each_other() {
    for seed in 1..=150 {
        let (html, _) = scene(seed);
        let laid = lay_out(&html, (800.0, 600.0));
        let floats: Vec<(FragId, Rect)> = all_frags(&laid)
            .filter(|&f| laid.frags.get(f).flags.contains(FragFlags::FLOAT))
            .map(|f| (f, margin_box(&laid, f)))
            .collect();
        for (i, (_, a)) in floats.iter().enumerate() {
            for (_, b) in &floats[i + 1..] {
                assert!(
                    !overlap(a, b),
                    "seed {seed}: floats {a:?} and {b:?} overlap\n{html}"
                );
            }
        }
    }
}

#[test]
fn text_and_context_roots_never_intersect_floats() {
    for seed in 1..=150 {
        let (html, _) = scene(seed);
        let laid = lay_out(&html, (800.0, 600.0));
        let floats: Vec<Rect> = all_frags(&laid)
            .filter(|&f| laid.frags.get(f).flags.contains(FragFlags::FLOAT))
            .map(|f| margin_box(&laid, f))
            .collect();
        for frag in all_frags(&laid) {
            let f = laid.frags.get(frag);
            let text = matches!(f.kind, FragmentKind::Text(_));
            let root = f.flags.contains(FragFlags::ESTABLISHES_BFC)
                && !f.flags.contains(FragFlags::FLOAT)
                && laid.frags.parent(frag).is_some_and(|p| {
                    laid.frags
                        .parent(p)
                        .is_some_and(|pp| laid.frags.parent(pp).is_some())
                });
            if !(text || root) || inside_float(&laid, frag) || squeezed(&laid, frag) {
                continue;
            }
            let rect = laid.frags.absolute_border_box(frag);
            for float in &floats {
                assert!(
                    !overlap(&rect, float),
                    "seed {seed}: {:?} fragment {rect:?} meets float {float:?}\n{html}",
                    if text { "text" } else { "root" }
                );
            }
        }
    }
}

#[test]
fn floats_stay_inside_their_container_when_they_fit() {
    for seed in 1..=150 {
        let (html, width) = scene(seed);
        let laid = lay_out(&html, (800.0, 600.0));
        let container = laid.rect("p");
        for frag in all_frags(&laid) {
            if !laid.frags.get(frag).flags.contains(FragFlags::FLOAT) {
                continue;
            }
            let b = margin_box(&laid, frag);
            if b.w > width {
                continue;
            }
            assert!(
                b.x >= container.x - 0.02 && b.right() <= container.right() + 0.02,
                "seed {seed}: float {b:?} leaves container {container:?}\n{html}"
            );
        }
    }
}

#[test]
fn floats_are_never_higher_than_an_earlier_float() {
    for seed in 1..=150 {
        let (html, _) = scene(seed);
        let laid = lay_out(&html, (800.0, 600.0));
        let mut top = f32::NEG_INFINITY;
        // Fragments are pushed in layout order, which is source order.
        for frag in all_frags(&laid) {
            if !laid.frags.get(frag).flags.contains(FragFlags::FLOAT) {
                continue;
            }
            let b = margin_box(&laid, frag);
            assert!(
                b.y >= top - 0.02,
                "seed {seed}: float {b:?} above an earlier one\n{html}"
            );
            top = top.max(b.y);
        }
    }
}

#[test]
fn layout_with_floats_is_deterministic() {
    for seed in [3, 17, 42] {
        let (html, _) = scene(seed);
        let a = lay_out(&html, (800.0, 600.0));
        let b = lay_out(&html, (800.0, 600.0));
        assert_eq!(a.frags.len(), b.frags.len(), "seed {seed}");
        for frag in all_frags(&a) {
            assert_eq!(
                a.frags.get(frag).rect,
                b.frags.get(frag).rect,
                "seed {seed}"
            );
        }
    }
}

#[test]
fn cleared_blocks_start_below_the_floats_they_clear() {
    for (id, side) in [("l", "left"), ("r", "right"), ("b", "both")] {
        let html = format!(
            "<!doctype html><html><head><style>html,body{{margin:0}}</style></head><body><div style='float:left;width:50px;height:40px'></div><div style='float:right;width:50px;height:70px'></div><div id={id} style='clear:{side};height:5px'></div></body></html>"
        );
        let laid = lay_out(&html, (800.0, 600.0));
        let expected = match side {
            "left" => 40.0,
            "right" => 70.0,
            _ => 70.0,
        };
        assert_eq!(laid.rect(id).y, expected, "clear: {side}");
    }
}
