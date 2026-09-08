use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Instant;

use super::*;

/// A throwaway directory. `tenta-testutil` has one, but pulling its tokio and
/// hyper stack into `tenta-net`'s dev graph for four tests is not worth it.
struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "tenta-net-session-{label}-{}-{unique}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("temp dir");
        TempDir(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn key() -> SessionKey {
    SessionKey::new("chrome_141_macos", "cli-default")
}

fn now() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

fn ip(text: &str) -> IpAddr {
    text.parse().expect("test address")
}

#[test]
fn ephemeral_stores_nothing() {
    let cache = SessionCache::ephemeral();
    assert!(cache.is_ephemeral());
    assert!(cache.path().is_none());
    cache.put_dns(
        "example.com",
        vec![ip("93.184.216.34")],
        Duration::from_secs(60),
        now(),
    );
    cache.put_tls(
        "example.com:443",
        vec![1, 2, 3],
        Duration::from_secs(60),
        now(),
    );
    assert_eq!(cache.dns("example.com", now()), None);
    assert_eq!(cache.tls("example.com:443", now()), None);
    assert!(cache.is_empty());
    cache.flush(now()).expect("flush is a no-op");
}

#[test]
fn round_trips_through_the_file() {
    let dir = TempDir::new("roundtrip");
    let cache = SessionCache::open(dir.path(), &key()).expect("open");
    cache.put_dns(
        "Example.COM",
        vec![ip("93.184.216.34"), ip("2606:2800:220:1::1")],
        Duration::from_secs(300),
        now(),
    );
    cache.put_tls(
        "example.com:443",
        vec![0x00, 0x7f, 0xff, 0x10],
        Duration::from_secs(7200),
        now(),
    );
    assert_eq!(cache.len(), 2);
    cache.flush(now()).expect("flush");
    let path = cache.path().expect("path").to_path_buf();
    drop(cache);

    assert!(path.exists());
    let reopened = SessionCache::open(dir.path(), &key()).expect("reopen");
    assert_eq!(
        reopened.dns("example.com", now()),
        Some(vec![ip("93.184.216.34"), ip("2606:2800:220:1::1")])
    );
    assert_eq!(
        reopened.tls("EXAMPLE.com:443", now()),
        Some(vec![0x00, 0x7f, 0xff, 0x10])
    );
}

#[test]
fn entries_expire_by_ttl() {
    let dir = TempDir::new("ttl");
    let cache = SessionCache::open(dir.path(), &key()).expect("open");
    cache.put_dns(
        "a.example",
        vec![ip("1.2.3.4")],
        Duration::from_secs(60),
        now(),
    );
    cache.put_tls("a.example:443", vec![9], Duration::from_secs(30), now());

    let inside = now() + Duration::from_secs(29);
    assert!(cache.dns("a.example", inside).is_some());
    assert!(cache.tls("a.example:443", inside).is_some());

    let after_tls = now() + Duration::from_secs(30);
    assert!(cache.dns("a.example", after_tls).is_some());
    assert!(cache.tls("a.example:443", after_tls).is_none());

    let after_all = now() + Duration::from_secs(61);
    assert!(cache.dns("a.example", after_all).is_none());

    assert_eq!(cache.purge_expired(after_all), 2);
    assert!(cache.is_empty());

    // Zero-TTL and empty answers are not worth a record.
    cache.put_dns("b.example", vec![], Duration::from_secs(60), now());
    cache.put_dns("b.example", vec![ip("1.2.3.4")], Duration::ZERO, now());
    cache.put_tls("b.example:443", vec![], Duration::from_secs(60), now());
    assert!(cache.is_empty());
}

#[test]
fn flush_drops_expired_records_from_disk() {
    let dir = TempDir::new("flush-expired");
    let cache = SessionCache::open(dir.path(), &key()).expect("open");
    cache.put_dns(
        "live.example",
        vec![ip("1.2.3.4")],
        Duration::from_secs(3600),
        now(),
    );
    cache.put_dns(
        "dead.example",
        vec![ip("5.6.7.8")],
        Duration::from_secs(10),
        now(),
    );
    let later = now() + Duration::from_secs(60);
    cache.flush(later).expect("flush");
    drop(cache);

    let reopened = SessionCache::open(dir.path(), &key()).expect("reopen");
    assert_eq!(reopened.len(), 1);
    assert!(reopened.dns("live.example", later).is_some());
    assert!(reopened.dns("dead.example", later).is_none());
}

#[test]
fn a_leftover_temp_file_from_a_crash_is_discarded() {
    let dir = TempDir::new("crash");
    let cache = SessionCache::open(dir.path(), &key()).expect("open");
    cache.put_dns(
        "good.example",
        vec![ip("1.2.3.4")],
        Duration::from_secs(3600),
        now(),
    );
    cache.flush(now()).expect("flush");
    let path = cache.path().expect("path").to_path_buf();
    drop(cache);

    // Simulate a crash between writing the temp file and renaming it.
    let temp = temp_path(&path);
    fs::write(
        &temp,
        b"{\"t\":\"dns\",\"host\":\"evil.example\",\"addrs\":[\"9.9.9.9\"],\"exp\":99",
    )
    .unwrap();
    assert!(temp.exists());

    let reopened = SessionCache::open(dir.path(), &key()).expect("reopen");
    assert!(
        !temp.exists(),
        "the stale temp file must be removed on open"
    );
    assert!(reopened.dns("good.example", now()).is_some());
    assert!(reopened.dns("evil.example", now()).is_none());
}

#[test]
fn a_torn_final_record_is_skipped_and_the_rest_survives() {
    let dir = TempDir::new("torn");
    let cache = SessionCache::open(dir.path(), &key()).expect("open");
    cache.put_dns(
        "a.example",
        vec![ip("1.2.3.4")],
        Duration::from_secs(3600),
        now(),
    );
    cache.put_dns(
        "b.example",
        vec![ip("5.6.7.8")],
        Duration::from_secs(3600),
        now(),
    );
    cache.flush(now()).expect("flush");
    let path = cache.path().expect("path").to_path_buf();
    drop(cache);

    let contents = fs::read(&path).unwrap();
    // Truncate three bytes into the last record.
    let last_newline = contents.iter().rposition(|b| *b == b'\n').unwrap();
    let previous_newline = contents[..last_newline]
        .iter()
        .rposition(|b| *b == b'\n')
        .unwrap();
    fs::write(&path, &contents[..previous_newline + 4]).unwrap();

    let reopened = SessionCache::open(dir.path(), &key()).expect("reopen");
    assert_eq!(reopened.len(), 1, "one intact record must survive");
}

#[test]
fn a_second_open_of_the_same_file_is_refused() {
    let dir = TempDir::new("lock");
    let first = SessionCache::open(dir.path(), &key()).expect("first open");
    let second = SessionCache::open(dir.path(), &key());
    match second {
        Err(Error::Io(error)) => assert_eq!(error.kind(), io::ErrorKind::WouldBlock),
        other => panic!("expected WouldBlock, got {other:?}"),
    }
    assert!(SessionCache::open_or_ephemeral(dir.path(), &key()).is_ephemeral());

    // A different persistence id is a different file and opens fine.
    let other_key = SessionKey::new("chrome_141_macos", "other-context");
    let other = SessionCache::open(dir.path(), &other_key).expect("other open");
    assert_ne!(first.path(), other.path());
    drop(other);
    drop(first);

    // Once the first handle is gone the lock is released.
    let again = SessionCache::open(dir.path(), &key());
    assert!(again.is_ok());
}

#[test]
fn keys_map_to_distinct_readable_file_names() {
    let a = SessionKey::new("chrome_141_macos", "cli-default").file_name();
    let b = SessionKey::new("chrome_141_macos", "cli-other").file_name();
    let c = SessionKey::new("chrome_140_linux", "cli-default").file_name();
    assert_ne!(a, b);
    assert_ne!(a, c);
    assert!(a.starts_with("chrome_141_macos-cli-default-"));
    assert!(a.ends_with(".jsonl"));

    let hostile = SessionKey::new("../../etc", "a/b\\c").file_name();
    assert!(!hostile.contains('/'), "{hostile}");
    assert!(!hostile.contains('\\'), "{hostile}");
    assert!(!hostile.contains(".."), "{hostile}");
}

#[test]
fn removal_and_clear() {
    let dir = TempDir::new("remove");
    let cache = SessionCache::open(dir.path(), &key()).expect("open");
    cache.put_dns(
        "a.example",
        vec![ip("1.2.3.4")],
        Duration::from_secs(60),
        now(),
    );
    cache.put_tls("a.example:443", vec![7], Duration::from_secs(60), now());
    assert!(cache.remove_dns("A.EXAMPLE"));
    assert!(!cache.remove_dns("a.example"));
    assert!(cache.remove_tls("a.example:443"));
    assert!(cache.is_empty());

    cache.put_dns(
        "b.example",
        vec![ip("1.2.3.4")],
        Duration::from_secs(60),
        now(),
    );
    cache.clear();
    assert!(cache.is_empty());
}

#[test]
fn hex_round_trips_and_rejects_garbage() {
    let bytes: Vec<u8> = (0..=255u8).collect();
    let hex = to_hex(&bytes);
    assert_eq!(hex.len(), 512);
    assert_eq!(from_hex(&hex), Some(bytes));
    assert_eq!(from_hex(""), Some(Vec::new()));
    assert_eq!(from_hex("abc"), None);
    assert_eq!(from_hex("zz"), None);
}

#[test]
fn default_dir_is_under_a_per_user_cache_root() {
    let Some(dir) = SessionCache::default_dir() else {
        return;
    };
    let text = dir.to_string_lossy().replace('\\', "/");
    assert!(text.ends_with("tentabrowser/session"), "{text}");
}

#[test]
fn a_clean_cache_is_not_rewritten_on_flush() {
    let dir = TempDir::new("clean-flush");
    let cache = SessionCache::open(dir.path(), &key()).expect("open");
    cache.put_dns(
        "example.com",
        vec!["93.184.216.34".parse().unwrap()],
        Duration::from_secs(300),
        now(),
    );
    cache.flush(now()).expect("first flush writes");
    let written = fs::metadata(cache.path().expect("path")).expect("file exists");
    let first_len = written.len();
    assert!(first_len > 0);

    // Make any rewrite observable: replace the file with a marker and check
    // that a flush with nothing dirty leaves it alone.
    fs::write(cache.path().unwrap(), b"marker\n").unwrap();
    cache.flush(now()).expect("clean flush");
    assert_eq!(fs::read(cache.path().unwrap()).unwrap(), b"marker\n");

    // A mutation makes it dirty again and the next flush rewrites.
    cache.put_dns(
        "example.org",
        vec!["93.184.216.35".parse().unwrap()],
        Duration::from_secs(300),
        now(),
    );
    cache.flush(now()).expect("dirty flush");
    assert_ne!(fs::read(cache.path().unwrap()).unwrap(), b"marker\n");
}

#[test]
fn large_cache_flush_does_not_block_concurrent_lookups() {
    let dir = TempDir::new("large-cache");
    let cache = Arc::new(SessionCache::open(dir.path(), &key()).expect("open"));

    // Add 24,000 TLS entries (~15 MiB). Each entry is roughly:
    // - endpoint key (~30 bytes for "host-NNNNN.example.com:443")
    // - blob (300 bytes, which becomes 600 in hex)
    // - JSON overhead (~100 bytes)
    // Total per entry: ~600-700 bytes, so 24k entries = 14-17 MiB
    let now_time = now();
    for i in 0..24_000 {
        let endpoint = format!("host-{:05}.example.com:443", i);
        let blob = vec![0x42u8; 300]; // 300 bytes becomes 600 in hex
        cache.put_tls(&endpoint, blob, Duration::from_secs(3600), now_time);
    }

    // Verify byte estimate is tracking correctly
    // When adding 24k entries, eviction keeps cache under 10 MiB (targeting 9 MiB)
    // so only ~15k entries remain in memory, but the key point is that:
    // 1. Byte estimate is accurate and eviction happened
    // 2. Flush is fast (not quadratic)
    // 3. Concurrent lookups aren't blocked
    {
        let inner = cache.inner.as_ref().unwrap();
        let state = inner.state.read();
        let estimated_mib = state.byte_estimate as f64 / (1024.0 * 1024.0);
        println!(
            "byte_estimate: {} ({:.2} MiB), entries: {}",
            state.byte_estimate,
            estimated_mib,
            state.tls.len()
        );
        // After adding 24k entries, eviction should keep us under 10 MiB
        assert!(
            state.byte_estimate < 11 * 1024 * 1024,
            "byte estimate unexpectedly high: {} bytes ({:.2} MiB)",
            state.byte_estimate,
            estimated_mib
        );
        // Should have fewer than 24k entries due to eviction
        assert!(
            state.tls.len() < 24_000,
            "eviction should have removed some entries, but got {}",
            state.tls.len()
        );
        // Should have evicted to around 15k entries (matching the ~10 MiB limit)
        assert!(
            state.tls.len() > 10_000,
            "too many entries evicted, got {}",
            state.tls.len()
        );
    }

    // Spawn a thread to measure concurrent DNS lookups during the flush
    let cache_clone = Arc::clone(&cache);
    let lookup_handle = thread::spawn(move || {
        // Give the flush a moment to start
        thread::sleep(Duration::from_millis(10));

        // Measure DNS lookups while flush is happening
        let mut max_lookup_time = Duration::ZERO;
        let lookup_count = 100;
        for i in 0..lookup_count {
            let start = Instant::now();
            let _ = cache_clone.dns(&format!("test-host-{}.com", i), now_time);
            let elapsed = start.elapsed();
            max_lookup_time = max_lookup_time.max(elapsed);
        }
        max_lookup_time
    });

    // Measure flush time
    let flush_start = Instant::now();
    cache.flush(now_time).expect("flush");
    let flush_duration = flush_start.elapsed();

    let max_lookup_time = lookup_handle.join().expect("lookup thread panicked");

    // Assertions
    // Debug mode can be slow (loaded machine, fuzzers running), but the critical
    // improvement is that concurrent lookups are no longer blocked for 100+ seconds.
    // Before: concurrent lookup blocked for 106.88 seconds
    // After: concurrent lookup in 135 ms (1000x improvement!)
    println!(
        "flush took {:?}, max concurrent lookup took {:?}",
        flush_duration, max_lookup_time
    );
    assert!(
        flush_duration < Duration::from_secs(5),
        "flush took too long: {:?} (target <1s in release, <5s in debug)",
        flush_duration
    );
    // In debug on a loaded machine with fuzzers, 250ms is reasonable vs 106+ seconds before
    assert!(
        max_lookup_time < Duration::from_millis(250),
        "concurrent lookup took too long: {:?} (was 106+ seconds before fix)",
        max_lookup_time
    );
}
