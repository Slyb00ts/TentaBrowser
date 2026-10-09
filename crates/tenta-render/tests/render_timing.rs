//! Timing of the whole pipeline on article pages. Ignored by default because
//! the numbers only mean something in a release build on a quiet machine:
//!
//! ```text
//! cargo nextest run --release -p tenta-render --run-ignored only --no-capture
//! ```

use std::time::{Duration, Instant};

use tenta_core::atom::{Atom, atoms};
use tenta_css::cascade::test_util::style_html;
use tenta_css::cascade::{StyleEnv, StyleSource};
use tenta_dom::{Document, Mutation};
use tenta_html::QualName;
use tenta_render::{RenderConfig, RenderSession, Screenshot, ScreenshotOptions, StageTimes};

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

fn percentile(sorted: &[Duration], q: f64) -> Duration {
    sorted[((sorted.len() - 1) as f64 * q).round() as usize]
}

fn summary(label: &str, mut runs: Vec<Duration>) {
    runs.sort();
    eprintln!(
        "  {label:<22} p50 {:>9.2?}  p95 {:>9.2?}  (n={})",
        percentile(&runs, 0.5),
        percentile(&runs, 0.95),
        runs.len()
    );
}

fn stage_summary(label: &str, shots: &[StageTimes]) {
    let median = |pick: fn(&StageTimes) -> Duration| {
        let mut all: Vec<_> = shots.iter().map(pick).collect();
        all.sort();
        percentile(&all, 0.5)
    };
    eprintln!(
        "  {label:<22} layout {:>9.2?}  list {:>9.2?}  raster {:>9.2?}  png {:>9.2?}",
        median(|t| t.layout),
        median(|t| t.display_list),
        median(|t| t.raster),
        median(|t| t.encode),
    );
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

fn measure(name: &str, html: &str, runs: usize) {
    let (mut doc, engine) = style_html(html, StyleEnv::default());
    let sources: Vec<StyleSource> = engine.sources().to_vec();
    let opts = ScreenshotOptions::default();
    let fresh = || {
        let mut session = RenderSession::new(RenderConfig::default());
        session.set_style_sources(sources.clone());
        session
    };
    eprintln!("{name}: {} bytes of HTML", html.len());

    let (mut cold, mut cold_stages) = (Vec::new(), Vec::new());
    let mut last: Option<Screenshot> = None;
    for _ in 0..runs {
        let mut session = fresh();
        let started = Instant::now();
        let shot = session.screenshot(&doc, &opts).expect("renders");
        cold.push(started.elapsed());
        cold_stages.push(shot.times);
        last = Some(shot);
    }
    let last = last.expect("at least one run");
    summary("new session, capture", cold);
    stage_summary("  stages", &cold_stages);
    let mut probe = fresh();
    eprintln!(
        "  png {} bytes, skipped {:?}, {} fragments",
        last.bytes.len(),
        last.stats.skipped,
        probe.layout(&doc).len()
    );
    if let Some(dir) = std::env::var_os("DUMP_DIR") {
        std::fs::write(
            std::path::Path::new(&dir).join(format!("{name}.png")),
            &last.bytes,
        )
        .expect("dump directory is writable");
    }

    let mut session = fresh();
    session.screenshot(&doc, &opts).expect("renders");
    let (mut warm, mut warm_stages) = (Vec::new(), Vec::new());
    for _ in 0..runs * 3 {
        let started = Instant::now();
        let shot = session.screenshot(&doc, &opts).expect("renders");
        warm.push(started.elapsed());
        warm_stages.push(shot.times);
    }
    summary("warm, nothing changed", warm);
    stage_summary("  stages", &warm_stages);

    let (mut relayout, mut relayout_stages) = (Vec::new(), Vec::new());
    for generation in 0..runs * 3 {
        touch(&mut doc, generation);
        let started = Instant::now();
        let shot = session.screenshot(&doc, &opts).expect("renders");
        relayout.push(started.elapsed());
        relayout_stages.push(shot.times);
    }
    summary("warm, after a mutation", relayout);
    stage_summary("  stages", &relayout_stages);
}

#[test]
#[ignore = "timing report; run in release"]
fn timing_report() {
    measure("typical-article", &article(Some(9)), 25);
    measure("static-article-full", &article(None), 5);
}
