//! Name resolution (`docs/spec/03-network.md` §1).
//!
//! hickory with the system configuration, an in-process cache with TTL, and
//! the on-disk [`SessionCache`] in front of both, so a cold process resolves
//! nothing it already knew (`docs/spec/03-network.md` §4).
//!
//! `localhost` and `*.localhost` are loopback without a query, as in Chrome
//! (RFC 6761 §6.3); every other name goes to the system resolver with the
//! system's own search list, timeouts and retry policy, so the engine sees
//! exactly what a browser on the same machine sees.

use std::net::IpAddr;
use std::time::{Duration, Instant, SystemTime};

use hickory_resolver::Resolver as HickoryResolver;
use hickory_resolver::TokioResolver;
use hickory_resolver::config::{ResolverConfig, ResolverOpts};
use hickory_resolver::net::runtime::TokioRuntimeProvider;
use parking_lot::Mutex;
use tokio::sync::OnceCell;

use crate::error::{ErrorName, NetError};
use crate::policy::session_cache::SessionCache;

/// Answers, with the time they stop being usable.
#[derive(Clone, Debug)]
struct Entry {
    addresses: Vec<IpAddr>,
    expires: Instant,
}

/// `localhost` and any name under it, which Chrome answers with the
/// loopback addresses without consulting a resolver (RFC 6761 §6.3).
fn loopback_addresses(host: &str) -> Option<Vec<IpAddr>> {
    let name = host.trim_end_matches('.').as_bytes();
    let suffix = b".localhost";
    let is_localhost = name.eq_ignore_ascii_case(b"localhost")
        || (name.len() > suffix.len()
            && name[name.len() - suffix.len()..].eq_ignore_ascii_case(suffix));
    is_localhost.then(|| {
        vec![
            IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
            IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        ]
    })
}

/// Where an answer comes from once both caches have missed.
enum Backend {
    /// Nothing to ask: IP literals and cached names only.
    None,
    /// A resolver built up front from an explicit configuration.
    Ready(TokioResolver),
    /// The system configuration, read on the first name that needs it.
    /// Reading it costs tens of milliseconds (`/etc/resolv.conf`, the macOS
    /// dynamic store, the Windows adapter list), which is why it never
    /// happens while a client is being constructed.
    System(OnceCell<TokioResolver>),
}

/// A resolver with a cache in front of it.
pub struct Resolver {
    backend: Backend,
    session: SessionCache,
    cache: Mutex<hashbrown::HashMap<String, Entry>>,
    /// Upper bound on how long an answer is reused, whatever its TTL says.
    max_ttl: Duration,
}

impl std::fmt::Debug for Resolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Resolver")
            .field("cached", &self.cache.lock().len())
            .field("initialized", &self.is_initialized())
            .field("max_ttl", &self.max_ttl)
            .finish_non_exhaustive()
    }
}

impl Resolver {
    /// A resolver that reads the system configuration (`/etc/resolv.conf`,
    /// the macOS dynamic store, the Windows adapter configuration) the first
    /// time a name has to go to the network.
    ///
    /// Constructing one is free: IP literals, in-process cache hits and
    /// [`SessionCache`] hits are answered without ever touching the system
    /// configuration, so a cold client pays nothing for a resolver it may
    /// never use. A broken configuration therefore surfaces as a
    /// `ERR_NAME_NOT_RESOLVED` from [`Resolver::resolve`], not from the
    /// constructor.
    pub fn system(session: SessionCache) -> Self {
        Self {
            backend: Backend::System(OnceCell::new()),
            session,
            cache: Mutex::new(hashbrown::HashMap::new()),
            max_ttl: Duration::from_secs(60),
        }
    }

    /// A resolver with an explicit configuration; used by tests.
    pub fn with_config(
        config: ResolverConfig,
        opts: ResolverOpts,
        session: SessionCache,
    ) -> Result<Self, NetError> {
        let mut builder = HickoryResolver::builder_with_config(
            config,
            hickory_resolver::net::runtime::TokioRuntimeProvider::default(),
        );
        *builder.options_mut() = opts;
        let inner = builder.build().map_err(|err| {
            NetError::new(
                ErrorName::NameNotResolved,
                format!("building the resolver: {err}"),
            )
        })?;
        Ok(Self {
            backend: Backend::Ready(inner),
            session,
            cache: Mutex::new(hashbrown::HashMap::new()),
            max_ttl: Duration::from_secs(60),
        })
    }

    /// A resolver that only handles IP literals and cached names. Used where
    /// no system configuration exists (and by tests, which only ever talk to
    /// `127.0.0.1`).
    pub fn literals_only(session: SessionCache) -> Self {
        Self {
            backend: Backend::None,
            session,
            cache: Mutex::new(hashbrown::HashMap::new()),
            max_ttl: Duration::from_secs(60),
        }
    }

    /// Resolves `host` to at least one address.
    /// On macOS, multi-label names (containing a dot) are resolved as FQDN to
    /// match the OS resolver's behavior (search list applies only to single-label
    /// names). On other platforms, hickory's default behavior is used.
    pub async fn resolve(&self, host: &str) -> Result<Vec<IpAddr>, NetError> {
        if let Ok(ip) = host.parse::<IpAddr>() {
            return Ok(vec![ip]);
        }
        let unbracketed = host.trim_start_matches('[').trim_end_matches(']');
        if let Ok(ip) = unbracketed.parse::<IpAddr>() {
            return Ok(vec![ip]);
        }
        if let Some(loopback) = loopback_addresses(host) {
            return Ok(loopback);
        }

        // RFC 6761 §6.4: `.invalid` special use domain always fails.
        // MINOR-5: Check case-insensitively and handle trailing dot form.
        let normalized = host.trim_end_matches('.');
        // Compare on the byte slice, not a `str` index: a non-ASCII name (from
        // the public `Resolver` API — a URL host is already punycode) would
        // panic if `len - 8` fell mid-character (MINOR-4). `.invalid` is ASCII,
        // so a byte-wise case-insensitive suffix check is exact.
        let bytes = normalized.as_bytes();
        let is_invalid = normalized.eq_ignore_ascii_case("invalid")
            || (bytes.len() > 8 && bytes[bytes.len() - 8..].eq_ignore_ascii_case(b".invalid"));
        if is_invalid {
            return Err(NetError::new(
                ErrorName::NameNotResolved,
                format!("{host} is in the .invalid special use domain"),
            ));
        }

        if let Some(entry) = self.cache.lock().get(host)
            && entry.expires > Instant::now()
        {
            return Ok(entry.addresses.clone());
        }
        if let Some(addresses) = self.session.dns(host, SystemTime::now())
            && !addresses.is_empty()
        {
            self.remember(host, &addresses, self.max_ttl);
            return Ok(addresses);
        }

        let resolver = self.backend(host).await?;
        // On macOS, resolve multi-label names as FQDN to match getaddrinfo behavior
        // (search list applies only to single-label names). Append a trailing dot
        // to make hickory treat it as fully qualified and skip the search list.
        #[cfg(target_os = "macos")]
        let query_name = if host.contains('.') && !host.ends_with('.') {
            std::borrow::Cow::Owned(format!("{}.", host))
        } else {
            std::borrow::Cow::Borrowed(host)
        };
        #[cfg(not(target_os = "macos"))]
        let query_name = std::borrow::Cow::Borrowed(host);

        let lookup = resolver
            .lookup_ip(query_name.as_ref())
            .await
            .map_err(|err| {
                NetError::new(
                    ErrorName::NameNotResolved,
                    format!("resolving {host}: {err}"),
                )
            })?;
        let ttl = lookup
            .valid_until()
            .checked_duration_since(Instant::now())
            .unwrap_or_default()
            .min(self.max_ttl);
        let addresses: Vec<IpAddr> = lookup.iter().collect();
        if addresses.is_empty() {
            return Err(NetError::new(
                ErrorName::NameNotResolved,
                format!("{host} has no addresses"),
            ));
        }
        self.remember(host, &addresses, ttl);
        self.session
            .put_dns(host, addresses.clone(), ttl, SystemTime::now());
        Ok(addresses)
    }

    /// The resolver to ask, building the system one on first use.
    async fn backend(&self, host: &str) -> Result<&TokioResolver, NetError> {
        match &self.backend {
            Backend::Ready(resolver) => Ok(resolver),
            Backend::System(cell) => cell.get_or_try_init(Self::build_system).await,
            Backend::None => Err(NetError::new(
                ErrorName::NameNotResolved,
                format!("{host} is not an address and no resolver is configured"),
            )),
        }
    }

    /// Reads the system configuration on a blocking thread so the io thread
    /// keeps driving its other connections while the platform is consulted.
    async fn build_system() -> Result<TokioResolver, NetError> {
        let read = tokio::task::spawn_blocking(|| {
            hickory_resolver::system_conf::read_system_conf().map_err(|err| err.to_string())
        });
        let (config, options) = read
            .await
            .map_err(|err| {
                NetError::new(
                    ErrorName::NameNotResolved,
                    format!("reading the system resolver configuration: {err}"),
                )
            })?
            .map_err(|err| {
                NetError::new(
                    ErrorName::NameNotResolved,
                    format!("system resolver configuration: {err}"),
                )
            })?;

        let mut builder =
            HickoryResolver::builder_with_config(config, TokioRuntimeProvider::default());
        *builder.options_mut() = options;
        builder.build().map_err(|err| {
            NetError::new(
                ErrorName::NameNotResolved,
                format!("building the system resolver: {err}"),
            )
        })
    }

    /// Whether a backing resolver exists yet. A [`Resolver::system`] that has
    /// only ever seen IP literals or cached names has not read the system
    /// configuration and answers `false`.
    pub fn is_initialized(&self) -> bool {
        match &self.backend {
            Backend::None => false,
            Backend::Ready(_) => true,
            Backend::System(cell) => cell.initialized(),
        }
    }

    /// Puts an answer in the cache without asking anyone; used by
    /// `Client::preconnect` and by tests with a loopback fixture server.
    pub fn remember(&self, host: &str, addresses: &[IpAddr], ttl: Duration) {
        self.cache.lock().insert(
            host.to_owned(),
            Entry {
                addresses: addresses.to_vec(),
                expires: Instant::now() + ttl.min(self.max_ttl),
            },
        );
    }

    /// Drops every cached answer.
    pub fn clear_cache(&self) {
        self.cache.lock().clear();
    }

    /// Whether `host` currently has a usable cached answer.
    pub fn is_cached(&self, host: &str) -> bool {
        self.cache
            .lock()
            .get(host)
            .is_some_and(|entry| entry.expires > Instant::now())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolver() -> Resolver {
        Resolver::literals_only(SessionCache::ephemeral())
    }

    #[tokio::test]
    async fn ip_literals_never_reach_the_resolver() {
        let resolver = resolver();
        assert_eq!(
            resolver.resolve("127.0.0.1").await.expect("literal"),
            vec!["127.0.0.1".parse::<IpAddr>().expect("ip")]
        );
        assert_eq!(
            resolver.resolve("[::1]").await.expect("literal"),
            vec!["::1".parse::<IpAddr>().expect("ip")]
        );
    }

    #[tokio::test]
    async fn cached_answers_are_reused_and_expire() {
        let resolver = resolver();
        let addresses = vec!["10.0.0.1".parse::<IpAddr>().expect("ip")];
        resolver.remember("fixture.test", &addresses, Duration::from_secs(30));
        assert!(resolver.is_cached("fixture.test"));
        assert_eq!(
            resolver.resolve("fixture.test").await.expect("cached"),
            addresses
        );

        resolver.remember("expired.test", &addresses, Duration::ZERO);
        assert!(!resolver.is_cached("expired.test"));
        let err = resolver
            .resolve("expired.test")
            .await
            .expect_err("no answer");
        assert_eq!(err.name, ErrorName::NameNotResolved);

        resolver.clear_cache();
        assert!(!resolver.is_cached("fixture.test"));
    }

    #[tokio::test]
    async fn a_system_resolver_stays_uninitialized_until_a_name_needs_the_network() {
        let dir = tenta_testutil::temp_dir("dns-lazy");
        let key = crate::policy::session_cache::SessionKey::new("chrome_144_macos", "dns-lazy");
        let session = SessionCache::open(dir.path(), &key).expect("session cache");
        session.put_dns(
            "stored.test",
            vec!["192.0.2.9".parse().expect("ip")],
            Duration::from_secs(300),
            SystemTime::now(),
        );

        let resolver = Resolver::system(session);
        assert!(!resolver.is_initialized());
        resolver.resolve("127.0.0.1").await.expect("literal");
        resolver.resolve("stored.test").await.expect("session hit");
        resolver.resolve("stored.test").await.expect("cache hit");
        assert!(
            !resolver.is_initialized(),
            "literals and cached names must not read the system configuration"
        );
    }

    #[tokio::test]
    async fn the_session_cache_answers_before_the_network() {
        let dir = tenta_testutil::temp_dir("dns-session");
        let key = crate::policy::session_cache::SessionKey::new("chrome_144_macos", "dns-test");
        let session = SessionCache::open(dir.path(), &key).expect("session cache");
        session.put_dns(
            "stored.test",
            vec!["192.0.2.7".parse().expect("ip")],
            Duration::from_secs(300),
            SystemTime::now(),
        );

        let resolver = Resolver::literals_only(session);
        assert_eq!(
            resolver.resolve("stored.test").await.expect("stored"),
            vec!["192.0.2.7".parse::<IpAddr>().expect("ip")]
        );
        // ... and is then in the in-process cache.
        assert!(resolver.is_cached("stored.test"));
    }

    #[tokio::test]
    async fn localhost_names_are_loopback_without_a_resolver() {
        let resolver = resolver();
        let loopback = vec![
            "::1".parse::<IpAddr>().expect("ip"),
            "127.0.0.1".parse::<IpAddr>().expect("ip"),
        ];
        for name in ["localhost", "LocalHost.", "app.localhost", "a.b.LOCALHOST."] {
            assert_eq!(
                resolver.resolve(name).await.expect("loopback"),
                loopback,
                "{name}"
            );
        }
        let err = resolver
            .resolve("notlocalhost")
            .await
            .expect_err("only the localhost label and names under it are loopback");
        assert_eq!(err.name, ErrorName::NameNotResolved);
        assert!(!resolver.is_initialized());
    }
}
