//! CSS table layout against Chrome, plus structural invariants.
//!
//! The expectations in `tables/chrome_cases.rs` are `getBoundingClientRect`
//! values recorded from headless Chrome 154 for exactly the HTML next to them
//! (`render-fixtures/tools/table_cases.py`, recorded with `record_geometry.py`),
//! with the bundled Liberation Sans declared through `@font-face`. The cases
//! are grouped by name prefix: `bs_` structure and attributes, `an_` anonymous
//! boxes, `cw_` column widths, `tw_` table width and contexts, `fx_` fixed
//! layout, `rh_` row heights and alignment, `bc_` collapsed borders, `cp_`
//! captions, `ct_` tables among other layouts, `rw_` shapes taken from real
//! pages (Wikipedia infoboxes, the periodic table, Hacker News).

// `FontSystem::new_embedded` exists only with the bundled fonts.
#![cfg(feature = "embedded-fonts")]

#[path = "tables/chrome_cases.rs"]
mod chrome_cases;
mod support;

use support::{Case, Laid, lay_out, rect_mismatch};
use tenta_layout::{FragId, Rect};

/// What `getBoundingClientRect` reports: the union of every fragment of the
/// element, so an inline box split over several lines covers all of them.
fn client_rect(laid: &Laid, id: &str) -> Rect {
    let node = laid.node(id);
    let mut union: Option<Rect> = None;
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
fn structure_and_attributes_match_chrome() {
    check_prefix("bs_");
}

#[test]
fn anonymous_table_boxes_match_chrome() {
    check_prefix("an_");
}

#[test]
fn column_widths_match_chrome() {
    check_prefix("cw_");
}

#[test]
fn table_widths_and_contexts_match_chrome() {
    check_prefix("tw_");
}

#[test]
fn fixed_layout_matches_chrome() {
    check_prefix("fx_");
}

#[test]
fn row_heights_and_alignment_match_chrome() {
    check_prefix("rh_");
}

#[test]
fn collapsed_borders_match_chrome() {
    check_prefix("bc_");
}

#[test]
fn captions_match_chrome() {
    check_prefix("cp_");
}

#[test]
fn tables_among_other_layouts_match_chrome() {
    check_prefix("ct_");
}

#[test]
fn shapes_from_real_pages_match_chrome() {
    check_prefix("rw_");
}

// ------------------------------------------------------- anonymous boxes

use tenta_layout::{BoxId, BoxKind, BoxTree, TableKind};

fn table_kind(tree: &BoxTree, id: BoxId) -> Option<TableKind> {
    match tree.get(id).kind {
        BoxKind::Table(kind) => Some(kind),
        _ => None,
    }
}

/// The principal box of element `id`.
fn box_of(laid: &Laid, id: &str) -> BoxId {
    laid.tree.boxes_of(laid.node(id))[0]
}

fn children(laid: &Laid, id: BoxId) -> Vec<BoxId> {
    laid.tree.get(id).children.to_vec()
}

#[test]
fn loose_cells_get_an_anonymous_table_group_and_row() {
    let laid = lay_out(
        "<!doctype html><div id=p><div id=a style='display:table-cell'>x</div>\
         <div id=b style='display:table-cell'>y</div></div>",
        (400.0, 300.0),
    );
    let table = children(&laid, box_of(&laid, "p"))[0];
    assert_eq!(table_kind(&laid.tree, table), Some(TableKind::Table));
    assert!(
        !laid.tree.get(table).node.is_some(),
        "the table is anonymous"
    );
    let group = children(&laid, table)[0];
    assert_eq!(table_kind(&laid.tree, group), Some(TableKind::RowGroup));
    let row = children(&laid, group)[0];
    assert_eq!(table_kind(&laid.tree, row), Some(TableKind::Row));
    assert_eq!(
        children(&laid, row),
        vec![box_of(&laid, "a"), box_of(&laid, "b")]
    );
}

#[test]
fn loose_text_in_a_row_goes_into_an_anonymous_cell() {
    let laid = lay_out(
        "<!doctype html><div style='display:table'><div id=r style='display:table-row'>\
         text<div id=a style='display:table-cell'>x</div>more</div></div>",
        (400.0, 300.0),
    );
    let kids = children(&laid, box_of(&laid, "r"));
    assert_eq!(kids.len(), 3);
    for (index, kid) in kids.iter().enumerate() {
        assert_eq!(table_kind(&laid.tree, *kid), Some(TableKind::Cell));
        assert_eq!(laid.tree.get(*kid).node.is_some(), index == 1);
    }
}

#[test]
fn white_space_between_table_parts_makes_no_box() {
    let laid = lay_out(
        "<!doctype html><div id=t style='display:table'> <div id=r style='display:table-row'> \
         <div id=a style='display:table-cell'>x</div> </div> </div>",
        (400.0, 300.0),
    );
    let group = children(&laid, box_of(&laid, "t"));
    assert_eq!(
        group.len(),
        1,
        "one anonymous row group, no wrappers for blanks"
    );
    assert_eq!(children(&laid, group[0]), vec![box_of(&laid, "r")]);
    assert_eq!(
        children(&laid, box_of(&laid, "r")),
        vec![box_of(&laid, "a")]
    );
}

#[test]
fn a_float_inside_a_table_part_is_content_that_needs_a_cell() {
    let laid = lay_out(
        "<!doctype html><div id=t style='display:table'>\
         <div id=f style='float:left;width:30px;height:20px'></div></div>",
        (400.0, 300.0),
    );
    assert_eq!(laid.rect("f").w, 30.0);
    assert_eq!(laid.rect("t").h, 20.0);
}

#[test]
fn header_and_footer_groups_move_to_the_ends() {
    let laid = lay_out(
        "<!doctype html><style>td{padding:0}table{border-spacing:0}</style>\
         <table><tfoot><tr><td id=f>f</td></tr></tfoot><tbody><tr><td id=b>b</td></tr></tbody>\
         <thead><tr><td id=h>h</td></tr></thead></table>",
        (400.0, 300.0),
    );
    assert!(laid.rect("h").y < laid.rect("b").y);
    assert!(laid.rect("b").y < laid.rect("f").y);
}

// --------------------------------------------------------- robustness

#[test]
fn absurd_spans_neither_hang_nor_blow_up() {
    let started = std::time::Instant::now();
    let laid = lay_out(
        "<!doctype html><table id=t><tr><td id=a colspan=4294967295 rowspan=4294967295>x</td>\
         <td id=b colspan=999999>y</td></tr><tr><td id=c rowspan=0>z</td></tr></table>",
        (800.0, 600.0),
    );
    let t = laid.rect("t");
    for value in [t.x, t.y, t.w, t.h] {
        assert!(value.is_finite());
    }
    assert!(laid.rect("a").w > 0.0);
    assert!(started.elapsed().as_secs() < 20);
}

#[test]
fn a_thousand_rows_of_wide_cells_stay_finite() {
    let mut html = String::from("<!doctype html><table id=t>");
    for row in 0..1000 {
        html.push_str(&format!(
            "<tr><td colspan=1000>row {row}</td><td>x</td></tr>"
        ));
    }
    html.push_str("</table>");
    let laid = lay_out(&html, (800.0, 600.0));
    assert!(laid.rect("t").w.is_finite());
    assert!(laid.rect("t").h > 1000.0);
}

#[test]
fn deeply_nested_tables_lay_out() {
    let depth = 40;
    let mut html = String::from("<!doctype html>");
    for _ in 0..depth {
        html.push_str("<table><tr><td>");
    }
    html.push_str("<span id=leaf>deep</span>");
    for _ in 0..depth {
        html.push_str("</td></tr></table>");
    }
    let laid = lay_out(&html, (800.0, 600.0));
    assert!(laid.rect("leaf").w > 0.0);
}

#[test]
fn floated_children_of_a_flex_container_are_items() {
    let laid = lay_out(
        "<!doctype html><style>body{margin:0}</style>\
         <div id=r style='display:flex;width:300px'>\
         <div id=a style='float:left;width:100px;height:50px'></div>\
         <div id=b style='float:left;width:80px;height:40px'></div></div>",
        (800.0, 600.0),
    );
    assert_eq!(laid.rect("a").w, 100.0);
    assert_eq!(laid.rect("b").x, 100.0);
    assert_eq!(laid.rect("r").h, 50.0);
}

// ---------------------------------------------------------- invariants

/// A small deterministic generator, so a failing seed can be replayed.
struct Rng(u64);

impl Rng {
    fn next(&mut self, below: u32) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as u32) % below
    }

    fn chance(&mut self, percent: u32) -> bool {
        self.next(100) < percent
    }
}

/// A random table with spans, widths, borders and alignment, and the ids of
/// its cells.
fn random_table(seed: u64) -> (String, usize) {
    let mut rng = Rng(seed);
    let rows = 1 + rng.next(6);
    let cols = 1 + rng.next(6);
    let collapse = rng.chance(40);
    let fixed = rng.chance(25);
    let mut style = format!("border-spacing:{}px {}px;", rng.next(6), rng.next(6));
    if collapse {
        style += "border-collapse:collapse;";
    }
    if fixed {
        style += "table-layout:fixed;width:300px;";
    } else if rng.chance(30) {
        style += &format!("width:{}px;", 100 + rng.next(400));
    } else if rng.chance(20) {
        style += &format!("width:{}%;", 20 + rng.next(80));
    }
    if rng.chance(20) {
        style += &format!("height:{}px;", 50 + rng.next(300));
    }
    let mut html = format!(
        "<!doctype html><style>body{{margin:0;font:16px 'Liberation Sans'}}\
         td{{padding:{}px;border:{}px solid #888}}</style>\
         <div style='width:{}px'><table id=t style='{style}'>",
        rng.next(8),
        rng.next(4),
        150 + rng.next(500),
    );
    let (rows, cols) = (rows as usize, cols as usize);
    // Slots a spanning cell already covers, so cells never overlap.
    let mut taken = vec![vec![false; cols]; rows];
    let mut cells = 0;
    for r in 0..rows {
        html.push_str("<tr>");
        let mut c = 0;
        while c < cols {
            if taken[r][c] {
                c += 1;
                continue;
            }
            if rng.chance(10) {
                break;
            }
            let mut colspan = 1;
            if rng.chance(25) {
                let want = 1 + rng.next(3) as usize;
                while colspan < want && c + colspan < cols && !taken[r][c + colspan] {
                    colspan += 1;
                }
            }
            let mut rowspan = 1;
            if rng.chance(25) {
                let want = 1 + rng.next(3) as usize;
                while rowspan < want
                    && r + rowspan < rows
                    && (c..c + colspan).all(|x| !taken[r + rowspan][x])
                {
                    rowspan += 1;
                }
            }
            for row in taken.iter_mut().skip(r).take(rowspan) {
                for slot in row.iter_mut().skip(c).take(colspan) {
                    *slot = true;
                }
            }
            let mut attrs = String::new();
            if colspan > 1 {
                attrs += &format!(" colspan={colspan}");
            }
            if rowspan > 1 {
                attrs += &format!(" rowspan={rowspan}");
            }
            let mut css = String::new();
            if rng.chance(20) {
                css += &format!("width:{}px;", 20 + rng.next(120));
            } else if rng.chance(10) {
                css += &format!("width:{}%;", 5 + rng.next(40));
            }
            if rng.chance(15) {
                css += &format!(
                    "vertical-align:{};",
                    ["top", "bottom", "baseline"][rng.next(3) as usize]
                );
            }
            if !css.is_empty() {
                attrs += &format!(" style='{css}'");
            }
            let words = 1 + rng.next(8) as usize;
            let text = ["lorem", "ipsum", "dolor", "sit", "amet"]
                .iter()
                .cycle()
                .take(words)
                .copied()
                .collect::<Vec<_>>()
                .join(" ");
            html.push_str(&format!("<td id=c{cells}{attrs}>{text}</td>"));
            cells += 1;
            c += colspan;
        }
        html.push_str("</tr>");
    }
    html.push_str("</table></div>");
    (html, cells)
}

#[test]
fn random_tables_keep_their_cells_apart_and_inside() {
    for seed in 0..150u64 {
        let (html, cells) = random_table(seed);
        let laid = lay_out(&html, (800.0, 600.0));
        let table = laid.rect("t");
        let rects: Vec<Rect> = (0..cells).map(|i| laid.rect(&format!("c{i}"))).collect();
        for (i, r) in rects.iter().enumerate() {
            assert!(
                [r.x, r.y, r.w, r.h]
                    .iter()
                    .all(|v| v.is_finite() && *v >= -0.01),
                "seed {seed}: cell {i} has a bad rect {r:?}"
            );
            assert!(
                r.x >= table.x - 0.01
                    && r.y >= table.y - 0.01
                    && r.right() <= table.right() + 0.01
                    && r.bottom() <= table.bottom() + 0.01,
                "seed {seed}: cell {i} {r:?} leaves the table {table:?}\n{html}"
            );
        }
        for i in 0..rects.len() {
            for j in i + 1..rects.len() {
                let (a, b) = (rects[i], rects[j]);
                let overlap_w = a.right().min(b.right()) - a.x.max(b.x);
                let overlap_h = a.bottom().min(b.bottom()) - a.y.max(b.y);
                assert!(
                    overlap_w <= 0.02 || overlap_h <= 0.02,
                    "seed {seed}: cells {i} and {j} overlap: {a:?} {b:?}\n{html}"
                );
            }
        }
    }
}

#[test]
fn random_tables_lay_out_the_same_way_twice() {
    for seed in 0..60u64 {
        let (html, cells) = random_table(seed + 1000);
        let first = lay_out(&html, (800.0, 600.0));
        let second = lay_out(&html, (800.0, 600.0));
        for i in 0..cells {
            let id = format!("c{i}");
            assert_eq!(first.rect(&id), second.rect(&id), "seed {seed}: {id}");
        }
        assert_eq!(first.rect("t"), second.rect("t"), "seed {seed}");
    }
}
