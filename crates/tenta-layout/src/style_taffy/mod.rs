//! Conversion from Stylo computed values to `taffy` styles for flex and grid.
//!
//! Taffy runs only the flex and grid algorithms; block, inline, float and table
//! layout stay in this crate, and `display: contents` boxes never reach Taffy
//! because the box tree promotes their children. [`container_kind`] tells a
//! caller which algorithm owns a box, and [`to_taffy_style`] converts the rest.
//!
//! `calc()` lengths cross the boundary as a pointer into the computed style
//! ([`calc::resolve`] turns one back into pixels). [`convert::ConvertedStyle`]
//! borrows the `ComputedValues`, so a converted style cannot outlive them.

mod calc;
mod convert;
mod grid;
#[cfg(test)]
mod tests;

pub(crate) use calc::resolve as resolve_calc;
pub(crate) use convert::{
    ConvertedStyle, block_alignment_is_normal, preferred_aspect_ratio, to_taffy_style,
};
