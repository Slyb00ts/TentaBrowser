pub use tracing;
pub use tracing_subscriber;

use tracing_subscriber::util::SubscriberInitExt;

/// Installs a test-local subscriber for the duration of one test.
///
/// Upstream uses `tracing-tree` over `atty`; both are dropped here (the
/// `atty` advisory would fail `cargo deny`), so this is the plain `fmt`
/// layer writing through the test harness capture.
pub fn init() -> tracing::dispatcher::DefaultGuard {
    tracing_subscriber::fmt()
        .with_test_writer()
        .with_max_level(tracing::Level::TRACE)
        .set_default()
}
