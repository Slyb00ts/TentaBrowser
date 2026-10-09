//! EXIF orientation.

use super::ctx::Ctx;
use super::error::DecodeError;
use super::pixels::Pixels;

/// How stored pixels must be transformed to display upright (EXIF tag 0x0112).
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
pub(super) enum Orientation {
    #[default]
    Identity,
    FlipHorizontal,
    Rotate180,
    FlipVertical,
    Transpose,
    Rotate90,
    Transverse,
    Rotate270,
}

impl Orientation {
    pub(super) fn from_exif(value: u16) -> Self {
        match value {
            2 => Self::FlipHorizontal,
            3 => Self::Rotate180,
            4 => Self::FlipVertical,
            5 => Self::Transpose,
            6 => Self::Rotate90,
            7 => Self::Transverse,
            8 => Self::Rotate270,
            _ => Self::Identity,
        }
    }

    /// True when the displayed width is the stored height.
    pub(super) fn swaps_axes(self) -> bool {
        matches!(
            self,
            Self::Transpose | Self::Rotate90 | Self::Transverse | Self::Rotate270
        )
    }

    /// Returns the transformed copy of a `width x height` RGBA8 buffer.
    pub(super) fn apply(
        self,
        rgba: &[u8],
        width: u32,
        height: u32,
        ctx: &Ctx,
    ) -> Result<(Pixels, u32, u32), DecodeError> {
        let (sw, sh) = (width as usize, height as usize);
        let (ow, oh) = if self.swaps_axes() {
            (sh, sw)
        } else {
            (sw, sh)
        };
        let mut out = Pixels::zeroed(rgba.len())?;
        for dy in 0..oh {
            ctx.checkpoint(dy, ow * 4)?;
            for dx in 0..ow {
                let (sx, sy) = match self {
                    Self::Identity => (dx, dy),
                    Self::FlipHorizontal => (sw - 1 - dx, dy),
                    Self::Rotate180 => (sw - 1 - dx, sh - 1 - dy),
                    Self::FlipVertical => (dx, sh - 1 - dy),
                    Self::Transpose => (dy, dx),
                    Self::Rotate90 => (dy, sh - 1 - dx),
                    Self::Transverse => (sw - 1 - dy, sh - 1 - dx),
                    Self::Rotate270 => (sw - 1 - dy, dx),
                };
                let from = (sy * sw + sx) * 4;
                let to = (dy * ow + dx) * 4;
                out[to..to + 4].copy_from_slice(&rgba[from..from + 4]);
            }
        }
        Ok((out, ow as u32, oh as u32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::ctx::DecodeControl;
    use crate::image::{DecodeLimits, DecodeParams};

    /// A 3x2 image whose pixel value is its index, so every move is visible.
    fn sample() -> Vec<u8> {
        (0..6u8).flat_map(|i| [i, 0, 0, 255]).collect()
    }

    fn reds(rgba: &[u8]) -> Vec<u8> {
        rgba.chunks(4).map(|p| p[0]).collect()
    }

    #[test]
    fn each_orientation_moves_pixels_as_exif_defines() {
        // Stored:  0 1 2
        //          3 4 5
        let cases: [(u16, &[u8], (u32, u32)); 8] = [
            (1, &[0, 1, 2, 3, 4, 5], (3, 2)),
            (2, &[2, 1, 0, 5, 4, 3], (3, 2)),
            (3, &[5, 4, 3, 2, 1, 0], (3, 2)),
            (4, &[3, 4, 5, 0, 1, 2], (3, 2)),
            (5, &[0, 3, 1, 4, 2, 5], (2, 3)),
            (6, &[3, 0, 4, 1, 5, 2], (2, 3)),
            (7, &[5, 2, 4, 1, 3, 0], (2, 3)),
            (8, &[2, 5, 1, 4, 0, 3], (2, 3)),
        ];
        for (exif, expected, dims) in cases {
            let (limits, control) = (DecodeLimits::default(), DecodeControl::new());
            let ctx = Ctx::new(&limits, &DecodeParams::default(), &control);
            let (out, w, h) = Orientation::from_exif(exif)
                .apply(&sample(), 3, 2, &ctx)
                .expect("orients");
            assert_eq!(reds(&out), expected, "orientation {exif}");
            assert_eq!((w, h), dims, "orientation {exif}");
        }
    }

    #[test]
    fn out_of_range_values_are_upright() {
        assert_eq!(Orientation::from_exif(0), Orientation::Identity);
        assert_eq!(Orientation::from_exif(9), Orientation::Identity);
    }
}
