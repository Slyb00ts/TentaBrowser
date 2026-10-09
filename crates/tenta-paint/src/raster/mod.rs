//! CPU rasterisation of display lists with tiny-skia.

use crate::PaintError;
use crate::dl::{DisplayList, Item};
use crate::glyph;
use crate::image::ImageStore;
use tenta_layout::Color;

/// A premultiplied RGBA8 raster.
#[derive(Clone, Debug)]
pub struct Pixmap(tiny_skia::Pixmap);

impl Pixmap {
    /// A transparent pixmap; `None` when a dimension is zero or the size
    /// overflows.
    pub fn new(width: u32, height: u32) -> Option<Pixmap> {
        tiny_skia::Pixmap::new(width, height).map(Pixmap)
    }

    /// Width in pixels.
    pub fn width(&self) -> u32 {
        self.0.width()
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        self.0.height()
    }

    /// Premultiplied RGBA8 pixels, row-major.
    pub fn data(&self) -> &[u8] {
        self.0.data()
    }
}

/// How and where to rasterise.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct RasterOptions {
    /// Output width in device pixels.
    pub width: u32,
    /// Output height in device pixels.
    pub height: u32,
    /// Device pixels per CSS pixel.
    pub scale: f32,
    /// Top-left of the output in CSS pixels.
    pub origin: (f32, f32),
    /// Background; `None` leaves the pixmap transparent.
    pub background: Option<Color>,
}

/// Rasterises `list` into a new pixmap.
pub fn rasterize(
    list: &DisplayList,
    opts: &RasterOptions,
    images: &ImageStore,
) -> Result<Pixmap, PaintError> {
    let mut canvas =
        tiny_skia::Pixmap::new(opts.width, opts.height).ok_or(PaintError::InvalidSize {
            width: opts.width,
            height: opts.height,
        })?;
    let transform = tiny_skia::Transform::from_scale(opts.scale, opts.scale)
        .post_translate(-opts.origin.0 * opts.scale, -opts.origin.1 * opts.scale);
    for item in &list.items {
        draw_item(&mut canvas, transform, item, images)?;
    }
    Ok(Pixmap(canvas))
}

/// Draws one item. Glyph runs and text decorations are drawn by the glyph
/// module; every other item is drawn here.
fn draw_item(
    canvas: &mut tiny_skia::Pixmap,
    transform: tiny_skia::Transform,
    item: &Item,
    images: &ImageStore,
) -> Result<(), PaintError> {
    match item {
        Item::Text { origin, run, color } => {
            glyph::draw_text(canvas, transform, *origin, run, *color)
        }
        Item::Decoration { rect, color, style } => {
            glyph::draw_decoration(canvas, transform, *rect, *color, *style)
        }
        _ => {
            let _ = (canvas, transform, images);
            todo!("P1")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixmap_rejects_zero_dimensions_and_reports_its_size() {
        assert!(Pixmap::new(0, 4).is_none());
        let pix = Pixmap::new(3, 2).unwrap();
        assert_eq!((pix.width(), pix.height()), (3, 2));
        assert_eq!(pix.data().len(), 3 * 2 * 4);
        assert!(pix.data().iter().all(|&b| b == 0));
    }
}
