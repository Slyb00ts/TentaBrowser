//! `text-transform`, applied to the characters before they are shaped.

use std::borrow::Cow;

/// The case-changing values of `text-transform`. `full-width` and
/// `full-size-kana` are not handled.
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
pub(crate) enum TextTransform {
    /// `none`.
    #[default]
    None,
    /// `uppercase`.
    Uppercase,
    /// `lowercase`.
    Lowercase,
    /// `capitalize`.
    Capitalize,
}

/// Applies `transform` with the language-independent Unicode case mappings. The
/// result can be longer than the input (`ß` becomes `SS`), so byte offsets into
/// it are not offsets into the source; the caller keeps its own mapping.
pub(crate) fn apply(text: &str, transform: TextTransform) -> Cow<'_, str> {
    match transform {
        TextTransform::None => Cow::Borrowed(text),
        TextTransform::Uppercase => Cow::Owned(text.to_uppercase()),
        TextTransform::Lowercase => Cow::Owned(text.to_lowercase()),
        TextTransform::Capitalize => {
            let mut out = String::with_capacity(text.len());
            let mut in_word = false;
            for ch in text.chars() {
                if ch.is_alphanumeric() {
                    if in_word {
                        out.push(ch);
                    } else {
                        out.extend(titlecase(ch));
                    }
                    in_word = true;
                } else {
                    out.push(ch);
                    // An apostrophe or a combining-less mark inside a word does
                    // not start a new one; every other separator does.
                    in_word = in_word && matches!(ch, '\'' | '\u{2019}');
                }
            }
            Cow::Owned(out)
        }
    }
}

/// The first letter of a word in uppercase. Digraph letters have a distinct
/// titlecase form that std does not expose, so uppercase stands in for it.
fn titlecase(ch: char) -> impl Iterator<Item = char> {
    ch.to_uppercase()
}
