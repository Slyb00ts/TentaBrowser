//! Tier 0: HTTP client, TLS profiles, cookie jar, cache, DNS, proxy, WebSocket and the socket factory.
//!
//! `docs/spec/03-network.md`. Transport lives in the modules owned by the
//! client; pure policy (cookies, cache, session cache, robots, private
//! network checks) lives under [`policy`].

pub mod body;
pub mod client;
pub mod dns;
pub mod error;
pub mod h1;
mod h1_case;
pub mod h2;
pub mod headers;
pub mod hooks;
pub mod policy;
pub mod pool;
pub mod profile;
pub mod proxy;
pub mod request;
pub mod stream;
pub mod tls;

pub use body::{Body, BodyStream};
pub use client::{
    Client, ClientConfig, Coalescing, NetRuntime, Protocol, RedirectHop, Response, Timing,
};
pub use error::{ErrorName, NetError};
pub use policy::cache::HttpCache;
pub use policy::cookies::CookieJar;
pub use policy::private_network::DestinationPolicy;
pub use policy::session_cache::SessionCache;
pub use profile::{ClientProfile, Profile};
pub use request::{
    CredentialsMode, Priority, PriorityClass, RedirectPolicy, Request, ResourceType,
};
