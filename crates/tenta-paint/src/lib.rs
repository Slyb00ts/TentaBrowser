//! Display lists, CPU and GPU raster, images, SVG, fonts and the glyph cache.
//!
//! The paint-value types (`Color`, `Rect`, `BorderStyle`, ...) are defined in
//! `tenta-layout`, because fragments carry them and layout cannot depend on this
//! crate; they are re-exported here. This crate defines only what layout never
//! mentions: the display list, rasterisation and images. Dependency direction
//! is enforced by `cargo xtask lint-deps` (`docs/spec/02-architecture.md` §2).

mod build;
mod dl;
mod encode;
mod glyph;
mod image;
mod raster;

use std::fmt;

pub use build::{PaintContext, build};
pub use dl::{DisplayList, Item, Radii};
pub use encode::{PngProfile, encode_jpeg, encode_png};
pub use image::{DecodedImage, ImageId, ImageQuality, ImageStore};
pub use raster::{Pixmap, RasterOptions, rasterize};
pub use tenta_layout::types::{
    Affine, BackgroundContent, BackgroundLayer, BackgroundSize, BlendMode, BorderPaint,
    BorderStyle, BoxArea, Color, ColorStop, DecorationPaint, Edges, FilterOp, FontBlob, FontRef,
    Glyph, GlyphRun, GradientPaint, ImageKey, ImageRendering, LengthPercent, LineStyle, MarkerKind,
    MarkerPaint, ObjectFit, OutlinePaint, OverflowKind, PaintStyle, PositionKind, Rect, RepeatKind,
    ShadowPaint,
};

/// Why rasterisation failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaintError {
    /// A display-list item the rasteriser does not implement.
    Unsupported(&'static str),
    /// The requested output size is zero or too large to allocate.
    InvalidSize {
        /// Requested width in device pixels.
        width: u32,
        /// Requested height in device pixels.
        height: u32,
    },
}

impl fmt::Display for PaintError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PaintError::Unsupported(kind) => write!(f, "unsupported display item: {kind}"),
            PaintError::InvalidSize { width, height } => {
                write!(f, "cannot rasterise a {width}x{height} image")
            }
        }
    }
}

impl std::error::Error for PaintError {}

#[cfg(test)]
mod tests {
    use super::PaintError;

    #[test]
    fn errors_describe_themselves() {
        assert_eq!(
            PaintError::Unsupported("Image").to_string(),
            "unsupported display item: Image"
        );
        assert_eq!(
            PaintError::InvalidSize {
                width: 0,
                height: 5
            }
            .to_string(),
            "cannot rasterise a 0x5 image"
        );
    }
}
