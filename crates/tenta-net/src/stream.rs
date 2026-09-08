//! The socket: TCP, optionally through a proxy, optionally wrapped in TLS.
//!
//! Every connection the engine makes is opened here, which is what lets the
//! proxy and private-network policies apply uniformly
//! (`docs/spec/03-network.md` §1, `19-security.md` §2).

use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;

use crate::dns::Resolver;
use crate::error::{ErrorName, NetError};
use crate::policy::private_network::{DestinationPolicy, RebindGuard};
use crate::proxy::{Proxy, ProxyKind, http_connect, socks5_connect};
use crate::tls::{TlsConnector, TlsStream, tls_info};

/// A connected socket.
#[derive(Debug)]
pub enum NetStream {
    Plain(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
}

impl NetStream {
    pub fn peer_addr(&self) -> Option<SocketAddr> {
        match self {
            NetStream::Plain(stream) => stream.peer_addr().ok(),
            NetStream::Tls(stream) => stream.get_ref().peer_addr().ok(),
        }
    }

    /// The ALPN protocol the handshake selected, if this is a TLS stream.
    pub fn alpn(&self) -> Option<String> {
        match self {
            NetStream::Plain(_) => None,
            NetStream::Tls(stream) => tls_info(stream).alpn,
        }
    }

    pub fn is_tls(&self) -> bool {
        matches!(self, NetStream::Tls(_))
    }
}

impl AsyncRead for NetStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            NetStream::Plain(stream) => Pin::new(stream).poll_read(cx, buf),
            NetStream::Tls(stream) => Pin::new(stream.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for NetStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            NetStream::Plain(stream) => Pin::new(stream).poll_write(cx, buf),
            NetStream::Tls(stream) => Pin::new(stream.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            NetStream::Plain(stream) => Pin::new(stream).poll_flush(cx),
            NetStream::Tls(stream) => Pin::new(stream.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            NetStream::Plain(stream) => Pin::new(stream).poll_shutdown(cx),
            NetStream::Tls(stream) => Pin::new(stream.as_mut()).poll_shutdown(cx),
        }
    }
}

/// Where a connection goes.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Destination {
    pub secure: bool,
    pub host: String,
    pub port: u16,
}

impl Destination {
    /// Parses the destination out of a URL.
    pub fn from_url(url: &url::Url) -> Result<Self, NetError> {
        let secure = url.scheme() == "https";
        let host = url
            .host_str()
            .ok_or_else(|| NetError::new(ErrorName::InvalidArgument, "URL has no host"))?
            .to_owned();
        let port = url
            .port_or_known_default()
            .unwrap_or(if secure { 443 } else { 80 });
        Ok(Self { secure, host, port })
    }
}

/// How long each phase of opening a connection took.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConnectTiming {
    pub dns: Duration,
    pub connect: Duration,
    pub tls: Duration,
}

/// A freshly opened connection.
#[derive(Debug)]
pub struct OpenedConnection {
    pub stream: NetStream,
    pub remote_addr: Option<SocketAddr>,
    pub timing: ConnectTiming,
    /// The ALPN protocol, or `None` on plaintext connections.
    pub alpn: Option<String>,
}

/// The destination rules of one client, applied to every connection and
/// every redirect hop (`docs/spec/19-security.md` §2).
#[derive(Clone, Copy)]
pub struct Destinations<'a> {
    pub policy: &'a DestinationPolicy,
    pub rebind: &'a RebindGuard,
}

impl Destinations<'_> {
    /// Scheme, port and host name; callable before DNS.
    pub fn check_url(&self, url: &url::Url) -> Result<(), NetError> {
        self.policy.check_url(url).map_err(NetError::from_policy)
    }

    /// The URL together with what it resolved to, plus the rebinding check
    /// (a name that used to be public and now points at a private address).
    pub fn check_resolved(&self, url: &url::Url, resolved: &[IpAddr]) -> Result<(), NetError> {
        self.policy
            .check(url, resolved)
            .map_err(NetError::from_policy)?;
        if let Some(host) = url.host_str() {
            self.rebind
                .check(host, resolved, self.policy)
                .map_err(NetError::from_policy)?;
            self.rebind.record(host, resolved);
        }
        Ok(())
    }
}

/// Opens one connection: resolve, policy check, TCP, proxy, TLS.
pub async fn open(
    destination: &Destination,
    proxy: Option<&Proxy>,
    resolver: &Resolver,
    connector: &TlsConnector,
    destinations: Destinations<'_>,
    url: &url::Url,
    connect_timeout: Duration,
) -> Result<OpenedConnection, NetError> {
    let mut timing = ConnectTiming::default();

    // What we open a socket to: the proxy, or the destination itself.
    let (tcp_host, tcp_port) = match proxy {
        Some(proxy) => proxy.address(),
        None => (destination.host.clone(), destination.port),
    };

    let dns_started = Instant::now();
    let addresses: Vec<IpAddr> = if proxy.is_some() {
        // Only the proxy is reached directly; the destination is the proxy's
        // business and was checked by name before this call.
        resolver.resolve(&tcp_host).await?
    } else {
        let addresses = resolver.resolve(&tcp_host).await?;
        // The policy sees the addresses, not just the name.
        destinations.check_resolved(url, &addresses)?;
        addresses
    };
    timing.dns = dns_started.elapsed();

    let connect_started = Instant::now();
    let stream = connect_any(&addresses, tcp_port, connect_timeout).await?;
    timing.connect = connect_started.elapsed();
    let remote_addr = stream.peer_addr().ok();
    let mut stream = stream;

    if let Some(proxy) = proxy {
        match proxy.kind {
            ProxyKind::Http => {
                // MINOR-8: Pass the profile's User-Agent to CONNECT tunnel.
                http_connect(
                    &mut stream,
                    &destination.host,
                    destination.port,
                    proxy.auth.as_ref(),
                    connector.user_agent(),
                )
                .await?
            }
            ProxyKind::Socks5 { remote_dns } => {
                let target = if remote_dns {
                    destination.host.clone()
                } else {
                    let addresses = resolver.resolve(&destination.host).await?;
                    destinations.check_resolved(url, &addresses)?;
                    addresses
                        .first()
                        .expect("resolve returns at least one address")
                        .to_string()
                };
                socks5_connect(&mut stream, &target, destination.port, proxy.auth.as_ref()).await?
            }
        }
    }

    if !destination.secure {
        return Ok(OpenedConnection {
            stream: NetStream::Plain(stream),
            remote_addr,
            timing,
            alpn: None,
        });
    }

    let tls_started = Instant::now();
    let tls = connector
        .connect(&destination.host, destination.port, stream)
        .await?;
    timing.tls = tls_started.elapsed();
    let alpn = tls_info(&tls).alpn;
    Ok(OpenedConnection {
        stream: NetStream::Tls(Box::new(tls)),
        remote_addr,
        timing,
        alpn,
    })
}

/// How long one attempt gets before the next family is tried as well.
///
/// RFC 8305 §5 recommends 250 ms; Chrome's `TransportConnectJob` waits 300 ms
/// before falling back from IPv6 (`kIPv6FallbackTime`), and a host that
/// measures the delay sees the browser's value, not the RFC's minimum.
const ATTEMPT_STAGGER: Duration = Duration::from_millis(300);

/// Connects to the first address that answers, like Chrome's connect job.
async fn connect_any(
    addresses: &[IpAddr],
    port: u16,
    timeout: Duration,
) -> Result<TcpStream, NetError> {
    let stream = race_addresses(addresses, port, timeout, |socket| {
        TcpStream::connect(socket)
    })
    .await?;
    // Interactive traffic: never wait for a full segment.
    let _ = stream.set_nodelay(true);
    Ok(stream)
}

/// Happy Eyeballs (RFC 8305): the families are interleaved, an attempt is
/// started every [`ATTEMPT_STAGGER`] (or immediately when the previous one
/// fails), attempts stay in flight together and the first to connect wins.
/// `timeout` is a wall-clock cap on the whole operation, not on each address,
/// so a long address list cannot multiply the caller's budget.
async fn race_addresses<C, F, S>(
    addresses: &[IpAddr],
    port: u16,
    timeout: Duration,
    connect: C,
) -> Result<S, NetError>
where
    C: Fn(SocketAddr) -> F,
    F: std::future::Future<Output = std::io::Result<S>> + Send + 'static,
    S: Send + 'static,
{
    let ordered = interleave_families(addresses);
    if ordered.is_empty() {
        return Err(NetError::new(
            ErrorName::AddressUnreachable,
            "no address to connect to".to_owned(),
        ));
    }

    let deadline = tokio::time::Instant::now() + timeout;
    // Dropping the set aborts every attempt that is still in flight, which is
    // what closes the losing sockets once one has won.
    let mut attempts = tokio::task::JoinSet::new();
    let mut queue = ordered.iter().copied().peekable();
    let mut next_launch = tokio::time::Instant::now();
    // The first failure observed, which on a list the resolver ordered is the
    // preferred address's, and is the one worth reporting.
    let mut failure: Option<NetError> = None;

    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            // Every address that failed outright is accounted for below, so
            // reaching the deadline means an attempt was still in flight.
            return Err(NetError::new(
                ErrorName::ConnectionTimedOut,
                format!("connecting to {} timed out", describe(&ordered, port)),
            ));
        }
        if queue.peek().is_some() && now >= next_launch {
            let socket = SocketAddr::new(queue.next().expect("peeked"), port);
            attempts.spawn(connect(socket));
            next_launch = now + ATTEMPT_STAGGER;
            continue;
        }

        let wake_at = if queue.peek().is_some() {
            next_launch.min(deadline)
        } else {
            deadline
        };
        if attempts.is_empty() {
            tokio::time::sleep_until(wake_at).await;
            continue;
        }
        match tokio::time::timeout_at(wake_at, attempts.join_next()).await {
            // The next attempt is due, or the whole budget is spent.
            Err(_) => continue,
            Ok(None) => continue,
            Ok(Some(Ok(Ok(stream)))) => return Ok(stream),
            Ok(Some(Ok(Err(err)))) => {
                failure = failure.or_else(|| Some(NetError::from_connect_io(&err)));
                // A refused or unreachable address does not hold the next one
                // back for the rest of its stagger.
                next_launch = tokio::time::Instant::now();
            }
            Ok(Some(Err(err))) => {
                failure = failure
                    .or_else(|| Some(NetError::new(ErrorName::ConnectionFailed, err.to_string())));
            }
        }
        if attempts.is_empty() && queue.peek().is_none() {
            return Err(failure.unwrap_or_else(|| {
                NetError::new(
                    ErrorName::AddressUnreachable,
                    "no address to connect to".to_owned(),
                )
            }));
        }
    }
}

/// RFC 8305 §4: alternate address families, starting with the family of the
/// first address the resolver returned, and keep the resolver's order within
/// each family.
fn interleave_families(addresses: &[IpAddr]) -> Vec<IpAddr> {
    let Some(first) = addresses.first() else {
        return Vec::new();
    };
    let (mut same, mut other): (Vec<IpAddr>, Vec<IpAddr>) = addresses
        .iter()
        .copied()
        .partition(|address| address.is_ipv6() == first.is_ipv6());
    let mut ordered = Vec::with_capacity(addresses.len());
    let mut same = same.drain(..).peekable();
    let mut other = other.drain(..).peekable();
    while same.peek().is_some() || other.peek().is_some() {
        if let Some(address) = same.next() {
            ordered.push(address);
        }
        if let Some(address) = other.next() {
            ordered.push(address);
        }
    }
    ordered
}

fn describe(addresses: &[IpAddr], port: u16) -> String {
    addresses
        .iter()
        .map(|address| SocketAddr::new(*address, port).to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use url::Url;

    #[test]
    fn destinations_take_the_default_port_from_the_scheme() {
        let https = Destination::from_url(&Url::parse("https://example.com/x").expect("url"))
            .expect("destination");
        assert_eq!(
            https,
            Destination {
                secure: true,
                host: "example.com".into(),
                port: 443
            }
        );
        let http = Destination::from_url(&Url::parse("http://example.com:8080/").expect("url"))
            .expect("destination");
        assert!(!http.secure);
        assert_eq!(http.port, 8080);
    }

    #[tokio::test]
    async fn a_refused_port_maps_to_err_connection_refused() {
        // Bind and drop, so the port is almost certainly free.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("addr").port();
        drop(listener);

        let err = connect_any(
            &["127.0.0.1".parse().expect("ip")],
            port,
            Duration::from_secs(5),
        )
        .await
        .expect_err("must fail");
        assert_eq!(err.name, ErrorName::ConnectionRefused);
    }

    #[tokio::test]
    async fn connect_falls_through_to_the_next_address() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("addr").port();

        // 2001:db8::1 is the documentation prefix and never answers, whether
        // the host drops the SYN or has no route for it at all; the second
        // address wins either way, after the stagger at the latest.
        let started = std::time::Instant::now();
        let stream = connect_any(
            &[
                "2001:db8::1".parse().expect("ip"),
                "127.0.0.1".parse().expect("ip"),
            ],
            port,
            Duration::from_secs(10),
        )
        .await
        .expect("second address answers");
        assert_eq!(stream.peer_addr().expect("addr").port(), port);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the dead address must not cost the whole connect budget: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn the_address_families_are_interleaved_starting_with_the_first() {
        let v6a: IpAddr = "2001:db8::1".parse().expect("ip");
        let v6b: IpAddr = "2001:db8::2".parse().expect("ip");
        let v6c: IpAddr = "2001:db8::3".parse().expect("ip");
        let v4a: IpAddr = "192.0.2.1".parse().expect("ip");
        let v4b: IpAddr = "192.0.2.2".parse().expect("ip");

        assert_eq!(
            interleave_families(&[v6a, v6b, v6c, v4a]),
            vec![v6a, v4a, v6b, v6c],
            "one address of the other family is tried second, the rest keep their order"
        );
        assert_eq!(
            interleave_families(&[v4a, v6a, v6b, v4b]),
            vec![v4a, v6a, v4b, v6b],
            "the resolver's first address decides which family goes first"
        );
        assert_eq!(interleave_families(&[v4a, v4b]), vec![v4a, v4b]);
        assert!(interleave_families(&[]).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn the_second_family_starts_after_the_stagger_not_after_a_timeout() {
        use std::sync::Arc;
        use std::sync::Mutex;

        // The first address never answers and never fails, which is what a
        // dropped SYN looks like. The second must not wait for the connect
        // timeout, only for the stagger.
        let started = tokio::time::Instant::now();
        let launches: Arc<Mutex<Vec<(SocketAddr, Duration)>>> = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&launches);
        let winner = SocketAddr::new("127.0.0.1".parse().expect("ip"), 80);
        let connected = race_addresses(
            &[
                "2001:db8::1".parse().expect("ip"),
                "127.0.0.1".parse().expect("ip"),
            ],
            80,
            Duration::from_secs(30),
            move |socket| {
                recorder
                    .lock()
                    .expect("recorder")
                    .push((socket, started.elapsed()));
                async move {
                    if socket.is_ipv6() {
                        std::future::pending::<()>().await;
                    }
                    Ok(socket)
                }
            },
        )
        .await
        .expect("the reachable address wins");

        assert_eq!(connected, winner);
        let launches = launches.lock().expect("recorder").clone();
        assert_eq!(launches.len(), 2, "both addresses were attempted");
        assert_eq!(
            launches[0].1,
            Duration::ZERO,
            "the first attempt is immediate"
        );
        assert_eq!(
            launches[1].1, ATTEMPT_STAGGER,
            "the second family starts one stagger later"
        );
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "the connect timeout was never waited out: {:?}",
            started.elapsed()
        );
    }
}
