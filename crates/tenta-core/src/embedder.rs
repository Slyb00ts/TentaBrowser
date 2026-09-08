//! Embedder traits (`docs/spec/02-architecture.md` §2): where a lower crate
//! needs a service from a higher one, the trait lives here and the higher
//! crate registers an implementation at engine construction.
//!
//! The signatures are the P0 minimum; each phase that implements a trait
//! extends it in place (paint in P3c, media in P6a, page in P2).

use std::sync::Arc;

use bytes::Bytes;

use crate::ids::{FrameId, PageId};

/// A decoded video frame as consumed by the paint layer. Planes are shared
/// buffers so a frame can be painted without copying.
#[derive(Clone, Debug)]
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    /// Plane data in `format` order (one plane for RGBA, three for I420).
    pub planes: Vec<Bytes>,
    /// Row strides matching `planes`.
    pub strides: Vec<u32>,
    /// Presentation timestamp in microseconds on the media clock.
    pub pts_us: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PixelFormat {
    Rgba8,
    I420,
    Nv12,
}

/// Implemented by `tenta-media` for `<video>` elements and canvases fed by
/// media streams; consumed by `tenta-paint` without depending on media.
pub trait VideoFrameSource: Send + Sync {
    /// The most recent frame to paint, if any has been decoded.
    fn current_frame(&self) -> Option<Arc<VideoFrame>>;
    /// Intrinsic size in CSS pixels, once known.
    fn intrinsic_size(&self) -> Option<(u32, u32)>;
}

/// How a navigation request from script or a link should be treated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NavigationRequest {
    pub page: PageId,
    pub frame: FrameId,
    pub url: String,
    pub referrer: Option<String>,
    /// `true` for `location.replace`, meta refresh and similar.
    pub replace: bool,
}

/// Implemented by `tenta-page`; used by the bindings for `location`,
/// `history`, `window.open` and form submission.
pub trait NavigationOps: Send + Sync {
    /// Starts a navigation. Returns `Err` when policy refuses it.
    fn navigate(&self, request: NavigationRequest) -> crate::Result<()>;
    /// `history.go(delta)`.
    fn traverse(&self, page: PageId, delta: i32) -> crate::Result<()>;
    /// `window.stop()`.
    fn stop(&self, page: PageId);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogKind {
    Alert,
    Confirm,
    Prompt,
    BeforeUnload,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DialogRequest {
    pub page: PageId,
    pub frame: FrameId,
    pub kind: DialogKind,
    pub message: String,
    pub default_value: Option<String>,
}

/// The host's answer to a dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialogResponse {
    Accept { value: Option<String> },
    Dismiss,
}

/// Implemented by `tenta-page`, which runs the nested loop while the calling
/// script is parked (`docs/spec/02-architecture.md` §3).
pub trait DialogHost: Send + Sync {
    /// Blocks the calling context until the host answers or
    /// `dialog_timeout` elapses.
    fn show(&self, request: DialogRequest) -> DialogResponse;
}
