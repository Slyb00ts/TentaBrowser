//! Images in screenshots: `<img>` of every supported format, `srcset`, `sizes`
//! and `<picture>`, CSS background images, and what happens when an image
//! fails, all served by the in-process fixture server.

#![cfg(feature = "render")]

use std::io::Cursor;
use std::time::Duration;

use tenta_browser::{
    BrowserContext, Capture, ContextOptions, Engine, Goto, ImageFormat, Page, PageEvent,
    PageOptions, Screenshot, ScreenshotOptions, Settle, Viewport,
};
use tenta_core::{FsPolicy, Tier};
use tenta_testutil::{FixtureServer, FixtureServerConfig, TempDir};

const PATIENCE: Duration = Duration::from_secs(60);

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const ORANGE: [u8; 4] = [255, 128, 0, 255];
const MAGENTA: [u8; 4] = [255, 0, 255, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];

// ---------------------------------------------------------------- encoders

fn png(w: u32, h: u32, pixel: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("png header");
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            data.extend_from_slice(&pixel(x, y));
        }
    }
    writer.write_image_data(&data).expect("png pixels");
    drop(writer);
    out
}

fn png_solid(w: u32, h: u32, rgba: [u8; 4]) -> Vec<u8> {
    png(w, h, |_, _| rgba)
}

fn jpeg_solid(w: u16, h: u16, rgb: [u8; 3]) -> Vec<u8> {
    let data: Vec<u8> = (0..u32::from(w) * u32::from(h)).flat_map(|_| rgb).collect();
    let mut out = Vec::new();
    jpeg_encoder::Encoder::new(&mut out, 100)
        .encode(&data, w, h, jpeg_encoder::ColorType::Rgb)
        .expect("jpeg");
    out
}

fn webp_solid(w: u32, h: u32, rgba: [u8; 4]) -> Vec<u8> {
    let data: Vec<u8> = (0..w * h).flat_map(|_| rgba).collect();
    let mut out = Vec::new();
    image_webp::WebPEncoder::new(&mut out)
        .encode(&data, w, h, image_webp::ColorType::Rgba8)
        .expect("webp");
    out
}

fn gif_solid(w: u16, h: u16, rgb: [u8; 3]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = gif::Encoder::new(&mut out, w, h, &rgb).expect("gif header");
        let frame = gif::Frame {
            width: w,
            height: h,
            buffer: std::borrow::Cow::Owned(vec![0; usize::from(w) * usize::from(h)]),
            ..gif::Frame::default()
        };
        encoder.write_frame(&frame).expect("gif frame");
    }
    out
}

fn svg_rect(w: u32, h: u32, fill: &str) -> String {
    format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='{w}' height='{h}' viewBox='0 0 {w} {h}'>\
         <rect width='{w}' height='{h}' fill='{fill}'/></svg>"
    )
}

/// A PNG that declares `w x h` and carries no pixels: a decoder that checks the
/// header refuses it without allocating.
fn png_header_only(w: u32, h: u32) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IEND", &[]);
    out
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in kind.iter().chain(data) {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    out.extend_from_slice(&(!crc).to_be_bytes());
}

// ---------------------------------------------------------------- the rig

struct Rig {
    server: FixtureServer,
    context: BrowserContext,
    engine: Engine,
    _files: TempDir,
}

impl Rig {
    async fn new(files: Vec<(&str, Vec<u8>)>) -> Rig {
        Self::with_delays(files, &[]).await
    }

    async fn with_delays(files: Vec<(&str, Vec<u8>)>, delays: &[(&str, Duration)]) -> Rig {
        let temp = tenta_testutil::temp_dir("images");
        for (name, contents) in files {
            if let Some((dir, _)) = name.rsplit_once('/') {
                std::fs::create_dir_all(temp.path().join(dir)).expect("subdirectory");
            }
            temp.write(name, contents);
        }
        let mut config = FixtureServerConfig::new(temp.path());
        for (glob, delay) in delays {
            config = config.delay_path(glob, *delay);
        }
        let server = FixtureServer::start(config).await.expect("fixture server");
        let builder = Engine::builder()
            .max_tier(Tier::Render)
            .io_threads(1)
            .page_threads(1)
            .fs_policy(FsPolicy::none().allow_read(temp.path()))
            .ephemeral(true);
        #[cfg(feature = "v8")]
        let builder = builder.js(tenta_browser::JsEngine::V8);
        let engine = builder.build().expect("engine");
        let context = engine
            .new_context(ContextOptions {
                allow_private_network: true,
                ..ContextOptions::default()
            })
            .expect("context");
        Rig {
            server,
            context,
            engine,
            _files: temp,
        }
    }

    async fn page(&self, path: &str, viewport: Viewport) -> Page {
        self.page_with(path, viewport, false).await
    }

    /// A page, with its event subscription made before the navigation.
    async fn page_with_events(
        &self,
        path: &str,
        viewport: Viewport,
    ) -> (Page, tenta_browser::PageEvents) {
        let page = self
            .context
            .new_page(PageOptions {
                viewport,
                ..PageOptions::default()
            })
            .await
            .expect("page");
        let events = page.events();
        page.goto(
            &self.server.url(path),
            Goto::default().wait_until(Settle::Load).timeout(PATIENCE),
        )
        .await
        .expect("navigation");
        (page, events)
    }

    async fn page_with(&self, path: &str, viewport: Viewport, javascript: bool) -> Page {
        let page = self
            .context
            .new_page(PageOptions {
                javascript,
                viewport,
                ..PageOptions::default()
            })
            .await
            .expect("page");
        page.goto(
            &self.server.url(path),
            Goto::default().wait_until(Settle::Load).timeout(PATIENCE),
        )
        .await
        .expect("navigation");
        page
    }

    fn finish(self) {
        self.context.close();
        self.engine.close();
    }
}

fn viewport(width: u32, height: u32, device_scale_factor: f32) -> Viewport {
    Viewport {
        width,
        height,
        device_scale_factor,
        mobile: false,
    }
}

struct Frame {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl Frame {
    fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        assert!(x < self.width && y < self.height, "({x}, {y}) is outside");
        let at = ((y * self.width + x) * 4) as usize;
        self.rgba[at..at + 4].try_into().expect("four channels")
    }

    /// Whether `x, y` is within `tolerance` of `want` per channel, which JPEG
    /// needs.
    fn near(&self, x: u32, y: u32, want: [u8; 4], tolerance: u8) -> bool {
        self.pixel(x, y)
            .iter()
            .zip(want)
            .all(|(a, b)| a.abs_diff(b) <= tolerance)
    }
}

fn decode(shot: &Screenshot) -> Frame {
    let mut reader = png::Decoder::new(Cursor::new(&shot.bytes))
        .read_info()
        .expect("a decodable PNG");
    let mut rgba = vec![0; reader.output_buffer_size().expect("a sized buffer")];
    let info = reader.next_frame(&mut rgba).expect("a frame");
    rgba.truncate(info.buffer_size());
    Frame {
        width: info.width,
        height: info.height,
        rgba,
    }
}

async fn shoot(page: &Page) -> Frame {
    decode(
        &page
            .screenshot(ScreenshotOptions::default())
            .await
            .expect("screenshot"),
    )
}

fn image_failures(events: &mut tenta_browser::PageEvents) -> Vec<String> {
    events
        .drain()
        .into_iter()
        .filter_map(|event| match event {
            PageEvent::ImageFailed { url, .. } => Some(url),
            _ => None,
        })
        .collect()
}

const BLOCK: &str = "display:block;width:10px;height:10px";

fn page_of(body: &str) -> Vec<u8> {
    format!("<!doctype html><html><head><style>html,body{{margin:0}}</style></head><body>{body}</body></html>")
        .into_bytes()
}

// ---------------------------------------------------------------- tests

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_supported_format_is_drawn() {
    let html = page_of(&format!(
        "<img src='a.png' style='{BLOCK}'><img src='b.jpg' style='{BLOCK}'>\
         <img src='c.webp' style='{BLOCK}'><img src='d.gif' style='{BLOCK}'>\
         <img src='e.svg' style='{BLOCK}'>"
    ));
    let rig = Rig::new(vec![
        ("formats.html", html),
        ("a.png", png_solid(10, 10, RED)),
        ("b.jpg", jpeg_solid(10, 10, [0, 255, 0])),
        ("c.webp", webp_solid(10, 10, BLUE)),
        ("d.gif", gif_solid(10, 10, [255, 0, 255])),
        ("e.svg", svg_rect(10, 10, "#ff8000").into_bytes()),
    ])
    .await;
    let page = rig.page("/formats.html", viewport(40, 60, 1.0)).await;
    let mut events = page.events();
    let frame = shoot(&page).await;
    assert_eq!(frame.pixel(5, 5), RED, "png");
    assert!(frame.near(5, 15, GREEN, 6), "jpeg {:?}", frame.pixel(5, 15));
    assert_eq!(frame.pixel(5, 25), BLUE, "webp");
    assert_eq!(frame.pixel(5, 35), MAGENTA, "gif");
    assert_eq!(frame.pixel(5, 45), ORANGE, "svg");
    assert_eq!(frame.pixel(15, 5), WHITE, "outside the images");
    assert!(image_failures(&mut events).is_empty());
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn srcset_sizes_and_picture_choose_by_the_device() {
    let html = page_of(&format!(
        "<img src='lo.png' srcset='lo.png 1x, hi.png 2x' style='{BLOCK}'>\
         <img src='lo.png' srcset='lo.png 100w, hi.png 200w' sizes='100px' style='{BLOCK}'>\
         <picture><source media='(min-width: 500px)' srcset='wide.png'>\
         <source type='image/avif' srcset='nope.png'>\
         <img src='lo.png' style='{BLOCK}'></picture>"
    ));
    let rig = Rig::new(vec![
        ("sets.html", html),
        ("lo.png", png_solid(10, 10, RED)),
        ("hi.png", png_solid(20, 20, BLUE)),
        ("wide.png", png_solid(10, 10, GREEN)),
    ])
    .await;
    // At 1x the 1x candidate and the 100w one are chosen, and the narrow
    // viewport skips the wide source.
    let page = rig.page("/sets.html", viewport(40, 40, 1.0)).await;
    let frame = shoot(&page).await;
    assert_eq!(frame.pixel(5, 5), RED);
    assert_eq!(frame.pixel(5, 15), RED);
    assert_eq!(frame.pixel(5, 25), RED);
    // At 2x the second candidate of each set wins.
    let page = rig.page("/sets.html", viewport(40, 40, 2.0)).await;
    let frame = shoot(&page).await;
    assert_eq!(frame.pixel(10, 10), BLUE);
    assert_eq!(frame.pixel(10, 30), BLUE);
    // A wide viewport matches the picture's first source.
    let page = rig.page("/sets.html", viewport(600, 40, 1.0)).await;
    let frame = shoot(&page).await;
    assert_eq!(frame.pixel(5, 25), GREEN);
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn css_backgrounds_resolve_against_their_stylesheet_and_tile() {
    let html = page_of(
        "<link rel=stylesheet href='css/site.css'>\
         <div id=tile></div><div id=cover></div><div id=inline style=\"width:20px;height:10px;background:url(img/tile.png) no-repeat\"></div>",
    );
    let css = "#tile{width:20px;height:10px;background:url(../img/tile.png) repeat-x}\
               #cover{width:20px;height:10px;background:url(../img/tile.png) 0 0/20px 10px no-repeat}";
    let rig = Rig::new(vec![
        ("bg.html", html),
        ("css/site.css", css.as_bytes().to_vec()),
        // Left half red, right half blue, 10x10.
        (
            "img/tile.png",
            png(10, 10, |x, _| if x < 5 { RED } else { BLUE }),
        ),
    ])
    .await;
    let page = rig.page("/bg.html", viewport(40, 40, 1.0)).await;
    let frame = shoot(&page).await;
    // Repeated horizontally: red, blue, red, blue.
    assert_eq!(frame.pixel(2, 5), RED);
    assert_eq!(frame.pixel(7, 5), BLUE);
    assert_eq!(frame.pixel(12, 5), RED);
    assert_eq!(frame.pixel(17, 5), BLUE);
    // Stretched to 20x10: the halves are 10 wide.
    assert_eq!(frame.pixel(5, 15), RED);
    assert_eq!(frame.pixel(15, 15), BLUE);
    // The inline style resolves against the document.
    assert_eq!(frame.pixel(2, 25), RED);
    assert_eq!(frame.pixel(12, 25), WHITE, "no-repeat");
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failing_image_is_a_broken_image_not_a_failed_screenshot() {
    let html = page_of(
        "<img src='missing.png' alt='gone' style='font:10px sans-serif'>\
         <img src='corrupt.png' width=30 height=20 id=corrupt>\
         <img src='huge.png' width=30 height=20 id=huge>\
         <img src='text.png' width=30 height=20 id=text>",
    );
    let rig = Rig::new(vec![
        ("broken.html", html),
        ("corrupt.png", b"\x89PNG\r\n\x1a\ngarbage".to_vec()),
        ("huge.png", png_header_only(30_000, 30_000)),
        ("text.png", b"<html>not an image</html>".to_vec()),
    ])
    .await;
    // The images fail while the page loads, so the subscription comes first.
    let (page, mut events) = rig
        .page_with_events("/broken.html", viewport(200, 60, 1.0))
        .await;
    let frame = shoot(&page).await;
    let mut failed = image_failures(&mut events);
    failed.sort();
    assert_eq!(failed.len(), 4, "{failed:?}");
    assert!(failed.iter().any(|u| u.ends_with("/missing.png")));
    assert!(failed.iter().any(|u| u.ends_with("/huge.png")));
    // Something is drawn for the alt text, and the boxes keep their frames.
    let ink = (0..60).any(|y| (0..40).any(|x| frame.pixel(x, y) != WHITE));
    assert!(ink, "the alt text and frames are painted");
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_url_is_fetched_once_however_many_elements_use_it() {
    let many = "<img src='one.png' style='display:inline-block;width:2px;height:2px'>".repeat(30);
    let rig = Rig::new(vec![
        ("many.html", page_of(&many)),
        ("one.png", png_solid(2, 2, RED)),
    ])
    .await;
    let page = rig.page("/many.html", viewport(100, 40, 1.0)).await;
    // The document and the one image: they began loading when they entered
    // the document, so `load` has waited for them.
    assert_eq!(rig.server.requests_served(), 2);
    let _ = shoot(&page).await;
    // The capture reuses what the loader installed, and so does the next.
    let _ = shoot(&page).await;
    assert_eq!(rig.server.requests_served(), 2);
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_the_screenshot_cancels_the_image_fetch() {
    let rig = Rig::with_delays(
        vec![
            (
                "slow.html",
                page_of(&format!("<img src='slow.png' style='{BLOCK}'>")),
            ),
            ("slow.png", png_solid(10, 10, RED)),
        ],
        &[("/slow.png", Duration::from_secs(600))],
    )
    .await;
    let page = rig.page("/slow.html", viewport(40, 40, 1.0)).await;
    let started = std::time::Instant::now();
    let outcome = tokio::time::timeout(
        Duration::from_millis(500),
        page.screenshot(ScreenshotOptions {
            capture: Capture::Viewport,
            format: ImageFormat::Png,
            ..ScreenshotOptions::default()
        }),
    )
    .await;
    assert!(outcome.is_err(), "the image never arrives");
    assert!(started.elapsed() < Duration::from_secs(10));
    // The page still answers other work.
    let other = rig.page("/slow.html", viewport(40, 40, 1.0)).await;
    drop(other);
    rig.finish();
}

// ---------------------------------------------------------------- Chrome parity

const QUAD: [[u8; 4]; 4] = [
    [255, 0, 0, 255],
    [0, 160, 0, 255],
    [0, 0, 255, 255],
    [255, 220, 0, 255],
];

/// 20x20, one colour per quadrant: red, green over blue, yellow.
fn quad_pixel(x: u32, y: u32) -> [u8; 4] {
    QUAD[(usize::from(x >= 10)) + 2 * usize::from(y >= 10)]
}

fn quad_svg() -> String {
    String::from(
        "<svg xmlns='http://www.w3.org/2000/svg' width='20' height='20' viewBox='0 0 20 20'>\
         <rect width='10' height='10' fill='#ff0000'/>\
         <rect x='10' width='10' height='10' fill='#00a000'/>\
         <rect y='10' width='10' height='10' fill='#0000ff'/>\
         <rect x='10' y='10' width='10' height='10' fill='#ffdc00'/></svg>",
    )
}

fn quad_gif() -> Vec<u8> {
    let palette: Vec<u8> = QUAD.iter().flat_map(|c| [c[0], c[1], c[2]]).collect();
    let mut out = Vec::new();
    {
        let mut encoder = gif::Encoder::new(&mut out, 20, 20, &palette).expect("gif header");
        let indices: Vec<u8> = (0..400u32)
            .map(|i| (u8::from(i % 20 >= 10)) + 2 * u8::from(i / 20 >= 10))
            .collect();
        let frame = gif::Frame {
            width: 20,
            height: 20,
            buffer: std::borrow::Cow::Owned(indices),
            ..gif::Frame::default()
        };
        encoder.write_frame(&frame).expect("gif frame");
    }
    out
}

/// A smooth 64x64 image: red to blue across, brighter towards the bottom.
fn photo_jpeg() -> Vec<u8> {
    let mut data = Vec::new();
    for y in 0..64u32 {
        for x in 0..64u32 {
            data.extend_from_slice(&[(255 - x * 4) as u8, (y * 3) as u8, (x * 4) as u8]);
        }
    }
    let mut out = Vec::new();
    jpeg_encoder::Encoder::new(&mut out, 95)
        .encode(&data, 64, 64, jpeg_encoder::ColorType::Rgb)
        .expect("jpeg");
    out
}

/// The files of the parity page, by name.
fn parity_files() -> Vec<(&'static str, Vec<u8>)> {
    let webp: Vec<u8> = {
        let data: Vec<u8> = (0..400u32)
            .flat_map(|i| quad_pixel(i % 20, i / 20))
            .collect();
        let mut out = Vec::new();
        image_webp::WebPEncoder::new(&mut out)
            .encode(&data, 20, 20, image_webp::ColorType::Rgba8)
            .expect("webp");
        out
    };
    vec![
        ("parity.html", PARITY_HTML.as_bytes().to_vec()),
        ("quad.png", png(20, 20, quad_pixel)),
        ("quad.svg", quad_svg().into_bytes()),
        ("quad.gif", quad_gif()),
        ("quad.webp", webp),
        (
            "pix.png",
            png(4, 4, |x, y| QUAD[((x + 2 * y) % 4) as usize]),
        ),
        ("alpha.png", png_solid(8, 8, [255, 0, 255, 128])),
        ("photo.jpg", photo_jpeg()),
    ]
}

/// Cell `i` sits at column `i % 4`, row `i / 4` of a 120 px pitch.
const PARITY_HTML: &str = "<!doctype html><html><head><style>\
html,body{margin:0;background:#fff}\
.c{position:absolute;width:100px;height:100px}\
img.c{display:block}\
.g{background-color:#eee}\
</style></head><body>\
<img class=c style='left:10px;top:10px;object-fit:fill' src=quad.png>\
<img class=c style='left:130px;top:10px;height:50px;object-fit:contain' src=quad.png>\
<img class=c style='left:250px;top:10px;height:50px;object-fit:cover' src=quad.png>\
<img class=c style='left:370px;top:10px;object-fit:none' src=quad.png>\
<img class=c style='left:10px;top:130px;object-fit:scale-down' src=quad.png>\
<img class=c style='left:130px;top:130px;object-fit:none;object-position:0 0' src=quad.png>\
<img class=c style='left:250px;top:130px;image-rendering:pixelated' src=pix.png>\
<img class=c style='left:370px;top:130px' src=quad.png>\
<img class=c style='left:10px;top:250px;border-radius:50%' src=quad.png>\
<img class=c style='left:130px;top:250px;opacity:.5' src=quad.png>\
<div class=c style='left:250px;top:250px;background:#00f'></div><img class=c style='left:250px;top:250px' src=alpha.png>\
<img class=c style='left:370px;top:250px' src=quad.svg>\
<div class=c style='left:10px;top:370px;background:url(quad.png) 0 0/50px 50px'></div>\
<div class=c style='left:130px;top:370px;background:url(quad.png) no-repeat 0 0/cover'></div>\
<div class=c style='left:250px;top:370px;background:url(quad.png) no-repeat center/60px 40px'></div>\
<div class=c style='left:370px;top:370px;background:url(quad.png) repeat-x 0 50px/25px 25px'></div>\
<div class=c style='left:10px;top:490px;background:url(quad.png) space 0 0/30px 30px'></div>\
<div class=c style='left:130px;top:490px;background:url(quad.png) round 0 0/45px 45px'></div>\
<div class=c style='left:250px;top:490px;background:linear-gradient(to bottom,rgba(0,0,0,.5),rgba(0,0,0,0)),url(quad.png) 0 0/100px 100px'></div>\
<div class=c style='left:370px;top:490px;box-sizing:content-box;width:60px;height:60px;padding:10px;border:10px solid rgba(0,0,255,.3);background:url(quad.png) no-repeat 0 0/40px 40px content-box,#eee;background-origin:content-box;background-clip:padding-box'></div>\
<img class=c style='left:10px;top:610px' src=photo.jpg>\
<img class=c style='left:130px;top:610px' src=quad.webp>\
<img class=c style='left:250px;top:610px' src=quad.gif>\
<div class=c style='left:370px;top:610px;background:url(quad.svg) 0 0/100px 100px'></div>\
<div class=c style='left:10px;top:730px;background:linear-gradient(45deg,#f00,#00f)'></div>\
<div class=c style='left:130px;top:730px;background:repeating-linear-gradient(90deg,#f00 0 10px,#00f 10px 20px)'></div>\
<div class=c style='left:250px;top:730px;background:linear-gradient(to right,#f00,transparent)'></div>\
<div class=c style='left:370px;top:730px;border-radius:30px;background:url(quad.png) 0 0/100px 100px'></div>\
</body></html>";

/// Where the probe grid of cell `i` samples: a 4 x 4 grid inside the cell,
/// clear of the edges that resampling and anti-aliasing blur.
fn probes(cell: usize) -> Vec<(u32, u32)> {
    let origin = (10 + 120 * (cell as u32 % 4), 10 + 120 * (cell as u32 / 4));
    let mut points = Vec::new();
    for dy in [12, 32, 68, 88] {
        for dx in [12, 32, 68, 88] {
            points.push((origin.0 + dx, origin.1 + dy));
        }
    }
    points
}

/// Writes the parity page and its files to `$PARITY_DIR`, for recording the
/// reference with Chrome. Not a test: run it by hand.
#[test]
#[ignore = "writes the files Chrome records its reference from"]
fn write_the_parity_page() {
    let dir = std::path::PathBuf::from(std::env::var("PARITY_DIR").expect("PARITY_DIR"));
    std::fs::create_dir_all(&dir).expect("directory");
    for (name, bytes) in parity_files() {
        std::fs::write(dir.join(name), bytes).expect("file");
    }
    let points: Vec<Vec<(u32, u32)>> = (0..28).map(probes).collect();
    std::fs::write(
        dir.join("probes.json"),
        format!("{points:?}").replace('(', "[").replace(')', "]"),
    )
    .expect("probes");
}
/// Colours Chrome 154 drew at the probe points of each cell of the parity page
/// (`probes`, row-major), as `[r, g, b]` over the white page.
const CHROME: [[[u8; 3]; 16]; 28] = [
    [
        [255, 0, 0],
        [255, 0, 0],
        [0, 160, 0],
        [0, 160, 0],
        [255, 0, 0],
        [255, 0, 0],
        [0, 160, 0],
        [0, 160, 0],
        [0, 0, 255],
        [0, 0, 255],
        [255, 220, 0],
        [255, 220, 0],
        [0, 0, 255],
        [0, 0, 255],
        [255, 220, 0],
        [255, 220, 0],
    ],
    [
        [255, 255, 255],
        [255, 0, 0],
        [0, 160, 0],
        [255, 255, 255],
        [255, 255, 255],
        [0, 0, 255],
        [255, 220, 0],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
    ],
    [
        [255, 0, 0],
        [255, 0, 0],
        [0, 160, 0],
        [0, 160, 0],
        [0, 0, 255],
        [0, 0, 255],
        [255, 220, 0],
        [255, 220, 0],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
    ],
    [
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
    ],
    [
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
    ],
    [
        [255, 220, 0],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
    ],
    [
        [255, 0, 0],
        [0, 160, 0],
        [0, 0, 255],
        [255, 220, 0],
        [0, 0, 255],
        [255, 220, 0],
        [255, 0, 0],
        [0, 160, 0],
        [255, 0, 0],
        [0, 160, 0],
        [0, 0, 255],
        [255, 220, 0],
        [0, 0, 255],
        [255, 220, 0],
        [255, 0, 0],
        [0, 160, 0],
    ],
    [
        [255, 0, 0],
        [255, 0, 0],
        [0, 160, 0],
        [0, 160, 0],
        [255, 0, 0],
        [255, 0, 0],
        [0, 160, 0],
        [0, 160, 0],
        [0, 0, 255],
        [0, 0, 255],
        [255, 220, 0],
        [255, 220, 0],
        [0, 0, 255],
        [0, 0, 255],
        [255, 220, 0],
        [255, 220, 0],
    ],
    [
        [255, 255, 255],
        [255, 0, 0],
        [0, 160, 0],
        [255, 255, 255],
        [255, 0, 0],
        [255, 0, 0],
        [0, 160, 0],
        [0, 160, 0],
        [0, 0, 255],
        [0, 0, 255],
        [255, 220, 0],
        [255, 220, 0],
        [255, 255, 255],
        [0, 0, 255],
        [255, 220, 0],
        [255, 255, 255],
    ],
    [
        [255, 127, 127],
        [255, 127, 127],
        [127, 207, 127],
        [127, 207, 127],
        [255, 127, 127],
        [255, 127, 127],
        [127, 207, 127],
        [127, 207, 127],
        [127, 127, 255],
        [127, 127, 255],
        [255, 237, 127],
        [255, 237, 127],
        [127, 127, 255],
        [127, 127, 255],
        [255, 237, 127],
        [255, 237, 127],
    ],
    [
        [128, 0, 255],
        [128, 0, 255],
        [128, 0, 255],
        [128, 0, 255],
        [128, 0, 255],
        [128, 0, 255],
        [128, 0, 255],
        [128, 0, 255],
        [128, 0, 255],
        [128, 0, 255],
        [128, 0, 255],
        [128, 0, 255],
        [128, 0, 255],
        [128, 0, 255],
        [128, 0, 255],
        [128, 0, 255],
    ],
    [
        [255, 0, 0],
        [255, 0, 0],
        [0, 160, 0],
        [0, 160, 0],
        [255, 0, 0],
        [255, 0, 0],
        [0, 160, 0],
        [0, 160, 0],
        [0, 0, 255],
        [0, 0, 255],
        [255, 220, 0],
        [255, 220, 0],
        [0, 0, 255],
        [0, 0, 255],
        [255, 220, 0],
        [255, 220, 0],
    ],
    [
        [255, 0, 0],
        [0, 160, 0],
        [255, 0, 0],
        [0, 160, 0],
        [0, 0, 255],
        [255, 220, 0],
        [0, 0, 255],
        [255, 220, 0],
        [255, 0, 0],
        [0, 160, 0],
        [255, 0, 0],
        [0, 160, 0],
        [0, 0, 255],
        [255, 220, 0],
        [0, 0, 255],
        [255, 220, 0],
    ],
    [
        [255, 0, 0],
        [255, 0, 0],
        [0, 160, 0],
        [0, 160, 0],
        [255, 0, 0],
        [255, 0, 0],
        [0, 160, 0],
        [0, 160, 0],
        [0, 0, 255],
        [0, 0, 255],
        [255, 220, 0],
        [255, 220, 0],
        [0, 0, 255],
        [0, 0, 255],
        [255, 220, 0],
        [255, 220, 0],
    ],
    [
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 0, 0],
        [0, 160, 0],
        [255, 255, 255],
        [255, 255, 255],
        [0, 0, 255],
        [255, 220, 0],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
    ],
    [
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [127, 110, 128],
        [0, 0, 255],
        [255, 220, 0],
        [255, 220, 0],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
    ],
    [
        [255, 0, 0],
        [255, 255, 255],
        [255, 255, 255],
        [0, 160, 0],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [0, 0, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 220, 0],
    ],
    [
        [255, 0, 0],
        [0, 160, 0],
        [255, 0, 0],
        [0, 160, 0],
        [0, 0, 255],
        [255, 220, 0],
        [0, 0, 255],
        [255, 220, 0],
        [255, 0, 0],
        [0, 160, 0],
        [255, 0, 0],
        [0, 160, 0],
        [0, 0, 255],
        [255, 220, 0],
        [0, 0, 255],
        [255, 220, 0],
    ],
    [
        [143, 0, 0],
        [143, 0, 0],
        [0, 90, 0],
        [0, 90, 0],
        [169, 0, 0],
        [169, 0, 0],
        [0, 106, 0],
        [0, 106, 0],
        [0, 0, 215],
        [0, 0, 215],
        [215, 185, 0],
        [215, 185, 0],
        [0, 0, 240],
        [0, 0, 240],
        [240, 207, 0],
        [240, 207, 0],
    ],
    [
        [238, 238, 238],
        [238, 238, 238],
        [238, 238, 238],
        [238, 238, 238],
        [238, 238, 238],
        [255, 0, 0],
        [238, 238, 238],
        [238, 238, 238],
        [238, 238, 238],
        [238, 238, 238],
        [238, 238, 238],
        [238, 238, 238],
        [238, 238, 238],
        [238, 238, 238],
        [238, 238, 238],
        [238, 238, 238],
    ],
    [
        [224, 22, 29],
        [175, 21, 81],
        [81, 23, 173],
        [31, 21, 223],
        [225, 60, 28],
        [174, 60, 80],
        [81, 61, 173],
        [30, 60, 224],
        [225, 130, 30],
        [174, 129, 80],
        [81, 129, 173],
        [30, 129, 225],
        [226, 168, 29],
        [174, 168, 79],
        [82, 168, 173],
        [30, 168, 223],
    ],
    [
        [255, 0, 0],
        [255, 0, 0],
        [0, 160, 0],
        [0, 160, 0],
        [255, 0, 0],
        [255, 0, 0],
        [0, 160, 0],
        [0, 160, 0],
        [0, 0, 255],
        [0, 0, 255],
        [255, 220, 0],
        [255, 220, 0],
        [0, 0, 255],
        [0, 0, 255],
        [255, 220, 0],
        [255, 220, 0],
    ],
    [
        [255, 0, 0],
        [255, 0, 0],
        [0, 160, 0],
        [0, 160, 0],
        [255, 0, 0],
        [255, 0, 0],
        [0, 160, 0],
        [0, 160, 0],
        [0, 0, 255],
        [0, 0, 255],
        [255, 220, 0],
        [255, 220, 0],
        [0, 0, 255],
        [0, 0, 255],
        [255, 220, 0],
        [255, 220, 0],
    ],
    [
        [255, 0, 0],
        [255, 0, 0],
        [0, 160, 0],
        [0, 160, 0],
        [255, 0, 0],
        [255, 0, 0],
        [0, 160, 0],
        [0, 160, 0],
        [0, 0, 255],
        [0, 0, 255],
        [255, 220, 0],
        [255, 220, 0],
        [0, 0, 255],
        [0, 0, 255],
        [255, 220, 0],
        [255, 220, 0],
    ],
    [
        [127, 0, 127],
        [102, 0, 153],
        [56, 0, 199],
        [30, 0, 224],
        [153, 0, 102],
        [127, 0, 127],
        [81, 0, 173],
        [56, 0, 199],
        [199, 0, 56],
        [173, 0, 81],
        [127, 0, 127],
        [102, 0, 153],
        [224, 0, 30],
        [199, 0, 56],
        [153, 0, 102],
        [127, 0, 127],
    ],
    [
        [0, 0, 255],
        [0, 0, 255],
        [255, 0, 0],
        [255, 0, 0],
        [0, 0, 255],
        [0, 0, 255],
        [255, 0, 0],
        [255, 0, 0],
        [0, 0, 255],
        [0, 0, 255],
        [255, 0, 0],
        [255, 0, 0],
        [0, 0, 255],
        [0, 0, 255],
        [255, 0, 0],
        [255, 0, 0],
    ],
    [
        [255, 32, 32],
        [255, 83, 83],
        [255, 175, 175],
        [255, 226, 226],
        [255, 32, 32],
        [255, 83, 83],
        [255, 175, 175],
        [255, 226, 226],
        [255, 32, 32],
        [255, 83, 83],
        [255, 175, 175],
        [255, 226, 226],
        [255, 32, 32],
        [255, 83, 83],
        [255, 175, 175],
        [255, 226, 226],
    ],
    [
        [255, 0, 0],
        [255, 0, 0],
        [0, 160, 0],
        [0, 160, 0],
        [255, 0, 0],
        [255, 0, 0],
        [0, 160, 0],
        [0, 160, 0],
        [0, 0, 255],
        [0, 0, 255],
        [255, 220, 0],
        [255, 220, 0],
        [0, 0, 255],
        [0, 0, 255],
        [255, 220, 0],
        [255, 220, 0],
    ],
];

/// Per-channel tolerance for cell `i`: a JPEG decodes with different rounding,
/// and gradients and scaled images differ by a unit or two in how they
/// resample.
fn parity_tolerance(cell: usize) -> u8 {
    match cell {
        20 => 12,
        _ => 5,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn images_match_chrome_pixels_at_known_points() {
    let rig = Rig::new(parity_files()).await;
    let page = rig.page("/parity.html", viewport(490, 860, 1.0)).await;
    let mut events = page.events();
    let frame = shoot(&page).await;
    assert_eq!((frame.width, frame.height), (490, 860));
    assert!(image_failures(&mut events).is_empty());
    let mut mismatches = Vec::new();
    for (cell, expected) in CHROME.iter().enumerate() {
        let tolerance = parity_tolerance(cell);
        for (&(x, y), want) in probes(cell).iter().zip(expected) {
            let got = frame.pixel(x, y);
            if got[..3]
                .iter()
                .zip(want)
                .any(|(a, b)| a.abs_diff(*b) > tolerance)
            {
                mismatches.push(format!(
                    "cell {cell} at ({x}, {y}): got {:?}, Chrome {want:?}",
                    &got[..3]
                ));
            }
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} of {} probes differ:\n{}",
        mismatches.len(),
        CHROME.len() * 16,
        mismatches.join("\n")
    );
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn images_are_fetched_when_they_enter_the_document_not_at_the_capture() {
    let rig = Rig::new(vec![
        (
            "late.html",
            page_of(&format!("<img src='late.png' style='{BLOCK}'>")),
        ),
        ("late.png", png_solid(10, 10, RED)),
    ])
    .await;
    let page = rig.page("/late.html", viewport(40, 40, 1.0)).await;
    // `goto` waited for `load`, which waits for the images: the document and
    // the image have been requested already.
    let after_load = rig.server.requests_served();
    assert_eq!(after_load, 2, "the document and its image");
    let frame = shoot(&page).await;
    assert_eq!(frame.pixel(5, 5), RED);
    assert_eq!(
        rig.server.requests_served(),
        after_load,
        "the capture fetches nothing"
    );
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lazy_images_far_below_the_viewport_load_only_for_a_full_page_capture() {
    let html = page_of(&format!(
        "<img src='top.png' loading=lazy style='{BLOCK}'><div style='height:6000px'></div>\
         <img src='bottom.png' loading=lazy style='{BLOCK}'>"
    ));
    let rig = Rig::new(vec![
        ("lazy.html", html),
        ("top.png", png_solid(10, 10, RED)),
        ("bottom.png", png_solid(10, 10, BLUE)),
    ])
    .await;
    let page = rig.page("/lazy.html", viewport(40, 40, 1.0)).await;
    let before = rig.server.requests_served();
    let _ = shoot(&page).await;
    assert_eq!(
        rig.server.requests_served() - before,
        1,
        "only the image near the viewport"
    );
    let full = decode(
        &page
            .screenshot(ScreenshotOptions::full_page())
            .await
            .expect("full page"),
    );
    assert_eq!(
        rig.server.requests_served() - before,
        2,
        "the far one for a full page"
    );
    assert_eq!(full.pixel(5, 6015), BLUE);
    rig.finish();
}

#[cfg(feature = "v8")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_image_a_script_adds_is_loaded_and_drawn() {
    let html = page_of(
        "<script>\
         var img = document.createElement('img');\
         img.setAttribute('src', 'made.png');\
         img.setAttribute('style', 'display:block;width:10px;height:10px');\
         document.body.appendChild(img);\
         </script>",
    );
    let rig = Rig::new(vec![
        ("made.html", html),
        ("made.png", png_solid(10, 10, GREEN)),
    ])
    .await;
    let page = rig
        .page_with("/made.html", viewport(40, 40, 1.0), true)
        .await;
    let frame = shoot(&page).await;
    assert_eq!(frame.pixel(5, 5), GREEN);
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_navigation_frees_the_old_documents_render_session_at_commit() {
    let rig = Rig::new(vec![
        (
            "one.html",
            page_of(&format!("<img src='one.png' style='{BLOCK}'>")),
        ),
        ("two.html", page_of("<p>plain</p>")),
        ("one.png", png_solid(10, 10, RED)),
    ])
    .await;
    let page = rig.page("/one.html", viewport(40, 40, 1.0)).await;
    let _ = shoot(&page).await;
    assert!(
        page.render_counters().await.expect("counters").is_some(),
        "the session holds the loaded image"
    );
    page.goto(
        &rig.server.url("/two.html"),
        Goto::default().wait_until(Settle::Load).timeout(PATIENCE),
    )
    .await
    .expect("navigation");
    // No screenshot since: the old session, its pinned images and its retained
    // bodies are gone already.
    assert!(page.render_counters().await.expect("counters").is_none());
    rig.finish();
}
