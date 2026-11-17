//! Navigations a page starts itself, through the real page loop: a
//! `location` assignment, a form submission by POST, an activated link. The
//! script leaves a request on the document; the page takes it when the script
//! entry returns and navigates like any other navigation, with nobody
//! awaiting it.
#![cfg(feature = "v8")]

use std::time::Duration;

use tenta_browser::{
    BrowserContext, ContextOptions, Engine, Goto, JsEngine, Page, PageOptions, Settle,
};
use tenta_core::Tier;
use tenta_testutil::{FixtureServer, FixtureServerConfig};

const FIRST: &str = r#"<!doctype html><title>First</title><body>
<form id="login" action="/api/echo?from=form" method="post">
  <input name="user" value="ada"><input name="pass" type="password" value="s3cret&more">
  <button id="go" name="action" value="login">Log in</button>
</form>
<a id="link" href="/second.html">Second</a>
</body>"#;
const SECOND: &str = "<!doctype html><title>Second</title><body><p id=p>second page</p></body>";
const REDIRECT: &str = "<!doctype html><title>Redirecting</title><body>\
    <script>location.replace('/second.html?from=script');</script><p>never shown for long</p></body>";

struct World {
    /// Kept so the fixture files outlive the server that serves them.
    _fixtures: tenta_testutil::TempDir,
    server: FixtureServer,
    engine: Engine,
    context: BrowserContext,
    page: Page,
}

async fn world() -> World {
    let fixtures = tenta_testutil::temp_dir("navigation-js");
    fixtures.write("first.html", FIRST);
    fixtures.write("second.html", SECOND);
    fixtures.write("redirect.html", REDIRECT);
    let server = FixtureServer::start(FixtureServerConfig::new(fixtures.path()))
        .await
        .expect("fixture server");
    let engine = Engine::builder()
        .js(JsEngine::V8)
        .max_tier(Tier::Js)
        .io_threads(1)
        .page_threads(1)
        .ephemeral(true)
        .build()
        .expect("browser");
    let context = engine
        .new_context(ContextOptions {
            allow_private_network: true,
            ..ContextOptions::default()
        })
        .expect("context");
    let page = context
        .new_page(PageOptions::default())
        .await
        .expect("page");
    World {
        _fixtures: fixtures,
        server,
        engine,
        context,
        page,
    }
}

impl World {
    async fn open(&self, path: &str) {
        self.page
            .goto(
                &self.server.url(path),
                Goto::default()
                    .wait_until(Settle::Load)
                    .timeout(Duration::from_secs(10)),
            )
            .await
            .expect("navigation");
    }

    /// Runs `script` in the page, then waits for the navigation it started.
    async fn act(&self, script: &str) {
        self.page.evaluate(script).await.expect("the script ran");
        assert!(
            self.page.navigation_in_flight(),
            "the navigation counts as in flight as soon as the script entry returns"
        );
        tokio::time::timeout(Duration::from_secs(10), self.page.navigation_settled())
            .await
            .expect("the navigation finished");
    }

    async fn text(&self, expression: &str) -> String {
        self.page
            .evaluate(expression)
            .await
            .expect("an evaluation")
            .into_value::<String>()
            .expect("a string")
    }

    async fn close(self) {
        self.page.close().await.expect("close page");
        self.context.close();
        self.engine.close();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn assigning_location_navigates_the_page() {
    let world = world().await;
    world.open("/first.html").await;
    world.act("location.href = '/second.html?x=1'").await;
    let navigation = world.page.navigation().expect("a navigation");
    assert_eq!(
        navigation.final_url.as_str(),
        world.server.url("/second.html?x=1")
    );
    assert_eq!(navigation.status, 200);
    assert_eq!(world.page.title().as_deref(), Some("Second"));
    assert_eq!(
        world
            .text("document.getElementById('p').textContent + '|' + location.search")
            .await,
        "second page|?x=1",
        "the new document has its own realm"
    );
    world.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_script_that_redirects_while_loading_replaces_the_navigation_that_loaded_it() {
    let world = world().await;
    let outcome = world
        .page
        .goto(
            &world.server.url("/redirect.html"),
            Goto::default().timeout(Duration::from_secs(10)),
        )
        .await;
    assert!(
        outcome.is_err(),
        "the navigation the host asked for was superseded: {outcome:?}"
    );
    tokio::time::timeout(Duration::from_secs(10), world.page.navigation_settled())
        .await
        .expect("the redirect finished");
    assert_eq!(world.page.title().as_deref(), Some("Second"));
    assert_eq!(
        world
            .page
            .navigation()
            .expect("a navigation")
            .final_url
            .as_str(),
        world.server.url("/second.html?from=script")
    );
    world.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_form_is_submitted_by_post_with_its_current_values() {
    let world = world().await;
    world.open("/first.html").await;
    world
        .act(
            "document.querySelector('[name=user]').value = 'grace hopper'; \
             document.getElementById('go').click()",
        )
        .await;
    let echoed: serde_json::Value =
        serde_json::from_str(&world.text("document.body.textContent").await).expect("the echo");
    assert_eq!(echoed["method"], "POST");
    assert_eq!(echoed["query"], "from=form");
    assert_eq!(
        echoed["body"],
        "user=grace+hopper&pass=s3cret%26more&action=login"
    );
    assert_eq!(
        echoed["headers"]["content-type"],
        "application/x-www-form-urlencoded"
    );
    // `Origin` and `Referer` are written by the fingerprint profile, which
    // this engine is built without; `tentabrowser-cli`'s MCP test covers them.
    world.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_clicked_link_is_followed() {
    let world = world().await;
    world.open("/first.html").await;
    world.act("document.getElementById('link').click()").await;
    assert_eq!(world.page.title().as_deref(), Some("Second"));
    world.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_can_navigate_by_post() {
    let world = world().await;
    let navigation = world
        .page
        .goto(
            &world.server.url("/api/echo"),
            Goto::default()
                .timeout(Duration::from_secs(10))
                .post("text/plain", b"hello from the host".to_vec()),
        )
        .await
        .expect("a POST navigation");
    assert_eq!(navigation.status, 200);
    let echoed: serde_json::Value =
        serde_json::from_str(&world.text("document.body.textContent").await).expect("the echo");
    assert_eq!(echoed["method"], "POST");
    assert_eq!(echoed["body"], "hello from the host");
    assert_eq!(echoed["headers"]["content-type"], "text/plain");
    world.close().await;
}

/// A tiny origin whose routes wait for each other, so the order the test is
/// about is a fact of the exchange and not of the clock:
///
///  - `/old.html` is a page that waits for `/gate`, then assigns `location`
///    and asks for `/after`;
///  - `/gate` is answered once `/new.html` has been asked for;
///  - `/new.html` is answered once `/after` has been asked for.
///
/// So the old page asks to navigate exactly while the host's navigation to
/// `/new.html` is waiting for its response.
async fn racing_origin() -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::Notify;

    const PATIENCE: Duration = Duration::from_secs(30);
    const OLD: &str = "<!doctype html><title>Old</title><body><script>\
        fetch('/gate').then(() => { location.href = '/stolen.html'; return fetch('/after'); });\
        </script></body>";

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("an address"));
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = Arc::clone(&seen);
    let new_asked = Arc::new(Notify::new());
    let after_asked = Arc::new(Notify::new());
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let log = Arc::clone(&log);
            let new_asked = Arc::clone(&new_asked);
            let after_asked = Arc::clone(&after_asked);
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut chunk = [0_u8; 2048];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut chunk).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => head.extend_from_slice(&chunk[..n]),
                    }
                }
                let text = String::from_utf8_lossy(&head).to_string();
                let path = text.split_whitespace().nth(1).unwrap_or("/").to_owned();
                log.lock().expect("the log").push(path.clone());
                // A permit is stored when the notification comes first, so the
                // order the two connections are served in does not matter.
                let (kind, body) = match path.as_str() {
                    "/old.html" => ("text/html", OLD.to_owned()),
                    "/gate" => {
                        let _ = tokio::time::timeout(PATIENCE, new_asked.notified()).await;
                        ("text/plain", "open".to_owned())
                    }
                    "/after" => {
                        after_asked.notify_one();
                        ("text/plain", "seen".to_owned())
                    }
                    "/new.html" => {
                        new_asked.notify_one();
                        let _ = tokio::time::timeout(PATIENCE, after_asked.notified()).await;
                        (
                            "text/html",
                            "<!doctype html><title>New</title><body>new</body>".to_owned(),
                        )
                    }
                    _ => (
                        "text/html",
                        "<!doctype html><title>Stolen</title><body>stolen</body>".to_owned(),
                    ),
                };
                let _ = stream
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: {kind}\r\ncontent-length: {}\r\n\
                             connection: close\r\n\r\n{body}",
                            body.len()
                        )
                        .as_bytes(),
                    )
                    .await;
            });
        }
    });
    (base, seen)
}

#[tokio::test(flavor = "multi_thread")]
async fn the_page_being_replaced_cannot_take_the_hosts_navigation_away() {
    let world = world().await;
    let (base, seen) = racing_origin().await;
    world
        .page
        .goto(
            &format!("{base}/old.html"),
            Goto::default()
                .wait_until(Settle::Load)
                .timeout(Duration::from_secs(30)),
        )
        .await
        .expect("the old page");
    let navigation = world
        .page
        .goto(
            &format!("{base}/new.html"),
            Goto::default()
                .wait_until(Settle::Load)
                .timeout(Duration::from_secs(60)),
        )
        .await
        .expect("the navigation the host asked for is the one that happens");
    assert_eq!(navigation.final_url.as_str(), format!("{base}/new.html"));
    assert_eq!(world.page.title().as_deref(), Some("New"));
    assert!(
        !world.page.navigation_in_flight(),
        "nothing the old page asked for is still on its way"
    );
    let seen = seen.lock().expect("the log").clone();
    assert!(
        seen.iter().any(|path| path == "/after"),
        "the old page ran past its `location` assignment: {seen:?}"
    );
    assert!(
        !seen.iter().any(|path| path == "/stolen.html"),
        "the old page's navigation was not followed: {seen:?}"
    );
    world.close().await;
}
