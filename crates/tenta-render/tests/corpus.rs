//! The render corpus: every `render-fixtures/*/index.html` rendered through
//! the Tier-1 pipeline (parse, style, layout, paint) at the capture viewport
//! and graded per check against `render-fixtures/EXPECTED.toml`.
//!
//! * Colour-component checks (`color-checks.json`) are evaluated natively and
//!   are the graded unit. `M1-GATE.toml` (frozen, read only) names the checks
//!   that must pass for the M1 exit; `EXPECTED.toml` is the ratchet: a check
//!   listed as `pass` that fails is a regression and fails the test.
//! * Element rects (`checks.json`) and pixel distance to `chromium.png` are
//!   reported, not graded.
//! * Fixtures in [`SCRIPTED`] need the JavaScript page path, which this crate
//!   cannot depend on (`cargo xtask lint-deps`), so they are skipped by name.
//!   The list is asserted against the corpus, so a new scripted fixture fails
//!   the test instead of being skipped silently.
//!
//! Output: `render-fixtures/<name>/tentabrowser.png` (gitignored) and
//! `target/render-corpus/{report.json,summary.txt}`.
//!
//! Environment: `UPDATE_EXPECTED` rewrites `EXPECTED.toml`; `1` blesses the
//! passing gate checks, `all` every passing check, `a,b` every passing check
//! of fixtures `a` and `b`; existing `pass` entries are always kept.
//! `RENDER_CORPUS_PYCHECK=1` also runs `tools/check.py` on our PNGs
//! and fails on any verdict difference or when its Python modules are missing
//! (`RENDER_CORPUS_PYTHON` picks the interpreter).

#[path = "support/components.rs"]
mod components;
#[path = "support/structural.rs"]
mod structural;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tenta_core::atom::{Atom, atoms};
use tenta_css::cascade::StyleEnv;
use tenta_css::cascade::test_util::style_html;
use tenta_dom::{Document, Mutation};
use tenta_html::QualName;
use tenta_render::{RenderConfig, RenderSession, ScreenshotOptions, Viewport};

use components::{ColorResult, PixelMetrics, Rgb};
use structural::ElementResult;

const VIEWPORT: (u32, u32) = (900, 1000);

/// Fixtures whose result is produced by script. They are skipped here and
/// belong to the JavaScript-capable corpus run.
const SCRIPTED: &[&str] = &[
    "animation-fill-forwards",
    "eventtarget-lifecycle",
    "flex-flow",
    "media-text-track",
    "modern-hydration-contracts",
    "modern-streams",
    "viewport-consistency",
];

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../render-fixtures")
}

fn output_dir() -> PathBuf {
    let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"),
        PathBuf::from,
    );
    target.join("render-corpus")
}

fn read_json(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    Some(serde_json::from_str(&text).unwrap_or_else(|err| panic!("{}: {err}", path.display())))
}

fn json_list(path: &Path) -> Vec<Value> {
    read_json(path)
        .map(|value| value.as_array().cloned().expect("a JSON array"))
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

struct Rendered {
    png: Vec<u8>,
    complete: bool,
    doc: Document,
    session: RenderSession,
    elapsed: Duration,
}

/// Renders `html` at the capture viewport. `mutate` edits the parsed document
/// before the first layout.
fn render(html: &str, mutate: impl FnOnce(&mut Document)) -> Rendered {
    let started = Instant::now();
    let (mut doc, engine) = style_html(html, StyleEnv::default());
    mutate(&mut doc);
    let mut session = RenderSession::new(RenderConfig {
        viewport: Viewport {
            width: VIEWPORT.0,
            height: VIEWPORT.1,
            device_pixel_ratio: 1.0,
        },
        ..RenderConfig::default()
    });
    session.set_style_sources(engine.sources().to_vec());
    let shot = session
        .screenshot(&doc, &ScreenshotOptions::default())
        .expect("the fixture renders");
    Rendered {
        png: shot.bytes,
        complete: shot.stats.is_complete(),
        doc,
        session,
        elapsed: started.elapsed(),
    }
}

// ---------------------------------------------------------------------------
// Per-fixture results
// ---------------------------------------------------------------------------

struct FixtureRun {
    name: String,
    skipped: Option<&'static str>,
    check_names: Vec<String>,
    colour: Vec<ColorResult>,
    elements: Vec<ElementResult>,
    metrics: Option<PixelMetrics>,
    raster_complete: bool,
    elapsed: Duration,
}

fn text_rects(checks: &[Value]) -> Vec<[f64; 4]> {
    checks
        .iter()
        .filter(|check| check["kind"] == "text")
        .map(|check| ["x", "y", "width", "height"].map(|key| check[key].as_f64().unwrap_or(0.0)))
        .collect()
}

fn run_fixture(name: &str, mutate: impl FnOnce(&mut Document), write_png: bool) -> FixtureRun {
    let dir = corpus_dir().join(name);
    let html = std::fs::read_to_string(dir.join("index.html")).expect("index.html");
    let colour_checks = json_list(&dir.join("color-checks.json"));
    let mut run = FixtureRun {
        name: name.to_owned(),
        skipped: None,
        check_names: colour_checks
            .iter()
            .map(|check| check["name"].as_str().expect("a check name").to_owned())
            .collect(),
        colour: Vec::new(),
        elements: Vec::new(),
        metrics: None,
        raster_complete: true,
        elapsed: Duration::ZERO,
    };
    if SCRIPTED.contains(&name) {
        run.skipped = Some("needs the JavaScript page path");
        return run;
    }
    let mut rendered = render(&html, mutate);
    if write_png {
        std::fs::write(dir.join("tentabrowser.png"), &rendered.png).expect("write the capture");
    }
    let ours = Rgb::decode(&rendered.png);
    run.colour = components::evaluate_color_checks(&ours, &colour_checks);
    run.raster_complete = rendered.complete;
    run.elapsed = rendered.elapsed;

    let structural = read_json(&dir.join("checks.json"))
        .and_then(|v| v["checks"].as_array().cloned())
        .unwrap_or_default();
    run.elements =
        structural::evaluate_element_checks(&rendered.doc, &mut rendered.session, &structural);
    if let Ok(reference) = std::fs::read(dir.join("chromium.png")) {
        let reference = Rgb::decode(&reference);
        run.metrics = Some(components::pixel_metrics(
            &ours,
            &reference,
            &text_rects(&structural),
        ));
    }
    run
}

fn fixture_names() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(corpus_dir())
        .expect("render-fixtures exists")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().join("index.html").is_file())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Layout recurses, so renders get a generous stack.
const WORKER_STACK: usize = 32 << 20;

/// Renders are independent (each owns its document, fonts and caches), so the
/// fixtures are spread over a few worker threads.
fn run_corpus() -> Vec<FixtureRun> {
    let names = fixture_names();
    let scripted: BTreeSet<&str> = names
        .iter()
        .filter(|name| {
            std::fs::read_to_string(corpus_dir().join(name).join("index.html"))
                .is_ok_and(|html| html.contains("<script"))
        })
        .map(String::as_str)
        .collect();
    assert_eq!(
        scripted,
        SCRIPTED.iter().copied().collect::<BTreeSet<_>>(),
        "the fixtures with <script> differ from SCRIPTED"
    );
    let next = AtomicUsize::new(0);
    let workers = std::thread::available_parallelism().map_or(1, |n| n.get().min(8));
    let mut runs: Vec<FixtureRun> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                std::thread::Builder::new()
                    .stack_size(WORKER_STACK)
                    .spawn_scoped(scope, || {
                        let mut done = Vec::new();
                        while let Some(name) = names.get(next.fetch_add(1, Ordering::Relaxed)) {
                            done.push(run_fixture(name, |_| {}, true));
                        }
                        done
                    })
                    .expect("spawn a render worker")
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("a render worker panicked"))
            .collect()
    });
    runs.sort_by(|a, b| a.name.cmp(&b.name));
    runs
}

// ---------------------------------------------------------------------------
// Gate and expectations
// ---------------------------------------------------------------------------

type CheckKey = (String, String);

#[derive(Default)]
struct Gate {
    required: BTreeSet<CheckKey>,
    deferred: BTreeMap<CheckKey, String>,
    deferred_fixtures: BTreeMap<String, String>,
}

impl Gate {
    fn load() -> Gate {
        let text =
            std::fs::read_to_string(corpus_dir().join("M1-GATE.toml")).expect("M1-GATE.toml");
        let table: toml::Table = text.parse().expect("M1-GATE.toml parses");
        let mut gate = Gate::default();
        for (fixture, value) in &table {
            if fixture == "deferred_fixtures" {
                for group in value["list"].as_array().expect("a list") {
                    let milestone = group["milestone"].as_str().expect("a milestone");
                    for name in group["fixtures"].as_array().expect("fixtures") {
                        gate.deferred_fixtures.insert(
                            name.as_str().expect("a fixture name").to_owned(),
                            milestone.to_owned(),
                        );
                    }
                }
                continue;
            }
            let names = |key: &str| -> Vec<(String, Option<String>)> {
                value[key]
                    .as_array()
                    .map(|entries| {
                        entries
                            .iter()
                            .map(|entry| {
                                (
                                    entry["name"].as_str().expect("a check name").to_owned(),
                                    entry
                                        .get("milestone")
                                        .and_then(|m| m.as_str())
                                        .map(str::to_owned),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            };
            for (name, _) in names("checks") {
                gate.required.insert((fixture.clone(), name));
            }
            for (name, milestone) in names("deferred") {
                gate.deferred.insert(
                    (fixture.clone(), name),
                    milestone.expect("a deferred entry names a milestone"),
                );
            }
        }
        gate
    }

    fn milestone(&self, key: &CheckKey) -> Option<&String> {
        self.deferred
            .get(key)
            .or_else(|| self.deferred_fixtures.get(&key.0))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Status {
    Pass,
    Fail,
    Deferred(String),
    Informational,
}

impl Status {
    fn parse(text: &str) -> Status {
        match text {
            "pass" => Status::Pass,
            "fail" => Status::Fail,
            "informational" => Status::Informational,
            other => Status::Deferred(
                other
                    .strip_prefix("deferred:")
                    .unwrap_or_else(|| panic!("EXPECTED.toml: unknown status {other:?}"))
                    .to_owned(),
            ),
        }
    }

    fn render(&self) -> String {
        match self {
            Status::Pass => "pass".into(),
            Status::Fail => "fail".into(),
            Status::Informational => "informational".into(),
            Status::Deferred(milestone) => format!("deferred:{milestone}"),
        }
    }
}

fn load_expected() -> BTreeMap<CheckKey, Status> {
    let Ok(text) = std::fs::read_to_string(corpus_dir().join("EXPECTED.toml")) else {
        return BTreeMap::new();
    };
    let table: toml::Table = text.parse().expect("EXPECTED.toml parses");
    table
        .iter()
        .flat_map(|(fixture, checks)| {
            checks
                .as_table()
                .expect("a table per fixture")
                .iter()
                .map(|(name, status)| {
                    (
                        (fixture.clone(), name.clone()),
                        Status::parse(status.as_str().expect("a status string")),
                    )
                })
        })
        .collect()
}

fn quote(text: &str) -> String {
    toml::Value::String(text.to_owned()).to_string()
}

/// What `UPDATE_EXPECTED` promotes to `pass`.
enum Bless {
    Gate,
    All,
    Fixtures(BTreeSet<String>),
}

impl Bless {
    fn parse(value: &str) -> Bless {
        match value {
            "1" => Bless::Gate,
            "all" => Bless::All,
            list => Bless::Fixtures(list.split(',').map(|name| name.trim().to_owned()).collect()),
        }
    }

    fn admits(&self, gate: &Gate, key: &CheckKey) -> bool {
        match self {
            Bless::Gate => gate.required.contains(key),
            Bless::All => true,
            Bless::Fixtures(names) => names.contains(&key.0),
        }
    }
}

fn write_expected(
    runs: &[FixtureRun],
    gate: &Gate,
    previous: &BTreeMap<CheckKey, Status>,
    bless: &Bless,
) {
    let mut out = String::from(
        "# Ratchet of the render corpus: one status per colour check.\n\
         #\n\
         # pass            the test fails if this check stops passing\n\
         # fail            known failing, not yet scheduled\n\
         # deferred:<M>    belongs to milestone <M> (see M1-GATE.toml)\n\
         # informational   reported only\n\
         #\n\
         # Checks that start passing are listed by the test; moving them to `pass`\n\
         # is a deliberate edit. M1-GATE.toml is frozen and never edited here.\n",
    );
    for run in runs.iter().filter(|run| !run.check_names.is_empty()) {
        let _ = writeln!(out, "\n[{}]", quote(&run.name));
        for (index, name) in run.check_names.iter().enumerate() {
            let key = (run.name.clone(), name.clone());
            let passes = run.colour.get(index).is_some_and(|result| result.passed);
            let status = match previous.get(&key) {
                Some(Status::Pass) => Status::Pass,
                _ if passes && bless.admits(gate, &key) => Status::Pass,
                Some(Status::Informational) => Status::Informational,
                _ => gate
                    .milestone(&key)
                    .map_or(Status::Fail, |m| Status::Deferred(m.clone())),
            };
            let _ = writeln!(out, "{} = {}", quote(name), quote(&status.render()));
        }
    }
    std::fs::write(corpus_dir().join("EXPECTED.toml"), out).expect("write EXPECTED.toml");
}

// ---------------------------------------------------------------------------
// Verdict
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Verdict {
    /// Expected `pass` and failing now.
    regressions: Vec<String>,
    /// Gate checks that fail (or were skipped): the M1 exit blockers.
    gate_failures: Vec<String>,
    gate_total: usize,
    /// Passing now but not expected to.
    newly_passing: Vec<String>,
    /// Checks missing from `EXPECTED.toml`, or listed there but gone.
    bookkeeping: Vec<String>,
}

fn label(key: &CheckKey) -> String {
    format!("{} / {}", key.0, key.1)
}

fn describe(result: &ColorResult) -> String {
    let nearest: Vec<String> = result
        .nearest
        .iter()
        .map(|c| format!("({},{} {}x{})", c.x, c.y, c.width, c.height))
        .collect();
    format!(
        "expected {} x{}, found {} match(es); nearest wrong-geometry components: {}",
        result.expected,
        result.expected_count,
        result.matches.len(),
        if nearest.is_empty() {
            "none".into()
        } else {
            nearest.join(" ")
        }
    )
}

fn judge(runs: &[FixtureRun], gate: &Gate, expected: &BTreeMap<CheckKey, Status>) -> Verdict {
    let mut verdict = Verdict {
        gate_total: gate.required.len(),
        ..Verdict::default()
    };
    let mut seen = BTreeSet::new();
    for run in runs {
        for (index, name) in run.check_names.iter().enumerate() {
            let key = (run.name.clone(), name.clone());
            seen.insert(key.clone());
            let result = run.colour.get(index);
            let passes = result.is_some_and(|result| result.passed);
            let detail = result.map_or_else(|| "skipped".to_owned(), describe);
            let status = expected.get(&key);
            if status.is_none() {
                verdict
                    .bookkeeping
                    .push(format!("{}: not in EXPECTED.toml", label(&key)));
            }
            if gate.required.contains(&key) && !passes {
                verdict
                    .gate_failures
                    .push(format!("{}: {detail}", label(&key)));
            }
            match (status, passes) {
                (Some(Status::Pass), false) => verdict
                    .regressions
                    .push(format!("{}: {detail}", label(&key))),
                (Some(Status::Pass), true) | (None, _) => {}
                (Some(_), true) => verdict.newly_passing.push(label(&key)),
                (Some(_), false) => {}
            }
        }
    }
    for key in expected.keys().filter(|key| !seen.contains(*key)) {
        verdict.bookkeeping.push(format!(
            "{}: in EXPECTED.toml but not in the corpus",
            label(key)
        ));
    }
    for key in gate.required.iter().filter(|key| !seen.contains(*key)) {
        verdict.bookkeeping.push(format!(
            "{}: in M1-GATE.toml but not in the corpus",
            label(key)
        ));
    }
    verdict
}

// ---------------------------------------------------------------------------
// Reports
// ---------------------------------------------------------------------------

fn round(value: f64) -> f64 {
    (value * 1e6).round() / 1e6
}

fn metrics_json(metrics: &PixelMetrics) -> Value {
    if let Some((ours, reference)) = metrics.size_mismatch {
        return json!({ "size_mismatch": { "ours": ours, "chromium": reference } });
    }
    json!({
        "rgb_mae": round(metrics.rgb_mae),
        "pixels_gt_10": round(metrics.pixels_gt_10),
        "pixels_gt_50": round(metrics.pixels_gt_50),
        "text_mae": metrics.text_mae.map(round),
        "non_text_mae": round(metrics.non_text_mae),
        "text_pixels": metrics.text_pixels,
        "text_rects_measured": metrics.text_rects,
        "text_ink_ratio": metrics.text_ink_ratio.map(round),
        "text_centroid_dx": metrics.text_centroid_dx.map(round),
        "text_centroid_dy": metrics.text_centroid_dy.map(round),
        "text_centroid_abs_dy": metrics.text_centroid_abs_dy.map(round),
    })
}

fn element_summary(elements: &[ElementResult]) -> Value {
    let mut classes = BTreeMap::new();
    for element in elements {
        let entry = classes
            .entry(element.class.label())
            .or_insert_with(|| json!({ "total": 0, "within_tolerance": 0 }));
        entry["total"] = json!(entry["total"].as_u64().unwrap_or(0) + 1);
        if element.passed {
            entry["within_tolerance"] = json!(entry["within_tolerance"].as_u64().unwrap_or(0) + 1);
        }
    }
    let worst: Vec<Value> = elements
        .iter()
        .filter(|element| !element.passed)
        .take(5)
        .map(|element| {
            json!({
                "selector": element.selector, "class": element.class.label(),
                "expected": element.expected, "actual": element.actual,
            })
        })
        .collect();
    json!({ "by_class": classes, "first_misses": worst })
}

fn report_json(
    runs: &[FixtureRun],
    gate: &Gate,
    verdict: &Verdict,
    expected: &BTreeMap<CheckKey, Status>,
) -> Value {
    let fixtures: serde_json::Map<String, Value> = runs
        .iter()
        .map(|run| {
            let checks: Vec<Value> = run
                .check_names
                .iter()
                .enumerate()
                .map(|(index, name)| {
                    let key = (run.name.clone(), name.clone());
                    let result = run.colour.get(index);
                    json!({
                        "name": name,
                        "passed": result.map(|r| r.passed),
                        "gate": gate.required.contains(&key),
                        "expected_status": expected.get(&key).map(Status::render),
                        "detail": result.filter(|r| !r.passed).map(describe),
                    })
                })
                .collect();
            (
                run.name.clone(),
                json!({
                    "skipped": run.skipped,
                    "render_ms": run.elapsed.as_millis() as u64,
                    "raster_complete": run.raster_complete,
                    "color_checks": checks,
                    "pixel_metrics": run.metrics.as_ref().map(metrics_json),
                    "element_rects": element_summary(&run.elements),
                }),
            )
        })
        .collect();
    let graded: Vec<&ColorResult> = runs.iter().flat_map(|run| run.colour.iter()).collect();
    json!({
        "viewport": [VIEWPORT.0, VIEWPORT.1],
        "m1_gate": {
            "passed": verdict.gate_total - verdict.gate_failures.len(),
            "total": verdict.gate_total,
            "failures": verdict.gate_failures,
        },
        "corpus": {
            "color_checks_passed": graded.iter().filter(|r| r.passed).count(),
            "color_checks_graded": graded.len(),
            "fixtures_skipped": runs.iter().filter(|r| r.skipped.is_some()).map(|r| r.name.clone()).collect::<Vec<_>>(),
        },
        "regressions": verdict.regressions,
        "newly_passing": verdict.newly_passing,
        "bookkeeping": verdict.bookkeeping,
        "fixtures": fixtures,
    })
}

fn summary_text(runs: &[FixtureRun], verdict: &Verdict) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{:<38} {:>7} {:>8} {:>8} {:>8} {:>7} {:>7}",
        "fixture", "colour", "mae", "textMAE", "nontxtMAE", "inkRat", "dY"
    );
    for run in runs {
        let passed = run.colour.iter().filter(|r| r.passed).count();
        let cell = |value: Option<f64>| value.map_or("-".to_owned(), |v| format!("{v:.4}"));
        let metrics = run.metrics.as_ref();
        let _ = writeln!(
            out,
            "{:<38} {:>7} {:>8} {:>8} {:>8} {:>7} {:>7}{}",
            run.name,
            format!("{passed}/{}", run.check_names.len()),
            cell(metrics.map(|m| m.rgb_mae)),
            cell(metrics.and_then(|m| m.text_mae)),
            cell(metrics.map(|m| m.non_text_mae)),
            cell(metrics.and_then(|m| m.text_ink_ratio)),
            cell(metrics.and_then(|m| m.text_centroid_dy)),
            run.skipped
                .map_or(String::new(), |why| format!("  SKIPPED: {why}"))
                + if run.raster_complete {
                    ""
                } else {
                    "  INCOMPLETE RASTER"
                },
        );
    }
    let graded: Vec<&ColorResult> = runs.iter().flat_map(|run| run.colour.iter()).collect();
    let _ = writeln!(
        out,
        "\ncorpus: {}/{} colour checks pass ({} scripted fixtures skipped, {} with incomplete raster)",
        graded.iter().filter(|r| r.passed).count(),
        graded.len(),
        runs.iter().filter(|r| r.skipped.is_some()).count(),
        runs.iter().filter(|r| !r.raster_complete).count()
    );
    let _ = writeln!(
        out,
        "M1 gate: {}/{} pass",
        verdict.gate_total - verdict.gate_failures.len(),
        verdict.gate_total
    );
    for (title, lines) in [
        (
            "M1 EXIT BLOCKERS (gate checks failing)",
            &verdict.gate_failures,
        ),
        ("REGRESSIONS (expected pass, failing)", &verdict.regressions),
        (
            "BOOKKEEPING (EXPECTED.toml / gate out of sync)",
            &verdict.bookkeeping,
        ),
        (
            "NEWLY PASSING (bless by editing EXPECTED.toml)",
            &verdict.newly_passing,
        ),
    ] {
        if !lines.is_empty() {
            let _ = writeln!(out, "\n{title}:");
            for line in lines {
                let _ = writeln!(out, "  {line}");
            }
        }
    }
    out
}

fn publish(
    runs: &[FixtureRun],
    gate: &Gate,
    verdict: &Verdict,
    expected: &BTreeMap<CheckKey, Status>,
) -> String {
    let out = output_dir();
    std::fs::create_dir_all(&out).expect("create target/render-corpus");
    let report = report_json(runs, gate, verdict, expected);
    std::fs::write(
        out.join("report.json"),
        serde_json::to_string_pretty(&report).expect("serialisable") + "\n",
    )
    .expect("write report.json");
    let summary = summary_text(runs, verdict);
    std::fs::write(out.join("summary.txt"), &summary).expect("write summary.txt");
    summary
}

// ---------------------------------------------------------------------------
// Python cross-check
// ---------------------------------------------------------------------------

/// Runs `tools/check.py` on the PNGs this run wrote and compares its verdict
/// per colour check with the native one. Returns the differences.
fn python_cross_check(runs: &[FixtureRun]) -> Vec<String> {
    let python = std::env::var("RENDER_CORPUS_PYTHON").unwrap_or_else(|_| "python3".to_owned());
    let probe = Command::new(&python)
        .args(["-c", "import numpy, scipy, PIL"])
        .output()
        .unwrap_or_else(|err| panic!("RENDER_CORPUS_PYCHECK=1: cannot run {python}: {err}"));
    assert!(
        probe.status.success(),
        "RENDER_CORPUS_PYCHECK=1 needs numpy, scipy and pillow in {python} \
         (see docs/dev/render-fixtures.md): {}",
        String::from_utf8_lossy(&probe.stderr)
    );
    let report_path = output_dir().join("check-py.json");
    let mut command = Command::new(&python);
    command
        .arg(corpus_dir().join("tools/check.py"))
        .args(["--engine", "tentabrowser", "--report"])
        .arg(&report_path);
    for run in runs.iter().filter(|run| run.skipped.is_none()) {
        command.args(["--fixture", &run.name]);
    }
    // check.py exits non-zero when any check fails; its report is still written.
    let output = command.output().expect("run check.py");
    let report = read_json(&report_path).unwrap_or_else(|| {
        panic!(
            "check.py wrote no report: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    let mut differences = Vec::new();
    for run in runs.iter().filter(|run| run.skipped.is_none()) {
        let Some(python_results) =
            report["fixtures"][&run.name]["behavior"]["tentabrowser"].as_array()
        else {
            differences.push(format!("{}: missing from the check.py report", run.name));
            continue;
        };
        if python_results.len() != run.colour.len() {
            differences.push(format!("{}: check count differs", run.name));
            continue;
        }
        for (native, python) in run.colour.iter().zip(python_results) {
            if python["passed"].as_bool() != Some(native.passed) {
                differences.push(format!(
                    "{} / {}: native {} but check.py {}",
                    run.name, native.name, native.passed, python["passed"]
                ));
            }
        }
    }
    differences
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn corpus_does_not_regress() {
    let started = Instant::now();
    let runs = run_corpus();
    let gate = Gate::load();
    let previous = load_expected();
    if let Ok(mode) = std::env::var("UPDATE_EXPECTED") {
        write_expected(&runs, &gate, &previous, &Bless::parse(&mode));
    }
    let expected = load_expected();
    let verdict = judge(&runs, &gate, &expected);
    let summary = publish(&runs, &gate, &verdict, &expected);
    eprintln!("{summary}corpus rendered in {:.1?}", started.elapsed());

    let mut problems = Vec::new();
    if std::env::var_os("RENDER_CORPUS_PYCHECK").is_some() {
        problems.extend(python_cross_check(&runs));
    }
    problems.extend(
        verdict
            .regressions
            .iter()
            .map(|line| format!("regression: {line}")),
    );
    problems.extend(verdict.bookkeeping.iter().cloned());
    assert!(
        problems.is_empty(),
        "{} problem(s) (UPDATE_EXPECTED=1|all|<fixtures> reclassifies new checks):\n{}",
        problems.len(),
        problems.join("\n")
    );
}

/// The M1 exit criterion: every `M1-GATE.toml` check passes.
#[test]
fn m1_gate_is_green() {
    let runs = run_corpus();
    let gate = Gate::load();
    let verdict = judge(&runs, &gate, &load_expected());
    assert!(
        verdict.gate_failures.is_empty(),
        "{}/{} gate checks pass; failing:\n{}",
        verdict.gate_total - verdict.gate_failures.len(),
        verdict.gate_total,
        verdict.gate_failures.join("\n")
    );
}

/// The native port must see what `check.py` sees: Chromium's own captures
/// satisfy every gate check.
#[test]
fn reference_captures_satisfy_the_gate() {
    let gate = Gate::load();
    let mut failures = Vec::new();
    for name in fixture_names() {
        let dir = corpus_dir().join(&name);
        let Ok(png) = std::fs::read(dir.join("chromium.png")) else {
            continue;
        };
        let checks = json_list(&dir.join("color-checks.json"));
        let results = components::evaluate_color_checks(&Rgb::decode(&png), &checks);
        failures.extend(
            results
                .iter()
                .filter(|r| !r.passed && gate.required.contains(&(name.clone(), r.name.clone())))
                .map(|r| format!("{name} / {}: {}", r.name, describe(r))),
        );
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The harness can go red: a margin added through the document's inline
/// style must break gate checks of `block-auto-margins`.
#[test]
fn a_document_mutation_turns_gate_checks_red() {
    let gate = Gate::load();
    let fixture = "block-auto-margins";
    let failing = |run: &FixtureRun| {
        run.colour
            .iter()
            .filter(|r| {
                !r.passed
                    && gate
                        .required
                        .contains(&(fixture.to_owned(), r.name.clone()))
            })
            .count()
    };
    let baseline = failing(&run_fixture(fixture, |_| {}, false));
    let mutated = run_fixture(
        fixture,
        |doc| {
            let body = doc.body().expect("a body");
            doc.mutate(Mutation::SetAttribute {
                element: body,
                name: QualName::new(atoms::EMPTY, Atom::new("style")),
                value: "margin: 20px".to_owned(),
            })
            .expect("set the style attribute");
        },
        false,
    );
    assert!(
        failing(&mutated) > baseline,
        "mutation left {} failing gate checks, baseline {baseline}",
        failing(&mutated)
    );
}
