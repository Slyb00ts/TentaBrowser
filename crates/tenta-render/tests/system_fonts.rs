//! D-036: the font policy decides whether scripts the bundled fonts lack are
//! drawn from system fonts. Bundled-only output is byte-stable.
//!
//! `TENTA_RENDER_OUT_DIR=<dir>` also writes both PNGs there for inspection.

use tenta_css::cascade::StyleEnv;
use tenta_css::cascade::test_util::style_html;
use tenta_render::{FontPolicy, RenderConfig, RenderSession, ScreenshotOptions, Viewport};

const HTML: &str = r#"<!doctype html><meta charset="utf-8"><body style="font:20px serif">
<p>English: Hello world</p>
<p>日本語: 日本語のテキスト、ひらがな、カタカナ</p>
<p>中文: 简体中文文本與繁體中文文字</p>
<p>한국어: 한국어 텍스트</p>
<p>Arabic: مرحبا بالعالم</p>
<p>Thai: สวัสดีชาวโลก</p>
<p>Emoji: 😀 🎉 ❤</p>"#;

fn render(fonts: FontPolicy) -> Vec<u8> {
    render_html(HTML, fonts)
}

fn render_html(html: &str, fonts: FontPolicy) -> Vec<u8> {
    let (doc, engine) = style_html(html, StyleEnv::default());
    let mut session = RenderSession::new(RenderConfig {
        viewport: Viewport {
            width: 640,
            height: 330,
            device_pixel_ratio: 1.0,
        },
        fonts,
        ..RenderConfig::default()
    });
    session.set_style_sources(engine.sources().to_vec());
    let shot = session
        .screenshot(&doc, &ScreenshotOptions::default())
        .expect("the screenshot succeeds");
    shot.bytes
}

#[test]
fn font_policy_changes_only_scripts_the_bundle_lacks() {
    let bundled = render(FontPolicy::Embedded);
    assert_eq!(
        bundled,
        render(FontPolicy::Embedded),
        "bundled-only output must be stable"
    );
    let system = render(FontPolicy::SystemFallback);
    if let Some(dir) = std::env::var_os("TENTA_RENDER_OUT_DIR") {
        let dir = std::path::PathBuf::from(dir);
        std::fs::write(dir.join("cjk-bundled.png"), &bundled).expect("write bundled PNG");
        std::fs::write(dir.join("cjk-system.png"), &system).expect("write system PNG");
    }
    if cfg!(target_os = "macos") {
        assert!(bundled != system, "system fonts must change the CJK lines");
    }
}

#[test]
fn latin_only_pages_render_identically_under_both_policies() {
    let html = "<!doctype html><body style=\"font:18px sans-serif\"><p>Hello, world. \u{2713} \u{3b1}\u{3b2}\u{3b3} \u{436}</p>";
    assert!(
        render_html(html, FontPolicy::Embedded) == render_html(html, FontPolicy::SystemFallback),
        "scripts the bundle covers must not change with system fonts"
    );
}
