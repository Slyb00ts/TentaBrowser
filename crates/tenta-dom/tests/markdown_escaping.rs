//! Markdown escaping (`docs/spec/04-dom-and-css.md` §5).
//!
//! Page text is content, not markup. A paragraph that reads `*not emphasis*`
//! or `1. not a list` must render as those literal characters, or an agent
//! reading the extraction sees structure the page did not have. The property
//! asserted here is a round-trip: removing one level of backslash escaping
//! from the rendered Markdown gives back exactly the document's text.

mod support;

use tenta_dom::extract;

/// CommonMark's backslash-escape rule: a backslash before ASCII punctuation
/// escapes it, and the pair stands for the punctuation alone.
fn unescape(markdown: &str) -> String {
    let mut out = String::with_capacity(markdown.len());
    let mut chars = markdown.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some(next) if next.is_ascii_punctuation() => out.push(next),
            Some(next) => {
                out.push('\\');
                out.push(next);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Paragraphs whose text is itself Markdown.
const TEXTS: &[&str] = &[
    "*not emphasis*",
    "_not emphasis either_",
    "**not strong**",
    "`not code`",
    "[not a link](https://example.test/)",
    "![not an image](x.png)",
    "# not a heading",
    "## still not a heading",
    "> not a block quote",
    "- not a bullet",
    "+ not a bullet",
    "1. not an ordered item",
    "42) not an ordered item",
    "| not | a | table |",
    "--- not a rule",
    "=== not a setext underline",
    "a backslash \\ and a pipe |",
    "<not-a-tag> and &amp; already decoded",
    "{braces} and (parens)",
    "plain text with no markup at all",
];

fn page() -> String {
    let mut html = String::from("<!doctype html><html><body>");
    for text in TEXTS {
        // The text is written as an escaped HTML attribute-free run; only `<`
        // and `&` need escaping to reach the DOM unchanged.
        html.push_str("<p>");
        html.push_str(&text.replace('&', "&amp;").replace('<', "&lt;"));
        html.push_str("</p>");
    }
    html.push_str("</body></html>");
    html
}

#[test]
fn escaped_markdown_unescapes_back_to_the_document_text() {
    let document = support::parse(&page());
    let body = document.body().expect("body");
    let markdown = extract::markdown(&document, body);

    let rendered: Vec<String> = markdown
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(unescape)
        .collect();
    assert_eq!(
        rendered.len(),
        TEXTS.len(),
        "one block per paragraph\n--- got ---\n{markdown}"
    );
    for (text, line) in TEXTS.iter().zip(&rendered) {
        assert_eq!(
            line, text,
            "unescaping did not give the text back\n--- got ---\n{markdown}"
        );
    }
}

#[test]
fn block_openers_are_escaped_at_the_start_of_a_line() {
    let document = support::parse(&page());
    let body = document.body().expect("body");
    let markdown = extract::markdown(&document, body);
    let lines: Vec<&str> = markdown
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();

    for line in &lines {
        let first = line.chars().next().expect("non-empty");
        assert!(
            !matches!(first, '#' | '>' | '-' | '+' | '=' | '|'),
            "line {line:?} opens a block the document did not contain"
        );
        if first.is_ascii_digit() {
            let marker = line
                .find(['.', ')'])
                .expect("the fixture's numbered lines carry a marker");
            assert_eq!(
                &line[marker - 1..marker],
                "\\",
                "line {line:?} would open an ordered list"
            );
        }
    }
    assert!(
        lines.iter().any(|line| line.starts_with("1\\.")),
        "an ordered-list marker in text must be escaped\n{markdown}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("\\# not a heading")),
        "a leading # must be escaped\n{markdown}"
    );
}

#[test]
fn real_structure_is_still_rendered_as_structure() {
    // The escaping must not swallow markup the document really has.
    let document = support::parse(
        "<!doctype html><html><body>\
         <h1>Heading</h1><p>Text with <em>emphasis</em>, <strong>strength</strong> \
         and <a href=\"https://example.test/x\">a link</a>.</p>\
         <ul><li>item</li></ul><blockquote><p>quote</p></blockquote></body></html>",
    );
    let markdown = extract::markdown(&document, document.body().expect("body"));
    assert_eq!(
        markdown,
        "# Heading\n\n\
         Text with *emphasis*, **strength** and [a link](https://example.test/x).\n\n\
         - item\n\n\
         > quote\n",
        "\n--- got ---\n{markdown}"
    );
}

#[test]
fn text_inside_a_code_span_is_left_literal() {
    // Inside backticks the delimiters have no meaning, so escaping there would
    // change the code.
    let document = support::parse(
        "<!doctype html><html><body><p>Call <code>a*b_c[d]</code> now.</p>\
         <pre><code>let x = *p; // [note]\n</code></pre></body></html>",
    );
    let markdown = extract::markdown(&document, document.body().expect("body"));
    assert!(
        markdown.contains("`a*b_c[d]`"),
        "code span was escaped\n{markdown}"
    );
    assert!(
        markdown.contains("let x = *p; // [note]"),
        "code block was escaped\n{markdown}"
    );
}
