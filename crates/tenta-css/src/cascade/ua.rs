//! The user-agent stylesheet and its quirks-mode layer.

/// The base user-agent sheet, applicable in every mode.
const UA_SHEET: &str = include_str!("ua.css");

/// The quirks-mode-only layer, applied after the base sheet.
const QUIRKS_SHEET: &str = include_str!("quirks.css");

/// The user-agent sheet texts to parse as `Origin::UserAgent`, in cascade
/// order. The quirks layer is present only for quirks-mode documents.
pub(crate) fn sheet_texts(quirks: bool) -> Vec<&'static str> {
    let mut sheets = vec![UA_SHEET];
    if quirks {
        sheets.push(QUIRKS_SHEET);
    }
    sheets
}

#[cfg(test)]
mod tests {
    use super::sheet_texts;

    #[test]
    fn the_quirks_layer_follows_the_base_sheet_only_in_quirks_mode() {
        assert_eq!(sheet_texts(false).len(), 1);
        assert_eq!(sheet_texts(true).len(), 2);
    }
}
