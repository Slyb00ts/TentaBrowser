//! Family-list parsing and the mapping from CSS family names to the bundled faces.

use std::iter::Peekable;
use std::str::Chars;

use parley::fontique::GenericFamily;

use crate::embedded_fonts::{EMBEDDED_FONTS, EmbeddedFont};

/// A bundled family. The set is closed because the font database is built only
/// from the embedded files.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) enum Family {
    Sans,
    Serif,
    Mono,
    DejaVu,
}

impl Family {
    /// The family name the font files carry, which is also the name registered
    /// with the parley collection.
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Sans => "Liberation Sans",
            Self::Serif => "Liberation Serif",
            Self::Mono => "Liberation Mono",
            Self::DejaVu => "DejaVu Sans",
        }
    }

    /// Whether the bundled files include a real bold face. Without one a
    /// requested bold is synthesised.
    fn has_bold_face(self) -> bool {
        self != Self::DejaVu
    }

    /// The bundled file for this family at regular or bold weight.
    pub(super) fn face(self, bold: bool) -> &'static EmbeddedFont {
        let regular = match self {
            Self::Sans => 0,
            Self::Serif => 2,
            Self::Mono => 4,
            Self::DejaVu => 6,
        };
        &EMBEDDED_FONTS[regular + usize::from(bold && self.has_bold_face())]
    }
}

/// Generic keywords, the parley generic each registers under, and the bundled
/// family it resolves to. DejaVu Sans stands in for `math` and `emoji` because
/// it is the only bundled face with symbol coverage; `cursive` and `fantasy`
/// have no bundled equivalent and take the serif face so the choice is
/// deterministic.
pub(super) const GENERICS: [(&str, GenericFamily, Family); 13] = [
    ("serif", GenericFamily::Serif, Family::Serif),
    ("sans-serif", GenericFamily::SansSerif, Family::Sans),
    ("monospace", GenericFamily::Monospace, Family::Mono),
    ("cursive", GenericFamily::Cursive, Family::Serif),
    ("fantasy", GenericFamily::Fantasy, Family::Serif),
    ("system-ui", GenericFamily::SystemUi, Family::Sans),
    ("ui-serif", GenericFamily::UiSerif, Family::Serif),
    ("ui-sans-serif", GenericFamily::UiSansSerif, Family::Sans),
    ("ui-monospace", GenericFamily::UiMonospace, Family::Mono),
    ("ui-rounded", GenericFamily::UiRounded, Family::Sans),
    ("emoji", GenericFamily::Emoji, Family::DejaVu),
    ("math", GenericFamily::Math, Family::DejaVu),
    ("fangsong", GenericFamily::FangSong, Family::Serif),
];

/// Named families, lowercase. Only names a plain Linux machine would resolve
/// through fontconfig are aliased, so a stack that names other platform fonts
/// falls through to its next entry exactly as it would there. The Liberation
/// faces are metric-compatible with Times New Roman, Arial and Courier New
/// (and so with the Times, Helvetica and Courier names that map to the same
/// files), which keeps line breaks stable. Verdana is not metric-compatible
/// with anything bundled; DejaVu Sans (the Bitstream Vera lineage) is its
/// fontconfig substitute.
const ALIASES: &[(&str, Family)] = &[
    ("liberation sans", Family::Sans),
    ("arial", Family::Sans),
    ("helvetica", Family::Sans),
    ("liberation serif", Family::Serif),
    ("times new roman", Family::Serif),
    ("times", Family::Serif),
    ("liberation mono", Family::Mono),
    ("courier new", Family::Mono),
    ("courier", Family::Mono),
    ("dejavu sans", Family::DejaVu),
    ("verdana", Family::DejaVu),
];

/// Vendor spellings of the `system-ui` generic.
const SYSTEM_UI_ALIASES: [&str; 2] = ["-apple-system", "blinkmacsystemfont"];

/// The family an unresolvable list falls back to: the browser's standard font.
const DEFAULT_FAMILY: Family = Family::Serif;

/// One entry of a CSS `font-family` list.
struct Entry {
    /// Quoted entries are family names even when they spell a generic keyword.
    quoted: bool,
    text: String,
}

/// Reads the character after a backslash: up to six hex digits plus one
/// optional trailing whitespace, or else the next character literally.
fn read_escape(chars: &mut Peekable<Chars<'_>>) -> Option<char> {
    let first = chars.next()?;
    if !first.is_ascii_hexdigit() {
        return Some(first);
    }
    let mut value = first.to_digit(16)?;
    for _ in 0..5 {
        let Some(digit) = chars.peek().and_then(|c| c.to_digit(16)) else {
            break;
        };
        value = value * 16 + digit;
        chars.next();
    }
    if chars.peek().is_some_and(|c| c.is_whitespace()) {
        chars.next();
    }
    Some(
        char::from_u32(value)
            .filter(|c| *c != '\0')
            .unwrap_or('\u{FFFD}'),
    )
}

/// Splits a serialized `font-family` value on top-level commas, unquoting
/// strings, resolving CSS escapes and collapsing whitespace runs in unquoted
/// names.
fn parse_list(list: &str) -> Vec<Entry> {
    let mut entries = Vec::new();
    let mut text = String::new();
    let mut quoted = false;
    let mut quote: Option<char> = None;
    let mut chars = list.chars().peekable();
    let mut flush = |text: &mut String, quoted: &mut bool| {
        let name = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if !name.is_empty() {
            entries.push(Entry {
                quoted: *quoted,
                text: name,
            });
        }
        text.clear();
        *quoted = false;
    };
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (_, '\\') => text.extend(read_escape(&mut chars)),
            (Some(_), c) => text.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                quoted = true;
            }
            (None, ',') => flush(&mut text, &mut quoted),
            (None, c) => text.push(c),
        }
    }
    flush(&mut text, &mut quoted);
    entries
}

fn lookup(entry: &Entry) -> Option<Family> {
    let mut name = entry.text.to_ascii_lowercase();
    if !entry.quoted && SYSTEM_UI_ALIASES.contains(&name.as_str()) {
        name = "system-ui".to_owned();
    }
    if !entry.quoted
        && let Some(&(_, _, family)) = GENERICS.iter().find(|(keyword, _, _)| *keyword == name)
    {
        return Some(family);
    }
    ALIASES
        .iter()
        .find(|(alias, _)| *alias == name)
        .map(|&(_, family)| family)
}

/// The bundled families a list names, in list order without repeats. Names the
/// database does not know are skipped; a list that names nothing known resolves
/// to the standard font.
pub(super) fn resolve_list(list: &str) -> Vec<Family> {
    let mut families: Vec<Family> = Vec::new();
    for family in parse_list(list).iter().filter_map(lookup) {
        if !families.contains(&family) {
            families.push(family);
        }
    }
    if families.is_empty() {
        families.push(DEFAULT_FAMILY);
    }
    families
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &str) -> Vec<&'static str> {
        resolve_list(list).into_iter().map(Family::name).collect()
    }

    #[test]
    fn family_list_resolution_table() {
        let table: &[(&str, &[&str])] = &[
            ("serif", &["Liberation Serif"]),
            ("sans-serif", &["Liberation Sans"]),
            ("monospace", &["Liberation Mono"]),
            ("system-ui", &["Liberation Sans"]),
            ("cursive", &["Liberation Serif"]),
            ("fantasy", &["Liberation Serif"]),
            ("ui-monospace", &["Liberation Mono"]),
            ("emoji", &["DejaVu Sans"]),
            ("Arial", &["Liberation Sans"]),
            ("ARIAL", &["Liberation Sans"]),
            ("Helvetica", &["Liberation Sans"]),
            ("\"Times New Roman\"", &["Liberation Serif"]),
            ("'Times New Roman'", &["Liberation Serif"]),
            ("Times New Roman", &["Liberation Serif"]),
            ("Times  New \t Roman", &["Liberation Serif"]),
            ("Times", &["Liberation Serif"]),
            ("\"Courier New\", monospace", &["Liberation Mono"]),
            ("Courier", &["Liberation Mono"]),
            ("Verdana, sans-serif", &["DejaVu Sans", "Liberation Sans"]),
            (
                "Nonexistent, Arial, serif",
                &["Liberation Sans", "Liberation Serif"],
            ),
            (
                "\"Helvetica Neue\", Helvetica, Arial, sans-serif",
                &["Liberation Sans"],
            ),
            (
                "DejaVu Sans, monospace",
                &["DejaVu Sans", "Liberation Mono"],
            ),
            ("\"Segoe UI\", Georgia, serif", &["Liberation Serif"]),
            ("Menlo, \"Courier New\", monospace", &["Liberation Mono"]),
            ("Tahoma, Roboto, Consolas", &["Liberation Serif"]),
            ("Foo\\ Bar, Arial", &["Liberation Sans"]),
            ("Times\\ New\\ Roman", &["Liberation Serif"]),
            ("\\41 rial", &["Liberation Sans"]),
            ("\\41rial", &["Liberation Sans"]),
            ("\"\\41 rial\"", &["Liberation Sans"]),
            // An escaped comma stays inside one unknown name.
            ("Foo\\, Arial", &["Liberation Serif"]),
            ("\\6d onospace", &["Liberation Mono"]),
            ("mono\\space", &["Liberation Mono"]),
            // Quoting turns a keyword into an ordinary, unknown, family name.
            ("\"serif\", monospace", &["Liberation Mono"]),
            ("\"sans-serif\"", &["Liberation Serif"]),
            (
                "-apple-system, BlinkMacSystemFont, Roboto",
                &["Liberation Sans"],
            ),
            ("\"Comma, Name\", Arial", &["Liberation Sans"]),
            ("Unknown Font", &["Liberation Serif"]),
            ("", &["Liberation Serif"]),
            (",,", &["Liberation Serif"]),
        ];
        assert_eq!(table.len(), 40);
        for (list, expected) in table {
            assert_eq!(&names(list), expected, "family list {list:?}");
        }
    }

    #[test]
    fn dejavu_has_no_bold_face() {
        assert_eq!(Family::DejaVu.face(true).file_name, "DejaVuSans.ttf");
        assert_eq!(Family::Sans.face(true).file_name, "LiberationSans-Bold.ttf");
        assert_eq!(
            Family::Mono.face(false).file_name,
            "LiberationMono-Regular.ttf"
        );
        assert_eq!(
            Family::Serif.face(true).file_name,
            "LiberationSerif-Bold.ttf"
        );
    }
}
