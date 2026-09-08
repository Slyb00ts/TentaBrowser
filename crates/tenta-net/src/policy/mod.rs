//! Pure policy modules of Tier 0 (`docs/spec/03-network.md` §4–§8,
//! `docs/spec/20-platform-behaviour.md` §11): they take URLs, headers and
//! clocks and never touch a socket, so the transport (`crate::client`) stays
//! free of policy and these modules are testable without IO.

pub mod cache;
pub mod clock;
pub mod cookies;
pub mod error_names;
pub mod private_files;
pub mod private_network;
pub mod rate_limit;
pub mod redact;
pub mod robots;
pub mod session_cache;

#[cfg(test)]
mod fuzz_lite;

pub use cache::{CacheDecision, CacheRequest, CachedResponse, HttpCache};
pub use clock::{Clock, ManualClock, SystemClock};
pub use cookies::{Cookie, CookieJar, CookiePolicy, PartitionKey, SameSite, Site};
pub use error_names::ErrorName;
pub use private_network::{AddressClass, DestinationPolicy, RebindGuard};
pub use rate_limit::{Acquire, HostLimiter};
pub use robots::{RobotsCache, RobotsStatus, RobotsTxt};
pub use session_cache::SessionCache;
