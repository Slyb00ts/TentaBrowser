//! Stylo `Device` construction and the font-metrics provider.

use std::fmt;
use std::sync::Arc;

use style::context::QuirksMode;
use style::device::Device;
use style::device::servo::FontMetricsProvider;
use style::font_metrics::FontMetrics as StyloFontMetrics;
use style::media_queries::MediaType;
use style::properties::ComputedValues;
use style::properties::style_structs::Font;
use style::queries::values::PrefersColorScheme;
use style::servo::media_features::PointerCapabilities;
use style::values::computed::font::{FontStyle, GenericFontFamily};
use style::values::computed::{CSSPixelLength, Length};
use style::values::specified::font::QueryFontMetricsFlags;
use style_traits::{CSSPixel, DevicePixel, ToCss};

use super::types::{FontMetrics, StyleEnv};

/// Size of `medium` for every generic family but `monospace`, in CSS pixels.
const DEFAULT_FONT_SIZE_PX: f32 = 16.0;

/// Size of `medium` while the family is `monospace`, in CSS pixels.
const DEFAULT_MONOSPACE_SIZE_PX: f32 = 13.0;

/// A Stylo [`Device`] for `env`.
///
/// The initial `font-family` is the `serif` generic and `medium` is 16px, both
/// Stylo's own initial values; the Servo flavour has no minimum font size and
/// no text zoom, so there is nothing to configure for those. `ex`, `ch`, `cap`
/// and `ic` are answered by `metrics`. Reduced motion is not a media feature of
/// this flavour, so `env.reduced_motion` changes nothing here.
pub(super) fn new_device(env: &StyleEnv, metrics: &Arc<dyn FontMetrics>) -> Device {
    let (width, height) = env.viewport;
    let ratio = env.device_pixel_ratio;
    let media_type = if env.print {
        MediaType::print()
    } else {
        MediaType::screen()
    };
    let quirks = if env.quirks {
        QuirksMode::Quirks
    } else {
        QuirksMode::NoQuirks
    };
    let color_scheme = if env.dark {
        PrefersColorScheme::Dark
    } else {
        PrefersColorScheme::Light
    };
    // The reference browser reports a mouse: fine pointer that can hover.
    let pointer = PointerCapabilities::FINE | PointerCapabilities::HOVER;
    Device::new(
        media_type,
        quirks,
        euclid::Size2D::<f32, CSSPixel>::new(width, height),
        euclid::Size2D::<f32, DevicePixel>::new(width * ratio, height * ratio),
        euclid::Scale::new(ratio),
        Box::new(MetricsBridge(Arc::clone(metrics))),
        ComputedValues::initial_values_with_font_override(Font::initial_values()),
        color_scheme,
        pointer,
        pointer,
    )
}

/// Answers Stylo's font queries from the engine's [`FontMetrics`].
struct MetricsBridge(Arc<dyn FontMetrics>);

impl fmt::Debug for MetricsBridge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MetricsBridge").finish_non_exhaustive()
    }
}

impl FontMetricsProvider for MetricsBridge {
    // `vertical` is ignored: the trait carries no vertical measurements, and
    // Stylo falls back to its own ratios for the upright `ch`.
    fn query_font_metrics(
        &self,
        _vertical: bool,
        font: &Font,
        base_size: CSSPixelLength,
        _flags: QueryFontMetricsFlags,
    ) -> StyloFontMetrics {
        let weight = font.font_weight.value().clamp(1.0, 1000.0).round() as u16;
        let italic = font.font_style != FontStyle::NORMAL;
        let data = self.0.metrics(
            &font.font_family.to_css_string(),
            weight,
            italic,
            base_size.px(),
        );
        StyloFontMetrics {
            x_height: data.x_height.map(Length::new),
            zero_advance_measure: data.zero_advance.map(Length::new),
            cap_height: data.cap_height.map(Length::new),
            ic_width: data.ic_width.map(Length::new),
            ascent: Length::new(data.ascent),
            script_percent_scale_down: data.script_percent_scale_down,
            script_script_percent_scale_down: data.script_script_percent_scale_down,
        }
    }

    fn base_size_for_generic(&self, generic: GenericFontFamily) -> Length {
        Length::new(match generic {
            GenericFontFamily::Monospace => DEFAULT_MONOSPACE_SIZE_PX,
            _ => DEFAULT_FONT_SIZE_PX,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::cascade::types::FontMetricsData;

    /// Records the last query and answers with distinct values per field.
    #[derive(Default)]
    struct Recorder(Mutex<Vec<(String, u16, bool, f32)>>);

    impl FontMetrics for Recorder {
        fn metrics(&self, list: &str, weight: u16, italic: bool, size: f32) -> FontMetricsData {
            self.0
                .lock()
                .unwrap()
                .push((list.to_owned(), weight, italic, size));
            FontMetricsData {
                x_height: Some(size * 0.25),
                zero_advance: None,
                cap_height: Some(size * 0.75),
                ic_width: None,
                ascent: size * 0.5,
                script_percent_scale_down: Some(0.7),
                script_script_percent_scale_down: None,
            }
        }
    }

    #[test]
    fn the_device_follows_the_environment() {
        let metrics: Arc<dyn FontMetrics> = Arc::new(Recorder::default());
        let env = StyleEnv {
            viewport: (320.0, 480.0),
            device_pixel_ratio: 2.0,
            dark: true,
            print: true,
            quirks: true,
            ..StyleEnv::default()
        };
        let device = new_device(&env, &metrics);
        assert_eq!(device.viewport_size().width, 320.0);
        assert_eq!(device.viewport_size().height, 480.0);
        assert_eq!(device.device_size().width, 640.0);
        assert_eq!(device.device_pixel_ratio().get(), 2.0);
        assert_eq!(device.color_scheme(), PrefersColorScheme::Dark);
        assert_eq!(device.media_type(), MediaType::print());
        assert_eq!(device.quirks_mode(), QuirksMode::Quirks);

        let screen = new_device(&StyleEnv::default(), &metrics);
        assert_eq!(screen.media_type(), MediaType::screen());
        assert_eq!(screen.color_scheme(), PrefersColorScheme::Light);
        assert_eq!(screen.quirks_mode(), QuirksMode::NoQuirks);
    }

    #[test]
    fn generic_families_start_at_sixteen_pixels_except_monospace() {
        let metrics: Arc<dyn FontMetrics> = Arc::new(Recorder::default());
        let device = new_device(&StyleEnv::default(), &metrics);
        assert_eq!(
            device.base_size_for_generic(GenericFontFamily::Serif).px(),
            16.0
        );
        assert_eq!(
            device
                .base_size_for_generic(GenericFontFamily::Monospace)
                .px(),
            13.0
        );
    }

    #[test]
    fn font_queries_reach_the_metrics_source_and_come_back_converted() {
        let recorder = Arc::new(Recorder::default());
        let metrics: Arc<dyn FontMetrics> = recorder.clone();
        let device = new_device(&StyleEnv::default(), &metrics);
        let mut font = Font::initial_values();
        font.font_style = FontStyle::ITALIC;
        let answer = device.query_font_metrics(
            false,
            &font,
            CSSPixelLength::new(20.0),
            QueryFontMetricsFlags::empty(),
            false,
        );
        assert_eq!(answer.x_height.map(|l| l.px()), Some(5.0));
        assert_eq!(answer.zero_advance_measure, None);
        assert_eq!(answer.cap_height.map(|l| l.px()), Some(15.0));
        assert_eq!(answer.ascent.px(), 10.0);
        assert_eq!(answer.script_percent_scale_down, Some(0.7));
        assert_eq!(
            recorder.0.lock().unwrap().as_slice(),
            [("serif".to_owned(), 400, true, 20.0)],
            "the initial family list is the serif generic"
        );
    }
}
