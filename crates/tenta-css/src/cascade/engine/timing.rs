//! Per-stage flush timing on a generated 5,000-element page. Ignored by
//! default: it is a measurement, meaningful only in release on a quiet machine.
//!
//! `cargo nextest run -p tenta-css --features stylo --release --run-ignored only \
//!     --no-capture -E 'test(flush_stages)'`

use std::sync::Arc;
use std::time::{Duration, Instant};

use tenta_core::atom::{Atom, atoms};
use tenta_dom::{Document, Mutation};
use tenta_html::{Parser, ParserOptions, QualName};

use super::*;
use crate::cascade::types::FontMetricsData;

const CARDS: usize = 1250;
const RUNS: usize = 30;

struct Flat;

impl FontMetrics for Flat {
    fn metrics(&self, _: &str, _: u16, _: bool, size_px: f32) -> FontMetricsData {
        FontMetricsData {
            ascent: size_px,
            ..FontMetricsData::default()
        }
    }
}

fn rules() -> String {
    let mut sheet = String::from(
        "body { font: 14px/1.4 serif; color: rgb(30, 30, 30) } .card { padding: 4px; margin: 2px }\
         .card > p { margin: 0 } a[href] { color: rgb(0, 0, 200) } .card:nth-child(2n+1) { color: red }\n",
    );
    for i in 0..100 {
        sheet.push_str(&format!(
            ".card-{i} .t{i} {{ color: rgb({i}, 0, 0) }} .group-{i} > span {{ margin-left: {i}px }} \
             div.item-{i}:not(.off) {{ width: calc({i}px + 2em) }}\n"
        ));
    }
    sheet
}

fn page() -> String {
    let mut html = String::from("<!DOCTYPE html><html><head></head><body>");
    for i in 0..CARDS {
        let k = i % 100;
        html.push_str(&format!(
            "<div id=c{i} class='card card-{k} item-{k} group-{k}'>\
             <p class='t{k}'>text {i}</p><span>label</span><a href='/{i}'>link</a></div>"
        ));
    }
    html.push_str("</body></html>");
    html
}

fn source() -> StyleSource {
    StyleSource {
        owner: None,
        url: url::Url::parse("https://example.test/a.css").unwrap(),
        text: rules(),
        media: None,
        disabled: false,
        imports: Vec::new(),
    }
}

fn report(label: &str, mut samples: Vec<Duration>) {
    samples.sort();
    let at = |fraction: f64| samples[((samples.len() - 1) as f64 * fraction).round() as usize];
    println!(
        "{label:<22} p50 {:7.3} ms  p95 {:7.3} ms  ({} runs)",
        at(0.5).as_secs_f64() * 1e3,
        at(0.95).as_secs_f64() * 1e3,
        samples.len()
    );
}

#[test]
#[ignore = "timing measurement; run in release"]
fn flush_stages() {
    let mut parser = Parser::new(Document::new(), ParserOptions::default());
    parser.feed_last(page().as_bytes());
    let mut doc = parser.finish();
    let mut engine = StyleEngine::new(StyleEnv::default(), Arc::new(Flat));
    engine.set_sources(vec![source()]);
    engine.flush(&doc);
    let body = doc.body().unwrap();

    let mut rebuild = Vec::new();
    for _ in 0..RUNS {
        let started = Instant::now();
        let stylist = build_stylist(&engine.env, &engine.metrics, &engine.sources, &engine.lock);
        rebuild.push(started.elapsed());
        drop(stylist);
    }

    let (mut clear, mut refresh, mut begin, mut traverse, mut total) =
        (vec![], vec![], vec![], vec![], vec![]);
    for run in 0..RUNS {
        doc.mutate(Mutation::SetAttribute {
            element: body,
            name: QualName::new(atoms::EMPTY, Atom::new("title")),
            value: run.to_string(),
        })
        .unwrap();
        let started = Instant::now();
        engine.discard_styles();
        let after_clear = Instant::now();
        engine.claim_slots(&doc);
        engine.store.refresh(&doc);
        let after_refresh = Instant::now();
        engine.atoms.begin_flush(&doc);
        let after_begin = Instant::now();
        let outcome = engine.traverse(&doc);
        let done = Instant::now();
        assert!(matches!(outcome, Some(Ok(()))));
        clear.push(after_clear - started);
        refresh.push(after_refresh - after_clear);
        begin.push(after_begin - after_refresh);
        traverse.push(done - after_begin);
        total.push(done - started);
    }
    report("stylist rebuild", rebuild);
    report("clear", clear);
    report("claim + refresh", refresh);
    report("atoms begin_flush", begin);
    report("traverse", traverse);
    report("flush total", total);
}
