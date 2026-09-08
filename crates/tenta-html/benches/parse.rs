//! Parse timing for the 1 MB static-article fixture
//! (`docs/spec/04-dom-and-css.md` §8: ≤ 6 ms single-threaded).
//!
//! Run with `cargo bench -p tenta-html`. The tree is the owned test tree, not
//! the arena, so the number is an upper bound on the parser's own cost.

#[path = "../tests/common/tree.rs"]
mod tree;

use std::time::{Duration, Instant};

use tenta_html::{Parser, ParserOptions, PreloadScanner, ScanMode};
use tree::Tree;

fn percentile(samples: &[Duration], fraction: f64) -> Duration {
    let index = ((samples.len() - 1) as f64 * fraction).round() as usize;
    samples[index]
}

fn measure(name: &str, iterations: usize, mut body: impl FnMut()) {
    // Warm up: first-run atom interning is not what we are measuring.
    body();
    let mut samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let start = Instant::now();
        body();
        samples.push(start.elapsed());
    }
    samples.sort_unstable();
    println!(
        "{name:<40} p50 {:>8.3} ms   p95 {:>8.3} ms",
        percentile(&samples, 0.50).as_secs_f64() * 1e3,
        percentile(&samples, 0.95).as_secs_f64() * 1e3,
    );
}

fn main() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/static-article/index.html");
    let bytes = std::fs::read(&path).expect("the static-article fixture");
    let text = String::from_utf8(bytes.clone()).expect("UTF-8 fixture");
    println!("document: {} bytes", bytes.len());

    measure("parse 1 MB (bytes, sniffed)", 20, || {
        let mut parser = Parser::new(Tree::new(), ParserOptions::default());
        parser.feed(&bytes);
        std::hint::black_box(parser.finish());
    });

    measure("parse 1 MB (decoded text)", 20, || {
        let mut parser = Parser::new(Tree::new(), ParserOptions::default());
        parser.feed_str(&text);
        std::hint::black_box(parser.finish());
    });

    measure("parse 1 MB (scalar scanner)", 20, || {
        let options = ParserOptions {
            scan_mode: ScanMode::Scalar,
            ..ParserOptions::default()
        };
        let mut parser = Parser::new(Tree::new(), options);
        parser.feed_str(&text);
        std::hint::black_box(parser.finish());
    });

    measure("parse 1 MB + preload scanner", 20, || {
        let mut hints = 0usize;
        let options = ParserOptions {
            preload: Some(Box::new(|_| hints += 1)),
            ..ParserOptions::default()
        };
        let mut parser = Parser::new(Tree::new(), options);
        parser.feed_str(&text);
        std::hint::black_box(parser.finish());
        std::hint::black_box(hints);
    });

    measure("preload scan 1 MB", 20, || {
        std::hint::black_box(PreloadScanner::scan(&text, true));
    });

    measure("parse 1 MB in 4 KB chunks", 20, || {
        let mut parser = Parser::new(Tree::new(), ParserOptions::default());
        for chunk in bytes.chunks(4096) {
            parser.feed(chunk);
        }
        std::hint::black_box(parser.finish());
    });
}
