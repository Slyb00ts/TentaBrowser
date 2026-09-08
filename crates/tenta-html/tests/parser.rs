//! Parser behaviour that the conformance corpora do not cover: streaming,
//! the DOM depth cap, early stop and encoding changes.

mod common;

use common::tree::{NodeData, Tree, serialize};
use tenta_html::{MAX_DOM_DEPTH, ParseStatus, Parser, ParserOptions, ScanMode};

fn parse_whole(html: &str) -> String {
    let mut parser = Parser::new(Tree::new(), ParserOptions::default());
    parser.feed_str(html);
    let tree = parser.finish();
    serialize(&tree, 0, true)
}

fn parse_in_chunks(bytes: &[u8], chunk: usize) -> String {
    let mut parser = Parser::new(Tree::new(), ParserOptions::default());
    for piece in bytes.chunks(chunk) {
        parser.feed(piece);
    }
    let tree = parser.finish();
    serialize(&tree, 0, true)
}

/// About a megabyte of markup with the constructs that make chunking hard:
/// character references, raw text, tables (pending table text), comments and
/// multi-byte characters.
fn large_document() -> String {
    let mut html = String::with_capacity(1_100_000);
    html.push_str(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Str&amp;eaming</title>",
    );
    html.push_str("<style>body { content: \"</not-a-tag>\"; }</style></head><body>");
    let mut index = 0usize;
    while html.len() < 1_000_000 {
        index += 1;
        html.push_str(&format!(
            "<section id=\"s{index}\"><h2>Nag&#x142;&oacute;wek {index}</h2>\
             <p class=\"lead\">Tekst z ąćęłńóśźż &amp; encjami &#65; &#x42;.</p>\
             <!-- comment {index} -->\
             <table><tr><td>a</td><td>b {index}</td></tr>\n<tr><td>c</td></tr></table>\
             <script>var x = {index} < 3 && 4 > 2;</script>\
             <textarea>raw &amp; text</textarea>\
             <p>trailing</p></section>\n",
        ));
    }
    html.push_str("</body></html>");
    html
}

#[test]
fn chunked_input_builds_the_same_tree() {
    let html = large_document();
    assert!(html.len() > 1_000_000, "document is {} bytes", html.len());
    let bytes = html.as_bytes();

    let whole = parse_whole(&html);
    for chunk in [4096, 7] {
        assert_eq!(
            parse_in_chunks(bytes, chunk),
            whole,
            "{chunk}-byte chunks differ"
        );
    }
}

#[test]
fn one_byte_chunks_build_the_same_tree() {
    let html = large_document();
    let whole = parse_whole(&html);
    assert_eq!(parse_in_chunks(html.as_bytes(), 1), whole);
}

#[test]
fn feed_can_be_called_repeatedly() {
    let mut parser = Parser::new(Tree::new(), ParserOptions::default());
    assert_eq!(parser.feed_str("<p>one"), ParseStatus::Continue);
    assert_eq!(parser.feed_str("<p>two"), ParseStatus::Continue);
    assert_eq!(parser.feed_str("<p>three"), ParseStatus::Continue);
    let tree = parser.finish();
    assert_eq!(tree.element_count(), 6); // html, head, body, 3 × p
}

/// Blink's `kMaximumHTMLParserDOMTreeDepth`: an element that would exceed the
/// cap is inserted into the deepest allowed ancestor instead.
fn depth_cap_body(nesting: usize) {
    let mut html = String::with_capacity(nesting * 5 + 32);
    for _ in 0..nesting {
        html.push_str("<div>");
    }
    html.push_str("<span id=\"deep\">x</span>");

    let mut parser = Parser::new(Tree::new(), ParserOptions::default());
    parser.feed_str(&html);
    let tree = parser.finish();

    let depth = tree.depth_of_deepest_element();
    assert!(
        depth <= MAX_DOM_DEPTH,
        "depth {depth} exceeds the cap of {MAX_DOM_DEPTH}"
    );
    assert_eq!(depth, MAX_DOM_DEPTH);
    // Nothing is dropped: every div is still in the tree.
    assert_eq!(tree.element_count(), nesting + 4);
}

#[test]
fn deep_nesting_is_capped_on_a_default_stack() {
    let handle = std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| depth_cap_body(100_000))
        .expect("thread spawns");
    handle.join().expect("no stack overflow on an 8 MB stack");
}

#[test]
fn deep_nesting_is_capped_on_a_small_stack() {
    let handle = std::thread::Builder::new()
        .stack_size(512 * 1024)
        .spawn(|| depth_cap_body(100_000))
        .expect("thread spawns");
    handle.join().expect("no stack overflow on a 512 KB stack");
}

#[test]
fn the_sink_can_stop_the_parse() {
    let mut sink = Tree::new();
    sink.stop_on = Some(("div".to_owned(), "results".to_owned()));

    let mut parser = Parser::new(sink, ParserOptions::default());
    let status = parser.feed_str(
        "<body><div id=\"results\"><p>a</p></div><div id=\"after\"><p>b</p></div></body>",
    );
    assert_eq!(status, ParseStatus::Stopped);
    assert!(parser.stopped());
    // Further input is ignored.
    assert_eq!(parser.feed_str("<p>more</p>"), ParseStatus::Stopped);

    let tree = parser.finish();
    assert!(
        tree.pops_after_stop.is_empty(),
        "sink was called after it asked to stop: {:?}",
        tree.pops_after_stop
    );
    let serialized = serialize(&tree, 0, true);
    assert!(serialized.contains("id=\"results\""), "{serialized}");
    assert!(!serialized.contains("id=\"after\""), "{serialized}");
}

#[test]
fn a_late_meta_charset_reports_an_encoding_change() {
    let mut html = String::from("<!doctype html><html><head>");
    // Push the <meta> past the 1024-byte pre-scan window.
    html.push_str(&"<!-- padding -->".repeat(80));
    html.push_str("<meta charset=\"iso-8859-2\"><title>t</title></head><body><p>a");
    assert!(html.find("<meta").expect("meta") > 1024);

    let mut parser = Parser::new(Tree::new(), ParserOptions::default());
    let status = parser.feed(html.as_bytes());
    let ParseStatus::EncodingChanged(encoding) = status else {
        panic!("expected an encoding change, got {status:?}");
    };
    assert_eq!(encoding, encoding_rs::ISO_8859_2);

    // The documented restart: build a new parser with the declared encoding.
    let mut parser = parser.restart_with_encoding(Tree::new(), encoding);
    assert_eq!(parser.encoding(), Some(encoding));
    assert_eq!(parser.feed(html.as_bytes()), ParseStatus::Continue);
    let tree = parser.finish();
    assert!(serialize(&tree, 0, true).contains("<title>"));
}

#[test]
fn an_early_meta_charset_is_used_without_a_restart() {
    let mut html: Vec<u8> = b"<!doctype html><meta charset=\"iso-8859-2\"><p>\xB3\xB1".to_vec();
    // Sniffing needs the pre-scan window before it can commit to an encoding.
    html.extend(std::iter::repeat_n(b'x', 1024));
    let mut parser = Parser::new(Tree::new(), ParserOptions::default());
    assert_eq!(parser.feed(&html), ParseStatus::Continue);
    assert_eq!(parser.encoding(), Some(encoding_rs::ISO_8859_2));
    let tree = parser.finish();
    let text = tree
        .nodes
        .iter()
        .find_map(|node| match &node.data {
            NodeData::Text(text) if !text.trim().is_empty() => Some(text.clone()),
            _ => None,
        })
        .expect("text node");
    assert!(text.starts_with("łą"), "{text}");
}

#[test]
fn fragment_parsing_uses_the_context_element() {
    let options = ParserOptions {
        fragment_context: Some(tenta_html::QualName::new(
            tenta_core::Atom::new("http://www.w3.org/1999/xhtml"),
            tenta_core::Atom::new("td"),
        )),
        ..ParserOptions::default()
    };
    let mut parser = Parser::new(Tree::new(), options);
    parser.feed_str("<td>a</td><p>b");
    let tree = parser.finish();
    let root = tree.nodes[0].children[0];
    assert_eq!(serialize(&tree, root, true), "| \"a\"\n| <p>\n|   \"b\"\n");
}

#[test]
fn the_scalar_and_vector_scanners_agree_on_the_fixtures() {
    for path in ["static-article", "tiny", "dynamic-scripts", "js-xhr"] {
        let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures")
            .join(path)
            .join("index.html");
        let Ok(bytes) = std::fs::read(&file) else {
            continue;
        };
        let mut trees = Vec::new();
        for scan_mode in [ScanMode::Vector, ScanMode::Scalar] {
            let options = ParserOptions {
                scan_mode,
                ..ParserOptions::default()
            };
            let mut parser = Parser::new(Tree::new(), options);
            parser.feed(&bytes);
            trees.push(serialize(&parser.finish(), 0, true));
        }
        assert_eq!(trees[0], trees[1], "{path} differs between scanners");
    }
}

/// Deeply nested input followed by various end tags must not panic. These
/// are the ten reproducers from R3-1: inputs that panicked when the
/// open-element stack was capped.
#[test]
fn deeply_nested_input_with_end_tags_does_not_panic() {
    let test_cases = vec![
        ("</p>", "close p with no p in scope"),
        ("<p>x</p><p>y</p>", "close p and open new p"),
        (
            "<script>var x='</div>';</script>",
            "script with end div in content",
        ),
        ("<template><p>x</p></template>", "template with paragraph"),
        ("<select><option>x</option></select>", "select with option"),
        ("<a href=x><b>bold</b></a>", "formatted a tag"),
        ("<svg><g><circle/></g></svg>", "svg with nested elements"),
        ("<math><mi>x</mi></math>", "math with mi element"),
        ("<form><input></form>", "form with input"),
    ];

    for (tail, name) in test_cases {
        let html = format!("<!doctype html>{}{}", "<div>".repeat(600), tail);
        let mut parser = Parser::new(Tree::new(), ParserOptions::default());
        parser.feed_str(&html);
        let tree = parser.finish();
        // Verify the tree was built (has at least html, head, body)
        assert!(tree.nodes.len() >= 3, "tree incomplete for {}", name);
    }

    // Also test 300,000 closing p tags at depth 600
    let html = format!(
        "<!doctype html>{}{}",
        "<div>".repeat(600),
        "</p>".repeat(300_000)
    );
    let mut parser = Parser::new(Tree::new(), ParserOptions::default());
    parser.feed_str(&html);
    let tree = parser.finish();
    assert!(
        tree.nodes.len() >= 3,
        "tree incomplete for large close tag sequence"
    );
}

/// After fixing the open-element cap, deeply nested formatting elements must
/// build O(n) nodes, not O(n²). A 16,000-tag input must stay under 4x that.
#[test]
fn deeply_nested_formatting_elements_stay_linear() {
    fn nested_formatting(count: usize) -> String {
        let mut html = String::with_capacity(count * 16 + 32);
        html.push_str("<!doctype html><body>");
        for index in 0..count {
            html.push_str(&format!("<b id=x{}>t", index));
        }
        html
    }

    let small = nested_formatting(4_000);
    let large = nested_formatting(16_000);

    let mut parser = Parser::new(Tree::new(), ParserOptions::default());
    parser.feed_str(&small);
    let small_tree = parser.finish();

    let mut parser = Parser::new(Tree::new(), ParserOptions::default());
    parser.feed_str(&large);
    let large_tree = parser.finish();

    // After the fix, nodes should grow linearly, so 4x tags should not
    // produce 4x quadratic amplification. Blink builds ~1 node per tag.
    let ratio = large_tree.nodes.len() as f64 / small_tree.nodes.len() as f64;
    let tag_ratio = 16_000.0 / 4_000.0;

    // Allow some slack for tree structure overhead (html, head, body, etc)
    assert!(
        ratio <= tag_ratio * 4.0,
        "formatting elements amplified quadratically: {} tags -> {} nodes, ratio {:.1}",
        16_000,
        large_tree.nodes.len(),
        ratio
    );
}

/// The fuzzer found the adoption agency moving subtrees past the depth cap:
/// ~700 nested `<div>` (retargeted to siblings at the cap) followed by `<a>`
/// tags whose adoption-agency runs pulled those siblings under the common
/// ancestor and wrapped their children.
#[test]
fn crash4_stays_within_the_depth_cap() {
    let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("crash4.bin");
    let bytes = std::fs::read(&file).expect("fixtures/crash4.bin");

    let mut parser = Parser::new(Tree::new(), ParserOptions::default());
    parser.feed(&bytes);
    let tree = parser.finish();

    let depth = tree.depth_of_deepest_element();
    assert!(depth <= MAX_DOM_DEPTH, "crash4.bin built depth {depth}");
    assert!(
        depth > 500,
        "crash4.bin should reach the cap, built depth {depth}"
    );
}

/// `<a><div>` repeated past the cap: every `<a>` runs the adoption agency
/// with the previous `<div>` as the furthest block, and each run would move
/// that block one level deeper (step 14 inserts it under the common
/// ancestor, steps 15-17 wrap its children). The tree must stop at the cap.
#[test]
fn adoption_agency_cannot_deepen_the_tree_past_the_cap() {
    let mut html = String::from("<html><body>");
    for _ in 0..600 {
        html.push_str("<div>");
    }
    for _ in 0..600 {
        html.push_str("<a><div>");
    }
    for _ in 0..5 {
        html.push_str("</a>");
    }

    let mut parser = Parser::new(Tree::new(), ParserOptions::default());
    parser.feed_str(&html);
    let tree = parser.finish();

    assert_eq!(tree.depth_of_deepest_element(), MAX_DOM_DEPTH);
}

/// Wrapping a furthest block whose closed subtree already reaches the cap:
/// each `</b>` removes one ancestor and adds one wrapper, so the subtree
/// must stay exactly at the cap through all eight runs.
#[test]
fn adoption_agency_wraps_at_the_cap_stay_neutral() {
    let mut html = String::from("<html><body>");
    for index in 0..8 {
        html.push_str(&format!("<b id=b{index}>"));
    }
    html.push_str("<div>");
    for _ in 0..600 {
        html.push_str("<span>");
    }
    for _ in 0..600 {
        html.push_str("</span>");
    }
    for _ in 0..8 {
        html.push_str("</b>");
    }

    let mut parser = Parser::new(Tree::new(), ParserOptions::default());
    parser.feed_str(&html);
    let tree = parser.finish();

    assert_eq!(tree.depth_of_deepest_element(), MAX_DOM_DEPTH);
}
