//! Viewport and render configuration.

/// The layout viewport and its device scale.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Viewport {
    /// Width in CSS pixels.
    pub width: u32,
    /// Height in CSS pixels.
    pub height: u32,
    /// Device pixels per CSS pixel.
    pub device_pixel_ratio: f32,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 800,
            device_pixel_ratio: 1.0,
        }
    }
}

/// Where fonts come from.
#[derive(Copy, Clone, Default, Debug, PartialEq, Eq)]
pub enum FontPolicy {
    /// Only the fonts bundled with the engine; deterministic output.
    #[default]
    Embedded,
    /// The bundled fonts, plus the platform font database for scripts they
    /// lack (CJK, Thai, ...). Output depends on the installed fonts (D-036).
    SystemFallback,
}

/// Configuration of a [`crate::RenderSession`].
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct RenderConfig {
    /// Initial viewport.
    pub viewport: Viewport,
    /// Font source.
    pub fonts: FontPolicy,
    /// `prefers-color-scheme: dark`.
    pub dark: bool,
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            viewport: Viewport::default(),
            fonts: FontPolicy::Embedded,
            dark: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_screenshot_defaults() {
        let cfg = RenderConfig::default();
        assert_eq!((cfg.viewport.width, cfg.viewport.height), (1280, 800));
        assert_eq!(cfg.viewport.device_pixel_ratio, 1.0);
        assert_eq!(cfg.fonts, FontPolicy::Embedded);
        assert!(!cfg.dark);
    }
}
