//! A server over a Tier 1 or richer engine, a fixture site and a client that
//! talks to a connection in process.
#![allow(dead_code)] // each test binary uses a subset of the helpers

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use serde_json::{Value, json};
use tenta_browser::{ContextOptions, Engine, JsEngine};
use tenta_core::Tier;
use tenta_mcp::{Connection, McpOptions, McpServer};
use tenta_testutil::server::{FixtureServer, FixtureServerConfig};

pub fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

pub struct Harness {
    pub server: McpServer,
    pub engine: Engine,
    pub site: FixtureServer,
}

impl Harness {
    pub async fn start(tier: Tier) -> Self {
        Self::with_options(tier, |_| {}).await
    }

    pub async fn with_options(tier: Tier, configure: impl FnOnce(&mut McpOptions)) -> Self {
        let engine = Engine::builder()
            .js(if cfg!(feature = "v8") && tier >= Tier::Js {
                JsEngine::V8
            } else {
                JsEngine::Auto
            })
            .max_tier(tier)
            .page_threads(2)
            .io_threads(1)
            .ephemeral(true)
            .settle(tenta_mcp::settle_config())
            .build()
            .expect("engine");
        let mut options = McpOptions {
            context: ContextOptions {
                allow_private_network: true,
                ..ContextOptions::default()
            },
            ..McpOptions::default()
        };
        configure(&mut options);
        let server = McpServer::new(engine.clone(), options);
        let site = FixtureServer::start(FixtureServerConfig::new(fixtures_dir()))
            .await
            .expect("fixture server");
        Self {
            server,
            engine,
            site,
        }
    }

    pub fn url(&self, path: &str) -> String {
        self.site.url(path)
    }

    pub fn client(&self) -> Client {
        Client::new(self.server.connect().expect("a connection"))
    }
}

pub struct Client {
    pub connection: Arc<Connection>,
    next: AtomicI64,
}

impl Client {
    pub fn new(connection: Arc<Connection>) -> Self {
        Self {
            connection,
            next: AtomicI64::new(1),
        }
    }

    /// The whole JSON-RPC response.
    pub async fn request(&self, method: &str, params: Value) -> Value {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let response = self
            .connection
            .handle(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await
            .expect("a response to a request");
        assert_eq!(response["id"], id, "{response}");
        response
    }

    /// The `result` of a request that must succeed.
    pub async fn result(&self, method: &str, params: Value) -> Value {
        let response = self.request(method, params).await;
        assert!(response.get("error").is_none(), "{response}");
        response["result"].clone()
    }

    pub async fn initialize(&self) -> Value {
        let result = self
            .result(
                "initialize",
                json!({
                    "protocolVersion": "2025-11-25",
                    "capabilities": {},
                    "clientInfo": {"name": "test", "version": "1"},
                }),
            )
            .await;
        assert!(
            self.connection
                .handle(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
                .await
                .is_none()
        );
        result
    }

    pub async fn tool(&self, name: &str, arguments: Value) -> ToolResult {
        ToolResult(
            self.result("tools/call", json!({"name": name, "arguments": arguments}))
                .await,
        )
    }

    /// Takes a snapshot and returns the ref of the first line holding all of `needles`.
    pub async fn ref_of(&self, needles: &[&str]) -> String {
        let snapshot = self.tool("browser_snapshot", json!({})).await;
        snapshot.ok();
        find_ref(&snapshot.text(), needles)
    }

    pub async fn navigate(&self, url: &str) -> ToolResult {
        let result = self.tool("browser_navigate", json!({"url": url})).await;
        result.ok();
        result
    }

    pub async fn evaluate(&self, expression: &str) -> Value {
        let result = self
            .tool("browser_evaluate", json!({"expression": expression}))
            .await;
        result.ok();
        result.structured()["result"].clone()
    }
}

pub fn find_ref(text: &str, needles: &[&str]) -> String {
    let line = text
        .lines()
        .find(|line| needles.iter().all(|needle| line.contains(needle)))
        .unwrap_or_else(|| panic!("no line with {needles:?} in:\n{text}"));
    let start = line.find("[ref=").expect("a ref") + "[ref=".len();
    let end = line[start..].find(']').expect("a closing bracket") + start;
    line[start..end].to_owned()
}

pub struct ToolResult(pub Value);

impl ToolResult {
    pub fn is_error(&self) -> bool {
        self.0["isError"] == true
    }

    /// Panics with the result when the tool failed.
    pub fn ok(&self) -> &Self {
        assert!(!self.is_error(), "tool failed: {}", self.0);
        self
    }

    pub fn text(&self) -> String {
        self.0["content"]
            .as_array()
            .expect("content")
            .iter()
            .filter(|block| block["type"] == "text")
            .map(|block| block["text"].as_str().unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn structured(&self) -> &Value {
        &self.0["structuredContent"]
    }

    pub fn error_type(&self) -> String {
        assert!(self.is_error(), "expected an error, got {}", self.0);
        self.0["structuredContent"]["error"]["type"]
            .as_str()
            .expect("an error type")
            .to_owned()
    }

    /// The image blocks as (mime type, bytes).
    pub fn images(&self) -> Vec<(String, Vec<u8>)> {
        use base64::Engine as _;
        self.0["content"]
            .as_array()
            .expect("content")
            .iter()
            .filter(|block| block["type"] == "image")
            .map(|block| {
                (
                    block["mimeType"].as_str().unwrap().to_owned(),
                    base64::engine::general_purpose::STANDARD
                        .decode(block["data"].as_str().unwrap())
                        .expect("base64"),
                )
            })
            .collect()
    }
}
