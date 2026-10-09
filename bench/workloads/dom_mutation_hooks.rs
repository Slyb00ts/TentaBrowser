//! Shared real-engine workload for native DOM mutation routing without observers.
//!
//! Both subjects execute this exact script. Task-end collection is included in
//! each timed evaluation; engine setup, warm-up, final checks, and teardown are not.

use std::cell::RefCell;
use std::error::Error;
use std::io;
use std::rc::Rc;
use std::time::{Duration, Instant};

use tenta_dom::{Document, SharedDocument};
use tenta_js::engine::{PageEngine, ScriptError, ScriptKind};

const WORKLOAD: &str = include_str!("dom_mutation_hooks.js");
const WARMUP_ITERATIONS: usize = 128;
const ITERATIONS: usize = 8_192;
const EXPECTED_NODES: usize = 9;
const MUTATIONS_PER_ITERATION: usize = 5;

fn script_error(error: ScriptError) -> io::Error {
    io::Error::other(format!("{:?}: {}", error.kind, error.message))
}

fn healthy<E: PageEngine>(engine: &E) -> Result<(), io::Error> {
    if let Some(boundary) = engine.native_failure() {
        return Err(io::Error::other(format!(
            "DOM mutation workload failed at native boundary {boundary}"
        )));
    }
    Ok(())
}

fn evaluate<E: PageEngine>(engine: &mut E, source: &str) -> Result<String, io::Error> {
    let result = engine
        .evaluate(source, Duration::from_secs(30))
        .map_err(script_error)?;
    healthy(engine)?;
    Ok(result)
}

fn require(actual: &str, expected: &str, phase: &str) -> Result<(), io::Error> {
    if actual != expected {
        return Err(io::Error::other(format!(
            "{phase} returned {actual:?}; expected {expected:?}"
        )));
    }
    Ok(())
}

pub fn run<E: PageEngine>(
    backend: &str,
    make_engine: impl FnOnce(SharedDocument) -> Result<E, io::Error>,
) -> Result<(), Box<dyn Error>> {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = make_engine(document.clone())?;
    engine
        .run_script(WORKLOAD, ScriptKind::Classic)
        .map_err(script_error)?;
    healthy(&engine)?;
    require(
        &evaluate(&mut engine, "setupDomMutationHooks()")?,
        "ready",
        "setup",
    )?;
    let nodes = document.borrow().len();
    if nodes != EXPECTED_NODES {
        return Err(io::Error::other(format!(
            "setup created {nodes} native nodes; expected {EXPECTED_NODES}"
        ))
        .into());
    }

    let warmup_checksum = (WARMUP_ITERATIONS * 5).to_string();
    for mode in ["detached", "connected"] {
        require(
            &evaluate(
                &mut engine,
                &format!("runDomMutationHooks('{mode}', {WARMUP_ITERATIONS})"),
            )?,
            &warmup_checksum,
            "warm-up",
        )?;
    }

    let detached_call = format!("runDomMutationHooks('detached', {ITERATIONS})");
    let connected_call = format!("runDomMutationHooks('connected', {ITERATIONS})");
    let started = Instant::now();
    let detached_checksum = evaluate(&mut engine, &detached_call)?;
    let detached_elapsed_ns = started.elapsed().as_nanos();

    let started = Instant::now();
    let connected_checksum = evaluate(&mut engine, &connected_call)?;
    let connected_elapsed_ns = started.elapsed().as_nanos();
    let expected_checksum = (ITERATIONS * 5).to_string();
    require(&detached_checksum, &expected_checksum, "detached mutations")?;
    require(
        &connected_checksum,
        &expected_checksum,
        "connected mutations",
    )?;
    require(
        &evaluate(&mut engine, "verifyDomMutationHooks()")?,
        "ok",
        "verify",
    )?;
    let final_nodes = document.borrow().len();
    if final_nodes != EXPECTED_NODES {
        return Err(io::Error::other(format!(
            "mutations retained {final_nodes} native nodes; expected {EXPECTED_NODES}"
        ))
        .into());
    }
    require(
        &evaluate(&mut engine, "cleanupDomMutationHooks()")?,
        "ok",
        "cleanup",
    )?;
    drop(engine);
    println!(
        "{{\"scenario\":\"S-dom-mutation-hooks\",\"backend\":\"{backend}\",\"phase_order\":\"detached_then_connected\",\"warmup_iterations_per_phase\":{WARMUP_ITERATIONS},\"iterations_per_phase\":{ITERATIONS},\"mutations_per_iteration\":{MUTATIONS_PER_ITERATION},\"native_nodes\":{final_nodes},\"detached_checksum\":{detached_checksum},\"connected_checksum\":{connected_checksum},\"detached_elapsed_ns\":{detached_elapsed_ns},\"connected_elapsed_ns\":{connected_elapsed_ns}}}"
    );
    Ok(())
}
