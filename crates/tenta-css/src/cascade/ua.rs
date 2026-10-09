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

#[cfg(feature = "stylo")]
#[cfg(test)]
mod stylo_tests {
    use std::cell::RefCell;

    use cssparser::{SourceLocation, ToCss};
    use style::context::QuirksMode;
    use style::error_reporting::{ContextualParseError, ParseErrorReporter};
    use style::media_queries::MediaList;
    use style::shared_lock::SharedRwLock;
    use style::stylesheets::{AllowImportRules, CssRule, Origin, Stylesheet, UrlExtraData};
    use url::Url;

    use super::{QUIRKS_SHEET, UA_SHEET, sheet_texts};

    /// Collects every parse error Stylo reports, so a dropped rule fails a test.
    #[derive(Default)]
    struct ErrorCollector(RefCell<Vec<String>>);

    impl ParseErrorReporter for ErrorCollector {
        fn report_error(
            &self,
            _url: &UrlExtraData,
            location: SourceLocation,
            error: ContextualParseError,
        ) {
            self.0
                .borrow_mut()
                .push(format!("{}:{}: {error:?}", location.line, location.column));
        }
    }

    /// One style rule as Stylo serialises it: the selector items and the
    /// declarations as (property, serialised value) pairs, `!important`
    /// included in the value.
    struct StyleRule {
        selectors: Vec<String>,
        declarations: Vec<(String, String)>,
    }

    /// Parses `css` as a user-agent sheet and returns it with its reported errors.
    fn parse_ua(css: &str) -> (Stylesheet, Vec<String>) {
        crate::cssom::configure_stylo();
        let lock = SharedRwLock::new();
        let media = servo_arc::Arc::new(lock.wrap(MediaList::empty()));
        let url = Url::parse("about:tenta-user-agent").expect("static URL parses");
        let reporter = ErrorCollector::default();
        let sheet = Stylesheet::from_str(
            css,
            UrlExtraData::from(url),
            Origin::UserAgent,
            media,
            lock,
            None,
            Some(&reporter),
            QuirksMode::NoQuirks,
            AllowImportRules::No,
        );
        let errors = reporter.0.into_inner();
        (sheet, errors)
    }

    /// Splits a serialised declaration block (`a: b; c: d;`) into pairs. The
    /// sheets contain no `;` inside values, so a plain split is exact.
    fn split_declarations(block: &str) -> Vec<(String, String)> {
        block
            .split(';')
            .filter_map(|declaration| {
                let (property, value) = declaration.split_once(':')?;
                Some((property.trim().to_owned(), value.trim().to_owned()))
            })
            .collect()
    }

    fn style_rules(css: &str) -> Vec<StyleRule> {
        let (sheet, _) = parse_ua(css);
        let guard = sheet.shared_lock.read();
        let contents = sheet.contents.read_with(&guard);
        contents
            .rules(&guard)
            .iter()
            .filter_map(|rule| match rule {
                CssRule::Style(style) => {
                    let style = style.read_with(&guard);
                    let mut block = String::new();
                    style
                        .block
                        .read_with(&guard)
                        .to_css(&mut block)
                        .expect("writing to a String cannot fail");
                    // Serialised attribute values may be quoted; assertions use
                    // the unquoted spelling written in the sheet.
                    let selectors =
                        selector_items(&style.selectors.to_css_string().replace('"', ""));
                    Some(StyleRule {
                        selectors,
                        declarations: split_declarations(&block),
                    })
                }
                _ => None,
            })
            .collect()
    }

    /// The top-level items of a serialised selector list, split on commas that
    /// are not inside `(...)` or `[...]`.
    fn selector_items(list: &str) -> Vec<String> {
        let mut items = Vec::new();
        let (mut depth, mut start) = (0usize, 0usize);
        for (index, byte) in list.bytes().enumerate() {
            match byte {
                b'(' | b'[' => depth += 1,
                b')' | b']' => depth = depth.saturating_sub(1),
                b',' if depth == 0 => {
                    items.push(list[start..index].trim().to_owned());
                    start = index + 1;
                }
                _ => {}
            }
        }
        items.push(list[start..].trim().to_owned());
        items
    }

    /// Whether some rule whose selector list has `selector` as a top-level item
    /// declares exactly `property: value`.
    fn declares(rules: &[StyleRule], selector: &str, property: &str, value: &str) -> bool {
        rules.iter().any(|rule| {
            rule.selectors.iter().any(|item| item == selector)
                && rule
                    .declarations
                    .iter()
                    .any(|(p, v)| p == property && v == value)
        })
    }

    fn assert_declares(rules: &[StyleRule], selector: &str, property: &str, value: &str) {
        assert!(
            declares(rules, selector, property, value),
            "no rule `{selector}` declares `{property}: {value}`; rules for it: {:?}",
            rules
                .iter()
                .filter(|rule| rule.selectors.iter().any(|item| item == selector))
                .map(|rule| &rule.declarations)
                .collect::<Vec<_>>()
        );
    }

    fn assert_parses_cleanly(css: &str, name: &str) {
        let (_, errors) = parse_ua(css);
        assert!(
            errors.is_empty(),
            "{name} reported parse errors: {errors:#?}"
        );
    }

    fn ua_rules() -> Vec<StyleRule> {
        style_rules(UA_SHEET)
    }

    #[test]
    fn the_base_sheet_parses_without_errors() {
        assert_parses_cleanly(UA_SHEET, "ua.css");
    }

    #[test]
    fn the_quirks_sheet_parses_without_errors() {
        assert_parses_cleanly(QUIRKS_SHEET, "quirks.css");
        assert_eq!(sheet_texts(true).len(), 2);
    }

    #[test]
    fn the_error_collector_sees_bogus_css() {
        let (_, errors) =
            parse_ua("a { color: ; } b { no-such-property: 1 } c { margin: 1px 2px 3px 4px 5px }");
        assert!(
            errors.len() >= 2,
            "expected at least two errors: {errors:#?}"
        );
    }

    #[test]
    fn the_declaration_matcher_rejects_a_wrong_value() {
        let rules = ua_rules();
        assert_declares(&rules, "body", "margin", "8px");
        assert!(!declares(&rules, "body", "margin", "9px"));
        assert!(!declares(&rules, "body", "padding", "8px"));
        assert!(!declares(&rules, "no-such-element", "margin", "8px"));
    }

    #[test]
    fn non_rendered_elements_are_hidden() {
        let rules = ua_rules();
        for name in ["head", "script", "style", "title", "template", "area"] {
            assert_declares(&rules, name, "display", "none");
        }
        assert_declares(
            &rules,
            "[hidden]:not([hidden=until-found i])",
            "display",
            "none",
        );
        assert_declares(&rules, "input[type=hidden i]", "display", "none !important");
        assert_declares(
            &rules,
            "audio:not([controls])",
            "display",
            "none !important",
        );
    }

    #[test]
    fn noscript_is_hidden_unconditionally() {
        assert_declares(&ua_rules(), "noscript", "display", "none !important");
    }

    #[test]
    fn closed_details_content_is_removed_from_the_box_tree() {
        assert_declares(
            &ua_rules(),
            "details:not([open]) > :not(summary:first-of-type)",
            "display",
            "none !important",
        );
    }

    #[test]
    fn an_open_dialog_is_a_centred_absolutely_positioned_box() {
        let rules = ua_rules();
        assert_declares(&rules, "dialog:not([open])", "display", "none");
        for (property, value) in [
            ("position", "absolute"),
            ("inset-inline", "0px"),
            ("width", "fit-content"),
            ("height", "fit-content"),
            ("margin", "auto"),
            ("padding", "1em"),
            ("background-color", "canvas"),
            ("color", "canvastext"),
        ] {
            assert_declares(&rules, "dialog", property, value);
        }
    }

    #[test]
    fn the_page_has_an_eight_pixel_body_margin() {
        assert_declares(&ua_rules(), "body", "margin", "8px");
    }

    #[test]
    fn headings_use_flat_sizes_and_margins() {
        let rules = ua_rules();
        assert_declares(&rules, "h1", "font-size", "2em");
        assert_declares(&rules, "h1", "margin-block", "0.67em");
        assert_declares(&rules, "h2", "font-size", "1.5em");
    }

    #[test]
    fn list_items_are_list_items_and_lists_are_indented() {
        let rules = ua_rules();
        assert_declares(&rules, "li", "display", "list-item");
        assert_declares(&rules, "ul", "padding-inline-start", "40px");
        assert_declares(&rules, "ol", "list-style-type", "decimal");
    }

    #[test]
    fn nested_lists_lose_their_block_margins_and_change_markers() {
        let rules = ua_rules();
        assert_declares(
            &rules,
            ":is(dir, dl, menu, ol, ul) :is(dir, dl, menu, ol, ul)",
            "margin-block",
            "0px",
        );
        assert_declares(
            &rules,
            ":is(dir, menu, ol, ul) :is(dir, menu, ul)",
            "list-style-type",
            "circle",
        );
        assert_declares(
            &rules,
            ":is(dir, menu, ol, ul) :is(dir, menu, ol, ul) :is(dir, menu, ul)",
            "list-style-type",
            "square",
        );
    }

    #[test]
    fn tables_are_tables_with_two_pixel_spacing_and_one_pixel_cells() {
        let rules = ua_rules();
        assert_declares(&rules, "table", "display", "table");
        assert_declares(&rules, "table", "border-spacing", "2px");
        assert_declares(&rules, "td", "padding", "1px");
    }

    #[test]
    fn nowrap_cells_do_not_wrap() {
        assert_declares(&ua_rules(), "td[nowrap]", "white-space", "nowrap");
        assert_declares(&ua_rules(), "th[nowrap]", "white-space", "nowrap");
    }

    #[test]
    fn links_are_blue_and_underlined_and_visited_links_have_no_rule() {
        let rules = ua_rules();
        assert_declares(&rules, ":any-link", "color", "rgb(0, 0, 238)");
        assert_declares(&rules, ":any-link", "text-decoration", "underline");
        assert_declares(&rules, ":any-link", "cursor", "pointer");
        assert!(
            rules
                .iter()
                .all(|rule| rule.selectors.iter().all(|item| !item.contains(":visited"))),
            "a :visited rule would distinguish visited links"
        );
    }

    #[test]
    fn centring_keeps_an_inherited_alignment_only_for_th() {
        let rules = ua_rules();
        assert_declares(&rules, "th", "text-align", "-moz-center-or-inherit");
        assert_declares(&rules, "th", "font-weight", "bold");
        // Chrome keeps `-webkit-center` on these even under `text-align: right`.
        assert_declares(&rules, "center", "text-align", "-moz-center");
        assert_declares(&rules, "caption", "text-align", "-moz-center");
    }

    #[test]
    fn border_attribute_draws_the_table_frame_and_cell_lines() {
        let rules = ua_rules();
        assert_declares(
            &rules,
            "table:-servo-nonzero-border",
            "border-style",
            "outset",
        );
        assert_declares(
            &rules,
            "table:-servo-nonzero-border > tr > :is(td, th)",
            "border-style",
            "inset",
        );
        assert_declares(
            &rules,
            "table:-servo-nonzero-border > :is(thead, tbody, tfoot) > tr > :is(td, th)",
            "border-width",
            "1px",
        );
    }

    #[test]
    fn frame_and_rules_attributes_have_their_rules() {
        let rules = ua_rules();
        assert_declares(&rules, "table[frame=void i]", "border-style", "hidden");
        assert_declares(&rules, "table[frame=box i]", "border-style", "solid");
        assert_declares(
            &rules,
            "table[rules=none i] > tr > :is(td, th)",
            "border-width",
            "0px",
        );
    }

    #[test]
    fn iframes_have_a_default_border_that_frameborder_zero_removes() {
        let rules = ua_rules();
        assert_declares(&rules, "iframe", "border", "2px inset");
        assert_declares(&rules, "iframe[frameborder=0]", "border", "medium");
    }

    #[test]
    fn border_box_controls_match_chrome() {
        let rules = ua_rules();
        for selector in ["button", "select", "meter", "progress"] {
            assert_declares(&rules, selector, "box-sizing", "border-box");
        }
        let input = "input:is([type=button i], [type=submit i], [type=reset i], \
                     [type=checkbox i], [type=radio i], [type=color i], [type=search i])";
        assert_declares(&rules, input, "box-sizing", "border-box");
        assert!(
            rules.iter().all(|rule| {
                !rule
                    .selectors
                    .iter()
                    .any(|item| item == "textarea" || item == "input")
                    || !rule.declarations.iter().any(|(p, _)| p == "box-sizing")
            }),
            "text inputs and textarea stay content-box outside quirks mode"
        );
    }

    #[test]
    fn placeholder_text_is_grey() {
        assert_declares(&ua_rules(), "::placeholder", "color", "rgb(117, 117, 117)");
    }

    #[test]
    fn the_quirks_layer_resets_table_text_and_margins_form() {
        let rules = style_rules(QUIRKS_SHEET);
        for (property, value) in [
            ("font-weight", "initial"),
            ("font-style", "initial"),
            ("font-variant", "initial"),
            ("font-size", "initial"),
            ("line-height", "initial"),
            ("white-space", "initial"),
            ("text-align", "initial"),
        ] {
            assert_declares(&rules, "table", property, value);
        }
        assert_declares(&rules, "form", "margin-block-end", "1em");
    }

    #[test]
    fn the_quirks_layer_sizes_form_controls_border_box_and_gutters_floated_images() {
        let rules = style_rules(QUIRKS_SHEET);
        assert_declares(
            &rules,
            "input:not([type=image i])",
            "box-sizing",
            "border-box",
        );
        assert_declares(&rules, "textarea", "box-sizing", "border-box");
        assert_declares(&rules, "img[align=left i]", "margin-right", "3px");
        assert_declares(&rules, "img[align=right i]", "margin-left", "3px");
    }
}
