//! Times `layout()` on text-heavy pages.
//!
//! Run: `cargo run --release -p tenta-layout --example inline_timing [runs]`.
//! Two shapes: 1,000 paragraphs of about 330 characters with a bold and a link
//! (3,002 boxes), and 1,666 short paragraphs with one bold run (about 5,000
//! boxes). Prints the cold first layout and the p50/p95/min of the following
//! ones, then the same pages with distinct text in every run, which the shaping
//! cache has never seen (the cost of a first load). Style and box-tree
//! construction are outside the timed region. Compare two builds by running
//! their binaries alternately.

#[cfg(feature = "embedded-fonts")]
fn main() {
    let runs: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(25);
    report("long paragraphs", long_page, runs);
    report("short paragraphs", short_page, runs);
}

#[cfg(feature = "embedded-fonts")]
const VIEWPORT: (f32, f32) = (900.0, 1000.0);

#[cfg(feature = "embedded-fonts")]
fn long_page(seed: usize) -> String {
    let mut html =
        String::from("<!doctype html><style>body{margin:0}p{margin:4px 8px}</style><body>");
    for i in 0..1000 {
        html.push_str(&format!(
            "<p>para{seed}-{i} The quick brown fox jumps over the lazy dog and keeps running through \
             the <b>forest</b> until it reaches the <a href=x>river bank</a> where many other \
             animals gather to drink water in the evening light. Lorem ipsum dolor sit amet, \
             consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore \
             magna aliqua.</p>"
        ));
    }
    html
}

#[cfg(feature = "embedded-fonts")]
fn short_page(seed: usize) -> String {
    let mut html =
        String::from("<!doctype html><style>body{margin:0}p{margin:2px 8px}</style><body>");
    for i in 0..1666 {
        html.push_str(&format!("<p>item {seed}-{i} <b>bold</b> short text</p>"));
    }
    html
}

#[cfg(feature = "embedded-fonts")]
fn report(name: &str, page: fn(usize) -> String, runs: usize) {
    use std::time::Instant;

    use tenta_css::cascade::StyleEnv;
    use tenta_css::cascade::test_util::style_html;
    use tenta_layout::{BoxTree, FontSystem, LayoutCx, NoImages, layout};

    let summary = |mut samples: Vec<f64>| {
        samples.sort_by(f64::total_cmp);
        let at = |q: f64| samples[((samples.len() - 1) as f64 * q).round() as usize];
        format!(
            "p50={:.2} ms p95={:.2} ms min={:.2} ms",
            at(0.5),
            at(0.95),
            samples[0]
        )
    };
    let mut fonts = FontSystem::new_embedded();
    let mut times = |seed: usize, rounds: usize| {
        let (doc, engine) = style_html(
            &page(seed),
            StyleEnv {
                viewport: VIEWPORT,
                ..StyleEnv::default()
            },
        );
        let mut cx = LayoutCx {
            doc: &doc,
            styles: &engine,
            fonts: &mut fonts,
            images: &NoImages,
            viewport: VIEWPORT,
            quirks: false,
        };
        let tree = BoxTree::build(&cx);
        let mut samples = Vec::with_capacity(rounds);
        let mut fragments = 0;
        for _ in 0..rounds {
            let start = Instant::now();
            let frags = layout(&mut cx, &tree);
            samples.push(start.elapsed().as_secs_f64() * 1e3);
            fragments = frags.len();
        }
        (tree.len(), fragments, samples)
    };
    let (boxes, fragments, mut warm) = times(0, runs + 1);
    let first = warm.remove(0);
    println!(
        "{name}: boxes={boxes} fragments={fragments} first={first:.2} ms; warm over {runs} runs: {}",
        summary(warm)
    );
    let cold: Vec<f64> = (1..=runs).map(|seed| times(seed, 1).2[0]).collect();
    println!(
        "{name}: cold (new text every run) over {runs} runs: {}",
        summary(cold)
    );
}

#[cfg(not(feature = "embedded-fonts"))]
fn main() {
    eprintln!("this example needs the `embedded-fonts` feature");
}
