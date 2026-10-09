//! HTTP/2 behaviour over a real TLS connection: ALPN, multiplexing, request
//! bodies and pooling (`docs/spec/03-network.md` §1, §3).

mod common;

use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::Duration;

use common::tls::{TlsMode, TlsServer};
use http::StatusCode;
use tenta_net::client::{Client, ClientConfig, NetRuntime, Protocol};
use tenta_net::dns::Resolver;
use tenta_net::error::ErrorName;
use tenta_net::policy::private_network::DestinationPolicy;
use tenta_net::policy::session_cache::SessionCache;
use tenta_net::profile::{ClientProfile, Profile};
use tenta_net::request::Request;
use tenta_net::tls::TlsOptions;

fn client(runtime: &Arc<NetRuntime>, roots: Vec<Vec<u8>>, alpn: Option<Vec<String>>) -> Client {
    let resolver = Resolver::literals_only(SessionCache::ephemeral());
    resolver.remember(
        "localhost",
        &[IpAddr::V4(Ipv4Addr::LOCALHOST)],
        Duration::from_secs(60),
    );
    runtime
        .client(ClientConfig {
            profile: Profile::Browser(Arc::new(
                ClientProfile::builtin("chrome_144_macos").expect("profile"),
            )),
            tls: TlsOptions {
                extra_root_cas: roots,
                alpn_override: alpn,
                ..TlsOptions::default()
            },
            destination: DestinationPolicy::permissive(),
            resolver: Some(Arc::new(resolver)),
            ..ClientConfig::default()
        })
        .expect("client")
}

#[tokio::test]
async fn alpn_selects_h2_and_the_response_comes_back_over_it() {
    let server = TlsServer::start(TlsMode::ServeH2).await;
    let runtime = NetRuntime::new(1).expect("io runtime");
    let client = client(&runtime, vec![server.cert_pem.clone()], None);

    let response = client.get(&server.url("/page")).await.expect("response");
    assert_eq!(response.protocol, Protocol::Http2);
    assert_eq!(response.version, http::Version::HTTP_2);
    assert_eq!(response.text().await.expect("body"), "hello from h2");
}

#[tokio::test]
async fn a_hundred_requests_share_one_http2_connection() {
    let server = TlsServer::start(TlsMode::ServeH2).await;
    let runtime = NetRuntime::new(1).expect("io runtime");
    let client = client(&runtime, vec![server.cert_pem.clone()], None);

    // Warm the pool so the burst cannot open a second connection while the
    // first is still handshaking.
    client
        .get(&server.url("/warm"))
        .await
        .expect("response")
        .bytes()
        .await
        .expect("body");

    let mut tasks = Vec::new();
    for index in 0..100 {
        let client = client.clone();
        let url = server.url(&format!("/item/{index}"));
        tasks.push(tokio::spawn(async move {
            let request = Request::post(&url, format!("body-{index}")).expect("request");
            let response = client.send(request).await.expect("response");
            assert_eq!(response.protocol, Protocol::Http2);
            response.text().await.expect("body")
        }));
    }
    for (index, task) in tasks.into_iter().enumerate() {
        assert_eq!(task.await.expect("task"), format!("body-{index}"));
    }

    let stats = client.pool_stats().await;
    assert_eq!(
        stats.h2_connections, 1,
        "one HTTP/2 connection served everything"
    );
}

#[tokio::test]
async fn an_http2_request_body_is_written_with_flow_control() {
    let server = TlsServer::start(TlsMode::ServeH2).await;
    let runtime = NetRuntime::new(1).expect("io runtime");
    let client = client(&runtime, vec![server.cert_pem.clone()], None);

    // Larger than the 64 KiB default stream window, so capacity has to be
    // waited for at least once.
    let payload = "x".repeat(200 * 1024);
    let request = Request::post(&server.url("/upload"), payload.clone()).expect("request");
    let response = client.send(request).await.expect("response");
    assert_eq!(response.text().await.expect("body").len(), payload.len());
}

#[tokio::test]
async fn a_streamed_request_body_reaches_the_server_over_http2() {
    let server = TlsServer::start(TlsMode::ServeH2).await;
    let runtime = NetRuntime::new(1).expect("io runtime");
    let client = client(&runtime, vec![server.cert_pem.clone()], None);

    let (sender, body) = tenta_net::Body::channel(None);
    tokio::spawn(async move {
        for part in ["alpha", "beta", "gamma"] {
            sender
                .send(Ok(bytes::Bytes::from_static(part.as_bytes())))
                .await
                .expect("send");
        }
    });
    let request = Request::new(
        http::Method::POST,
        url::Url::parse(&server.url("/stream")).expect("url"),
    )
    .with_body(body);
    let response = client.send(request).await.expect("response");
    assert_eq!(response.text().await.expect("body"), "alphabetagamma");
}

#[tokio::test]
async fn preconnect_warms_an_http2_connection() {
    let server = TlsServer::start(TlsMode::ServeH2).await;
    let runtime = NetRuntime::new(1).expect("io runtime");
    let client = client(&runtime, vec![server.cert_pem.clone()], None);

    client
        .preconnect(&server.url("/"))
        .await
        .expect("preconnect");
    assert_eq!(client.pool_stats().await.h2_connections, 1);

    let response = client.get(&server.url("/after")).await.expect("response");
    assert_eq!(response.timing.tls, Duration::ZERO);
    assert_eq!(response.timing.connect, Duration::ZERO);
    assert_eq!(response.protocol, Protocol::Http2);
}

#[tokio::test]
async fn a_client_that_only_offers_http11_gets_http11_over_tls() {
    let server = TlsServer::start(TlsMode::ServeH2).await;
    let runtime = NetRuntime::new(1).expect("io runtime");
    let client = client(
        &runtime,
        vec![server.cert_pem.clone()],
        Some(vec!["http/1.1".to_owned()]),
    );

    // The server speaks HTTP/2 only, so the request fails; what matters is
    // that ALPN negotiated http/1.1 and the client did not send an HTTP/2
    // preface to a connection that was not negotiated for it.
    let _ = client.get(&server.url("/")).await;
    assert_eq!(
        server.capture.alpn().as_deref(),
        Some(b"http/1.1".as_slice())
    );
}

#[tokio::test]
async fn a_pooled_http2_connection_closed_without_goaway_is_evicted() {
    // BLOCKING-1: A pooled HTTP/2 connection that closes without GOAWAY must
    // be evicted. The critical fix marks the connection as not alive when its
    // task exits, and the pool checks liveness before handing out a handle.
    // Additionally, failures on reused connections are retried on a fresh connection.
    let server = TlsServer::start(TlsMode::ServeH2DropAfterOne).await;
    let runtime = NetRuntime::new(1).expect("io runtime");
    let client = client(&runtime, vec![server.cert_pem.clone()], None);

    // First request succeeds (pool is empty, opens new connection).
    let response = client
        .get(&server.url("/request-0"))
        .await
        .expect("response");
    assert_eq!(response.status, StatusCode::OK);
    let pool_stats = client.pool_stats().await;
    assert_eq!(pool_stats.h2_connections, 1, "one connection is open");
    assert_eq!(response.text().await.expect("body"), "hello from h2");
    server.capture.drop_h2_after_response();
    server.capture.wait_h2_closed(1).await;

    // Second request (GET): must retry on a fresh connection (reused is retryable).
    let response = client
        .get(&server.url("/request-1"))
        .await
        .expect("response");
    assert_eq!(
        response.status,
        StatusCode::OK,
        "second GET succeeds on new connection"
    );
    assert_eq!(response.text().await.expect("body"), "hello from h2");
    server.capture.drop_h2_after_response();
    server.capture.wait_h2_closed(2).await;

    // Pool should still report the dead handle as gone or no longer usable.
    let pool_stats = client.pool_stats().await;
    assert_eq!(
        pool_stats.h2_connections, 1,
        "connection replaced by fresh one"
    );

    // Third request (POST): must succeed on fresh connection.
    let request = Request::post(&server.url("/request-2"), "body").expect("request");
    let response = client.send(request).await.expect("response");
    assert_eq!(
        response.status,
        StatusCode::OK,
        "POST succeeds on fresh connection"
    );
    assert_eq!(response.text().await.expect("body"), "body");
    server.capture.drop_h2_after_response();
    server.capture.wait_h2_closed(3).await;
}

#[tokio::test]
async fn an_http2_request_with_userinfo_stripped() {
    // BLOCKING-4: Userinfo in URLs must be stripped. The fix strips userinfo
    // at execute entry, so Request::new(GET, url) without userinfo stripping
    // sends a clean :authority header.
    let server = TlsServer::start(TlsMode::ServeH2).await;
    let runtime = NetRuntime::new(1).expect("io runtime");
    let client = client(&runtime, vec![server.cert_pem.clone()], None);

    // Create a URL with userinfo
    let url = format!("https://user:password@localhost:{}/test", server.port());

    // Request should succeed and strip the userinfo
    let response = client.get(&url).await.expect("response");
    assert_eq!(response.status, StatusCode::OK);
    // Response URL should not contain userinfo
    assert!(
        !response.url.username().contains("password"),
        "userinfo should be stripped from URL"
    );
    assert!(
        response.url.password().is_none(),
        "password should be stripped from URL"
    );
}

/// A remote REFUSED_STREAM on a reused HTTP/2 stream guarantees no
/// processing, so a replayable POST can be sent once more, then redirected
/// with its original body preserved.
#[tokio::test]
async fn a_retried_post_keeps_its_body_through_a_redirect() {
    let server = TlsServer::start(TlsMode::ServeH2RefusePostOnce).await;
    let runtime = NetRuntime::new(1).expect("io runtime");
    let client = client(&runtime, vec![server.cert_pem.clone()], None);

    client
        .get(&server.url("/warm"))
        .await
        .expect("response")
        .bytes()
        .await
        .expect("body");

    let request = Request::post(&server.url("/temporary"), "replayed body").expect("request");
    let response = client.send(request).await.expect("retried and redirected");
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.redirects.len(), 1);
    assert_eq!(response.redirects[0].status, 307);
    assert_eq!(
        response.text().await.expect("body"),
        "replayed body",
        "the 307 target must receive the same body"
    );
    assert_eq!(
        server.capture.replay_paths(),
        ["/warm", "/temporary", "/temporary", "/echo"]
    );
    assert_eq!(
        server.capture.replay_bodies(),
        [
            ("/warm".to_owned(), Vec::new()),
            ("/temporary".to_owned(), b"replayed body".to_vec()),
            ("/echo".to_owned(), b"replayed body".to_vec()),
        ]
    );
    assert_eq!(server.capture.client_hellos().len(), 1);
}

#[tokio::test]
async fn repeated_refused_stream_is_not_retried_without_bound() {
    let server = TlsServer::start(TlsMode::ServeH2RefusePostTwice).await;
    let runtime = NetRuntime::new(1).expect("io runtime");
    let client = client(&runtime, vec![server.cert_pem.clone()], None);
    client
        .get(&server.url("/warm"))
        .await
        .expect("response")
        .bytes()
        .await
        .expect("body");

    let request = Request::post(&server.url("/temporary"), "replayed body").expect("request");
    let error = client
        .send(request)
        .await
        .expect_err("a second refusal is final");
    assert_eq!(error.name, ErrorName::Http2ProtocolError);
    assert_eq!(
        server.capture.replay_paths(),
        ["/warm", "/temporary", "/temporary"]
    );
    assert_eq!(server.capture.client_hellos().len(), 1);
}

#[tokio::test]
async fn a_non_refused_stream_reset_does_not_replay_a_post() {
    let server = TlsServer::start(TlsMode::ServeH2CancelPost).await;
    let runtime = NetRuntime::new(1).expect("io runtime");
    let client = client(&runtime, vec![server.cert_pem.clone()], None);
    client
        .get(&server.url("/warm"))
        .await
        .expect("response")
        .bytes()
        .await
        .expect("body");

    let request = Request::post(&server.url("/temporary"), "replayed body").expect("request");
    let error = client
        .send(request)
        .await
        .expect_err("CANCEL is not safe to replay");
    assert_eq!(error.name, ErrorName::Http2ProtocolError);
    assert_eq!(server.capture.replay_paths(), ["/warm", "/temporary"]);
    assert_eq!(server.capture.client_hellos().len(), 1);
}

#[tokio::test]
async fn a_post_received_before_transport_close_is_not_replayed() {
    let server = TlsServer::start(TlsMode::ServeH2CloseAfterPostBody).await;
    let runtime = NetRuntime::new(1).expect("io runtime");
    let client = client(&runtime, vec![server.cert_pem.clone()], None);
    client
        .get(&server.url("/warm"))
        .await
        .expect("response")
        .bytes()
        .await
        .expect("body");

    let request = Request::post(&server.url("/temporary"), "applied once")
        .expect("request")
        .timeout(Duration::from_secs(5));
    let error = client
        .send(request)
        .await
        .expect_err("connection closed before response head");
    assert!(
        error.retryable(),
        "fixture must exercise ambiguous transport retry: {error:?}"
    );
    assert_eq!(
        server.capture.replay_paths(),
        ["/warm", "/temporary"],
        "an ambiguous close must not send the POST again"
    );
    assert_eq!(
        server.capture.replay_bodies(),
        [
            ("/warm".to_owned(), Vec::new()),
            ("/temporary".to_owned(), b"applied once".to_vec()),
        ]
    );
}

/// A pooled HTTP/2 connection whose peer goes silent — the socket stays open
/// but no head ever comes back — must be evicted so the origin recovers, not
/// left to time out every later request for the life of the client
/// (BLOCKING-2, round 5). The request that meets the hung connection fails at
/// its own deadline; the next request opens a fresh connection and succeeds.
#[tokio::test]
async fn a_pooled_http2_connection_whose_peer_goes_silent_is_evicted() {
    let server = TlsServer::start(TlsMode::ServeH2HangAfterOne).await;
    let runtime = NetRuntime::new(1).expect("io runtime");
    let client = client(&runtime, vec![server.cert_pem.clone()], None);

    // First request succeeds and warms the pool with the connection the server
    // will then stop answering.
    let first = client
        .get(&server.url("/request-0"))
        .await
        .expect("first response");
    assert_eq!(first.status, StatusCode::OK);
    assert_eq!(client.pool_stats().await.h2_connections, 1);

    // The second request reuses the pooled connection, whose peer is now
    // silent: it must fail at its own short deadline rather than hang.
    let request = Request::get(&server.url("/request-1"))
        .expect("request")
        .timeout(Duration::from_millis(800));
    let hung = client.send(request).await;
    assert!(
        matches!(&hung, Err(err) if err.name == tenta_net::error::ErrorName::TimedOut),
        "the request on the silent connection times out: {hung:?}"
    );

    // The origin has recovered: the timeout evicted the dead handle, so the
    // next request opens a fresh connection and succeeds.
    let recovered = tokio::time::timeout(
        Duration::from_secs(5),
        client.get(&server.url("/request-2")),
    )
    .await
    .expect("the client must not stay wedged on the dead connection")
    .expect("recovered response");
    assert_eq!(recovered.status, StatusCode::OK);
    assert_eq!(recovered.text().await.expect("body"), "hello from h2");
}

/// A coalescing follower must not inherit the leader's pre-head failure: when
/// the leader takes a silently-hung pooled connection and times out (evicting
/// it), the follower runs its own request on a fresh connection and succeeds,
/// rather than failing where running alone would have worked (round-6 MAJOR).
#[tokio::test]
async fn a_coalescing_follower_does_not_inherit_a_leaders_head_timeout() {
    let server = TlsServer::start(TlsMode::ServeH2HangAfterOne).await;
    let runtime = NetRuntime::new(1).expect("io runtime");
    let client = client(&runtime, vec![server.cert_pem.clone()], None);

    // Warm the pool with a connection the server then stops answering.
    client
        .get(&server.url("/warm"))
        .await
        .expect("warm response")
        .bytes()
        .await
        .expect("body");

    // Two identical in-flight GETs. The leader takes the poisoned pooled
    // connection and times out at its short deadline; the follower, registered
    // behind it, must fall through to its own request on a fresh connection.
    let url = server.url("/same");
    let leader = {
        let client = client.clone();
        let request = Request::get(&url)
            .expect("request")
            .timeout(Duration::from_millis(800));
        tokio::spawn(async move { client.send(request).await })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    let follower = {
        let client = client.clone();
        let request = Request::get(&url)
            .expect("request")
            .timeout(Duration::from_secs(5));
        tokio::spawn(async move { client.send(request).await })
    };

    let leader_result = leader.await.expect("leader task");
    let follower_result = follower.await.expect("follower task");
    assert!(
        matches!(&leader_result, Err(err) if err.name == tenta_net::error::ErrorName::TimedOut),
        "the leader times out on the hung connection: {leader_result:?}"
    );
    let follower_response =
        follower_result.expect("the follower must run its own request and succeed");
    assert_eq!(follower_response.status, StatusCode::OK);
}
