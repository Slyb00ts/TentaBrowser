//! Shared real-engine runner for the listener GC benchmark examples.

use std::cell::RefCell;
use std::error::Error;
use std::fmt::Write as _;
use std::io;
use std::rc::Rc;
use std::time::{Duration, Instant};

use tenta_dom::{Document, SharedDocument};
use tenta_js::engine::{PageEngine, ScriptError, ScriptKind};

const WORKLOAD: &str = include_str!("listener_gc.js");
const NODES: usize = 256;
const GC_ROUNDS: usize = 3;

struct PhaseSpec {
    name: &'static str,
    setup: &'static str,
    setup_argument: usize,
    created_nodes: usize,
    verify: Option<&'static str>,
}

const PHASES: [PhaseSpec; 5] = [
    PhaseSpec {
        name: "connected",
        setup: "setupConnected",
        setup_argument: NODES,
        created_nodes: NODES + 1,
        verify: Some("verifyConnected"),
    },
    PhaseSpec {
        name: "self_capture",
        setup: "setupSelfCapture",
        setup_argument: NODES,
        created_nodes: NODES,
        verify: None,
    },
    PhaseSpec {
        name: "cross_capture",
        setup: "setupCrossCapture",
        setup_argument: NODES / 2,
        created_nodes: NODES,
        verify: None,
    },
    PhaseSpec {
        name: "reachable_detached",
        setup: "setupReachableDetached",
        setup_argument: NODES,
        created_nodes: NODES,
        verify: Some("verifyReachableDetached"),
    },
    PhaseSpec {
        name: "noncapturing_control",
        setup: "setupNoncapturing",
        setup_argument: NODES,
        created_nodes: NODES,
        verify: None,
    },
];

struct PhaseResult {
    name: &'static str,
    created_nodes: usize,
    before_nodes: usize,
    after_nodes: usize,
    freed_nodes: usize,
    retained_nodes: usize,
    round_ns: [u128; GC_ROUNDS],
    dispatch_hits: Option<usize>,
}

fn script_error(error: ScriptError) -> io::Error {
    io::Error::other(format!("{:?}: {}", error.kind, error.message))
}

fn ensure_healthy<E: PageEngine>(engine: &E, phase: &str) -> Result<(), io::Error> {
    if let Some(boundary) = engine.native_failure() {
        return Err(io::Error::other(format!(
            "{phase} poisoned the engine at native callback {boundary}"
        )));
    }
    Ok(())
}

fn run_phase<E: PageEngine, F: Fn(SharedDocument) -> Result<E, io::Error>>(
    spec: &PhaseSpec,
    make_engine: &F,
) -> Result<PhaseResult, Box<dyn Error>> {
    let document = Rc::new(RefCell::new(Document::new()));
    let mut engine = make_engine(document.clone())?;
    engine
        .run_script(WORKLOAD, ScriptKind::Classic)
        .map_err(script_error)?;
    ensure_healthy(&engine, spec.name)?;
    engine
        .run_script(
            &format!("{}({})", spec.setup, spec.setup_argument),
            ScriptKind::Classic,
        )
        .map_err(script_error)?;
    ensure_healthy(&engine, spec.name)?;

    let before_nodes = document.borrow().len();
    if before_nodes != spec.created_nodes + 1 {
        return Err(io::Error::other(format!(
            "{} created {before_nodes} total nodes; expected {}",
            spec.name,
            spec.created_nodes + 1
        ))
        .into());
    }

    let mut round_ns = [0; GC_ROUNDS];
    let mut freed_nodes = 0;
    for elapsed in &mut round_ns {
        // A checkpoint may run callbacks, so it is deliberately outside the
        // timed GC/sweep window. The second trim makes this an explicit
        // complete-drain protocol after the native sweep.
        engine.run_microtasks();
        ensure_healthy(&engine, spec.name)?;
        let started = Instant::now();
        engine.trim_memory();
        // A native callback can fail during GC; never sweep a poisoned world.
        ensure_healthy(&engine, spec.name)?;
        freed_nodes += document.borrow_mut().sweep_detached().nodes_freed;
        engine.trim_memory();
        *elapsed = started.elapsed().as_nanos();
        ensure_healthy(&engine, spec.name)?;
    }
    let after_nodes = document.borrow().len();
    if after_nodes + freed_nodes != before_nodes || freed_nodes > spec.created_nodes {
        return Err(io::Error::other(format!(
            "{} inconsistent native sweep: before {before_nodes}, after {after_nodes}, freed {freed_nodes}",
            spec.name
        ))
        .into());
    }
    let retained_nodes = after_nodes - 1;
    if matches!(spec.name, "connected" | "reachable_detached")
        && retained_nodes != spec.created_nodes
    {
        return Err(io::Error::other(format!(
            "{} lost live nodes: retained {retained_nodes}, expected {}",
            spec.name, spec.created_nodes
        ))
        .into());
    }
    if spec.name == "noncapturing_control" && freed_nodes != spec.created_nodes {
        return Err(io::Error::other(format!(
            "noncapturing control freed {freed_nodes}, expected {}",
            spec.created_nodes
        ))
        .into());
    }

    let dispatch_hits = spec
        .verify
        .map(|function| {
            engine
                .evaluate(&format!("{function}({NODES})"), Duration::from_secs(5))
                .map_err(script_error)?
                .parse::<usize>()
                .map_err(|error| io::Error::other(format!("{} hits: {error}", spec.name)))
        })
        .transpose()?;
    ensure_healthy(&engine, spec.name)?;
    if dispatch_hits.is_some_and(|hits| hits != NODES) {
        return Err(io::Error::other(format!(
            "{} dispatched {dispatch_hits:?} listeners; expected {NODES}",
            spec.name
        ))
        .into());
    }

    Ok(PhaseResult {
        name: spec.name,
        created_nodes: spec.created_nodes,
        before_nodes,
        after_nodes,
        freed_nodes,
        retained_nodes,
        round_ns,
        dispatch_hits,
    })
}

/// Runs the same source and GC protocol against a concrete page engine.
pub fn run<E: PageEngine, F: Fn(SharedDocument) -> Result<E, io::Error>>(
    backend: &str,
    make_engine: F,
) -> Result<(), Box<dyn Error>> {
    let results = PHASES
        .iter()
        .map(|spec| run_phase(spec, &make_engine))
        .collect::<Result<Vec<_>, _>>()?;

    let mut payload = format!(
        "{{\"scenario\":\"S-listener-gc\",\"backend\":\"{backend}\",\"nodes_per_phase\":{NODES},\"gc_rounds\":{GC_ROUNDS},\"phases\":{{"
    );
    for (index, phase) in results.iter().enumerate() {
        if index > 0 {
            payload.push(',');
        }
        write!(
            payload,
            "\"{}\":{{\"created_nodes\":{},\"before_nodes\":{},\"after_nodes\":{},\"freed_nodes\":{},\"retained_nodes\":{},\"safe_point_ns\":{},\"round_ns\":[{},{},{}],\"dispatch_hits\":{}}}",
            phase.name,
            phase.created_nodes,
            phase.before_nodes,
            phase.after_nodes,
            phase.freed_nodes,
            phase.retained_nodes,
            phase.round_ns.iter().sum::<u128>(),
            phase.round_ns[0],
            phase.round_ns[1],
            phase.round_ns[2],
            phase
                .dispatch_hits
                .map_or_else(|| "null".to_string(), |hits| hits.to_string())
        )?;
    }
    payload.push_str("}}");
    println!("{payload}");
    Ok(())
}
