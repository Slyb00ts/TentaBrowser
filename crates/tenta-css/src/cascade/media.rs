//! `window.matchMedia`: a media query list evaluated against the page's
//! environment by Stylo, the same evaluation `@media` rules get.

use std::sync::Arc;

use cssparser::{Parser, ParserInput};
use style::context::QuirksMode;
use style::media_queries::MediaList;
use style::parser::ParserContext;
use style::stylesheets::{CssRuleType, CustomMediaEvaluator, Origin, UrlExtraData};
use style_traits::{ParsingMode, ToCss};
use url::Url;

use super::device::new_device;
use super::types::{FontMetrics, FontMetricsData, StyleEnv};

/// What `matchMedia(query)` reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaQueryMatch {
    /// The serialised media query list, which is `MediaQueryList.media`: a
    /// query that does not parse is `not all`.
    pub media: String,
    pub matches: bool,
}

/// Font measurements for the only use a media query has for them: an `em`,
/// `ex` or `ch` length is relative to the initial font, whose size is all
/// that matters here, so every face has the proportions of a typical one.
struct InitialFontMetrics;

impl FontMetrics for InitialFontMetrics {
    fn metrics(&self, _: &str, _: u16, _: bool, size_px: f32) -> FontMetricsData {
        FontMetricsData {
            x_height: Some(size_px * 0.5),
            zero_advance: Some(size_px * 0.5),
            cap_height: Some(size_px * 0.7),
            ic_width: Some(size_px),
            ascent: size_px * 0.9,
            script_percent_scale_down: None,
            script_script_percent_scale_down: None,
        }
    }
}

/// Parses `query` as a media query list and evaluates it for `env`.
pub fn match_media(query: &str, env: &StyleEnv) -> MediaQueryMatch {
    crate::cssom::configure_stylo();
    let base = Url::parse("about:blank").expect("`about:blank` is a URL");
    let url_data = UrlExtraData::from(base);
    let mut context = ParserContext::new(
        Origin::Author,
        &url_data,
        Some(CssRuleType::Media),
        ParsingMode::DEFAULT,
        QuirksMode::NoQuirks,
        Default::default(),
        None,
        None,
        Default::default(),
    );
    let mut input = ParserInput::new(query);
    let mut parser = Parser::new(&mut input);
    let list = MediaList::parse(&mut context, &mut parser);
    let metrics: Arc<dyn FontMetrics> = Arc::new(InitialFontMetrics);
    let device = new_device(env, &metrics);
    MediaQueryMatch {
        media: list.to_css_string(),
        // A document's `@custom-media` rules are not in scope for
        // `matchMedia`, so a custom media name is unknown and does not match.
        matches: list.evaluate(
            &device,
            QuirksMode::NoQuirks,
            &mut CustomMediaEvaluator::none(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(width: f32, height: f32) -> StyleEnv {
        StyleEnv {
            viewport: (width, height),
            ..StyleEnv::default()
        }
    }

    fn matches(query: &str, env: &StyleEnv) -> bool {
        match_media(query, env).matches
    }

    #[test]
    fn width_height_and_orientation_follow_the_viewport() {
        let desktop = env(1280.0, 800.0);
        let phone = env(390.0, 844.0);
        for (query, on_desktop, on_phone) in [
            ("(min-width: 768px)", true, false),
            ("(max-width: 767.98px)", false, true),
            ("(width >= 1024px)", true, false),
            ("(400px <= width <= 1300px)", true, false),
            ("(orientation: landscape)", true, false),
            ("(orientation: portrait)", false, true),
            ("(min-width: 48em)", true, false),
            (
                "screen and (min-width: 600px) and (max-width: 1400px)",
                true,
                false,
            ),
            ("(max-width: 500px), (min-width: 1200px)", true, true),
            ("not screen", false, false),
            ("print", false, false),
            ("all", true, true),
            ("", true, true),
        ] {
            assert_eq!(matches(query, &desktop), on_desktop, "{query} on a desktop");
            assert_eq!(matches(query, &phone), on_phone, "{query} on a phone");
        }
    }

    #[test]
    fn preferences_pointer_and_resolution_come_from_the_environment() {
        let light = env(1280.0, 800.0);
        let dark = StyleEnv {
            dark: true,
            device_pixel_ratio: 2.0,
            ..light.clone()
        };
        assert!(matches("(prefers-color-scheme: light)", &light));
        assert!(!matches("(prefers-color-scheme: dark)", &light));
        assert!(matches("(prefers-color-scheme: dark)", &dark));
        assert!(matches("(hover: hover) and (pointer: fine)", &light));
        assert!(!matches("(pointer: coarse)", &light));
        assert!(matches("(min-resolution: 2dppx)", &dark));
        assert!(!matches("(min-resolution: 2dppx)", &light));
    }

    #[test]
    fn the_media_text_is_serialised_and_a_bad_query_is_not_all() {
        let screen = env(1280.0, 800.0);
        assert_eq!(
            match_media("  SCREEN   and (MIN-WIDTH:600px)  ", &screen).media,
            "screen and (min-width: 600px)"
        );
        for bad in ["screen and", "min-width: 600px", "@!"] {
            let result = match_media(bad, &screen);
            assert_eq!(result.media, "not all", "{bad}");
            assert!(!result.matches, "{bad}");
        }
        // An unclosed block is closed at the end of input, as CSS parses it.
        assert!(!match_media("(min-width: ", &screen).matches);
        let unknown = match_media("(no-such-feature: 1)", &screen);
        assert!(!unknown.matches, "an unknown feature never matches");
    }
}
