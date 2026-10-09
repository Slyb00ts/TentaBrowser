//! `tentabrowser mcp` as a process: JSON-RPC on stdin and stdout, streamable
//! HTTP on a port it announces, and the refusal of an exposed bind without a
//! token.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};
use tenta_testutil::{FixtureServer, FixtureServerConfig, temp_dir};

const BIN: &str = env!("CARGO_BIN_EXE_tentabrowser");

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tenta-mcp/tests/fixtures")
}

async fn site() -> FixtureServer {
    FixtureServer::start(FixtureServerConfig::new(fixtures()))
        .await
        .expect("fixture server")
}

/// Kills the server however the test ends.
struct Process(Child);

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn command(home: &std::path::Path, args: &[&str]) -> Command {
    let mut command = Command::new(BIN);
    command
        .args(["--ephemeral", "--allow-private-network", "mcp"])
        .args(args)
        .env("HOME", home)
        .env("XDG_CACHE_HOME", home)
        .env("LOCALAPPDATA", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

/// A client on the process's pipes.
struct Stdio2 {
    process: Process,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    /// Drains stderr so a chatty log cannot fill the pipe and stall the server.
    log: std::thread::JoinHandle<String>,
    next: i64,
}

impl Stdio2 {
    fn start(home: &std::path::Path, args: &[&str]) -> Self {
        let mut child = command(home, args).spawn().expect("the binary starts");
        let input = child.stdin.take().expect("piped stdin");
        let output = BufReader::new(child.stdout.take().expect("piped stdout"));
        let mut stderr = child.stderr.take().expect("piped stderr");
        let log = std::thread::spawn(move || {
            let mut log = String::new();
            let _ = stderr.read_to_string(&mut log);
            log
        });
        Self {
            process: Process(child),
            input,
            output,
            log,
            next: 1,
        }
    }

    fn send(&mut self, value: &Value) {
        writeln!(self.input, "{value}").expect("write");
        self.input.flush().expect("flush");
    }

    fn read(&mut self) -> Value {
        let mut line = String::new();
        self.output.read_line(&mut line).expect("read");
        assert!(!line.is_empty(), "the server closed stdout");
        serde_json::from_str(&line)
            .unwrap_or_else(|_| panic!("stdout carries only JSON, got {line:?}"))
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let response = self.read();
        assert_eq!(response["id"], id, "{response}");
        response
    }

    fn tool(&mut self, name: &str, arguments: Value) -> Value {
        let response = self.call("tools/call", json!({"name": name, "arguments": arguments}));
        assert!(response.get("error").is_none(), "{response}");
        response["result"].clone()
    }
}

#[test]
fn the_schema_version_prints_and_exits() {
    let home = temp_dir("cli-mcp-version");
    let output = Command::new(BIN)
        .args(["mcp", "--schema-version"])
        .env("HOME", home.path())
        .output()
        .expect("runs");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "1.1.0");
}

#[tokio::test(flavor = "multi_thread")]
async fn stdio_serves_a_session_and_keeps_stdout_clean() {
    let server = site().await;
    let home = temp_dir("cli-mcp-stdio");
    let mut client = Stdio2::start(home.path(), &["--log", "debug"]);

    let init = client.call(
        "initialize",
        json!({"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}),
    );
    assert_eq!(init["result"]["serverInfo"]["name"], "tentabrowser");
    client.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));

    let listed = client.call("tools/list", json!({}));
    assert_eq!(listed["result"]["tools"].as_array().unwrap().len(), 10);

    let navigated = client.tool(
        "browser_navigate",
        json!({"url": server.url("/index.html")}),
    );
    assert_ne!(navigated["isError"], true, "{navigated}");
    assert_eq!(navigated["structuredContent"]["title"], "Fixture Home");

    let snapshot = client.tool("browser_snapshot", json!({}));
    let text = snapshot["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("- link \"About us\" [ref=e"), "{text}");

    // Closing stdin ends the session cleanly.
    drop(client.input);
    let mut rest = String::new();
    client.output.read_to_string(&mut rest).expect("drains");
    assert!(rest.is_empty(), "nothing is left on stdout: {rest:?}");
    let status = client.process.0.wait().expect("exits");
    assert!(status.success(), "{status}");
    // The debug log went to stderr, not into the protocol.
    let log = client.log.join().expect("the log drains");
    assert!(log.contains("tools/call"), "{log}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_piped_batch_is_answered_before_the_process_exits() {
    let server = site().await;
    let home = temp_dir("cli-mcp-batch");
    let mut child = command(home.path(), &[]).spawn().expect("starts");
    let mut input = child.stdin.take().unwrap();
    for message in [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-11-25"}}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
            "name": "browser_navigate", "arguments": {"url": server.url("/about.html")}}}),
    ] {
        writeln!(input, "{message}").unwrap();
    }
    drop(input);
    let output = child.wait_with_output().expect("exits");
    assert!(output.status.success());
    let replies: Vec<Value> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| serde_json::from_str(line).expect("JSON"))
        .collect();
    assert_eq!(replies.len(), 2, "{replies:?}");
    let navigate = replies
        .iter()
        .find(|reply| reply["id"] == 2)
        .expect("the navigate reply");
    assert_eq!(navigate["result"]["structuredContent"]["title"], "About");
}

#[cfg(all(feature = "v8", feature = "render"))]
#[tokio::test(flavor = "multi_thread")]
async fn the_full_build_drives_a_page_over_stdio() {
    let server = site().await;
    let home = temp_dir("cli-mcp-full");
    let mut client = Stdio2::start(home.path(), &[]);
    client.call("initialize", json!({"protocolVersion": "2025-11-25"}));
    client.tool("browser_navigate", json!({"url": server.url("/form.html")}));
    let snapshot = client.tool("browser_snapshot", json!({}));
    let text = snapshot["content"][0]["text"].as_str().unwrap().to_owned();
    let line = |needle: &str| {
        let line = text
            .lines()
            .find(|line| line.contains(needle))
            .expect(needle);
        let start = line.find("[ref=").unwrap() + 5;
        line[start..].split(']').next().unwrap().to_owned()
    };
    let (name, greet) = (line("textbox \"Your name\""), line("button \"Greet\""));
    let filled = client.tool("browser_fill", json!({"ref": name, "value": "Ada"}));
    assert_ne!(filled["isError"], true, "{filled}");
    client.tool("browser_click", json!({"ref": greet}));
    let evaluated = client.tool(
        "browser_evaluate",
        json!({"expression": "document.getElementById('out').textContent"}),
    );
    assert_eq!(evaluated["structuredContent"]["result"], "Hello, Ada!");
    let shot = client.tool("browser_screenshot", json!({}));
    assert_eq!(shot["content"][0]["type"], "image");
    assert_eq!(shot["content"][0]["mimeType"], "image/png");
}

#[test]
fn http_announces_its_endpoint_and_serves_requests() {
    let home = temp_dir("cli-mcp-http");
    let mut command = command(home.path(), &["--http", "--no-auth", "--port", "0"]);
    command.stdin(Stdio::null());
    let mut server = Process(command.spawn().expect("the binary starts"));
    let stderr = server.0.stderr.take().expect("piped stderr");
    let mut line = String::new();
    BufReader::new(stderr)
        .read_line(&mut line)
        .expect("a first line");
    let endpoint = line
        .trim()
        .strip_prefix("MCP listening on http://")
        .unwrap_or_else(|| panic!("the announcement, not {line:?}"));
    let (authority, path) = endpoint.split_once('/').expect("a path");
    assert_eq!(path, "mcp");
    assert!(
        authority.starts_with("127.0.0.1:"),
        "loopback by default: {authority}"
    );

    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}).to_string();
    let mut stream = TcpStream::connect(authority).expect("the endpoint accepts");
    write!(
        stream,
        "POST /mcp HTTP/1.1\r\nHost: {authority}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let payload: Value = serde_json::from_str(response.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(payload["result"]["tools"].as_array().unwrap().len(), 10);
}

#[test]
fn an_exposed_http_bind_needs_a_token() {
    let home = temp_dir("cli-mcp-exposed");
    let mut command = command(home.path(), &["--http", "--host", "0.0.0.0", "--port", "0"]);
    command.stdin(Stdio::null());
    let output = command.output().expect("the binary runs");
    assert_eq!(output.status.code(), Some(2), "a usage error");
    let message = String::from_utf8_lossy(&output.stderr);
    assert!(message.contains("bearer token"), "{message}");
}

#[test]
fn the_http_token_may_come_from_the_environment() {
    let home = temp_dir("cli-mcp-token");
    let mut command = command(home.path(), &["--http", "--host", "0.0.0.0", "--port", "0"]);
    command
        .stdin(Stdio::null())
        .env("TENTA_MCP_TOKEN", "0123456789abcdef-token");
    let mut server = Process(command.spawn().expect("the binary starts"));
    let stderr = server.0.stderr.take().expect("piped stderr");
    let mut line = String::new();
    BufReader::new(stderr)
        .read_line(&mut line)
        .expect("a first line");
    let endpoint = line
        .trim()
        .strip_prefix("MCP listening on http://")
        .unwrap_or_else(|| panic!("{line:?}"));
    let port = endpoint
        .rsplit_once(':')
        .unwrap()
        .1
        .split('/')
        .next()
        .unwrap()
        .to_owned();

    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}).to_string();
    let request = |auth: &str| {
        let mut stream = TcpStream::connect(format!("127.0.0.1:{port}")).unwrap();
        write!(
            stream,
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\n{auth}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    };
    assert!(request("").starts_with("HTTP/1.1 401"));
    assert!(
        request("Authorization: Bearer 0123456789abcdef-token\r\n").starts_with("HTTP/1.1 200")
    );
}

#[test]
fn http_only_flags_need_http() {
    let home = temp_dir("cli-mcp-flags");
    let output = command(home.path(), &["--port", "9"])
        .output()
        .expect("runs");
    assert_eq!(output.status.code(), Some(2));
    let output = command(home.path(), &["--viewport", "wide"])
        .output()
        .expect("runs");
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("WIDTHxHEIGHT"));
}

#[test]
fn loopback_http_gets_a_generated_token_unless_opted_out() {
    let home = temp_dir("cli-mcp-autotoken");
    let mut command = command(home.path(), &["--http", "--port", "0"]);
    command.stdin(Stdio::null());
    let mut server = Process(command.spawn().expect("the binary starts"));
    let mut stderr = BufReader::new(server.0.stderr.take().expect("piped stderr"));
    let (mut url, mut token) = (String::new(), String::new());
    stderr.read_line(&mut url).unwrap();
    stderr.read_line(&mut token).unwrap();
    let authority = url
        .trim()
        .strip_prefix("MCP listening on http://")
        .unwrap_or_else(|| panic!("{url:?}"))
        .split('/')
        .next()
        .unwrap()
        .to_owned();
    let token = token
        .trim()
        .strip_prefix("MCP bearer token: ")
        .unwrap_or_else(|| panic!("{token:?}"))
        .to_owned();
    assert!(token.len() >= 16, "{token}");

    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}).to_string();
    let request = |auth: String| {
        let mut stream = TcpStream::connect(&authority).unwrap();
        write!(
            stream,
            "POST /mcp HTTP/1.1\r\nHost: {authority}\r\nContent-Type: application/json\r\n{auth}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    };
    assert!(request(String::new()).starts_with("HTTP/1.1 401"));
    assert!(request(format!("Authorization: Bearer {token}\r\n")).starts_with("HTTP/1.1 200"));

    let output = command_with(home.path(), &["--http", "--no-auth", "--token", "x"])
        .output()
        .expect("runs");
    assert_eq!(
        output.status.code(),
        Some(2),
        "--no-auth conflicts with --token"
    );
}

fn command_with(home: &std::path::Path, args: &[&str]) -> Command {
    let mut command = command(home, args);
    command.stdin(Stdio::null());
    command
}
