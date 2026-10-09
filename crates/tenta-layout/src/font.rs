//! Font registration, family resolution and font metrics.

use std::sync::Arc;

use tenta_css::cascade::FontMetrics;

/// Owns the font database and shaping contexts of one page thread. Layout
/// reaches fonts only through this type.
#[derive(Debug)]
pub struct FontSystem {
    _private: (),
}

impl FontSystem {
    /// A system over the bundled fonts. Never enumerates system fonts.
    #[cfg(feature = "embedded-fonts")]
    pub fn new_embedded() -> Self {
        Self { _private: () }
    }

    /// The `ex`/`ch` metrics source the style engine needs.
    pub fn metrics(&self) -> Arc<dyn FontMetrics> {
        todo!("T1a")
    }
}
