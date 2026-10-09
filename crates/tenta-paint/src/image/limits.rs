//! Decode limits and per-request decode parameters.

use tenta_core::Budgets;

const MIB: usize = 1024 * 1024;

/// Hard bounds on one decode. Header-declared dimensions are checked before any
/// pixel buffer is allocated.
///
/// Two budgets apply. `max_decoded_bytes` bounds what the decode **retains**:
/// the finished (post-shrink) image, which is what the cache holds.
/// `max_transient_bytes` bounds the **peak heap while decoding**: each decoder
/// estimates the buffers it holds at its largest (its own scratch planes, the
/// full-size RGBA image, the shrunk or reoriented copy) and refuses an image
/// whose estimate exceeds it. A large photo can therefore be decoded and shrunk
/// to fit the cache without the cache budget having to cover its scratch memory.
/// `max_pixels` is a separate ceiling on the source size alone.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct DecodeLimits {
    /// Largest source image, in pixels.
    pub max_pixels: u64,
    /// Largest retained (post-downscale) image, in bytes of RGBA8.
    pub max_decoded_bytes: usize,
    /// Largest peak decode memory, in bytes; at least `max_decoded_bytes` in
    /// any sensible configuration.
    pub max_transient_bytes: usize,
}

impl DecodeLimits {
    /// Limits derived from the context budget: the retained image may fill the
    /// per-context image cache, and a decode may transiently use twice that on
    /// desktop and one and a half times on mobile. A source is at most as many
    /// pixels as fit the transient budget as RGBA8, the least any format needs.
    pub fn from_budgets(budgets: &Budgets) -> Self {
        let max_decoded_bytes = budgets.context_memory_mb as usize * MIB;
        let mobile = budgets.context_memory_mb <= Budgets::mobile().context_memory_mb;
        let max_transient_bytes = if mobile {
            max_decoded_bytes + max_decoded_bytes / 2
        } else {
            max_decoded_bytes * 2
        };
        Self {
            max_pixels: (max_transient_bytes / 4) as u64,
            max_decoded_bytes,
            max_transient_bytes,
        }
    }
}

impl Default for DecodeLimits {
    fn default() -> Self {
        Self::from_budgets(&Budgets::platform_default())
    }
}

/// What layout needs from a decode; part of the [`ImageStore`](super::ImageStore) key.
#[derive(Copy, Clone, Default, PartialEq, Eq, Hash, Debug)]
pub struct DecodeParams {
    /// Largest size, in device pixels, the image will be drawn at. The decoder
    /// shrinks the image to fit, keeping the aspect ratio, and never enlarges.
    pub max_size: Option<(u32, u32)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_transient_budget_is_twice_the_cache_on_desktop_and_one_and_a_half_on_mobile() {
        let desktop = DecodeLimits::from_budgets(&Budgets::desktop());
        assert_eq!(desktop.max_decoded_bytes, 64 * MIB);
        assert_eq!(desktop.max_transient_bytes, 128 * MIB);
        assert_eq!(desktop.max_pixels, 32 * 1024 * 1024);
        let mobile = DecodeLimits::from_budgets(&Budgets::mobile());
        assert_eq!(mobile.max_decoded_bytes, 24 * MIB);
        assert_eq!(mobile.max_transient_bytes, 36 * MIB);
    }
}
