//! One uninterrupted materialization task in a fresh engine, without warmup.

use std::cell::RefCell;
use std::error::Error;
use std::io;
use std::rc::Rc;
use std::time::{Duration, Instant};

use tenta_dom::{Document, SharedDocument};
use tenta_js::engine::{PageEngine, ScriptError, ScriptKind};

const WORKLOAD: &str = include_str!("wrapper_pressure.js");

fn script_error(error: ScriptError) -> io::Error {
    io::Error::other(format!("{:?}: {}", error.kind, error.message))
}

fn healthy<E: PageEngine>(engine: &E) -> Result<(), io::Error> {
    if let Some(boundary) = engine.native_failure() {
        return Err(io::Error::other(format!(
            "native wrapper pressure failed at {boundary}"
        )));
    }
    Ok(())
}

/// Runs one fixed shape; external drivers measure process CPU and peak RSS.
pub fn run<E: PageEngine, F: FnOnce(SharedDocument) -> Result<E, io::Error>>(
    backend: &str,
    make_engine: F,
) -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args().skip(1);
    if arguments.next().as_deref() != Some("--shape") {
        return Err(io::Error::other("usage: wrapper_pressure --shape SHAPE").into());
    }
    let shape = arguments
        .next()
        .ok_or_else(|| io::Error::other("missing pressure shape"))?;
    if arguments.next().is_some() {
        return Err(io::Error::other("unexpected pressure argument").into());
    }
    let iterations: usize = match shape.as_str() {
        "detached" => 12_000,
        "owners" => 6_000,
        "connected" => 5_000,
        "deep" => 1_200,
        _ => return Err(io::Error::other("unknown pressure shape").into()),
    };
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = make_engine(document.clone())?;
    // Only defines a function: no warmup or native wrapper workload precedes
    // the timed task. Initialization still contributes to process CPU and RSS.
    engine
        .run_script(WORKLOAD, ScriptKind::Classic)
        .map_err(script_error)?;
    healthy(&engine)?;
    let nodes_before_task = document.borrow().len();
    let gc_epoch_before = engine.gc_epoch();
    let source = format!("runWrapperPressure('{shape}', {iterations})");
    let started = Instant::now();
    let checksum: usize = engine
        .evaluate(&source, Duration::from_secs(30))
        .map_err(script_error)?
        .parse()?;
    let elapsed_ns = started.elapsed().as_nanos();
    healthy(&engine)?;
    let expected_checksum = iterations * (iterations - 1) / 2;
    let gc_epoch_after = engine.gc_epoch();
    let nodes_after_task = document.borrow().len();
    if nodes_before_task != 1 || checksum != expected_checksum {
        return Err(io::Error::other("pressure task count or checksum mismatch").into());
    }
    if gc_epoch_after <= gc_epoch_before + 1 {
        return Err(io::Error::other("pressure did not collect within the task").into());
    }
    let valid_nodes = match shape.as_str() {
        "detached" | "owners" => (2..4096).contains(&nodes_after_task),
        _ => nodes_after_task == iterations + 2,
    };
    if !valid_nodes {
        return Err(io::Error::other(format!(
            "unexpected {nodes_after_task} native nodes after {shape} pressure"
        ))
        .into());
    }
    engine
        .run_script(
            "pressureRoot.remove(); delete globalThis.pressureRoot;",
            ScriptKind::Classic,
        )
        .map_err(script_error)?;
    for _ in 0..3 {
        engine.run_microtasks();
        healthy(&engine)?;
        engine.trim_memory();
        healthy(&engine)?;
        document.borrow_mut().sweep_detached();
    }
    let nodes_after_release_sweep = document.borrow().len();
    if nodes_after_release_sweep != nodes_before_task {
        return Err(io::Error::other("pressure cleanup retained an unreachable subtree").into());
    }
    println!(
        "{{\"scenario\":\"S-wrapper-pressure\",\"backend\":\"{backend}\",\"shape\":\"{shape}\",\"warmup_policy\":\"fresh_engine_no_warmup\",\"iterations\":{iterations},\"elapsed_ns\":{elapsed_ns},\"gc_epoch_before\":{gc_epoch_before},\"gc_epoch_after\":{gc_epoch_after},\"nodes_before_task\":{nodes_before_task},\"nodes_after_task\":{nodes_after_task},\"nodes_after_release_sweep\":{nodes_after_release_sweep},\"checksum\":{checksum},\"expected_checksum\":{expected_checksum}}}"
    );
    Ok(())
}
