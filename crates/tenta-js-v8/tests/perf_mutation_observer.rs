//! Cost of DOM mutation with and without a `MutationObserver` (release only).
//!
//! `cargo nextest run --release -p tenta-js-v8 --test perf_mutation_observer --run-ignored only --no-capture`
//!
//! Each round builds a 10,000-node subtree and then rewrites two attributes on
//! every node (30,000 mutations). The variants are interleaved round by round
//! so machine drift hits all of them equally. The numbers print; nothing
//! asserts a budget, because a timing assertion in a unit test is a flake.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_js_v8::Engine;

const ROUNDS: usize = 15;

fn fixture() -> Rc<RefCell<Document>> {
    let mut document = Document::new();
    let html = document.create_html_element(atoms::HTML);
    let body = document.create_html_element(atoms::BODY);
    let root = document.document_node();
    for (parent, child) in [(root, html), (html, body)] {
        document
            .mutate(Mutation::Append { parent, child })
            .expect("build the fixture");
    }
    Rc::new(RefCell::new(document))
}

const LOOP: &str = r#"
    globalThis.run = function (n) {
        const root = document.createElement('div');
        document.body.appendChild(root);
        %OBSERVE%
        const kids = [];
        for (let i = 0; i < n; i++) {
            const d = document.createElement('div');
            d.setAttribute('data-i', i);
            root.appendChild(d);
            kids.push(d);
        }
        for (let i = 0; i < n; i++) {
            kids[i].setAttribute('data-x', String(i));
            kids[i].className = 'c' + (i & 7);
        }
        root.remove();
    };
"#;

fn round(observe: &str) -> (Duration, Duration) {
    let mut engine = Engine::with_document(fixture());
    engine
        .evaluate(&LOOP.replace("%OBSERVE%", observe))
        .expect("install the loop");
    // One warm-up pass so the first-call costs are paid before the clock starts.
    engine.evaluate("run(200)").expect("warm up");
    engine.run_microtasks();
    let start = Instant::now();
    engine.evaluate("run(10000)").expect("run the loop");
    let mutate = start.elapsed();
    let start = Instant::now();
    engine.run_microtasks();
    let deliver = start.elapsed();
    if observe.contains("seen") {
        // 10,000 appends, then 20,000 attribute writes; the run's observer
        // resets the count.
        let seen = engine.evaluate("seen").expect("read the count");
        assert_eq!(seen, "30000", "every record reached the callback");
    }
    (mutate, deliver)
}

fn percentile(sorted: &[Duration], q: f64) -> Duration {
    sorted[((sorted.len() - 1) as f64 * q).round() as usize]
}

#[test]
#[ignore = "release-only measurement"]
fn mutation_loop_with_and_without_an_observer() {
    let variants = [
        ("none", ""),
        (
            "idle observer (detached target)",
            "globalThis.mo = new MutationObserver(() => {}); mo.observe(document.createElement('p'), {childList: true});",
        ),
        (
            "observer, subtree + attributes, callback counts records",
            "globalThis.seen = 0; globalThis.mo = new MutationObserver((r) => { seen += r.length; }); \
             mo.observe(root, {childList: true, attributes: true, subtree: true});",
        ),
    ];
    let mut samples: Vec<Vec<(Duration, Duration)>> = vec![Vec::new(); variants.len()];
    for _ in 0..ROUNDS {
        for (index, (_, observe)) in variants.iter().enumerate() {
            samples[index].push(round(observe));
        }
    }
    for ((name, _), samples) in variants.iter().zip(&samples) {
        let mut mutate: Vec<Duration> = samples.iter().map(|sample| sample.0).collect();
        let mut deliver: Vec<Duration> = samples.iter().map(|sample| sample.1).collect();
        mutate.sort();
        deliver.sort();
        println!(
            "{name}: mutate p50 {:?} p95 {:?}; deliver p50 {:?} p95 {:?}",
            percentile(&mutate, 0.5),
            percentile(&mutate, 0.95),
            percentile(&deliver, 0.5),
            percentile(&deliver, 0.95),
        );
    }
}

fn rss_kib() -> u64 {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .expect("ps runs");
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .expect("a number")
}

#[test]
#[ignore = "release-only measurement"]
fn ten_thousand_observing_observers_have_bounded_growth() {
    let mut engine = Engine::with_document(fixture());
    engine
        .evaluate("globalThis.hosts = []; for (let i = 0; i < 200; i++) hosts.push(document.createElement('p'));")
        .unwrap();
    let before = rss_kib();
    engine
        .evaluate(
            "for (let i = 0; i < 10000; i++) \
             new MutationObserver(() => {}).observe(hosts[i % 200], {attributes: true});",
        )
        .unwrap();
    let after = rss_kib();
    println!(
        "10,000 observing observers: +{} KiB RSS ({} B each)",
        after - before,
        (after - before) * 1024 / 10_000
    );
    assert!(after - before < 128 * 1024, "growth stays bounded");
}
