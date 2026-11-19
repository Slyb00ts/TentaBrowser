//! Finds the `@import` targets of a stylesheet so the host can fetch them
//! before the sheet is parsed.
//!
//! This only reads the prologue CSS allows imports in. It does not evaluate
//! the import's `media`, `layer()` or `supports()`: Stylo applies those itself
//! when the sheet is parsed with a loader that serves the prefetched bodies.

use cssparser::{Delimiter, ParseError, Parser, ParserInput, Token};
use url::Url;

/// The absolute URLs of `text`'s `@import` rules, in source order, resolved
/// against `sheet_url`.
///
/// Per CSS Cascade, imports are only valid before every other rule except
/// `@charset` and `@layer` statements, so the scan stops at the first style
/// rule, other at-rule or `@layer` block. A malformed import, or one whose URL
/// does not resolve, is skipped as a browser skips an invalid rule. The same
/// URL may appear more than once; deduplication is the caller's.
pub fn import_urls(text: &str, sheet_url: &Url) -> Vec<Url> {
    let mut found = Vec::new();
    scan_prologue(text, |target| {
        if let Ok(url) = sheet_url.join(target) {
            found.push(url);
        }
    });
    found
}

/// The part of `text` that imports can stand in, verbatim: everything before
/// the first style rule, other at-rule or `@layer` block.
///
/// A sheet that script has edited is handed to the cascade as this prologue
/// followed by its rules as they serialize, because the script view of a
/// sheet has no `@import` rules to serialize.
pub fn import_prologue(text: &str) -> &str {
    &text[..scan_prologue(text, |_| {})]
}

/// Walks the prologue, reporting each import's target, and returns how many
/// bytes of `text` it spans.
fn scan_prologue(text: &str, mut import: impl FnMut(&str)) -> usize {
    let mut input = ParserInput::new(text);
    let mut parser = Parser::new(&mut input);
    let start = parser.position();
    loop {
        let before = parser.position();
        let keyword = match parser.next() {
            Ok(Token::AtKeyword(keyword)) => keyword.to_ascii_lowercase(),
            Ok(Token::CDO | Token::CDC) => continue,
            // A style rule, a stray token or the end of the sheet.
            _ => return parser.slice(start..before).len(),
        };
        match keyword.as_str() {
            "charset" => skip_statement(&mut parser),
            "import" => {
                let target = parser.parse_until_after(Delimiter::Semicolon, |p| {
                    let target = p.expect_url_or_string()?.to_string();
                    // Media, layer and supports conditions are Stylo's to read.
                    while p.next().is_ok() {}
                    Ok::<String, ParseError<'_, ()>>(target)
                });
                if let Ok(target) = target {
                    import(&target);
                }
            }
            "layer" => {
                // `@layer a, b;` is a statement; `@layer a { }` is a rule.
                let is_block = parser
                    .parse_until_before(Delimiter::Semicolon | Delimiter::CurlyBracketBlock, |p| {
                        while p.next().is_ok() {}
                        Ok::<(), ParseError<'_, ()>>(())
                    })
                    .is_ok()
                    && matches!(parser.next(), Ok(Token::CurlyBracketBlock));
                if is_block {
                    return parser.slice(start..before).len();
                }
            }
            _ => return parser.slice(start..before).len(),
        }
    }
}

fn skip_statement(parser: &mut Parser<'_, '_>) {
    let _ = parser.parse_until_after(Delimiter::Semicolon, |p| {
        while p.next().is_ok() {}
        Ok::<(), ParseError<'_, ()>>(())
    });
}
