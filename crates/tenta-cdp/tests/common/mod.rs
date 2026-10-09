//! The WebSocket client and the fixture site the interaction tests share.
#![allow(dead_code)] // each test binary uses a subset of the helpers

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tenta_browser::{Engine, JsEngine};
use tenta_cdp::{CdpOptions, CdpServer};
use tenta_core::Tier;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{WebSocketStream, client_async};

/// A bound for tests that would otherwise hang on a regression. It is never
/// waited out on a passing run.
pub const PATIENCE: Duration = Duration::from_secs(30);

// ---- fixture ------------------------------------------------------------

/// Serves fixed pages by path on a loopback port.
pub struct Site {
    pub base: String,
    /// `/slow` answers once this is notified.
    pub gate: Arc<Notify>,
}

impl Site {
    pub async fn start(pages: &[(&str, &str)]) -> Self {
        let pages: Arc<HashMap<String, String>> = Arc::new(
            pages
                .iter()
                .map(|(path, body)| ((*path).to_owned(), (*body).to_owned()))
                .collect(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let base = format!("http://{}", listener.local_addr().unwrap());
        let gate = Arc::new(Notify::new());
        let held = Arc::clone(&gate);
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let pages = Arc::clone(&pages);
                let held = Arc::clone(&held);
                tokio::spawn(async move {
                    let mut head = Vec::new();
                    let mut chunk = [0_u8; 2048];
                    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
                        match stream.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(read) => head.extend_from_slice(&chunk[..read]),
                        }
                    }
                    let head = String::from_utf8_lossy(&head);
                    let path = head.split_whitespace().nth(1).unwrap_or("/");
                    if path == "/stall" {
                        // A response that promises more than it sends.
                        let partial = "<html><body>partial";
                        let head = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{partial}",
                            partial.len() + 100
                        );
                        let _ = stream.write_all(head.as_bytes()).await;
                        held.notified().await;
                        return;
                    }
                    if path == "/slow" {
                        held.notified().await;
                    }
                    let (status, body) = match pages.get(path) {
                        Some(body) => ("200 OK", body.as_str()),
                        None => ("404 Not Found", "not found"),
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.shutdown().await;
                });
            }
        });
        Self { base, gate }
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }
}

pub struct Fixture {
    pub server: CdpServer,
    pub engine: Engine,
    pub site: Site,
}

impl Fixture {
    pub async fn start(pages: &[(&str, &str)]) -> Self {
        Self::start_with(pages, |_| {}).await
    }

    pub async fn start_with(
        pages: &[(&str, &str)],
        configure: impl FnOnce(&mut CdpOptions),
    ) -> Self {
        let engine = Engine::builder()
            .js(JsEngine::V8)
            .page_threads(1)
            .ephemeral(true)
            .max_tier(Tier::Render)
            .build()
            .expect("engine");
        let mut options = CdpOptions {
            bind: "127.0.0.1:0".parse().unwrap(),
            ..CdpOptions::default()
        };
        options.default_context.allow_private_network = true;
        configure(&mut options);
        let server = CdpServer::bind(engine.clone(), options)
            .await
            .expect("bind");
        Self {
            server,
            engine,
            site: Site::start(pages).await,
        }
    }

    /// A page session on a blank page, as a client holds it after
    /// `Target.attachToTarget`.
    pub async fn page(&self) -> Page {
        let mut client = Client::connect(&self.server.browser_ws_url()).await;
        let target = client
            .result(None, "Target.createTarget", json!({ "url": "about:blank" }))
            .await["targetId"]
            .as_str()
            .unwrap()
            .to_owned();
        let session = client
            .result(
                None,
                "Target.attachToTarget",
                json!({ "targetId": target, "flatten": true }),
            )
            .await["sessionId"]
            .as_str()
            .unwrap()
            .to_owned();
        Page {
            client,
            session,
            target,
        }
    }

    /// A second client attached to an existing page.
    pub async fn attach(&self, target: &str) -> Page {
        let mut client = Client::connect(&self.server.browser_ws_url()).await;
        let session = client
            .result(
                None,
                "Target.attachToTarget",
                json!({ "targetId": target, "flatten": true }),
            )
            .await["sessionId"]
            .as_str()
            .unwrap()
            .to_owned();
        Page {
            client,
            session,
            target: target.to_owned(),
        }
    }

    pub async fn stop(self) {
        self.server.shutdown().await;
        self.engine.close();
    }
}

pub struct Client {
    pub ws: WebSocketStream<TcpStream>,
    pub next_id: u64,
    pub backlog: Vec<Value>,
}

impl Client {
    pub async fn connect(url: &str) -> Self {
        let request = url.into_client_request().expect("request");
        let authority = request.uri().authority().expect("authority").to_string();
        let stream = TcpStream::connect(authority).await.expect("tcp");
        let (ws, _) = client_async(request, stream).await.expect("handshake");
        Self {
            ws,
            next_id: 1,
            backlog: Vec::new(),
        }
    }

    pub async fn read_wire(&mut self) -> Value {
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

    pub async fn send(&mut self, session: Option<&str>, method: &str, params: Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let mut message = json!({ "id": id, "method": method, "params": params });
        if let Some(session) = session {
            message["sessionId"] = json!(session);
        }
        self.ws
            .send(Message::text(message.to_string()))
            .await
            .expect("send");
        id
    }

    pub async fn response(&mut self, id: u64) -> Value {
        if let Some(index) = self.backlog.iter().position(|m| m["id"] == id) {
            return self.backlog.remove(index);
        }
        loop {
            let message = self.read_wire().await;
            if message["id"] == id {
                return message;
            }
            self.backlog.push(message);
        }
    }

    pub async fn call(&mut self, session: Option<&str>, method: &str, params: Value) -> Value {
        let id = self.send(session, method, params).await;
        self.response(id).await
    }

    pub async fn result(&mut self, session: Option<&str>, method: &str, params: Value) -> Value {
        let response = self.call(session, method, params).await;
        assert!(
            response.get("error").is_none(),
            "{method} failed: {response}"
        );
        response["result"].clone()
    }

    /// The next event of any name, from the backlog first.
    pub async fn next_event(&mut self) -> Value {
        if let Some(index) = self.backlog.iter().position(|m| m.get("id").is_none()) {
            return self.backlog.remove(index);
        }
        loop {
            let message = self.read_wire().await;
            if message.get("id").is_none() {
                return message;
            }
            self.backlog.push(message);
        }
    }

    /// The next event named `method`, from the backlog first.
    pub async fn event(&mut self, method: &str) -> Value {
        if let Some(index) = self.backlog.iter().position(|m| m["method"] == method) {
            return self.backlog.remove(index)["params"].clone();
        }
        loop {
            let message = self.read_wire().await;
            if message["method"] == method {
                return message["params"].clone();
            }
            self.backlog.push(message);
        }
    }
}

/// A client attached to one page.
pub struct Page {
    pub client: Client,
    pub session: String,
    pub target: String,
}

impl Page {
    pub async fn call(&mut self, method: &str, params: Value) -> Value {
        let session = self.session.clone();
        self.client.call(Some(&session), method, params).await
    }

    pub async fn result(&mut self, method: &str, params: Value) -> Value {
        let session = self.session.clone();
        self.client.result(Some(&session), method, params).await
    }

    pub async fn event(&mut self, method: &str) -> Value {
        self.client.event(method).await
    }

    pub async fn evaluate(&mut self, expression: &str) -> Value {
        self.result(
            "Runtime.evaluate",
            json!({ "expression": expression, "returnByValue": true }),
        )
        .await["result"]["value"]
            .clone()
    }

    /// `Page.enable` and `setLifecycleEventsEnabled`, the way Puppeteer begins.
    pub async fn begin(&mut self) {
        self.result("Page.enable", json!({})).await;
        self.result("Page.setLifecycleEventsEnabled", json!({ "enabled": true }))
            .await;
    }

    /// The names of the events from now on up to and including the lifecycle
    /// event `last` of `loader`, as `Page.x` or `lifecycle:name`.
    pub async fn events_through(&mut self, loader: &str, last: &str) -> Vec<String> {
        let mut seen = Vec::new();
        loop {
            let event = self.client.next_event().await;
            let method = event["method"].as_str().unwrap_or_default();
            if !method.starts_with("Page.") {
                continue;
            }
            if method == "Page.lifecycleEvent" {
                let params = &event["params"];
                assert_eq!(params["frameId"], self.target.as_str());
                if params["loaderId"] != loader {
                    continue;
                }
                let name = params["name"].as_str().unwrap().to_owned();
                seen.push(format!("lifecycle:{name}"));
                if name == last {
                    return seen;
                }
            } else {
                seen.push(method.to_owned());
            }
        }
    }

    /// The names of the events from now on up to the first `last`.
    pub async fn events_until(&mut self, last: &str) -> Vec<String> {
        let mut seen = Vec::new();
        loop {
            let event = self.client.next_event().await;
            let method = event["method"].as_str().unwrap_or_default().to_owned();
            let done = method == last;
            seen.push(method);
            if done {
                return seen;
            }
        }
    }

    /// Navigates and returns the loader id.
    pub async fn navigate(&mut self, url: &str) -> String {
        self.result("Page.navigate", json!({ "url": url })).await["loaderId"]
            .as_str()
            .expect("a loader id")
            .to_owned()
    }

    pub async fn screenshot(&mut self, params: Value) -> Vec<u8> {
        let data = self.result("Page.captureScreenshot", params).await["data"]
            .as_str()
            .expect("base64 data")
            .to_owned();
        base64::engine::general_purpose::STANDARD
            .decode(data)
            .expect("valid base64")
    }
}

pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Decoded {
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * self.width + x) * 4) as usize;
        self.rgba[at..at + 4].try_into().expect("four channels")
    }
}

pub fn decode_png(bytes: &[u8]) -> Decoded {
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "a PNG signature");
    let mut reader = png::Decoder::new(Cursor::new(bytes))
        .read_info()
        .expect("a decodable PNG");
    let mut rgba = vec![0; reader.output_buffer_size().expect("a sized buffer")];
    let info = reader.next_frame(&mut rgba).expect("a frame");
    assert_eq!(info.color_type, png::ColorType::Rgba);
    Decoded {
        width: info.width,
        height: info.height,
        rgba,
    }
}

pub fn error_of(response: &Value) -> (i64, &str) {
    (
        response["error"]["code"].as_i64().expect("an error"),
        response["error"]["message"].as_str().unwrap_or_default(),
    )
}
