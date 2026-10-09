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
///
/// `in_word` says whether the text continues a word begun before it, so that
/// `capitalize` does not start a new word at an inline box boundary inside one,
/// and is updated to say whether the text ends inside a word.
pub(crate) fn apply<'a>(
    text: &'a str,
    transform: TextTransform,
    in_word: &mut bool,
) -> Cow<'a, str> {
    match transform {
        TextTransform::None => {
            *in_word = false;
            Cow::Borrowed(text)
        }
        TextTransform::Uppercase => {
            *in_word = false;
            Cow::Owned(text.to_uppercase())
        }
        TextTransform::Lowercase => {
            *in_word = false;
            Cow::Owned(text.to_lowercase())
        }
        TextTransform::Capitalize => {
            let mut out = String::with_capacity(text.len());
            for ch in text.chars() {
                if ch.is_alphanumeric() {
                    if *in_word {
                        out.push(ch);
                    } else {
                        out.extend(titlecase(ch));
                    }
                    *in_word = true;
                } else {
                    out.push(ch);
                    // An apostrophe or a combining-less mark inside a word does
                    // not start a new one; every other separator does.
                    *in_word = *in_word && matches!(ch, '\'' | '\u{2019}');
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
