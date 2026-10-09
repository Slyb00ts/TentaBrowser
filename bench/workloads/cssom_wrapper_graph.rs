//! Shared real-engine CSS wrapper and collection workload.
//!
//! Output checks native CSSOM values on both the red baseline and candidate.
//! Wrapper expando preservation is asserted by the separate behavioral tests.

use std::cell::RefCell;
use std::error::Error;
use std::io;
use std::rc::Rc;
use std::time::{Duration, Instant};

use tenta_dom::{Document, SharedDocument};
use tenta_js::engine::{PageEngine, ScriptKind};

const WORKLOAD: &str = include_str!("cssom_wrapper_graph.js");
const RULES: usize = 64;
const READ_CALLS: usize = 8_192;
const READ_WARMUP: usize = 128;
const MUTATION_PAIRS: usize = 256;
const TRIM_ROUNDS: usize = 3;

fn healthy<E: PageEngine>(engine: &E) -> Result<(), io::Error> {
    if let Some(boundary) = engine.native_failure() {
        return Err(io::Error::other(format!(
            "CSS wrapper graph failed at native boundary {boundary}"
        )));
    }
    Ok(())
}

fn evaluate<E: PageEngine>(engine: &mut E, source: &str) -> Result<String, io::Error> {
    let result = engine
        .evaluate(source, Duration::from_secs(30))
        .map_err(|error| io::Error::other(format!("{:?}: {}", error.kind, error.message)))?;
    healthy(engine)?;
    Ok(result)
}

fn require(actual: &str, expected: &str) -> Result<(), io::Error> {
    if actual != expected {
        return Err(io::Error::other(format!(
            "CSS wrapper graph returned {actual:?}; expected {expected:?}"
        )));
    }
    Ok(())
}

pub fn run<E: PageEngine>(
    backend: &str,
    make_engine: impl FnOnce(SharedDocument) -> Result<E, io::Error>,
) -> Result<(), Box<dyn Error>> {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = make_engine(document)?;
    engine
        .run_script(WORKLOAD, ScriptKind::Classic)
        .map_err(|error| io::Error::other(format!("{:?}: {}", error.kind, error.message)))?;
    healthy(&engine)?;
    require(
        &evaluate(&mut engine, "setupCssWrapperGraph()")?,
        &(RULES * 3).to_string(),
    )?;
    require(
        &evaluate(&mut engine, &format!("readCssWrapperGraph({READ_WARMUP})"))?,
        &(READ_WARMUP * 3).to_string(),
    )?;

    let started = Instant::now();
    let read_checksum = evaluate(&mut engine, &format!("readCssWrapperGraph({READ_CALLS})"))?;
    let read_elapsed_ns = started.elapsed().as_nanos();
    require(&read_checksum, &(READ_CALLS * 3).to_string())?;

    let started = Instant::now();
    let mutation_checksum = evaluate(
        &mut engine,
        &format!("churnCssWrapperGraph({MUTATION_PAIRS})"),
    )?;
    let mutation_elapsed_ns = started.elapsed().as_nanos();
    require(&mutation_checksum, &(MUTATION_PAIRS * 4).to_string())?;
    require(&evaluate(&mut engine, "verifyCssWrapperGraph()")?, "ok")?;

    PageEngine::run_microtasks(&mut engine);
    healthy(&engine)?;
    let gc_epoch_before = engine.gc_epoch();
    let started = Instant::now();
    for _ in 0..TRIM_ROUNDS {
        engine.trim_memory();
        healthy(&engine)?;
    }
    let trim_elapsed_ns = started.elapsed().as_nanos();
    let gc_epoch_after = engine.gc_epoch();
    if gc_epoch_after.saturating_sub(gc_epoch_before) < TRIM_ROUNDS as u64 {
        return Err(io::Error::other(format!(
            "CSS wrapper graph ran {TRIM_ROUNDS} trim rounds but advanced only {} GC epochs",
            gc_epoch_after.saturating_sub(gc_epoch_before)
        ))
        .into());
    }
    require(&evaluate(&mut engine, "verifyCssWrapperGraph()")?, "ok")?;
    println!(
        "{{\"scenario\":\"S-cssom-wrapper-graph\",\"backend\":\"{backend}\",\"rules\":{RULES},\"read_warmup_calls\":{READ_WARMUP},\"read_calls\":{READ_CALLS},\"mutation_pairs\":{MUTATION_PAIRS},\"trim_rounds\":{TRIM_ROUNDS},\"read_elapsed_ns\":{read_elapsed_ns},\"mutation_elapsed_ns\":{mutation_elapsed_ns},\"trim_elapsed_ns\":{trim_elapsed_ns},\"read_checksum\":{read_checksum},\"mutation_checksum\":{mutation_checksum},\"gc_epoch_before\":{gc_epoch_before},\"gc_epoch_after\":{gc_epoch_after}}}"
    );
    Ok(())
}
