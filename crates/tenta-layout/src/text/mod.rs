//! Shaping, line breaking and text measurement over parley.
//!
//! Inline layout hands a paragraph (the white-space processed text, the style of
//! each range and the base direction) to [`shape::shape_chunk`], which returns it
//! shaped as one line together with its wrap opportunities. Nothing here reads the DOM or computed styles; [`style::TextStyle`]
//! carries the values.
//!
//! Output is a pure function of the input and the bundled fonts: no system font
//! is enumerated, nothing depends on iteration order or time.

#[cfg(feature = "embedded-fonts")]
pub(crate) mod chunk;
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
