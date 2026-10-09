//! Font files compiled into the crate (`embedded-fonts`, D-028).
//!
//! The bytes are the upstream files, unmodified. `fonts/README.md` records their
//! source and SHA-256, and `fonts/NOTICE` records their licences.

/// One font file embedded in the crate.
#[derive(Clone, Copy, Debug)]
pub struct EmbeddedFont {
    /// File name under `crates/tenta-layout/fonts/`.
    pub file_name: &'static str,
    /// The complete font file.
    pub bytes: &'static [u8],
}

/// The Liberation faces, then DejaVu Sans as the symbol and coverage fallback.
pub static EMBEDDED_FONTS: &[EmbeddedFont] = &[
    EmbeddedFont {
        file_name: "LiberationSans-Regular.ttf",
        bytes: include_bytes!("../fonts/LiberationSans-Regular.ttf"),
    },
    EmbeddedFont {
        file_name: "LiberationSans-Bold.ttf",
        bytes: include_bytes!("../fonts/LiberationSans-Bold.ttf"),
    },
    EmbeddedFont {
        file_name: "LiberationSerif-Regular.ttf",
        bytes: include_bytes!("../fonts/LiberationSerif-Regular.ttf"),
    },
    EmbeddedFont {
        file_name: "LiberationSerif-Bold.ttf",
        bytes: include_bytes!("../fonts/LiberationSerif-Bold.ttf"),
    },
    EmbeddedFont {
        file_name: "LiberationMono-Regular.ttf",
        bytes: include_bytes!("../fonts/LiberationMono-Regular.ttf"),
    },
    EmbeddedFont {
        file_name: "LiberationMono-Bold.ttf",
        bytes: include_bytes!("../fonts/LiberationMono-Bold.ttf"),
    },
    EmbeddedFont {
        file_name: "DejaVuSans.ttf",
        bytes: include_bytes!("../fonts/DejaVuSans.ttf"),
    },
];

#[cfg(test)]
mod tests {
    use super::EMBEDDED_FONTS;
    use skrifa::attribute::Weight;
    use skrifa::raw::TableProvider;
    use skrifa::string::StringId;
    use skrifa::{FontRef, MetadataProvider};

    /// Expected identity of each embedded file, in the order of `EMBEDDED_FONTS`:
    /// file name, English family name, English subfamily name (name ID 2 for the
    /// Liberation faces, which is what tells a Regular from a Bold file), the
    /// OS/2 weight class as a `Weight`, and `unitsPerEm`. DejaVu Sans names its
    /// regular face subfamily "Book".
    const EXPECTED: &[(&str, &str, &str, Weight, u16)] = &[
        (
            "LiberationSans-Regular.ttf",
            "Liberation Sans",
            "Regular",
            Weight::NORMAL,
            2048,
        ),
        (
            "LiberationSans-Bold.ttf",
            "Liberation Sans",
            "Bold",
            Weight::BOLD,
            2048,
        ),
        (
            "LiberationSerif-Regular.ttf",
            "Liberation Serif",
            "Regular",
            Weight::NORMAL,
            2048,
        ),
        (
            "LiberationSerif-Bold.ttf",
            "Liberation Serif",
            "Bold",
            Weight::BOLD,
            2048,
        ),
        (
            "LiberationMono-Regular.ttf",
            "Liberation Mono",
            "Regular",
            Weight::NORMAL,
            2048,
        ),
        (
            "LiberationMono-Bold.ttf",
            "Liberation Mono",
            "Bold",
            Weight::BOLD,
            2048,
        ),
        (
            "DejaVuSans.ttf",
            "DejaVu Sans",
            "Book",
            Weight::NORMAL,
            2048,
        ),
    ];

    fn english_name(font: &FontRef<'_>, id: StringId) -> Option<String> {
        font.localized_strings(id)
            .english_or_first()
            .map(|name| name.to_string())
    }

    #[test]
    fn every_embedded_font_matches_its_expected_identity() {
        assert_eq!(EMBEDDED_FONTS.len(), EXPECTED.len());
        for (font, &(file_name, family, subfamily, weight, units_per_em)) in
            EMBEDDED_FONTS.iter().zip(EXPECTED)
        {
            assert_eq!(font.file_name, file_name);
            let parsed = FontRef::new(font.bytes)
                .unwrap_or_else(|err| panic!("{file_name} does not parse: {err}"));
            assert_eq!(
                english_name(&parsed, StringId::FAMILY_NAME).as_deref(),
                Some(family),
                "{file_name}: family name"
            );
            assert_eq!(
                english_name(&parsed, StringId::SUBFAMILY_NAME).as_deref(),
                Some(subfamily),
                "{file_name}: subfamily name"
            );
            assert_eq!(
                parsed.attributes().weight,
                weight,
                "{file_name}: OS/2 weight class"
            );
            let head = parsed
                .head()
                .unwrap_or_else(|err| panic!("{file_name} has no head table: {err}"));
            assert_eq!(head.units_per_em(), units_per_em, "{file_name}: unitsPerEm");
        }
    }
}
