//! `calc()` crossing into Taffy as an opaque handle.
//!
//! Taffy's `CompactLength` can carry a pointer in place of a number and calls
//! back with that pointer and the percentage basis when it needs pixels. The
//! handle here is the address of the `CalcLengthPercentage` inside the computed
//! style, so it is valid exactly as long as that style is alive.

use style::values::computed::Length;
use style::values::computed::length_percentage::CalcLengthPercentage;

/// A Taffy length that resolves through [`resolve`].
pub(super) fn length_percentage(calc: &CalcLengthPercentage) -> taffy::LengthPercentage {
    let handle = taffy::CompactLength::calc(std::ptr::from_ref(calc).cast());
    // SAFETY: `CompactLength::calc` yields the calc-tagged variant, which is one of
    // the variants `LengthPercentage` accepts (a calc is a length-percentage). The
    // address stays valid because Stylo keeps the `CalcLengthPercentage` in its own
    // heap allocation owned by the `LengthPercentage` inside the computed-values
    // `Arc`, so it does not move while those values live; `ConvertedStyle` borrows
    // them.
    unsafe { taffy::LengthPercentage::from_raw(handle) }
}

/// Resolves a handle made by [`length_percentage`] against `basis` pixels.
///
/// # Safety
///
/// The only place a handle is dereferenced. `handle` must come from [`length_percentage`], and the computed style it
/// was made from must still be alive.
pub(crate) unsafe fn resolve(handle: *const (), basis: f32) -> f32 {
    // SAFETY: the caller guarantees the handle points at a live
    // `CalcLengthPercentage`.
    let calc = unsafe { &*handle.cast::<CalcLengthPercentage>() };
    calc.resolve(Length::new(basis)).px()
}
