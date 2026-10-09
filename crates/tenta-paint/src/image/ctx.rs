//! What one decode runs under: its memory limits, its deadline and its target
//! size, plus the peak-memory admission every decoder goes through.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::ImageFormat;
use super::error::DecodeError;
use super::limits::{DecodeLimits, DecodeParams};

/// Bytes of work between two reads of the clock. Reading it costs more than
/// decoding a small row, so loops check about once per this much output.
const CHECK_INTERVAL_BYTES: usize = 64 * 1024;

/// Cooperative stop conditions for one decode: a deadline and a cancel flag.
///
/// Decoders check them between PNG and BMP rows, RLE operations, GIF frames,
/// ICO entries, resize rows and orientation rows. `zune-jpeg` and `image-webp`
/// are opaque, so for them the checks sit before and after the call and the
/// pixel and memory limits are what bound the time spent inside it.
#[derive(Clone, Debug, Default)]
pub struct DecodeControl {
    deadline: Option<Instant>,
    cancel: Option<Arc<AtomicBool>>,
}

impl DecodeControl {
    /// No deadline and no cancel flag.
    pub fn new() -> Self {
        Self::default()
    }

    /// Stops the decode with [`DecodeError::TimedOut`] once `deadline` passes.
    pub fn with_deadline(mut self, deadline: Instant) -> Self {
        self.deadline = Some(deadline);
        self
    }

    /// Stops the decode with [`DecodeError::TimedOut`] after `timeout`.
    pub fn with_timeout(self, timeout: Duration) -> Self {
        match Instant::now().checked_add(timeout) {
            Some(deadline) => self.with_deadline(deadline),
            None => self,
        }
    }

    /// Stops the decode with [`DecodeError::Cancelled`] once the flag is set.
    pub fn with_cancel(mut self, flag: Arc<AtomicBool>) -> Self {
        self.cancel = Some(flag);
        self
    }

    pub(super) fn check(&self) -> Result<(), DecodeError> {
        if self
            .cancel
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Relaxed))
        {
            return Err(DecodeError::Cancelled);
        }
        if self.deadline.is_some_and(|d| Instant::now() >= d) {
            return Err(DecodeError::TimedOut);
        }
        Ok(())
    }
}

/// The inputs every decoder receives.
#[derive(Copy, Clone)]
pub(super) struct Ctx<'a> {
    pub(super) limits: &'a DecodeLimits,
    control: &'a DecodeControl,
    max_size: Option<(u32, u32)>,
    /// True when the decoder may shrink while it decodes instead of producing
    /// the full-size image first (PNG, at the top level only).
    pub(super) streams: bool,
}

impl<'a> Ctx<'a> {
    pub(super) fn new(
        limits: &'a DecodeLimits,
        params: &DecodeParams,
        control: &'a DecodeControl,
    ) -> Self {
        Self {
            limits,
            control,
            max_size: params.max_size,
            streams: true,
        }
    }

    /// The same decode for a nested image whose pixels are shrunk afterwards.
    pub(super) fn nested(self) -> Self {
        Self {
            streams: false,
            ..self
        }
    }

    pub(super) fn check(&self) -> Result<(), DecodeError> {
        self.control.check()
    }

    /// Checks the control when `index` falls on the check interval, given
    /// `unit_bytes` of work per index.
    pub(super) fn checkpoint(&self, index: usize, unit_bytes: usize) -> Result<(), DecodeError> {
        let every = (CHECK_INTERVAL_BYTES / unit_bytes.max(1)).max(1);
        if index.is_multiple_of(every) {
            self.control.check()
        } else {
            Ok(())
        }
    }

    /// The size the image is shrunk to, in the stored axes of an image whose
    /// orientation swaps them when `swaps_axes`.
    pub(super) fn target(&self, width: u32, height: u32, swaps_axes: bool) -> (u32, u32) {
        match self.max_size {
            Some((max_w, max_h)) => {
                let (max_w, max_h) = if swaps_axes {
                    (max_h, max_w)
                } else {
                    (max_w, max_h)
                };
                fit_within(width, height, max_w, max_h)
            }
            None => (width, height),
        }
    }

    /// Validates the declared dimensions and the size of the result, before a
    /// pixel buffer exists. This is the only place that decides `TooLarge`
    /// for the source and the output; [`Ctx::admit`] decides it for the peak.
    pub(super) fn plan(
        &self,
        format: ImageFormat,
        width: u32,
        height: u32,
        swaps_axes: bool,
    ) -> Result<Plan, DecodeError> {
        if width == 0 || height == 0 {
            return Err(DecodeError::malformed(format, "zero image dimension"));
        }
        let pixels = u64::from(width) * u64::from(height);
        let too_large = DecodeError::TooLarge { width, height };
        if pixels > self.limits.max_pixels {
            return Err(too_large);
        }
        let target = self.target(width, height, swaps_axes);
        let plan = Plan {
            width,
            height,
            pixels,
            target,
        };
        if plan.output_bytes() > self.limits.max_decoded_bytes as u64 {
            return Err(too_large);
        }
        Ok(plan)
    }

    /// Refuses a decode whose estimated peak heap exceeds the transient budget. `peak`
    /// is everything the decoder holds at its largest, including the finishing
    /// step (see [`Plan::finish_bytes`]).
    pub(super) fn admit(&self, plan: &Plan, peak: u64) -> Result<(), DecodeError> {
        if peak > self.limits.max_transient_bytes as u64 {
            return Err(DecodeError::TooLarge {
                width: plan.width,
                height: plan.height,
            });
        }
        Ok(())
    }
}

/// A source image that passed the dimension checks.
#[derive(Copy, Clone, Debug)]
pub(super) struct Plan {
    pub(super) width: u32,
    pub(super) height: u32,
    /// Source pixels.
    pub(super) pixels: u64,
    /// Size after the decode-time shrink, in stored axes.
    pub(super) target: (u32, u32),
}

impl Plan {
    pub(super) fn shrinks(&self) -> bool {
        self.target != (self.width, self.height)
    }

    /// Bytes of the shrunk RGBA8 output.
    pub(super) fn output_bytes(&self) -> u64 {
        u64::from(self.target.0) * u64::from(self.target.1) * 4
    }

    /// Bytes of the full-size RGBA8 image.
    pub(super) fn source_bytes(&self) -> u64 {
        self.pixels * 4
    }

    /// What the finishing step allocates while the decoded buffer is still
    /// alive: a shrunk or reoriented copy. Without either, the decoded buffer
    /// is the result and nothing more is allocated.
    pub(super) fn finish_bytes(&self, oriented: bool) -> u64 {
        if self.shrinks() || oriented {
            self.output_bytes()
        } else {
            0
        }
    }
}

/// The largest size with the aspect ratio of `width x height` that fits the
/// maximum; never larger than the source and never empty.
pub(super) fn fit_within(width: u32, height: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    let (max_w, max_h) = (max_w.max(1), max_h.max(1));
    if width <= max_w && height <= max_h {
        return (width, height);
    }
    // Exact integer comparison of max_w/width and max_h/height.
    if u64::from(max_w) * u64::from(height) <= u64::from(max_h) * u64::from(width) {
        let h = (u64::from(height) * u64::from(max_w) + u64::from(width) / 2) / u64::from(width);
        (max_w, (h as u32).clamp(1, max_h))
    } else {
        let w = (u64::from(width) * u64::from(max_h) + u64::from(height) / 2) / u64::from(height);
        ((w as u32).clamp(1, max_w), max_h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_keeps_aspect_and_never_enlarges() {
        assert_eq!(fit_within(400, 200, 100, 100), (100, 50));
        assert_eq!(fit_within(200, 400, 100, 100), (50, 100));
        assert_eq!(fit_within(40, 20, 100, 100), (40, 20));
        assert_eq!(fit_within(1000, 1, 10, 10), (10, 1));
        assert_eq!(fit_within(10, 10, 0, 0), (1, 1));
    }

    #[test]
    fn the_control_reports_what_stopped_the_decode() {
        assert_eq!(DecodeControl::new().check(), Ok(()));
        let past = DecodeControl::new().with_deadline(Instant::now());
        assert_eq!(past.check(), Err(DecodeError::TimedOut));
        let flag = Arc::new(AtomicBool::new(false));
        let control = DecodeControl::new().with_cancel(flag.clone());
        assert_eq!(control.check(), Ok(()));
        flag.store(true, Ordering::Relaxed);
        assert_eq!(control.check(), Err(DecodeError::Cancelled));
    }

    #[test]
    fn checkpoints_read_the_clock_at_the_interval_only() {
        let limits = DecodeLimits::default();
        let control = DecodeControl::new().with_deadline(Instant::now());
        let ctx = Ctx::new(&limits, &DecodeParams::default(), &control);
        // 4096 bytes per row: every 16th row is a checkpoint.
        assert_eq!(ctx.checkpoint(0, 4096), Err(DecodeError::TimedOut));
        assert_eq!(ctx.checkpoint(1, 4096), Ok(()));
        assert_eq!(ctx.checkpoint(16, 4096), Err(DecodeError::TimedOut));
    }
}
