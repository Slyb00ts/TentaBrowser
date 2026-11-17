//! Render pipeline benchmark subject, driven by `bench/render.py`.
//!
//! ```text
//! cargo build --release -p tenta-render --example render_bench
//! target/release/examples/render_bench --list
//! target/release/examples/render_bench --scenario S-layout-5k [--iterations N]
//! ```
//!
//! One invocation runs one scenario and prints one JSON object per measured
//! iteration on stdout (JSON lines). Stage times are milliseconds and `null`
//! when the scenario does not time that stage. The `cache` field is `cold`
//! only when the text was new to this thread: the chunk cache is per thread
//! and outlives a `RenderSession`, so a new session after the first one is
//! `warm-text`. `warm` marks repeated work on an unchanged or mutated
//! document. For session scenarios only the first record of a process is cold;
//! run one iteration per process (`bench/render.py --fresh-process`) to
//! sample it repeatedly.
//!
//! Sessions measure the product path, `RenderSession::screenshot`, which
//! reports style flush, box tree and layout as one number; the staged
//! scenarios run the same calls by hand so the three can be told apart. Page
//! construction, parsing and RSS sampling are outside every timed region.

use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tenta_core::atom::{Atom, atoms};
use tenta_css::cascade::test_util::style_html;
use tenta_css::cascade::{StyleEngine, StyleEnv, StyleSource};
use tenta_dom::{Document, Mutation};
use tenta_html::QualName;
use tenta_layout::{BoxTree, FontSystem, FragmentTree, LayoutCx, NoImages, layout};
use tenta_paint::{
    Color, DisplayList, ImageStore, PaintContext, PngProfile, RasterOptions, build, encode_png,
    rasterize,
};
use tenta_render::{Capture, RenderConfig, RenderSession, ScreenshotOptions, StageTimes};

const VIEWPORT: (u32, u32) = (1280, 800);

/// Number of `<h2>` sections of the static article that make a typical article
/// page; the whole fixture is a 1 MB stress document.
const TYPICAL_SECTIONS: usize = 9;

/// Sections whose full-page capture is about 12 000 CSS pixels tall, the
/// `S-fullpage` height of the spec.
const FULLPAGE_12000_SECTIONS: usize = 5;

/// One scenario: its name, default iterations per phase, and its runner.
struct Scenario {
    name: &'static str,
    iterations: usize,
    run: fn(usize),
}

const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "S-screenshot-inproc",
        iterations: 10,
        run: screenshot,
    },
    Scenario {
        name: "S-fullpage-inproc",
        iterations: 3,
        run: fullpage_whole_article,
    },
    Scenario {
        name: "S-fullpage-12000",
        iterations: 10,
        run: fullpage_12000,
    },
    Scenario {
        name: "S-fixtures",
        iterations: 10,
        run: fixtures,
    },
    Scenario {
        name: "S-style-10k",
        iterations: 10,
        run: style_10k,
    },
    Scenario {
        name: "S-layout-5k",
        iterations: 10,
        run: layout_5k,
    },
    Scenario {
        name: "S-layout-nested",
        iterations: 10,
        run: layout_nested,
    },
    Scenario {
        name: "S-dl-5k",
        iterations: 25,
        run: display_list_5k,
    },
    Scenario {
        name: "S-raster-1280",
        iterations: 25,
        run: raster_1280,
    },
    Scenario {
        name: "S-font-init",
        iterations: 25,
        run: font_init,
    },
];

fn main() {
    let mut args = std::env::args().skip(1);
    let (mut scenario, mut iterations) = (None, None);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--list" => {
                for s in SCENARIOS {
                    println!("{}", s.name);
                }
                return;
            }
            "--scenario" => scenario = args.next(),
            "--iterations" => iterations = args.next().and_then(|n| n.parse::<usize>().ok()),
            other => usage(&format!("unknown argument {other:?}")),
        }
    }
    let Some(name) = scenario else {
        usage("--scenario is required")
    };
    let Some(found) = SCENARIOS.iter().find(|s| s.name == name) else {
        usage(&format!("unknown scenario {name:?}"))
    };
    let iterations = iterations.unwrap_or(found.iterations);
    if iterations == 0 {
        usage("--iterations must be positive");
    }
    (found.run)(iterations);
}

fn usage(problem: &str) -> ! {
    eprintln!("render_bench: {problem}");
    eprintln!("usage: render_bench --list | --scenario NAME [--iterations N]");
    std::process::exit(2);
}

// ---------------------------------------------------------------------------
// Measurement and output

/// Times of the stages a scenario measured.
#[derive(Default)]
struct Stages {
    style_flush: Option<Duration>,
    box_tree: Option<Duration>,
    layout: Option<Duration>,
    display_list: Option<Duration>,
    raster: Option<Duration>,
    encode: Option<Duration>,
}

impl Stages {
    fn total(&self) -> Duration {
        [
            self.style_flush,
            self.box_tree,
            self.layout,
            self.display_list,
            self.raster,
            self.encode,
        ]
        .into_iter()
        .flatten()
        .sum()
    }
}

/// What one iteration reports besides its stage times.
struct Sample {
    stages: Stages,
    /// Wall time of the whole timed call when it is more than the stages.
    total: Option<Duration>,
    /// Layout covers style flush and box tree construction too.
    layout_covers_style_and_boxes: bool,
    fragments: Option<usize>,
    extra: Value,
}

impl Sample {
    fn staged(stages: Stages, fragments: Option<usize>) -> Self {
        Self {
            stages,
            total: None,
            layout_covers_style_and_boxes: false,
            fragments,
            extra: Value::Null,
        }
    }
}

/// `cold` for the first session capture of the process, `warm-text` after:
/// a new session keeps the per-thread chunk cache.
fn session_cache() -> &'static str {
    static SPENT: AtomicBool = AtomicBool::new(false);
    if SPENT.swap(true, Ordering::Relaxed) {
        "warm-text"
    } else {
        "cold"
    }
}

fn ms(d: Option<Duration>) -> Value {
    d.map_or(Value::Null, |d| json!(d.as_secs_f64() * 1e3))
}

/// Resident set size of this process in KiB, sampled with `ps` so that the
/// example needs neither `unsafe` nor a platform crate. `None` where `ps`
/// is unavailable.
fn rss_kib() -> Option<u64> {
    let out = Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    String::from_utf8(out.stdout).ok()?.trim().parse().ok()
}

/// Runs one iteration and prints its JSON line.
fn record(
    scenario: &str,
    variant: &str,
    cache: &str,
    iteration: usize,
    body: impl FnOnce() -> Sample,
) {
    let before = rss_kib();
    let sample = body();
    let after = rss_kib();
    let total = sample.total.unwrap_or_else(|| sample.stages.total());
    let rss_delta = before.zip(after).map(|(b, a)| a as i64 - b as i64);
    let line = json!({
        "scenario": scenario,
        "variant": variant,
        "cache": cache,
        "iteration": iteration,
        "viewport": [VIEWPORT.0, VIEWPORT.1],
        "stages_ms": {
            "style_flush": ms(sample.stages.style_flush),
            "box_tree": ms(sample.stages.box_tree),
            "layout": ms(sample.stages.layout),
            "display_list": ms(sample.stages.display_list),
            "raster": ms(sample.stages.raster),
            "encode": ms(sample.stages.encode),
        },
        "layout_covers_style_and_boxes": sample.layout_covers_style_and_boxes,
        "total_ms": total.as_secs_f64() * 1e3,
        "fragments": sample.fragments,
        "rss_before_kib": before,
        "rss_after_kib": after,
        "rss_delta_kib": rss_delta,
        "detail": sample.extra,
    });
    println!("{line}");
}

// ---------------------------------------------------------------------------
// Pages

fn article(sections: Option<usize>) -> String {
    let path = tenta_testutil::fixtures_root().join("static-article/index.html");
    let html = std::fs::read_to_string(&path).expect("the static-article fixture exists");
    let Some(sections) = sections else {
        return html;
    };
    let cut = html
        .match_indices("<h2")
        .nth(sections)
        .map(|(at, _)| at)
        .expect("the article has that many sections");
    format!(
        "{}</article></main><footer class=\"site\"><p>end</p></footer></body></html>",
        &html[..cut]
    )
}

/// About 5 000 boxes of text; `seed` makes every word sequence new to the
/// shaping caches.
fn text_page_5k(seed: usize) -> String {
    let mut html =
        String::from("<!doctype html><style>body{margin:0}p{margin:2px 8px}</style><body>");
    for i in 0..2500 {
        html.push_str(&format!("<p>item {seed}-{i} <b>bold</b> short text</p>"));
    }
    html
}

/// A component-built page: flex rows, grids and flex columns inside one
/// another, eight containers deep, with block wrappers between some levels.
/// About 430 containers, each holding little text.
fn nested_containers_page() -> String {
    let mut html = String::from(
        "<!doctype html><style>body{margin:0}main{display:flex;flex-direction:column;gap:8px}\
         .shell{display:grid;grid-template-columns:minmax(0,1fr)}\
         .stack{display:flex;flex-direction:column}\
         .row{display:flex;gap:12px}.col{flex:1}\
         .grid{display:grid;grid-template-columns:1fr 1fr;gap:6px}\
         .card{display:flex;flex-direction:column;padding:4px}\
         .head{display:flex;align-items:center;gap:4px}.icon{width:16px;height:16px}\
         .line{display:flex;gap:4px}.pair{display:grid;grid-template-columns:1fr auto;flex:1}\
         p{margin:0}</style><body><main>",
    );
    for section in 0..12 {
        html.push_str("<div class=shell><div class=stack><section class=row>");
        for col in 0..3 {
            html.push_str("<div class=col><div class=grid>");
            for card in 0..2 {
                html.push_str(&format!(
                    "<div class=card><div class=head><span class=icon></span>\
                     <div>card {section}-{col}-{card}</div></div><div><div class=line>"
                ));
                for pair in 0..2 {
                    html.push_str(&format!(
                        "<div class=pair><p>some text of pair {pair} that wraps</p><p>{pair}</p></div>"
                    ));
                }
                html.push_str("</div></div></div>");
            }
            html.push_str("</div></div>");
        }
        html.push_str("</section></div></div>");
    }
    html.push_str("</main>");
    html
}

/// 10 000 elements styled by 200 rules.
fn style_page_10k() -> String {
    let mut html = String::from("<!doctype html><style>");
    for rule in 0..200 {
        html.push_str(&format!(
            ".c{} .x{}{{color:#{:06x};margin:{}px}}",
            rule % 50,
            rule % 20,
            rule * 997 % 0xffffff,
            rule % 7
        ));
    }
    html.push_str("</style><body>");
    for i in 0..2_500 {
        html.push_str(&format!(
            "<div class=c{}><span class=x{}>a</span><span class=x{}>b</span><i>c</i></div>",
            i % 50,
            i % 20,
            (i + 3) % 20
        ));
    }
    html
}

fn parse(html: &str, viewport: (f32, f32)) -> (Document, Vec<StyleSource>) {
    let env = StyleEnv {
        viewport,
        ..StyleEnv::default()
    };
    let (doc, engine) = style_html(html, env);
    let sources = engine.sources().to_vec();
    (doc, sources)
}

fn viewport_f32() -> (f32, f32) {
    (VIEWPORT.0 as f32, VIEWPORT.1 as f32)
}

fn style_env() -> StyleEnv {
    StyleEnv {
        viewport: viewport_f32(),
        ..StyleEnv::default()
    }
}

/// Style flush, box tree and layout of `doc` with a new style engine, each
/// timed on its own.
fn lay_out(
    fonts: &mut FontSystem,
    doc: &Document,
    sources: &[StyleSource],
) -> (Stages, FragmentTree, usize) {
    let mut engine = StyleEngine::new(style_env(), fonts.metrics());
    let sources = sources.to_vec();
    let started = Instant::now();
    engine.set_sources(sources);
    engine.flush(doc);
    let style_flush = started.elapsed();

    let mut cx = LayoutCx {
        doc,
        styles: &engine,
        fonts,
        images: &NoImages,
        viewport: viewport_f32(),
        quirks: engine.env().quirks,
    };
    let started = Instant::now();
    let tree = BoxTree::build(&cx);
    let box_tree = started.elapsed();

    let started = Instant::now();
    let frags = layout(&mut cx, &tree);
    let layout_time = started.elapsed();
    let stages = Stages {
        style_flush: Some(style_flush),
        box_tree: Some(box_tree),
        layout: Some(layout_time),
        ..Stages::default()
    };
    (stages, frags, tree.len())
}

fn touch(doc: &mut Document, generation: usize) {
    let body = doc.body().expect("a body");
    doc.mutate(Mutation::SetAttribute {
        element: body,
        name: QualName::new(atoms::EMPTY, Atom::new("data-run")),
        value: generation.to_string(),
    })
    .expect("set attribute");
}

// ---------------------------------------------------------------------------
// Session scenarios

fn session_stages(t: StageTimes) -> Stages {
    Stages {
        layout: Some(t.layout),
        display_list: Some(t.display_list),
        raster: Some(t.raster),
        encode: Some(t.encode),
        ..Stages::default()
    }
}

/// One `RenderSession::screenshot` call, timed as a whole.
fn capture(
    session: &mut RenderSession,
    doc: &Document,
    opts: &ScreenshotOptions,
    fragments: impl FnOnce(&mut RenderSession, &Document) -> usize,
) -> Sample {
    let started = Instant::now();
    let shot = session.screenshot(doc, opts).expect("the page renders");
    let total = started.elapsed();
    Sample {
        stages: session_stages(shot.times),
        total: Some(total),
        layout_covers_style_and_boxes: true,
        fragments: Some(fragments(session, doc)),
        extra: json!({
            "png_bytes": shot.bytes.len(),
            "width": shot.width,
            "height": shot.height,
            "clamped": shot.clamped,
            "raster_complete": shot.stats.is_complete(),
        }),
    }
}

/// Fragment count of the session's current layout. `layout` does no work while
/// nothing changed, so asking after the timed call does not perturb it.
fn fragment_count(session: &mut RenderSession, doc: &Document) -> usize {
    session.layout(doc).len()
}

/// Cold (new session every time), warm with nothing changed, and warm after a
/// mutation that forces style, layout and display list again.
fn session_variants(scenario: &str, html: &str, opts: &ScreenshotOptions, iterations: usize) {
    let (mut doc, sources) = parse(html, viewport_f32());
    let fresh = || {
        let mut session = RenderSession::new(RenderConfig::default());
        session.set_style_sources(sources.clone());
        session
    };
    for i in 0..iterations {
        let mut session = fresh();
        record(scenario, "new-session", session_cache(), i, || {
            capture(&mut session, &doc, opts, fragment_count)
        });
    }
    let mut session = fresh();
    session.screenshot(&doc, opts).expect("the page renders");
    for i in 0..iterations {
        record(scenario, "unchanged", "warm", i, || {
            capture(&mut session, &doc, opts, fragment_count)
        });
    }
    for i in 0..iterations {
        touch(&mut doc, i);
        record(scenario, "relayout", "warm", i, || {
            capture(&mut session, &doc, opts, fragment_count)
        });
    }
}

fn screenshot(iterations: usize) {
    session_variants(
        "S-screenshot-inproc",
        &article(Some(TYPICAL_SECTIONS)),
        &ScreenshotOptions::default(),
        iterations,
    );
}

fn full_page() -> ScreenshotOptions {
    ScreenshotOptions {
        capture: Capture::FullPage,
        ..ScreenshotOptions::default()
    }
}

/// The 1 MB article is far taller than the 16 384 px capture limit, so every
/// iteration reports `clamped: true`.
fn fullpage_whole_article(iterations: usize) {
    session_variants(
        "S-fullpage-inproc",
        &article(None),
        &full_page(),
        iterations,
    );
}

fn fullpage_12000(iterations: usize) {
    session_variants(
        "S-fullpage-12000",
        &article(Some(FULLPAGE_12000_SECTIONS)),
        &full_page(),
        iterations,
    );
}

/// The small fixture pages: dominated by session start-up, so only cold.
fn fixtures(iterations: usize) {
    let dir = tenta_testutil::fixtures_root().join("render");
    let mut pages: Vec<_> = std::fs::read_dir(&dir)
        .expect("tests/fixtures/render exists")
        .map(|entry| entry.expect("readable entry").path())
        .filter(|path| path.extension().is_some_and(|e| e == "html"))
        .collect();
    pages.sort();
    for path in pages {
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .expect("a UTF-8 file name")
            .to_owned();
        let html = std::fs::read_to_string(&path).expect("readable fixture");
        let (doc, sources) = parse(&html, viewport_f32());
        for i in 0..iterations {
            let mut session = RenderSession::new(RenderConfig::default());
            session.set_style_sources(sources.clone());
            record("S-fixtures", &name, session_cache(), i, || {
                capture(
                    &mut session,
                    &doc,
                    &ScreenshotOptions::default(),
                    fragment_count,
                )
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Staged scenarios

fn style_10k(iterations: usize) {
    let (doc, sources) = parse(&style_page_10k(), viewport_f32());
    let fonts = FontSystem::new_embedded();
    for i in 0..iterations {
        record("S-style-10k", "initial-traversal", "cold", i, || {
            let mut engine = StyleEngine::new(style_env(), fonts.metrics());
            let sources = sources.clone();
            let started = Instant::now();
            engine.set_sources(sources);
            engine.flush(&doc);
            let stages = Stages {
                style_flush: Some(started.elapsed()),
                ..Stages::default()
            };
            Sample::staged(stages, None)
        });
    }
}

/// Cold: every iteration lays out text the shaping caches have never seen, as
/// on a first load. Warm: the same document laid out again.
fn layout_5k(iterations: usize) {
    let mut fonts = FontSystem::new_embedded();
    for i in 0..iterations {
        let (doc, sources) = parse(&text_page_5k(i + 1), viewport_f32());
        record("S-layout-5k", "distinct-text", "cold", i, || {
            let (stages, frags, boxes) = lay_out(&mut fonts, &doc, &sources);
            let mut sample = Sample::staged(stages, Some(frags.len()));
            sample.extra = json!({ "boxes": boxes });
            sample
        });
    }
    let (doc, sources) = parse(&text_page_5k(0), viewport_f32());
    lay_out(&mut fonts, &doc, &sources);
    for i in 0..iterations {
        record("S-layout-5k", "repeat", "warm", i, || {
            let (stages, frags, boxes) = lay_out(&mut fonts, &doc, &sources);
            let mut sample = Sample::staged(stages, Some(frags.len()));
            sample.extra = json!({ "boxes": boxes });
            sample
        });
    }
}

/// The same nested-container document laid out again, so shaping is warm and
/// what is timed is how often the container algorithms run.
fn layout_nested(iterations: usize) {
    let mut fonts = FontSystem::new_embedded();
    let (doc, sources) = parse(&nested_containers_page(), viewport_f32());
    lay_out(&mut fonts, &doc, &sources);
    for i in 0..iterations {
        record("S-layout-nested", "repeat", "warm", i, || {
            let (stages, frags, boxes) = lay_out(&mut fonts, &doc, &sources);
            let mut sample = Sample::staged(stages, Some(frags.len()));
            sample.extra = json!({ "boxes": boxes });
            sample
        });
    }
}

fn display_list_5k(iterations: usize) {
    let mut fonts = FontSystem::new_embedded();
    let (doc, sources) = parse(&text_page_5k(0), viewport_f32());
    let (_, frags, _) = lay_out(&mut fonts, &doc, &sources);
    let images = ImageStore::new();
    for i in 0..iterations {
        record("S-dl-5k", "build", "warm", i, || {
            let started = Instant::now();
            let list = build(&frags, &PaintContext::new(&images));
            let stages = Stages {
                display_list: Some(started.elapsed()),
                ..Stages::default()
            };
            let mut sample = Sample::staged(stages, Some(frags.len()));
            sample.extra = json!({ "items": list.items.len() });
            sample
        });
    }
}

fn raster_1280(iterations: usize) {
    let mut fonts = FontSystem::new_embedded();
    let (doc, sources) = parse(&article(Some(TYPICAL_SECTIONS)), viewport_f32());
    let (_, frags, _) = lay_out(&mut fonts, &doc, &sources);
    let images = ImageStore::new();
    let list: DisplayList = build(&frags, &PaintContext::new(&images));
    let options = RasterOptions {
        width: VIEWPORT.0,
        height: VIEWPORT.1,
        scale: 1.0,
        origin: (0.0, 0.0),
        background: Some(Color::WHITE),
    };
    for i in 0..iterations {
        record("S-raster-1280", "viewport", "warm", i, || {
            let started = Instant::now();
            let output = rasterize(&list, &options, &images).expect("the list rasterises");
            let raster = started.elapsed();
            let started = Instant::now();
            let png = encode_png(&output.pixmap, PngProfile::Fast);
            let encode = started.elapsed();
            let stages = Stages {
                raster: Some(raster),
                encode: Some(encode),
                ..Stages::default()
            };
            let mut sample = Sample::staged(stages, Some(frags.len()));
            sample.extra = json!({
                "png_bytes": png.len(),
                "items": list.items.len(),
                "raster_complete": output.stats.is_complete(),
            });
            sample
        });
    }
}

/// Construction of the bundled font system. Only the first build in a process
/// parses anything (the 1 ms budget applies to it); later ones are clones of
/// shared data, so run it with `--fresh-process` for a repeatable cold number.
fn font_init(iterations: usize) {
    for i in 0..iterations {
        let (variant, cache) = if i == 0 {
            ("first-build", "cold")
        } else {
            ("clone", "warm")
        };
        record("S-font-init", variant, cache, i, || {
            let started = Instant::now();
            let fonts = FontSystem::new_embedded();
            let elapsed = started.elapsed();
            drop(fonts);
            Sample {
                total: Some(elapsed),
                ..Sample::staged(Stages::default(), None)
            }
        });
    }
}
