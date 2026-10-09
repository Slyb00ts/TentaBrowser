//! D-036: scripts the bundled fonts lack resolve to platform faces only when a
//! `FontSystem` opts in, and stay `.notdef` otherwise.

#![cfg(feature = "system-fonts")]

use tenta_css::cascade::StyleEnv;
use tenta_css::cascade::test_util::style_html;
use tenta_layout::{BoxTree, FontSystem, FragId, FragmentKind, LayoutCx, NoImages, layout};

const SAMPLES: [(&str, &str); 3] = [
    ("japanese", "日本語のテキスト"),
    ("chinese", "中文文本"),
    ("korean", "한국어텍스트"),
];

/// Glyph counts of the page's text: (total, `.notdef`).
fn glyph_counts(text: &str, mut fonts: FontSystem) -> (usize, usize) {
    let viewport = (800.0, 600.0);
    let env = StyleEnv {
        viewport,
        ..StyleEnv::default()
    };
    let html = format!("<!doctype html><body><p>{text}</p>");
    let (doc, engine) = style_html(&html, env);
    let mut cx = LayoutCx {
        doc: &doc,
        styles: &engine,
        fonts: &mut fonts,
        images: &NoImages,
        viewport,
        quirks: StyleEnv::quirks_of(&doc),
    };
    let tree = BoxTree::build(&cx);
    let frags = layout(&mut cx, &tree);
    let (mut total, mut notdef) = (0, 0);
    for index in 0..frags.len() {
        let fragment = frags.get(FragId(index as u32));
        if let FragmentKind::Text(text) = &fragment.kind {
            for run in &text.runs {
                total += run.glyphs.len();
                notdef += run.glyphs.iter().filter(|glyph| glyph.id == 0).count();
            }
        }
    }
    (total, notdef)
}

#[test]
fn bundled_only_renders_cjk_as_notdef_deterministically() {
    for (name, text) in SAMPLES {
        let (total, notdef) = glyph_counts(text, FontSystem::new_embedded());
        assert!(total > 0, "{name}: no glyphs");
        assert_eq!(notdef, total, "{name}: bundled fonts must not cover it");
        let again = glyph_counts(text, FontSystem::new_embedded());
        assert_eq!((total, notdef), again, "{name}: not deterministic");
    }
}

#[test]
fn system_fallback_finds_a_covering_face() {
    let mut covered = 0;
    for (name, text) in SAMPLES {
        let (total, notdef) = glyph_counts(text, FontSystem::new_with_system_fallback());
        assert!(total > 0, "{name}: no glyphs");
        if notdef == 0 {
            covered += 1;
        } else if cfg!(target_os = "macos") {
            panic!("{name}: {notdef} of {total} glyphs are .notdef with system fonts");
        } else {
            eprintln!("{name}: no covering system font on this machine, skipped");
        }
    }
    eprintln!("{covered} of {} samples covered", SAMPLES.len());
}

#[test]
fn bundled_scripts_do_not_change_with_system_fallback() {
    let embedded = glyph_counts("Hello, world", FontSystem::new_embedded());
    let system = glyph_counts("Hello, world", FontSystem::new_with_system_fallback());
    assert_eq!(embedded, system);
    assert_eq!(embedded.1, 0);
}

#[test]
fn shaping_cache_does_not_leak_between_font_systems() {
    // Same thread, same text: the bundled run caches `.notdef` glyphs, which
    // the system run must not pick up.
    let (_, bundled) = glyph_counts("日本語", FontSystem::new_embedded());
    assert!(bundled > 0);
    let (_, system) = glyph_counts("日本語", FontSystem::new_with_system_fallback());
    assert!(system < bundled || !cfg!(target_os = "macos"));
}
