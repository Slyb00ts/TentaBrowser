//! The `tentabrowser` binary: `fetch`, `search` and `scrape` at Tier 1
//! (`docs/spec/16-roadmap.md` P1).
//!
//! The process parses its arguments, builds an engine with the tier ceiling
//! at `dom` and sends the first request before anything else exists: no
//! script engine, no style, no fonts (`docs/spec/13-performance.md` §1, §2).
//! `screenshot`, `serve`, `mcp`, `test` and `daemon` arrive with the phases
//! that implement what they need.

mod cli;
mod error;
mod fetch;
mod logging;
mod output;
mod phase;
mod rate_limit;
mod scrape;
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
