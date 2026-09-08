//! Preload scanner behaviour (`docs/spec/04-dom-and-css.md` §3.2).

mod common;

use common::tree::Tree;
use tenta_html::{
    ParserOptions, PreloadHint, PreloadKind, PreloadScanner, choose_srcset_candidate,
};

fn scan(html: &str) -> Vec<PreloadHint> {
    PreloadScanner::scan(html, true)
}

fn urls(hints: &[PreloadHint], kind: PreloadKind) -> Vec<&str> {
    hints
        .iter()
        .filter(|hint| hint.kind == kind)
        .map(|hint| hint.url.as_str())
        .collect()
}

#[test]
fn scripts_styles_and_links() {
    let hints = scan(
        r#"<!doctype html><html><head>
        <script src="app.js" defer integrity="sha384-x" crossorigin="anonymous"></script>
        <script src="mod.js" type="module"></script>
        <script src="legacy.js" nomodule async></script>
        <script>var inline = "<script src=fake.js>";</script>
        <link rel="stylesheet" href="site.css" media="screen">
        <link rel="preload" href="hero.woff2" as="font" crossorigin>
        <link rel="preload" href="data.json" as="fetch">
        <link rel="modulepreload" href="chunk.js">
        <link rel="preconnect" href="https://cdn.example">
        <link rel="dns-prefetch" href="https://other.example">
        <link rel="icon" href="favicon.ico">
        </head><body></body></html>"#,
    );

    assert_eq!(urls(&hints, PreloadKind::Script), ["app.js", "legacy.js"]);
    assert_eq!(urls(&hints, PreloadKind::Module), ["mod.js", "chunk.js"]);
    assert_eq!(urls(&hints, PreloadKind::Style), ["site.css"]);
    assert_eq!(urls(&hints, PreloadKind::Font), ["hero.woff2"]);
    assert_eq!(urls(&hints, PreloadKind::Fetch), ["data.json"]);
    assert_eq!(
        urls(&hints, PreloadKind::Preconnect),
        ["https://cdn.example", "https://other.example"]
    );

    let app = &hints[0];
    assert!(app.attrs.defer);
    assert!(!app.attrs.r#async);
    assert_eq!(app.attrs.integrity.as_deref(), Some("sha384-x"));
    assert_eq!(app.attrs.crossorigin.as_deref(), Some("anonymous"));
    let legacy = &hints[2];
    assert!(legacy.attrs.nomodule);
    assert!(legacy.attrs.r#async);
}

#[test]
fn images_pictures_and_media() {
    let hints = scan(
        r#"<img src="a.png">
        <img srcset="b-1x.png 1x, b-2x.png 2x" sizes="100vw" src="fallback.png">
        <img srcset="c-400.png 400w, c-800.png 800w">
        <picture><source srcset="d.avif" type="image/avif"><img src="d.png"></picture>
        <source srcset="outside-picture.png">
        <video poster="poster.jpg" src="movie.mp4"></video>
        <iframe src="frame.html"></iframe>"#,
    );

    assert_eq!(
        urls(&hints, PreloadKind::Image),
        [
            "a.png",
            "b-1x.png",
            "c-400.png",
            "d.avif",
            "d.png",
            "poster.jpg"
        ]
    );
    // R3-15: <iframe> yields no preload hints
    assert_eq!(urls(&hints, PreloadKind::Fetch), &[] as &[&str]);
    assert_eq!(
        hints[1].attrs.srcset.as_deref(),
        Some("b-1x.png 1x, b-2x.png 2x")
    );
    assert_eq!(hints[1].attrs.sizes.as_deref(), Some("100vw"));
}

#[test]
fn base_href_is_reported() {
    let hints = scan("<base href=\"/assets/\"><img src=\"a.png\">");
    assert_eq!(hints[0].kind, PreloadKind::BaseChanged);
    assert_eq!(hints[0].url, "/assets/");
    assert_eq!(hints[1].kind, PreloadKind::Image);
}

#[test]
fn raw_text_and_comments_cannot_fool_the_scanner() {
    let hints = scan(
        r#"<script>document.write('<img src="from-script.png">')</script>
        <style>/* <img src="from-style.png"> */</style>
        <textarea><img src="from-textarea.png"></textarea>
        <title><img src="from-title.png"></title>
        <!-- <img src="from-comment.png"> -->
        <template><img src="from-template.png"><script src="from-template.js"></script></template>
        <xmp><img src="from-xmp.png"></xmp>
        <img src="real.png">"#,
    );
    assert_eq!(urls(&hints, PreloadKind::Image), ["real.png"]);
    assert!(hints.iter().all(|hint| !hint.url.contains("from-")));
}

#[test]
fn noscript_is_raw_text_only_when_scripting_is_enabled() {
    let html = "<noscript><img src=\"fallback.png\"></noscript><img src=\"main.png\">";
    let with_scripting = PreloadScanner::scan(html, true);
    assert_eq!(urls(&with_scripting, PreloadKind::Image), ["main.png"]);

    let without_scripting = PreloadScanner::scan(html, false);
    assert_eq!(
        urls(&without_scripting, PreloadKind::Image),
        ["fallback.png", "main.png"]
    );
}

#[test]
fn hints_arrive_while_streaming() {
    let mut scanner = PreloadScanner::new(true);
    let mut hints = scanner.feed("<html><head><link rel=stylesheet href=a.c");
    assert!(hints.is_empty(), "an incomplete tag is not a hint yet");
    hints.extend(scanner.feed("ss><script src=b.js></script>"));
    hints.extend(scanner.finish());
    assert_eq!(urls(&hints, PreloadKind::Style), ["a.css"]);
    assert_eq!(urls(&hints, PreloadKind::Script), ["b.js"]);
}

#[test]
fn the_parser_reports_hints_through_the_callback() {
    let mut seen: Vec<String> = Vec::new();
    {
        let options = ParserOptions {
            preload: Some(Box::new(|hint: PreloadHint| seen.push(hint.url))),
            ..ParserOptions::default()
        };
        let mut parser = tenta_html::Parser::new(Tree::new(), options);
        parser.feed_str("<link rel=stylesheet href=a.css><img src=b.png>");
        let _ = parser.finish();
    }
    assert_eq!(seen, ["a.css", "b.png"]);
}

#[test]
fn srcset_candidates() {
    assert_eq!(choose_srcset_candidate("a.png"), Some("a.png".to_owned()));
    assert_eq!(
        choose_srcset_candidate("a.png 2x, b.png 1x"),
        Some("b.png".to_owned())
    );
    assert_eq!(
        choose_srcset_candidate(" a.png 400w , b.png 800w "),
        Some("a.png".to_owned())
    );
    assert_eq!(choose_srcset_candidate(""), None);
}

#[test]
fn the_fixture_pages_are_scanned() {
    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");

    let article = std::fs::read_to_string(fixtures.join("static-article/index.html"))
        .expect("the static-article fixture");
    let hints = scan(&article);
    let images = urls(&hints, PreloadKind::Image);
    assert_eq!(images.len(), 12, "{images:?}");
    assert!(images.iter().all(|url| url.starts_with("img/figure-")));
    // The fixture's inline <style> and <script> bodies must not produce hints.
    assert_eq!(hints.len(), images.len());

    let dynamic = std::fs::read_to_string(fixtures.join("dynamic-scripts/index.html"))
        .expect("the dynamic-scripts fixture");
    // Scripts inserted from JavaScript are invisible to a preload scanner.
    assert!(scan(&dynamic).is_empty());
}
