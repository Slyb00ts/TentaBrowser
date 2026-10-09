//! The Page domain against a real page engine, in the command sequences
//! Puppeteer and Playwright send: enable, lifecycle events, navigation,
//! screenshots, layout metrics and scripts for new documents.

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
const PATIENCE: Duration = Duration::from_secs(30);

// ---- fixture ------------------------------------------------------------

/// Serves fixed pages by path on a loopback port.
struct Site {
    base: String,
    /// `/slow` answers once this is notified.
    gate: Arc<Notify>,
}

impl Site {
    async fn start(pages: &[(&str, &str)]) -> Self {
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

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }
}

struct Fixture {
    server: CdpServer,
    engine: Engine,
    site: Site,
}

impl Fixture {
    async fn start(pages: &[(&str, &str)]) -> Self {
        Self::start_with(pages, |_| {}).await
    }

    async fn start_with(pages: &[(&str, &str)], configure: impl FnOnce(&mut CdpOptions)) -> Self {
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
    async fn page(&self) -> Page {
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
    async fn attach(&self, target: &str) -> Page {
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

    async fn stop(self) {
        self.server.shutdown().await;
        self.engine.close();
    }
}

struct Client {
    ws: WebSocketStream<TcpStream>,
    next_id: u64,
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

    /// The next event of any name, from the backlog first.
    async fn next_event(&mut self) -> Value {
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
    async fn event(&mut self, method: &str) -> Value {
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
struct Page {
    client: Client,
    session: String,
    target: String,
}

impl Page {
    async fn call(&mut self, method: &str, params: Value) -> Value {
        let session = self.session.clone();
        self.client.call(Some(&session), method, params).await
    }

    async fn result(&mut self, method: &str, params: Value) -> Value {
        let session = self.session.clone();
        self.client.result(Some(&session), method, params).await
    }

    async fn event(&mut self, method: &str) -> Value {
        self.client.event(method).await
    }

    async fn evaluate(&mut self, expression: &str) -> Value {
        self.result(
            "Runtime.evaluate",
            json!({ "expression": expression, "returnByValue": true }),
        )
        .await["result"]["value"]
            .clone()
    }

    /// `Page.enable` and `setLifecycleEventsEnabled`, the way Puppeteer begins.
    async fn begin(&mut self) {
        self.result("Page.enable", json!({})).await;
        self.result("Page.setLifecycleEventsEnabled", json!({ "enabled": true }))
            .await;
    }

    /// The names of the events from now on up to and including the lifecycle
    /// event `last` of `loader`, as `Page.x` or `lifecycle:name`.
    async fn events_through(&mut self, loader: &str, last: &str) -> Vec<String> {
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
    async fn events_until(&mut self, last: &str) -> Vec<String> {
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
    async fn navigate(&mut self, url: &str) -> String {
        self.result("Page.navigate", json!({ "url": url })).await["loaderId"]
            .as_str()
            .expect("a loader id")
            .to_owned()
    }

    async fn screenshot(&mut self, params: Value) -> Vec<u8> {
        let data = self.result("Page.captureScreenshot", params).await["data"]
            .as_str()
            .expect("base64 data")
            .to_owned();
        base64::engine::general_purpose::STANDARD
            .decode(data)
            .expect("valid base64")
    }
}

struct Decoded {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl Decoded {
    fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * self.width + x) * 4) as usize;
        self.rgba[at..at + 4].try_into().expect("four channels")
    }
}

fn decode_png(bytes: &[u8]) -> Decoded {
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

const INDEX: &str = "<!doctype html><html><head><title>Fixture</title></head>\
    <body><p>hello</p><script>document.title += '!';</script></body></html>";

/// Two thousand pixels of red under a zero margin: taller than the viewport.
const TALL: &str = "<!doctype html><html><body style=\"margin:0\">\
    <div style=\"height:2000px;background:rgb(255,0,0)\"></div></body></html>";

fn error_of(response: &Value) -> (i64, &str) {
    (
        response["error"]["code"].as_i64().expect("an error"),
        response["error"]["message"].as_str().unwrap_or_default(),
    )
}

// ---- navigation and lifecycle --------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_navigation_reports_its_lifecycle_in_order_under_one_loader() {
    let fixture = Fixture::start(&[("/", INDEX)]).await;
    let mut page = fixture.page().await;

    // The blank document the page starts with is already complete.
    page.result("Page.enable", json!({})).await;
    let tree = page.result("Page.getFrameTree", json!({})).await;
    let blank = tree["frameTree"]["frame"].clone();
    assert_eq!(blank["id"], page.target.as_str());
    assert_eq!(blank["url"], "about:blank");
    page.result("Page.setLifecycleEventsEnabled", json!({ "enabled": true }))
        .await;
    let blank_loader = blank["loaderId"].as_str().unwrap().to_owned();
    assert_eq!(
        page.events_through(&blank_loader, "networkIdle").await,
        [
            "lifecycle:init",
            "lifecycle:DOMContentLoaded",
            "lifecycle:load",
            "lifecycle:networkAlmostIdle",
            "lifecycle:networkIdle",
        ]
    );

    let url = fixture.site.url("/");
    let reply = page.result("Page.navigate", json!({ "url": url })).await;
    assert_eq!(reply["frameId"], page.target.as_str());
    assert!(reply.get("errorText").is_none(), "{reply}");
    let loader = reply["loaderId"].as_str().expect("a loader id").to_owned();
    assert_ne!(loader, blank_loader);

    let seen = page.events_through(&loader, "networkIdle").await;
    assert_eq!(
        seen,
        [
            "Page.frameStartedLoading",
            "lifecycle:init",
            "Page.frameNavigated",
            "lifecycle:commit",
            "lifecycle:DOMContentLoaded",
            "Page.domContentEventFired",
            "lifecycle:load",
            "Page.loadEventFired",
            "Page.frameStoppedLoading",
            "lifecycle:networkAlmostIdle",
            "lifecycle:networkIdle",
        ]
    );

    assert_eq!(page.evaluate("document.title").await, "Fixture!");
    let frame = page.result("Page.getFrameTree", json!({})).await["frameTree"]["frame"].clone();
    assert_eq!(frame["loaderId"], loader.as_str());
    assert_eq!(frame["url"], url.as_str());
    assert_eq!(frame["mimeType"], "text/html");
    assert_eq!(frame["securityOrigin"], fixture.site.base.as_str());
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn frame_navigated_carries_the_frame_of_the_new_document() {
    let fixture = Fixture::start(&[("/", INDEX)]).await;
    let mut page = fixture.page().await;
    page.begin().await;
    let reply = page
        .result("Page.navigate", json!({ "url": fixture.site.url("/") }))
        .await;
    let navigated = page.event("Page.frameNavigated").await;
    assert_eq!(navigated["type"], "Navigation");
    assert_eq!(navigated["frame"]["id"], page.target.as_str());
    assert_eq!(navigated["frame"]["loaderId"], reply["loaderId"]);
    assert_eq!(navigated["frame"]["url"], fixture.site.url("/").as_str());
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_http_error_status_is_a_navigation_without_error_text() {
    let fixture = Fixture::start(&[]).await;
    let mut page = fixture.page().await;
    page.begin().await;
    let reply = page
        .result(
            "Page.navigate",
            json!({ "url": fixture.site.url("/missing") }),
        )
        .await;
    assert!(reply.get("errorText").is_none(), "{reply}");
    assert!(reply["loaderId"].is_string());
    let loader = reply["loaderId"].as_str().unwrap().to_owned();
    page.events_through(&loader, "load").await;
    assert_eq!(
        page.evaluate("document.body.textContent").await,
        "not found"
    );
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_connection_is_an_error_text_and_an_error_document() {
    let fixture = Fixture::start(&[]).await;
    // A port nothing listens on: bound, then released.
    let refused = {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        listener.local_addr().unwrap()
    };
    let mut page = fixture.page().await;
    page.begin().await;
    let url = format!("http://{refused}/");
    let reply = page.result("Page.navigate", json!({ "url": url })).await;
    assert_eq!(reply["errorText"], "net::ERR_CONNECTION_REFUSED");
    assert_eq!(reply["frameId"], page.target.as_str());
    let frame = page.result("Page.getFrameTree", json!({})).await["frameTree"]["frame"].clone();
    assert_eq!(frame["unreachableUrl"], url.as_str());
    assert!(frame["url"].as_str().unwrap().starts_with("tenta-error:"));
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unresolvable_host_is_name_not_resolved() {
    let fixture = Fixture::start(&[]).await;
    let mut page = fixture.page().await;
    page.begin().await;
    // `.invalid` never resolves (RFC 6761).
    let reply = page
        .result("Page.navigate", json!({ "url": "http://nowhere.invalid/" }))
        .await;
    assert_eq!(reply["errorText"], "net::ERR_NAME_NOT_RESOLVED", "{reply}");
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn navigate_validates_its_parameters() {
    let fixture = Fixture::start(&[]).await;
    let mut page = fixture.page().await;
    let invalid = page
        .call("Page.navigate", json!({ "url": "not a url" }))
        .await;
    assert_eq!(error_of(&invalid).1, "Cannot navigate to invalid URL");
    let elsewhere = page
        .call(
            "Page.navigate",
            json!({ "url": fixture.site.url("/"), "frameId": "other" }),
        )
        .await;
    assert_eq!(error_of(&elsewhere).0, -32000);
    let missing = page.call("Page.navigate", json!({})).await;
    assert_eq!(error_of(&missing).0, -32602);
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reload_loads_the_same_url_under_a_new_loader() {
    let fixture = Fixture::start(&[("/", INDEX)]).await;
    let mut page = fixture.page().await;
    page.begin().await;
    let first = page
        .result("Page.navigate", json!({ "url": fixture.site.url("/") }))
        .await["loaderId"]
        .as_str()
        .unwrap()
        .to_owned();
    page.events_through(&first, "load").await;

    page.result("Page.reload", json!({})).await;
    let navigated = page.event("Page.frameNavigated").await;
    let second = navigated["frame"]["loaderId"].as_str().unwrap().to_owned();
    assert_ne!(second, first);
    assert_eq!(navigated["frame"]["url"], fixture.site.url("/").as_str());
    page.events_through(&second, "load").await;
    assert_eq!(page.evaluate("document.title").await, "Fixture!");
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_loading_aborts_a_navigation_waiting_for_its_response() {
    let fixture = Fixture::start(&[("/slow", INDEX)]).await;
    let mut page = fixture.page().await;
    page.begin().await;
    let session = page.session.clone();
    let url = fixture.site.url("/slow");
    // The session is free while the navigation waits for the response head.
    let navigate = page
        .client
        .send(Some(&session), "Page.navigate", json!({ "url": url }))
        .await;
    page.event("Page.frameStartedLoading").await;
    page.result("Page.stopLoading", json!({})).await;
    let reply = page.client.response(navigate).await;
    assert_eq!(reply["result"]["errorText"], "net::ERR_ABORTED", "{reply}");
    fixture.site.gate.notify_one();
    fixture.stop().await;
}

// ---- screenshots and metrics -------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_png_screenshot_is_the_viewport() {
    let fixture = Fixture::start(&[("/tall", TALL)]).await;
    let mut page = fixture.page().await;
    page.begin().await;
    let loader = page
        .result("Page.navigate", json!({ "url": fixture.site.url("/tall") }))
        .await["loaderId"]
        .as_str()
        .unwrap()
        .to_owned();
    page.events_through(&loader, "load").await;

    let shot = decode_png(&page.screenshot(json!({})).await);
    assert_eq!((shot.width, shot.height), (1280, 800));
    assert_eq!(shot.pixel(5, 5), [255, 0, 0, 255]);
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_jpeg_screenshot_decodes_at_the_viewport_size() {
    let fixture = Fixture::start(&[("/tall", TALL)]).await;
    let mut page = fixture.page().await;
    page.begin().await;
    let loader = page
        .result("Page.navigate", json!({ "url": fixture.site.url("/tall") }))
        .await["loaderId"]
        .as_str()
        .unwrap()
        .to_owned();
    page.events_through(&loader, "load").await;

    let bytes = page
        .screenshot(json!({ "format": "jpeg", "quality": 60 }))
        .await;
    let mut decoder =
        zune_jpeg::JpegDecoder::new(zune_jpeg::zune_core::bytestream::ZCursor::new(&bytes));
    let rgb = decoder.decode().expect("a decodable JPEG");
    assert_eq!(decoder.dimensions(), Some((1280, 800)));
    assert!(
        rgb[..3]
            .iter()
            .zip([255_u8, 0, 0])
            .all(|(g, w)| g.abs_diff(w) <= 12)
    );

    let webp = page
        .call("Page.captureScreenshot", json!({ "format": "webp" }))
        .await;
    assert_eq!(error_of(&webp).0, -32000);
    let unknown = page
        .call("Page.captureScreenshot", json!({ "format": "gif" }))
        .await;
    assert_eq!(error_of(&unknown).0, -32602);
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_clip_scales_the_raster_and_beyond_the_viewport_is_honoured() {
    let fixture = Fixture::start(&[("/tall", TALL)]).await;
    let mut page = fixture.page().await;
    page.begin().await;
    let loader = page
        .result("Page.navigate", json!({ "url": fixture.site.url("/tall") }))
        .await["loaderId"]
        .as_str()
        .unwrap()
        .to_owned();
    page.events_through(&loader, "load").await;

    let scaled = decode_png(
        &page
            .screenshot(
                json!({ "clip": { "x": 10, "y": 20, "width": 100, "height": 50, "scale": 2 } }),
            )
            .await,
    );
    assert_eq!((scaled.width, scaled.height), (200, 100));

    let whole = json!({ "x": 0, "y": 0, "width": 1280, "height": 2000, "scale": 1 });
    let cut = decode_png(&page.screenshot(json!({ "clip": whole })).await);
    assert_eq!((cut.width, cut.height), (1280, 800), "cut to the viewport");

    let beyond = decode_png(
        &page
            .screenshot(json!({ "clip": whole, "captureBeyondViewport": true }))
            .await,
    );
    assert_eq!((beyond.width, beyond.height), (1280, 2000));
    assert_eq!(beyond.pixel(640, 1900), [255, 0, 0, 255]);

    let outside = page
        .call(
            "Page.captureScreenshot",
            json!({ "clip": { "x": 0, "y": 900, "width": 10, "height": 10, "scale": 1 } }),
        )
        .await;
    assert_eq!(error_of(&outside).0, -32000);
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn layout_metrics_report_the_viewport_and_the_content() {
    let fixture = Fixture::start(&[("/tall", TALL)]).await;
    let mut page = fixture.page().await;
    page.begin().await;
    let loader = page
        .result("Page.navigate", json!({ "url": fixture.site.url("/tall") }))
        .await["loaderId"]
        .as_str()
        .unwrap()
        .to_owned();
    page.events_through(&loader, "load").await;

    let metrics = page.result("Page.getLayoutMetrics", json!({})).await;
    assert_eq!(metrics["layoutViewport"]["clientWidth"], 1280);
    assert_eq!(metrics["layoutViewport"]["clientHeight"], 800);
    assert_eq!(metrics["cssContentSize"]["width"], 1280.0);
    assert_eq!(metrics["cssContentSize"]["height"], 2000.0);
    assert_eq!(metrics["contentSize"], metrics["cssContentSize"]);
    fixture.stop().await;
}

// ---- scripts for new documents -----------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_document_script_runs_before_the_pages_own() {
    let page_script = "<!doctype html><html><body>\
        <script>document.title = 'seen:' + window.injected;</script></body></html>";
    let fixture = Fixture::start(&[("/", page_script)]).await;
    let mut page = fixture.page().await;
    page.begin().await;
    let added = page
        .result(
            "Page.addScriptToEvaluateOnNewDocument",
            json!({ "source": "window.injected = 42;" }),
        )
        .await["identifier"]
        .as_str()
        .unwrap()
        .to_owned();

    let loader = page
        .result("Page.navigate", json!({ "url": fixture.site.url("/") }))
        .await["loaderId"]
        .as_str()
        .unwrap()
        .to_owned();
    page.events_through(&loader, "load").await;
    assert_eq!(page.evaluate("document.title").await, "seen:42");

    page.result(
        "Page.removeScriptToEvaluateOnNewDocument",
        json!({ "identifier": added }),
    )
    .await;
    let again = page
        .call(
            "Page.removeScriptToEvaluateOnNewDocument",
            json!({ "identifier": added }),
        )
        .await;
    assert_eq!(error_of(&again).0, -32000);
    page.result("Page.reload", json!({})).await;
    let loader = page.event("Page.frameNavigated").await["frame"]["loaderId"]
        .as_str()
        .unwrap()
        .to_owned();
    page.events_through(&loader, "load").await;
    assert_eq!(page.evaluate("document.title").await, "seen:undefined");
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unsupported_requests_are_refused_by_name() {
    let fixture = Fixture::start(&[]).await;
    let mut page = fixture.page().await;
    for (method, params) in [(
        "Page.setDocumentContent",
        json!({ "frameId": page.target, "html": "" }),
    )] {
        let response = page.call(method, params).await;
        assert_eq!(error_of(&response).0, -32000, "{method}");
    }
    let dialog = page
        .call("Page.handleJavaScriptDialog", json!({ "accept": true }))
        .await;
    assert_eq!(error_of(&dialog).0, -32601);
    page.result("Page.setBypassCSP", json!({ "enabled": true }))
        .await;
    page.result("Page.bringToFront", json!({})).await;
    fixture.stop().await;
}

const BLOCKED: &str = "<!doctype html><html><body><p>waiting</p>\
    <script src=\"/slow\"></script></body></html>";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fresh_page_has_a_blank_frame_without_enabling_anything() {
    let fixture = Fixture::start(&[]).await;
    let mut page = fixture.page().await;
    let frame = page.result("Page.getFrameTree", json!({})).await["frameTree"]["frame"].clone();
    assert_eq!(frame["id"], page.target.as_str());
    assert_eq!(frame["url"], "about:blank");
    assert_eq!(frame["mimeType"], "text/html");
    assert_eq!(frame["securityOrigin"], "://");
    assert!(frame["loaderId"].as_str().is_some_and(|id| id.len() == 32));
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lifecycle_events_asked_for_before_enable_arrive_with_it() {
    let fixture = Fixture::start(&[]).await;
    let mut page = fixture.page().await;
    page.result("Page.setLifecycleEventsEnabled", json!({ "enabled": true }))
        .await;
    page.result("Page.enable", json!({})).await;
    let loader =
        page.result("Page.getFrameTree", json!({})).await["frameTree"]["frame"]["loaderId"]
            .as_str()
            .unwrap()
            .to_owned();
    assert_eq!(
        page.events_through(&loader, "networkIdle").await,
        [
            "lifecycle:init",
            "lifecycle:DOMContentLoaded",
            "lifecycle:load",
            "lifecycle:networkAlmostIdle",
            "lifecycle:networkIdle",
        ]
    );
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_that_joins_mid_load_is_told_what_happened_and_hears_the_rest() {
    let fixture = Fixture::start(&[("/blocked", BLOCKED)]).await;
    let mut first = fixture.page().await;
    let loader = first.navigate(&fixture.site.url("/blocked")).await;

    let mut second = fixture.attach(&first.target).await;
    second.begin().await;
    fixture.site.gate.notify_one();
    assert_eq!(
        second.events_through(&loader, "networkIdle").await,
        [
            "lifecycle:init",
            "lifecycle:commit",
            "lifecycle:DOMContentLoaded",
            "Page.domContentEventFired",
            "lifecycle:load",
            "Page.loadEventFired",
            "Page.frameStoppedLoading",
            "lifecycle:networkAlmostIdle",
            "lifecycle:networkIdle",
        ]
    );
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_load_that_never_finishes_stops_loading_without_a_load_event() {
    let fixture = Fixture::start_with(&[], |options| {
        options.navigation_timeout = Duration::from_millis(300);
    })
    .await;
    let mut page = fixture.page().await;
    page.begin().await;
    page.navigate(&fixture.site.url("/stall")).await;
    let seen = page.events_until("Page.frameStoppedLoading").await;
    assert!(
        !seen.iter().any(|name| name == "Page.loadEventFired"),
        "{seen:?}"
    );
    fixture.site.gate.notify_one();
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_navigation_replaced_before_it_commits_is_aborted_and_the_new_one_answers() {
    let fixture = Fixture::start(&[("/", INDEX), ("/slow", INDEX)]).await;
    let mut page = fixture.page().await;
    page.begin().await;
    let session = page.session.clone();
    let slow = page
        .client
        .send(
            Some(&session),
            "Page.navigate",
            json!({ "url": fixture.site.url("/slow") }),
        )
        .await;
    page.event("Page.frameStartedLoading").await;
    let loader = page.navigate(&fixture.site.url("/")).await;
    let replaced = page.client.response(slow).await;
    assert_eq!(
        replaced["result"]["errorText"], "net::ERR_ABORTED",
        "{replaced}"
    );
    let frame = page.result("Page.getFrameTree", json!({})).await["frameTree"]["frame"].clone();
    assert_eq!(frame["loaderId"], loader.as_str());
    fixture.site.gate.notify_one();
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_clip_partly_outside_the_viewport_is_cut_to_it() {
    let fixture = Fixture::start(&[("/tall", TALL)]).await;
    let mut page = fixture.page().await;
    page.begin().await;
    let loader = page.navigate(&fixture.site.url("/tall")).await;
    page.events_through(&loader, "load").await;
    let cut = decode_png(
        &page
            .screenshot(
                json!({ "clip": { "x": 1200, "y": 0, "width": 200, "height": 100, "scale": 1 } }),
            )
            .await,
    );
    assert_eq!((cut.width, cut.height), (80, 100));
    fixture.stop().await;
}

// ---- isolated worlds and init scripts ------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_isolated_world_has_its_own_context_that_follows_the_document() {
    let fixture = Fixture::start(&[("/", INDEX), ("/two", "<p>two</p>")]).await;
    let mut page = fixture.page().await;
    page.begin().await;
    page.result("Runtime.enable", json!({})).await;
    page.event("Runtime.executionContextCreated").await;

    let loader = page.navigate(&fixture.site.url("/")).await;
    page.events_through(&loader, "load").await;
    let name = "__puppeteer_utility_world__";
    let id = page
        .result(
            "Page.createIsolatedWorld",
            json!({ "frameId": page.target, "worldName": name }),
        )
        .await["executionContextId"]
        .as_u64()
        .expect("a context id");
    let created = loop {
        let event = page.event("Runtime.executionContextCreated").await;
        if event["context"]["id"] == id {
            break event["context"].clone();
        }
    };
    assert_eq!(created["name"], name);
    assert_eq!(created["auxData"]["type"], "isolated");
    assert_eq!(created["auxData"]["isDefault"], false);
    assert_eq!(created["auxData"]["frameId"], page.target.as_str());

    let sum = page
        .result(
            "Runtime.evaluate",
            json!({ "expression": "1 + 1", "contextId": id, "returnByValue": true }),
        )
        .await;
    assert_eq!(sum["result"]["value"], 2);

    // The document goes, and so does the context; the world comes back.
    page.navigate(&fixture.site.url("/two")).await;
    loop {
        let gone = page.event("Runtime.executionContextDestroyed").await;
        if gone["executionContextId"] == id {
            break;
        }
    }
    let again = loop {
        let event = page.event("Runtime.executionContextCreated").await;
        if event["context"]["name"] == name {
            break event["context"].clone();
        }
    };
    assert_ne!(again["id"], created["id"]);
    let stale = page
        .call(
            "Runtime.evaluate",
            json!({ "expression": "1", "contextId": id }),
        )
        .await;
    assert_eq!(error_of(&stale).0, -32000);
    let wrong = page
        .call(
            "Page.createIsolatedWorld",
            json!({ "frameId": "other", "worldName": name }),
        )
        .await;
    assert_eq!(error_of(&wrong).0, -32000);
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_named_init_script_announces_its_world_in_every_document() {
    let fixture = Fixture::start(&[("/", INDEX)]).await;
    let mut page = fixture.page().await;
    page.begin().await;
    page.result("Runtime.enable", json!({})).await;
    page.event("Runtime.executionContextCreated").await;
    let name = "__playwright_utility_world__";
    page.result(
        "Page.addScriptToEvaluateOnNewDocument",
        json!({ "source": "window.fromInit = 1;", "worldName": name }),
    )
    .await;

    page.navigate(&fixture.site.url("/")).await;
    let world = loop {
        let event = page.event("Runtime.executionContextCreated").await;
        if event["context"]["name"] == name {
            break event["context"].clone();
        }
    };
    assert_eq!(world["auxData"]["type"], "isolated");
    assert_eq!(page.evaluate("window.fromInit").await, 1);
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn init_scripts_belong_to_the_session_that_added_them() {
    let fixture = Fixture::start(&[("/", "<p>plain</p>")]).await;
    let mut owner = fixture.page().await;
    let mut other = fixture.attach(&owner.target).await;
    owner
        .result(
            "Page.addScriptToEvaluateOnNewDocument",
            json!({ "source": "window.injected = 42;" }),
        )
        .await;
    other.navigate(&fixture.site.url("/")).await;
    assert_eq!(other.evaluate("window.injected").await, 42);

    let session = owner.session.clone();
    owner
        .client
        .result(
            None,
            "Target.detachFromTarget",
            json!({ "sessionId": session }),
        )
        .await;
    // The session's state is released on the server's runtime. Each round is a
    // whole navigation, so this waits on work, not on a clock.
    let mut rounds = 0;
    loop {
        other.navigate(&fixture.site.url("/")).await;
        if other.evaluate("typeof window.injected").await == "undefined" {
            break;
        }
        rounds += 1;
        assert!(rounds < 200, "the script outlived its session");
    }
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_immediately_runs_in_the_current_document_too() {
    let fixture = Fixture::start(&[]).await;
    let mut page = fixture.page().await;
    page.result(
        "Page.addScriptToEvaluateOnNewDocument",
        json!({ "source": "window.now = 7;", "runImmediately": true }),
    )
    .await;
    assert_eq!(page.evaluate("window.now").await, 7);
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_init_script_hears_the_load_events_of_a_document_without_scripts() {
    let fixture = Fixture::start(&[("/", "<!doctype html><p>no script here</p>")]).await;
    let mut page = fixture.page().await;
    page.begin().await;
    page.result(
        "Page.addScriptToEvaluateOnNewDocument",
        json!({ "source": "document.addEventListener('DOMContentLoaded', () => { window.dcl = 1; });\
            window.addEventListener('load', () => { window.ld = 1; });" }),
    )
    .await;
    let loader = page.navigate(&fixture.site.url("/")).await;
    page.events_through(&loader, "load").await;
    assert_eq!(
        page.evaluate("[window.dcl, window.ld]").await,
        json!([1, 1])
    );
    fixture.stop().await;
}
