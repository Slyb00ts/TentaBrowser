//! Times `layout()` on synthetic flex and grid pages of about 1,000 to 2,000 items.
//!
//! Run: `cargo run --release -p tenta-layout --example flex_timing [runs]`.
//! Prints p50/p95 of the layout step alone (style and box-tree construction
//! are done once, outside the timed region) for a wrapping container holding
//! 2,000 text items, 2,000 rows of three items, and flex containers nested
//! sixteen levels deep with a block between each pair, and binary trees of
//! nested flex containers, plus 1,000-item grids (six fixed columns, auto-fill
//! with `minmax`, and dense placement with spans), and nested grids (binary
//! trees and a three-level 12x12x6 page, which exercise the per-level re-solve).

#[cfg(feature = "embedded-fonts")]
fn main() {
    let runs: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(30);
    report("wrap: 2,000 text items", &wrap_page(), runs);
    report("rows: 2,000 rows of 3 items", &rows_page(), runs);
    report("nested: 16 levels", &nested_page(), runs);
    report("grid: 1,000 items, 6 columns", &grid_page(GRID_FIXED), runs);
    report(
        "grid: 1,000 items, auto-fill",
        &grid_page(GRID_AUTO_FILL),
        runs,
    );
    report(
        "grid: 1,000 items, dense spans",
        &grid_page(GRID_DENSE),
        runs,
    );
    for depth in [6, 8, 10] {
        report(
            &format!("grid tree: binary depth {depth}"),
            &grid_tree_page(depth),
            runs,
        );
    }
    report(
        "grid nested 12x12x6, fixed columns",
        &grid_nested_page("repeat(4,1fr)"),
        runs,
    );
    report(
        "grid nested 12x12x6, auto-fill",
        &grid_nested_page("repeat(auto-fill,minmax(20px,1fr))"),
        runs,
    );
    for depth in [6, 8, 10] {
        report(
            &format!("tree: binary depth {depth}"),
            &tree_page(depth),
            runs,
        );
    }
}

#[cfg(feature = "embedded-fonts")]
const VIEWPORT: (f32, f32) = (900.0, 1000.0);

#[cfg(feature = "embedded-fonts")]
const STYLE: &str = "<!doctype html><style>body{margin:0;font:14px 'Liberation Sans'}\
     .w{display:flex;flex-wrap:wrap;gap:4px}.w>div{flex:1 1 80px;padding:2px;border:1px solid}\
     .r{display:flex;align-items:center;gap:6px}.r>.a{width:40px}.r>.b{flex:1}.r>.c{padding:0 4px}\
     .n{display:flex}.n>.m{flex:1;padding:1px}</style><body>";

#[cfg(feature = "embedded-fonts")]
const GRID_FIXED: &str = "grid-template-columns:repeat(6,1fr);gap:4px";
#[cfg(feature = "embedded-fonts")]
const GRID_AUTO_FILL: &str = "grid-template-columns:repeat(auto-fill,minmax(90px,1fr));gap:4px";
#[cfg(feature = "embedded-fonts")]
const GRID_DENSE: &str = "grid-template-columns:repeat(8,1fr);grid-auto-flow:row dense;gap:4px";

/// One grid container holding 1,000 items with the given container style;
/// every seventh item spans two columns.
#[cfg(feature = "embedded-fonts")]
fn grid_page(container: &str) -> String {
    let mut html = String::from(STYLE);
    html.push_str(&format!("<div style=\"display:grid;{container}\">"));
    for i in 0..1000 {
        let span = if i % 7 == 0 {
            " style=\"grid-column:span 2\""
        } else {
            ""
        };
        html.push_str(&format!("<div{span}>cell {i} text</div>"));
    }
    html.push_str("</div>");
    html
}

/// Binary tree of nested grids with two columns per level.
#[cfg(feature = "embedded-fonts")]
fn grid_tree_page(depth: u32) -> String {
    fn node(html: &mut String, depth: u32) {
        if depth == 0 {
            html.push_str("leaf");
            return;
        }
        html.push_str("<div class=gn>");
        for _ in 0..2 {
            html.push_str("<div>");
            node(html, depth - 1);
            html.push_str("</div>");
        }
        html.push_str("</div>");
    }
    let mut html = String::from(STYLE);
    html.push_str("<style>.gn{display:grid;grid-template-columns:1fr 1fr}</style>");
    node(&mut html, depth);
    html
}

/// A grid of 12 grids of 12 grids of 6 cells (1,023 boxes of grid and item).
#[cfg(feature = "embedded-fonts")]
fn grid_nested_page(columns: &str) -> String {
    let mut html = String::from(STYLE);
    html.push_str(&format!(
        "<style>.g{{display:grid;grid-template-columns:{columns}}}</style><div class=g>"
    ));
    for _ in 0..12 {
        html.push_str("<div class=g>");
        for _ in 0..12 {
            html.push_str("<div class=g>");
            for c in 0..6 {
                html.push_str(&format!("<div>c{c}</div>"));
            }
            html.push_str("</div>");
        }
        html.push_str("</div>");
    }
    html.push_str("</div>");
    html
}

#[cfg(feature = "embedded-fonts")]
fn wrap_page() -> String {
    let mut html = String::from(STYLE);
    html.push_str("<div class=w>");
    for i in 0..2000 {
        html.push_str(&format!("<div>item {i} text</div>"));
    }
    html.push_str("</div>");
    html
}

#[cfg(feature = "embedded-fonts")]
fn rows_page() -> String {
    let mut html = String::from(STYLE);
    for i in 0..2000 {
        html.push_str(&format!(
            "<div class=r><div class=a>#{i}</div><div class=b>row label {i}</div><div class=c>go</div></div>"
        ));
    }
    html
}

#[cfg(feature = "embedded-fonts")]
fn nested_page() -> String {
    let mut html = String::from(STYLE);
    for _ in 0..16 {
        html.push_str("<div class=n><div class=m>");
    }
    html.push_str("leaf text");
    for _ in 0..16 {
        html.push_str("</div></div>");
    }
    html
}

#[cfg(feature = "embedded-fonts")]
fn tree_page(depth: u32) -> String {
    fn node(html: &mut String, depth: u32) {
        if depth == 0 {
            html.push_str("leaf");
            return;
        }
        html.push_str("<div class=n>");
        for _ in 0..2 {
            html.push_str("<div class=m>");
            node(html, depth - 1);
            html.push_str("</div>");
        }
        html.push_str("</div>");
    }
    let mut html = String::from(STYLE);
    node(&mut html, depth);
    html
}

#[cfg(feature = "embedded-fonts")]
fn report(name: &str, html: &str, runs: usize) {
    use std::time::Instant;

    use tenta_css::cascade::StyleEnv;
    use tenta_css::cascade::test_util::style_html;
    use tenta_layout::{BoxTree, FontSystem, LayoutCx, NoImages, layout};

    let (doc, engine) = style_html(
        html,
        StyleEnv {
            viewport: VIEWPORT,
            ..StyleEnv::default()
        },
    );
    let mut fonts = FontSystem::new_embedded();
    let mut cx = LayoutCx {
        doc: &doc,
        styles: &engine,
        fonts: &mut fonts,
        images: &NoImages,
        viewport: VIEWPORT,
        quirks: false,
    };
    let tree = BoxTree::build(&cx);

    let mut samples = Vec::with_capacity(runs);
    let mut fragments = 0;
    for _ in 0..runs {
        let start = Instant::now();
        let frags = layout(&mut cx, &tree);
        samples.push(start.elapsed().as_secs_f64() * 1e3);
        fragments = frags.len();
    }
    // The first run shapes every distinct string; later runs hit the chunk cache.
    let cold = samples[0];
    samples.sort_by(f64::total_cmp);
    let at = |q: f64| samples[((samples.len() - 1) as f64 * q).round() as usize];
    println!(
        "{name}: boxes={} fragments={fragments}; cold={cold:.3} ms; layout over {runs} runs: p50={:.3} ms p95={:.3} ms min={:.3} ms",
        tree.len(),
        at(0.5),
        at(0.95),
        samples[0]
    );
}

#[cfg(not(feature = "embedded-fonts"))]
fn main() {
    eprintln!("this example needs the `embedded-fonts` feature");
}
