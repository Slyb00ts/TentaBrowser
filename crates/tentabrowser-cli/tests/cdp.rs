//! `tentabrowser cdp` as a process: it announces its endpoint the way Chromium
//! does, serves `/json/version`, and refuses an exposed bind without a token.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};

use tenta_testutil::temp_dir;

const BIN: &str = env!("CARGO_BIN_EXE_tentabrowser");

/// Kills the server however the test ends.
struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn command(home: &std::path::Path, args: &[&str]) -> Command {
    let mut command = Command::new(BIN);
    command
        .args(["--ephemeral", "cdp"])
        .args(args)
        .env("HOME", home)
        .env("XDG_CACHE_HOME", home)
        .env("LOCALAPPDATA", home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    command
}

#[test]
fn the_cdp_command_announces_its_endpoint_and_serves_the_version() {
    let home = temp_dir("cli-cdp");
    let child = command(home.path(), &["--port", "0"])
        .spawn()
        .expect("the binary starts");
    let mut server = Server(child);
    let stderr = server.0.stderr.take().expect("piped stderr");
    let mut line = String::new();
    BufReader::new(stderr)
        .read_line(&mut line)
        .expect("a first line");
    let endpoint = line
        .trim()
        .strip_prefix("DevTools listening on ws://")
        .unwrap_or_else(|| panic!("the announcement, not {line:?}"));
    let (authority, path) = endpoint.split_once('/').expect("a path");
    assert!(path.starts_with("devtools/browser/"), "{path}");
    assert!(
        authority.starts_with("127.0.0.1:"),
        "loopback by default: {authority}"
    );

    let mut stream = TcpStream::connect(authority).expect("the endpoint accepts");
    write!(
        stream,
        "GET /json/version HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let body = response.split("\r\n\r\n").nth(1).expect("a body");
    let version: serde_json::Value = serde_json::from_str(body).expect("JSON");
    assert!(
        version["Browser"]
            .as_str()
            .unwrap()
            .starts_with("TentaBrowser/")
    );
    assert!(
        version["webSocketDebuggerUrl"]
            .as_str()
            .unwrap()
            .ends_with(path)
    );
}

#[test]
fn an_exposed_bind_needs_a_token() {
    let home = temp_dir("cli-cdp-exposed");
    let output = command(home.path(), &["--host", "0.0.0.0", "--port", "0"])
        .output()
        .expect("the binary runs");
    assert_eq!(output.status.code(), Some(2), "a usage error");
    let message = String::from_utf8_lossy(&output.stderr);
    assert!(message.contains("token"), "{message}");
}

#[test]
fn the_token_may_come_from_the_environment() {
    let home = temp_dir("cli-cdp-token");
    let mut command = command(
        home.path(),
        &["--port", "0", "--allowed-origin", "http://app.test"],
    );
    command.env("TENTA_CDP_TOKEN", "0123456789abcdef-token");
    let mut server = Server(command.spawn().expect("the binary starts"));
    let stderr = server.0.stderr.take().expect("piped stderr");
    let mut line = String::new();
    BufReader::new(stderr)
        .read_line(&mut line)
        .expect("a first line");
    assert!(
        line.trim().ends_with("?token=0123456789abcdef-token"),
        "{line}"
    );
}
