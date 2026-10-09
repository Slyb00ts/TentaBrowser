//! Page-level orchestration of style, layout and paint: geometry APIs, screenshots, PDF, screencast, hit testing.
//!
//! [`RenderSession`] is the single owner of everything derived from a
//! document. The page creates it lazily on the first geometry or capture
//! request (Tier 3) and holds the only copy. Dependency direction is enforced
//! by `cargo xtask lint-deps` (`docs/spec/02-architecture.md` §2).

mod geometry;
mod screenshot;
mod session;
mod viewport;

pub use screenshot::{
    Capture, ImageFormat, RenderError, Screenshot, ScreenshotOptions, StageTimes,
};
pub use session::{RenderSession, WorkCounters};
pub use tenta_paint::RasterStats;
pub use viewport::{FontPolicy, RenderConfig, Viewport};
