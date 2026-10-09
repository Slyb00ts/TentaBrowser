//! Area-averaging downscale of premultiplied RGBA8.

use super::ctx::Ctx;
use super::error::DecodeError;
use super::pixels::{Pixels, filled};

/// Shrinks an image to `dw x dh` (both no larger than the source) one source
/// row at a time.
///
/// Every destination pixel is the exact area average of the source pixels it
/// covers, which is what a mip chain would converge on without its blur.
/// Averaging premultiplied values is linear, so colour never bleeds out of
/// transparent texels. The scratch memory is one source row of `f32` plus one
/// destination row, so a decoder that produces rows in order (PNG) can shrink
/// without ever holding the full-size image.
pub(super) struct Scaler {
    sw: usize,
    sh: usize,
    dw: usize,
    dh: usize,
    x_ratio: f64,
    y_ratio: f64,
    next_sy: usize,
    dy: usize,
    acc: Vec<f32>,
    done: Vec<u8>,
}

impl Scaler {
    pub(super) fn new(sw: u32, sh: u32, dw: u32, dh: u32) -> Result<Self, DecodeError> {
        let (sw, sh, dw, dh) = (sw as usize, sh as usize, dw as usize, dh as usize);
        Ok(Self {
            sw,
            sh,
            dw,
            dh,
            x_ratio: sw as f64 / dw as f64,
            y_ratio: sh as f64 / dh as f64,
            next_sy: 0,
            dy: 0,
            acc: filled(sw * 4, 0f32)?,
            done: filled(dw * 4, 0u8)?,
        })
    }

    /// Feeds the next source row (`sw * 4` bytes). Returns the index of the
    /// destination row this row completed; its pixels are in
    /// [`Scaler::finished`] until the next call.
    pub(super) fn push_row(&mut self, row: &[u8]) -> Option<usize> {
        debug_assert_eq!(row.len(), self.sw * 4);
        let sy = self.next_sy;
        self.next_sy += 1;
        let bottom = (sy + 1) as f64;
        let mut completed = None;
        while self.dy < self.dh {
            let y0 = self.dy as f64 * self.y_ratio;
            if y0 >= bottom {
                break;
            }
            let y1 = (self.dy + 1) as f64 * self.y_ratio;
            let weight = overlap(sy, y0, y1) as f32;
            for (acc, &v) in self.acc.iter_mut().zip(row) {
                *acc += weight * f32::from(v);
            }
            // The last destination row also ends at the last source row, which
            // guards against `y1` landing a rounding error past the edge.
            let ends_here = y1 <= bottom || (sy + 1 == self.sh && self.dy + 1 == self.dh);
            if !ends_here {
                break;
            }
            self.emit(y1 - y0);
            completed = Some(self.dy);
            self.dy += 1;
            self.acc.fill(0.0);
        }
        completed
    }

    /// The destination row completed by the last [`Scaler::push_row`].
    pub(super) fn finished(&self) -> &[u8] {
        &self.done
    }

    fn emit(&mut self, y_span: f64) {
        for dx in 0..self.dw {
            let (x0, x1) = (dx as f64 * self.x_ratio, (dx + 1) as f64 * self.x_ratio);
            let mut sum = [0f32; 4];
            for sx in x0 as usize..(x1.ceil() as usize).min(self.sw) {
                let weight = overlap(sx, x0, x1) as f32;
                for (s, &v) in sum.iter_mut().zip(&self.acc[sx * 4..sx * 4 + 4]) {
                    *s += weight * v;
                }
            }
            let norm = 1.0 / ((x1 - x0) * y_span) as f32;
            let px = &mut self.done[dx * 4..][..4];
            for (o, s) in px.iter_mut().zip(sum) {
                *o = (s * norm + 0.5).clamp(0.0, 255.0) as u8;
            }
            // Rounding must not push a colour above its alpha.
            let alpha = px[3];
            for c in &mut px[..3] {
                *c = (*c).min(alpha);
            }
        }
    }
}

/// Shrinks `src` (`sw x sh`) to `dw x dh` into a new buffer.
pub(super) fn downscale(
    src: &[u8],
    sw: u32,
    sh: u32,
    dw: u32,
    dh: u32,
    ctx: &Ctx,
) -> Result<Pixels, DecodeError> {
    let mut scaler = Scaler::new(sw, sh, dw, dh)?;
    let mut out = Pixels::zeroed(dw as usize * dh as usize * 4)?;
    let row_len = sw as usize * 4;
    for (sy, row) in src.chunks_exact(row_len).take(sh as usize).enumerate() {
        ctx.checkpoint(sy, row_len)?;
        if let Some(dy) = scaler.push_row(row) {
            let at = dy * dw as usize * 4;
            out[at..at + dw as usize * 4].copy_from_slice(scaler.finished());
        }
    }
    Ok(out)
}

/// Length of `[index, index + 1]` inside `[start, end]`.
fn overlap(index: usize, start: f64, end: f64) -> f64 {
    (end.min(index as f64 + 1.0) - start.max(index as f64)).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::ctx::DecodeControl;
    use crate::image::{DecodeLimits, DecodeParams};

    fn downscale(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
        let (limits, control) = (DecodeLimits::default(), DecodeControl::new());
        let ctx = Ctx::new(&limits, &DecodeParams::default(), &control);
        super::downscale(src, sw, sh, dw, dh, &ctx)
            .expect("downscale")
            .to_vec()
    }

    #[test]
    fn a_uniform_image_stays_uniform() {
        let src = [10, 20, 30, 255].repeat(7 * 5);
        let out = downscale(&src, 7, 5, 3, 2);
        assert_eq!(out, [10, 20, 30, 255].repeat(6));
    }

    #[test]
    fn two_by_two_halves_to_the_mean() {
        let src = [
            0, 0, 0, 255, 100, 100, 100, 255, //
            200, 200, 200, 255, 100, 100, 100, 255,
        ];
        assert_eq!(downscale(&src, 2, 2, 1, 1), [100, 100, 100, 255]);
    }

    #[test]
    fn transparent_texels_contribute_no_colour() {
        // Opaque red next to fully transparent black, premultiplied.
        let src = [255, 0, 0, 255, 0, 0, 0, 0];
        assert_eq!(downscale(&src, 2, 1, 1, 1), [128, 0, 0, 128]);
    }

    #[test]
    fn fractional_ratios_weight_by_area() {
        // 3 -> 2: the middle source pixel is shared half and half.
        let src = [0, 0, 0, 255, 90, 90, 90, 255, 180, 180, 180, 255];
        let out = downscale(&src, 3, 1, 2, 1);
        assert_eq!(out, [30, 30, 30, 255, 150, 150, 150, 255]);
    }
}
