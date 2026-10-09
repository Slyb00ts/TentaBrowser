//! Times `layout()` on a synthetic page of about 5,000 block boxes.
//!
//! Run: `cargo run --release -p tenta-layout --example block_timing [runs]`.
//! For a page without text and one with a short text line in every card,
//! prints p50/p95 of the layout step alone (style and box-tree construction
//! are done once, outside the timed region) and the fragment count.

#[cfg(feature = "embedded-fonts")]
fn main() {
    let runs: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(30);
    for (name, text) in [("blocks only", ""), ("with text", "card text")] {
        report(name, &page(text), runs);
    }
}

#[cfg(feature = "embedded-fonts")]
const VIEWPORT: (f32, f32) = (900.0, 1000.0);

/// 100 sections of 16 cards (card, inner box, spacer) plus one spacer each:
/// 5,000 boxes with collapsing margins, padding, borders and percentage widths.
#[cfg(feature = "embedded-fonts")]
fn page(text: &str) -> String {
    let mut html = String::from(
        "<!doctype html><style>body{margin:0}section{margin:8px 0;padding:2px}\
         .card{margin:6px 10px;width:90%}.inner{margin:3px 0;border:1px solid;padding:2px}\
         .gap{margin:4px 0}</style><body>",
    );
    for _ in 0..100 {
        html.push_str("<section>");
        for _ in 0..16 {
            html.push_str("<div class=card><div class=inner>");
            html.push_str(text);
            html.push_str("</div></div><div class=gap></div>");
        }
        html.push_str("<div class=gap></div></section>");
    }
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
    let build_start = Instant::now();
    let tree = BoxTree::build(&cx);
    let build_ms = build_start.elapsed().as_secs_f64() * 1e3;

    let mut samples = Vec::with_capacity(runs);
    let mut fragments = 0;
    for _ in 0..runs {
        let start = Instant::now();
        let frags = layout(&mut cx, &tree);
        samples.push(start.elapsed().as_secs_f64() * 1e3);
        fragments = frags.len();
    }
    samples.sort_by(f64::total_cmp);
    let at = |q: f64| samples[((samples.len() - 1) as f64 * q).round() as usize];
    println!(
        "{name}: boxes={} fragments={fragments} box-tree build={build_ms:.2} ms; layout over {runs} runs: p50={:.3} ms p95={:.3} ms min={:.3} ms",
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
