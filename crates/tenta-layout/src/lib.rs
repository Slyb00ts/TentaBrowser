//! Tier 3: box tree, block/inline/flex/grid/table/positioned layout, text and form controls.
//!
//! Skeleton crate created in P0; the implementation lands in the phase named
//! for it in `docs/spec/16-roadmap.md`. Dependency direction is enforced by
//! `cargo xtask lint-deps` (`docs/spec/02-architecture.md` §2).

#[allow(unused_imports)]
use tenta_core as _;
#[allow(unused_imports)]
use tenta_css as _;
#[allow(unused_imports)]
use tenta_dom as _;
#[allow(unused_imports)]
use tenta_js as _;

#[cfg(feature = "embedded-fonts")]
pub mod embedded_fonts;
