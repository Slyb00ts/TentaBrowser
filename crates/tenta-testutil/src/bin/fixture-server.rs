//! `fixture-server`: the loopback fixture server as a process, for
//! `bench/run.py` and manual poking. Prints `LISTENING http://127.0.0.1:PORT`
//! on stdout once bound and runs until SIGINT/SIGTERM.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use tenta_testutil::{FixtureServer, FixtureServerConfig, PathDelay};

const USAGE: &str = "\
fixture-server: serve tests/fixtures over HTTP/1.1 on 127.0.0.1

Options:
  --root <DIR>            directory served at / (default: tests/fixtures)
  --port <N>              port to bind; 0 picks a free one (default: 0)
  --rtt <MS>              simulated round-trip time: half before the head, half before the body
  --delay-path <GLOB=MS>  extra delay before the head for matching paths (repeatable)
  --log-requests          print `REQ <unix_nanos> <method> <path>` to stderr per request
  -h, --help              this text

Endpoints: /api/echo (JSON echo), /api/slow?ms=N, /api/never, /api/sse (event every 100 ms).
";

fn parse_args() -> Result<FixtureServerConfig, String> {
    let mut config = FixtureServerConfig::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| {
            args.next()
                .ok_or_else(|| format!("{name} needs a value\n\n{USAGE}"))
        };
        match arg.as_str() {
            "--root" => config.root = PathBuf::from(value("--root")?),
            "--port" => {
                config.port = value("--port")?
                    .parse()
                    .map_err(|e| format!("--port: {e}"))?;
            }
            "--rtt" => {
                let millis: u64 = value("--rtt")?.parse().map_err(|e| format!("--rtt: {e}"))?;
                config.rtt = Duration::from_millis(millis);
            }
            "--delay-path" => config
                .path_delays
                .push(PathDelay::parse(&value("--delay-path")?)?),
            "--log-requests" => config.log_requests = true,
            "-h" | "--help" => return Err(USAGE.to_owned()),
            other => return Err(format!("unknown argument {other:?}\n\n{USAGE}")),
        }
    }
    Ok(config)
}

#[tokio::main]
async fn main() -> ExitCode {
    let config = match parse_args() {
        Ok(config) => config,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    let server = match FixtureServer::start(config).await {
        Ok(server) => server,
        Err(e) => {
            eprintln!("fixture-server: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "LISTENING {}", server.url(""));
    let _ = stdout.flush();
    drop(stdout);

    wait_for_stop_signal().await;
    server.shutdown();
    ExitCode::SUCCESS
}

#[cfg(unix)]
async fn wait_for_stop_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = match signal(SignalKind::terminate()) {
        Ok(term) => term,
        Err(_) => {
            let _ = tokio::signal::ctrl_c().await;
            return;
        }
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}

#[cfg(not(unix))]
async fn wait_for_stop_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
