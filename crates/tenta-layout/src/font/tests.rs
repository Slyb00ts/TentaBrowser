use std::time::{Duration, Instant};

use parley::fontique::{
    Attributes, FallbackKey, FontStyle, FontWeight, GenericFamily, QueryFamily, QueryStatus, Script,
};
use skrifa::instance::{LocationRef, Size};
use skrifa::metrics::{GlyphMetrics, Metrics};
use skrifa::{FontRef, MetadataProvider};

use super::{FontSystem, ResolvedFont};
use crate::embedded_fonts::EMBEDDED_FONTS;

fn skrifa_font(file_name: &str) -> FontRef<'static> {
    let font = EMBEDDED_FONTS
        .iter()
        .find(|font| font.file_name == file_name)
        .expect("bundled font");
    FontRef::new(font.bytes).expect("bundled font parses")
}

fn file_of(font: &ResolvedFont) -> &'static str {
    EMBEDDED_FONTS
        .iter()
        .find(|embedded| std::ptr::eq(embedded.bytes, font.bytes()))
        .expect("resolved bytes come from the bundle")
        .file_name
}

#[test]
fn weight_and_style_table() {
    // (family list, weight, italic, file, synthetic bold, synthetic oblique)
    let table: &[(&str, u16, bool, &str, bool, bool)] = &[
        (
            "Arial",
            100,
            false,
            "LiberationSans-Regular.ttf",
            false,
            false,
        ),
        (
            "Arial",
            300,
            false,
            "LiberationSans-Regular.ttf",
            false,
            false,
        ),
        (
            "Arial",
            400,
            false,
            "LiberationSans-Regular.ttf",
            false,
            false,
        ),
        (
            "Arial",
            500,
            false,
            "LiberationSans-Regular.ttf",
            false,
            false,
        ),
        ("Arial", 501, false, "LiberationSans-Bold.ttf", false, false),
        ("Arial", 600, false, "LiberationSans-Bold.ttf", false, false),
        ("Arial", 700, false, "LiberationSans-Bold.ttf", false, false),
        ("Arial", 900, false, "LiberationSans-Bold.ttf", false, false),
        (
            "Arial",
            400,
            true,
            "LiberationSans-Regular.ttf",
            false,
            true,
        ),
        ("Arial", 700, true, "LiberationSans-Bold.ttf", false, true),
        (
            "serif",
            700,
            false,
            "LiberationSerif-Bold.ttf",
            false,
            false,
        ),
        (
            "monospace",
            600,
            true,
            "LiberationMono-Bold.ttf",
            false,
            true,
        ),
        // DejaVu Sans has no bold face: bold is synthesised from 600 only.
        ("DejaVu Sans", 400, false, "DejaVuSans.ttf", false, false),
        ("DejaVu Sans", 500, false, "DejaVuSans.ttf", false, false),
        ("DejaVu Sans", 600, false, "DejaVuSans.ttf", true, false),
        ("Verdana", 700, true, "DejaVuSans.ttf", true, true),
    ];
    for &(list, weight, italic, file, bold, oblique) in table {
        let font = ResolvedFont::resolve(list, weight, italic);
        let case = format!("{list} {weight} italic={italic}");
        assert_eq!(file_of(&font), file, "{case}");
        assert_eq!(font.synthetic_bold(), bold, "{case}");
        assert_eq!(font.synthetic_oblique(), oblique, "{case}");
    }
}

#[test]
fn database_holds_only_the_bundled_families() {
    let mut fonts = FontSystem::new_embedded();
    let (font_cx, _) = fonts.parley_contexts();
    let mut names: Vec<String> = font_cx
        .collection
        .family_names()
        .map(str::to_owned)
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "DejaVu Sans",
            "Liberation Mono",
            "Liberation Sans",
            "Liberation Serif"
        ]
    );
}

/// The first family of a parley query for a generic, with the synthesis
/// parley would apply at the weight and style `font` asks for.
fn query_first(
    fonts: &mut FontSystem,
    family: QueryFamily<'static>,
    font: &ResolvedFont,
) -> (String, bool, bool) {
    let (font_cx, _) = fonts.parley_contexts();
    let mut query = font_cx.collection.query(&mut font_cx.source_cache);
    query.set_families([family]);
    query.set_attributes(Attributes {
        weight: FontWeight::new(font.parley_weight()),
        style: if font.synthetic_oblique() {
            FontStyle::Italic
        } else {
            FontStyle::Normal
        },
        ..Attributes::default()
    });
    let mut found = None;
    query.matches_with(|candidate| {
        found = Some((
            candidate.family.0,
            candidate.synthesis.embolden(),
            candidate.synthesis.skew().is_some(),
        ));
        QueryStatus::Stop
    });
    drop(query);
    let (id, embolden, skew) = found.expect("a face matches");
    let name = font_cx
        .collection
        .family_name(id)
        .expect("registered family")
        .to_owned();
    (name, embolden, skew)
}

#[test]
fn parley_generics_and_synthesis_agree_with_the_resolver() {
    let mut fonts = FontSystem::new_embedded();
    for (generic, list) in [
        (GenericFamily::Serif, "serif"),
        (GenericFamily::SansSerif, "sans-serif"),
        (GenericFamily::Monospace, "monospace"),
        (GenericFamily::SystemUi, "system-ui"),
    ] {
        for (weight, italic) in [(400, false), (500, true), (700, false)] {
            let font = ResolvedFont::resolve(list, weight, italic);
            let (name, embolden, skew) = query_first(&mut fonts, generic.into(), &font);
            assert_eq!(name, font.family_name(), "{list}");
            assert_eq!(embolden, font.synthetic_bold(), "{list} {weight}");
            assert_eq!(skew, font.synthetic_oblique(), "{list} {italic}");
        }
    }
    for (weight, bold) in [(400, false), (500, false), (600, true), (700, true)] {
        let font = ResolvedFont::resolve("Verdana", weight, false);
        let (name, embolden, _) = query_first(&mut fonts, QueryFamily::Named("DejaVu Sans"), &font);
        assert_eq!(name, "DejaVu Sans");
        assert_eq!(embolden, bold, "DejaVu Sans {weight}");
    }
}

#[test]
fn missing_glyphs_fall_back_to_dejavu_sans() {
    // Liberation Sans has no snowman; DejaVu Sans does.
    let snowman = '\u{2603}';
    assert!(
        skrifa_font("LiberationSans-Regular.ttf")
            .charmap()
            .map(snowman)
            .is_none()
    );
    assert!(
        skrifa_font("DejaVuSans.ttf")
            .charmap()
            .map(snowman)
            .is_some()
    );

    let mut fonts = FontSystem::new_embedded();
    let stack = fonts.family_stack("Arial, sans-serif");
    assert_eq!(stack, ["Liberation Sans", "DejaVu Sans"]);

    let (font_cx, _) = fonts.parley_contexts();
    let mut query = font_cx.collection.query(&mut font_cx.source_cache);
    query.set_families(stack.iter().map(|name| QueryFamily::Named(name)));
    query.set_fallbacks(FallbackKey::new(Script::from_bytes(*b"Zyyy"), None));
    let mut covering = None;
    query.matches_with(|candidate| {
        if candidate
            .charmap()
            .is_some_and(|map| map.map(snowman).is_some())
        {
            covering = Some(candidate.family.0);
            QueryStatus::Stop
        } else {
            QueryStatus::Continue
        }
    });
    drop(query);
    let id = covering.expect("some face covers the snowman");
    assert_eq!(font_cx.collection.family_name(id), Some("DejaVu Sans"));
}

#[test]
fn family_stack_keeps_list_order_and_ends_with_dejavu() {
    let fonts = FontSystem::new_embedded();
    assert_eq!(
        fonts.family_stack("Nope, \"Courier New\", Times, monospace"),
        ["Liberation Mono", "Liberation Serif", "DejaVu Sans"]
    );
    assert_eq!(
        fonts.family_stack("Verdana, Arial"),
        ["DejaVu Sans", "Liberation Sans"]
    );
    assert_eq!(
        fonts.family_stack("nothing"),
        ["Liberation Serif", "DejaVu Sans"]
    );
}

#[test]
fn metrics_equal_the_values_in_the_font_tables() {
    let fonts = FontSystem::new_embedded();
    let metrics = fonts.metrics();
    let cases = [
        ("Arial", 400, "LiberationSans-Regular.ttf", 16.0),
        ("Helvetica", 700, "LiberationSans-Bold.ttf", 13.0),
        ("Times New Roman", 400, "LiberationSerif-Regular.ttf", 24.0),
        ("Courier New", 400, "LiberationMono-Regular.ttf", 10.0),
        ("DejaVu Sans", 400, "DejaVuSans.ttf", 12.5),
    ];
    for (list, weight, file, size) in cases {
        let data = metrics.metrics(list, weight, false, size);
        let font = skrifa_font(file);
        let expected = Metrics::new(&font, Size::new(size), LocationRef::default());
        let glyphs = GlyphMetrics::new(&font, Size::new(size), LocationRef::default());
        let zero = glyphs
            .advance_width(font.charmap().map('0').expect("zero"))
            .expect("zero advance");
        // DejaVu's OS/2 table predates sxHeight and sCapHeight, so those two
        // come from the outline bounds of `x` and `H`.
        let top = |ch: char| {
            glyphs
                .bounds(font.charmap().map(ch).expect("glyph"))
                .expect("bounds")
                .y_max
        };
        assert_eq!(
            data.x_height,
            Some(expected.x_height.unwrap_or_else(|| top('x'))),
            "{file} x-height"
        );
        assert_eq!(
            data.cap_height,
            Some(expected.cap_height.unwrap_or_else(|| top('H'))),
            "{file} cap height"
        );
        assert_eq!(data.zero_advance, Some(zero), "{file} ch");
        assert_eq!(data.ascent, expected.ascent, "{file} ascent");
        assert_eq!(data.ic_width, None, "{file} has no water ideograph");
        assert!(data.x_height.expect("x-height") > 0.0);
        assert!(data.cap_height.expect("cap height") > data.x_height.expect("x-height"));
    }
}

#[test]
fn metrics_use_the_resolved_face_and_known_values() {
    let fonts = FontSystem::new_embedded();
    let metrics = fonts.metrics();
    // Liberation Mono is 1229/2048 em wide per glyph.
    let mono = metrics.metrics("\"Courier New\", monospace", 400, false, 2048.0);
    assert_eq!(mono.zero_advance, Some(1229.0));
    let sans = metrics.metrics("Arial", 400, false, 2048.0);
    assert_eq!(sans.zero_advance, Some(1139.0));
    assert_eq!(sans.ascent, 1854.0);
    // Unknown names resolve to the standard font, so metrics never fail.
    let unknown = metrics.metrics("No Such Family", 400, false, 2048.0);
    let serif = metrics.metrics("serif", 400, false, 2048.0);
    assert_eq!(unknown, serif);
}

#[test]
fn degenerate_sizes_yield_empty_metrics() {
    let font = ResolvedFont::resolve("serif", 400, false);
    for size in [0.0, -4.0, f32::NAN, f32::INFINITY] {
        assert_eq!(font.metrics_data(size), Default::default(), "{size}");
        assert_eq!(font.vertical_metrics(size), Default::default(), "{size}");
    }
}

#[test]
fn normal_line_height_rounds_ascent_and_descent_separately() {
    // 16px values match Blink's `line-height: normal` for these faces.
    let cases = [
        ("Arial", 14.0, 3.0, 18.0),
        ("Times New Roman", 14.0, 3.0, 18.0),
        ("Courier New", 13.0, 5.0, 18.0),
    ];
    for (list, ascent, descent, spacing) in cases {
        let vertical = ResolvedFont::resolve(list, 400, false).vertical_metrics(16.0);
        assert_eq!(vertical.ascent, ascent, "{list}");
        assert_eq!(vertical.descent, descent, "{list}");
        assert_eq!(vertical.line_spacing(), spacing, "{list}");
    }
}

#[test]
fn two_systems_are_independent_but_resolve_identically() {
    let mut first = FontSystem::new_embedded();
    let mut second = FontSystem::new_embedded();
    let count =
        |fonts: &mut FontSystem| fonts.parley_contexts().0.collection.family_names().count();
    assert_eq!(count(&mut first), count(&mut second));
    assert_eq!(
        first.resolve("Arial", 700, true),
        second.resolve("Arial", 700, true)
    );
}

#[test]
fn initialisation_is_far_below_a_generous_bound() {
    // nextest runs each test in its own process, so the first system pays the
    // one-time registration. The bound only catches accidental eager parsing
    // or I/O; the precise figure belongs to the benchmark harness.
    let start = Instant::now();
    let mut cold = FontSystem::new_embedded();
    let cold_time = start.elapsed();
    let start = Instant::now();
    let _second = FontSystem::new_embedded();
    let second_time = start.elapsed();
    assert!(
        cold_time < Duration::from_millis(50),
        "cold init {cold_time:?}"
    );
    assert!(
        second_time < cold_time,
        "second {second_time:?}, cold {cold_time:?}"
    );
    assert_eq!(
        cold.parley_contexts().0.collection.family_names().count(),
        4
    );
}
