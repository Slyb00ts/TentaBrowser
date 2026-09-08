//! Growth of a single text node (`docs/spec/04-dom-and-css.md` §2, §8).
//!
//! Source text reaches one node in pieces: `\r\n` normalisation flushes a
//! segment per CR, every character reference splits the run around it, and
//! every `feed()` boundary splits it again. Appending is only free while the
//! node's bytes are the tail of the rope's open buffer, so a node left behind
//! by a seal is promoted to its own growable buffer once and grows in place
//! from then on (`crates/tenta-dom/src/text.rs`).
//!
//! The scaling tests run in the normal suite. Each of them parses the same
//! bytes twice, once with the text spread over several nodes and once with it
//! all in one, and asserts the two cost the same: that is exactly the property
//! promotion buys, and it holds whatever the machine is doing. The absolute
//! 6 ms/MB budget of §8 is asserted separately, in the `#[ignore]`d release
//! tests.

mod support;

use std::time::{Duration, Instant};

use tenta_dom::Document;
use tenta_html::{ParseStatus, Parser, ParserOptions};

fn parse_bytes(bytes: &[u8]) -> Document {
    let mut parser = Parser::new(Document::new(), ParserOptions::default());
    assert_eq!(parser.feed(bytes), ParseStatus::Continue);
    parser.finish()
}

fn parse_in_chunks(bytes: &[u8], chunk: usize) -> Document {
    let mut parser = Parser::new(Document::new(), ParserOptions::default());
    for piece in bytes.chunks(chunk) {
        assert_eq!(parser.feed(piece), ParseStatus::Continue);
    }
    parser.finish()
}

/// Cost of the two documents, measured alternately, best of three rounds.
///
/// Both documents hold the same bytes and produce the same number of parser
/// tokens; they differ only in how many text nodes those bytes land in. That
/// makes the comparison a ratio of two measurements of the same amount of
/// work, taken in the same window on the same machine, so a loaded machine
/// slows both equally — which matters, because these run in the normal suite
/// alongside every other test binary.
fn best_pair(mut split: impl FnMut(), mut single: impl FnMut()) -> (Duration, Duration) {
    let (mut best_split, mut best_single) = (Duration::MAX, Duration::MAX);
    for _ in 0..3 {
        let start = Instant::now();
        split();
        best_split = best_split.min(start.elapsed());
        let start = Instant::now();
        single();
        best_single = best_single.min(start.elapsed());
    }
    (best_split, best_single)
}

/// Asserts that gathering the same bytes into one text node instead of
/// `blocks` of them costs no more than `slack` times as much.
///
/// Appending to a text node has to be O(1) amortised in the node's length, so
/// the answer is 1.0 however long the node is. An append that copies the node
/// forward makes it `blocks`, because each block is then quadratic in its own
/// length and there are `blocks` times fewer of them. The slack covers the
/// cache and allocator noise of a loaded machine and still leaves the two
/// answers far apart.
fn assert_append_is_flat(name: &str, split: Duration, single: Duration, blocks: u32, slack: f64) {
    let allowed = split.mul_f64(slack) + Duration::from_millis(5);
    println!(
        "{name}: {blocks} nodes {split:?}, 1 node {single:?} \
         (ratio {:.2}, allowed {allowed:?}, a copying append would be {blocks}.00)",
        single.as_secs_f64() / split.as_secs_f64()
    );
    assert!(
        single <= allowed,
        "{name} took {single:?} with the bytes in one text node against {split:?} with the \
         same bytes in {blocks}, so appending is not O(1) amortised (allowed {allowed:?})"
    );
}

/// `lines` CRLF-terminated lines spread over `blocks` `<pre>` elements, so
/// the tokenizer flushes a separate text run per line and each block's runs
/// coalesce into one text node.
fn crlf_pre_blocks(lines: usize, blocks: usize) -> Vec<u8> {
    let mut html = String::with_capacity(lines * 40 + blocks * 16 + 64);
    html.push_str("<!doctype html><html><body>");
    let per_block = lines / blocks;
    for block in 0..blocks {
        html.push_str("<pre>");
        for index in 0..per_block {
            let index = block * per_block + index;
            html.push_str(&format!("line {index:06} of preformatted text\r\n"));
        }
        html.push_str("</pre>");
    }
    html.push_str("</body></html>");
    html.into_bytes()
}

/// One `<pre>` holding all `lines`.
fn crlf_pre(lines: usize) -> Vec<u8> {
    crlf_pre_blocks(lines, 1)
}

/// `entities` runs, each split around three character references, spread over
/// `blocks` paragraphs.
fn entity_dense_blocks(entities: usize, blocks: usize) -> Vec<u8> {
    let mut html = String::with_capacity(entities * 16 + blocks * 8 + 64);
    html.push_str("<!doctype html><html><body>");
    let per_block = entities / blocks;
    for block in 0..blocks {
        html.push_str("<p>");
        for index in 0..per_block {
            let index = block * per_block + index;
            html.push_str(&format!("a{index}&amp;b&#x41;c&nbsp;"));
        }
        html.push_str("</p>");
    }
    html.push_str("</body></html>");
    html.into_bytes()
}

/// One paragraph holding all `entities`.
fn entity_dense(entities: usize) -> Vec<u8> {
    entity_dense_blocks(entities, 1)
}

#[test]
fn a_crlf_preformatted_block_keeps_its_lines_and_grows_linearly() {
    const LINES: usize = 8_000;
    const BLOCKS: usize = 8;
    let split = crlf_pre_blocks(LINES, BLOCKS);
    let single = crlf_pre(LINES);
    assert!(
        split.len().abs_diff(single.len()) < 128,
        "the two documents must hold the same bytes"
    );

    let document = parse_bytes(&single);
    let pre = document
        .query_selector(document.document_node(), "pre")
        .unwrap()
        .expect("the <pre>");
    let text = document.text_content(pre);
    assert_eq!(
        text.lines().count(),
        LINES,
        "every line survives normalisation"
    );
    assert!(!text.contains('\r'), "CRLF is normalised to LF");
    assert!(text.starts_with("line 000000 of preformatted text\n"));
    assert!(text.ends_with("line 007999 of preformatted text\n"));
    assert_eq!(
        document
            .children(pre)
            .filter(|node| document.text_data(*node).is_some())
            .count(),
        1,
        "the lines coalesce into one text node, which is what has to grow"
    );
    let split_document = parse_bytes(&split);
    assert_eq!(
        split_document.text_content(split_document.body().expect("body")),
        document.text_content(document.body().expect("body")),
        "the split document holds the same characters"
    );

    // Warm the atom table and the allocator.
    parse_bytes(&split);
    parse_bytes(&single);
    let (split_time, single_time) = best_pair(
        || {
            std::hint::black_box(parse_bytes(&split));
        },
        || {
            std::hint::black_box(parse_bytes(&single));
        },
    );
    assert_append_is_flat("CRLF <pre>", split_time, single_time, BLOCKS as u32, 3.0);
}

#[test]
fn entity_dense_text_grows_linearly() {
    const ENTITIES: usize = 8_000;
    const BLOCKS: usize = 8;
    let split = entity_dense_blocks(ENTITIES, BLOCKS);
    let single = entity_dense(ENTITIES);

    let document = parse_bytes(&single);
    let paragraph = document
        .query_selector(document.document_node(), "p")
        .unwrap()
        .expect("the <p>");
    let text = document.text_content(paragraph);
    assert_eq!(
        text.matches('&').count(),
        ENTITIES,
        "&amp; decoded once each"
    );
    assert_eq!(text.matches('A').count(), ENTITIES, "&#x41; decoded");
    assert_eq!(text.matches('\u{00A0}').count(), ENTITIES, "&nbsp; decoded");
    let split_document = parse_bytes(&split);
    assert_eq!(
        split_document.text_content(split_document.body().expect("body")),
        document.text_content(document.body().expect("body")),
        "the split document holds the same characters"
    );

    parse_bytes(&split);
    parse_bytes(&single);
    let (split_time, single_time) = best_pair(
        || {
            std::hint::black_box(parse_bytes(&split));
        },
        || {
            std::hint::black_box(parse_bytes(&single));
        },
    );
    assert_append_is_flat(
        "entity-dense text",
        split_time,
        single_time,
        BLOCKS as u32,
        3.0,
    );
}

#[test]
fn feeding_one_byte_at_a_time_builds_the_same_tree() {
    const LINES: usize = 2_000;
    let single = crlf_pre(LINES);

    let streamed = parse_in_chunks(&single, 1);
    let whole = parse_bytes(&single);
    assert_eq!(
        streamed.outer_html(streamed.document_element().expect("root")),
        whole.outer_html(whole.document_element().expect("root")),
        "chunking must not change the tree"
    );
}

#[test]
#[ignore = "budget measurement"]
fn feeding_one_byte_at_a_time_stays_linear() {
    const LINES: usize = 2_000;
    const BLOCKS: usize = 8;
    let split = crlf_pre_blocks(LINES, BLOCKS);
    let single = crlf_pre(LINES);

    parse_in_chunks(&split, 1);
    parse_in_chunks(&single, 1);
    let (split_time, single_time) = best_pair(
        || {
            std::hint::black_box(parse_in_chunks(&split, 1));
        },
        || {
            std::hint::black_box(parse_in_chunks(&single, 1));
        },
    );
    assert_append_is_flat(
        "1-byte feeding",
        split_time,
        single_time,
        BLOCKS as u32,
        3.0,
    );
}

#[test]
fn a_grown_text_node_is_promoted_at_most_once() {
    // Enough text to cross several 4 KB chunk seals inside one node.
    let document = parse_bytes(&crlf_pre(4_000));
    let (chunks, written, _open) = document.rope_stats();
    let bytes = document.text().len();
    println!(
        "{bytes} bytes of text, {chunks} sealed chunks, {written} bytes written, \
         {} promoted nodes",
        document.promoted_text_nodes()
    );
    assert!(
        written < bytes * 3,
        "{written} bytes written for {bytes} bytes of text: a node is being recopied"
    );
}

#[test]
fn text_bytes_report_live_bytes_not_the_bytes_ever_written() {
    let mut document = parse_bytes(&crlf_pre(4_000));
    let report = document.memory_report();
    assert!(
        report.text_bytes_written >= report.text_bytes,
        "the write counter includes copies the document no longer holds"
    );

    let pre = document
        .query_selector(document.document_node(), "pre")
        .unwrap()
        .expect("the <pre>");
    let before = document.memory_report();
    let text_node = document
        .children(pre)
        .find(|node| document.text_data(*node).is_some())
        .expect("the text node");
    document
        .mutate(tenta_dom::Mutation::Remove { child: text_node })
        .expect("remove");
    document.sweep_detached();
    let after = document.memory_report();
    assert!(
        after.text_bytes < before.text_bytes,
        "dropping the text must drop the live byte count ({} -> {})",
        before.text_bytes,
        after.text_bytes
    );
    assert!(
        after.text_bytes_written >= before.text_bytes_written,
        "the write counter never shrinks"
    );
    assert_eq!(
        after.total(),
        after.node_columns
            + after.element_columns
            + after.attributes
            + after.attribute_values
            + after.classes
            + after.text_columns
            + after.text_bytes
            + after.names
            + after.indexes,
        "bytes ever written must not be part of the memory total"
    );
}

// ------------------------------------------------------- release budgets

/// The §8 parse budget for a document of `bytes` bytes: 6 ms per megabyte.
fn budget_for(bytes: usize) -> Duration {
    Duration::from_secs_f64(6e-3 * bytes as f64 / (1024.0 * 1024.0))
}

/// The fastest of `runs` measurements. Parsing is deterministic, so the
/// minimum is the measurement and everything above it is interference from
/// the rest of the machine.
fn best_of(runs: usize, mut body: impl FnMut()) -> Duration {
    body();
    (0..runs)
        .map(|_| {
            let start = Instant::now();
            body();
            start.elapsed()
        })
        .min()
        .expect("one run")
}

fn profile() -> &'static str {
    if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    }
}

/// A `<pre>` of roughly 468 KB of CRLF-terminated lines.
fn large_crlf_pre() -> Vec<u8> {
    let bytes = crlf_pre(14_093);
    let kib = bytes.len() / 1024;
    assert!((464..472).contains(&kib), "fixture is {kib} KiB");
    bytes
}

#[test]
#[ignore = "budget measurement; run with --release"]
fn crlf_preformatted_block_meets_the_parse_budget() {
    // 468 KB of text arriving as 14 093 separate runs, all of them one text
    // node. This is the shape the promotion rule exists for, and it is
    // ordinary document text, so it is held to the §8 budget.
    let bytes = large_crlf_pre();
    let elapsed = best_of(10, || {
        std::hint::black_box(parse_bytes(&bytes));
    });
    let budget = budget_for(bytes.len());
    println!(
        "468 KB CRLF <pre> into Document: {elapsed:?} (04 §8 budget {budget:?}, {} profile)",
        profile()
    );
    #[cfg(not(debug_assertions))]
    assert!(
        elapsed <= budget,
        "the CRLF <pre> fixture took {elapsed:?}, over the {budget:?} budget of 04 §8"
    );
}

/// What a token may cost once the input stops being ordinary prose.
///
/// The §8 budget is denominated in bytes and assumes a document's markup
/// density is that of the reference fixture (`tests/fixtures/static-article`,
/// about one token per 25 bytes). The two fixtures below are deliberately
/// denser than any real page — a character reference every four bytes, and one
/// `feed()` call per byte — so a byte budget says nothing useful about them;
/// what has to hold is that the per-token cost stays flat.
///
/// The ceiling is generous because these run alongside the rest of the release
/// gate and the measurement moves by a factor of three with the load on the
/// machine. It is a sanity bound, not the number to read: the numbers to read
/// are printed, and the guard against the quadratic append and the quadratic
/// scan is the pair of linearity tests above, which compare two workloads
/// measured in the same window and so do not depend on the load at all. On an
/// idle machine both fixtures measure around 90 ns per token.
const NANOS_PER_TOKEN_CEILING: u128 = 400;
const NANOS_PER_FEED_CEILING: u128 = 400;

fn report_per_token(name: &str, tokens: usize, bytes: usize, elapsed: Duration, ceiling: u128) {
    let per_token = elapsed.as_nanos() / tokens as u128;
    let budget = budget_for(bytes);
    println!(
        "{name}: {elapsed:?} for {tokens} tokens over {} KiB = {per_token} ns/token \
         (ceiling {ceiling} ns; 04 §8 would allow {budget:?} for this many bytes, {} profile)",
        bytes / 1024,
        profile()
    );
    #[cfg(not(debug_assertions))]
    assert!(
        per_token <= ceiling,
        "{name} cost {per_token} ns per token, over the {ceiling} ns ceiling"
    );
}

#[test]
#[ignore = "budget measurement; run with --release"]
fn entity_dense_text_stays_flat_per_token() {
    const ENTITIES: usize = 24_000;
    let bytes = entity_dense(ENTITIES);
    let elapsed = best_of(10, || {
        std::hint::black_box(parse_bytes(&bytes));
    });
    // Three character references and three text runs per iteration.
    report_per_token(
        "entity-dense text into Document",
        ENTITIES * 6,
        bytes.len(),
        elapsed,
        NANOS_PER_TOKEN_CEILING,
    );
}

#[test]
#[ignore = "budget measurement; run with --release"]
fn one_byte_feeding_stays_flat_per_call() {
    let bytes = large_crlf_pre();
    // Half a million calls into the parser is the most contention-sensitive
    // measurement in the file, so it takes the best of more rounds.
    let elapsed = best_of(15, || {
        std::hint::black_box(parse_in_chunks(&bytes, 1));
    });
    report_per_token(
        "468 KB fed one byte at a time",
        bytes.len(),
        bytes.len(),
        elapsed,
        NANOS_PER_FEED_CEILING,
    );
}

#[test]
#[ignore = "budget measurement; run with --release"]
fn chunked_feeding_meets_the_parse_budget() {
    // The chunk size the network layer actually delivers.
    let bytes = large_crlf_pre();
    let elapsed = best_of(10, || {
        std::hint::black_box(parse_in_chunks(&bytes, 4096));
    });
    let budget = budget_for(bytes.len());
    println!(
        "468 KB fed in 4 KB chunks: {elapsed:?} (04 §8 budget {budget:?}, {} profile)",
        profile()
    );
    #[cfg(not(debug_assertions))]
    assert!(
        elapsed <= budget,
        "chunked feeding took {elapsed:?}, over the {budget:?} budget of 04 §8"
    );
}
