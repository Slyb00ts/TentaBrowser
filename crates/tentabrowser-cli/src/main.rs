//! The `tentabrowser` binary: Tier 1 `fetch`, `search` and `scrape`, with an
//! opt-in V8 path for `fetch --eval` (`docs/spec/16-roadmap.md` P1/P2).
//!
//! By default the process builds an engine with the tier ceiling at `dom`
//! and sends the first request without a script engine, style or fonts
//! (`docs/spec/13-performance.md` §1, §2).
//! `cdp` serves the DevTools protocol. `serve`, `mcp`, `test` and `daemon` arrive with the phases that implement
//! what they need.

mod cdp;
mod cli;
mod error;
mod fetch;
mod logging;
mod output;
mod phase;
mod rate_limit;
mod scrape;
mod screenshot;
mod search;
mod setup;

use std::io::Write;
use std::process::ExitCode;

use clap::Parser;

use cli::{Cli, Command};
use error::{CliError, EXIT_OK, EXIT_USAGE};

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            // clap prints `--help` and `--version` through the same channel;
            // neither is a failure.
            let _ = error.print();
            return match error.exit_code() {
                0 => ExitCode::from(EXIT_OK),
                _ => ExitCode::from(EXIT_USAGE),
            };
        }
    };
    logging::init(&cli.global.log, cli.global.log_unredacted);

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => return fail(&CliError::new(1, format!("starting the runtime: {error}"))),
    };

    match runtime.block_on(run(&cli)) {
        Ok(()) => ExitCode::from(EXIT_OK),
        Err(error) => fail(&error),
    }
}

async fn run(cli: &Cli) -> Result<(), CliError> {
    match &cli.command {
        Command::Fetch(args) => fetch::run(&cli.global, args).await,
        Command::Search(args) => search::run(&cli.global, args).await,
        Command::Scrape(args) => scrape::run(&cli.global, args).await,
        Command::Screenshot(args) => screenshot::run(&cli.global, args).await,
        Command::Cdp(args) => cdp::run(&cli.global, args).await,
        Command::Version => {
            let mut stdout = std::io::stdout().lock();
            writeln!(stdout, "tentabrowser {}", env!("CARGO_PKG_VERSION"))
                .map_err(|err| CliError::new(1, format!("writing output: {err}")))
        }
    }
}

fn fail(error: &CliError) -> ExitCode {
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "{}", error.report());
    ExitCode::from(error.exit_code())
}
