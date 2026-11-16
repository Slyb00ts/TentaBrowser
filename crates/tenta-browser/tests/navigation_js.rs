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
