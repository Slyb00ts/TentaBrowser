//! RTCPeerConnection over str0m: ICE, TURN client, DTLS/SRTP/SCTP, codec negotiation, simulcast, stats.
//!
//! Skeleton crate created in P0; the implementation lands in the phase named
//! for it in `docs/spec/16-roadmap.md`. Dependency direction is enforced by
//! `cargo xtask lint-deps` (`docs/spec/02-architecture.md` §2).

#[allow(unused_imports)]
use tenta_core as _;
#[allow(unused_imports)]
use tenta_js as _;
#[allow(unused_imports)]
use tenta_media as _;
#[allow(unused_imports)]
use tenta_net as _;
