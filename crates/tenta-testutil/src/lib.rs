//! Test support: loopback fixture server with simulated latency, temporary directories, fixture paths. Never a dependency of a shipping crate.
//!
//! The server in [`server`] serves `tests/fixtures/` (or any root) over
//! HTTP/1.1 on `127.0.0.1`, applies a simulated round-trip time and per-path
//! delays, and exposes the `/api/*` endpoints the fixture pages and the settle
//! tests rely on. It runs in-process from tests (`FixtureServer::start`) and as
//! the `fixture-server` binary for `bench/run.py`.

pub mod client;
pub mod fingerprint;
pub mod glob;
pub mod handshake_faults;
pub mod server;

use std::path::PathBuf;

pub use glob::Glob;
pub use handshake_faults::{HandshakeFault, HandshakeFaultServer};
pub use server::{FixtureServer, FixtureServerConfig, PathDelay};

/// Absolute path of the repository's `tests/fixtures/` directory.
pub fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tests")
        .join("fixtures")
}

/// Creates a fresh, uniquely named directory under the system temp root.
///
/// The directory is removed when the returned guard is dropped.
pub fn temp_dir(prefix: &str) -> TempDir {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path =
        std::env::temp_dir().join(format!("tenta-{prefix}-{}-{nanos}-{n}", std::process::id()));
    std::fs::create_dir_all(&path).expect("create temp dir");
    TempDir { path }
}

/// A directory removed on drop; see [`temp_dir`].
#[derive(Debug)]
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Writes `contents` to `relative` inside the directory, creating parents.
    pub fn write(&self, relative: &str, contents: impl AsRef<[u8]>) -> PathBuf {
        let path = self.path.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent dir");
        }
        std::fs::write(&path, contents).expect("write temp file");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
