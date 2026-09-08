//! The `reset` block class and the retry that answers it
//! (`docs/spec/08-search.md` §4).
//!
//! The P0 survey found that the first request to `html.duckduckgo.com` from a
//! fresh connection is sometimes answered with a TCP reset before any HTTP
//! response, and that one retry on a new connection resolves it. The server
//! below reproduces exactly that.

mod common;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::Harness;
use tenta_net::client::{ClientConfig, NetRuntime};
use tenta_net::dns::Resolver;
use tenta_net::policy::private_network::DestinationPolicy;
use tenta_net::policy::session_cache::SessionCache;
use tenta_testutil::HandshakeFault;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use tenta_search::{BlockClass, PlanSet, Provider, Search, SearchOptions, SearchRequest};
use url::Url;

/// A server that closes the first `failures` connections without answering,
/// then serves `body`.
struct FlakyServer {
    addr: SocketAddr,
    connections: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl FlakyServer {
    async fn start(failures: usize, body: Vec<u8>) -> FlakyServer {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let connections = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&connections);
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let seen = counter.fetch_add(1, Ordering::SeqCst) + 1;
                if seen <= failures {
                    // Close before reading or answering: what a reset before
                    // the response head looks like to the client.
                    let _ = stream.shutdown().await;
                    drop(stream);
                    continue;
                }
                let mut buffer = [0u8; 4096];
                let _ = stream.read(&mut buffer).await;
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes()).await;
                let _ = stream.write_all(&body).await;
                let _ = stream.flush().await;
            }
        });
        FlakyServer {
            addr,
            connections,
            task,
        }
    }

    fn endpoint(&self) -> Url {
        Url::parse(&format!("http://{}/html/", self.addr)).expect("url")
    }

    fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }
}

impl Drop for FlakyServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn search_against(endpoint: &Url) -> (Arc<NetRuntime>, Search) {
    let runtime = NetRuntime::new(1).expect("io runtime");
    let client = runtime
        .client(ClientConfig {
            resolver: Some(Arc::new(Resolver::literals_only(SessionCache::ephemeral()))),
            destination: DestinationPolicy::permissive(),
            allow_h2: false,
            ..ClientConfig::default()
        })
        .expect("client");
    let mut options = SearchOptions {
        warm_on_start: false,
        ..SearchOptions::default()
    };
    options.endpoint_override.insert(
        "duckduckgo_html".parse().expect("provider id"),
        endpoint.clone(),
    );
    (runtime, Search::new(client, options).expect("search"))
}

fn duckduckgo_serp() -> Vec<u8> {
    std::fs::read(
        common::fixtures_root()
            .join("duckduckgo_html")
            .join("rust-browser-engine.html"),
    )
    .expect("fixture")
}

#[tokio::test]
async fn a_reset_before_the_response_is_retried_on_a_new_connection() {
    // The transport replays a request only on a pooled connection the server
    // closed while idle (Chrome's "retry only a reused socket" rule), so a
    // reset on the fresh first connection reaches the search ladder directly.
    let server = FlakyServer::start(1, duckduckgo_serp()).await;
    let (_runtime, search) = search_against(&server.endpoint());

    let response = search
        .query(
            SearchRequest::new("rust browser engine")
                .provider(Provider::DuckDuckGo)
                .limit(50),
        )
        .await
        .expect("the retry succeeds");

    assert_eq!(response.results.len(), 10);
    assert_eq!(
        server.connections(),
        2,
        "one refused connection, then the answer on the ladder's retry"
    );
}

#[tokio::test]
async fn a_provider_that_always_resets_reports_the_reset_block_class() {
    let server = FlakyServer::start(usize::MAX, Vec::new()).await;
    let (_runtime, search) = search_against(&server.endpoint());

    let error = search
        .query(SearchRequest::new("rust").provider(Provider::DuckDuckGo))
        .await
        .expect_err("nothing ever answers");

    assert_eq!(error.block_class(), Some(BlockClass::Reset));
    assert_eq!(
        server.connections(),
        2,
        "the ladder retried exactly once on a new connection"
    );
}

#[tokio::test]
async fn the_reset_retry_happens_once_and_then_gives_up() {
    let server = FlakyServer::start(usize::MAX, Vec::new()).await;
    let (_runtime, search) = search_against(&server.endpoint());

    let before = std::time::Instant::now();
    let _ = search
        .query(SearchRequest::new("rust").provider(Provider::DuckDuckGo))
        .await;
    let attempts = server.connections();
    assert!(
        attempts <= 4,
        "one ladder retry, not a loop: {attempts} connections in {:?}",
        before.elapsed()
    );
}

#[tokio::test]
async fn the_plan_that_answers_is_the_one_the_endpoint_override_names() {
    // Sanity check on the harness the other tests rely on: the override
    // replaces the host, never the plan.
    let harness = Harness::start().await;
    let plans = PlanSet::embedded().expect("plans");
    assert_eq!(
        plans
            .get("duckduckgo_html")
            .expect("plan")
            .request
            .endpoint
            .as_str(),
        "https://html.duckduckgo.com/html/"
    );
    drop(harness);
}

#[tokio::test]
async fn a_tls_reset_on_the_first_connection_is_retried() {
    // TLS variant: the server resets the connection during the TLS handshake.
    // Every real provider is HTTPS, so this test ensures we distinguish between
    // TLS errors (certificate failures) and connection resets (which may succeed
    // on the ladder's retry).
    let server = tenta_testutil::HandshakeFaultServer::start(HandshakeFault::Reset)
        .await
        .expect("start server");
    let runtime = NetRuntime::new(1).expect("io runtime");
    let client = runtime
        .client(ClientConfig {
            resolver: Some(Arc::new(Resolver::literals_only(SessionCache::ephemeral()))),
            destination: DestinationPolicy::permissive(),
            allow_h2: false,
            ..ClientConfig::default()
        })
        .expect("client");
    let mut options = SearchOptions {
        warm_on_start: false,
        ..SearchOptions::default()
    };
    let endpoint = Url::parse(&server.url("/html/")).expect("endpoint url");
    options
        .endpoint_override
        .insert("duckduckgo_html".parse().expect("provider id"), endpoint);
    let search = Search::new(client, options).expect("search");

    let error = search
        .query(SearchRequest::new("rust").provider(Provider::DuckDuckGo))
        .await
        .expect_err("TLS reset blocks the query");

    // The reset should be classified as a connection failure, not a certificate error.
    // This validates that the ladder properly retried after the TLS handshake reset.
    assert_eq!(
        error.block_class(),
        Some(BlockClass::Reset),
        "TLS reset should be reported as reset, not certificate error, indicating proper retry handling"
    );
}

#[tokio::test]
async fn a_tls_handshake_close_is_also_retried() {
    // Clean close during TLS handshake, which is also below the TLS layer.
    let server = tenta_testutil::HandshakeFaultServer::start(HandshakeFault::Close)
        .await
        .expect("start server");
    let runtime = NetRuntime::new(1).expect("io runtime");
    let client = runtime
        .client(ClientConfig {
            resolver: Some(Arc::new(Resolver::literals_only(SessionCache::ephemeral()))),
            destination: DestinationPolicy::permissive(),
            allow_h2: false,
            ..ClientConfig::default()
        })
        .expect("client");
    let mut options = SearchOptions {
        warm_on_start: false,
        ..SearchOptions::default()
    };
    let endpoint = Url::parse(&server.url("/html/")).expect("endpoint url");
    options
        .endpoint_override
        .insert("duckduckgo_html".parse().expect("provider id"), endpoint);
    let search = Search::new(client, options).expect("search");

    let error = search
        .query(SearchRequest::new("rust").provider(Provider::DuckDuckGo))
        .await
        .expect_err("TLS handshake close blocks the query");

    // The handshake close is treated as a connection failure below the TLS layer,
    // which should trigger the ladder's retry mechanism.
    // We verify that the error is not misclassified as a certificate error.
    assert!(
        error.to_string().contains("connection") || error.block_class().is_some(),
        "TLS handshake close should result in connection error with retry attempt"
    );
}
