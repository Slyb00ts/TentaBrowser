//! Text extraction against the reference fixture
//! (`tests/fixtures/static-article/expected.txt`).

mod support;

use std::path::PathBuf;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/static-article")
        .canonicalize()
        .expect("fixture directory")
}

#[test]
fn static_article_text_matches_the_reference_block_text() {
    let dir = fixture_dir();
    let html = std::fs::read_to_string(dir.join("index.html")).expect("fixture html");
    let expected = std::fs::read_to_string(dir.join("expected.txt")).expect("expected text");
    let document = support::parse_with_url(&html, "http://127.0.0.1/static-article/index.html");

    let text = document.text();
    assert!(
        text.contains("END-OF-ARTICLE-7F3A"),
        "the sentinel of the final paragraph must survive extraction"
    );

    // `expected.txt` is generated from the source blocks, not from the DOM:
    // its first four lines are the navigation anchors listed one per line and
    // it omits the header brand. A DOM extractor sees the anchors as inline
    // content of one <nav> block and does see the brand, so the comparison
    // starts at the <h1>, from where the two agree line for line.
    let anchor = "The Long Read: A Field Guide to Headless Engines";
    let expected_body = expected
        .split_once(anchor)
        .map(|(_, rest)| rest)
        .expect("anchor in expected.txt");
    let actual_body = text
        .split_once(anchor)
        .map(|(_, rest)| rest)
        .expect("anchor in extracted text");

    let expected_lines: Vec<&str> = expected_body.lines().collect();
    let actual_lines: Vec<&str> = actual_body.lines().collect();
    assert_eq!(
        expected_lines.len(),
        actual_lines.len(),
        "block count differs; first difference at {:?}",
        expected_lines
            .iter()
            .zip(actual_lines.iter())
            .position(|(a, b)| a != b)
    );
    for (index, (expected_line, actual_line)) in
        expected_lines.iter().zip(actual_lines.iter()).enumerate()
    {
        assert_eq!(
            expected_line, actual_line,
            "block {index} differs after the article heading"
        );
    }

    // The header is the documented difference; check it explicitly so a
    // regression elsewhere cannot hide behind the offset comparison.
    let head_lines: Vec<&str> = text.lines().take(3).collect();
    assert_eq!(
        head_lines,
        vec![
            "Static Article Fixture",
            "HomeArticlesAboutContact",
            "The Long Read: A Field Guide to Headless Engines",
        ]
    );
}

#[test]
fn static_article_links_are_absolute_and_deduplicated() {
    let dir = fixture_dir();
    let html = std::fs::read_to_string(dir.join("index.html")).expect("fixture html");
    let document = support::parse_with_url(&html, "http://127.0.0.1/static-article/index.html");
    let links = document.links();
    assert!(links.iter().all(|link| link.url.starts_with("http://")));
    let mut urls: Vec<&str> = links.iter().map(|link| link.url.as_str()).collect();
    let count = urls.len();
    urls.sort_unstable();
    urls.dedup();
    assert_eq!(urls.len(), count, "links must be deduplicated by URL");
    assert!(
        links.iter().any(
            |link| link.url == "http://127.0.0.1/static-article/index.html#s1"
                && link.text == "Section 1"
        ),
        "the table of contents links resolve against the document URL"
    );
}

#[test]
fn static_article_metadata_and_main_content() {
    let dir = fixture_dir();
    let html = std::fs::read_to_string(dir.join("index.html")).expect("fixture html");
    let document = support::parse_with_url(&html, "http://127.0.0.1/static-article/index.html");

    let metadata = document.metadata();
    assert_eq!(
        metadata.title.as_deref(),
        Some("The Long Read: A Field Guide to Headless Engines")
    );
    assert_eq!(metadata.lang.as_deref(), Some("en"));
    assert_eq!(
        metadata.meta.get("viewport").map(String::as_str),
        Some("width=device-width, initial-scale=1")
    );

    let main = document.main_content().expect("the article has content");
    assert_eq!(
        document.tag(main.node).map(|tag| tag.as_str()),
        Some("article"),
        "the <article> element wins over <body> and the table of contents"
    );
    assert!(main.text.contains("END-OF-ARTICLE-7F3A"));
    assert!(
        main.link_density < 0.2,
        "link density was {}",
        main.link_density
    );
}

#[test]
fn javascript_urls_are_filtered_even_when_obfuscated() {
    // Each href is a javascript: URL disguised so a raw prefix check misses it:
    // interior ASCII tab/CR/LF and a leading C0 control are all stripped by the
    // WHATWG URL parser, yielding the `javascript` scheme after resolution.
    let html = "\
<!doctype html><html><body>\
<a href=\"javascript:alert(1)\">plain</a>\
<a href=\"JavaScript:alert(1)\">case</a>\
<a href=\"java\tscript:alert(1)\">tab</a>\
<a href=\"java\nscript:alert(1)\">newline</a>\
<a href=\"\u{1}javascript:alert(1)\">control</a>\
<a href=\"  javascript:alert(1)\">leading-space</a>\
<area href=\"java\rscript:alert(1)\">\
<a href=\"/safe\">safe</a>\
<form action=\"java\tscript:alert(1)\"></form>\
</body></html>";
    let document = support::parse_with_url(html, "http://127.0.0.1/page.html");

    let links = document.links();
    assert!(
        links
            .iter()
            .all(|link| !link.url.to_ascii_lowercase().contains("javascript")),
        "no javascript: URL may survive link extraction: {:?}",
        links.iter().map(|l| &l.url).collect::<Vec<_>>()
    );
    assert!(
        links.iter().any(|link| link.url == "http://127.0.0.1/safe"),
        "the safe link must still be extracted"
    );

    let forms = document.forms();
    assert!(
        forms.iter().all(|form| form
            .action
            .as_deref()
            .is_none_or(|action| !action.to_ascii_lowercase().contains("javascript"))),
        "a javascript: form action must fall back to the document URL: {:?}",
        forms.iter().map(|f| &f.action).collect::<Vec<_>>()
    );
}
