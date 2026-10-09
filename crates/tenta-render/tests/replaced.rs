//! Replaced-element geometry against Chrome: every case loads real images (SVG
//! `data:` URLs whose natural size is known) through the session's image
//! pipeline, lays the page out and compares border boxes with
//! `getBoundingClientRect` values recorded from Chrome 154 for exactly the
//! HTML next to them. Block, inline, float, flex, grid and absolutely
//! positioned replaced boxes, `width`/`height` attributes, ratio-only images and
//! broken images are covered.
//!
//! The recording recipe is `docs/dev/render-fixtures.md` (a headless run that
//! waits for every image to decode, then reads the rectangles by `id`).

#[path = "replaced/chrome_abs.rs"]
mod chrome_abs;
#[path = "replaced/chrome_block.rs"]
mod chrome_block;
#[path = "replaced/chrome_flex.rs"]
mod chrome_flex;
#[path = "replaced/chrome_grid.rs"]
mod chrome_grid;
#[path = "replaced/chrome_inline.rs"]
mod chrome_inline;
#[path = "replaced/chrome_misc.rs"]
mod chrome_misc;

use tenta_css::cascade::StyleEnv;
use tenta_css::cascade::test_util::style_html;
use tenta_render::{RenderConfig, RenderSession, Viewport};
use url::Url;

/// One recorded Chrome page: its HTML and the border boxes (`[x, y, w, h]`,
/// document coordinates) of the elements that carry an id, plus the root's
/// scroll size.
pub struct Case {
    pub name: &'static str,
    pub viewport: (f32, f32),
    pub html: &'static str,
    pub scroll: (f32, f32),
    pub expect: &'static [(&'static str, [f32; 4])],
}

/// Chrome's rounding policy for the corpus: 0.02 px for whole numbers, 0.05
/// otherwise, plus the 1/100 px the recording rounds to.
fn within(actual: f32, expected: f32) -> bool {
    let tolerance = if expected.fract() == 0.0 { 0.02 } else { 0.05 };
    (actual - expected).abs() <= tolerance
}

fn check_cases(cases: &[Case]) {
    let mut failures = Vec::new();
    for case in cases {
        let (doc, engine) = style_html(case.html, StyleEnv::default());
        let mut session = RenderSession::new(RenderConfig {
            viewport: Viewport {
                width: case.viewport.0 as u32,
                height: case.viewport.1 as u32,
                device_pixel_ratio: 1.0,
            },
            ..RenderConfig::default()
        });
        session.set_style_sources(engine.sources().to_vec());
        let base = Url::parse("https://example.test/page.html").expect("a constant URL");
        session.load_inline_images(&doc, &base);
        let frags = session.layout(&doc);
        for (id, expected) in case.expect {
            let node = doc
                .get_element_by_id(id)
                .unwrap_or_else(|| panic!("{}: no element {id}", case.name));
            let rect = frags
                .fragments_of(node)
                .first()
                .map(|&f| frags.absolute_border_box(f))
                .unwrap_or_default();
            let got = [rect.x, rect.y, rect.w, rect.h];
            if got.iter().zip(expected).any(|(a, e)| !within(*a, *e)) {
                failures.push(format!(
                    "{} / #{id}: got {got:?}, Chrome {expected:?}",
                    case.name
                ));
            }
        }
        let extent = frags.scroll_extent;
        if (extent.w, extent.h) != case.scroll {
            failures.push(format!(
                "{} / scroll extent: got {:?}, Chrome {:?}",
                case.name,
                (extent.w, extent.h),
                case.scroll
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn block_level_images_match_chrome() {
    check_cases(chrome_block::CASES);
}

#[test]
fn inline_and_floated_images_match_chrome() {
    check_cases(chrome_inline::CASES);
}

#[test]
fn flex_items_match_chrome() {
    check_cases(chrome_flex::CASES);
}

#[test]
fn grid_items_match_chrome() {
    check_cases(chrome_grid::CASES);
}

#[test]
fn absolutely_positioned_images_match_chrome() {
    check_cases(chrome_abs::CASES);
}

#[test]
fn broken_images_and_miscellany_match_chrome() {
    check_cases(chrome_misc::CASES);
}
