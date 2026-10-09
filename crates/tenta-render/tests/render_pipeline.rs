//! End-to-end renders of small documents: parse, style, lay out, paint and
//! encode, then decode the PNG and check pixels.

use std::io::Cursor;
use std::path::PathBuf;

use tenta_core::NodeId;
use tenta_core::atom::{Atom, atoms};
use tenta_css::cascade::test_util::style_html;
use tenta_css::cascade::{StyleEnv, StyleSource};
use tenta_dom::{Document, Mutation};
use tenta_html::QualName;
use tenta_layout::Rect;
use tenta_render::{
    Capture, ImageFormat, RenderConfig, RenderError, RenderSession, Screenshot, ScreenshotOptions,
    Viewport,
};

const WHITE: [u8; 4] = [255, 255, 255, 255];
const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const YELLOW: [u8; 4] = [255, 255, 0, 255];

/// A decoded screenshot.
struct Image {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl Image {
    fn decode(png_bytes: &[u8]) -> Image {
        let mut reader = png::Decoder::new(Cursor::new(png_bytes))
            .read_info()
            .expect("valid PNG header");
        let mut rgba = vec![0; reader.output_buffer_size().expect("a frame fits in memory")];
        let info = reader.next_frame(&mut rgba).expect("valid PNG frame");
        assert_eq!(info.color_type, png::ColorType::Rgba);
        rgba.truncate(info.buffer_size());
        Image {
            width: info.width,
            height: info.height,
            rgba,
        }
    }

    fn px(&self, x: u32, y: u32) -> [u8; 4] {
        assert!(x < self.width && y < self.height, "({x}, {y}) is outside");
        let at = (y * self.width + x) as usize * 4;
        self.rgba[at..at + 4].try_into().expect("four channels")
    }

    /// Pixels of exactly `color` inside the half-open box.
    fn count(&self, color: [u8; 4], x: std::ops::Range<u32>, y: std::ops::Range<u32>) -> usize {
        y.flat_map(|y| x.clone().map(move |x| (x, y)))
            .filter(|&(x, y)| self.px(x, y) == color)
            .count()
    }

    /// Pixels in the box that are not `background`.
    fn ink(&self, background: [u8; 4], x: std::ops::Range<u32>, y: std::ops::Range<u32>) -> usize {
        y.flat_map(|y| x.clone().map(move |x| (x, y)))
            .filter(|&(x, y)| self.px(x, y) != background)
            .count()
    }
}

struct Page {
    doc: Document,
    session: RenderSession,
    sources: Vec<StyleSource>,
}

fn config(width: u32, height: u32, device_pixel_ratio: f32) -> RenderConfig {
    RenderConfig {
        viewport: Viewport {
            width,
            height,
            device_pixel_ratio,
        },
        ..RenderConfig::default()
    }
}

fn page_with(html: &str, cfg: RenderConfig) -> Page {
    let (doc, engine) = style_html(html, StyleEnv::default());
    let sources = engine.sources().to_vec();
    let mut session = RenderSession::new(cfg);
    session.set_style_sources(sources.clone());
    Page {
        doc,
        session,
        sources,
    }
}

fn page(html: &str, width: u32, height: u32) -> Page {
    page_with(html, config(width, height, 1.0))
}

fn fixture(name: &str) -> String {
    let path = tenta_testutil::fixtures_root().join("render").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
}

impl Page {
    fn try_shoot(&mut self, opts: ScreenshotOptions) -> Result<Screenshot, RenderError> {
        self.session.screenshot(&self.doc, &opts)
    }

    fn shoot(&mut self, opts: ScreenshotOptions) -> (Screenshot, Image) {
        let shot = self.try_shoot(opts).expect("the screenshot succeeds");
        let image = Image::decode(&shot.bytes);
        assert_eq!((image.width, image.height), (shot.width, shot.height));
        (shot, image)
    }

    fn viewport(&mut self) -> (Screenshot, Image) {
        self.shoot(ScreenshotOptions::default())
    }

    fn node(&self, id: &str) -> NodeId {
        self.doc.get_element_by_id(id).expect("the fixture has it")
    }

    fn set_attribute(&mut self, id: &str, name: &str, value: &str) {
        let element = self.node(id);
        self.doc
            .mutate(Mutation::SetAttribute {
                element,
                name: QualName::new(atoms::EMPTY, Atom::new(name)),
                value: value.to_owned(),
            })
            .expect("set attribute");
    }

    /// Border box of the element's first fragment, in document coordinates.
    fn border_box(&mut self, id: &str) -> Rect {
        let node = self.node(id);
        let frags = self.session.layout(&self.doc);
        let first = *frags
            .fragments_of(node)
            .first()
            .expect("the element has a fragment");
        frags.absolute_border_box(first)
    }
}

fn with(capture: Capture) -> ScreenshotOptions {
    ScreenshotOptions {
        capture,
        ..ScreenshotOptions::default()
    }
}

const BOXES: &str = r#"<!doctype html><style>body{margin:0}
#r{width:100px;height:50px;background:#f00}
#g{width:40px;height:30px;margin:20px 0 0 30px;background:#0f0}</style>
<div id="r"></div><div id="g"></div>"#;

#[test]
fn blocks_paint_at_their_layout_positions() {
    let mut p = page(&fixture("solid-boxes.html"), 200, 120);
    let (shot, img) = p.viewport();
    assert_eq!((shot.width, shot.height), (200, 120));
    assert!(shot.stats.is_complete(), "{:?}", shot.stats.skipped);
    assert!(!shot.clamped);

    // #a is 100x40 red at the origin, #b 60x30 green at x=20, y=40.
    assert_eq!(img.count(RED, 0..100, 0..40), 4000);
    assert_eq!(img.px(100, 10), WHITE);
    assert_eq!(img.px(99, 39), RED);
    assert_eq!(img.px(10, 40), WHITE);
    assert_eq!(img.count(GREEN, 20..80, 40..70), 1800);
    assert_eq!(img.px(19, 50), WHITE);
    assert_eq!(img.px(80, 50), WHITE);
    // #c: a 4px blue border around a yellow 80x20 content box, 88x28 in total.
    assert_eq!(img.px(0, 70), BLUE);
    assert_eq!(img.px(3, 73), BLUE);
    assert_eq!(img.px(4, 74), YELLOW);
    assert_eq!(img.px(83, 93), YELLOW);
    assert_eq!(img.px(84, 93), BLUE);
    assert_eq!(img.px(87, 97), BLUE);
    assert_eq!(img.px(88, 97), WHITE);
    assert_eq!(img.px(0, 98), WHITE);
    assert_eq!(img.count(YELLOW, 0..200, 0..120), 80 * 20);
}

#[test]
fn the_body_keeps_its_default_margin() {
    let mut p = page(
        "<!doctype html><div id=d style='width:50px;height:20px;background:#00f'></div>",
        100,
        60,
    );
    let (_, img) = p.viewport();
    assert_eq!(img.px(7, 8), WHITE);
    assert_eq!(img.px(8, 7), WHITE);
    assert_eq!(img.px(8, 8), BLUE);
    assert_eq!(img.px(57, 27), BLUE);
    assert_eq!(img.px(58, 27), WHITE);
    assert_eq!(img.px(57, 28), WHITE);
}

#[test]
fn text_is_drawn_inside_the_body_margin() {
    let mut p = page(&fixture("text-basic.html"), 600, 60);
    let (shot, img) = p.viewport();
    assert!(shot.stats.is_complete(), "{:?}", shot.stats.skipped);
    // The paragraph's margin (16px) collapses with the body's 8px, so the
    // text starts at y=16 and x=8.
    assert_eq!(img.ink(WHITE, 0..600, 0..16), 0);
    assert_eq!(img.ink(WHITE, 0..8, 0..60), 0);
    assert!(img.ink(WHITE, 8..600, 16..36) > 150, "text rows have ink");
    assert_eq!(
        img.ink(WHITE, 0..600, 40..60),
        0,
        "the text fits on one line"
    );
    let darkest = (8..600)
        .flat_map(|x| (16..36).map(move |y| (x, y)))
        .map(|(x, y)| img.px(x, y)[0])
        .min()
        .expect("pixels");
    assert!(darkest < 60, "black text reaches near black, got {darkest}");
}

#[test]
fn list_markers_are_drawn_left_of_each_item() {
    let mut p = page(&fixture("nested-lists.html"), 200, 120);
    let (_, img) = p.viewport();
    for (id, indent) in [("one", 24.0), ("two", 48.0), ("three", 24.0)] {
        let border_box = p.border_box(id);
        assert_eq!(border_box.x, indent, "#{id} is indented by its lists");
        let rows = border_box.y as u32..(border_box.y + 18.0) as u32;
        assert!(
            img.ink(WHITE, (indent as u32 - 18)..(indent as u32 - 2), rows) > 4,
            "#{id} has a marker"
        );
    }
    assert_eq!(
        img.ink(WHITE, 0..6, 0..120),
        0,
        "nothing left of the markers"
    );
}

#[test]
fn the_body_background_fills_the_canvas() {
    let html = "<!doctype html><body style='background:#0f0'><p>x</p>";
    let mut p = page(html, 120, 90);
    let (_, img) = p.viewport();
    for (x, y) in [(0, 0), (119, 0), (0, 89), (119, 89), (60, 45)] {
        assert_eq!(img.px(x, y), GREEN, "({x}, {y})");
    }
}

#[test]
fn the_root_background_wins_over_the_body_background() {
    let html = "<!doctype html><html style='background:#ff0'><body style='background:#0f0;\
                margin:20px'><div style='height:10px'></div>";
    let mut p = page(html, 100, 60);
    let (_, img) = p.viewport();
    assert_eq!(img.px(0, 0), YELLOW);
    assert_eq!(img.px(50, 30), YELLOW);
    assert_eq!(img.px(99, 59), YELLOW);
}

#[test]
fn omit_background_leaves_the_canvas_transparent() {
    let mut p = page(BOXES, 120, 100);
    let (_, img) = p.shoot(ScreenshotOptions {
        omit_background: true,
        ..ScreenshotOptions::default()
    });
    assert_eq!(img.px(110, 80), [0, 0, 0, 0]);
    assert_eq!(img.px(10, 10), RED);

    let html = "<!doctype html><body style='background:#0f0'>";
    let mut p = page(html, 20, 20);
    let (_, img) = p.shoot(ScreenshotOptions {
        omit_background: true,
        ..ScreenshotOptions::default()
    });
    assert_eq!(img.px(10, 10), GREEN, "a page background is not omitted");
}

#[test]
fn full_page_height_is_the_document_height() {
    let html = "<!doctype html><body style='margin:0'><div style='height:3000px;\
                background:#f00'></div><div style='height:10px;background:#00f'></div>";
    let mut p = page(html, 100, 200);
    let (shot, img) = p.shoot(with(Capture::FullPage));
    assert_eq!((shot.width, shot.height), (100, 3010));
    assert!(!shot.clamped);
    assert_eq!(img.px(50, 0), RED);
    assert_eq!(img.px(50, 2999), RED);
    assert_eq!(img.px(50, 3000), BLUE);
    assert_eq!(img.px(50, 3009), BLUE);
}

#[test]
fn full_page_of_a_short_document_is_the_viewport() {
    let mut p = page(BOXES, 150, 120);
    let (shot, _) = p.shoot(with(Capture::FullPage));
    assert_eq!((shot.width, shot.height), (150, 120));
}

#[test]
fn full_page_is_cut_at_the_size_limit_and_says_so() {
    let html = "<!doctype html><body style='margin:0'><div style='height:20000px;\
                background:#f00'></div>";
    let mut p = page(html, 100, 100);
    let (shot, img) = p.shoot(with(Capture::FullPage));
    assert_eq!((shot.width, shot.height), (100, 16_384));
    assert!(shot.clamped);
    assert_eq!(img.px(50, 16_383), RED);
}

#[test]
fn element_capture_is_the_border_box() {
    let html = "<!doctype html><body style='margin:0'><div style='height:13px'></div>\
                <div id=t style='margin-left:7px;width:40px;height:25px;border:3px solid #00f;\
                background:#ff0'></div><p>after</p>";
    let mut p = page(html, 100, 100);
    let target = p.node("t");
    let (shot, img) = p.shoot(with(Capture::Element(target)));
    assert_eq!((shot.width, shot.height), (46, 31));
    assert_eq!(img.px(0, 0), BLUE);
    assert_eq!(img.px(45, 30), BLUE);
    assert_eq!(img.px(3, 3), YELLOW);
    assert_eq!(img.px(42, 27), YELLOW);
    assert_eq!(img.ink(WHITE, 0..46, 0..31), 46 * 31);
}

#[test]
fn element_capture_reaches_below_the_viewport() {
    let html = "<!doctype html><body style='margin:0'><div style='height:500px'></div>\
                <div id=t style='height:20px;background:#f00'></div>";
    let mut p = page(html, 100, 100);
    let target = p.node("t");
    let (shot, img) = p.shoot(with(Capture::Element(target)));
    assert_eq!((shot.width, shot.height), (100, 20));
    assert_eq!(img.count(RED, 0..100, 0..20), 2000);
}

#[test]
fn element_capture_covers_every_fragment_of_a_wrapped_inline() {
    let html = "<!doctype html><body style='margin:0;width:60px;font:12px/16px monospace'>\
                <span id=s style='background:#f00'>aaaa bbbb cccc dddd</span>";
    let mut p = page(html, 100, 100);
    let s = p.node("s");
    let fragments = p.session.layout(&p.doc).fragments_of(s).len();
    assert!(fragments > 1, "the span wraps onto {fragments} lines");
    let (shot, _) = p.shoot(with(Capture::Element(s)));
    // Each line box is 16px; the span's own box is a little shorter than that.
    let lines = fragments as u32;
    assert!(shot.height > 16 * (lines - 1) && shot.height <= 16 * lines);
}

#[test]
fn element_capture_rejects_nodes_without_a_box() {
    let html = "<!doctype html><div id=gone style='display:none'>x</div>";
    let mut p = page(html, 50, 50);
    let gone = p.node("gone");
    assert_eq!(
        p.try_shoot(with(Capture::Element(gone))).err(),
        Some(RenderError::NoBox(gone))
    );
}

#[test]
fn clip_is_in_css_pixels_of_the_document() {
    let mut p = page(BOXES, 200, 150);
    let (shot, img) = p.shoot(with(Capture::Clip(Rect::new(90.0, 40.0, 50.0, 40.0))));
    assert_eq!((shot.width, shot.height), (50, 40));
    // Red covers x<100 and y<50 of the document; green starts at (30, 70).
    assert_eq!(img.px(9, 9), RED);
    assert_eq!(img.px(10, 9), WHITE);
    assert_eq!(img.px(9, 10), WHITE);
    assert_eq!(img.px(0, 29), WHITE);
    assert_eq!(img.count(RED, 0..50, 0..40), 10 * 10);
}

#[test]
fn clip_may_extend_past_the_document() {
    let mut p = page(BOXES, 100, 100);
    let (shot, img) = p.shoot(ScreenshotOptions {
        capture: Capture::Clip(Rect::new(-10.0, -10.0, 30.0, 30.0)),
        omit_background: true,
        ..ScreenshotOptions::default()
    });
    assert_eq!((shot.width, shot.height), (30, 30));
    assert_eq!(img.px(9, 9), [0, 0, 0, 0]);
    assert_eq!(img.px(10, 10), RED);
}

#[test]
fn clip_of_fractional_size_is_widened_to_whole_pixels() {
    let mut p = page(BOXES, 100, 100);
    let (shot, img) = p.shoot(with(Capture::Clip(Rect::new(95.5, 0.0, 10.0, 10.0))));
    assert_eq!((shot.width, shot.height), (11, 10));
    assert_eq!(img.px(4, 5), RED);
    assert_eq!(img.px(5, 5), WHITE);
}

#[test]
fn a_scale_multiplies_every_device_dimension() {
    let mut p = page(BOXES, 200, 120);
    let (shot, img) = p.shoot(ScreenshotOptions {
        scale: Some(2.0),
        ..ScreenshotOptions::default()
    });
    assert_eq!((shot.width, shot.height), (400, 240));
    assert_eq!(img.px(199, 99), RED);
    assert_eq!(img.px(200, 99), WHITE);
    assert_eq!(img.px(199, 100), WHITE);
    assert_eq!(img.count(RED, 0..400, 0..240), 200 * 100);
    assert_eq!(img.count(GREEN, 0..400, 0..240), 80 * 60);
    assert_eq!(img.px(60, 140), GREEN);

    let (shot, _) = p.shoot(ScreenshotOptions {
        scale: Some(2.0),
        capture: Capture::Clip(Rect::new(0.0, 0.0, 10.5, 5.0)),
        ..ScreenshotOptions::default()
    });
    assert_eq!((shot.width, shot.height), (21, 10));
}

#[test]
fn the_viewport_device_pixel_ratio_scales_the_output() {
    let mut p = page_with(BOXES, config(100, 50, 2.0));
    let (shot, img) = p.viewport();
    assert_eq!((shot.width, shot.height), (200, 100));
    assert_eq!(img.px(199, 99), RED);
    assert_eq!(img.px(200 - 1, 100 - 1), RED);
}

#[test]
fn device_pixels_are_css_pixels_times_ratio_times_scale() {
    let mut p = page_with(BOXES, config(100, 50, 2.0));
    let (shot, img) = p.shoot(ScreenshotOptions {
        scale: Some(1.5),
        ..ScreenshotOptions::default()
    });
    assert_eq!((shot.width, shot.height), (300, 150));
    // The red box is 100 CSS px wide and 50 high, which is the whole image.
    assert_eq!(img.count(RED, 0..300, 0..150), 300 * 150);
}

const RESOLUTION: &str = "<!doctype html><style>body{margin:0}\
    #r{width:10px;height:10px;background:#f00}\
    @media (min-resolution: 2dppx) { #r { background: #00f } }</style><div id=r></div>";

#[test]
fn the_viewport_device_pixel_ratio_reaches_media_queries_but_the_scale_does_not() {
    let mut p = page_with(RESOLUTION, config(20, 20, 2.0));
    assert_eq!(p.viewport().1.px(5, 5), BLUE);

    let mut p = page_with(RESOLUTION, config(20, 20, 1.0));
    let (shot, img) = p.shoot(ScreenshotOptions {
        scale: Some(2.0),
        ..ScreenshotOptions::default()
    });
    assert_eq!((shot.width, shot.height), (40, 40));
    assert_eq!(img.px(5, 5), RED, "a clip scale is invisible to style");
}

#[test]
fn changing_only_the_device_pixel_ratio_restyles_once() {
    let mut p = page_with(RESOLUTION, config(20, 20, 1.0));
    assert_eq!(p.viewport().1.px(5, 5), RED);
    let before = p.session.counters();

    let doubled = Viewport {
        width: 20,
        height: 20,
        device_pixel_ratio: 2.0,
    };
    p.session.set_viewport(doubled);
    let (shot, img) = p.viewport();
    assert_eq!((shot.width, shot.height), (40, 40));
    assert_eq!(img.px(5, 5), BLUE);
    let after = p.session.counters();
    assert_eq!(after.style_traversals, before.style_traversals + 1);
    assert_eq!(after.layouts, before.layouts + 1);

    p.session.set_viewport(doubled);
    p.viewport();
    assert_eq!(p.session.counters(), after, "the same viewport is free");
}

#[test]
fn full_page_keeps_viewport_units_unchanged() {
    let html = "<!doctype html><body style='margin:0'><div style='min-height:200vh'></div>";
    let mut p = page(html, 100, 200);
    let (shot, _) = p.shoot(with(Capture::FullPage));
    assert_eq!(
        (shot.width, shot.height),
        (100, 400),
        "200vh of the 200px viewport"
    );
    assert_eq!(
        p.session.counters().layouts,
        1,
        "one layout, no viewport resize"
    );
}

#[test]
fn full_page_paints_fixed_boxes_once_where_the_first_screen_has_them() {
    let html = "<!doctype html><body style='margin:0'><div style='height:1000px'></div>\
                <div style='position:fixed;bottom:0;left:0;width:50px;height:20px;\
                background:#f00'></div>";
    let mut p = page(html, 100, 200);
    let (shot, img) = p.shoot(with(Capture::FullPage));
    assert_eq!((shot.width, shot.height), (100, 1000));
    assert_eq!(img.count(RED, 0..100, 0..1000), 50 * 20, "painted once");
    assert_eq!(img.px(10, 190), RED, "at the bottom of the first screen");
    assert_eq!(img.px(10, 990), WHITE);
}

#[test]
fn full_page_is_as_wide_as_the_content() {
    let html = "<!doctype html><body style='margin:0'>\
                <div style='width:300px;height:10px;background:#f00'></div>";
    let mut p = page(html, 100, 50);
    let (shot, img) = p.shoot(with(Capture::FullPage));
    assert_eq!((shot.width, shot.height), (300, 50));
    assert_eq!(img.px(299, 5), RED);
    let (narrow, _) = p.viewport();
    assert_eq!(narrow.width, 100);
}

#[test]
fn element_capture_uses_the_untransformed_layout_box() {
    let html = "<!doctype html><body style='margin:0'><div id=t style='width:40px;height:20px;\
                background:#f00;transform:translateX(100px)'></div>";
    let mut p = page(html, 200, 50);
    let t = p.node("t");
    let (shot, img) = p.shoot(with(Capture::Element(t)));
    assert_eq!((shot.width, shot.height), (40, 20));
    assert_eq!(
        img.count(RED, 0..40, 0..20),
        0,
        "the painted box moved away"
    );
    let (_, whole) = p.viewport();
    assert_eq!(whole.px(120, 10), RED);
}

#[test]
fn a_jpeg_capture_has_the_capture_size_and_a_valid_quality() {
    let mut p = page(BOXES, 50, 40);
    let shot = p
        .try_shoot(ScreenshotOptions {
            format: ImageFormat::Jpeg { quality: 80 },
            ..ScreenshotOptions::default()
        })
        .expect("a JPEG capture");
    assert_eq!((shot.width, shot.height), (50, 40));
    assert_eq!(&shot.bytes[..2], [0xff, 0xd8]);
    for quality in [0, 101] {
        let result = p.try_shoot(ScreenshotOptions {
            format: ImageFormat::Jpeg { quality },
            ..ScreenshotOptions::default()
        });
        assert!(matches!(result, Err(RenderError::InvalidOption(_))));
    }
}

#[test]
fn unsupported_and_invalid_options_are_typed_errors() {
    let mut p = page(BOXES, 50, 50);
    let format = ImageFormat::Webp;
    {
        let opts = ScreenshotOptions {
            format,
            ..ScreenshotOptions::default()
        };
        assert_eq!(
            p.try_shoot(opts).err(),
            Some(RenderError::UnsupportedFormat(format))
        );
    }
    for scale in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        let opts = ScreenshotOptions {
            scale: Some(scale),
            ..ScreenshotOptions::default()
        };
        assert!(matches!(
            p.try_shoot(opts),
            Err(RenderError::InvalidOption(_))
        ));
    }
    for clip in [
        Rect::new(0.0, 0.0, 0.0, 10.0),
        Rect::new(0.0, 0.0, -5.0, 10.0),
        Rect::new(f32::NAN, 0.0, 5.0, 10.0),
        Rect::new(0.0, 0.0, f32::INFINITY, 10.0),
    ] {
        assert!(matches!(
            p.try_shoot(with(Capture::Clip(clip))),
            Err(RenderError::InvalidOption(_))
        ));
    }
    assert!(matches!(
        p.try_shoot(with(Capture::Clip(Rect::new(0.0, 0.0, 20_000.0, 10.0)))),
        Err(RenderError::TooLarge { width: 20_000, .. })
    ));
    assert!(matches!(
        p.try_shoot(ScreenshotOptions {
            scale: Some(400.0),
            ..ScreenshotOptions::default()
        }),
        Err(RenderError::TooLarge { .. })
    ));
}

#[test]
fn a_document_without_a_root_is_refused() {
    let mut session = RenderSession::new(RenderConfig::default());
    let result = session.screenshot(&Document::new(), &ScreenshotOptions::default());
    assert_eq!(result.err(), Some(RenderError::NoRoot));
}

#[test]
fn unchanged_sources_do_not_restyle() {
    let html = "<!doctype html><style>#r{background:#f00;width:10px;height:10px}</style>\
                <div id=r></div>";
    let mut p = page(html, 40, 30);
    p.viewport();
    let before = p.session.counters();
    assert_eq!(before.source_sets, 1);

    p.session.set_style_sources(p.sources.clone());
    p.viewport();
    let after = p.session.counters();
    assert_eq!(after, before, "equal sheets are a no-op");

    let mut changed = p.sources.clone();
    changed[0].text = "#r{background:#00f;width:10px;height:10px}".to_owned();
    p.session.set_style_sources(changed);
    let (_, img) = p.viewport();
    assert_eq!(img.px(8, 8), BLUE);
    let after = p.session.counters();
    assert_eq!(after.source_sets, 2);
    assert_eq!(after.style_traversals, before.style_traversals + 1);
    assert_eq!(after.layouts, before.layouts + 1);
}

#[test]
fn a_second_capture_neither_lays_out_nor_rebuilds_the_list() {
    let mut p = page(BOXES, 100, 100);
    let (_, first) = p.viewport();
    let one = p.session.counters();
    assert_eq!((one.layouts, one.display_lists), (1, 1));

    let (_, second) = p.viewport();
    p.session.layout(&p.doc);
    let two = p.session.counters();
    assert_eq!(two, one);
    assert_eq!(first.rgba, second.rgba, "repeatable output");

    // Other captures of the same layout reuse the list too.
    let target = p.node("g");
    p.shoot(with(Capture::Element(target)));
    p.shoot(with(Capture::Clip(Rect::new(0.0, 0.0, 10.0, 10.0))));
    assert_eq!(p.session.counters(), one);
}

#[test]
fn geometry_alone_does_not_paint() {
    let mut p = page(BOXES, 100, 100);
    p.session.layout(&p.doc);
    let counters = p.session.counters();
    assert_eq!((counters.layouts, counters.display_lists), (1, 0));
    p.viewport();
    let counters = p.session.counters();
    assert_eq!((counters.layouts, counters.display_lists), (1, 1));
}

#[test]
fn a_document_mutation_changes_the_output() {
    let mut p = page(BOXES, 100, 100);
    let (_, before) = p.viewport();
    assert_eq!(before.px(10, 10), RED);
    p.set_attribute("r", "style", "background:#00f;height:20px");
    let (_, after) = p.viewport();
    assert_eq!(after.px(10, 10), BLUE);
    assert_eq!(after.px(10, 30), WHITE, "the shorter box ends at 20");
    assert_eq!(after.px(40, 50), GREEN, "the second box did not move");
    let counters = p.session.counters();
    assert_eq!((counters.layouts, counters.display_lists), (2, 2));
}

#[test]
fn a_viewport_change_lays_out_again_but_the_same_viewport_does_not() {
    let mut p = page(BOXES, 100, 100);
    p.viewport();
    p.session.set_viewport(Viewport {
        width: 100,
        height: 100,
        device_pixel_ratio: 1.0,
    });
    p.viewport();
    assert_eq!(p.session.counters().layouts, 1);

    p.session.set_viewport(Viewport {
        width: 60,
        height: 40,
        device_pixel_ratio: 1.0,
    });
    let (shot, _) = p.viewport();
    assert_eq!((shot.width, shot.height), (60, 40));
    assert_eq!(p.session.counters().layouts, 2);
}

#[test]
fn media_queries_follow_the_viewport() {
    let html = "<!doctype html><style>body{margin:0}\
                #r{width:10px;height:10px;background:#f00}\
                @media (max-width: 99px) { #r { background: #00f } }</style><div id=r></div>";
    let mut p = page(html, 200, 20);
    assert_eq!(p.viewport().1.px(5, 5), RED);
    p.session.set_viewport(Viewport {
        width: 50,
        height: 20,
        device_pixel_ratio: 1.0,
    });
    assert_eq!(p.viewport().1.px(5, 5), BLUE);
}

#[test]
fn unsupported_painting_is_reported_not_hidden() {
    let html = "<!doctype html><body style='margin:0'><div style='width:40px;height:40px;\
                box-shadow:0 0 6px #000'></div>";
    let mut p = page(html, 50, 50);
    let (shot, _) = p.viewport();
    assert!(!shot.stats.is_complete());
    assert_eq!(shot.stats.skipped.get("BoxShadow"), Some(&1));
}

#[test]
fn linear_gradients_are_drawn_across_their_box() {
    let html = "<!doctype html><body style='margin:0'><div style='width:40px;height:40px;\
                background:linear-gradient(to right, red, blue)'></div>";
    let mut p = page(html, 50, 50);
    let (shot, image) = p.viewport();
    assert!(shot.stats.is_complete(), "{:?}", shot.stats);
    let (left, right) = (image.px(1, 20), image.px(38, 20));
    assert!(left[0] > 230 && left[2] < 25, "near red: {left:?}");
    assert!(right[2] > 230 && right[0] < 25, "near blue: {right:?}");
    assert_eq!(image.px(45, 20), [255, 255, 255, 255], "outside the box");
}

#[test]
fn approximated_drawing_is_reported_but_the_capture_is_complete() {
    let mut p = page("<!doctype html><a href=x>link</a>", 80, 40);
    let (shot, _) = p.viewport();
    assert!(shot.stats.is_complete());
    assert!(!shot.stats.is_exact());
    assert_eq!(
        shot.stats.approximated.get("TextDecorationSkipInk"),
        Some(&1)
    );
}

fn golden_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/render-goldens")
        .join(format!("{name}.png"))
}

/// Compares with the blessed golden within one unit per channel;
/// `UPDATE_GOLDEN=1` rewrites it.
fn assert_golden(name: &str, bytes: &[u8]) {
    let path = golden_path(name);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, bytes).expect("golden is writable");
        return;
    }
    let golden = std::fs::read(&path).unwrap_or_else(|err| {
        panic!(
            "missing golden {}: {err}; bless it with UPDATE_GOLDEN=1",
            path.display()
        )
    });
    let (expected, actual) = (Image::decode(&golden), Image::decode(bytes));
    assert_eq!(
        (actual.width, actual.height),
        (expected.width, expected.height),
        "golden {name} has another size"
    );
    let worst = expected
        .rgba
        .iter()
        .zip(&actual.rgba)
        .map(|(e, a)| e.abs_diff(*a))
        .max()
        .unwrap_or(0);
    assert!(worst <= 1, "golden {name} differs by {worst} per channel");
}

#[test]
fn synthetic_fixtures_match_their_goldens() {
    for (name, width, height) in [
        ("solid-boxes", 200, 120),
        ("text-basic", 600, 60),
        ("nested-lists", 200, 120),
    ] {
        let mut p = page(&fixture(&format!("{name}.html")), width, height);
        let (shot, _) = p.viewport();
        assert_golden(name, &shot.bytes);
    }
}

/// The head of `static-article` up to its `sections`-th `<h2>`, closed
/// properly. The full fixture is 1 MB and 330 000 px tall, which only the
/// timing test needs.
fn article_head(sections: usize) -> String {
    let path = tenta_testutil::fixtures_root().join("static-article/index.html");
    let html = std::fs::read_to_string(&path).expect("the static-article fixture exists");
    let cut = html
        .match_indices("<h2")
        .nth(sections)
        .map(|(at, _)| at)
        .expect("the article has that many sections");
    format!(
        "{}</article></main><footer class=\"site\"><p>end</p></footer></body></html>",
        &html[..cut]
    )
}

#[test]
fn the_static_article_renders_with_its_style_sheet() {
    const ACCENT: [u8; 4] = [0x0a, 0x4d, 0x8c, 255];
    const PAPER: [u8; 4] = [0xfb, 0xfa, 0xf7, 255];
    const RULE: [u8; 4] = [0xd9, 0xd9, 0xd9, 255];
    let mut p = page(&article_head(9), 1280, 800);
    let (shot, img) = p.viewport();
    assert_eq!((shot.width, shot.height), (1280, 800));

    let root = p.doc.document_element().expect("a root");
    let select = |p: &mut Page, selector: &str| {
        let node = p
            .doc
            .query_selector(root, selector)
            .expect("valid selector")
            .unwrap_or_else(|| panic!("{selector} exists"));
        let frags = p.session.layout(&p.doc);
        frags.absolute_border_box(frags.fragments_of(node)[0])
    };
    let header = select(&mut p, "header.site");
    let main = select(&mut p, "main");
    let h1 = select(&mut p, "h1");
    let byline = select(&mut p, "p.byline");
    let h2 = select(&mut p, "h2");
    let first_item = select(&mut p, "ul.toc li");
    let near = |actual: f32, expected: f32| {
        assert!(
            (actual - expected).abs() <= 1.0,
            "{actual} is not within 1px of {expected}"
        );
    };

    // `main` is `max-width: 760px` plus 24px padding, centred.
    assert_eq!((main.x, main.w), (236.0, 808.0));
    // The `h1` is 2.2rem on a 17px root, with a 0.6em top margin below the
    // padding of `main`, and wraps onto two lines of 1.15.
    let h1_size = 2.2 * 17.0;
    near(h1.y, header.bottom() + 24.0 + 0.6 * h1_size);
    near(h1.h, 2.0 * 1.15 * h1_size);
    assert_eq!((h1.x, h1.w), (260.0, 760.0));
    // `h2`'s 2em margin on a 1.5rem font beats the byline's 1em margin.
    near(h2.y, byline.bottom() + 2.0 * 1.5 * 17.0);

    let (top, bottom) = (|r: Rect| r.y as u32, |r: Rect| r.bottom().ceil() as u32);
    // The canvas is the root's background, outside and inside `main`.
    assert_eq!(img.px(5, 300), PAPER);
    assert_eq!(img.px(1275, 790), PAPER);
    assert_eq!(
        img.ink(PAPER, 0..236, bottom(header) + 2..800),
        0,
        "left gutter is empty"
    );
    assert_eq!(
        img.ink(PAPER, 1044..1280, bottom(header) + 2..800),
        0,
        "right gutter is empty"
    );
    // Heading and byline text sit in their rows and the gaps between are bare.
    assert!(img.ink(PAPER, 260..1020, top(h1)..bottom(h1)) > 500);
    assert!(img.ink(PAPER, 260..1020, top(byline)..bottom(byline)) > 100);
    assert_eq!(
        img.ink(PAPER, 236..1044, bottom(byline) + 1..top(h2) - 1),
        0
    );
    assert_eq!(img.ink(PAPER, 236..1044, bottom(h1) + 1..top(byline)), 0);
    // The header's bottom border and the `h2` rule are 1px rules of #d9d9d9.
    for rule_below in [header.bottom(), h2.bottom()] {
        let row = rule_below.round() as u32 - 1;
        assert!(
            (row - 1..=row + 1).any(|y| img.px(5, y) == RULE || img.px(500, y) == RULE),
            "a rule near y={rule_below}"
        );
    }
    // The first contents link is underlined in exactly the accent colour
    // and the list item has a marker on its left.
    let item_rows = top(first_item)..bottom(first_item);
    assert!(img.count(ACCENT, 300..1020, item_rows.clone()) >= 30);
    assert!(
        img.ink(PAPER, 280..298, item_rows) > 4,
        "a marker left of the item"
    );
    assert!(!shot.clamped);

    // A paragraph far below the viewport is captured whole, justified text
    // spanning the full measure of the column.
    let paragraphs = p.doc.query_selector_all(root, "p").expect("valid selector");
    let paragraph = paragraphs[1];
    let rect = {
        let frags = p.session.layout(&p.doc);
        frags.absolute_border_box(frags.fragments_of(paragraph)[0])
    };
    assert!(rect.y > 800.0, "the paragraph is below the first screen");
    let (shot, img) = p.shoot(with(Capture::Element(paragraph)));
    // The capture covers whole pixels around the border box, so a fractional
    // top edge can cost one more row than the height alone suggests.
    let rows = rect.bottom().ceil() - rect.y.floor();
    assert_eq!((shot.width, shot.height), (760, rows as u32));
    assert!(img.ink(PAPER, 0..760, 0..shot.height) > 1500);
    assert!(
        img.ink(PAPER, 740..760, 0..shot.height) > 20,
        "justified lines reach the right edge"
    );
}

fn abs_box(style: &str, color: &str) -> String {
    format!(
        "<div style='position:absolute;width:20px;height:20px;background:{color};{style}'></div>"
    )
}

#[test]
fn an_absolute_box_escapes_an_overflow_clip_it_is_not_contained_by() {
    // The clipper is not positioned, so the box's containing block is the
    // initial one and the clip does not apply to it.
    let html = format!(
        "<!doctype html><body style='margin:0'>\
         <div style='overflow:hidden;width:50px;height:50px'>{}</div>",
        abs_box("left:100px;top:0", "#f00")
    );
    let (_, img) = page(&html, 200, 100).viewport();
    assert_eq!(img.count(RED, 0..200, 0..100), 400);
    assert_eq!(img.px(110, 10), RED);
}

#[test]
fn an_absolute_box_is_clipped_by_a_positioned_overflow_clip() {
    let html = format!(
        "<!doctype html><body style='margin:0'>\
         <div style='position:relative;overflow:hidden;width:50px;height:50px'>{}</div>",
        abs_box("left:40px;top:0", "#f00")
    );
    let (_, img) = page(&html, 200, 100).viewport();
    assert_eq!(img.count(RED, 0..200, 0..100), 10 * 20);
    assert_eq!(img.px(45, 10), RED);
    assert_eq!(img.px(55, 10), WHITE);
}

#[test]
fn positioned_boxes_paint_in_tree_order_wherever_their_containing_block_is() {
    // The absolute box is earlier in the tree than the relative sibling, but
    // belongs to the outer positioned ancestor. Both have `z-index: auto`, so
    // the later one paints on top.
    let html = format!(
        "<!doctype html><body style='margin:0'><div style='position:relative'>\
         <div>{}</div>\
         <div style='position:relative;width:20px;height:20px;background:#00f'></div></div>",
        abs_box("left:0;top:0", "#f00")
    );
    let (_, img) = page(&html, 100, 100).viewport();
    assert_eq!(img.px(5, 5), BLUE);
    assert_eq!(img.count(RED, 0..100, 0..100), 0);
}

#[test]
fn z_index_orders_absolute_boxes_before_tree_order() {
    let html = format!(
        "<!doctype html><body style='margin:0'>{}{}{}",
        abs_box("left:0;top:0;z-index:2", "#f00"),
        abs_box("left:10px;top:0;z-index:1", "#0f0"),
        abs_box("left:5px;top:0;z-index:-1", "#00f")
    );
    let (_, img) = page(&html, 100, 100).viewport();
    // Red (z=2) over green (z=1) over blue (z=-1), whatever the tree order.
    assert_eq!(img.px(15, 5), RED);
    assert_eq!(img.px(25, 5), GREEN);
    assert_eq!(img.px(22, 5), GREEN);
    assert_eq!(img.px(2, 5), RED);
    assert_eq!(img.count(BLUE, 0..100, 0..100), 0, "fully covered");
}

#[test]
fn a_negative_z_index_paints_above_the_context_background_but_below_flow_content() {
    let html = "<!doctype html><body style='margin:0;background:#ff0'>\
                <div style='position:absolute;left:0;top:0;width:40px;height:40px;z-index:-1;background:#f00'></div>\
                <div style='width:20px;height:20px;background:#00f'></div>";
    let (_, img) = page(html, 100, 100).viewport();
    assert_eq!(img.px(30, 30), RED, "above the canvas");
    assert_eq!(img.px(10, 10), BLUE, "below in-flow content");
}

#[test]
fn fixed_boxes_and_scroll_extent_do_not_move_the_canvas() {
    let html = "<!doctype html><body style='margin:0'>\
                <div style='position:fixed;left:150px;top:10px;width:20px;height:20px;background:#f00'></div>";
    let mut p = page(html, 100, 50);
    let (shot, _) = p.shoot(with(Capture::FullPage));
    assert_eq!(
        (shot.width, shot.height),
        (100, 50),
        "fixed boxes add no scroll extent"
    );
}

#[test]
fn a_flex_item_with_z_index_is_a_stacking_context_without_being_positioned() {
    // The second item overlaps the first through a negative margin. Without
    // z-index it would paint over it; with `z-index: 1` the first item wins.
    let html = "<!doctype html><body style='margin:0'><div style='display:flex'>\
                <div style='width:40px;height:40px;background:#f00;z-index:1'></div>\
                <div style='width:40px;height:40px;background:#00f;margin-left:-20px'></div></div>";
    let (_, img) = page(html, 100, 100).viewport();
    assert_eq!(img.px(30, 10), RED);
    assert_eq!(img.px(50, 10), BLUE);
}

#[test]
fn an_inline_block_with_z_index_and_no_position_is_not_a_stacking_context() {
    let html = "<!doctype html><body style='margin:0'>\
                <span style='display:inline-block;width:40px;height:40px;background:#f00;z-index:1'></span>\
                <span style='display:inline-block;width:40px;height:40px;background:#00f;margin-left:-60px'></span>";
    let (_, img) = page(html, 100, 100).viewport();
    assert_eq!(
        img.px(10, 10),
        BLUE,
        "z-index does not apply to a static box"
    );
}

/// Whether `actual` is within `tolerance` of `expected` on every channel.
fn near(actual: [u8; 4], expected: [u8; 3], tolerance: u8) -> bool {
    actual[..3]
        .iter()
        .zip(expected)
        .all(|(a, e)| a.abs_diff(e) <= tolerance)
}

const ABS_RED: &str = "position:absolute;left:100px;top:0;width:80px;height:80px;background:#f00";

#[test]
fn an_opacity_wrapper_between_the_containing_block_and_an_absolute_box_applies_to_it() {
    // The wrapper is not the containing block, yet its opacity group holds the box.
    let html = format!(
        "<!doctype html><body style='margin:0'><div style='position:relative;width:200px;\
         height:200px;background:#ccc'><div style='opacity:.5;width:100px;height:100px;\
         background:#00f'><div style='{ABS_RED}'></div></div></div>"
    );
    let (_, img) = page(&html, 300, 250).viewport();
    assert!(
        near(img.px(140, 40), [230, 102, 102], 3),
        "{:?}",
        img.px(140, 40)
    );
    assert!(
        near(img.px(50, 50), [102, 102, 230], 3),
        "{:?}",
        img.px(50, 50)
    );
}

#[test]
fn a_transparent_wrapper_hides_the_absolute_boxes_inside_it() {
    let html = format!(
        "<!doctype html><body style='margin:0'><div style='position:relative;width:200px;\
         height:200px;background:#ccc'><div style='opacity:0;width:100px;height:100px'>\
         <div style='{ABS_RED}'></div></div></div>"
    );
    let (_, img) = page(&html, 300, 250).viewport();
    assert_eq!(
        img.count(RED, 0..300, 0..250),
        0,
        "the hidden-tooltip pattern"
    );
    assert!(near(img.px(140, 40), [204, 204, 204], 1));
}

#[test]
fn an_opacity_wrapper_applies_to_a_fixed_box_inside_it() {
    let html = format!(
        "<!doctype html><body style='margin:0'><div style='opacity:.5;width:100px;height:100px;\
         background:#00f'><div style='{}'></div></div>",
        ABS_RED.replace("absolute", "fixed")
    );
    let (_, img) = page(&html, 300, 250).viewport();
    assert!(
        near(img.px(140, 40), [255, 128, 128], 3),
        "{:?}",
        img.px(140, 40)
    );
    let html = html.replace("opacity:.5", "opacity:0");
    let (_, img) = page(&html, 300, 250).viewport();
    assert_eq!(img.count(RED, 0..300, 0..250), 0);
}

#[test]
fn an_isolating_wrapper_keeps_the_z_index_of_a_box_inside_its_own_group() {
    // The red box is z-index 10, but inside the isolated wrapper's group, which
    // is a z-index auto layer: the sibling at z-index 5 covers it.
    let html = "<!doctype html><body style='margin:0'><div style='position:relative;width:240px;\
                height:240px;background:#ddd'><div style='isolation:isolate;width:100px;height:100px;\
                background:#00f'><div style='position:absolute;z-index:10;left:60px;top:0;width:80px;\
                height:80px;background:#f00'></div></div>\
                <div style='position:absolute;z-index:5;left:100px;top:40px;width:80px;height:80px;\
                background:#0f0'></div></div>";
    let (_, img) = page(html, 300, 250).viewport();
    assert_eq!(img.px(120, 60), GREEN, "the green box covers the overlap");
    assert_eq!(img.px(80, 20), RED, "red outside the overlap");
}

#[test]
fn an_opacity_group_holds_a_positive_z_index_box_inside_it() {
    let html = "<!doctype html><body style='margin:0'><div style='position:relative;width:240px;\
                height:240px;background:#fff'><div style='opacity:.99;width:100px;height:100px'>\
                <div style='position:absolute;z-index:10;left:60px;top:0;width:80px;height:80px;\
                background:#f00'></div></div>\
                <div style='position:absolute;z-index:5;left:100px;top:40px;width:80px;height:80px;\
                background:#0f0'></div></div>";
    let (_, img) = page(html, 300, 250).viewport();
    assert_eq!(img.px(120, 60), GREEN);
}

#[test]
#[ignore = "ST1: filter and clip-path are not painted yet, for in-flow children either"]
fn a_filter_wrapper_is_the_containing_block_and_its_effect_applies() {
    let html = "<!doctype html><body style='margin:0'><div style='position:relative;width:300px;\
                height:200px;background:#fff'><div style='filter:opacity(.5);margin-left:20px;\
                width:100px;height:100px'><div style='position:absolute;left:0;top:0;width:40px;\
                height:40px;background:#f00'></div></div></div>";
    let (_, img) = page(html, 300, 250).viewport();
    assert!(
        near(img.px(30, 10), [255, 128, 128], 3),
        "{:?}",
        img.px(30, 10)
    );
}

#[test]
#[ignore = "ST1: filter and clip-path are not painted yet, for in-flow children either"]
fn a_clip_path_wrapper_cuts_an_absolute_box_inside_it() {
    let html = "<!doctype html><body style='margin:0'><div style='position:relative;width:240px;\
                height:240px;background:#fff'><div style='clip-path:inset(0);width:50px;height:50px;\
                background:#00f'><div style='position:absolute;left:30px;top:100px;width:80px;\
                height:80px;background:#f00'></div></div></div>";
    let (_, img) = page(html, 300, 250).viewport();
    assert_eq!(img.count(RED, 0..300, 0..250), 0);
}

#[test]
fn an_effect_wrapper_does_not_free_a_box_from_its_containing_blocks_clip() {
    let html = "<!doctype html><body style='margin:0'><div style='position:relative;overflow:hidden;\
                width:50px;height:50px'><div style='opacity:.5'><div style='position:absolute;left:40px;\
                top:0;width:40px;height:20px;background:#f00'></div></div></div>";
    let (_, img) = page(html, 200, 100).viewport();
    assert_eq!(img.count(RED, 0..200, 0..100), 0);
    assert!(near(img.px(45, 10), [255, 128, 128], 3));
    assert_eq!(img.px(60, 10), WHITE);
}

#[test]
fn an_absolutely_positioned_root_still_gives_its_background_to_the_canvas() {
    let html = "<!doctype html><html style='position:absolute;background:#eee'><body style='margin:0'>\
                <div style='width:10px;height:10px;background:#f00'></div>";
    let (_, img) = page(html, 100, 100).viewport();
    assert!(
        near(img.px(80, 80), [238, 238, 238], 0),
        "{:?}",
        img.px(80, 80)
    );
}

#[test]
fn a_fixed_box_without_a_transformed_ancestor_is_cut_by_no_overflow_clip_but_the_viewports() {
    let html = "<!doctype html><body style='margin:0'><div style='overflow:hidden;width:50px;\
                height:50px'><div style='position:fixed;left:100px;top:0;width:40px;height:20px;\
                background:#f00'></div><div style='position:fixed;left:180px;top:0;width:40px;\
                height:20px;background:#00f'></div></div>";
    let (_, img) = page(html, 200, 100).viewport();
    assert_eq!(
        img.count(RED, 0..200, 0..100),
        40 * 20,
        "escapes the wrapper's clip"
    );
    assert_eq!(
        img.count(BLUE, 0..200, 0..100),
        20 * 20,
        "cut by the viewport only"
    );
}

#[test]
#[ignore = "known gap: a box escaping an overflow clip inside an effect group opened under that clip (see As-built M5)"]
fn a_fixed_box_in_an_opacity_group_inside_an_overflow_clip_is_drawn_at_half() {
    let html = "<!doctype html><body style='margin:0'><div style='overflow:hidden;width:50px;\
                height:50px'><div style='opacity:.5'><div style='position:fixed;left:100px;top:0;\
                width:40px;height:20px;background:#f00'></div></div></div>";
    let (_, img) = page(html, 200, 100).viewport();
    assert!(
        near(img.px(110, 10), [255, 128, 128], 3),
        "{:?}",
        img.px(110, 10)
    );
}

// ------------------------------------------------------------ float paint order
// CSS 2.1 Appendix E: in-flow block backgrounds, then floats, then inline
// content, then positioned boxes.

const FLOAT_PAGE: &str = "<!doctype html><style>body{margin:0}</style>";

#[test]
fn a_float_paints_above_the_background_of_a_later_in_flow_block() {
    let html = format!(
        "{FLOAT_PAGE}<div style='float:left;width:60px;height:60px;background:#f00'></div>\
         <div style='height:40px;background:#00f'></div>"
    );
    let (_, img) = page(&html, 200, 120).viewport();
    // The block starts at the origin underneath the float.
    assert_eq!(img.px(10, 10), RED);
    assert_eq!(img.px(59, 59), RED);
    assert_eq!(img.px(100, 10), BLUE);
    assert_eq!(img.px(100, 50), WHITE);
}

#[test]
fn inline_content_paints_above_a_float_it_overlaps() {
    // The float's negative margin lets the line start inside it.
    let html = format!(
        "{FLOAT_PAGE}<div style='float:left;width:100px;height:30px;margin-right:-80px;background:#f00'></div>\
         <div style=\"height:30px;background:#00f;color:#fff;font:20px 'Liberation Sans'\">MMMM</div>"
    );
    let (_, img) = page(&html, 200, 120).viewport();
    assert_eq!(img.px(5, 28), RED);
    assert_eq!(img.px(150, 28), BLUE);
    // White glyph pixels sit on top of the red float, not hidden by it.
    let white_on_float = img.count(WHITE, 20..100, 0..30);
    assert!(
        white_on_float > 20,
        "{white_on_float} white pixels over the float"
    );
}

#[test]
fn a_later_float_paints_above_an_earlier_one() {
    let html = format!(
        "{FLOAT_PAGE}<div style='float:left;width:60px;height:40px;background:#f00'></div>\
         <div style='float:left;width:60px;height:40px;margin-left:-30px;background:#0f0'></div>"
    );
    let (_, img) = page(&html, 200, 120).viewport();
    assert_eq!(img.px(10, 10), RED);
    assert_eq!(img.px(40, 10), GREEN);
    assert_eq!(img.px(80, 10), GREEN);
}

#[test]
fn a_positioned_box_paints_above_a_float() {
    let html = format!(
        "{FLOAT_PAGE}<div style='position:relative'>\
         <div style='float:left;width:60px;height:40px;background:#f00'></div>\
         <div style='position:absolute;left:0;top:0;width:20px;height:20px;background:#00f'></div></div>"
    );
    let (_, img) = page(&html, 200, 120).viewport();
    assert_eq!(img.px(10, 10), BLUE);
    assert_eq!(img.px(30, 10), RED);
}

#[test]
fn a_cleared_block_and_the_containing_float_paint_where_layout_put_them() {
    let html = format!(
        "{FLOAT_PAGE}<div style='overflow:hidden'>\
         <div style='float:right;width:50px;height:30px;background:#f00'></div></div>\
         <div style='clear:both;height:20px;background:#00f'></div>"
    );
    let (_, img) = page(&html, 200, 120).viewport();
    assert_eq!(img.px(190, 10), RED);
    assert_eq!(img.px(149, 10), WHITE);
    // The overflow:hidden parent contains the float, so the next block follows it.
    assert_eq!(img.px(10, 35), BLUE);
    assert_eq!(img.px(10, 29), WHITE);
}

#[test]
fn a_negative_z_index_child_of_a_float_paints_below_the_float() {
    let html = format!(
        "{FLOAT_PAGE}<div style='float:left;width:60px;height:40px;background:#f00'>\
         <div style='position:relative;z-index:-1;width:30px;height:30px;background:#0f0'></div></div>"
    );
    let (_, img) = page(&html, 200, 120).viewport();
    // A float is not a stacking context, so the child sits in the root's
    // negative layer, beneath the float's own background.
    assert_eq!(img.px(10, 10), RED);
    assert_eq!(img.count(GREEN, 0..60, 0..40), 0);
}

#[test]
fn a_positioned_child_of_a_float_paints_above_it() {
    let html = format!(
        "{FLOAT_PAGE}<div style='float:left;width:60px;height:40px;background:#f00'>\
         <div style='position:absolute;left:5px;top:5px;width:20px;height:20px;background:#00f'></div></div>"
    );
    let (_, img) = page(&html, 200, 120).viewport();
    assert_eq!(img.px(10, 10), BLUE);
    assert_eq!(img.px(40, 30), RED);
}

#[test]
fn text_inside_a_float_is_drawn_over_its_background() {
    let html = format!(
        "{FLOAT_PAGE}<div style=\"float:left;width:100px;height:30px;background:#f00;color:#fff;font:20px 'Liberation Sans'\">MMM</div>"
    );
    let (_, img) = page(&html, 200, 120).viewport();
    assert_eq!(img.px(98, 28), RED);
    assert!(img.count(WHITE, 0..100, 0..30) > 20);
}

#[test]
fn a_float_inside_an_inline_block_paints_within_it() {
    let html = format!(
        "{FLOAT_PAGE}<span style='display:inline-block;width:100px;height:50px;background:#00f'>\
         <div style='float:left;width:30px;height:30px;background:#f00'></div></span>"
    );
    let (_, img) = page(&html, 200, 120).viewport();
    assert_eq!(img.px(10, 10), RED);
    assert_eq!(img.px(60, 10), BLUE);
    assert_eq!(img.px(110, 10), WHITE);
}
