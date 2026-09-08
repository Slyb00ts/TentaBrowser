//! Parse timing into the arena `Document`
//! (`docs/spec/04-dom-and-css.md` §8: ≤ 6 ms per megabyte, single-threaded).
//!
//! `tenta-html`'s own bench parses into the owned test tree, which measures
//! the tokenizer and tree builder alone. This one measures what the product
//! does: the parser writing straight into the arena, with the id, class and
//! tag indexes and the text rope in the loop.
//!
//! Run with `cargo bench -p tenta-dom`.

use std::time::{Duration, Instant};

use tenta_dom::Document;
use tenta_html::{Parser, ParserOptions, ScanMode};

fn percentile(samples: &[Duration], fraction: f64) -> Duration {
    let index = ((samples.len() - 1) as f64 * fraction).round() as usize;
    samples[index]
}

fn measure(name: &str, bytes: usize, iterations: usize, mut body: impl FnMut()) {
    // Warm up: first-run atom interning is not what we are measuring.
    body();
    let mut samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let start = Instant::now();
        body();
        samples.push(start.elapsed());
    }
    samples.sort_unstable();
    let p50 = percentile(&samples, 0.50);
    let budget = Duration::from_secs_f64(6e-3 * bytes as f64 / (1024.0 * 1024.0));
    println!(
        "{name:<44} p50 {:>8.3} ms   p95 {:>8.3} ms   {:>6.1} MB/s   budget {:>7.3} ms",
        p50.as_secs_f64() * 1e3,
        percentile(&samples, 0.95).as_secs_f64() * 1e3,
        bytes as f64 / 1e6 / p50.as_secs_f64(),
        budget.as_secs_f64() * 1e3,
    );
}

fn parse(bytes: &[u8], scan_mode: ScanMode) -> Document {
    let options = ParserOptions {
        scan_mode,
        ..ParserOptions::default()
    };
    let mut parser = Parser::new(Document::new(), options);
    parser.feed(bytes);
    parser.finish()
}

/// A `<pre>` of CRLF-terminated lines: one text node grown from thousands of
/// pieces, which is the append-heavy shape the rope has to handle.
fn crlf_pre(lines: usize) -> Vec<u8> {
    let mut html = String::with_capacity(lines * 40 + 64);
    html.push_str("<!doctype html><html><body><pre>");
    for index in 0..lines {
        html.push_str(&format!("line {index:06} of preformatted text\r\n"));
    }
    html.push_str("</pre></body></html>");
    html.into_bytes()
}

/// Text split around a character reference every few bytes.
fn entity_dense(entities: usize) -> Vec<u8> {
    let mut html = String::with_capacity(entities * 16 + 64);
    html.push_str("<!doctype html><html><body><p>");
    for index in 0..entities {
        html.push_str(&format!("a{index}&amp;b&#x41;c&nbsp;"));
    }
    html.push_str("</p></body></html>");
    html.into_bytes()
}

fn main() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/static-article/index.html");
    let bytes = std::fs::read(&path).expect("the static-article fixture");
    let document = parse(&bytes, ScanMode::Vector);
    println!(
        "static-article: {} bytes, {} nodes",
        bytes.len(),
        document.len()
    );
    drop(document);

    measure("static-article into Document", bytes.len(), 20, || {
        std::hint::black_box(parse(&bytes, ScanMode::Vector));
    });
    measure(
        "static-article into Document (scalar scanner)",
        bytes.len(),
        20,
        || {
            std::hint::black_box(parse(&bytes, ScanMode::Scalar));
        },
    );

    let pre = crlf_pre(14_093);
    println!("\nappend-heavy shapes");
    measure("CRLF <pre>, one grown text node", pre.len(), 20, || {
        std::hint::black_box(parse(&pre, ScanMode::Vector));
    });
    measure("CRLF <pre> (scalar scanner)", pre.len(), 20, || {
        std::hint::black_box(parse(&pre, ScanMode::Scalar));
    });
    measure("CRLF <pre>, fed one byte at a time", pre.len(), 5, || {
        let mut parser = Parser::new(Document::new(), ParserOptions::default());
        for chunk in pre.chunks(1) {
            parser.feed(chunk);
        }
        std::hint::black_box(parser.finish());
    });
    measure("CRLF <pre>, fed in 4 KB chunks", pre.len(), 20, || {
        let mut parser = Parser::new(Document::new(), ParserOptions::default());
        for chunk in pre.chunks(4096) {
            parser.feed(chunk);
        }
        std::hint::black_box(parser.finish());
    });

    let entities = entity_dense(24_000);
    measure("entity-dense text", entities.len(), 20, || {
        std::hint::black_box(parse(&entities, ScanMode::Vector));
    });
    measure(
        "entity-dense text (scalar scanner)",
        entities.len(),
        20,
        || {
            std::hint::black_box(parse(&entities, ScanMode::Scalar));
        },
    );
}
