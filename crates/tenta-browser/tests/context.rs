//! Context isolation: cookies, storage state, destinations, filesystem
//! policy, interception (`docs/spec/03-network.md` §4, §6, §9;
//! `docs/spec/19-security.md` §1, §2).

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tenta_browser::{ContextOptions, Engine};
use tenta_core::{Error, FsPolicy};
use tenta_net::Request;
use tenta_net::hooks::{BoxFuture, FulfilledResponse, Interception, Interceptor, Observer};

/// `sid=abc; Path=/`, percent-encoded for the `set` query parameter.
const SET_COOKIE_QUERY: &str = "/api/cookie?set=sid%3Dabc%3B%20Path%3D%2F";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cookies_are_kept_within_a_context_and_never_shared_between_two() {
    let server = support::server().await;
    let engine = support::engine(2);
    let first = support::context(&engine);

    let set = first
        .fetch(&server.url(SET_COOKIE_QUERY))
        .await
        .expect("first fetch");
    assert!(
        set.text().contains("\"cookie\":\"\""),
        "the first request carries no cookie: {}",
        set.text()
    );
    assert_eq!(first.cookies().len(), 1, "the jar stored the cookie");

    let again = first
        .fetch(&server.url("/api/cookie"))
        .await
        .expect("second fetch");
    assert!(
        again.text().contains("sid=abc"),
        "the second request of the same context sends it: {}",
        again.text()
    );

    let second = support::context(&engine);
    let isolated = second
        .fetch(&server.url("/api/cookie"))
        .await
        .expect("fetch in a fresh context");
    assert!(
        isolated.text().contains("\"cookie\":\"\""),
        "a second context starts with an empty jar: {}",
        isolated.text()
    );
    assert_eq!(second.cookies().len(), 0);
    engine.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn storage_state_moves_cookies_between_contexts() {
    let server = support::server().await;
    let engine = support::engine(2);
    let source = support::context(&engine);
    source
        .fetch(&server.url(SET_COOKIE_QUERY))
        .await
        .expect("fetch");

    let state = source.storage_state();
    assert_eq!(state.cookies.len(), 1);
    assert!(state.origins.is_empty(), "origin storage lands with Tier 2");
    let json = state.to_json().expect("storage state serialises");
    assert!(json.contains("sid"));

    let target = support::context(&engine);
    let loaded = target.set_storage_state(&state).expect("load");
    assert_eq!(loaded, 1);
    let document = target
        .fetch(&server.url("/api/cookie"))
        .await
        .expect("fetch");
    assert!(
        document.text().contains("sid=abc"),
        "the restored jar sends the cookie: {}",
        document.text()
    );
    engine.close();
}

/// Loopback is a private-network destination and is refused unless the
/// context opted in (`docs/spec/19-security.md` §2).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn private_network_destinations_are_refused_by_default() {
    let server = support::server().await;
    let engine = support::engine(1);
    let context = engine
        .new_context(ContextOptions::default())
        .expect("context");
    assert!(!context.destination_policy().allow_private_network);

    let error = context
        .fetch(&server.url("/tiny/index.html"))
        .await
        .expect_err("loopback is refused");
    match &error {
        // `ERR_ACCESS_DENIED` is the name the destination policy gives a
        // private-network refusal; a blocked scheme or port has its own.
        Error::Net { code, .. } => assert_eq!(code, "net::ERR_ACCESS_DENIED"),
        other => panic!("expected a blocked destination, got {other:?}"),
    }
    assert_eq!(
        server.requests_served(),
        0,
        "the refusal happens before a socket is opened"
    );

    let allowed = support::context(&engine);
    allowed
        .fetch(&server.url("/tiny/index.html"))
        .await
        .expect("a test context may reach loopback");
    engine.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cache_directory_outside_the_policy_roots_is_denied() {
    let temp = tenta_testutil::temp_dir("cache");
    let engine = support::engine(1);

    let error = engine
        .new_context(ContextOptions {
            allow_private_network: true,
            cache_dir: Some(temp.path().join("http")),
            ..ContextOptions::default()
        })
        .expect_err("the default policy allows no directory");
    match &error {
        Error::FsDenied { path } => assert!(path.ends_with("http")),
        other => panic!("expected FsDenied, got {other:?}"),
    }
    engine.close();

    let allowed = Engine::builder()
        .page_threads(1)
        .fs_policy(FsPolicy::none().allow_write(temp.path()))
        .build()
        .expect("engine");
    std::fs::create_dir_all(temp.path().join("http")).expect("cache dir");
    let context = allowed
        .new_context(ContextOptions {
            allow_private_network: true,
            cache_dir: Some(temp.path().join("http")),
            ..ContextOptions::default()
        })
        .expect("a permitted directory is accepted");
    assert_eq!(context.cache().len(), 0);
    allowed.close();
}

/// A context with a `persistence_id` keeps DNS answers and TLS tickets; an
/// ephemeral one reads and writes nothing (`docs/spec/03-network.md` §4).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_a_persistent_context_uses_the_session_cache() {
    let temp = tenta_testutil::temp_dir("session");
    let engine = Engine::builder()
        .page_threads(1)
        .session_cache_dir(temp.path())
        .fs_policy(FsPolicy::none().allow_write(temp.path()))
        .build()
        .expect("engine");

    let ephemeral = engine
        .new_context(ContextOptions::default())
        .expect("context");
    assert!(ephemeral.is_ephemeral());

    let persistent = engine
        .new_context(ContextOptions {
            persistence_id: Some("cli-default".to_owned()),
            ..ContextOptions::default()
        })
        .expect("context");
    assert!(!persistent.is_ephemeral());
    engine.close();

    let denied = Engine::builder()
        .page_threads(1)
        .session_cache_dir(temp.path())
        .build()
        .expect("engine");
    let error = denied
        .new_context(ContextOptions {
            persistence_id: Some("cli-default".to_owned()),
            ..ContextOptions::default()
        })
        .expect_err("persistence needs write access to the cache directory");
    assert!(matches!(error, Error::FsDenied { .. }), "{error:?}");
    denied.close();

    let forced = Engine::builder()
        .page_threads(1)
        .session_cache_dir(temp.path())
        .fs_policy(FsPolicy::none().allow_write(temp.path()))
        .ephemeral(true)
        .build()
        .expect("engine");
    let context = forced
        .new_context(ContextOptions {
            persistence_id: Some("cli-default".to_owned()),
            ..ContextOptions::default()
        })
        .expect("context");
    assert!(
        context.is_ephemeral(),
        "an ephemeral engine persists nothing, whatever the id"
    );
    forced.close();
}

struct StaticPage;

impl Interceptor for StaticPage {
    fn intercept<'a>(&'a self, request: &'a mut Request) -> BoxFuture<'a, Interception> {
        Box::pin(async move {
            if request.url.path().ends_with("/intercepted.html") {
                Interception::Fulfill(
                    FulfilledResponse::new(
                        http::StatusCode::OK,
                        "<html><body><p>from the interceptor</p></body></html>",
                    )
                    .header("content-type", "text/html; charset=utf-8"),
                )
            } else {
                Interception::Continue
            }
        })
    }
}

/// An interceptor that fulfils exactly one path with its own body, so two
/// contexts can be told apart by which one answered.
struct FixedPage {
    path: &'static str,
    body: &'static str,
}

impl Interceptor for FixedPage {
    fn intercept<'a>(&'a self, request: &'a mut Request) -> BoxFuture<'a, Interception> {
        Box::pin(async move {
            if request.url.path() == self.path {
                Interception::Fulfill(
                    FulfilledResponse::new(http::StatusCode::OK, self.body)
                        .header("content-type", "text/html; charset=utf-8"),
                )
            } else {
                Interception::Continue
            }
        })
    }
}

/// A second context on one engine has its own interceptor list, so context A's
/// interceptor can never answer context B's request.
///
/// The WPT driver's worker pool rests on exactly this: each worker owns a
/// context, and the context's interceptor answers the report-script path with a
/// hook naming the worker's current test. If the two contexts shared one hub,
/// both hook scripts would name whichever test the shared slot held, and the
/// other test would time out. The request count makes the negative half
/// non-vacuous: context B's crossed request reached the network rather than
/// being fulfilled by context A's interceptor.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_contexts_on_one_engine_have_separate_interceptor_lists() {
    let server = support::server().await;
    let engine = support::engine(2);
    let first = support::context(&engine);
    let second = support::context(&engine);

    first
        .intercept(Arc::new(FixedPage {
            path: "/only-first.html",
            body: "<html><body><p>from context A</p></body></html>",
        }))
        .expect("register on the first");
    second
        .intercept(Arc::new(FixedPage {
            path: "/only-second.html",
            body: "<html><body><p>from context B</p></body></html>",
        }))
        .expect("register on the second");

    assert_eq!(first.interceptor_count(), 1);
    assert_eq!(
        second.interceptor_count(),
        1,
        "the second context has its own list, not the first's"
    );

    // Each context's own interceptor answers its own path, without a socket.
    let a = first
        .fetch(&server.url("/only-first.html"))
        .await
        .expect("A");
    assert!(a.text().contains("from context A"));
    let b = second
        .fetch(&server.url("/only-second.html"))
        .await
        .expect("B");
    assert!(b.text().contains("from context B"));
    assert_eq!(
        server.requests_served(),
        0,
        "both were fulfilled by their own context's interceptor"
    );

    // The first context's interceptor does not answer the second's request: it
    // goes to the server, which has no such file.
    let crossed = second.fetch(&server.url("/only-first.html")).await;
    let body = crossed.map(|document| document.text()).unwrap_or_default();
    assert!(
        !body.contains("from context A"),
        "context A's interceptor answered context B's request: {body}"
    );
    assert_eq!(
        server.requests_served(),
        1,
        "context B's crossed request reached the network"
    );
    engine.close();
}

#[derive(Default)]
struct CountingObserver {
    requests: AtomicUsize,
}

impl Observer for CountingObserver {
    fn on_request(&self, _request: &Request) {
        self.requests.fetch_add(1, Ordering::Relaxed);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interceptors_and_observers_are_passed_through_to_the_client() {
    let server = support::server().await;
    let engine = support::engine(1);
    let context = support::context(&engine);
    let observer = Arc::new(CountingObserver::default());
    context
        .intercept(Arc::new(StaticPage))
        .expect("register an interceptor");
    context
        .observe(Arc::clone(&observer) as Arc<dyn Observer>)
        .expect("register an observer");
    assert_eq!(context.interceptor_count(), 1);
    assert_eq!(context.observer_count(), 1);

    let intercepted = context
        .fetch(&server.url("/intercepted.html"))
        .await
        .expect("document");
    assert!(intercepted.text().contains("from the interceptor"));
    assert_eq!(
        server.requests_served(),
        0,
        "a fulfilled request never reaches the network"
    );
    assert_eq!(
        observer.requests.load(Ordering::Relaxed),
        0,
        "an interceptor decides before the observers see a request"
    );

    context
        .fetch(&server.url("/tiny/index.html"))
        .await
        .expect("document");
    assert_eq!(server.requests_served(), 1);
    assert_eq!(observer.requests.load(Ordering::Relaxed), 1);
    engine.close();
}

/// A context-level `User-Agent` and `Accept-Language` reach the wire (O-3).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_user_agent_and_accept_language_overrides_reach_the_request() {
    let server = support::server().await;
    let engine = support::engine(1);
    let context = engine
        .new_context(ContextOptions {
            allow_private_network: true,
            user_agent: Some("TentaTest/1.0".to_owned()),
            accept_language: Some("pl-PL,pl;q=0.9".to_owned()),
            ..ContextOptions::default()
        })
        .expect("context");

    let document = context.fetch(&server.url("/api/echo")).await.expect("echo");
    let echoed = document.text();
    assert!(echoed.contains("TentaTest/1.0"), "echo was {echoed}");
    assert!(echoed.contains("pl-PL"), "echo was {echoed}");
    engine.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_closed_context_refuses_further_work() {
    let server = support::server().await;
    let engine = support::engine(1);
    let context = support::context(&engine);
    context
        .fetch(&server.url("/tiny/index.html"))
        .await
        .expect("document");
    context.close();

    let error = context
        .fetch(&server.url("/tiny/index.html"))
        .await
        .expect_err("a closed context takes no work");
    assert!(matches!(error, Error::Closed), "{error:?}");
    engine.close();
}
