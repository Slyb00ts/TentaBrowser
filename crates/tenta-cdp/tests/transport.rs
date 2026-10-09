//! The transport end to end: a real WebSocket client against a server on an
//! ephemeral loopback port, with a probe domain registered through the public
//! seam to observe routing and ordering.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tenta_browser::Engine;
use tenta_cdp::{
    CdpOptions, CdpServer, CommandContext, CommandFuture, DomainHandler, DomainRegistry, Stats,
    parse_params,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Notify;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{WebSocketStream, client_async};

/// A bound for tests that would otherwise hang on a regression. It is never
/// waited out on a passing run.
const PATIENCE: Duration = Duration::from_secs(30);

const TOKEN: &str = "0123456789abcdef-token";

// ---- probe domain -------------------------------------------------------

struct Probe {
    gate: Arc<Notify>,
    emitted: Arc<AtomicUsize>,
}

#[derive(Default)]
struct Counter(AtomicU64);

impl DomainHandler for Probe {
    fn domain(&self) -> &'static str {
        "Probe"
    }

    fn methods(&self) -> &'static [&'static str] {
        &[
            "whoami",
            "count",
            "park",
            "parkReleasing",
            "release",
            "emit",
            "strict",
            "explode",
            "flood",
        ]
    }

    fn handle<'a>(
        &'a self,
        cx: CommandContext,
        method: &'a str,
        params: Value,
    ) -> CommandFuture<'a> {
        Box::pin(async move {
            match method {
                "whoami" => Ok(json!({
                    "session": cx.session().id(),
                    "target": cx.session().target().map(|target| target.id()),
                })),
                "count" => {
                    let counter = cx.session().state::<Counter>();
                    Ok(json!({ "count": counter.0.fetch_add(1, Ordering::SeqCst) + 1 }))
                }
                "park" => {
                    self.gate.notified().await;
                    Ok(json!({ "parked": true }))
                }
                "parkReleasing" => {
                    cx.release_order();
                    self.gate.notified().await;
                    Ok(json!({ "parked": true }))
                }
                "release" => {
                    self.gate.notify_one();
                    Ok(json!({}))
                }
                "emit" => {
                    cx.emit("Probe.fired", json!({ "n": 1 })).await;
                    Ok(json!({}))
                }
                "strict" => {
                    #[derive(serde::Deserialize)]
                    #[serde(rename_all = "camelCase")]
                    struct Params {
                        #[allow(dead_code)]
                        needed: String,
                    }
                    let _: Params = parse_params(params)?;
                    Ok(json!({}))
                }
                "explode" => panic!("probe handler panic"),
                "flood" => {
                    let payload = "x".repeat(32 * 1024);
                    for _ in 0..3000 {
                        cx.emit("Probe.flood", json!({ "payload": payload })).await;
                        self.emitted.fetch_add(1, Ordering::SeqCst);
                    }
                    Ok(json!({}))
                }
                _ => unreachable!("the dispatcher only routes listed methods"),
            }
        })
    }
}

// ---- fixture ------------------------------------------------------------

struct Fixture {
    server: CdpServer,
    engine: Engine,
    gate: Arc<Notify>,
    emitted: Arc<AtomicUsize>,
}

impl Fixture {
    async fn start() -> Self {
        Self::with(|_| {}).await
    }

    async fn with(configure: impl FnOnce(&mut CdpOptions)) -> Self {
        let engine = Engine::builder()
            .page_threads(1)
            .ephemeral(true)
            .build()
            .expect("engine");
        let mut options = CdpOptions {
            bind: "127.0.0.1:0".parse().unwrap(),
            ..CdpOptions::default()
        };
        configure(&mut options);
        let gate = Arc::new(Notify::new());
        let emitted = Arc::new(AtomicUsize::new(0));
        let mut registry = DomainRegistry::with_defaults();
        registry.register(Probe {
            gate: Arc::clone(&gate),
            emitted: Arc::clone(&emitted),
        });
        let server = CdpServer::bind_with(engine.clone(), options, registry)
            .await
            .expect("bind");
        Self {
            server,
            engine,
            gate,
            emitted,
        }
    }

    fn addr(&self) -> std::net::SocketAddr {
        self.server.local_addr()
    }

    async fn browser(&self) -> Client {
        Client::connect(&self.server.browser_ws_url()).await
    }

    async fn wait_for_stats(&self, wanted: impl Fn(&Stats) -> bool) {
        let mut stats = self.server.stats();
        tokio::time::timeout(PATIENCE, stats.wait_for(|stats| wanted(stats)))
            .await
            .expect("stats did not reach the expected state")
            .expect("server dropped");
    }

    /// Waits until the flood command has queued more events than the
    /// outbound queue holds, which can only happen while the client is not
    /// reading.
    async fn wait_for_stalled_writer(&self) {
        tokio::time::timeout(PATIENCE, async {
            while self.emitted.load(Ordering::SeqCst) < 1100 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the flood never filled the queue");
    }

    async fn stop(self) {
        self.server.shutdown().await;
        self.engine.close();
    }
}

struct Client {
    ws: WebSocketStream<TcpStream>,
    next_id: u64,
    /// Messages read while waiting for something else, oldest first.
    backlog: Vec<Value>,
}

impl Client {
    async fn connect(url: &str) -> Self {
        Self::try_connect(url).await.expect("websocket handshake")
    }

    async fn try_connect(url: &str) -> Result<Self, WsError> {
        let request = url.into_client_request().expect("request");
        Self::try_connect_request(request).await
    }

    async fn try_connect_request(
        request: tokio_tungstenite::tungstenite::handshake::client::Request,
    ) -> Result<Self, WsError> {
        let authority = request.uri().authority().expect("authority").to_string();
        let stream = TcpStream::connect(authority).await.expect("tcp");
        let (ws, _) = client_async(request, stream).await?;
        Ok(Self {
            ws,
            next_id: 1,
            backlog: Vec::new(),
        })
    }

    async fn send_raw(&mut self, text: &str) {
        self.ws.send(Message::text(text)).await.expect("send");
    }

    /// Sends a command without waiting; returns its id.
    async fn send(&mut self, session: Option<&str>, method: &str, params: Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let mut message = json!({ "id": id, "method": method, "params": params });
        if let Some(session) = session {
            message["sessionId"] = json!(session);
        }
        self.send_raw(&message.to_string()).await;
        id
    }

    async fn read(&mut self) -> Value {
        if !self.backlog.is_empty() {
            return self.backlog.remove(0);
        }
        self.read_wire().await
    }

    async fn read_wire(&mut self) -> Value {
        loop {
            let frame = tokio::time::timeout(PATIENCE, self.ws.next())
                .await
                .expect("timed out waiting for a message")
                .expect("connection ended")
                .expect("frame");
            if let Message::Text(text) = frame {
                return serde_json::from_str(text.as_str()).expect("json");
            }
        }
    }

    /// Reads until the response with `id`, keeping other messages for later.
    async fn response(&mut self, id: u64) -> Value {
        for index in 0..self.backlog.len() {
            if self.backlog[index]["id"] == id {
                return self.backlog.remove(index);
            }
        }
        loop {
            let message = self.read_wire().await;
            if message["id"] == id {
                return message;
            }
            self.backlog.push(message);
        }
    }

    async fn call(&mut self, session: Option<&str>, method: &str, params: Value) -> Value {
        let id = self.send(session, method, params).await;
        self.response(id).await
    }

    async fn result(&mut self, session: Option<&str>, method: &str, params: Value) -> Value {
        let response = self.call(session, method, params).await;
        assert!(
            response.get("error").is_none(),
            "{method} failed: {response}"
        );
        response["result"].clone()
    }

    async fn event(&mut self, method: &str) -> Value {
        for index in 0..self.backlog.len() {
            if self.backlog[index]["method"] == method {
                return self.backlog.remove(index);
            }
        }
        loop {
            let message = self.read_wire().await;
            if message["method"] == method {
                return message;
            }
            self.backlog.push(message);
        }
    }

    async fn create_target(&mut self) -> String {
        let result = self
            .result(None, "Target.createTarget", json!({ "url": "about:blank" }))
            .await;
        result["targetId"].as_str().expect("targetId").to_owned()
    }

    async fn attach(&mut self, target_id: &str) -> String {
        let result = self
            .result(
                None,
                "Target.attachToTarget",
                json!({ "targetId": target_id, "flatten": true }),
            )
            .await;
        result["sessionId"].as_str().expect("sessionId").to_owned()
    }

    /// True once the server has ended the connection.
    async fn ended(&mut self) -> bool {
        loop {
            match tokio::time::timeout(PATIENCE, self.ws.next())
                .await
                .expect("timed out waiting for the connection to end")
            {
                None | Some(Err(_) | Ok(Message::Close(_))) => return true,
                Some(Ok(_)) => {}
            }
        }
    }
}

struct Http {
    status: u16,
    body: String,
}

impl Http {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|_| panic!("not JSON: {}", self.body))
    }
}

async fn http(addr: std::net::SocketAddr, method: &str, target: &str, headers: &[&str]) -> Http {
    let mut stream = TcpStream::connect(addr).await.expect("tcp");
    let mut request = format!("{method} {target} HTTP/1.1\r\nHost: {addr}\r\n");
    for header in headers {
        request.push_str(header);
        request.push_str("\r\n");
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).await.expect("write");
    let mut raw = Vec::new();
    tokio::time::timeout(PATIENCE, stream.read_to_end(&mut raw))
        .await
        .expect("timed out")
        .expect("read");
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").expect("response head");
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|status| status.parse().ok())
        .expect("status line");
    Http {
        status,
        body: body.to_owned(),
    }
}

fn handshake_status(result: Result<Client, WsError>) -> u16 {
    match result {
        Err(WsError::Http(response)) => response.status().as_u16(),
        Err(other) => panic!("expected an HTTP refusal, got {other}"),
        Ok(_) => 101,
    }
}

// ---- /json endpoints ----------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn version_endpoint_describes_the_browser() {
    let fixture = Fixture::start().await;
    let reply = http(fixture.addr(), "GET", "/json/version", &[]).await;
    assert_eq!(reply.status, 200);
    let version = reply.json();
    assert!(
        version["Browser"]
            .as_str()
            .unwrap()
            .starts_with("TentaBrowser/")
    );
    assert_eq!(version["Protocol-Version"], "1.3");
    assert!(
        version["User-Agent"]
            .as_str()
            .is_some_and(|ua| !ua.is_empty())
    );
    assert_eq!(
        version["webSocketDebuggerUrl"].as_str().unwrap(),
        fixture.server.browser_ws_url()
    );
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn targets_can_be_created_listed_and_closed_over_http() {
    let fixture = Fixture::start().await;
    let addr = fixture.addr();
    assert_eq!(http(addr, "GET", "/json/list", &[]).await.json(), json!([]));

    let created = http(addr, "PUT", "/json/new?about:blank", &[]).await;
    assert_eq!(created.status, 200);
    let created = created.json();
    let id = created["id"].as_str().unwrap().to_owned();
    assert_eq!(created["type"], "page");
    assert_eq!(created["url"], "about:blank");
    assert_eq!(
        created["webSocketDebuggerUrl"],
        format!("ws://{addr}/devtools/page/{id}")
    );

    for path in ["/json/list", "/json", "/json/"] {
        let listed = http(addr, "GET", path, &[]).await.json();
        assert_eq!(listed.as_array().unwrap().len(), 1, "{path}");
        assert_eq!(listed[0]["id"], id.as_str());
    }

    let closed = http(addr, "GET", &format!("/json/close/{id}"), &[]).await;
    assert_eq!(closed.status, 200);
    assert_eq!(http(addr, "GET", "/json/list", &[]).await.json(), json!([]));
    assert_eq!(
        http(addr, "GET", &format!("/json/close/{id}"), &[])
            .await
            .status,
        404
    );
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn new_target_rejects_unsafe_verbs_and_bad_urls() {
    let fixture = Fixture::start().await;
    let addr = fixture.addr();
    assert_eq!(
        http(addr, "GET", "/json/new?about:blank", &[]).await.status,
        405
    );
    assert_eq!(
        http(addr, "PUT", "/json/new?not%20a%20url", &[])
            .await
            .status,
        400
    );
    assert_eq!(http(addr, "GET", "/json/unknown", &[]).await.status, 404);
    assert_eq!(http(addr, "POST", "/json/version", &[]).await.status, 405);
    assert_eq!(http(addr, "GET", "/json/list", &[]).await.json(), json!([]));
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn protocol_endpoint_lists_the_registered_commands() {
    let fixture = Fixture::start().await;
    let protocol = http(fixture.addr(), "GET", "/json/protocol", &[])
        .await
        .json();
    let target = protocol["domains"]
        .as_array()
        .unwrap()
        .iter()
        .find(|domain| domain["domain"] == "Target")
        .expect("Target domain");
    assert!(
        target["commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|command| command["name"] == "createTarget")
    );
    fixture.stop().await;
}

// ---- framing and errors -------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unimplemented_methods_answer_32601_with_the_original_id() {
    let fixture = Fixture::start().await;
    let mut client = fixture.browser().await;
    client.next_id = 41;
    for method in [
        "Nope.nothing",
        "Browser.nothing",
        "Page.startScreencast",
        "no-dot",
    ] {
        let id = client.next_id;
        let response = client.call(None, method, json!({})).await;
        assert_eq!(response["id"], id, "{method}");
        assert_eq!(response["error"]["code"], -32601, "{method}");
        assert!(
            response["error"]["message"]
                .as_str()
                .unwrap()
                .contains(method),
            "{method}: {response}"
        );
    }
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_messages_are_answered_and_the_connection_survives() {
    let fixture = Fixture::start().await;
    let mut client = fixture.browser().await;

    client.send_raw("{oops").await;
    let parse_error = client.read().await;
    assert_eq!(parse_error["error"]["code"], -32700);
    assert!(parse_error.get("id").is_none());

    client.send_raw(r#"{"id":5}"#).await;
    let invalid = client.read().await;
    assert_eq!(invalid["id"], 5);
    assert_eq!(invalid["error"]["code"], -32600);

    client.send_raw(r#"[1,2]"#).await;
    assert_eq!(client.read().await["error"]["code"], -32600);

    let version = client.result(None, "Browser.getVersion", json!({})).await;
    assert_eq!(version["protocolVersion"], "1.3");
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bad_params_answer_32602() {
    let fixture = Fixture::start().await;
    let mut client = fixture.browser().await;
    for (method, params) in [
        ("Target.attachToTarget", json!({})),
        ("Target.closeTarget", json!({ "targetId": "missing" })),
        ("Probe.strict", json!({ "needed": 3 })),
    ] {
        let id = client.next_id;
        let response = client.call(None, method, params).await;
        assert_eq!(response["id"], id, "{method}");
        assert_eq!(response["error"]["code"], -32602, "{method}: {response}");
    }
    let response = client
        .call(
            None,
            "Target.createTarget",
            json!({ "url": "not%20a%20url" }),
        )
        .await;
    assert_eq!(response["error"]["code"], -32602);
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_panicking_handler_fails_one_command_not_the_connection() {
    let fixture = Fixture::start().await;
    let mut client = fixture.browser().await;
    let response = client.call(None, "Probe.explode", json!({})).await;
    assert_eq!(response["error"]["code"], -32603);
    client.result(None, "Browser.getVersion", json!({})).await;
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_session_is_reported_with_its_id() {
    let fixture = Fixture::start().await;
    let mut client = fixture.browser().await;
    let response = client.call(Some("NOPE"), "Probe.whoami", json!({})).await;
    assert_eq!(response["error"]["code"], -32001);
    assert_eq!(response["sessionId"], "NOPE");
    fixture.stop().await;
}

// ---- sessions -----------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interleaved_sessions_route_to_their_own_target_and_state() {
    let fixture = Fixture::start().await;
    let mut client = fixture.browser().await;
    let target_a = client.create_target().await;
    let target_b = client.create_target().await;
    let session_a = client.attach(&target_a).await;
    let session_b = client.attach(&target_b).await;
    assert_ne!(session_a, session_b);

    let attached = client.event("Target.attachedToTarget").await;
    assert_eq!(attached["params"]["sessionId"], session_a.as_str());
    assert_eq!(
        attached["params"]["targetInfo"]["targetId"],
        target_a.as_str()
    );
    assert!(attached.get("sessionId").is_none());

    // All six commands are on the wire before any answer is read.
    let mut sent = Vec::new();
    let (mut counts_a, mut counts_b) = (0, 0);
    for (session, method) in [
        (&session_a, "Probe.whoami"),
        (&session_b, "Probe.whoami"),
        (&session_a, "Probe.count"),
        (&session_b, "Probe.count"),
        (&session_a, "Probe.count"),
        (&session_b, "Probe.whoami"),
    ] {
        let counted = if method == "Probe.count" {
            let counts = if *session == session_a {
                &mut counts_a
            } else {
                &mut counts_b
            };
            *counts += 1;
            Some(*counts)
        } else {
            None
        };
        let id = client.send(Some(session), method, json!({})).await;
        sent.push((id, session.clone(), counted));
    }
    for (id, session, counted) in sent {
        let response = client.response(id).await;
        assert_eq!(response["sessionId"], session.as_str());
        match counted {
            Some(expected) => assert_eq!(response["result"]["count"], expected),
            None => {
                let own_target = if session == session_a {
                    &target_a
                } else {
                    &target_b
                };
                assert_eq!(response["result"]["session"], session.as_str());
                assert_eq!(response["result"]["target"], own_target.as_str());
            }
        }
    }
    let a = client
        .result(Some(&session_a), "Probe.count", json!({}))
        .await;
    let b = client
        .result(Some(&session_b), "Probe.count", json!({}))
        .await;
    assert_eq!(
        (a["count"].as_u64(), b["count"].as_u64()),
        (Some(3), Some(2))
    );
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_runs_commands_in_order_while_others_proceed() {
    let fixture = Fixture::start().await;
    let mut client = fixture.browser().await;
    let target_a = client.create_target().await;
    let target_b = client.create_target().await;
    let session_a = client.attach(&target_a).await;
    let session_b = client.attach(&target_b).await;

    let parked = client.send(Some(&session_a), "Probe.park", json!({})).await;
    let behind = client
        .send(Some(&session_a), "Probe.whoami", json!({}))
        .await;
    let other = client
        .send(Some(&session_b), "Probe.whoami", json!({}))
        .await;

    // Session B answers while A is parked, and A's second command has not run.
    let answer = client.response(other).await;
    assert_eq!(answer["result"]["session"], session_b.as_str());

    let release = client
        .send(Some(&session_b), "Probe.release", json!({}))
        .await;
    // Arrival order, not lookup order: the second command must not have been
    // answered before the parked one finished.
    let mut arrived = Vec::new();
    while arrived.len() < 3 {
        let message = client.read().await;
        // Attachment events are still queued ahead of the responses.
        arrived.extend(message["id"].as_u64());
    }
    let position = |id: u64| arrived.iter().position(|seen| *seen == id).unwrap();
    assert!(arrived.contains(&release));
    assert!(position(parked) < position(behind), "{arrived:?}");
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_released_command_lets_the_next_one_start() {
    let fixture = Fixture::start().await;
    let mut client = fixture.browser().await;
    let target = client.create_target().await;
    let session = client.attach(&target).await;

    let parked = client
        .send(Some(&session), "Probe.parkReleasing", json!({}))
        .await;
    let behind = client.send(Some(&session), "Probe.whoami", json!({})).await;
    let answer = client.response(behind).await;
    assert_eq!(answer["result"]["session"], session.as_str());

    fixture.gate.notify_one();
    assert_eq!(client.response(parked).await["result"]["parked"], true);
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn events_precede_the_response_of_the_command_that_caused_them() {
    let fixture = Fixture::start().await;
    let mut client = fixture.browser().await;
    let target = client.create_target().await;
    let session = client.attach(&target).await;
    let _ = client.event("Target.attachedToTarget").await;

    let id = client.send(Some(&session), "Probe.emit", json!({})).await;
    let first = client.read().await;
    assert_eq!(first["method"], "Probe.fired");
    assert_eq!(first["sessionId"], session.as_str());
    assert_eq!(first["params"]["n"], 1);
    let second = client.read().await;
    assert_eq!(second["id"], id);
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn detaching_closes_the_session_and_announces_it() {
    let fixture = Fixture::start().await;
    let mut client = fixture.browser().await;
    let target = client.create_target().await;
    let session = client.attach(&target).await;
    fixture.wait_for_stats(|stats| stats.sessions == 1).await;

    client
        .result(
            None,
            "Target.detachFromTarget",
            json!({ "sessionId": session }),
        )
        .await;
    let event = client.event("Target.detachedFromTarget").await;
    assert_eq!(event["params"]["sessionId"], session.as_str());
    assert_eq!(event["params"]["targetId"], target.as_str());
    fixture.wait_for_stats(|stats| stats.sessions == 0).await;
    let response = client.call(Some(&session), "Probe.whoami", json!({})).await;
    assert_eq!(response["error"]["code"], -32001);
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_a_target_detaches_its_sessions() {
    let fixture = Fixture::start().await;
    let mut client = fixture.browser().await;
    let target = client.create_target().await;
    let session = client.attach(&target).await;

    let closed = client
        .result(None, "Target.closeTarget", json!({ "targetId": target }))
        .await;
    assert_eq!(closed["success"], true);
    let event = client.event("Target.detachedFromTarget").await;
    assert_eq!(event["params"]["sessionId"], session.as_str());
    let targets = client.result(None, "Target.getTargets", json!({})).await;
    assert_eq!(targets["targetInfos"], json!([]));
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_the_connection_cleans_up_its_sessions() {
    let fixture = Fixture::start().await;
    let mut client = fixture.browser().await;
    let target = client.create_target().await;
    let _first = client.attach(&target).await;
    let _second = client.attach(&target).await;
    fixture
        .wait_for_stats(|stats| stats.connections == 1 && stats.sessions == 2)
        .await;
    let page = fixture.server.backend().target(&target).expect("target");
    assert!(page.is_attached());

    drop(client);
    fixture
        .wait_for_stats(|stats| stats.connections == 0 && stats.sessions == 0)
        .await;
    assert!(!page.is_attached());
    assert_eq!(
        fixture.server.backend().targets().len(),
        1,
        "pages outlive clients"
    );
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_page_endpoint_is_bound_to_its_page() {
    let fixture = Fixture::start().await;
    let created = http(fixture.addr(), "PUT", "/json/new?about:blank", &[])
        .await
        .json();
    let target = created["id"].as_str().unwrap().to_owned();
    let mut client = Client::connect(created["webSocketDebuggerUrl"].as_str().unwrap()).await;

    let who = client.result(None, "Probe.whoami", json!({})).await;
    assert_eq!(who["target"], target.as_str());
    assert_eq!(who["session"], Value::Null);

    http(fixture.addr(), "GET", &format!("/json/close/{target}"), &[]).await;
    assert!(client.ended().await);
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn websocket_paths_must_name_something_that_exists() {
    let fixture = Fixture::start().await;
    let addr = fixture.addr();
    let status = |path: &str| {
        let url = format!("ws://{addr}{path}");
        async move { handshake_status(Client::try_connect(&url).await) }
    };
    assert_eq!(status("/devtools/browser/NOPE").await, 404);
    assert_eq!(status("/devtools/page/NOPE").await, 404);
    // A plain GET of a WebSocket endpoint is not an upgrade.
    let path = format!(
        "/devtools/browser/{}",
        fixture.server.backend().browser_id()
    );
    assert_eq!(http(addr, "GET", &path, &[]).await.status, 400);
    fixture.stop().await;
}

// ---- limits, exposure, shutdown ----------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connections_beyond_the_limit_get_503() {
    let fixture = Fixture::with(|options| options.max_connections = 1).await;
    let first = fixture.browser().await;
    fixture.wait_for_stats(|stats| stats.connections == 1).await;

    let refused = Client::try_connect(&fixture.server.browser_ws_url()).await;
    assert_eq!(handshake_status(refused), 503);
    assert_eq!(
        http(fixture.addr(), "GET", "/json/version", &[])
            .await
            .status,
        503
    );

    drop(first);
    fixture.wait_for_stats(|stats| stats.connections == 0).await;
    assert_eq!(
        http(fixture.addr(), "GET", "/json/version", &[])
            .await
            .status,
        200
    );
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_non_loopback_bind_is_refused_without_the_explicit_option_and_a_token() {
    let engine = Engine::builder()
        .page_threads(1)
        .ephemeral(true)
        .build()
        .expect("engine");
    let attempts = [
        (false, None),
        (true, None),
        (true, Some("too-short")),
        (false, Some(TOKEN)),
    ];
    for (allow_non_loopback, token) in attempts {
        let options = CdpOptions {
            bind: "0.0.0.0:0".parse().unwrap(),
            allow_non_loopback,
            token: token.map(str::to_owned),
            ..CdpOptions::default()
        };
        let refused = CdpServer::bind(engine.clone(), options).await;
        assert!(
            refused.is_err(),
            "allow={allow_non_loopback} token={token:?} must be refused"
        );
    }
    engine.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_configured_token_is_required_everywhere() {
    let fixture = Fixture::with(|options| options.token = Some(TOKEN.to_owned())).await;
    let addr = fixture.addr();
    assert_eq!(http(addr, "GET", "/json/version", &[]).await.status, 401);
    assert_eq!(
        http(addr, "GET", "/json/version?token=wrong", &[])
            .await
            .status,
        401
    );
    assert_eq!(
        http(addr, "PUT", "/json/new?about:blank", &[]).await.status,
        401
    );

    let version = http(addr, "GET", &format!("/json/version?token={TOKEN}"), &[]).await;
    assert_eq!(version.status, 200);
    let url = version.json()["webSocketDebuggerUrl"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(url.ends_with(&format!("?token={TOKEN}")), "{url}");

    let bare = url.split('?').next().unwrap();
    assert_eq!(handshake_status(Client::try_connect(bare).await), 401);
    let mut client = Client::connect(&url).await;
    client.result(None, "Browser.getVersion", json!({})).await;

    // Listed page endpoints stay usable by whoever may list them.
    let created = http(
        addr,
        "PUT",
        &format!("/json/new?about:blank&token={TOKEN}"),
        &[],
    )
    .await;
    assert_eq!(created.status, 200);
    let page_url = created.json()["webSocketDebuggerUrl"]
        .as_str()
        .unwrap()
        .to_owned();
    Client::connect(&page_url).await;
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn foreign_origins_and_hosts_are_refused() {
    let fixture = Fixture::start().await;
    let addr = fixture.addr();
    let evil_origin = "Origin: http://evil.example";
    assert_eq!(
        http(addr, "GET", "/json/version", &[evil_origin])
            .await
            .status,
        403
    );

    // The Host header of a DNS-rebinding page names the attacker's domain.
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(b"GET /json/version HTTP/1.1\r\nHost: evil.example\r\n\r\n")
        .await
        .unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).await.unwrap();
    assert!(raw.starts_with("HTTP/1.1 403"), "{raw}");

    let mut request = fixture
        .server
        .browser_ws_url()
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("Origin", "http://evil.example".parse().unwrap());
    assert_eq!(
        handshake_status(Client::try_connect_request(request).await),
        403
    );
    fixture.stop().await;

    let trusting =
        Fixture::with(|options| options.allowed_origins = vec!["http://devtools.local".into()])
            .await;
    let mut request = trusting
        .server
        .browser_ws_url()
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("Origin", "http://devtools.local".parse().unwrap());
    Client::try_connect_request(request)
        .await
        .expect("allow-listed origin");
    trusting.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_closes_connections_and_stops_listening() {
    let fixture = Fixture::start().await;
    let mut client = fixture.browser().await;
    let addr = fixture.addr();
    fixture.wait_for_stats(|stats| stats.connections == 1).await;

    let engine = fixture.engine.clone();
    fixture.server.shutdown().await;
    assert!(client.ended().await);
    assert!(TcpStream::connect(addr).await.is_err());
    engine.close();
}

// ---- resource limits ----------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_completes_while_a_client_refuses_to_read() {
    let fixture = Fixture::start().await;
    let mut client = fixture.browser().await;
    client.send(None, "Probe.flood", json!({})).await;
    fixture.wait_for_stalled_writer().await;

    let mut stats = fixture.server.stats();
    let engine = fixture.engine.clone();
    tokio::time::timeout(PATIENCE, fixture.server.shutdown())
        .await
        .expect("shutdown hung on a stalled writer");
    assert_eq!(stats.borrow_and_update().connections, 0);
    engine.close();
    drop(client);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reset_in_the_middle_of_a_write_releases_the_connection() {
    let fixture = Fixture::start().await;
    let mut client = fixture.browser().await;
    client.send(None, "Probe.flood", json!({})).await;
    fixture.wait_for_stalled_writer().await;

    // Linger 0 turns the close into a reset while the server is writing.
    client.ws.get_ref().set_zero_linger().expect("linger");
    drop(client);
    fixture.wait_for_stats(|stats| stats.connections == 0).await;
    assert_eq!(
        http(fixture.addr(), "GET", "/json/version", &[])
            .await
            .status,
        200
    );
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_oversized_message_closes_the_connection() {
    let fixture = Fixture::with(|options| options.max_message_bytes = Some(1024)).await;
    let mut client = fixture.browser().await;
    client.result(None, "Browser.getVersion", json!({})).await;
    client.send_raw(&"x".repeat(4096)).await;
    assert!(client.ended().await);
    fixture.wait_for_stats(|stats| stats.connections == 0).await;
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_head_that_never_finishes_is_dropped() {
    let fixture = Fixture::with(|options| options.head_timeout = Duration::from_millis(50)).await;
    let mut stream = TcpStream::connect(fixture.addr()).await.unwrap();
    stream.write_all(b"GET /json/ver").await.unwrap();
    let mut rest = Vec::new();
    tokio::time::timeout(PATIENCE, stream.read_to_end(&mut rest))
        .await
        .expect("the server kept the connection")
        .unwrap();
    assert!(rest.is_empty());
    fixture.wait_for_stats(|stats| stats.connections == 0).await;
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_oversized_request_head_gets_431() {
    let fixture = Fixture::start().await;
    let mut stream = TcpStream::connect(fixture.addr()).await.unwrap();
    let request = format!(
        "GET /json/version HTTP/1.1\r\nX-Pad: {}\r\n\r\n",
        "a".repeat(20_000)
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut raw = String::new();
    tokio::time::timeout(PATIENCE, stream.read_to_string(&mut raw))
        .await
        .expect("timed out")
        .unwrap();
    assert!(raw.starts_with("HTTP/1.1 431"), "{raw}");
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn targets_are_limited() {
    let fixture = Fixture::with(|options| options.max_targets = 1).await;
    let mut client = fixture.browser().await;
    let first = client.create_target().await;
    let refused = client
        .call(None, "Target.createTarget", json!({ "url": "about:blank" }))
        .await;
    assert_eq!(refused["error"]["code"], -32000);
    assert_eq!(
        http(fixture.addr(), "PUT", "/json/new?about:blank", &[])
            .await
            .status,
        400
    );

    client
        .result(None, "Target.closeTarget", json!({ "targetId": first }))
        .await;
    client.create_target().await;
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hung_command_times_out_without_stalling_its_session() {
    let fixture =
        Fixture::with(|options| options.command_timeout = Duration::from_millis(50)).await;
    let mut client = fixture.browser().await;
    let target = client.create_target().await;
    let session = client.attach(&target).await;

    let hung = client.call(Some(&session), "Probe.park", json!({})).await;
    assert_eq!(hung["error"]["code"], -32000);
    assert!(
        hung["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Probe.park")
    );
    client
        .result(Some(&session), "Probe.whoami", json!({}))
        .await;
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_idle_connection_is_closed_when_a_limit_is_set() {
    let fixture = Fixture::with(|options| {
        options.command_timeout = Duration::from_millis(50);
        options.idle_timeout = Some(Duration::from_millis(50));
    })
    .await;
    let mut client = fixture.browser().await;
    assert!(client.ended().await);
    fixture.wait_for_stats(|stats| stats.connections == 0).await;
    fixture.stop().await;
}
