//! Times `layout()` on pages of 500 text paragraphs with and without floats.
//!
//! Run: `cargo run --release -p tenta-layout --example float_timing [runs]`.
//! Prints the cold first layout and the p50/p95 of the following ones for the
//! same text with no floats, with one float inside every paragraph, with one
//! float between every two paragraphs, and with 500 floats in one column
//! (every lookup scans a long history).

#[cfg(feature = "embedded-fonts")]
const VIEWPORT: (f32, f32) = (900.0, 1000.0);

#[cfg(feature = "embedded-fonts")]
fn main() {
    let runs: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(30);
    let text = "The quick brown fox jumps over the lazy dog while the committee reviews \
                the quarterly figures and the weather turns colder over the hills";
    let page = |each: &dyn Fn(usize) -> (String, String)| {
        let mut html = String::from(
            "<!doctype html><style>body{margin:0;font:16px 'Liberation Sans'}p{margin:8px 0}\
             .f{width:60px;height:40px;margin:4px}.l{float:left}.r{float:right}</style><body>",
        );
        for i in 0..500 {
            let (before, inside) = each(i);
            html.push_str(&before);
            html.push_str(&format!("<p>{inside}{text} {i}</p>"));
        }
        html
    };
    let side = |i: usize| if i.is_multiple_of(2) { "l" } else { "r" };
    // The first page fills the shaping cache; its numbers are not comparable.
    let plain = page(&|_| (String::new(), String::new()));
    report("warm-up (cold shaping cache)", &plain, 3);
    report("500 paragraphs, no floats", &plain, runs);
    report(
        "500 paragraphs, a float in each",
        &page(&|i| {
            (
                String::new(),
                format!("<span class='f {}'></span>", side(i)),
            )
        }),
        runs,
    );
    report(
        "500 paragraphs, a float between each",
        &page(&|i| (format!("<div class='f {}'></div>", side(i)), String::new())),
        runs,
    );
    let mut column = String::from(
        "<!doctype html><style>body{margin:0;font:16px 'Liberation Sans'}\
         .f{float:left;clear:left;width:60px;height:10px}</style><body>",
    );
    for _ in 0..500 {
        column.push_str("<div class=f></div>");
    }
    report("500 floats stacked in one column", &column, runs);
    // Every level's first child is a margined formatting context beside a
    // tall float: the shape that made mispredicted positions multiply layouts.
    for depth in [16, 24, 64] {
        let mut nested = String::from(
            "<!doctype html><style>body{margin:0}.f{float:left;width:10px;height:10px}</style><body>\
             <div style='float:left;width:50px;height:400px'></div><div style='height:1px'></div>",
        );
        for _ in 0..depth {
            nested.push_str("<div><div style='overflow:hidden;margin-top:6px'><div class=f></div>");
        }
        nested.push_str("<p>text</p>");
        nested.push_str(&"</div></div>".repeat(depth));
        report(
            &format!("adversarial nesting, depth {depth}"),
            &nested,
            runs.min(10),
        );
    }
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
    let mut cold = 0.0;
    let mut fragments = 0;
    for run in 0..=runs {
        let start = Instant::now();
        let frags = layout(&mut cx, &tree);
        let ms = start.elapsed().as_secs_f64() * 1e3;
        fragments = frags.len();
        if run == 0 {
            cold = ms;
        } else {
            samples.push(ms);
        }
    }
    samples.sort_by(f64::total_cmp);
    let at = |q: f64| samples[((samples.len() - 1) as f64 * q).round() as usize];
    println!(
        "{name}: boxes={} fragments={fragments} cold={cold:.2} ms; warm over {runs} runs: p50={:.3} ms p95={:.3} ms",
        tree.len(),
        at(0.5),
        at(0.95),
    );
}

#[cfg(not(feature = "embedded-fonts"))]
fn main() {
    eprintln!("this example needs the `embedded-fonts` feature");
}
