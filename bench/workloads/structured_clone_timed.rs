//! Shared real-engine structured-clone benchmark with untimed full verification.

use std::error::Error;
use std::fmt::Write as _;
use std::io;
use std::time::{Duration, Instant};

use tenta_js::engine::{PageEngine, ScriptError, ScriptKind};

const WORKLOAD: &str = include_str!("structured_clone_timed.js");
const CALLS: u64 = 256;
const WARMUP_CALLS: u64 = 16;

pub(crate) fn script_error(error: ScriptError) -> io::Error {
    io::Error::other(format!("{:?}: {}", error.kind, error.message))
}

fn check_native_failure<E: PageEngine>(engine: &E) -> Result<(), io::Error> {
    if let Some(boundary) = engine.native_failure() {
        return Err(io::Error::other(format!("native failure at {boundary}")));
    }
    Ok(())
}

fn evaluate<E: PageEngine>(engine: &mut E, source: &str) -> Result<String, io::Error> {
    let result = engine
        .evaluate(source, Duration::from_secs(30))
        .map_err(script_error)?;
    check_native_failure(engine)?;
    Ok(result)
}

fn require(actual: &str, expected: &str, operation: &str) -> Result<(), io::Error> {
    if actual != expected {
        return Err(io::Error::other(format!(
            "{operation} returned {actual:?}; expected {expected:?}"
        )));
    }
    Ok(())
}

pub fn run<E: PageEngine>(backend: &str, mut engine: E) -> Result<(), Box<dyn Error>> {
    engine
        .run_script(WORKLOAD, ScriptKind::Classic)
        .map_err(script_error)?;
    check_native_failure(&engine)?;

    let mut output = format!(
        "{{\"scenario\":\"S-structured-clone-timed\",\"backend\":\"{backend}\",\"buffer_bytes\":65536,\"graph_sparse_length\":4097,\"calls_per_operation\":{CALLS},\"warmup_calls_per_operation\":{WARMUP_CALLS}"
    );
    for (name, function, verification, checksum_per_call) in [
        (
            "graph_clone",
            "timedGraphClones",
            "verifyCloneGraphState",
            171_828_u64,
        ),
        (
            "buffer_copy",
            "timedBufferCopies",
            "verifyCloneCopyState",
            65_982,
        ),
        (
            "buffer_transfer",
            "timedBufferTransfers",
            "verifyCloneTransferState",
            65_614,
        ),
    ] {
        let check = format!("{verification}()");
        require(&evaluate(&mut engine, &check)?, "ok", name)?;

        let warmup = format!("{function}({WARMUP_CALLS})");
        require(
            &evaluate(&mut engine, &warmup)?,
            &(checksum_per_call * WARMUP_CALLS).to_string(),
            name,
        )?;
        require(&evaluate(&mut engine, &check)?, "ok", name)?;

        let invocation = format!("{function}({CALLS})");
        let started = Instant::now();
        let result = evaluate(&mut engine, &invocation)?;
        let elapsed_ns = started.elapsed().as_nanos();
        let expected = checksum_per_call * CALLS;
        require(&result, &expected.to_string(), name)?;
        require(&evaluate(&mut engine, &check)?, "ok", name)?;

        write!(
            output,
            ",\"{name}_checksum\":{result},\"{name}_elapsed_ns\":{elapsed_ns},\"{name}_ns_per_call\":{}",
            elapsed_ns as f64 / CALLS as f64
        )?;
    }
    println!("{output}}}");
    Ok(())
}
