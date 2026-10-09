//! S-dom-calls: warmed JavaScript calls into a real V8-backed DOM document.
//!
//! Run with `cargo run --release -p tenta-js-v8 --example dom_calls`.
//! The Rust timer surrounds calls to warmed JavaScript functions. It includes
//! compilation of the small invocation expression and return conversion;
//! isolate creation, function-body compilation, and warm-up are outside it.

use std::cell::RefCell;
use std::error::Error;
use std::io;
use std::rc::Rc;
use std::time::Instant;

use tenta_dom::Document;
use tenta_js_v8::Engine;

const CALLS: usize = 1_000_000;
const WARMUP_CALLS: usize = 10_000;

// Preserve the former raw literal's closing-line indentation byte for byte.
const WORKLOAD: &str = concat!(
    include_str!("../../../bench/workloads/dom_calls.js"),
    "        "
);

fn require_result(actual: String, expected: usize, operation: &str) -> Result<(), io::Error> {
    if actual == expected.to_string() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "{operation} returned {actual:?}; expected {expected}"
        )))
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = Engine::with_document(document);
    engine.evaluate(WORKLOAD)?;

    require_result(
        engine.evaluate(&format!("benchHas({WARMUP_CALLS})"))?,
        WARMUP_CALLS,
        "hasAttribute warm-up",
    )?;
    require_result(
        engine.evaluate(&format!("benchGet({WARMUP_CALLS})"))?,
        WARMUP_CALLS * "bench-id".len(),
        "getAttribute warm-up",
    )?;

    let has_started = Instant::now();
    let has_result = engine.evaluate(&format!("benchHas({CALLS})"))?;
    let has_elapsed_ns = has_started.elapsed().as_nanos();
    require_result(has_result, CALLS, "hasAttribute")?;

    let get_started = Instant::now();
    let get_result = engine.evaluate(&format!("benchGet({CALLS})"))?;
    let get_elapsed_ns = get_started.elapsed().as_nanos();
    require_result(get_result, CALLS * "bench-id".len(), "getAttribute")?;

    println!(
        "{{\"scenario\":\"S-dom-calls\",\"backend\":\"v8\",\"calls_per_operation\":{CALLS},\"warmup_calls_per_operation\":{WARMUP_CALLS},\"has_elapsed_ns\":{has_elapsed_ns},\"get_elapsed_ns\":{get_elapsed_ns},\"has_ns_per_call\":{},\"get_ns_per_call\":{}}}",
        has_elapsed_ns as f64 / CALLS as f64,
        get_elapsed_ns as f64 / CALLS as f64,
    );
    Ok(())
}
