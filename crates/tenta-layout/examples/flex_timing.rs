//! Times `layout()` on synthetic flex pages of about 2,000 items.
//!
//! Run: `cargo run --release -p tenta-layout --example flex_timing [runs]`.
//! Prints p50/p95 of the layout step alone (style and box-tree construction
//! are done once, outside the timed region) for a wrapping container holding
//! 2,000 text items, 2,000 rows of three items, and flex containers nested
//! sixteen levels deep with a block between each pair, and binary trees of
//! nested flex containers.

#[cfg(feature = "embedded-fonts")]
fn main() {
    let runs: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(30);
    report("wrap: 2,000 text items", &wrap_page(), runs);
    report("rows: 2,000 rows of 3 items", &rows_page(), runs);
    report("nested: 16 levels", &nested_page(), runs);
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
