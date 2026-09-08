//! `Engine::search` and `BrowserContext::search` as a host drives them
//! (`docs/spec/08-search.md` §3), against the recorded SERPs of the provider
//! survey replayed by the fixture server.

use std::path::PathBuf;

use tentabrowser::{
    ContextOptions, ContextSearch, Engine, EngineSearch, FsPolicy, Provider, ProviderId,
    SearchError, SearchRequest, SearchSetup,
};

use tenta_testutil::server::{FixtureServer, FixtureServerConfig};
use tenta_testutil::{TempDir, temp_dir};

/// An engine whose session cache is a directory of its own, inside a
/// filesystem policy that allows exactly that directory
/// (`docs/spec/19-security.md` §1).
fn engine_with_session_cache(prefix: &str) -> (Engine, TempDir) {
    let dir = temp_dir(prefix);
    let engine = Engine::builder()
        .page_threads(1)
        .fs_policy(FsPolicy::none().allow_write(dir.path()))
        .session_cache_dir(dir.path())
        .build()
        .expect("engine");
    (engine, dir)
}

fn provider_fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("providers")
        .join("fixtures")
}

async fn serps() -> FixtureServer {
    FixtureServer::start(FixtureServerConfig::new(provider_fixtures()))
        .await
        .expect("provider fixture server")
}

/// A setup that replays the recorded SERPs over loopback.
///
/// Every plan a test can reach is overridden, so no test opens a connection
/// to a real provider whatever the request asks for.
fn setup(server: &FixtureServer, providers: &[Provider]) -> SearchSetup {
    let mut setup = SearchSetup::default().providers(providers.iter().cloned());
    for plan in ["duckduckgo_html", "duckduckgo_lite", "bing", "brave"] {
        let endpoint = url::Url::parse(&server.url(&format!("/{plan}/rust-browser-engine.html")))
            .expect("endpoint");
        setup = setup.endpoint_override(ProviderId::new(plan).expect("plan id"), endpoint);
    }
    setup.context.allow_private_network = true;
    setup
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_engine_answers_a_query_from_its_own_search_context() {
    let server = serps().await;
    let (engine, session) = engine_with_session_cache("search-warm");
    let setup = setup(&server, &[Provider::DuckDuckGo]);

    let response = engine
        .search_with(
            &setup,
            SearchRequest::new("rust browser engine")
                .provider(Provider::DuckDuckGo)
                .limit(5),
        )
        .await
        .expect("the recorded SERP parses");

    assert_eq!(response.results.len(), 5);
    assert_eq!(response.results[0].rank, 1);
    assert_eq!(response.results[0].source, Provider::DuckDuckGo);
    assert!(response.results[0].url.as_str().starts_with("https://"));
    assert_eq!(response.providers.len(), 1);
    assert!(
        !response.providers[0].cache_hit,
        "the first query is a miss"
    );

    // The search context is a context of this engine: created lazily, kept
    // for its life, and persistent under `search-warm`.
    assert_eq!(
        engine.contexts(),
        1,
        "one internal context, created on demand"
    );
    let state = engine.search_state(&setup).expect("state");
    assert!(
        !state.context().is_ephemeral(),
        "a non-ephemeral engine persists the search context's session cache"
    );

    // The second identical query is served from the response cache, so it
    // costs no network time at all.
    let again = engine
        .search_with(
            &setup,
            SearchRequest::new("rust browser engine")
                .provider(Provider::DuckDuckGo)
                .limit(5),
        )
        .await
        .expect("cached");
    assert!(again.providers[0].cache_hit, "the second query is a hit");
    assert_eq!(again.results, response.results);
    assert_eq!(engine.contexts(), 1, "no second context is created");

    // Closing flushes the session cache, which is keyed by the search
    // context's persistence id (`docs/spec/03-network.md` §4).
    engine.close();
    let files: Vec<String> = std::fs::read_dir(session.path())
        .expect("session dir")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        files.iter().any(|name| name.contains("search-warm")),
        "no session file for the search context: {files:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_ephemeral_engine_keeps_its_search_context_ephemeral() {
    let server = serps().await;
    let dir = temp_dir("search-ephemeral");
    let engine = Engine::builder()
        .page_threads(1)
        .ephemeral(true)
        .fs_policy(FsPolicy::none().allow_write(dir.path()))
        .session_cache_dir(dir.path())
        .build()
        .expect("engine");

    let setup = setup(&server, &[Provider::DuckDuckGo]);
    let state = engine.search_state(&setup).expect("state");
    assert!(
        state.context().is_ephemeral(),
        "--ephemeral wins over a perfectly usable session cache directory"
    );
    engine.close();
    assert_eq!(
        std::fs::read_dir(dir.path()).expect("session dir").count(),
        0,
        "an ephemeral engine writes nothing"
    );
}

/// The default filesystem policy is empty, so a host that has not opened a
/// session-cache directory still gets a working search, only without TLS
/// resumption (`docs/spec/19-security.md` §1).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_denied_session_cache_degrades_to_an_ephemeral_search_context() {
    let server = serps().await;
    let engine = Engine::builder()
        .page_threads(1)
        .fs_policy(FsPolicy::none())
        .session_cache_dir(temp_dir("search-denied").path())
        .build()
        .expect("engine");

    let setup = setup(&server, &[Provider::DuckDuckGo]);
    let state = engine.search_state(&setup).expect("state");
    assert!(state.context().is_ephemeral());

    let response = engine
        .search_with(&setup, SearchRequest::new("rust browser engine").limit(2))
        .await
        .expect("the query still runs");
    assert_eq!(response.results.len(), 2);
    engine.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_provider_outside_the_first_allow_list_widens_it_instead_of_failing() {
    let server = serps().await;
    let engine = Engine::builder().page_threads(1).build().expect("engine");

    // The subsystem is built for DuckDuckGo only...
    let setup = setup(&server, &[Provider::DuckDuckGo]);
    engine
        .search_with(&setup, SearchRequest::new("rust browser engine"))
        .await
        .expect("duckduckgo answers");

    // ...and a Bing query is answered rather than refused as "not enabled".
    let response = engine
        .search_with(
            &setup,
            SearchRequest::new("rust browser engine")
                .provider(Provider::Bing)
                .limit(3),
        )
        .await
        .expect("the allow-list widened to cover the new provider");
    assert_eq!(response.results.len(), 3);
    assert_eq!(response.results[0].source, Provider::Bing);
    assert_eq!(
        engine.contexts(),
        1,
        "widening reuses the search context, it does not open another"
    );

    // M-9: The response cache now survives provider switches because the
    // Search is built once over all embedded providers. Querying the
    // first provider again reuses the cached response from the first query.
    let response_again = engine
        .search_with(
            &setup,
            SearchRequest::new("rust browser engine")
                .provider(Provider::DuckDuckGo)
                .limit(5),
        )
        .await
        .expect("duckduckgo from cache");
    assert!(
        response_again.providers[0].cache_hit,
        "querying ddg again after bing should hit the cache from the first ddg query (M-9 fix)"
    );

    engine.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_query_can_run_through_a_context_the_caller_owns() {
    let server = serps().await;
    let engine = Engine::builder().page_threads(1).build().expect("engine");
    let ctx = engine
        .new_context(ContextOptions {
            allow_private_network: true,
            ..ContextOptions::default()
        })
        .expect("context");

    let response = ctx
        .search_with(
            &setup(&server, &[Provider::DuckDuckGo]),
            SearchRequest::new("rust browser engine").limit(3),
        )
        .await
        .expect("the recorded SERP parses");
    assert_eq!(response.results.len(), 3);
    assert_eq!(
        engine.contexts(),
        1,
        "the caller's context is used; no internal one is created"
    );
    engine.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn google_is_unsupported_until_full_mode_lands() {
    let engine = Engine::builder().page_threads(1).build().expect("engine");
    let error = engine
        .search(SearchRequest::new("anything").provider(Provider::Google))
        .await
        .expect_err("google needs JavaScript");
    assert!(
        matches!(error, SearchError::Unsupported { .. }),
        "{error:?}"
    );
    engine.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn warming_the_search_pool_opens_the_provider_connections() {
    let server = serps().await;
    let engine = Engine::builder().page_threads(1).build().expect("engine");
    let setup = setup(&server, &[Provider::DuckDuckGo]);

    let report = engine.warm_search(&setup).await.expect("warm");
    assert_eq!(report.warmed, [Provider::DuckDuckGo], "{report:?}");
    assert!(report.is_complete(), "{report:?}");

    // A query on the warm connection still answers.
    let response = engine
        .search_with(&setup, SearchRequest::new("rust browser engine").limit(2))
        .await
        .expect("query");
    assert_eq!(response.results.len(), 2);
    engine.close();
}
