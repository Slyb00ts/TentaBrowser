//! Times `BoxTree::build` and `layout()` on large and nested tables.
//!
//! Run: `cargo run --release -p tenta-layout --example table_timing [runs]`.
//! Prints, per scenario, the box-tree build time and the cold first layout and
//! the p50/p95 of the following ones.

#[cfg(feature = "embedded-fonts")]
const VIEWPORT: (f32, f32) = (1280.0, 1000.0);

#[cfg(feature = "embedded-fonts")]
fn grid(
    rows: usize,
    cols: usize,
    table_style: &str,
    cell: &dyn Fn(usize, usize) -> String,
) -> String {
    let mut html = format!(
        "<!doctype html><style>body{{margin:0;font:16px 'Liberation Sans'}}\
         td{{padding:2px 4px}}</style><body><table style='{table_style}'>"
    );
    for r in 0..rows {
        html.push_str("<tr>");
        for c in 0..cols {
            html.push_str(&format!("<td{}", cell(r, c)));
        }
        html.push_str("</tr>");
    }
    html.push_str("</table>");
    html
}

/// A table whose first cell holds the next level, `depth` levels deep, each
/// level with `rows` rows of two cells.
#[cfg(feature = "embedded-fonts")]
fn nested(depth: usize, rows: usize) -> String {
    let mut inner = String::from("leaf text that wraps a little when the column is narrow");
    for level in 0..depth {
        let mut table = String::from("<table style='border-spacing:1px'>");
        for r in 0..rows {
            table.push_str("<tr>");
            if r == 0 {
                table.push_str(&format!("<td>{inner}</td>"));
            } else {
                table.push_str(&format!("<td>level {level} row {r}</td>"));
            }
            table.push_str(&format!("<td>cell {level}.{r}</td></tr>"));
        }
        table.push_str("</table>");
        inner = table;
    }
    format!(
        "<!doctype html><style>body{{margin:0;font:16px 'Liberation Sans'}}td{{padding:1px}}</style><body>{inner}"
    )
}

/// A table of one row whose two cells each hold the next level: `2^depth`
/// leaves, the shape that exposes layouts repeated at every level.
#[cfg(feature = "embedded-fonts")]
fn branching(depth: usize) -> String {
    fn level(depth: usize, html: &mut String) {
        if depth == 0 {
            html.push_str("leaf text");
            return;
        }
        html.push_str("<table><tr><td>");
        level(depth - 1, html);
        html.push_str("</td><td>");
        level(depth - 1, html);
        html.push_str("</td></tr></table>");
    }
    let mut html = String::from(
        "<!doctype html><style>body{margin:0;font:16px 'Liberation Sans'}</style><body>",
    );
    level(depth, &mut html);
    html
}

#[cfg(feature = "embedded-fonts")]
fn main() {
    let runs: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(30);
    let text =
        |r: usize, c: usize| format!(">row {r} column {c} value {}</td>", (r * 31 + c * 7) % 1000);
    // The first page fills the shaping cache; its numbers are not comparable.
    let plain = grid(1000, 8, "", &text);
    report("warm-up (cold shaping cache)", &plain, 3);
    report("1000 x 8 table, auto layout", &plain, runs);
    // The same 8000 texts as blocks, to tell the table's share from the text's.
    let mut blocks = String::from(
        "<!doctype html><style>body{margin:0;font:16px 'Liberation Sans'}\
         div{padding:2px 4px}</style><body>",
    );
    for r in 0..1000 {
        for c in 0..8 {
            blocks.push_str(&format!(
                "<div>row {r} column {c} value {}</div>",
                (r * 31 + c * 7) % 1000
            ));
        }
    }
    report("reference: 8000 blocks with the same texts", &blocks, runs);
    report(
        "1000 x 8 table, table-layout: fixed",
        &grid(1000, 8, "table-layout:fixed;width:1200px", &text),
        runs,
    );
    report(
        "1000 x 8 table, border-collapse: collapse",
        &grid(1000, 8, "border-collapse:collapse", &|r, c| {
            format!(" style='border:1px solid #888'>row {r} column {c}</td>")
        }),
        runs,
    );
    report(
        "1000 x 8 table with a colspan and a rowspan per row pair",
        &grid(1000, 8, "", &|r, c| match (r % 2, c) {
            (0, 0) => " rowspan=2>tall</td>".to_owned(),
            (0, 1) => " colspan=2>wide cell</td>".to_owned(),
            (1, 0) => ">second</td>".to_owned(),
            _ => format!(">r{r} c{c}</td>"),
        }),
        runs,
    );
    for depth in [6, 8, 10] {
        report(
            &format!(
                "branching nested tables, depth {depth} ({} leaves)",
                1 << depth
            ),
            &branching(depth),
            runs.min(10),
        );
    }
    for (depth, rows) in [(4, 3), (6, 3), (8, 3), (10, 3)] {
        report(
            &format!("nested tables, depth {depth}, {rows} rows per level"),
            &nested(depth, rows),
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
    let start = Instant::now();
    let tree = BoxTree::build(&cx);
    let build_ms = start.elapsed().as_secs_f64() * 1e3;
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
        "{name}: boxes={} fragments={fragments} build={build_ms:.2} ms; layout cold={cold:.2} ms; warm over {runs} runs: p50={:.3} ms p95={:.3} ms",
        tree.len(),
        at(0.5),
        at(0.95),
    );
}

#[cfg(not(feature = "embedded-fonts"))]
fn main() {
    eprintln!("this example needs the `embedded-fonts` feature");
}
