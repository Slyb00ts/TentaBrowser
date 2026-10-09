//! The session's image pipeline end to end, without a network: images come
//! in as `data:` URLs.

use tenta_dom::Document;
use tenta_html::{Parser, ParserOptions};
use url::Url;

use crate::{Capture, ImageFormat, RenderConfig, RenderSession, ScreenshotOptions, Viewport};

pub(crate) fn parse(html: &str) -> Document {
    let mut parser = Parser::new(Document::new(), ParserOptions::default());
    parser.feed_last(html.as_bytes());
    parser.finish()
}

pub(crate) fn base() -> Url {
    Url::parse("https://example.test/page.html").expect("a constant URL")
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub(crate) fn base64(bytes: &[u8]) -> String {
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// A PNG of `w x h` where `pixel(x, y)` gives straight RGBA.
pub(crate) fn png(w: u32, h: u32, pixel: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("header");
    let mut data = Vec::new();
    for y in 0..h {
        for x in 0..w {
            data.extend_from_slice(&pixel(x, y));
        }
    }
    writer.write_image_data(&data).expect("pixels");
    drop(writer);
    out
}

pub(crate) fn data_url(mime: &str, bytes: &[u8]) -> String {
    format!("data:{mime};base64,{}", base64(bytes))
}

/// A solid-colour PNG as a data URL.
pub(crate) fn solid(w: u32, h: u32, rgba: [u8; 4]) -> String {
    data_url("image/png", &png(w, h, |_, _| rgba))
}

pub(crate) fn session(width: u32, height: u32) -> RenderSession {
    RenderSession::new(RenderConfig {
        viewport: Viewport {
            width,
            height,
            device_pixel_ratio: 1.0,
        },
        ..RenderConfig::default()
    })
}

pub(crate) fn pixels(session: &mut RenderSession, doc: &Document) -> (u32, u32, Vec<u8>) {
    let shot = session
        .screenshot(
            doc,
            &ScreenshotOptions {
                format: ImageFormat::Png,
                capture: Capture::Viewport,
                scale: None,
                omit_background: false,
            },
        )
        .expect("renders");
    let mut reader = png::Decoder::new(std::io::Cursor::new(&shot.bytes))
        .read_info()
        .expect("valid PNG");
    let mut rgba = vec![0; reader.output_buffer_size().expect("fits")];
    let info = reader.next_frame(&mut rgba).expect("valid frame");
    (info.width, info.height, rgba)
}

pub(crate) fn px(frame: &(u32, u32, Vec<u8>), x: u32, y: u32) -> [u8; 4] {
    let at = ((y * frame.0 + x) * 4) as usize;
    frame.2[at..at + 4].try_into().expect("four channels")
}

const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];

#[test]
fn an_image_is_drawn_at_its_natural_size() {
    let url = solid(4, 2, RED);
    let doc = parse(&format!(
        "<!doctype html><body style='margin:0'><img src='{url}' style='display:block'>"
    ));
    let mut session = session(40, 20);
    assert_eq!(session.load_inline_images(&doc, &base()), 1);
    let frame = pixels(&mut session, &doc);
    assert_eq!(px(&frame, 0, 0), RED);
    assert_eq!(px(&frame, 3, 1), RED);
    assert_eq!(px(&frame, 4, 0), WHITE, "right of the image");
    assert_eq!(px(&frame, 0, 8), WHITE, "below the image's line");
}

#[test]
fn width_and_height_resize_the_image_and_the_aspect_ratio_follows() {
    let url = solid(10, 5, BLUE);
    let doc = parse(&format!(
        "<!doctype html><body style='margin:0'><img src='{url}' style='display:block;width:20px'>"
    ));
    let mut session = session(60, 40);
    session.load_inline_images(&doc, &base());
    let frame = pixels(&mut session, &doc);
    assert_eq!(px(&frame, 19, 9), BLUE, "20 by 10 after the ratio");
    assert_eq!(px(&frame, 19, 10), WHITE);
    assert_eq!(px(&frame, 20, 0), WHITE);
}

#[test]
fn a_failed_image_with_alt_text_becomes_text_and_without_it_an_empty_box() {
    let doc = parse(
        "<!doctype html><body style='margin:0'><img src='gone.png' alt='gone'>\
         <img src='gone.png' width=30 height=20 id=box>",
    );
    let mut session = session(100, 40);
    assert_eq!(session.load_inline_images(&doc, &base()), 0);
    assert_eq!(session.image_stats().failed, 1, "one URL, failed once");
    let frame = pixels(&mut session, &doc);
    // The framed empty box has a one-pixel silver border.
    let ink = (0..100u32).any(|x| px(&frame, x, 10) != WHITE);
    assert!(ink, "alt text and the frame draw something");
}

#[test]
fn srcset_follows_the_device_pixel_ratio() {
    let lo = solid(10, 10, RED);
    let hi = solid(20, 20, BLUE);
    let doc = parse(&format!(
        "<!doctype html><body style='margin:0'><img src='{lo}' srcset='{lo} 1x, {hi} 2x' \
         style='display:block;width:10px;height:10px'>"
    ));
    let mut session = RenderSession::new(RenderConfig {
        viewport: Viewport {
            width: 40,
            height: 40,
            device_pixel_ratio: 2.0,
        },
        ..RenderConfig::default()
    });
    session.load_inline_images(&doc, &base());
    let frame = pixels(&mut session, &doc);
    assert_eq!(frame.0, 80);
    assert_eq!(px(&frame, 10, 10), BLUE);
}

#[test]
fn an_svg_is_rendered_again_when_its_box_is_larger_than_its_bitmap() {
    let svg = "<svg xmlns='http://www.w3.org/2000/svg' width='10' height='10' viewBox='0 0 10 10'>\
               <rect width='5' height='10' fill='#f00'/><rect x='5' width='5' height='10' fill='#00f'/></svg>";
    let url = data_url("image/svg+xml", svg.as_bytes());
    let doc = parse(&format!(
        "<!doctype html><body style='margin:0'><img src='{url}' style='display:block;width:100px;height:100px'>"
    ));
    let mut session = session(120, 120);
    session.load_inline_images(&doc, &base());
    let frame = pixels(&mut session, &doc);
    // Sharp at 100 px: the edge between the halves is on one pixel boundary,
    // not smeared across a ten-fold upscale of a 10 px bitmap.
    assert_eq!(px(&frame, 49, 50), RED);
    assert_eq!(px(&frame, 50, 50), BLUE);
}

use crate::{DecodeControl, DecodeError, DecodeSettings, ImageKind, LazyRegion, decode_image};
use tenta_layout::FragmentKind;

fn settings() -> DecodeSettings {
    session(100, 100).decode_settings()
}

#[test]
fn undecodable_bodies_fail_with_typed_errors() {
    let control = DecodeControl::new();
    let corrupt = decode_image(
        b"\x89PNG\r\n\x1a\nnope",
        ImageKind::Raster,
        &settings(),
        &control,
    );
    assert!(
        matches!(corrupt, Err(DecodeError::Malformed { .. })),
        "{corrupt:?}"
    );
    let text = decode_image(b"<html>", ImageKind::Raster, &settings(), &control);
    assert_eq!(text.err(), Some(DecodeError::UnknownFormat));
    let not_svg = decode_image(b"<html>", ImageKind::Svg, &settings(), &control);
    assert!(matches!(not_svg, Err(DecodeError::Malformed { .. })));

    let big = png(40, 40, |_, _| RED);
    let mut tight = settings();
    tight.limits.max_pixels = 100;
    let refused = decode_image(&big, ImageKind::Raster, &tight, &control);
    assert!(
        matches!(refused, Err(DecodeError::TooLarge { .. })),
        "{refused:?}"
    );
}

#[test]
fn a_decode_stops_at_its_deadline_and_when_cancelled() {
    let bytes = png(8, 8, |_, _| RED);
    let late = DecodeControl::new()
        .with_deadline(std::time::Instant::now() - std::time::Duration::from_millis(1));
    assert_eq!(
        decode_image(&bytes, ImageKind::Raster, &settings(), &late).err(),
        Some(DecodeError::TimedOut)
    );
    let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let cancelled = DecodeControl::new().with_cancel(flag);
    assert_eq!(
        decode_image(&bytes, ImageKind::Raster, &settings(), &cancelled).err(),
        Some(DecodeError::Cancelled)
    );
}

fn first_fragment_kind(session: &mut RenderSession, doc: &Document, id: &str) -> FragmentKind {
    let node = doc.get_element_by_id(id).expect("the element");
    let frags = session.layout(doc);
    let frag = frags
        .fragments_of(node)
        .first()
        .copied()
        .expect("a fragment");
    frags.get(frag).kind.clone()
}

#[test]
fn a_broken_image_with_alt_is_text_and_without_it_an_empty_replaced_box() {
    let doc = parse(
        "<!doctype html><body style='margin:0'><img id=text src='gone.png' alt='hello'>\
         <img id=empty src='gone.png' alt=''><img id=plain src='gone.png' width=20 height=10>",
    );
    let mut session = session(200, 100);
    session.load_inline_images(&doc, &base());
    assert!(matches!(
        first_fragment_kind(&mut session, &doc, "text"),
        FragmentKind::Box
    ));
    assert!(matches!(
        first_fragment_kind(&mut session, &doc, "empty"),
        FragmentKind::Replaced(_)
    ));
    let node = doc.get_element_by_id("text").expect("the element");
    let frags = session.layout(&doc);
    let width = frags.absolute_border_box(frags.fragments_of(node)[0]).w;
    assert!(width > 20.0, "the alt text has a width: {width}");
}

#[test]
fn an_image_with_no_source_at_all_is_broken_at_once() {
    let doc = parse("<!doctype html><body><img id=a alt='nothing here'>");
    let mut session = session(200, 100);
    let plan = session.plan_images(&doc, &base(), LazyRegion::All);
    assert!(plan.wants.is_empty(), "nothing to fetch");
    assert!(matches!(
        first_fragment_kind(&mut session, &doc, "a"),
        FragmentKind::Box
    ));
}

#[test]
fn input_type_image_and_video_poster_load_like_images() {
    let red = solid(6, 4, RED);
    let blue = solid(8, 8, BLUE);
    let doc = parse(&format!(
        "<!doctype html><body style='margin:0'><input type=image id=i src='{red}' style='display:block'>\
         <video id=v poster='{blue}' style='display:block'></video>"
    ));
    let mut session = session(100, 100);
    assert_eq!(session.load_inline_images(&doc, &base()), 2);
    let frame = pixels(&mut session, &doc);
    assert_eq!(px(&frame, 2, 2), RED);
    assert_eq!(
        px(&frame, 3, 8),
        BLUE,
        "the poster sits under the input's 4 px"
    );
}

#[test]
fn lazy_images_far_below_the_viewport_wait_for_a_full_page_capture() {
    let near = solid(2, 2, RED);
    let far = solid(2, 2, BLUE);
    let doc = parse(&format!(
        "<!doctype html><body style='margin:0'><img src='{near}' loading=lazy>\
         <div style='height:5000px'></div><img src='{far}' loading=lazy><img src='{far}' alt=''>"
    ));
    let mut session = session(100, 100);
    let viewport = session.plan_images(&doc, &base(), LazyRegion::Viewport);
    // The lazy image far away is not wanted, but the eager one with the same
    // URL is, once.
    assert_eq!(viewport.wants.len(), 2);
    let all = session.plan_images(&doc, &base(), LazyRegion::All);
    assert_eq!(all.wants.len(), 2);

    let doc = parse(&format!(
        "<!doctype html><body style='margin:0'><img src='{near}' loading=lazy>\
         <div style='height:5000px'></div><img src='{far}' loading=lazy>"
    ));
    let mut session = self::session(100, 100);
    assert_eq!(
        session
            .plan_images(&doc, &base(), LazyRegion::Viewport)
            .wants
            .len(),
        1
    );
    assert_eq!(
        session
            .plan_images(&doc, &base(), LazyRegion::All)
            .wants
            .len(),
        2
    );
}

#[test]
fn two_hundred_large_images_all_paint_and_shrink_to_their_boxes() {
    // 200 distinct 360x360 images are 104 MB decoded at first, above the
    // 64 MB budget but under the 128 MB hard ceiling, so all stay resident
    // until the boxes are known; then each is decoded again at 50 px.
    let body: String = (0..200u32)
        .map(|i| {
            let url = data_url(
                "image/png",
                &png(360, 360, |x, _| [i as u8, (x / 2) as u8, 0, 255]),
            );
            format!("<img src='{url}' style='display:block;width:50px;height:50px'>")
        })
        .collect();
    let doc = parse(&format!("<!doctype html><body style='margin:0'>{body}"));
    let mut session = session(100, 600);
    session.load_inline_images(&doc, &base());
    let stats = session.image_stats();
    assert_eq!(
        (stats.loaded, stats.failed, stats.evicted),
        (200, 0, 0),
        "{stats:?}"
    );
    let budget = tenta_core::Budgets::platform_default().context_memory_mb as usize * 1024 * 1024;
    assert!(
        stats.store_bytes <= budget,
        "shrunk to the boxes: {stats:?} against {budget}"
    );
    let shot = session
        .screenshot(
            &doc,
            &ScreenshotOptions {
                capture: Capture::FullPage,
                ..ScreenshotOptions::default()
            },
        )
        .expect("renders");
    assert!(shot.stats.is_complete());
    let mut reader = png::Decoder::new(std::io::Cursor::new(&shot.bytes))
        .read_info()
        .expect("valid PNG");
    let mut rgba = vec![0; reader.output_buffer_size().expect("fits")];
    let info = reader.next_frame(&mut rgba).expect("frame");
    for i in [0u32, 57, 123, 199] {
        let at = ((i * 50 + 10) * info.width * 4 + 10 * 4) as usize;
        assert_eq!(rgba[at], i as u8, "image {i} is painted, not blank");
    }
}

#[test]
fn images_that_cannot_fit_the_ceiling_fail_with_a_budget_reason() {
    // Three 3400x3400 images are 139 MB decoded: more than the 134 MB ceiling,
    // and the viewport is wide enough that none is shrunk while decoding.
    let body: String = (0..3u32)
        .map(|i| {
            let url = data_url(
                "image/png",
                &png(3400, 3400, |x, _| [i as u8, (x / 20) as u8, 0, 255]),
            );
            format!("<img src='{url}' style='display:block;width:3000px;height:3000px'>")
        })
        .collect();
    let doc = parse(&format!("<!doctype html><body style='margin:0'>{body}"));
    let mut session = session(3000, 600);
    session.load_inline_images(&doc, &base());
    let stats = session.image_stats();
    assert_eq!(
        (stats.loaded, stats.failed, stats.evicted),
        (2, 1, 0),
        "{stats:?}"
    );
    let reason = doc
        .descendants(doc.document_node())
        .find_map(|n| session.image_failure_for(&doc, n))
        .expect("a failed image says why");
    assert!(reason.contains("budget"), "{reason}");
}

#[test]
fn a_larger_device_ratio_decodes_a_raster_again_from_the_retained_size() {
    let url = data_url("image/png", &png(400, 400, |_, _| RED));
    let doc = parse(&format!(
        "<!doctype html><body style='margin:0'><img src='{url}' style='display:block;width:200px;height:200px'>"
    ));
    let mut session = session(100, 100);
    session.load_inline_images(&doc, &base());
    // At 1x the 200 px box needs 200 device pixels: the 400 px source is
    // decoded again smaller.
    assert!(session.image_stats().store_bytes < 400 * 400 * 4);
    session.set_viewport(Viewport {
        width: 100,
        height: 100,
        device_pixel_ratio: 2.0,
    });
    session.load_inline_images(&doc, &base());
    assert!(
        session.image_stats().store_bytes >= 400 * 400 * 4,
        "back to the source size"
    );
}

/// `cargo nextest run -p tenta-render --release --run-ignored only --no-capture image_pipeline_timing`
#[test]
#[ignore = "timing run, release only"]
fn image_pipeline_timing() {
    use std::time::Instant;
    // 40 distinct photos of 800x600 drawn at 200x150 (a 4x reduction) and a
    // 1600x900 background, in a 1000x800 viewport.
    let body: String = (0..40u32)
        .map(|i| {
            let photo = png(800, 600, |x, y| {
                [
                    ((x + i) % 256) as u8,
                    (y % 256) as u8,
                    ((x + y) % 256) as u8,
                    255,
                ]
            });
            let url = data_url("image/png", &photo);
            format!("<img src='{url}' style='width:200px;height:150px;display:inline-block'>")
        })
        .collect();
    let backdrop = data_url(
        "image/png",
        &png(1600, 900, |x, y| [(x / 7) as u8, (y / 5) as u8, 90, 255]),
    );
    let doc = parse(&format!(
        "<!doctype html><body style='margin:0;background:url({backdrop}) center/cover'>{body}"
    ));
    let mut session = session(1000, 800);
    let started = Instant::now();
    session.load_inline_images(&doc, &base());
    let load = started.elapsed();
    let mut micros: Vec<f64> = Vec::new();
    for _ in 0..15 {
        let started = Instant::now();
        let _ = pixels(&mut session, &doc);
        micros.push(started.elapsed().as_secs_f64() * 1e6);
    }
    micros.sort_by(f64::total_cmp);
    println!(
        "decode of 41 images (data: URLs, one thread): {:.0} ms; viewport capture p50 {:.0} us, p95 {:.0} us over {} runs; store {} KiB",
        load.as_secs_f64() * 1e3,
        micros[micros.len() / 2],
        micros[micros.len() * 95 / 100],
        micros.len(),
        session.image_stats().store_bytes / 1024
    );
}
