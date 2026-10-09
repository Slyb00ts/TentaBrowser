//! Fallible pixel buffers.
//!
//! Decoders write straight into the `Arc<[u8]>` the finished image is shared
//! as, so the largest buffer of a decode is never copied. Allocation failure is
//! reported as a typed error instead of aborting the process.

use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use super::error::DecodeError;

/// A zero-initialised, uniquely owned RGBA buffer that becomes an `Arc<[u8]>`
/// without a copy.
pub(super) struct Pixels(Arc<[u8]>);

impl Pixels {
    pub(super) fn zeroed(len: usize) -> Result<Self, DecodeError> {
        // `Arc` has no fallible constructor, so probe the allocator with one
        // that has. This is best effort: the probe is freed before the real
        // allocation, so under memory pressure that allocation can still abort.
        // The admission estimates are what keep it small.
        let mut probe = Vec::<u8>::new();
        probe
            .try_reserve_exact(len)
            .map_err(|_| DecodeError::OutOfMemory)?;
        drop(probe);
        let zeroed = Arc::<[u8]>::new_zeroed_slice(len);
        // SAFETY: `new_zeroed_slice` returns `len` bytes that are all zero, and
        // zero is a valid `u8`, so every element is initialised. Zeroed pages
        // also stay untouched until a decoder writes them, so an image that
        // fails early costs address space, not resident memory.
        Ok(Self(unsafe { zeroed.assume_init() }))
    }

    pub(super) fn into_shared(self) -> Arc<[u8]> {
        self.0
    }
}

impl Deref for Pixels {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.0
    }
}

impl DerefMut for Pixels {
    fn deref_mut(&mut self) -> &mut [u8] {
        Arc::get_mut(&mut self.0).expect("a Pixels buffer is never shared before into_shared")
    }
}

/// A vector of `len` copies of `value`, or `OutOfMemory`.
pub(super) fn filled<T: Clone>(len: usize, value: T) -> Result<Vec<T>, DecodeError> {
    let mut v = Vec::new();
    v.try_reserve_exact(len)
        .map_err(|_| DecodeError::OutOfMemory)?;
    v.resize(len, value);
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_buffer_is_zero_and_exactly_sized() {
        for len in [0, 1, 4097, 1 << 20] {
            let pixels = Pixels::zeroed(len).expect("allocates");
            assert_eq!(pixels.len(), len);
            assert!(pixels.iter().all(|&b| b == 0));
        }
    }

    #[test]
    fn a_buffer_is_writable_until_it_is_shared() {
        let mut pixels = Pixels::zeroed(8).expect("allocates");
        pixels[3] = 9;
        pixels[4..].copy_from_slice(&[1, 2, 3, 4]);
        let shared = pixels.into_shared();
        assert_eq!(&shared[..], &[0, 0, 0, 9, 1, 2, 3, 4]);
    }

    #[test]
    fn an_impossible_size_is_a_typed_error() {
        assert!(matches!(
            Pixels::zeroed(usize::MAX / 2),
            Err(DecodeError::OutOfMemory)
        ));
        assert!(matches!(
            filled(usize::MAX / 8, 0u64),
            Err(DecodeError::OutOfMemory)
        ));
    }
}
