//! Shaping, line breaking and text measurement over parley.
//!
//! Inline layout hands a paragraph (the white-space processed text, the style of
//! each range and the base direction) to [`shape::shape_and_break`] for lines and
//! glyph runs, or to [`shape::measure`] for its min-content and max-content
//! widths. Nothing here reads the DOM or computed styles; [`style::TextStyle`]
//! carries the values.
//!
//! Output is a pure function of the input and the bundled fonts: no system font
//! is enumerated, nothing depends on iteration order or time.

#![allow(
    dead_code,
    reason = "inline layout is the only caller and has not been written against this module yet"
)]

#[cfg(feature = "embedded-fonts")]
pub(crate) mod cache;
#[cfg(feature = "embedded-fonts")]
pub(crate) mod shape;
#[cfg(feature = "embedded-fonts")]
pub(crate) mod shaped;
#[cfg(feature = "embedded-fonts")]
pub(crate) mod style;
#[cfg(feature = "embedded-fonts")]
pub(crate) mod transform;

#[cfg(all(test, feature = "embedded-fonts"))]
mod tests;
