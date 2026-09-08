//! The connection pool (`docs/spec/03-network.md` §3).
//!
//! One pool per (io runtime, client). Keyed by scheme, host, port, proxy,
//! profile and context, because two contexts must never share a connection
//! and two profiles must never share a TLS session.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Semaphore;

use crate::h1::H1Handle;
use crate::h2::H2Handle;
use crate::proxy::Proxy;
use crate::stream::Destination;

/// What makes two connections interchangeable.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PoolKey {
    pub secure: bool,
    pub host: String,
    pub port: u16,
    /// The proxy's identity, without credentials.
    pub proxy: Option<String>,
    pub profile: String,
    pub context: u64,
}

impl PoolKey {
    pub fn new(
        destination: &Destination,
        proxy: Option<&Proxy>,
        profile: &str,
        context: u64,
    ) -> Self {
        Self {
            secure: destination.secure,
            host: destination.host.clone(),
            port: destination.port,
            proxy: proxy.map(|p| p.pool_key()),
            profile: profile.to_owned(),
            context,
        }
    }
}

/// Pool limits.
#[derive(Clone, Copy, Debug)]
pub struct PoolConfig {
    /// How long an idle HTTP/1.1 connection is kept.
    pub idle_timeout: Duration,
    /// Chrome's six connections per host.
    pub max_h1_per_host: usize,
    /// Interval between HTTP/2 keep-alive pings; `None` disables them.
    pub h2_keep_alive: Option<Duration>,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            idle_timeout: Duration::from_secs(90),
            max_h1_per_host: 6,
            h2_keep_alive: Some(Duration::from_secs(45)),
        }
    }
}

struct Slot {
    idle_h1: VecDeque<(H1Handle, Instant)>,
    h2: Option<H2Handle>,
    /// Caps concurrent HTTP/1.1 requests to this host.
    slots: Arc<Semaphore>,
}

/// Idle connections, per key.
pub struct Pool {
    slots: hashbrown::HashMap<PoolKey, Slot>,
    config: PoolConfig,
}

/// What the pool holds right now; used by tests and diagnostics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PoolStats {
    pub idle_h1: usize,
    pub h2_connections: usize,
    pub keys: usize,
}

impl std::fmt::Debug for Pool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pool")
            .field("stats", &self.stats())
            .field("config", &self.config)
            .finish()
    }
}

impl Pool {
    pub fn new(config: PoolConfig) -> Self {
        Self {
            slots: hashbrown::HashMap::new(),
            config,
        }
    }

    pub fn config(&self) -> &PoolConfig {
        &self.config
    }

    fn slot(&mut self, key: &PoolKey) -> &mut Slot {
        let max = self.config.max_h1_per_host;
        self.slots.entry(key.clone()).or_insert_with(|| Slot {
            idle_h1: VecDeque::new(),
            h2: None,
            slots: Arc::new(Semaphore::new(max)),
        })
    }

    /// The per-host request slots for HTTP/1.1.
    pub fn h1_slots(&mut self, key: &PoolKey) -> Arc<Semaphore> {
        Arc::clone(&self.slot(key).slots)
    }

    /// An existing HTTP/2 connection, if there is one and its task is still alive.
    pub fn h2(&mut self, key: &PoolKey) -> Option<H2Handle> {
        self.slots.get(key).and_then(|slot| {
            slot.h2.as_ref().and_then(|handle| {
                // BLOCKING-1: Check if the connection task is still running.
                if handle.is_alive() {
                    Some(handle.clone())
                } else {
                    None
                }
            })
        })
    }

    /// Installs a new HTTP/2 connection.
    pub fn put_h2(&mut self, key: &PoolKey, handle: H2Handle) {
        self.slot(key).h2 = Some(handle);
    }

    /// Drops the HTTP/2 connection for a key (it went away).
    pub fn drop_h2(&mut self, key: &PoolKey) {
        if let Some(slot) = self.slots.get_mut(key) {
            slot.h2 = None;
        }
    }

    /// Takes a warm HTTP/1.1 connection, discarding expired and closed ones.
    pub fn take_h1(&mut self, key: &PoolKey) -> Option<H1Handle> {
        let idle_timeout = self.config.idle_timeout;
        let slot = self.slots.get_mut(key)?;
        while let Some((handle, since)) = slot.idle_h1.pop_front() {
            if since.elapsed() <= idle_timeout && handle.is_usable() {
                return Some(handle);
            }
        }
        None
    }

    /// Returns a connection after its response body completed.
    pub fn put_h1(&mut self, key: &PoolKey, handle: H1Handle) {
        if !handle.is_usable() {
            return;
        }
        let max = self.config.max_h1_per_host;
        let slot = self.slot(key);
        if slot.idle_h1.len() < max {
            slot.idle_h1.push_back((handle, Instant::now()));
        }
    }

    /// Drops connections that have been idle for longer than the timeout.
    pub fn purge_idle(&mut self) {
        let idle_timeout = self.config.idle_timeout;
        for slot in self.slots.values_mut() {
            slot.idle_h1
                .retain(|(handle, since)| since.elapsed() <= idle_timeout && handle.is_usable());
        }
        self.slots
            .retain(|_, slot| !slot.idle_h1.is_empty() || slot.h2.is_some());
    }

    pub fn stats(&self) -> PoolStats {
        PoolStats {
            idle_h1: self.slots.values().map(|slot| slot.idle_h1.len()).sum(),
            h2_connections: self.slots.values().filter(|slot| slot.h2.is_some()).count(),
            keys: self.slots.len(),
        }
    }

    /// Forgets everything; used when a client is closed.
    pub fn clear(&mut self) {
        self.slots.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(host: &str) -> PoolKey {
        PoolKey::new(
            &Destination {
                secure: true,
                host: host.to_owned(),
                port: 443,
            },
            None,
            "chrome_144_macos",
            0,
        )
    }

    #[test]
    fn keys_separate_contexts_profiles_and_proxies() {
        let base = key("example.com");
        let mut other_context = base.clone();
        other_context.context = 1;
        assert_ne!(base, other_context);

        let mut other_profile = base.clone();
        other_profile.profile = "none".into();
        assert_ne!(base, other_profile);

        let proxied = PoolKey::new(
            &Destination {
                secure: true,
                host: "example.com".into(),
                port: 443,
            },
            Some(&Proxy::parse("http://proxy.test:3128").expect("proxy")),
            "chrome_144_macos",
            0,
        );
        assert_ne!(base, proxied);
        assert_eq!(proxied.proxy.as_deref(), Some("http://proxy.test:3128"));
    }

    #[test]
    fn six_request_slots_per_host_and_they_are_shared_per_key() {
        let mut pool = Pool::new(PoolConfig::default());
        let slots = pool.h1_slots(&key("example.com"));
        assert_eq!(slots.available_permits(), 6);
        let again = pool.h1_slots(&key("example.com"));
        let _permit = slots.try_acquire().expect("permit");
        assert_eq!(again.available_permits(), 5);
        assert_eq!(
            pool.h1_slots(&key("other.example")).available_permits(),
            6,
            "another host has its own slots"
        );
    }

    #[test]
    fn stats_and_clearing() {
        let mut pool = Pool::new(PoolConfig::default());
        assert_eq!(pool.stats(), PoolStats::default());
        let _ = pool.h1_slots(&key("example.com"));
        assert_eq!(pool.stats().keys, 1);
        assert!(pool.take_h1(&key("example.com")).is_none());
        assert!(pool.h2(&key("example.com")).is_none());
        pool.purge_idle();
        assert_eq!(pool.stats().keys, 0, "empty slots are dropped");
        pool.clear();
        assert_eq!(pool.stats(), PoolStats::default());
    }
}
