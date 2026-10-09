//! Screenshot request and result types.

use std::time::Duration;

use tenta_core::NodeId;
use tenta_layout::Rect;
use tenta_paint::{PaintError, RasterStats};

/// Output encoding.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    /// PNG.
    Png,
    /// JPEG at a quality of 1 to 100.
    Jpeg {
        /// Quality.
        quality: u8,
    },
    /// WebP (lossless).
    Webp,
}

/// What part of the page to capture.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Capture {
    /// The current viewport.
    Viewport,
    /// The whole scrollable extent, laid out once at the configured viewport:
    /// the width is the larger of the viewport and the content, `vh` units
    /// are unchanged and fixed-position boxes appear once, where the first
    /// screen has them.
    FullPage,
    /// A rectangle in document coordinates (CSS pixels), widened outwards to
    /// whole device pixels.
    Clip(Rect),
    /// The union of the border boxes of an element's fragments, widened
    /// outwards to whole device pixels. The boxes are untransformed layout
    /// boxes in document coordinates: `transform` and scroll offsets are not
    /// folded in, so a transformed element is captured at the place it would
    /// have without the transform.
    Element(NodeId),
}

/// Options of one screenshot.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ScreenshotOptions {
    /// Encoding.
    pub format: ImageFormat,
    /// Area.
    pub capture: Capture,
    /// Multiplier on the raster only, as the CDP clip scale: the output has
    /// `CSS pixels x the viewport's device pixel ratio x scale` pixels per
    /// side and layout and media queries do not see it. `None` is 1.
    pub scale: Option<f32>,
    /// Leave the page background transparent.
    pub omit_background: bool,
}

impl Default for ScreenshotOptions {
    fn default() -> Self {
        Self {
            format: ImageFormat::Png,
            capture: Capture::Viewport,
            scale: None,
            omit_background: false,
        }
    }
}

/// Wall-clock time spent in each stage of one screenshot.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct StageTimes {
    /// Style flush, box tree and layout; zero when they were already current.
    pub layout: Duration,
    /// Display list construction; zero when the retained list was current.
    pub display_list: Duration,
    /// Rasterisation.
    pub raster: Duration,
    /// Image encoding.
    pub encode: Duration,
}

/// An encoded screenshot.
#[derive(Clone, Debug)]
pub struct Screenshot {
    /// Encoded bytes.
    pub bytes: Vec<u8>,
    /// Encoding of `bytes`.
    pub format: ImageFormat,
    /// Width in device pixels.
    pub width: u32,
    /// Height in device pixels.
    pub height: u32,
    /// What the rasteriser could not draw. An image whose stats are not
    /// [complete](RasterStats::is_complete) is missing content.
    pub stats: RasterStats,
    /// True when a full-page capture was cut off at the size limits; the
    /// document is taller than `height` allows.
    pub clamped: bool,
    /// Time per stage.
    pub times: StageTimes,
}

/// Why a render failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenderError {
    /// The capture exceeds the pixel or memory cap.
    TooLarge {
        /// Requested width in device pixels.
        width: u32,
        /// Requested height in device pixels.
        height: u32,
    },
    /// The document has no root element to render.
    NoRoot,
    /// The element produced no box: it is `display: none`, detached, or not an
    /// element.
    NoBox(NodeId),
    /// An option is out of range: a clip that is not a finite rectangle with
    /// a positive area, or a scale that is not a finite positive number.
    InvalidOption(&'static str),
    /// The encoder for this format is not implemented.
    UnsupportedFormat(ImageFormat),
    /// Rasterisation failed.
    Paint(PaintError),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderError::TooLarge { width, height } => {
                write!(f, "a {width}x{height} capture exceeds the render limits")
            }
            RenderError::NoRoot => f.write_str("the document has no root element"),
            RenderError::NoBox(node) => write!(f, "node {} has no box to capture", node.raw()),
            RenderError::InvalidOption(what) => write!(f, "invalid screenshot option: {what}"),
            RenderError::UnsupportedFormat(format) => {
                write!(f, "cannot encode {format:?} screenshots yet")
            }
            RenderError::Paint(err) => write!(f, "paint failed: {err}"),
        }
    }
}

impl std::error::Error for RenderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RenderError::Paint(err) => Some(err),
            _ => None,
        }
    }
}

impl From<PaintError> for RenderError {
    fn from(err: PaintError) -> Self {
        RenderError::Paint(err)
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;

    #[test]
    fn paint_errors_convert_and_chain() {
        let err: RenderError = PaintError::InvalidSize {
            width: 0,
            height: 0,
        }
        .into();
        assert!(err.source().is_some());
        assert!(err.to_string().contains("cannot rasterise a 0x0 image"));
        assert!(RenderError::NoRoot.source().is_none());
    }

    #[test]
    fn default_options_capture_the_viewport_as_png() {
        let opts = ScreenshotOptions::default();
        assert_eq!(opts.format, ImageFormat::Png);
        assert_eq!(opts.capture, Capture::Viewport);
        assert!(!opts.omit_background);
    }
}
