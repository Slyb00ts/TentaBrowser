//! Requests through HTTP CONNECT and SOCKS5 proxies
//! (`docs/spec/03-network.md` §1, `19-security.md` §7).

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::TestServer;
use common::proxy::ProxyServer;
use tenta_net::client::{Client, ClientConfig, NetRuntime};
use tenta_net::dns::Resolver;
use tenta_net::error::ErrorName;
use tenta_net::policy::private_network::DestinationPolicy;
use tenta_net::policy::session_cache::SessionCache;
use tenta_net::proxy::Proxy;
use tenta_net::request::Request;

fn client(runtime: &Arc<NetRuntime>, proxy: Option<Proxy>) -> Client {
    runtime
        .client(ClientConfig {
            proxy,
            destination: DestinationPolicy::permissive(),
            resolver: Some(Arc::new(Resolver::literals_only(SessionCache::ephemeral()))),
            allow_h2: false,
            request_timeout: Some(Duration::from_secs(10)),
            ..ClientConfig::default()
        })
        .expect("client")
}

#[tokio::test]
async fn a_request_reaches_the_origin_through_an_http_connect_tunnel() {
    let origin = TestServer::start().await;
    let proxy = ProxyServer::http(None).await;
    let runtime = NetRuntime::new(1).expect("io runtime");
    let client = client(
        &runtime,
        Some(Proxy::parse(&proxy.http_url()).expect("proxy")),
    );

    let response = client.get(&origin.url("/hello")).await.expect("response");
    assert_eq!(response.text().await.expect("body"), "hello world");
    assert_eq!(
        proxy.log.targets(),
        vec![format!("127.0.0.1:{}", origin.port())]
    );
    assert_eq!(origin.state.connections(), 1);
}

#[tokio::test]
async fn proxy_credentials_are_sent_and_never_appear_in_debug_output() {
    let origin = TestServer::start().await;
    let proxy = ProxyServer::http(Some("agent:s3cret")).await;
    let runtime = NetRuntime::new(1).expect("io runtime");
    let configured = Proxy::parse(&format!(
        "http://agent:s3cret@127.0.0.1:{}",
        proxy.addr.port()
    ))
    .expect("proxy");
    assert!(
        !format!("{configured:?}").contains("s3cret"),
        "credentials must never be printed"
    );
    let client = client(&runtime, Some(configured));

    let response = client.get(&origin.url("/hello")).await.expect("response");
    assert_eq!(response.text().await.expect("body"), "hello world");
    assert_eq!(proxy.log.credentials(), vec!["agent:s3cret".to_owned()]);
}

#[tokio::test]
async fn a_proxy_that_demands_credentials_maps_to_err_proxy_auth_requested() {
    let origin = TestServer::start().await;
    let proxy = ProxyServer::http(Some("agent:s3cret")).await;
    let runtime = NetRuntime::new(1).expect("io runtime");
    let client = client(
        &runtime,
        Some(Proxy::parse(&proxy.http_url()).expect("proxy")),
    );

    let err = client
        .get(&origin.url("/hello"))
        .await
        .expect_err("no credentials were offered");
    assert_eq!(err.name, ErrorName::ProxyAuthRequested);
    assert_eq!(origin.state.connections(), 0);
}

#[tokio::test]
async fn a_request_reaches_the_origin_through_socks5() {
    let origin = TestServer::start().await;
    let proxy = ProxyServer::socks5(None).await;
    let runtime = NetRuntime::new(1).expect("io runtime");
    let client = client(
        &runtime,
        Some(Proxy::parse(&proxy.socks5_url(false)).expect("proxy")),
    );

    let response = client.get(&origin.url("/hello")).await.expect("response");
    assert_eq!(response.text().await.expect("body"), "hello world");
    assert_eq!(
        proxy.log.targets(),
        vec![format!("127.0.0.1:{}", origin.port())],
        "socks5 (not socks5h) resolves the name locally"
    );
}

#[tokio::test]
async fn socks5h_sends_the_host_name_to_the_proxy() {
    let origin = TestServer::start().await;
    let proxy = ProxyServer::socks5(Some("agent:s3cret")).await;
    let runtime = NetRuntime::new(1).expect("io runtime");
    let configured = Proxy::parse(&format!(
        "socks5h://agent:s3cret@127.0.0.1:{}",
        proxy.addr.port()
    ))
    .expect("proxy");
    let client = client(&runtime, Some(configured));

    // The name is never resolved on this side: the literal-only resolver
    // could not answer it anyway.
    let response = client
        .get(&format!("http://origin.test:{}/hello", origin.port()))
        .await
        .expect("response");
    assert_eq!(response.text().await.expect("body"), "hello world");
    assert_eq!(
        proxy.log.targets(),
        vec![format!("origin.test:{}", origin.port())]
    );
    assert_eq!(proxy.log.credentials(), vec!["agent:s3cret".to_owned()]);
}

#[tokio::test]
async fn a_per_request_proxy_overrides_the_client_default() {
    let origin = TestServer::start().await;
    let default_proxy = ProxyServer::http(None).await;
    let other_proxy = ProxyServer::http(None).await;
    let runtime = NetRuntime::new(1).expect("io runtime");
    let client = client(
        &runtime,
        Some(Proxy::parse(&default_proxy.http_url()).expect("proxy")),
    );

    let request = Request::get(&origin.url("/hello"))
        .expect("request")
        .proxy(Proxy::parse(&other_proxy.http_url()).expect("proxy"));
    client
        .send(request)
        .await
        .expect("response")
        .bytes()
        .await
        .expect("body");

    assert!(default_proxy.log.targets().is_empty());
    assert_eq!(other_proxy.log.targets().len(), 1);
}

#[tokio::test]
async fn connections_through_different_proxies_are_pooled_separately() {
    let origin = TestServer::start().await;
    let first = ProxyServer::http(None).await;
    let second = ProxyServer::http(None).await;
    let runtime = NetRuntime::new(1).expect("io runtime");
    let client = client(
        &runtime,
        Some(Proxy::parse(&first.http_url()).expect("proxy")),
    );

    for proxy in [&first, &second] {
        let request = Request::get(&origin.url("/hello"))
            .expect("request")
            .proxy(Proxy::parse(&proxy.http_url()).expect("proxy"));
        client
            .send(request)
            .await
            .expect("response")
            .bytes()
            .await
            .expect("body");
    }

    assert_eq!(client.pool_stats().await.keys, 2, "one pool key per proxy");
    assert_eq!(origin.state.connections(), 2);
}
