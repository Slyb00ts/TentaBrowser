//! S-dom-calls: warmed JavaScript calls into a real QuickJS-backed DOM document.
//!
//! Run with `cargo run --release -p tenta-js-qjs --example dom_calls_qjs`.
//! The Rust timer surrounds calls to warmed JavaScript functions. It includes
//! compilation of the small invocation expression and return conversion;
//! runtime creation, function-body compilation, and warm-up are outside it.

use std::cell::RefCell;
use std::error::Error;
use std::io;
use std::rc::Rc;
use std::time::Instant;

use tenta_dom::Document;
use tenta_js_qjs::Engine;

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
    let mut engine = Engine::with_document(document).map_err(script_error)?;
    evaluate(&mut engine, WORKLOAD)?;

    require_result(
        evaluate(&mut engine, &format!("benchHas({WARMUP_CALLS})"))?,
        WARMUP_CALLS,
        "hasAttribute warm-up",
    )?;
    require_result(
        evaluate(&mut engine, &format!("benchGet({WARMUP_CALLS})"))?,
        WARMUP_CALLS * "bench-id".len(),
        "getAttribute warm-up",
    )?;

    let has_started = Instant::now();
    let has_result = evaluate(&mut engine, &format!("benchHas({CALLS})"))?;
    let has_elapsed_ns = has_started.elapsed().as_nanos();
    require_result(has_result, CALLS, "hasAttribute")?;

    let get_started = Instant::now();
    let get_result = evaluate(&mut engine, &format!("benchGet({CALLS})"))?;
    let get_elapsed_ns = get_started.elapsed().as_nanos();
    require_result(get_result, CALLS * "bench-id".len(), "getAttribute")?;

    println!(
        "{{\"scenario\":\"S-dom-calls\",\"backend\":\"quickjs\",\"calls_per_operation\":{CALLS},\"warmup_calls_per_operation\":{WARMUP_CALLS},\"has_elapsed_ns\":{has_elapsed_ns},\"get_elapsed_ns\":{get_elapsed_ns},\"has_ns_per_call\":{},\"get_ns_per_call\":{}}}",
        has_elapsed_ns as f64 / CALLS as f64,
        get_elapsed_ns as f64 / CALLS as f64,
    );
    Ok(())
}

fn script_error(error: tenta_js::engine::ScriptError) -> io::Error {
    io::Error::other(format!("{:?}: {}", error.kind, error.message))
}

fn evaluate(engine: &mut Engine, source: &str) -> Result<String, io::Error> {
    engine.evaluate(source).map_err(script_error)
}
