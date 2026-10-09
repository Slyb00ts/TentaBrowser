//! The Browser, Target, Inspector, Log, Security and Storage domains through a
//! real WebSocket client, in the command sequences Puppeteer and Playwright
//! send when they connect and open pages.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tenta_browser::{Engine, Goto};
use tenta_cdp::{
    CdpOptions, CdpServer, CommandContext, CommandFuture, DomainHandler, DomainRegistry, Session,
    TargetEvent,
};
use tenta_core::Tier;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{WebSocketStream, client_async};

/// A bound for tests that would otherwise hang on a regression. It is never
/// waited out on a passing run.
const PATIENCE: Duration = Duration::from_secs(30);

// ---- fixture ------------------------------------------------------------

struct Fixture {
    server: CdpServer,
    engine: Engine,
}

impl Fixture {
    async fn start() -> Self {
        // Tier 1 pages keep their parsed document, which is where a title
        // comes from, whatever features the workspace build unified.
        let engine = Engine::builder()
            .page_threads(1)
            .ephemeral(true)
            .max_tier(Tier::Dom)
            .build()
            .expect("engine");
        let mut options = CdpOptions {
            bind: "127.0.0.1:0".parse().unwrap(),
            ..CdpOptions::default()
        };
        options.default_context.allow_private_network = true;
        let server = CdpServer::bind(engine.clone(), options)
            .await
            .expect("bind");
        Self { server, engine }
    }

    async fn browser(&self) -> Client {
        Client::connect(&self.server.browser_ws_url()).await
    }

    fn default_context(&self) -> String {
        self.server.backend().default_context_id().to_owned()
    }

    async fn stop(self) {
        self.server.shutdown().await;
        self.engine.close();
    }
}

/// An HTTP server that answers every request with the same page and records
/// the requests it saw.
struct Site {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<String>>>,
    hits: Arc<AtomicUsize>,
}

impl Site {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let hits = Arc::new(AtomicUsize::new(0));
        let (seen, counted) = (Arc::clone(&requests), Arc::clone(&hits));
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let (seen, counted) = (Arc::clone(&seen), Arc::clone(&counted));
                tokio::spawn(async move {
                    let mut head = Vec::new();
                    let mut chunk = [0_u8; 2048];
                    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
                        match stream.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(read) => head.extend_from_slice(&chunk[..read]),
                        }
                    }
                    seen.lock()
                        .unwrap()
                        .push(String::from_utf8_lossy(&head).into_owned());
                    counted.fetch_add(1, Ordering::SeqCst);
                    let body = "<html><head><title>Hello</title></head><body>hi</body></html>";
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.shutdown().await;
                });
            }
        });
        Self {
            addr,
            requests,
            hits,
        }
    }

    fn url(&self) -> String {
        format!("http://{}/", self.addr)
    }

    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }

    fn request_headers(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
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

    async fn send(&mut self, session: Option<&str>, method: &str, params: Value) -> u64 {
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

    async fn error_code(&mut self, session: Option<&str>, method: &str, params: Value) -> i64 {
        let response = self.call(session, method, params).await;
        response["error"]["code"]
            .as_i64()
            .unwrap_or_else(|| panic!("{method} should have failed: {response}"))
    }

    /// The next event named `method`, from the backlog first.
    async fn event(&mut self, method: &str) -> Value {
        if let Some(index) = self.backlog.iter().position(|m| m["method"] == method) {
            return self.backlog.remove(index);
        }
        loop {
            let message = self.read_wire().await;
            if message["method"] == method {
                return message;
            }
            self.backlog.push(message);
        }
    }

    /// Whether an event is already buffered, which for the events a command
    /// causes means it was sent before that command's response.
    fn buffered(&self, method: &str) -> bool {
        self.backlog.iter().any(|m| m["method"] == method)
    }

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

fn session_of(event: &Value) -> String {
    event["params"]["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_owned()
}

async fn put(addr: SocketAddr, target: &str) -> Value {
    let mut stream = TcpStream::connect(addr).await.expect("tcp");
    let request = format!("PUT {target} HTTP/1.1\r\nHost: {addr}\r\n\r\n");
    stream.write_all(request.as_bytes()).await.expect("write");
    let mut raw = Vec::new();
    tokio::time::timeout(PATIENCE, stream.read_to_end(&mut raw))
        .await
        .expect("timed out")
        .expect("read");
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (_, body) = text.split_once("\r\n\r\n").expect("response head");
    serde_json::from_str(body).expect("json body")
}

// ---- Puppeteer ----------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn puppeteer_connects_discovers_opens_and_closes_a_page() {
    let fixture = Fixture::start().await;
    let mut client = fixture.browser().await;

    let version = client.result(None, "Browser.getVersion", json!({})).await;
    assert!(
        version["product"]
            .as_str()
            .unwrap()
            .starts_with("TentaBrowser/")
    );

    client
        .result(
            None,
            "Target.setDiscoverTargets",
            json!({ "discover": true, "filter": [{}] }),
        )
        .await;
    let browser = client.event("Target.targetCreated").await;
    assert_eq!(browser["params"]["targetInfo"]["type"], "browser");
    assert_eq!(
        browser["params"]["targetInfo"]["targetId"],
        fixture.server.backend().browser_id()
    );

    client
        .result(
            None,
            "Target.setAutoAttach",
            json!({ "autoAttach": true, "waitForDebuggerOnStart": true, "flatten": true }),
        )
        .await;

    let create = client
        .send(None, "Target.createTarget", json!({ "url": "about:blank" }))
        .await;
    let created = client.response(create).await;
    let target_id = created["result"]["targetId"].as_str().expect("targetId");
    // Chromium announces the target before answering createTarget, and
    // Playwright reads the page from the announcement.
    let first = client
        .backlog
        .first()
        .expect("an event before the response");
    assert_eq!(first["method"], "Target.targetCreated");
    assert!(client.buffered("Target.attachedToTarget"));

    let created_event = client.event("Target.targetCreated").await;
    let info = &created_event["params"]["targetInfo"];
    assert_eq!(info["targetId"], target_id);
    assert_eq!(info["type"], "page");
    assert_eq!(info["url"], "about:blank");
    assert_eq!(info["canAccessOpener"], false);
    assert_eq!(info["browserContextId"], fixture.default_context().as_str());

    let attached = client.event("Target.attachedToTarget").await;
    let session = session_of(&attached);
    assert_eq!(session, format!("{target_id}-session"));
    assert_eq!(attached["params"]["waitingForDebugger"], true);
    assert_eq!(attached["params"]["targetInfo"]["targetId"], target_id);
    assert_eq!(attached["params"]["targetInfo"]["attached"], true);

    client
        .result(
            Some(&session),
            "Target.setAutoAttach",
            json!({ "autoAttach": true, "waitForDebuggerOnStart": true, "flatten": true }),
        )
        .await;
    client
        .result(Some(&session), "Runtime.runIfWaitingForDebugger", json!({}))
        .await;
    // A session that is not (or no longer) waiting still succeeds.
    client
        .result(Some(&session), "Runtime.runIfWaitingForDebugger", json!({}))
        .await;

    let close = client
        .send(None, "Target.closeTarget", json!({ "targetId": target_id }))
        .await;
    let closed = client.response(close).await;
    assert_eq!(closed["result"]["success"], true);
    assert!(client.buffered("Target.detachedFromTarget"));
    assert!(client.buffered("Target.targetDestroyed"));
    let detached = client.event("Target.detachedFromTarget").await;
    assert_eq!(detached["params"]["sessionId"], session.as_str());
    let destroyed = client.event("Target.targetDestroyed").await;
    assert_eq!(destroyed["params"]["targetId"], target_id);

    let targets = client.result(None, "Target.getTargets", json!({})).await;
    assert_eq!(targets["targetInfos"], json!([]));
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_waiting_target_does_not_navigate_until_it_is_released() {
    let fixture = Fixture::start().await;
    let site = Site::start().await;
    let mut client = fixture.browser().await;
    client
        .result(
            None,
            "Target.setDiscoverTargets",
            json!({ "discover": true }),
        )
        .await;
    client
        .result(
            None,
            "Target.setAutoAttach",
            json!({ "autoAttach": true, "waitForDebuggerOnStart": true, "flatten": true }),
        )
        .await;

    let created = client
        .result(None, "Target.createTarget", json!({ "url": site.url() }))
        .await;
    let target_id = created["targetId"].as_str().unwrap().to_owned();
    let attached = client.event("Target.attachedToTarget").await;
    assert_eq!(attached["params"]["waitingForDebugger"], true);
    let session = session_of(&attached);

    // The creator's hold was dropped before the announcement went out, so a
    // page that was not held back would already be on its way.
    assert_eq!(site.hits(), 0);

    // Nothing may have been requested: the page has not been released. The
    // round trips below give a navigation that started anyway time to show.
    client.result(None, "Browser.getVersion", json!({})).await;
    client.result(None, "Browser.getVersion", json!({})).await;
    let page = fixture.server.backend().target(&target_id).expect("target");
    assert_eq!(site.hits(), 0);
    assert!(page.page().navigation().is_none());

    client
        .result(Some(&session), "Runtime.runIfWaitingForDebugger", json!({}))
        .await;
    loop {
        let changed = client.event("Target.targetInfoChanged").await;
        let info = &changed["params"]["targetInfo"];
        assert_eq!(info["targetId"], target_id.as_str());
        if info["url"] == site.url().as_str() {
            break;
        }
    }
    assert_eq!(site.hits(), 1);
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_debugger_that_detaches_resumes_the_target() {
    let fixture = Fixture::start().await;
    let site = Site::start().await;
    let mut client = fixture.browser().await;
    client
        .result(
            None,
            "Target.setDiscoverTargets",
            json!({ "discover": true }),
        )
        .await;
    client
        .result(
            None,
            "Target.setAutoAttach",
            json!({ "autoAttach": true, "waitForDebuggerOnStart": true, "flatten": true }),
        )
        .await;
    client
        .result(None, "Target.createTarget", json!({ "url": site.url() }))
        .await;
    let session = session_of(&client.event("Target.attachedToTarget").await);

    client
        .result(
            None,
            "Target.detachFromTarget",
            json!({ "sessionId": session }),
        )
        .await;
    loop {
        let changed = client.event("Target.targetInfoChanged").await;
        if changed["params"]["targetInfo"]["url"] == site.url().as_str() {
            break;
        }
    }
    assert_eq!(site.hits(), 1);
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_waiting_the_target_navigates_at_once() {
    let fixture = Fixture::start().await;
    let site = Site::start().await;
    let mut client = fixture.browser().await;
    client
        .result(
            None,
            "Target.setDiscoverTargets",
            json!({ "discover": true }),
        )
        .await;
    client
        .result(
            None,
            "Target.setAutoAttach",
            json!({ "autoAttach": true, "waitForDebuggerOnStart": false, "flatten": true }),
        )
        .await;
    client
        .result(None, "Target.createTarget", json!({ "url": site.url() }))
        .await;
    let attached = client.event("Target.attachedToTarget").await;
    assert_eq!(attached["params"]["waitingForDebugger"], false);
    loop {
        let changed = client.event("Target.targetInfoChanged").await;
        if changed["params"]["targetInfo"]["url"] == site.url().as_str() {
            break;
        }
    }
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_target_info_carries_the_title_of_its_document() {
    let fixture = Fixture::start().await;
    let site = Site::start().await;
    let mut client = fixture.browser().await;
    let target_id = client
        .result(None, "Target.createTarget", json!({ "url": "about:blank" }))
        .await["targetId"]
        .as_str()
        .unwrap()
        .to_owned();
    let blank = client
        .result(
            None,
            "Target.getTargetInfo",
            json!({ "targetId": target_id }),
        )
        .await;
    assert_eq!(blank["targetInfo"]["title"], "about:blank");

    let target = fixture.server.backend().target(&target_id).expect("target");
    target
        .page()
        .goto(&site.url(), Goto::default())
        .await
        .expect("navigation");
    let info = client
        .result(
            None,
            "Target.getTargetInfo",
            json!({ "targetId": target_id }),
        )
        .await;
    assert_eq!(info["targetInfo"]["title"], "Hello");
    assert_eq!(info["targetInfo"]["url"], site.url().as_str());
    fixture.stop().await;
}

// ---- Playwright ---------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn playwright_opens_pages_in_a_context_and_disposes_it() {
    let fixture = Fixture::start().await;
    let mut client = fixture.browser().await;

    client.result(None, "Browser.getVersion", json!({})).await;
    client
        .result(
            None,
            "Target.setAutoAttach",
            json!({ "waitForDebuggerOnStart": true, "flatten": true, "autoAttach": true }),
        )
        .await;
    let browser = client.result(None, "Target.getTargetInfo", json!({})).await;
    assert_eq!(browser["targetInfo"]["type"], "browser");

    let context = client
        .result(
            None,
            "Target.createBrowserContext",
            json!({ "disposeOnDetach": true }),
        )
        .await["browserContextId"]
        .as_str()
        .unwrap()
        .to_owned();
    let listed = client
        .result(None, "Target.getBrowserContexts", json!({}))
        .await;
    assert_eq!(listed["browserContextIds"], json!([context]));

    let created = client
        .send(
            None,
            "Target.createTarget",
            json!({ "url": "about:blank", "browserContextId": context }),
        )
        .await;
    let created = client.response(created).await;
    let target_id = created["result"]["targetId"].as_str().unwrap();
    assert!(client.buffered("Target.attachedToTarget"));
    let attached = client.event("Target.attachedToTarget").await;
    assert_eq!(attached["params"]["waitingForDebugger"], true);
    assert_eq!(
        attached["params"]["targetInfo"]["browserContextId"],
        context.as_str()
    );
    let session = session_of(&attached);

    // What Playwright sends to every page session before it lets it run.
    client
        .result(
            Some(&session),
            "Target.setAutoAttach",
            json!({ "autoAttach": true, "waitForDebuggerOnStart": true, "flatten": true }),
        )
        .await;
    client
        .result(Some(&session), "Runtime.runIfWaitingForDebugger", json!({}))
        .await;
    let own = client
        .result(Some(&session), "Target.getTargetInfo", json!({}))
        .await;
    assert_eq!(own["targetInfo"]["targetId"], target_id);

    let disposed = client
        .send(
            None,
            "Target.disposeBrowserContext",
            json!({ "browserContextId": context }),
        )
        .await;
    client.response(disposed).await;
    assert!(client.buffered("Target.detachedFromTarget"));
    let detached = client.event("Target.detachedFromTarget").await;
    assert_eq!(detached["params"]["sessionId"], session.as_str());

    let listed = client
        .result(None, "Target.getBrowserContexts", json!({}))
        .await;
    assert_eq!(listed["browserContextIds"], json!([]));
    let targets = client.result(None, "Target.getTargets", json!({})).await;
    assert_eq!(targets["targetInfos"], json!([]));

    let params = json!({ "url": "about:blank", "browserContextId": context });
    assert_eq!(
        client.error_code(None, "Target.createTarget", params).await,
        -32602
    );
    let params = json!({ "browserContextId": context });
    assert_eq!(
        client
            .error_code(None, "Target.disposeBrowserContext", params)
            .await,
        -32602
    );
    let params = json!({ "browserContextId": fixture.default_context() });
    assert_eq!(
        client
            .error_code(None, "Target.disposeBrowserContext", params)
            .await,
        -32000
    );
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_context_made_with_dispose_on_detach_dies_with_its_connection() {
    let fixture = Fixture::start().await;
    let mut events = fixture.server.backend().subscribe();
    let mut client = fixture.browser().await;
    let context = client
        .result(
            None,
            "Target.createBrowserContext",
            json!({ "disposeOnDetach": true }),
        )
        .await["browserContextId"]
        .as_str()
        .unwrap()
        .to_owned();
    let kept = client
        .result(None, "Target.createBrowserContext", json!({}))
        .await["browserContextId"]
        .as_str()
        .unwrap()
        .to_owned();
    let target = client
        .result(
            None,
            "Target.createTarget",
            json!({ "url": "about:blank", "browserContextId": context }),
        )
        .await["targetId"]
        .as_str()
        .unwrap()
        .to_owned();
    drop(client);

    tokio::time::timeout(PATIENCE, async {
        loop {
            if let Ok(TargetEvent::Destroyed(gone)) = events.recv().await
                && gone.id() == target
            {
                break;
            }
        }
    })
    .await
    .expect("the context was never disposed of");
    let backend = fixture.server.backend();
    assert!(backend.context(&context).is_none());
    assert!(backend.context(&kept).is_some());
    fixture.stop().await;
}

// ---- discovery across connections ---------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_connection_discovers_existing_and_new_targets() {
    let fixture = Fixture::start().await;
    let mut opener = fixture.browser().await;
    let first = opener
        .result(None, "Target.createTarget", json!({ "url": "about:blank" }))
        .await["targetId"]
        .as_str()
        .unwrap()
        .to_owned();

    let mut watcher = fixture.browser().await;
    watcher
        .result(
            None,
            "Target.setDiscoverTargets",
            json!({ "discover": true }),
        )
        .await;
    let mut seen = Vec::new();
    while seen.is_empty() {
        let event = watcher.event("Target.targetCreated").await;
        seen.push(
            event["params"]["targetInfo"]["targetId"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    assert!(seen.contains(&first));

    let second = opener
        .result(None, "Target.createTarget", json!({ "url": "about:blank" }))
        .await["targetId"]
        .as_str()
        .unwrap()
        .to_owned();
    let live = watcher.event("Target.targetCreated").await;
    assert_eq!(live["params"]["targetInfo"]["targetId"], second.as_str());

    opener
        .result(None, "Target.closeTarget", json!({ "targetId": second }))
        .await;
    let destroyed = watcher.event("Target.targetDestroyed").await;
    assert_eq!(destroyed["params"]["targetId"], second.as_str());

    // Switching discovery off stops the events; switching it on again
    // announces what exists once more.
    watcher
        .result(
            None,
            "Target.setDiscoverTargets",
            json!({ "discover": false }),
        )
        .await;
    opener
        .result(None, "Target.closeTarget", json!({ "targetId": first }))
        .await;
    watcher
        .result(
            None,
            "Target.setDiscoverTargets",
            json!({ "discover": true, "filter": [{}] }),
        )
        .await;
    let again = watcher.event("Target.targetCreated").await;
    assert_eq!(again["params"]["targetInfo"]["type"], "browser");
    assert!(!watcher.buffered("Target.targetDestroyed"));
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_target_made_over_http_is_attached_without_waiting() {
    let fixture = Fixture::start().await;
    let mut client = fixture.browser().await;
    client
        .result(
            None,
            "Target.setAutoAttach",
            json!({ "autoAttach": true, "waitForDebuggerOnStart": true, "flatten": true }),
        )
        .await;
    let created = put(fixture.server.local_addr(), "/json/new?about:blank").await;
    let attached = client.event("Target.attachedToTarget").await;
    assert_eq!(attached["params"]["targetInfo"]["targetId"], created["id"]);
    assert_eq!(attached["params"]["waitingForDebugger"], false);
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auto_attach_covers_targets_that_already_exist() {
    let fixture = Fixture::start().await;
    let mut opener = fixture.browser().await;
    let existing = opener
        .result(None, "Target.createTarget", json!({ "url": "about:blank" }))
        .await["targetId"]
        .as_str()
        .unwrap()
        .to_owned();

    let mut client = fixture.browser().await;
    client
        .result(
            None,
            "Target.setAutoAttach",
            json!({ "autoAttach": true, "waitForDebuggerOnStart": true, "flatten": true }),
        )
        .await;
    let attached = client.event("Target.attachedToTarget").await;
    assert_eq!(
        attached["params"]["targetInfo"]["targetId"],
        existing.as_str()
    );
    assert_eq!(attached["params"]["waitingForDebugger"], false);
    // The opener's connection has a session of its own for the same target.
    assert_ne!(session_of(&attached), "");

    assert_eq!(
        client
            .error_code(
                None,
                "Target.setAutoAttach",
                json!({ "autoAttach": true, "waitForDebuggerOnStart": false }),
            )
            .await,
        -32000,
        "only flattened sessions are supported"
    );
    fixture.stop().await;
}

// ---- Storage ------------------------------------------------------------

async fn context_id(client: &mut Client) -> String {
    client
        .result(None, "Target.createBrowserContext", json!({}))
        .await["browserContextId"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn named<'a>(cookies: &'a Value, name: &str) -> &'a Value {
    cookies["cookies"]
        .as_array()
        .unwrap()
        .iter()
        .find(|cookie| cookie["name"] == name)
        .unwrap_or_else(|| panic!("no cookie {name} in {cookies}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cookies_are_set_read_and_cleared_per_context() {
    let fixture = Fixture::start().await;
    let site = Site::start().await;
    let mut client = fixture.browser().await;
    let first = context_id(&mut client).await;
    let second = context_id(&mut client).await;

    client
        .result(
            None,
            "Storage.setCookies",
            json!({
                "browserContextId": first,
                "cookies": [
                    { "name": "a", "value": "1", "url": "http://example.test/app/page" },
                    {
                        "name": "d", "value": "2", "domain": "example.test", "path": "/",
                        "secure": true, "httpOnly": true, "sameSite": "Strict",
                        "expires": 4_102_444_800_u64,
                    },
                    { "name": "site", "value": "mine", "url": site.url() },
                ],
            }),
        )
        .await;

    let cookies = client
        .result(
            None,
            "Storage.getCookies",
            json!({ "browserContextId": first }),
        )
        .await;
    assert_eq!(cookies["cookies"].as_array().unwrap().len(), 3);
    let a = named(&cookies, "a");
    assert_eq!(a["value"], "1");
    assert_eq!(a["domain"], "example.test");
    assert_eq!(a["path"], "/app");
    assert_eq!(a["session"], true);
    assert_eq!(a["expires"], -1.0);
    assert_eq!(a["size"], 2);
    assert_eq!(a["httpOnly"], false);
    assert_eq!(a["secure"], false);
    let d = named(&cookies, "d");
    assert_eq!(d["domain"], ".example.test");
    assert_eq!(d["path"], "/");
    assert_eq!(d["secure"], true);
    assert_eq!(d["httpOnly"], true);
    assert_eq!(d["sameSite"], "Strict");
    assert_eq!(d["session"], false);
    assert_eq!(d["expires"], 4_102_444_800.0);

    for other in [Some(second.as_str()), None] {
        let params = other.map_or_else(|| json!({}), |id| json!({ "browserContextId": id }));
        let empty = client.result(None, "Storage.getCookies", params).await;
        assert_eq!(empty["cookies"], json!([]), "context {other:?}");
    }

    // The jar is the one pages use: the cookie goes out with a navigation of
    // the context that owns it and with no other.
    let backend = fixture.server.backend();
    for (context, expected) in [(&first, true), (&second, false)] {
        let target = client
            .result(
                None,
                "Target.createTarget",
                json!({ "url": "about:blank", "browserContextId": context }),
            )
            .await["targetId"]
            .as_str()
            .unwrap()
            .to_owned();
        let before = site.request_headers().len();
        let page = backend.target(&target).expect("target");
        page.page()
            .goto(&site.url(), Goto::default())
            .await
            .expect("navigation");
        let requests = site.request_headers();
        let head = requests[before].to_ascii_lowercase();
        assert_eq!(head.contains("cookie: site=mine"), expected, "{head}");
    }

    client
        .result(
            None,
            "Storage.setCookies",
            json!({
                "browserContextId": second,
                "cookies": [{ "name": "a", "value": "other", "url": "http://example.test/" }],
            }),
        )
        .await;
    let theirs = client
        .result(
            None,
            "Storage.getCookies",
            json!({ "browserContextId": second }),
        )
        .await;
    assert_eq!(named(&theirs, "a")["value"], "other");
    let mine = client
        .result(
            None,
            "Storage.getCookies",
            json!({ "browserContextId": first }),
        )
        .await;
    assert_eq!(named(&mine, "a")["value"], "1");

    client
        .result(
            None,
            "Storage.clearCookies",
            json!({ "browserContextId": first }),
        )
        .await;
    let cleared = client
        .result(
            None,
            "Storage.getCookies",
            json!({ "browserContextId": first }),
        )
        .await;
    assert_eq!(cleared["cookies"], json!([]));
    let kept = client
        .result(
            None,
            "Storage.getCookies",
            json!({ "browserContextId": second }),
        )
        .await;
    assert_eq!(kept["cookies"].as_array().unwrap().len(), 1);
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_cookies_and_contexts_are_refused_without_a_partial_write() {
    let fixture = Fixture::start().await;
    let mut client = fixture.browser().await;

    let bad = [
        json!({ "name": "a", "value": "1" }),
        json!({ "name": "a", "value": "1", "url": "http://example.test/", "sameSite": "Sideways" }),
        json!({ "name": "a", "value": "1", "url": "not a url" }),
    ];
    for cookie in bad {
        let code = client
            .error_code(None, "Storage.setCookies", json!({ "cookies": [cookie] }))
            .await;
        assert_eq!(code, -32602);
    }
    let good = json!({ "name": "ok", "value": "1", "url": "http://example.test/" });
    let nameless = json!({ "name": "", "value": "", "url": "http://example.test/" });
    let code = client
        .error_code(
            None,
            "Storage.setCookies",
            json!({ "cookies": [good, nameless] }),
        )
        .await;
    assert_eq!(code, -32000);
    let cookies = client.result(None, "Storage.getCookies", json!({})).await;
    assert_eq!(cookies["cookies"], json!([]));

    for method in ["Storage.getCookies", "Storage.clearCookies"] {
        let code = client
            .error_code(None, method, json!({ "browserContextId": "nope" }))
            .await;
        assert_eq!(code, -32602, "{method}");
    }
    fixture.stop().await;
}

// ---- Browser, Inspector, Log, Security ----------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn download_behavior_is_validated_per_context() {
    let fixture = Fixture::start().await;
    let mut client = fixture.browser().await;
    let context = context_id(&mut client).await;

    for params in [
        json!({ "behavior": "deny" }),
        json!({ "behavior": "default", "browserContextId": context }),
        json!({ "behavior": "allow", "downloadPath": "/tmp/downloads", "eventsEnabled": true }),
        json!({ "behavior": "allowAndName", "downloadPath": "/tmp/downloads", "browserContextId": context }),
    ] {
        client
            .result(None, "Browser.setDownloadBehavior", params)
            .await;
    }
    for params in [
        json!({ "behavior": "allow" }),
        json!({ "behavior": "allowAndName", "downloadPath": "" }),
        json!({ "behavior": "sometimes" }),
        json!({ "behavior": "deny", "browserContextId": "nope" }),
        json!({}),
    ] {
        let code = client
            .error_code(None, "Browser.setDownloadBehavior", params.clone())
            .await;
        assert_eq!(code, -32602, "{params}");
    }
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn small_domains_accept_what_the_clients_send() {
    let fixture = Fixture::start().await;
    let mut client = fixture.browser().await;
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

    for method in [
        "Inspector.enable",
        "Inspector.disable",
        "Log.enable",
        "Log.disable",
        "Security.enable",
        "Security.disable",
    ] {
        client.result(Some(&session), method, json!({})).await;
    }
    client
        .result(
            None,
            "Security.setIgnoreCertificateErrors",
            json!({ "ignore": false }),
        )
        .await;
    assert_eq!(
        client
            .error_code(None, "Security.setIgnoreCertificateErrors", json!({}))
            .await,
        -32602
    );
    client
        .result(None, "Target.activateTarget", json!({ "targetId": target }))
        .await;
    assert_eq!(
        client
            .error_code(None, "Target.activateTarget", json!({ "targetId": "nope" }))
            .await,
        -32602
    );
    assert_eq!(
        client
            .error_code(None, "Target.getTargetInfo", json!({ "targetId": "nope" }))
            .await,
        -32602
    );
    assert_eq!(
        client
            .error_code(None, "Target.createTarget", json!({ "url": "not a url" }))
            .await,
        -32602
    );
    assert_eq!(
        client
            .error_code(
                None,
                "Target.createTarget",
                json!({ "url": "about:blank", "browserContextId": "nope" }),
            )
            .await,
        -32602
    );
    // A context made after the flag was set still opens pages.
    let context = context_id(&mut client).await;
    client
        .result(
            None,
            "Target.createTarget",
            json!({ "url": "about:blank", "browserContextId": context }),
        )
        .await;
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn methods_nobody_implements_still_answer_32601() {
    let fixture = Fixture::start().await;
    let mut client = fixture.browser().await;
    for method in [
        "Target.getDevToolsTarget",
        "Target.openDevTools",
        "Browser.setWindowBounds",
        "Runtime.queryObjects",
        "Page.printToPDF",
        "Inspector.nonsense",
    ] {
        assert_eq!(
            client.error_code(None, method, json!({})).await,
            -32601,
            "{method}"
        );
    }
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_the_browser_answers_then_shuts_down_and_ends_the_connection() {
    let fixture = Fixture::start().await;
    let mut client = fixture.browser().await;
    let target = client
        .result(None, "Target.createTarget", json!({ "url": "about:blank" }))
        .await["targetId"]
        .as_str()
        .unwrap()
        .to_owned();
    let page = fixture.server.backend().target(&target).expect("target");

    let closing = client.send(None, "Browser.close", json!({})).await;
    let answer = client.response(closing).await;
    assert_eq!(answer["result"], json!({}));
    assert!(client.ended().await);
    assert!(page.page().is_closed());
    assert!(fixture.engine.new_context(Default::default()).is_err());
    fixture.stop().await;
}

// ---- attachToBrowserTarget ----------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn playwright_connect_over_cdp_uses_a_browser_session() {
    let fixture = Fixture::start().await;
    let mut client = fixture.browser().await;
    client.result(None, "Browser.getVersion", json!({})).await;
    let session = client
        .result(None, "Target.attachToBrowserTarget", json!({}))
        .await["sessionId"]
        .as_str()
        .unwrap()
        .to_owned();

    // Browser-level commands work on it, and it auto-attaches like the
    // connection's own session.
    let info = client
        .result(Some(&session), "Target.getTargetInfo", json!({}))
        .await;
    assert_eq!(info["targetInfo"]["type"], "browser");
    client
        .result(
            Some(&session),
            "Target.setAutoAttach",
            json!({ "autoAttach": true, "waitForDebuggerOnStart": true, "flatten": true }),
        )
        .await;
    let created = client
        .call(
            Some(&session),
            "Target.createTarget",
            json!({ "url": "about:blank" }),
        )
        .await;
    assert_eq!(created["sessionId"], session.as_str());
    let attached = client.event("Target.attachedToTarget").await;
    assert_eq!(attached["sessionId"], session.as_str());
    assert_eq!(attached["params"]["waitingForDebugger"], true);

    client
        .result(
            None,
            "Target.detachFromTarget",
            json!({ "sessionId": session }),
        )
        .await;
    let detached = client.event("Target.detachedFromTarget").await;
    assert_eq!(detached["params"]["sessionId"], session.as_str());
    assert_eq!(
        client
            .error_code(Some(&session), "Browser.getVersion", json!({}))
            .await,
        -32001
    );
    fixture.stop().await;
}

// ---- Session::spawn -----------------------------------------------------

/// Drops `session` before `done`, so `done` signals only if the session's drop
/// returned.
struct Holder {
    _session: Arc<Session>,
    _done: oneshot::Sender<()>,
}

struct Hoard {
    done: Mutex<Option<oneshot::Receiver<()>>>,
}

impl DomainHandler for Hoard {
    fn domain(&self) -> &'static str {
        "Hoard"
    }

    fn methods(&self) -> &'static [&'static str] {
        &["hold", "released"]
    }

    fn handle<'a>(
        &'a self,
        cx: CommandContext,
        method: &'a str,
        _params: Value,
    ) -> CommandFuture<'a> {
        Box::pin(async move {
            if method == "released" {
                let done = self.done.lock().unwrap().take().expect("hold first");
                let _ = done.await;
                return Ok(json!({}));
            }
            let (sender, receiver) = oneshot::channel();
            *self.done.lock().unwrap() = Some(receiver);
            let holder = Holder {
                _session: Arc::clone(cx.session()),
                _done: sender,
            };
            cx.session().spawn(async move {
                let _holder = holder;
                std::future::pending::<()>().await;
            });
            Ok(json!({}))
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_task_holding_the_last_reference_to_its_session_does_not_deadlock() {
    let engine = Engine::builder()
        .page_threads(1)
        .ephemeral(true)
        .max_tier(Tier::Dom)
        .build()
        .expect("engine");
    let mut registry = DomainRegistry::with_defaults();
    registry.register(Hoard {
        done: Mutex::new(None),
    });
    let options = CdpOptions {
        bind: "127.0.0.1:0".parse().unwrap(),
        ..CdpOptions::default()
    };
    let server = CdpServer::bind_with(engine.clone(), options, registry)
        .await
        .expect("bind");
    let mut client = Client::connect(&server.browser_ws_url()).await;
    let session = client
        .result(None, "Target.attachToBrowserTarget", json!({}))
        .await["sessionId"]
        .as_str()
        .unwrap()
        .to_owned();
    client.result(Some(&session), "Hoard.hold", json!({})).await;
    client
        .result(
            None,
            "Target.detachFromTarget",
            json!({ "sessionId": session }),
        )
        .await;
    // Resolves only once the task, and with it the last reference to the
    // session, has been dropped.
    let released = client.send(None, "Hoard.released", json!({})).await;
    let answer = client.response(released).await;
    assert!(answer.get("error").is_none(), "{answer}");
    server.shutdown().await;
    engine.close();
}

// ---- filters, security scope, proxy -------------------------------------

async fn first_target_event(client: &mut Client, method: &str) -> Value {
    client.event(method).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn target_filters_decide_by_first_match_with_chromiums_default() {
    let fixture = Fixture::start().await;
    let mut opener = fixture.browser().await;
    let page = opener
        .result(None, "Target.createTarget", json!({ "url": "about:blank" }))
        .await["targetId"]
        .as_str()
        .unwrap()
        .to_owned();

    // No filter: pages, and not the browser.
    let mut plain = fixture.browser().await;
    plain
        .result(
            None,
            "Target.setDiscoverTargets",
            json!({ "discover": true }),
        )
        .await;
    let created = first_target_event(&mut plain, "Target.targetCreated").await;
    assert_eq!(created["params"]["targetInfo"]["targetId"], page.as_str());
    assert!(!plain.buffered("Target.targetCreated"));

    // Excluding pages and then matching everything leaves the browser only.
    let mut no_pages = fixture.browser().await;
    no_pages
        .result(
            None,
            "Target.setDiscoverTargets",
            json!({ "discover": true, "filter": [{ "type": "page", "exclude": true }, {}] }),
        )
        .await;
    let created = first_target_event(&mut no_pages, "Target.targetCreated").await;
    assert_eq!(created["params"]["targetInfo"]["type"], "browser");
    no_pages
        .result(
            None,
            "Target.setAutoAttach",
            json!({
                "autoAttach": true, "waitForDebuggerOnStart": false, "flatten": true,
                "filter": [{ "type": "page", "exclude": true }, {}],
            }),
        )
        .await;
    let second = opener
        .result(None, "Target.createTarget", json!({ "url": "about:blank" }))
        .await["targetId"]
        .as_str()
        .unwrap()
        .to_owned();
    // The plain watcher hears of the new page; the filtered one hears of
    // nothing, which the next round trip would have exposed.
    let live = first_target_event(&mut plain, "Target.targetCreated").await;
    assert_eq!(live["params"]["targetInfo"]["targetId"], second.as_str());
    no_pages.result(None, "Browser.getVersion", json!({})).await;
    assert!(!no_pages.buffered("Target.targetCreated"));
    assert!(!no_pages.buffered("Target.attachedToTarget"));

    // An empty filter matches nothing, and the default auto-attach filter
    // still admits pages.
    let mut nothing = fixture.browser().await;
    nothing
        .result(
            None,
            "Target.setDiscoverTargets",
            json!({ "discover": true, "filter": [] }),
        )
        .await;
    nothing.result(None, "Browser.getVersion", json!({})).await;
    assert!(!nothing.buffered("Target.targetCreated"));
    nothing
        .result(
            None,
            "Target.setAutoAttach",
            json!({ "autoAttach": true, "waitForDebuggerOnStart": false, "flatten": true }),
        )
        .await;
    first_target_event(&mut nothing, "Target.attachedToTarget").await;
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ignoring_certificate_errors_succeeds_and_is_scoped_to_the_connection() {
    let fixture = Fixture::start().await;
    let mut first = fixture.browser().await;
    let mut second = fixture.browser().await;
    for ignore in [true, false] {
        first
            .result(
                None,
                "Security.setIgnoreCertificateErrors",
                json!({ "ignore": ignore }),
            )
            .await;
    }
    first
        .result(
            None,
            "Security.setIgnoreCertificateErrors",
            json!({ "ignore": true }),
        )
        .await;
    context_id(&mut first).await;
    context_id(&mut second).await;
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_proxy_bypass_list_is_refused() {
    let fixture = Fixture::start().await;
    let mut client = fixture.browser().await;
    assert_eq!(
        client
            .error_code(
                None,
                "Target.createBrowserContext",
                json!({ "proxyServer": "http://127.0.0.1:1", "proxyBypassList": "localhost" }),
            )
            .await,
        -32602
    );
    client
        .result(
            None,
            "Target.createBrowserContext",
            json!({ "proxyServer": "http://127.0.0.1:1", "proxyBypassList": "" }),
        )
        .await;
    fixture.stop().await;
}
