//! Compile-only check of the public surface: every public type is
//! constructible and every entry point has its contract signature.

use tenta_core::NodeId;
use tenta_dom::Document;
use tenta_layout::Rect;
use tenta_render::{
    Capture, FontPolicy, ImageFormat, RenderConfig, RenderError, RenderSession, Screenshot,
    ScreenshotOptions, Viewport,
};

#[test]
fn public_types_are_constructible() {
    let viewport = Viewport {
        width: 800,
        height: 600,
        device_pixel_ratio: 2.0,
    };
    let cfg = RenderConfig {
        viewport,
        fonts: FontPolicy::Embedded,
        dark: true,
    };
    assert_eq!(cfg.viewport.width, 800);
    for capture in [
        Capture::Viewport,
        Capture::FullPage,
        Capture::Clip(Rect::new(0.0, 0.0, 10.0, 10.0)),
        Capture::Element(NodeId(1)),
    ] {
        let opts = ScreenshotOptions {
            format: ImageFormat::Jpeg { quality: 80 },
            capture,
            scale: Some(2.0),
            omit_background: true,
        };
        assert_eq!(opts.capture, capture);
    }
    let shot = Screenshot {
        bytes: Vec::new(),
        format: ImageFormat::Webp,
        width: 1,
        height: 1,
    };
    assert_eq!(shot.width, 1);
    assert_eq!(
        RenderError::TooLarge {
            width: 1,
            height: 2
        }
        .to_string(),
        "a 1x2 capture exceeds the render limits"
    );
}

#[test]
fn entry_points_have_the_contract_signatures() {
    let _: fn(RenderConfig) -> RenderSession = RenderSession::new;
    let _: fn(&mut RenderSession, Viewport) = RenderSession::set_viewport;
    let _: fn(&mut RenderSession, Vec<tenta_css::cascade::StyleSource>) =
        RenderSession::set_style_sources;
    let _: for<'a> fn(&'a mut RenderSession, &Document) -> &'a tenta_layout::FragmentTree =
        RenderSession::layout;
    let _: fn(
        &mut RenderSession,
        &Document,
        &ScreenshotOptions,
    ) -> Result<Screenshot, RenderError> = RenderSession::screenshot;
}
