//! Shared parser ownership cost, including parser and document teardown.

use std::error::Error;
use std::hint::black_box;
use std::rc::Rc;
use std::time::Instant;

use tenta_dom::SharedSink;
use tenta_html::{ParseStatus, Parser, ParserOptions};

const ITERATIONS: usize = 8;
const WARMUPS: usize = 2;
const CHUNKED_TEXT_BYTES: usize = 256 * 1024;
const FEED_CHUNK_BYTES: usize = 128;
const SOURCE_PREFIX: &str = "<!doctype html><html><head></head><body>";
const SOURCE_SUFFIX: &str = "</body></html>";

pub fn run() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<_> = std::env::args().collect();
    if arguments.len() != 3 || arguments[1] != "--shape" {
        return Err("expected --shape flat|template|deep|chunked-text".into());
    }
    let shape = arguments[2].as_str();
    let mut source = String::from(SOURCE_PREFIX);
    let mut chunked_text = None;
    let expected_nodes = match shape {
        "flat" => {
            for index in 0..8192 {
                source.push_str(&format!("<div data-index='{index}'>text</div>"));
            }
            5 + 8192 * 2
        }
        "template" => {
            for _ in 0..1024 {
                source.push_str("<template><span>text</span></template>");
            }
            5 + 1024 * 4
        }
        "deep" => {
            for _ in 0..1024 {
                source.push_str("<div>");
            }
            source.push_str("text");
            for _ in 0..1024 {
                source.push_str("</div>");
            }
            5 + 1024 + 1
        }
        "chunked-text" => {
            let text = "x".repeat(CHUNKED_TEXT_BYTES);
            source.push_str(&text);
            chunked_text = Some(text);
            6
        }
        _ => return Err("shape must be flat, template, deep or chunked-text".into()),
    };
    source.push_str(SOURCE_SUFFIX);
    let parse = |validate_chunked_text: bool| -> Result<(), Box<dyn Error>> {
        let (sink, document) = SharedSink::with_new_document();
        let mut parser = Parser::new(sink, ParserOptions::default());
        if let Some(text) = &chunked_text {
            if parser.feed(SOURCE_PREFIX.as_bytes()) != ParseStatus::Continue {
                return Err("generated benchmark prefix did not parse normally".into());
            }
            for chunk in text.as_bytes().chunks(FEED_CHUNK_BYTES) {
                if parser.feed(chunk) != ParseStatus::Continue {
                    return Err("generated benchmark text chunk did not parse normally".into());
                }
            }
            if parser.feed_last(SOURCE_SUFFIX.as_bytes()) != ParseStatus::Continue {
                return Err("generated benchmark suffix did not finish normally".into());
            }
        } else if parser.feed_last(source.as_bytes()) != ParseStatus::Continue {
            return Err("generated benchmark source did not finish normally".into());
        }
        let finished = parser.finish();
        if !Rc::ptr_eq(&document, &finished) || finished.borrow().len() != expected_nodes {
            return Err("shared parser returned an unexpected document or node count".into());
        }
        if validate_chunked_text {
            let expected_text = chunked_text
                .as_deref()
                .ok_or("chunked text validation requested for another shape")?;
            let body = finished
                .borrow()
                .body()
                .ok_or("chunked text document has no body element")?;
            let actual_text = finished.borrow().text_content(body);
            if actual_text.len() != CHUNKED_TEXT_BYTES || actual_text.as_str() != expected_text {
                return Err("chunked text document has unexpected body text".into());
            }
        }
        black_box(&finished);
        drop(finished);
        if Rc::strong_count(&document) != 1 {
            return Err("parser retained document ownership after finish".into());
        }
        drop(document);
        Ok(())
    };
    if chunked_text.is_some() {
        // Check the actual Document API outside the timed region so text extraction
        // does not hide the incremental parser and text-node append costs.
        parse(true)?;
    }
    for _ in 0..WARMUPS {
        parse(false)?;
    }
    let start = Instant::now();
    for _ in 0..ITERATIONS {
        parse(false)?;
    }
    let elapsed_ns = start.elapsed().as_nanos();
    println!(
        "{}",
        serde_json::json!({
            "scenario": "S-shared-parse",
            "shape": shape,
            "source_bytes": source.len(),
            "nodes": expected_nodes,
            "iterations": ITERATIONS,
            "warmups": WARMUPS,
            "elapsed_ns": elapsed_ns,
        })
    );
    Ok(())
}
