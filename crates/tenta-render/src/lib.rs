//! Page-level orchestration of style, layout and paint: geometry APIs, screenshots, PDF, screencast, hit testing.
//!
//! [`RenderSession`] is the single owner of everything derived from a
//! document. The page creates it lazily on the first geometry or capture
//! request (Tier 3) and holds the only copy. Dependency direction is enforced
//! by `cargo xtask lint-deps` (`docs/spec/02-architecture.md` §2).

mod geometry;
mod images;
mod screenshot;
mod script_geometry;
mod session;
mod viewport;

pub use geometry::{BoxQuads, ElementGeometry, Quad, element_geometry, hit_test, node_boxes};
pub use images::{
    DataUrl, DecodeSettings, Decoded, ElementImage, ImageKind, ImagePlan, ImageStats, ImageStatus,
    ImageWant, LazyRegion, Rerender, WantOrigin, decode_data_url, decode_image, is_image_element,
};
pub use screenshot::{
    Capture, ImageFormat, RenderError, Screenshot, ScreenshotOptions, StageTimes,
};
pub use script_geometry::{ElementMetrics, PseudoArgument, css_number, parse_pseudo};
pub use session::{RenderSession, WorkCounters};
pub use tenta_layout::ImageKey;
pub use tenta_paint::{DecodeControl, DecodeError, RasterStats};
pub use viewport::{FontPolicy, RenderConfig, Viewport};
