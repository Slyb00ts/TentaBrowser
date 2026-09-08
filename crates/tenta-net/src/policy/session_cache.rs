//! Per-user session cache (`docs/spec/03-network.md` §4).
//!
//! A cold CLI process should reach a known host in one RTT instead of three,
//! so DNS answers and opaque TLS session blobs survive across processes.
//! What must *not* survive is anything that lets a server link two contexts:
//! the cache is keyed by (profile, context persistence id), ephemeral
//! contexts never touch it, and it stores no cookies and no bodies
//! (`docs/spec/19-security.md` §7).
//!
//! The directory is chosen by the caller, which is expected to have run it
//! through `FsPolicy` first; [`SessionCache::default_dir`] only computes the
//! per-platform default path and touches nothing.

use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hashbrown::HashMap;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tenta_core::{Error, Result};

/// Which on-disk file a context uses.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionKey {
    /// Fingerprint profile name; a different profile must not reuse another
    /// profile's TLS tickets, or the two are linkable.
    pub profile: String,
    /// `BrowserContext` persistence id (`cli-default` for the CLI).
    pub persistence_id: String,
}

impl SessionKey {
    pub fn new(profile: impl Into<String>, persistence_id: impl Into<String>) -> Self {
        Self {
            profile: profile.into(),
            persistence_id: persistence_id.into(),
        }
    }

    /// A file name that is safe on all five platforms and still unique: the
    /// readable part is sanitised, the hash keeps distinct keys apart.
    pub fn file_name(&self) -> String {
        let mut name = String::with_capacity(48);
        for part in [self.profile.as_str(), self.persistence_id.as_str()] {
            for ch in part.chars().take(32) {
                if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                    name.push(ch.to_ascii_lowercase());
                } else {
                    name.push('_');
                }
            }
            name.push('-');
        }
        let hash = fnv1a(self.profile.as_bytes()) ^ fnv1a(self.persistence_id.as_bytes());
        name.push_str(&format!("{hash:016x}.jsonl"));
        name
    }
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t")]
enum Record {
    #[serde(rename = "dns")]
    Dns {
        host: String,
        addrs: Vec<IpAddr>,
        exp: u64,
    },
    #[serde(rename = "tls")]
    Tls {
        endpoint: String,
        /// Lowercase hex; the blob is opaque to us and never logged.
        blob: String,
        exp: u64,
    },
}

#[derive(Debug, Default)]
struct State {
    dns: HashMap<String, (Vec<IpAddr>, SystemTime)>,
    tls: HashMap<String, (Vec<u8>, SystemTime)>,
    dirty: bool,
    /// Incremented on every modification; used to detect if dirty flag should be cleared.
    generation: u64,
    /// Running estimate of total bytes used by DNS and TLS entries.
    byte_estimate: usize,
}

#[derive(Debug)]
struct Inner {
    path: PathBuf,
    /// Held for the lifetime of the cache so two processes cannot interleave
    /// rewrites of the same file.
    _lock: File,
    state: RwLock<State>,
}

/// DNS answers and TLS session blobs that outlive the process.
///
/// Cloning shares the same file and state.
#[derive(Debug, Clone)]
pub struct SessionCache {
    inner: Option<Arc<Inner>>,
}

impl SessionCache {
    /// A cache that stores nothing: what an ephemeral context gets.
    pub fn ephemeral() -> Self {
        Self { inner: None }
    }

    pub fn is_ephemeral(&self) -> bool {
        self.inner.is_none()
    }

    /// Whether [`SessionCache::flush`] writes to the filesystem.
    pub fn writes_block(&self) -> bool {
        self.inner.is_some()
    }

    /// The per-platform default directory. Does not create it and does not
    /// check `FsPolicy`; both are the caller's job.
    pub fn default_dir() -> Option<PathBuf> {
        #[cfg(target_os = "windows")]
        {
            let base = std::env::var_os("LOCALAPPDATA")?;
            Some(PathBuf::from(base).join("tentabrowser").join("session"))
        }
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        {
            let home = std::env::var_os("HOME")?;
            Some(
                PathBuf::from(home)
                    .join("Library")
                    .join("Caches")
                    .join("tentabrowser")
                    .join("session"),
            )
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "ios")))]
        {
            let base = match std::env::var_os("XDG_CACHE_HOME") {
                Some(dir) if !dir.is_empty() => PathBuf::from(dir),
                _ => PathBuf::from(std::env::var_os("HOME")?).join(".cache"),
            };
            Some(base.join("tentabrowser").join("session"))
        }
    }

    /// Opens (creating if needed) the file for `key` under `dir`.
    ///
    /// Fails with [`io::ErrorKind::WouldBlock`] when another process holds
    /// the same file; callers that would rather degrade than fail should use
    /// [`SessionCache::open_or_ephemeral`].
    ///
    /// The `<key>.lock` file next to the cache is the lock itself and stays
    /// on disk after the process exits: removing it would let a later process
    /// create a fresh inode and lock that instead, which is no lock at all.
    /// It is empty, `0600`, and re-used by every later run.
    pub fn open(dir: &Path, key: &SessionKey) -> Result<Self> {
        crate::policy::private_files::create_private_dir(dir)?;
        let path = dir.join(key.file_name());

        let lock_path = path.with_extension("lock");
        let lock = crate::policy::private_files::private_file()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)?;
        crate::policy::private_files::harden_file(&lock_path)?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(fs::TryLockError::WouldBlock) => {
                return Err(Error::Io(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    format!(
                        "session cache {} is in use by another process",
                        path.display()
                    ),
                )));
            }
            Err(fs::TryLockError::Error(error)) => return Err(Error::Io(error)),
        }

        // Clean up leftover temp files from crashed writers. Multiple processes
        // may have created temp files with different PIDs.
        if let Ok(dir_entries) = fs::read_dir(dir) {
            let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            for entry in dir_entries.flatten() {
                if let Ok(name) = entry.file_name().into_string()
                    && name.starts_with(file_name)
                    && name.contains(".tmp-")
                {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }

        let state = load(&path)?;
        Ok(Self {
            inner: Some(Arc::new(Inner {
                path,
                _lock: lock,
                state: RwLock::new(state),
            })),
        })
    }

    /// Like [`SessionCache::open`] but degrades to [`SessionCache::ephemeral`]
    /// instead of failing, which is what a second CLI process should do.
    pub fn open_or_ephemeral(dir: &Path, key: &SessionKey) -> Self {
        match Self::open(dir, key) {
            Ok(cache) => cache,
            Err(error) => {
                tracing::debug!(%error, "session cache unavailable, running ephemeral");
                Self::ephemeral()
            }
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.inner.as_ref().map(|inner| inner.path.as_path())
    }

    /// Cached addresses for `host`, or `None` when absent or expired.
    pub fn dns(&self, host: &str, now: SystemTime) -> Option<Vec<IpAddr>> {
        let inner = self.inner.as_ref()?;
        let state = inner.state.read();
        let (addrs, expires) = state.dns.get(&host.to_ascii_lowercase())?;
        if *expires <= now {
            return None;
        }
        Some(addrs.clone())
    }

    /// Records a DNS answer with the TTL the resolver reported.
    pub fn put_dns(&self, host: &str, addrs: Vec<IpAddr>, ttl: Duration, now: SystemTime) {
        let Some(inner) = self.inner.as_ref() else {
            return;
        };
        if addrs.is_empty() || ttl.is_zero() {
            return;
        }
        const MAX_SESSION_MEMORY: usize = 10 * 1024 * 1024; // 10 MiB

        let mut state = inner.state.write();
        let host_lower = host.to_ascii_lowercase();
        let estimated_size = estimate_dns_size(&host_lower, &addrs);

        // Remove old entry if it exists to subtract its size
        if let Some((old_addrs, _)) = state.dns.remove(&host_lower) {
            state.byte_estimate = state
                .byte_estimate
                .saturating_sub(estimate_dns_size(&host_lower, &old_addrs));
        }

        state.dns.insert(host_lower, (addrs, now + ttl));
        state.byte_estimate = state.byte_estimate.saturating_add(estimated_size);

        // Evict oldest-TLS-first if over limit
        if state.byte_estimate > MAX_SESSION_MEMORY {
            evict_oldest_tls_first(&mut state, MAX_SESSION_MEMORY);
        }

        state.dirty = true;
        state.generation = state.generation.wrapping_add(1);
    }

    /// The opaque TLS session blob for `host:port`.
    pub fn tls(&self, endpoint: &str, now: SystemTime) -> Option<Vec<u8>> {
        let inner = self.inner.as_ref()?;
        let state = inner.state.read();
        let (blob, expires) = state.tls.get(&endpoint.to_ascii_lowercase())?;
        if *expires <= now {
            return None;
        }
        Some(blob.clone())
    }

    /// Records a TLS session blob. `ttl` should be the ticket lifetime.
    pub fn put_tls(&self, endpoint: &str, blob: Vec<u8>, ttl: Duration, now: SystemTime) {
        let Some(inner) = self.inner.as_ref() else {
            return;
        };
        if blob.is_empty() || ttl.is_zero() {
            return;
        }
        const MAX_SESSION_MEMORY: usize = 10 * 1024 * 1024; // 10 MiB

        let mut state = inner.state.write();
        let endpoint_lower = endpoint.to_ascii_lowercase();
        let estimated_size = estimate_tls_size(&endpoint_lower, &blob);

        // Remove old entry if it exists to subtract its size
        if let Some((old_blob, _)) = state.tls.remove(&endpoint_lower) {
            state.byte_estimate = state
                .byte_estimate
                .saturating_sub(estimate_tls_size(&endpoint_lower, &old_blob));
        }

        state.tls.insert(endpoint_lower, (blob, now + ttl));
        state.byte_estimate = state.byte_estimate.saturating_add(estimated_size);

        // Evict oldest-TLS-first if over limit
        if state.byte_estimate > MAX_SESSION_MEMORY {
            evict_oldest_tls_first(&mut state, MAX_SESSION_MEMORY);
        }

        state.dirty = true;
        state.generation = state.generation.wrapping_add(1);
    }

    pub fn remove_dns(&self, host: &str) -> bool {
        let Some(inner) = self.inner.as_ref() else {
            return false;
        };
        let mut state = inner.state.write();
        let host_lower = host.to_ascii_lowercase();
        if let Some((addrs, _)) = state.dns.remove(&host_lower) {
            state.byte_estimate = state
                .byte_estimate
                .saturating_sub(estimate_dns_size(&host_lower, &addrs));
            state.dirty = true;
            state.generation = state.generation.wrapping_add(1);
            true
        } else {
            false
        }
    }

    pub fn remove_tls(&self, endpoint: &str) -> bool {
        let Some(inner) = self.inner.as_ref() else {
            return false;
        };
        let mut state = inner.state.write();
        let endpoint_lower = endpoint.to_ascii_lowercase();
        if let Some((blob, _)) = state.tls.remove(&endpoint_lower) {
            state.byte_estimate = state
                .byte_estimate
                .saturating_sub(estimate_tls_size(&endpoint_lower, &blob));
            state.dirty = true;
            state.generation = state.generation.wrapping_add(1);
            true
        } else {
            false
        }
    }

    pub fn clear(&self) {
        self.mutate(|state| {
            state.dns.clear();
            state.tls.clear();
            state.byte_estimate = 0;
        });
    }

    /// Drops expired entries. Returns how many were removed.
    pub fn purge_expired(&self, now: SystemTime) -> usize {
        let Some(inner) = self.inner.as_ref() else {
            return 0;
        };
        let mut state = inner.state.write();
        let before = state.dns.len() + state.tls.len();

        // Remove expired DNS entries and update byte_estimate
        let mut expired_dns = Vec::new();
        for (host, (_, expires)) in &state.dns {
            if *expires <= now {
                expired_dns.push(host.clone());
            }
        }
        for host in expired_dns {
            if let Some((addrs, _)) = state.dns.remove(&host) {
                state.byte_estimate = state
                    .byte_estimate
                    .saturating_sub(estimate_dns_size(&host, &addrs));
            }
        }

        // Remove expired TLS entries and update byte_estimate
        let mut expired_tls = Vec::new();
        for (endpoint, (_, expires)) in &state.tls {
            if *expires <= now {
                expired_tls.push(endpoint.clone());
            }
        }
        for endpoint in expired_tls {
            if let Some((blob, _)) = state.tls.remove(&endpoint) {
                state.byte_estimate = state
                    .byte_estimate
                    .saturating_sub(estimate_tls_size(&endpoint, &blob));
            }
        }

        let removed = before - (state.dns.len() + state.tls.len());
        if removed > 0 {
            state.dirty = true;
            state.generation = state.generation.wrapping_add(1);
        }
        removed
    }

    /// Number of live entries of both kinds.
    pub fn len(&self) -> usize {
        match self.inner.as_ref() {
            Some(inner) => {
                let state = inner.state.read();
                state.dns.len() + state.tls.len()
            }
            None => 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Writes the cache to disk, dropping expired entries first.
    ///
    /// A cache that has not changed since it was loaded or last flushed is
    /// not rewritten: the write ends in `sync_all`, which costs 10–25 ms on
    /// APFS and would otherwise land on every context close, including the
    /// cold CLI path that `03` §4 exists to make fast. Expired entries are
    /// then dropped at the next real write or at load.
    ///
    /// The write goes to a temp file that is then renamed over the target, so
    /// a crash leaves either the previous file or the new one, never a
    /// half-written one.
    pub fn flush(&self, now: SystemTime) -> Result<()> {
        let Some(inner) = self.inner.as_ref() else {
            return Ok(());
        };

        // Capture the current generation before taking the snapshot.
        let generation_at_start = {
            let state = inner.state.read();
            if !state.dirty {
                return Ok(());
            }
            state.generation
        };

        // Build the records snapshot and release the lock immediately.
        // The lock is NOT held during hex encoding, JSON serialization, or file write.
        let (dns_records, tls_blobs, path) = {
            let mut state = inner.state.write();
            // Remove expired entries from state and update byte_estimate
            let mut expired_dns = Vec::new();
            for (host, (_, expires)) in &state.dns {
                if *expires <= now {
                    expired_dns.push(host.clone());
                }
            }
            for host in expired_dns {
                if let Some((addrs, _)) = state.dns.remove(&host) {
                    state.byte_estimate = state
                        .byte_estimate
                        .saturating_sub(estimate_dns_size(&host, &addrs));
                }
            }

            let mut expired_tls = Vec::new();
            for (endpoint, (_, expires)) in &state.tls {
                if *expires <= now {
                    expired_tls.push(endpoint.clone());
                }
            }
            for endpoint in expired_tls {
                if let Some((blob, _)) = state.tls.remove(&endpoint) {
                    state.byte_estimate = state
                        .byte_estimate
                        .saturating_sub(estimate_tls_size(&endpoint, &blob));
                }
            }

            // Collect DNS records (no hex encoding while holding lock)
            let dns_records: Vec<_> = state
                .dns
                .iter()
                .map(|(host, (addrs, expires))| (host.clone(), addrs.clone(), to_unix(*expires)))
                .collect();

            // Collect TLS blobs (no hex encoding while holding lock)
            let tls_blobs: Vec<_> = state
                .tls
                .iter()
                .map(|(endpoint, (blob, expires))| {
                    (endpoint.clone(), blob.clone(), to_unix(*expires))
                })
                .collect();

            (dns_records, tls_blobs, inner.path.clone())
        };

        // Build records with hex encoding outside the lock
        let mut records = Vec::with_capacity(dns_records.len() + tls_blobs.len());
        for (host, addrs, exp) in dns_records {
            records.push(Record::Dns { host, addrs, exp });
        }
        for (endpoint, blob, exp) in tls_blobs {
            records.push(Record::Tls {
                endpoint,
                blob: to_hex(&blob),
                exp,
            });
        }

        // Serialize and write outside the lock
        let temp_path = temp_path(&path);
        {
            let mut file = crate::policy::private_files::private_file()
                .create_new(true)
                .write(true)
                .open(&temp_path)?;
            for record in &records {
                let line = serde_json::to_string(record).map_err(|error| Error::Other {
                    code: "session_cache_encode".into(),
                    message: error.to_string(),
                })?;
                file.write_all(line.as_bytes())?;
                file.write_all(b"\n")?;
            }
            file.sync_all()?;
        }
        fs::rename(&temp_path, &path)?;

        // Only clear the dirty flag if no modifications happened during the write.
        {
            let mut state = inner.state.write();
            if state.generation == generation_at_start {
                state.dirty = false;
            }
        }
        Ok(())
    }

    fn mutate<T>(&self, apply: impl FnOnce(&mut State) -> T) -> T
    where
        T: Default,
    {
        match self.inner.as_ref() {
            Some(inner) => {
                let mut state = inner.state.write();
                let out = apply(&mut state);
                state.dirty = true;
                state.generation = state.generation.wrapping_add(1);
                out
            }
            None => T::default(),
        }
    }
}

/// A temp path no other flusher can be using: the periodic flush and a
/// `flush()` at context close run on different threads of the same process,
/// so the process id alone is not unique. Each flush renames its own file
/// over the target; leftovers from a crash are removed at load time.
fn temp_path(path: &Path) -> PathBuf {
    static FLUSH_SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let sequence = FLUSH_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".tmp-{}-{sequence}", std::process::id()));
    PathBuf::from(name)
}

fn load(path: &Path) -> Result<State> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(State::default()),
        Err(error) => return Err(Error::Io(error)),
    };
    let mut state = State::default();
    for line in BufReader::new(file).lines() {
        // A torn final line from a crashed writer simply fails to parse.
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Record>(&line) else {
            continue;
        };
        match record {
            Record::Dns { host, addrs, exp } => {
                let size = estimate_dns_size(&host, &addrs);
                state.byte_estimate += size;
                state.dns.insert(host, (addrs, from_unix(exp)));
            }
            Record::Tls {
                endpoint,
                blob,
                exp,
            } => {
                if let Some(blob_bytes) = from_hex(&blob) {
                    let size = estimate_tls_size(&endpoint, &blob_bytes);
                    state.byte_estimate += size;
                    state.tls.insert(endpoint, (blob_bytes, from_unix(exp)));
                }
            }
        }
    }
    Ok(state)
}

fn to_unix(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn from_unix(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
        out.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('0'));
    }
    out
}

fn from_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.as_chunks::<2>().0 {
        let high = (pair[0] as char).to_digit(16)?;
        let low = (pair[1] as char).to_digit(16)?;
        out.push((high * 16 + low) as u8);
    }
    Some(out)
}

/// Estimate the serialized size of a DNS entry (host key + addresses + expiry).
fn estimate_dns_size(host: &str, addrs: &[IpAddr]) -> usize {
    // Approximate JSON size: key string + array of address strings
    // Each address is roughly 16-40 bytes when serialized
    let addr_size = addrs.len() * 40;
    host.len() + addr_size + 64 // Add overhead for JSON structure
}

/// Estimate the serialized size of a TLS entry (endpoint key + hex blob + expiry).
fn estimate_tls_size(endpoint: &str, blob: &[u8]) -> usize {
    // TLS blob serialized as hex (2 chars per byte) + endpoint string + overhead
    endpoint.len() + blob.len() * 2 + 64 // Add overhead for JSON structure
}

/// Evict oldest TLS entries first until byte_estimate is under the limit.
fn evict_oldest_tls_first(state: &mut State, max_bytes: usize) {
    const TARGET_BYTES: usize = 9 * 1024 * 1024; // Target 9 MiB to leave some headroom

    // Collect TLS entries sorted by expiry (oldest first)
    let mut tls_entries: Vec<_> = state
        .tls
        .iter()
        .map(|(endpoint, (blob, expires))| {
            (
                endpoint.clone(),
                blob.clone(),
                *expires,
                estimate_tls_size(endpoint, blob),
            )
        })
        .collect();
    tls_entries.sort_by_key(|(_, _, expires, _)| *expires);

    // Remove oldest TLS entries until we're under target
    for (endpoint, _blob, _, size) in tls_entries {
        if state.byte_estimate <= TARGET_BYTES {
            break;
        }
        state.tls.remove(&endpoint);
        state.byte_estimate = state.byte_estimate.saturating_sub(size);
    }

    // If still over limit, remove oldest DNS entries
    if state.byte_estimate > max_bytes {
        let mut dns_entries: Vec<_> = state
            .dns
            .iter()
            .map(|(host, (addrs, expires))| {
                (
                    host.clone(),
                    addrs.clone(),
                    *expires,
                    estimate_dns_size(host, addrs),
                )
            })
            .collect();
        dns_entries.sort_by_key(|(_, _, expires, _)| *expires);

        for (host, _addrs, _, size) in dns_entries {
            if state.byte_estimate <= TARGET_BYTES {
                break;
            }
            state.dns.remove(&host);
            state.byte_estimate = state.byte_estimate.saturating_sub(size);
        }
    }
}

#[cfg(test)]
mod tests;
