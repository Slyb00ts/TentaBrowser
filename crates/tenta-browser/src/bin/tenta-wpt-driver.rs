//! `tenta-wpt-driver <plan.json> <results.json>`: the WPT driver subprocess
//! `wpt/run.py --product tentabrowser` invokes. See [`tenta_browser::wpt`] for
//! the contract and the transport.
//!
//! The process owns nothing but the two paths: the plan is read, the run
//! happens, the results are written. A failure is a non-zero exit with a
//! message on stderr, which is what `run.py`'s `check=True` reports.
//!
//! Diagnostics go to stderr and the results go to the file, so the two channels
//! stay separate. The subscriber is installed here because the library only
//! emits events: a run against a whole directory takes minutes, and without a
//! subscriber the engine's warnings and `wpt::run`'s own notices are dropped.

use std::ffi::OsStr;
use std::process::ExitCode;

use tenta_browser::wpt;

// The engine's io and page threads are its own, so one current-thread runtime
// is enough to drive the per-test waits: the pool needs concurrency — several
// in-flight waits at once — not parallelism, and `wpt::run` gets it from a
// `LocalSet` whose worker futures never have to be `Send`.
#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    init_logging();
    let mut args = std::env::args_os().skip(1);
    let (Some(plan), Some(out), None) = (args.next(), args.next(), args.next()) else {
        eprintln!("usage: tenta-wpt-driver <plan.json> <results.json>");
        return ExitCode::from(2);
    };
    match run(&plan, &out).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("tenta-wpt-driver: {message}");
            ExitCode::FAILURE
        }
    }
}

/// Installs the stderr subscriber, at `TENTA_LOG` or `WARN`.
///
/// `WARN` is the default because a redirected run's stderr should stay quiet.
/// `TENTA_LOG=info` adds a line per test with its status and subtest passes,
/// which is the only progress signal a run against a whole directory has.
fn init_logging() {
    let level = std::env::var("TENTA_LOG")
        .ok()
        .and_then(|level| level.parse::<tracing::Level>().ok())
        .unwrap_or(tracing::Level::WARN);
    let _ = tracing_subscriber::fmt()
        .with_max_level(level)
        .with_writer(std::io::stderr)
        .with_target(false)
        .try_init();
}

async fn run(plan_path: &OsStr, out_path: &OsStr) -> Result<(), String> {
    let plan_path = std::path::Path::new(plan_path);
    let text = std::fs::read_to_string(plan_path)
        .map_err(|error| format!("reading {}: {error}", plan_path.display()))?;
    let plan: wpt::Plan = serde_json::from_str(&text)
        .map_err(|error| format!("parsing {}: {error}", plan_path.display()))?;
    let results = wpt::run(plan).await.map_err(|error| error.to_string())?;
    let json = serde_json::to_string_pretty(&results).map_err(|error| error.to_string())?;
    let out_path = std::path::Path::new(out_path);
    std::fs::write(out_path, json + "\n")
        .map_err(|error| format!("writing {}: {error}", out_path.display()))?;
    Ok(())
}
