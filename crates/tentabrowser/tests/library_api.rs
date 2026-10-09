//! The public API of `docs/spec/11-library-api.md` §1, driven exactly as a
//! host would drive it: nothing below is imported from an inner crate.

use tentabrowser::{
    Budgets, ContextOptions, Engine, Error, FsPolicy, Goto, PageEvent, PageOptions, Settle, Tier,
};

use tenta_testutil::server::{FixtureServer, FixtureServerConfig};

async fn server() -> FixtureServer {
    FixtureServer::start(FixtureServerConfig::default())
        .await
        .expect("fixture server")
}

/// A context that may reach loopback, which is what a fixture test needs
/// (`docs/spec/19-security.md` §2).
fn context(engine: &Engine) -> tentabrowser::BrowserContext {
    engine
        .new_context(ContextOptions {
            allow_private_network: true,
            ..ContextOptions::default()
        })
        .expect("context")
}

/// The documented one-shot fetch: build an engine, open a context, read a
/// document out of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_documented_fetch_example_runs_against_a_real_server() {
    let server = server().await;
    let engine = Engine::builder()
        .max_tier(Tier::Dom)
        .budgets(Budgets::default().page_memory_mb(256))
        .fs_policy(FsPolicy::none())
        .io_threads(1)
        .page_threads(2)
        .build()
        .expect("engine");
    assert_eq!(engine.max_tier(), Tier::Dom);

    let ctx = context(&engine);
    let doc = ctx
        .fetch(&server.url("/static-article/index.html"))
        .await
        .expect("document");

    assert_eq!(doc.status(), 200);
    assert!(doc.ok());
    assert_eq!(doc.encoding(), "UTF-8");
    assert!(
        doc.content_type()
            .is_some_and(|value| value.starts_with("text/html"))
    );
    assert!(
        doc.final_url()
            .as_str()
            .ends_with("/static-article/index.html")
    );
    assert!(doc.timing().total > std::time::Duration::ZERO);

    let text = doc.text();
    let markdown = doc.markdown();
    let links = doc.links();
    assert!(text.len() > 500_000, "the article is {} bytes", text.len());
    assert!(markdown.contains('#'), "markdown keeps the headings");
    assert!(!links.is_empty());
    assert!(
        links.iter().all(|link| link.url.starts_with("http")),
        "links are resolved against the document URL"
    );

    let metadata = doc.metadata();
    assert!(metadata.title.is_some());
    let json = doc.to_json();
    assert_eq!(json["status"], 200);
    assert_eq!(json["encoding"], "UTF-8");

    let heading = doc
        .query_selector("h1")
        .expect("valid selector")
        .expect("the article has a heading");
    assert!(!doc.text_of(heading).is_empty());
    let main = doc
        .main_content()
        .expect("the article has a main container");
    assert!(main.text.len() <= text.len() && main.text_length > 0);

    engine.close();
}

/// The documented page example: navigation, settle condition, events, the
/// parsed document, close.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_documented_page_example_runs_against_a_real_server() {
    let server = server().await;
    let engine = Engine::builder().page_threads(1).build().expect("engine");
    let ctx = context(&engine);

    let page = ctx.new_page(PageOptions::tier1()).await.expect("page");
    assert_eq!(page.max_tier(), Tier::Dom);
    let mut events = page.events();

    let navigation = page
        .goto(
            &server.url("/tiny/index.html"),
            Goto::default().wait_until(Settle::NetworkIdle),
        )
        .await
        .expect("navigation");
    assert_eq!(navigation.status, 200);
    assert!(matches!(
        navigation.settle,
        tentabrowser::SettleState::Settled { .. }
    ));
    assert_eq!(page.tier(), Tier::Dom, "the parse took the page to Tier 1");

    let seen = events.drain();
    assert!(
        seen.iter()
            .any(|event| matches!(event, PageEvent::Request { .. })),
        "the navigation request is reported: {seen:?}"
    );
    assert!(
        seen.iter()
            .any(|event| matches!(event, PageEvent::FrameNavigated { .. })),
        "the commit is reported: {seen:?}"
    );

    let title = page
        .document()
        .and_then(|doc| doc.metadata().title)
        .expect("title");
    assert_eq!(title, "Tiny");
    page.close().await.expect("close");
    engine.close();
}

/// What the facade refuses at this release: anything above Tier 1, and
/// JavaScript on an engine that cannot reach it
/// (`docs/spec/02-architecture.md` §1).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tiers_above_dom_are_refused_through_the_public_api() {
    // Each branch describes the build it is compiled in
    // (`docs/spec/11-library-api.md` §1). Without a backend Tier 2 is absent
    // from the build; with one it is beyond the ceiling of the engine these
    // assertions are made against. QuickJS has no implementation in either.
    const _: () = assert!(!cfg!(feature = "quickjs"));

    #[cfg(not(feature = "render"))]
    {
        let error = Engine::builder()
            .max_tier(Tier::Render)
            .build()
            .expect_err("render is not compiled in");
        assert!(matches!(error, Error::Unsupported { .. }), "{error:?}");
    }

    let engine = Engine::builder().page_threads(1).build().expect("engine");
    let ctx = context(&engine);
    let error = ctx
        .new_page(PageOptions {
            javascript: true,
            ..PageOptions::tier1()
        })
        .await
        .expect_err("javascript lands with Tier 2");
    #[cfg(not(feature = "v8"))]
    assert!(matches!(error, Error::Unsupported { .. }), "{error:?}");
    #[cfg(feature = "v8")]
    assert!(
        matches!(
            error,
            Error::TierDenied {
                needed: Tier::Js,
                ceiling: Tier::Dom
            }
        ),
        "{error:?}"
    );
    engine.close();
}

/// A navigation that cannot reach the server still yields a document: the
/// `tenta-error://` page carries the `net::ERR_*` name
/// (`docs/spec/20-platform-behaviour.md` §1).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_navigation_surfaces_the_error_name() {
    // A port that was in service a moment ago: the server is gone, so the OS
    // refuses the connection.
    let server = server().await;
    let port = server.port();
    server.shutdown();

    let engine = Engine::builder().page_threads(1).build().expect("engine");
    let ctx = context(&engine);
    let page = ctx.new_page(PageOptions::tier1()).await.expect("page");

    let error = page
        .goto(
            &format!("http://127.0.0.1:{port}/tiny/index.html"),
            Goto::default().timeout(std::time::Duration::from_secs(5)),
        )
        .await
        .expect_err("the connection is refused");
    match &error {
        Error::Net { code, .. } => assert_eq!(code, "net::ERR_CONNECTION_REFUSED"),
        other => panic!("expected Error::Net, got {other:?}"),
    }

    // The guard borrows the page, so it is dropped before the page is used
    // again.
    {
        let document = page.document().expect("an error document is committed");
        assert!(
            document
                .url()
                .is_some_and(|url| url.as_str().starts_with("tenta-error://"))
        );
        assert!(
            document.text().contains("net::ERR_CONNECTION_REFUSED"),
            "the error page names the failure: {}",
            document.text()
        );
    }
    page.close().await.expect("close");
    engine.close();
}
