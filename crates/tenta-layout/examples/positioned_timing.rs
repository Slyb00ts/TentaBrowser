//! Times `layout()` on synthetic pages of 1,000 absolutely positioned boxes.
//!
//! Run: `cargo run --release -p tenta-layout --example positioned_timing [runs]`.
//! Prints p50/p95 of the layout step alone (style and box-tree construction
//! are done once, outside the timed region) for 1,000 boxes with insets and a
//! size in one positioned container, the same boxes in normal flow as a
//! baseline, shrink-to-fit text boxes, boxes at their static position between
//! text lines, boxes that each have a containing block of their own, and a
//! chain of 100 nested positioned boxes.

#[cfg(feature = "embedded-fonts")]
fn main() {
    let runs: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(30);
    report("flow baseline: 1,000 blocks", &flow_page(), runs);
    report("abs: 1,000 with insets and size", &inset_page(), runs);
    report("abs: 1,000 shrink-to-fit text", &text_page(), runs);
    report("abs: 1,000 static position in lines", &static_page(), runs);
    report("abs: 1,000 own containing blocks", &own_cb_page(), runs);
    report("abs: chain of 100 nested", &chain_page(), runs);
}

#[cfg(feature = "embedded-fonts")]
const VIEWPORT: (f32, f32) = (900.0, 1000.0);

#[cfg(feature = "embedded-fonts")]
const STYLE: &str = "<!doctype html><style>body{margin:0;font:14px 'Liberation Sans'}\
     .c{position:relative;height:800px}.a{position:absolute;width:40px;height:12px;background:#ccc}\
     .t{position:absolute}.f{height:12px;width:40px}.w{position:relative;height:14px}</style><body>";

#[cfg(feature = "embedded-fonts")]
fn flow_page() -> String {
    let mut html = String::from(STYLE);
    html.push_str("<div class=c>");
    for _ in 0..1000 {
        html.push_str("<div class=f></div>");
    }
    html.push_str("</div>");
    html
}

#[cfg(feature = "embedded-fonts")]
fn inset_page() -> String {
    let mut html = String::from(STYLE);
    html.push_str("<div class=c>");
    for i in 0..1000 {
        html.push_str(&format!(
            "<div class=a style=\"left:{}px;top:{}px\"></div>",
            (i % 20) * 44,
            (i / 20) * 14
        ));
    }
    html.push_str("</div>");
    html
}

#[cfg(feature = "embedded-fonts")]
fn text_page() -> String {
    let mut html = String::from(STYLE);
    html.push_str("<div class=c>");
    for i in 0..1000 {
        html.push_str(&format!(
            "<div class=t style=\"left:{}px;top:{}px\">cell {i}</div>",
            (i % 10) * 88,
            (i / 10) * 14
        ));
    }
    html.push_str("</div>");
    html
}

#[cfg(feature = "embedded-fonts")]
fn static_page() -> String {
    let mut html = String::from(STYLE);
    html.push_str("<div class=c>");
    for i in 0..1000 {
        html.push_str(&format!(
            "<p style=\"margin:0\">line {i} <span class=a style=\"display:inline\"></span></p>"
        ));
    }
    html.push_str("</div>");
    html
}

#[cfg(feature = "embedded-fonts")]
fn own_cb_page() -> String {
    let mut html = String::from(STYLE);
    for _ in 0..1000 {
        html.push_str("<div class=w><div class=a style=\"right:0;bottom:0\"></div></div>");
    }
    html
}

#[cfg(feature = "embedded-fonts")]
fn chain_page() -> String {
    let mut html = String::from(STYLE);
    html.push_str("<style>.n{position:absolute;left:2px;top:2px;right:2px;bottom:2px}</style>");
    html.push_str("<div style=\"position:relative;width:600px;height:600px\">");
    for _ in 0..100 {
        html.push_str("<div class=n>");
    }
    for _ in 0..100 {
        html.push_str("</div>");
    }
    html.push_str("</div>");
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
