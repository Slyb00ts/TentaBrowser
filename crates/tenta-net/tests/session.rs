//! The on-disk session cache: a second process resumes TLS in one round trip
//! and resolves nothing it already knew (`docs/spec/03-network.md` §4).

mod common;

use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use common::tls::{TlsMode, TlsServer};
use tenta_net::client::{Client, ClientConfig, NetRuntime};
use tenta_net::dns::Resolver;
use tenta_net::policy::private_network::DestinationPolicy;
use tenta_net::policy::session_cache::{SessionCache, SessionKey};
use tenta_net::profile::{ClientProfile, Profile};
use tenta_net::tls::TlsOptions;

fn client(runtime: &Arc<NetRuntime>, roots: Vec<Vec<u8>>, session: SessionCache) -> Client {
    let resolver = Resolver::literals_only(session.clone());
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
                ..TlsOptions::default()
            },
            destination: DestinationPolicy::permissive(),
            resolver: Some(Arc::new(resolver)),
            session_cache: session,
            ..ClientConfig::default()
        })
        .expect("client")
}

#[tokio::test]
async fn a_second_runtime_resumes_the_tls_session_from_disk() {
    let server = TlsServer::start(TlsMode::ServeH2).await;
    let dir = tenta_testutil::temp_dir("session-resume");
    let key = SessionKey::new("chrome_144_macos", "resume");
    let roots = vec![server.cert_pem.clone()];

    // First process: a full handshake, a ticket, and a flush to disk.
    {
        let session = SessionCache::open(dir.path(), &key).expect("session cache");
        session.put_dns(
            "localhost",
            vec![IpAddr::V4(Ipv4Addr::LOCALHOST)],
            Duration::from_secs(300),
            SystemTime::now(),
        );
        let runtime = NetRuntime::new(1).expect("io runtime");
        let client = client(&runtime, roots.clone(), session.clone());
        client
            .get(&server.url("/first"))
            .await
            .expect("response")
            .bytes()
            .await
            .expect("body");

        // The TLS 1.3 ticket arrives after the handshake, on the same
        // connection, so give the io thread a moment before flushing.
        tokio::time::sleep(Duration::from_millis(100)).await;
        client.close();
        let endpoint = format!("localhost:{}", server.port());
        assert!(
            session.tls(&endpoint, SystemTime::now()).is_some(),
            "the session ticket reached the cache"
        );
        drop(client);
        drop(runtime);
        session.flush(SystemTime::now()).expect("flush");
    }

    // Second process: the same file, a new runtime, a new client.
    {
        let session = SessionCache::open(dir.path(), &key).expect("reopen session cache");
        assert_eq!(
            session.dns("localhost", SystemTime::now()),
            Some(vec![IpAddr::V4(Ipv4Addr::LOCALHOST)]),
            "the DNS answer survived the process"
        );

        let runtime = NetRuntime::new(1).expect("io runtime");
        let client = client(&runtime, roots, session.clone());
        client
            .get(&server.url("/second"))
            .await
            .expect("response")
            .bytes()
            .await
            .expect("body");
    }

    common::wait_for(|| server.capture.resumptions().len() >= 2).await;
    let resumptions = server.capture.resumptions();
    assert!(!resumptions[0], "the first handshake had nothing to resume");
    assert!(
        resumptions[1],
        "the second handshake resumed the stored session: {resumptions:?}"
    );
}

#[tokio::test]
async fn an_ephemeral_client_never_resumes() {
    let server = TlsServer::start(TlsMode::ServeH2).await;
    let roots = vec![server.cert_pem.clone()];
    let runtime = NetRuntime::new(1).expect("io runtime");

    for path in ["/one", "/two"] {
        let client = client(&runtime, roots.clone(), SessionCache::ephemeral());
        client
            .get(&server.url(path))
            .await
            .expect("response")
            .bytes()
            .await
            .expect("body");
        client.close();
    }

    common::wait_for(|| server.capture.resumptions().len() >= 2).await;
    assert!(
        server.capture.resumptions().iter().all(|resumed| !resumed),
        "an ephemeral cache keeps no ticket"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn the_stores_on_disk_are_readable_by_their_owner_only() {
    // The session file carries TLS resumption secrets and the cache log
    // carries whole response bodies, including authenticated ones; under the
    // usual `umask 022` both would be world-readable.
    use std::os::unix::fs::PermissionsExt;

    let dir = tenta_testutil::temp_dir("session-modes");
    let key = SessionKey::new("chrome_144_macos", "modes");
    let session = SessionCache::open(dir.path(), &key).expect("session cache");
    session.put_dns(
        "example.test",
        vec![IpAddr::V4(Ipv4Addr::LOCALHOST)],
        Duration::from_secs(60),
        SystemTime::now(),
    );
    session.flush(SystemTime::now()).expect("flush");

    let cache_dir = tenta_testutil::temp_dir("cache-modes");
    let cache = tenta_net::policy::cache::HttpCache::open_disk(
        tenta_net::policy::cache::CacheConfig::default(),
        cache_dir.path(),
    )
    .expect("disk cache");
    let mut headers = http::HeaderMap::new();
    headers.insert("cache-control", "max-age=60".parse().expect("value"));
    let url = url::Url::parse("https://example.test/a").expect("url");
    let request_headers = http::HeaderMap::new();
    let request = tenta_net::policy::cache::CacheRequest::new("GET", &url, &request_headers);
    cache.store(
        &request,
        200,
        &headers,
        bytes::Bytes::from_static(b"body"),
        SystemTime::now(),
    );

    let mode = |path: &std::path::Path| {
        std::fs::metadata(path)
            .unwrap_or_else(|err| panic!("metadata of {}: {err}", path.display()))
            .permissions()
            .mode()
            & 0o7777
    };
    assert_eq!(mode(dir.path()), 0o700, "session directory");
    assert_eq!(mode(cache_dir.path()), 0o700, "cache directory");
    for entry in std::fs::read_dir(dir.path())
        .expect("session dir")
        .chain(std::fs::read_dir(cache_dir.path()).expect("cache dir"))
    {
        let path = entry.expect("entry").path();
        if path.is_file() {
            assert_eq!(mode(&path), 0o600, "{}", path.display());
        }
    }
}

/// A context that ignores certificate errors never feeds the shared session
/// cache (round-4 BLOCKING-2): a ticket from an unverified handshake would
/// let a verifying client resume a session it never checked. The verifying
/// clients that follow start cold and only then resume among themselves.
#[tokio::test]
async fn an_ignore_https_errors_client_stores_no_ticket_for_verifying_clients() {
    let server = TlsServer::start(TlsMode::ServeH2).await;
    let dir = tenta_testutil::temp_dir("session-unverified");
    let key = SessionKey::new("chrome_144_macos", "unverified");
    let session = SessionCache::open(dir.path(), &key).expect("session cache");
    let runtime = NetRuntime::new(1).expect("io runtime");
    let endpoint = format!("localhost:{}", server.port());

    let ignoring = {
        let resolver = Resolver::literals_only(session.clone());
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
                    ignore_https_errors: true,
                    ..TlsOptions::default()
                },
                destination: DestinationPolicy::permissive(),
                resolver: Some(Arc::new(resolver)),
                session_cache: session.clone(),
                ..ClientConfig::default()
            })
            .expect("client")
    };
    ignoring
        .get(&server.url("/unverified"))
        .await
        .expect("response")
        .bytes()
        .await
        .expect("body");
    tokio::time::sleep(Duration::from_millis(100)).await;
    ignoring.close();
    assert!(
        session.tls(&endpoint, SystemTime::now()).is_none(),
        "an unverified handshake left a ticket in the shared cache"
    );

    for path in ["/first", "/second"] {
        let verifying = client(&runtime, vec![server.cert_pem.clone()], session.clone());
        verifying
            .get(&server.url(path))
            .await
            .expect("response")
            .bytes()
            .await
            .expect("body");
        tokio::time::sleep(Duration::from_millis(100)).await;
        verifying.close();
    }
    common::wait_for(|| server.capture.resumptions().len() >= 3).await;
    assert_eq!(
        server.capture.resumptions(),
        vec![false, false, true],
        "the first verifying client had nothing to resume; the second resumed its ticket"
    );
}
