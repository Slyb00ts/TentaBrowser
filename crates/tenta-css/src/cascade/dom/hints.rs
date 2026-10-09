//! Presentational hints synthesized from legacy HTML attributes.
//!
//! The mappings follow the HTML Standard's "Rendering" chapter. Hints enter the
//! cascade at the presentational-hint level: above the user-agent sheet and
//! below every author rule. Declarations are parsed from CSS text by Stylo so
//! the values are exactly what an author could have written; only fixed
//! property names and values built here (numbers, `#rrggbb`, keywords, quoted
//! strings) are ever formatted into that text.
//!
//! What the standard expresses as selector rules (`table[border]` styling,
//! `frame`, `rules`, the inset cell borders, `iframe[frameborder]`,
//! `td[nowrap]`) belongs to the user-agent sheet, not here.
//!
//! Known gaps: body margins taken from a frame owner (`marginwidth` and
//! `marginheight` on the enclosing `iframe` or `frame`) are not applied, and a
//! base URL change between `StyleStore::refresh` and the traversal is not
//! detected, because the base is compared only during the refresh.
//!
//! The hint cache is a `OnceCell` per element: a flush is sequential and the
//! store is not `Sync`.

use std::cell::OnceCell;

use cssparser::serialize_string;
use selectors::matching::VisitedHandlingMode;
use selectors::sink::Push;
use servo_arc::Arc;
use style::applicable_declarations::ApplicableDeclarationBlock;
use style::context::QuirksMode;
use style::properties::declaration_block::parse_one_declaration_into;
use style::properties::{
    Importance, PropertyDeclarationBlock, PropertyId, SourcePropertyDeclaration,
};
use style::rule_tree::{CascadeLevel, CascadeOrigin};
use style::stylesheets::layer_rule::LayerOrder;
use style::stylesheets::{CssRuleType, Origin, UrlExtraData};
use style_traits::ParsingMode;
use tenta_core::atom::atoms;
use tenta_dom::Document;

use super::{Slot, style_attr};
use crate::cssom::configure_stylo;

/// Pushes the declarations that `bgcolor`, `align`, `width` and the other legacy
/// attributes of the element in `slot` stand for.
///
/// Links are not told apart by visitedness: the document keeps no history, so
/// no link is ever `:visited` and the body's `vlink` and `alink` colours have
/// nothing to apply to.
///
/// Panics when the store was not refreshed for the document's current
/// generation: the cached block could be stale.
pub(crate) fn synthesize<V>(slot: &Slot<'_>, _visited: VisitedHandlingMode, hints: &mut V)
where
    V: Push<ApplicableDeclarationBlock>,
{
    let cache = &slot.entry.hints;
    let generation = slot.view.doc.mutation_generation();
    assert_eq!(
        slot.entry.refreshed_generation.get(),
        generation,
        "StyleStore::refresh must run after the last document mutation",
    );
    let block = cache.block.get_or_init(|| {
        cache.source_generation.set(generation);
        build(slot).map(|block| Arc::new(slot.view.lock.wrap(block)))
    });
    assert_eq!(cache.source_generation.get(), generation);
    if let Some(block) = block {
        hints.push(ApplicableDeclarationBlock::from_declarations(
            Arc::clone(block),
            CascadeLevel::new(CascadeOrigin::PresHints),
            LayerOrder::root(),
        ));
    }
}

fn build(slot: &Slot<'_>) -> Option<PropertyDeclarationBlock> {
    let doc = slot.view.doc;
    let node = slot.node;
    if doc.namespace(node) != Some(atoms::NS_HTML) {
        return None;
    }
    let mut hints = Hints {
        slot,
        url_data: OnceCell::new(),
        block: PropertyDeclarationBlock::new(),
    };
    match doc.tag(node)?.as_str() {
        "body" => {
            hints.color("text", "color");
            hints.background();
            hints.body_margins();
        }
        "a" | "area" => hints.link_colour(),
        "table" => {
            hints.background();
            hints.nonzero_dimension("width", "width");
            hints.dimension("height", "height");
            hints.table_border();
            hints.color("bordercolor", "border-color");
            hints.pixels("cellspacing", "border-spacing");
            hints.table_align();
        }
        "thead" | "tbody" | "tfoot" | "tr" => {
            hints.background();
            hints.dimension("height", "height");
            hints.cell_alignment();
        }
        "td" | "th" => {
            hints.background();
            hints.nonzero_dimension("width", "width");
            hints.nonzero_dimension("height", "height");
            hints.cell_alignment();
            hints.cell_padding();
            hints.quirky_nowrap();
        }
        "col" => hints.dimension("width", "width"),
        "hr" => hints.hr(),
        "img" => hints.image(),
        "input"
            if hints
                .attr("type")
                .is_some_and(|t| t.eq_ignore_ascii_case("image")) =>
        {
            hints.image();
        }
        "object" => {
            hints.sized();
            hints.image_border();
            hints.margins();
            hints.embedded_align();
        }
        "embed" => {
            hints.sized();
            hints.margins();
            hints.embedded_align();
        }
        "iframe" => {
            hints.sized();
            hints.embedded_align();
        }
        "video" => {
            hints.sized();
            hints.aspect_ratio_from_dimensions();
        }
        "canvas" => hints.aspect_ratio_from_integers(),
        "div" | "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => hints.text_align(),
        "ul" | "ol" | "li" => hints.list_type(),
        "br" => hints.clear(),
        "font" => hints.font(),
        _ => {}
    }
    hints.finish()
}

/// Declarations accumulating for one element.
struct Hints<'a, 'b> {
    slot: &'b Slot<'a>,
    url_data: OnceCell<UrlExtraData>,
    block: PropertyDeclarationBlock,
}

impl<'a> Hints<'a, '_> {
    fn doc(&self) -> &'a Document {
        self.slot.view.doc
    }

    fn attr(&self, name: &str) -> Option<&'a str> {
        self.doc().attribute_str(self.slot.node, name)
    }

    /// The attribute, when its value is an ASCII case-insensitive match for
    /// one of the keywords.
    fn keyword<const N: usize>(
        &self,
        name: &str,
        keywords: [&'static str; N],
    ) -> Option<&'static str> {
        let value = self.attr(name)?;
        keywords.into_iter().find(|k| value.eq_ignore_ascii_case(k))
    }

    fn finish(self) -> Option<PropertyDeclarationBlock> {
        (!self.block.is_empty()).then_some(self.block)
    }

    /// Adds `property: value`. A value Stylo rejects adds nothing, which is
    /// how an invalid attribute is ignored.
    fn declare(&mut self, property: &str, value: &str) {
        let Ok(id) = PropertyId::parse_enabled_for_all_content(property) else {
            debug_assert!(false, "hints name only known properties: {property}");
            return;
        };
        configure_stylo();
        let url_data = self
            .url_data
            .get_or_init(|| style_attr::url_data(self.slot.view));
        let mut source = SourcePropertyDeclaration::default();
        let parsed = parse_one_declaration_into(
            &mut source,
            id,
            value,
            Origin::Author,
            url_data,
            None,
            ParsingMode::DEFAULT,
            self.slot.view.quirks,
            CssRuleType::Style,
        );
        if parsed.is_ok() {
            self.block.extend(source.drain(), Importance::Normal);
        }
    }

    /// A legacy-colour attribute mapped to a colour property.
    fn color(&mut self, attribute: &str, property: &str) {
        if let Some(rgb) = self.attr(attribute).and_then(parse_legacy_color) {
            self.declare(property, &hex(rgb));
        }
    }

    /// `bgcolor` and `background`, which share their element lists.
    fn background(&mut self) {
        self.color("bgcolor", "background-color");
        if let Some(url) = self.attr("background").map(|v| v.trim_matches(is_ascii_ws))
            && !url.is_empty()
        {
            let mut value = String::from("url(");
            serialize_string(url, &mut value).expect("writing to a String cannot fail");
            value.push(')');
            self.declare("background-image", &value);
        }
    }

    /// "Maps to the dimension property".
    fn dimension(&mut self, attribute: &str, property: &str) {
        if let Some(dimension) = self.attr(attribute).and_then(parse_dimension) {
            self.declare(property, &dimension.to_css());
        }
    }

    /// "Maps to the dimension property (ignoring zero)".
    fn nonzero_dimension(&mut self, attribute: &str, property: &str) {
        if let Some(dimension) = self
            .attr(attribute)
            .and_then(parse_dimension)
            .filter(|dimension| !dimension.is_zero())
        {
            self.declare(property, &dimension.to_css());
        }
    }

    /// `width` and `height` of embedded content.
    fn sized(&mut self) {
        self.dimension("width", "width");
        self.dimension("height", "height");
    }

    /// A non-negative integer attribute mapped to a pixel length.
    fn pixels(&mut self, attribute: &str, property: &str) {
        if let Some(px) = self.attr(attribute).and_then(parse_non_negative_integer) {
            self.declare(property, &format!("{px}px"));
        }
    }

    /// `table border`: an unparsable value (including the empty one) means one
    /// pixel, which is also what `:-servo-nonzero-border` assumes. The border
    /// style is the user-agent sheet's.
    fn table_border(&mut self) {
        if let Some(value) = self.attr("border") {
            let width = parse_non_negative_integer(value).unwrap_or(1);
            self.declare("border-width", &format!("{width}px"));
        }
    }

    /// `img`, `object` and image-button `border`: a pixel width and a solid
    /// line, only for a value above zero.
    fn image_border(&mut self) {
        if let Some(width) = self
            .attr("border")
            .and_then(parse_non_negative_integer)
            .filter(|&width| width > 0)
        {
            self.declare("border-width", &format!("{width}px"));
            self.declare("border-style", "solid");
        }
    }

    /// `cellpadding` of the nearest enclosing table, as padding on the cell.
    fn cell_padding(&mut self) {
        let doc = self.doc();
        let table = doc.ancestors(self.slot.node).find(|&ancestor| {
            doc.namespace(ancestor) == Some(atoms::NS_HTML)
                && doc.tag(ancestor) == Some(atoms::TABLE)
        });
        let padding = table
            .and_then(|table| doc.attribute_str(table, "cellpadding"))
            .and_then(parse_non_negative_integer);
        if let Some(px) = padding {
            self.declare("padding", &format!("{px}px"));
        }
    }

    /// In quirks mode a `nowrap` cell with a nonzero length `width` wraps
    /// after all; every other `nowrap` cell is the user-agent sheet's.
    fn quirky_nowrap(&mut self) {
        let wide = self
            .attr("width")
            .and_then(parse_dimension)
            .is_some_and(|dimension| matches!(dimension, Dimension::Length(v) if v != 0.0));
        if self.slot.view.quirks == QuirksMode::Quirks && self.attr("nowrap").is_some() && wide {
            self.declare("white-space", "normal");
        }
    }

    /// `hspace` and `vspace` as horizontal and vertical margins.
    fn margins(&mut self) {
        self.dimension("hspace", "margin-left");
        self.dimension("hspace", "margin-right");
        self.dimension("vspace", "margin-top");
        self.dimension("vspace", "margin-bottom");
    }

    /// The body's margin attributes: the first one present per side counts,
    /// and an unparsable one leaves the user-agent margin.
    fn body_margins(&mut self) {
        let sides: [(&str, [&str; 2]); 4] = [
            ("margin-top", ["marginheight", "topmargin"]),
            ("margin-bottom", ["marginheight", "bottommargin"]),
            ("margin-left", ["marginwidth", "leftmargin"]),
            ("margin-right", ["marginwidth", "rightmargin"]),
        ];
        for (property, attributes) in sides {
            if let Some(first) = attributes
                .into_iter()
                .find(|name| self.attr(name).is_some())
            {
                self.pixels(first, property);
            }
        }
    }

    /// `align` as `text-align`. Left, right and centre use the `-webkit-`
    /// forms so that they also align block-level children, like other engines.
    fn text_align(&mut self) {
        let value = match self.attr("align") {
            Some(v) if v.eq_ignore_ascii_case("left") => "-webkit-left",
            Some(v) if v.eq_ignore_ascii_case("right") => "-webkit-right",
            Some(v) if v.eq_ignore_ascii_case("center") || v.eq_ignore_ascii_case("middle") => {
                "-webkit-center"
            }
            Some(v) if v.eq_ignore_ascii_case("justify") => "justify",
            _ => return,
        };
        self.declare("text-align", value);
    }

    /// `align` and `valign` of table rows, sections and cells.
    fn cell_alignment(&mut self) {
        self.text_align();
        if let Some(value) = self.keyword("valign", ["top", "middle", "bottom", "baseline"]) {
            self.declare("vertical-align", value);
        }
    }

    /// `table align`: left and right float the table, centre centres it.
    fn table_align(&mut self) {
        match self.keyword("align", ["left", "right", "center"]) {
            Some(side @ ("left" | "right")) => self.declare("float", side),
            Some(_) => {
                self.declare("margin-left", "auto");
                self.declare("margin-right", "auto");
            }
            None => {}
        }
    }

    /// `align` of images and other embedded content. `center` and `middle`
    /// ask for "the vertical middle on the parent's baseline", which Stylo
    /// 0.21 has no value for (`-webkit-baseline-middle` does not parse), so
    /// they are not mapped.
    fn embedded_align(&mut self) {
        let Some(align) = self.keyword(
            "align",
            [
                "left",
                "right",
                "top",
                "baseline",
                "texttop",
                "absmiddle",
                "abscenter",
                "bottom",
            ],
        ) else {
            return;
        };
        match align {
            "left" | "right" => self.declare("float", align),
            "texttop" => self.declare("vertical-align", "text-top"),
            "absmiddle" | "abscenter" => self.declare("vertical-align", "middle"),
            _ => self.declare("vertical-align", align),
        }
    }

    /// An `img` or image button.
    fn image(&mut self) {
        self.sized();
        self.image_border();
        self.margins();
        self.embedded_align();
        self.aspect_ratio_from_dimensions();
    }

    /// `width` and `height` as `aspect-ratio: auto w / h`, when both are
    /// lengths under the dimension rules.
    fn aspect_ratio_from_dimensions(&mut self) {
        let length = |value: Option<&str>| match value.and_then(parse_dimension) {
            Some(Dimension::Length(v)) => Some(v),
            _ => None,
        };
        if let (Some(w), Some(h)) = (length(self.attr("width")), length(self.attr("height"))) {
            self.declare("aspect-ratio", &format!("auto {w} / {h}"));
        }
    }

    /// The same for `canvas`, whose attributes are integers.
    fn aspect_ratio_from_integers(&mut self) {
        let integer = |value: Option<&str>| value.and_then(parse_non_negative_integer);
        if let (Some(w), Some(h)) = (integer(self.attr("width")), integer(self.attr("height"))) {
            self.declare("aspect-ratio", &format!("auto {w} / {h}"));
        }
    }

    /// The `hr` attributes.
    fn hr(&mut self) {
        match self.keyword("align", ["left", "right", "center"]) {
            Some("left") => {
                self.declare("margin-left", "0");
                self.declare("margin-right", "auto");
            }
            Some("right") => {
                self.declare("margin-left", "auto");
                self.declare("margin-right", "0");
            }
            Some(_) => {
                self.declare("margin-left", "auto");
                self.declare("margin-right", "auto");
            }
            None => {}
        }
        let coloured = self.attr("color").is_some() || self.attr("noshade").is_some();
        if coloured {
            self.declare("border-style", "solid");
        }
        if let Some(size) = self.attr("size").and_then(parse_non_negative_integer) {
            if coloured {
                self.declare("border-width", &format!("{}px", f64::from(size) / 2.0));
            } else if size == 1 {
                self.declare("border-bottom-width", "0");
            } else if size > 1 {
                self.declare("height", &format!("{}px", size - 2));
            }
        }
        self.dimension("width", "width");
        self.color("color", "color");
    }

    /// `ul`, `ol` and `li` `type`. The ordered keywords are case-sensitive
    /// and the bullet keywords are not; `li` accepts both families.
    fn list_type(&mut self) {
        let tag = self.doc().tag(self.slot.node);
        let (ordered, bullets) = match tag {
            Some(t) if t == atoms::UL => (false, true),
            Some(t) if t == atoms::OL => (true, false),
            _ => (true, true),
        };
        let Some(value) = self.attr("type") else {
            return;
        };
        let style = match value {
            "1" if ordered => "decimal",
            "a" if ordered => "lower-alpha",
            "A" if ordered => "upper-alpha",
            "i" if ordered => "lower-roman",
            "I" if ordered => "upper-roman",
            _ if bullets => match ["none", "disc", "circle", "square"]
                .into_iter()
                .find(|k| value.eq_ignore_ascii_case(k))
            {
                Some(keyword) => keyword,
                None => return,
            },
            _ => return,
        };
        self.declare("list-style-type", style);
    }

    /// `br clear`.
    fn clear(&mut self) {
        match self.keyword("clear", ["left", "right", "all", "both"]) {
            Some(side @ ("left" | "right")) => self.declare("clear", side),
            Some(_) => self.declare("clear", "both"),
            None => {}
        }
    }

    /// `font color`, `size` and `face`.
    fn font(&mut self) {
        self.color("color", "color");
        if let Some(keyword) = self.attr("size").and_then(font_size_keyword) {
            self.declare("font-size", keyword);
        }
        if let Some(family) = self.attr("face").and_then(font_family_list) {
            self.declare("font-family", &family);
        }
    }

    /// The `link` colour of the body applies to every unvisited link.
    fn link_colour(&mut self) {
        let doc = self.doc();
        if !doc.has_attribute(self.slot.node, atoms::HREF) {
            return;
        }
        let colour = doc
            .body()
            .filter(|&body| doc.tag(body) == Some(atoms::BODY))
            .and_then(|body| doc.attribute_str(body, "link"))
            .and_then(parse_legacy_color);
        if let Some(rgb) = colour {
            self.declare("color", &hex(rgb));
        }
    }
}

fn hex([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

fn is_ascii_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\x0c' | '\r')
}

/// The HTML "rules for parsing a legacy colour value".
fn parse_legacy_color(input: &str) -> Option<[u8; 3]> {
    if input.is_empty() {
        return None;
    }
    let input = input.trim_matches(is_ascii_ws);
    if input.eq_ignore_ascii_case("transparent") {
        return None;
    }
    if let Ok((r, g, b)) = cssparser::color::parse_named_color(input) {
        return Some([r, g, b]);
    }
    let chars: Vec<char> = input.chars().collect();
    if let ['#', a, b, c] = chars[..]
        && [a, b, c].iter().all(char::is_ascii_hexdigit)
    {
        let double = |digit: char| hex_value(&[digit, digit]);
        return Some([double(a), double(b), double(c)]);
    }
    // Characters outside the BMP stand for two zeros, so that every input
    // character contributes the same amount.
    let mut digits: Vec<char> = chars
        .into_iter()
        .flat_map(|c| {
            if u32::from(c) > 0xFFFF {
                vec!['0', '0']
            } else {
                vec![c]
            }
        })
        .take(128)
        .collect();
    if digits.first() == Some(&'#') {
        digits.remove(0);
    }
    for digit in &mut digits {
        if !digit.is_ascii_hexdigit() {
            *digit = '0';
        }
    }
    while digits.is_empty() || !digits.len().is_multiple_of(3) {
        digits.push('0');
    }
    let mut length = digits.len() / 3;
    let components: Vec<&[char]> = digits.chunks(length).collect();
    let mut start = 0;
    if length > 8 {
        start = length - 8;
        length = 8;
    }
    while length > 2 && components.iter().all(|component| component[start] == '0') {
        start += 1;
        length -= 1;
    }
    let kept = length.min(2);
    let component = |index: usize| hex_value(&components[index][start..start + kept]);
    Some([component(0), component(1), component(2)])
}

/// The value of one or two hexadecimal digits.
fn hex_value(digits: &[char]) -> u8 {
    digits.iter().fold(0, |value, digit| {
        value * 16 + digit.to_digit(16).unwrap_or(0) as u8
    })
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum Dimension {
    Length(f64),
    Percentage(f64),
}

impl Dimension {
    fn to_css(self) -> String {
        match self {
            Self::Length(px) => format!("{px}px"),
            Self::Percentage(percent) => format!("{percent}%"),
        }
    }

    fn is_zero(self) -> bool {
        matches!(self, Self::Length(v) | Self::Percentage(v) if v == 0.0)
    }
}

/// The HTML "rules for parsing dimension values". Anything after the number
/// and an optional `%` is ignored.
fn parse_dimension(input: &str) -> Option<Dimension> {
    let bytes = input.trim_start_matches(is_ascii_ws).as_bytes();
    if !bytes.first()?.is_ascii_digit() {
        return None;
    }
    let mut value = 0f64;
    let mut position = 0;
    while let Some(digit) = bytes.get(position).filter(|b| b.is_ascii_digit()) {
        value = value * 10.0 + f64::from(digit - b'0');
        position += 1;
    }
    if bytes.get(position) == Some(&b'.') {
        position += 1;
        let mut divisor = 1.0;
        while let Some(digit) = bytes.get(position).filter(|b| b.is_ascii_digit()) {
            divisor /= 10.0;
            value += f64::from(digit - b'0') * divisor;
            position += 1;
        }
    }
    if !value.is_finite() {
        return None;
    }
    Some(if bytes.get(position) == Some(&b'%') {
        Dimension::Percentage(value)
    } else {
        Dimension::Length(value)
    })
}

/// The HTML "rules for parsing non-negative integers", saturating instead of
/// overflowing. Trailing garbage after the digits is ignored.
fn parse_non_negative_integer(input: &str) -> Option<u32> {
    let rest = input.trim_start_matches(is_ascii_ws);
    let (negative, rest) = match rest.as_bytes().first()? {
        b'-' => (true, &rest[1..]),
        b'+' => (false, &rest[1..]),
        _ => (false, rest),
    };
    let digits = rest.bytes().take_while(u8::is_ascii_digit);
    let mut value = 0u32;
    let mut any = false;
    for digit in digits {
        any = true;
        value = value
            .saturating_mul(10)
            .saturating_add(u32::from(digit - b'0'));
    }
    (any && !(negative && value != 0)).then_some(value)
}

/// The HTML "rules for parsing a legacy font size", as the keyword it selects.
fn font_size_keyword(input: &str) -> Option<&'static str> {
    let rest = input.trim_start_matches(is_ascii_ws);
    let (relative, rest) = match rest.as_bytes().first()? {
        b'+' => (1, &rest[1..]),
        b'-' => (-1, &rest[1..]),
        _ => (0, rest),
    };
    let mut digits = rest.bytes().take_while(u8::is_ascii_digit).peekable();
    digits.peek()?;
    let mut value = 0i64;
    for digit in digits {
        value = value
            .saturating_mul(10)
            .saturating_add(i64::from(digit - b'0'));
    }
    let value = match relative {
        1 => 3 + value,
        -1 => 3 - value,
        _ => value,
    };
    Some(match value.clamp(1, 7) {
        1 => "x-small",
        2 => "small",
        3 => "medium",
        4 => "large",
        5 => "x-large",
        6 => "xx-large",
        _ => "xxx-large",
    })
}

/// A `font face` value as a `font-family` list: every name becomes a string
/// except the generic families, so a name never reads as a CSS keyword or
/// function. A CSS-wide keyword or `var()` anywhere makes the whole value
/// invalid.
fn font_family_list(input: &str) -> Option<String> {
    const GENERIC: [&str; 12] = [
        "serif",
        "sans-serif",
        "monospace",
        "cursive",
        "fantasy",
        "system-ui",
        "ui-serif",
        "ui-sans-serif",
        "ui-monospace",
        "ui-rounded",
        "math",
        "emoji",
    ];
    const REJECTED: [&str; 6] = [
        "initial",
        "inherit",
        "unset",
        "revert",
        "revert-layer",
        "default",
    ];
    let mut families = Vec::new();
    for name in input.split(',') {
        let name = name.trim_matches(is_ascii_ws);
        let name = name
            .strip_prefix('"')
            .and_then(|n| n.strip_suffix('"'))
            .or_else(|| name.strip_prefix('\'').and_then(|n| n.strip_suffix('\'')))
            .unwrap_or(name);
        if name.is_empty() {
            continue;
        }
        let lower = name.to_ascii_lowercase();
        if REJECTED.contains(&lower.as_str()) || lower.starts_with("var(") {
            return None;
        }
        if GENERIC.contains(&lower.as_str()) {
            families.push(lower);
        } else {
            let mut quoted = String::new();
            serialize_string(name, &mut quoted).expect("writing to a String cannot fail");
            families.push(quoted);
        }
    }
    (!families.is_empty()).then(|| families.join(", "))
}

#[cfg(test)]
mod tests {
    use selectors::matching::VisitedHandlingMode;
    use tenta_core::NodeId;

    use style::properties::PropertyDeclaration;
    use style::shared_lock::SharedRwLock;
    use tenta_core::atom::Atom;
    use tenta_dom::Mutation;
    use tenta_html::QualName;

    use super::*;
    use crate::cascade::dom::test_support::parse;
    use crate::cascade::dom::{AtomMaps, FlushView, StyleStore};

    #[test]
    fn legacy_colours_follow_the_html_algorithm() {
        let cases: &[(&str, Option<[u8; 3]>)] = &[
            ("", None),
            ("transparent", None),
            ("  TRANSPARENT ", None),
            ("red", Some([255, 0, 0])),
            (" Red\n", Some([255, 0, 0])),
            ("RebeccaPurple", Some([102, 51, 153])),
            ("#abc", Some([0xaa, 0xbb, 0xcc])),
            ("#ABC", Some([0xaa, 0xbb, 0xcc])),
            ("#c0ffee", Some([0xc0, 0xff, 0xee])),
            ("c0ffee", Some([0xc0, 0xff, 0xee])),
            ("chucknorris", Some([0xc0, 0x00, 0x00])),
            ("   ", Some([0, 0, 0])),
            ("#", Some([0, 0, 0])),
            ("#xyz", Some([0, 0, 0])),
            ("f", Some([0x0f, 0, 0])),
            ("#12", Some([0x01, 0x02, 0x00])),
            ("#1234", Some([0x12, 0x34, 0x00])),
            ("#ff0000ff", Some([0xff, 0x00, 0xff])),
            ("#0000ff00", Some([0x00, 0xff, 0x00])),
            ("rgb(1,2,3)", Some([0x00, 0x10, 0x30])),
            // Two zeros stand in for a character outside the BMP.
            ("a\u{1F600}", Some([0x0a, 0x00, 0x00])),
        ];
        for (input, expected) in cases {
            assert_eq!(parse_legacy_color(input), *expected, "colour {input:?}");
        }
    }

    #[test]
    fn long_colours_drop_leading_digits_then_common_leading_zeros() {
        // Ten digits per component: two are dropped to reach eight, then the
        // six common leading zeros.
        assert_eq!(
            parse_legacy_color("#000000001200000000340000000056"),
            Some([0x12, 0x34, 0x56]),
        );
        // More than 128 characters are cut off before splitting.
        let long = format!("#{}", "f".repeat(300));
        assert_eq!(parse_legacy_color(&long), Some([0xff, 0xff, 0xff]));
    }

    #[test]
    fn dimensions_parse_numbers_percentages_and_ignore_trailing_text() {
        let length = |v: f64| Some(Dimension::Length(v));
        let percent = |v: f64| Some(Dimension::Percentage(v));
        let cases: &[(&str, Option<Dimension>)] = &[
            ("", None),
            ("abc", None),
            (".5", None),
            ("-5", None),
            ("+5", None),
            ("0", length(0.0)),
            ("100", length(100.0)),
            ("  42", length(42.0)),
            ("10px", length(10.0)),
            ("12.5", length(12.5)),
            ("12.", length(12.0)),
            ("12.5%", percent(12.5)),
            ("50%", percent(50.0)),
            ("50 %", length(50.0)),
            ("7abc%", length(7.0)),
        ];
        for (input, expected) in cases {
            assert_eq!(parse_dimension(input), *expected, "dimension {input:?}");
        }
        assert_eq!(
            parse_dimension(&"9".repeat(400)),
            None,
            "overflow is an error"
        );
    }

    #[test]
    fn non_negative_integers_ignore_trailing_text_and_reject_negatives() {
        let cases: &[(&str, Option<u32>)] = &[
            ("", None),
            ("x", None),
            ("5", Some(5)),
            (" 5", Some(5)),
            ("+5", Some(5)),
            ("5px", Some(5)),
            ("-5", None),
            ("-0", Some(0)),
            ("99999999999", Some(u32::MAX)),
        ];
        for (input, expected) in cases {
            assert_eq!(
                parse_non_negative_integer(input),
                *expected,
                "integer {input:?}"
            );
        }
    }

    #[test]
    fn legacy_font_sizes_map_to_keywords() {
        let cases: &[(&str, Option<&str>)] = &[
            ("", None),
            ("x", None),
            ("+", None),
            ("1", Some("x-small")),
            ("3", Some("medium")),
            ("7", Some("xxx-large")),
            ("0", Some("x-small")),
            ("99", Some("xxx-large")),
            ("+1", Some("large")),
            ("+2", Some("x-large")),
            ("+9", Some("xxx-large")),
            ("-1", Some("small")),
            ("-2", Some("x-small")),
            (" 4px", Some("large")),
        ];
        for (input, expected) in cases {
            assert_eq!(font_size_keyword(input), *expected, "font size {input:?}");
        }
    }

    fn node_t(doc: &Document) -> NodeId {
        (0..doc.slots() as u32)
            .map(NodeId)
            .find(|&node| doc.element_id(node) == Some("t"))
            .expect("the fixture has an element with id=t")
    }

    /// A store and lock that live across flushes, as the engine's do.
    struct Engine {
        store: StyleStore,
        lock: SharedRwLock,
    }

    impl Engine {
        fn new(doc: &Document) -> Self {
            let mut engine = Self {
                store: StyleStore::new(),
                lock: SharedRwLock::new(),
            };
            engine.flush_prepare(doc);
            engine
        }

        fn flush_prepare(&mut self, doc: &Document) {
            for slot in 0..doc.slots() {
                self.store.claim(doc.node_ref(NodeId(slot as u32)));
            }
            self.store.refresh(doc);
        }

        /// The hint blocks of `node`, serialized and joined.
        fn hints(&self, doc: &Document, node: NodeId) -> String {
            self.with_hint_blocks(doc, node, |guard, blocks| {
                blocks
                    .iter()
                    .map(|block| {
                        assert_eq!(block.level(), CascadeLevel::new(CascadeOrigin::PresHints));
                        let mut css = String::new();
                        block.source.read(guard).to_css(&mut css).unwrap();
                        css
                    })
                    .collect::<Vec<_>>()
                    .join(" | ")
            })
        }

        fn with_hint_blocks<R>(
            &self,
            doc: &Document,
            node: NodeId,
            f: impl FnOnce(
                &style::shared_lock::SharedRwLockReadGuard,
                &[ApplicableDeclarationBlock],
            ) -> R,
        ) -> R {
            let mut atoms = AtomMaps::new();
            atoms.begin_flush(doc);
            let view = FlushView::new(doc, &atoms, &self.lock);
            let table = view.make_slots(&self.store);
            assert!(view.install_slots(&table));
            let handle = view.handle(node).expect("every slot has a handle");
            let mut blocks: Vec<ApplicableDeclarationBlock> = Vec::new();
            handle.synthesize_hints(VisitedHandlingMode::AllLinksUnvisited, &mut blocks);
            f(&self.lock.read(), &blocks)
        }
    }

    /// The hint declarations of the element with `id="t"`, serialized.
    fn hints_of(html: &str) -> String {
        let doc = parse(html);
        Engine::new(&doc).hints(&doc, node_t(&doc))
    }

    #[test]
    fn each_legacy_attribute_produces_its_declarations() {
        let cases: &[(&str, &str)] = &[
            // Colours.
            (
                "<body id=t bgcolor=red>",
                "background-color: rgb(255, 0, 0);",
            ),
            ("<body id=t text=chucknorris>", "color: rgb(192, 0, 0);"),
            (
                "<table id=t bgcolor='#0f0'>",
                "background-color: rgb(0, 255, 0);",
            ),
            (
                "<table><tr id=t bgcolor=blue>",
                "background-color: rgb(0, 0, 255);",
            ),
            (
                "<table><tr><td id=t bgcolor=blue>",
                "background-color: rgb(0, 0, 255);",
            ),
            (
                "<table><tr><th id=t bgcolor=blue>",
                "background-color: rgb(0, 0, 255);",
            ),
            (
                "<table><tbody id=t bgcolor=blue>",
                "background-color: rgb(0, 0, 255);",
            ),
            ("<body id=t bgcolor=transparent>", ""),
            ("<body id=t bgcolor=''>", ""),
            ("<div id=t bgcolor=red>", ""),
            ("<font id=t color=red>", "color: rgb(255, 0, 0);"),
            // Dimensions.
            ("<table id=t width=50%>", "width: 50%;"),
            (
                "<table id=t width=300 height=10.5>",
                "width: 300px; height: 10.5px;",
            ),
            ("<table id=t width=abc>", ""),
            ("<table id=t width=-4>", ""),
            (
                "<table><tr><td id=t width=10 height=20%>",
                "width: 10px; height: 20%;",
            ),
            ("<table><tr><th id=t width=0>", ""),
            ("<table><tr><td id=t width=0 height=0>", ""),
            ("<table><tr><td id=t width=0.0%>", ""),
            ("<table id=t width=0>", ""),
            ("<table id=t height=0>", "height: 0px;"),
            ("<table><tbody id=t height=12>", "height: 12px;"),
            ("<table><colgroup><col id=t width=0>", "width: 0px;"),
            ("<table><tr id=t height=30>", "height: 30px;"),
            ("<table><colgroup><col id=t width=40>", "width: 40px;"),
            ("<hr id=t width=75%>", "width: 75%;"),
            (
                "<iframe id=t width=320 height=240></iframe>",
                "width: 320px; height: 240px;",
            ),
            (
                "<canvas id=t width=64 height=32></canvas>",
                "aspect-ratio: auto 64 / 32;",
            ),
            ("<canvas id=t width=64 height=x></canvas>", ""),
            (
                "<video id=t width=16 height=9></video>",
                "width: 16px; height: 9px; aspect-ratio: auto 16 / 9;",
            ),
            (
                "<video id=t width=50% height=9></video>",
                "width: 50%; height: 9px;",
            ),
            (
                "<img id=t width=100 height=50>",
                "width: 100px; height: 50px; aspect-ratio: auto 100 / 50;",
            ),
            (
                "<input id=t type=IMAGE width=8 height=4>",
                "width: 8px; height: 4px; aspect-ratio: auto 8 / 4;",
            ),
            ("<input id=t type=text width=8 height=4>", ""),
            ("<embed id=t width=8 height=4>", "width: 8px; height: 4px;"),
            ("<object id=t width=8%></object>", "width: 8%;"),
            ("<div id=t width=5>", ""),
            // Borders and spacing.
            ("<table id=t border=2>", "border-width: 2px;"),
            ("<table id=t border>", "border-width: 1px;"),
            ("<table id=t border=0>", "border-width: 0px;"),
            ("<table id=t cellspacing=4>", "border-spacing: 4px;"),
            ("<table id=t cellspacing=-1>", ""),
            (
                "<img id=t border=3>",
                "border-width: 3px; border-style: solid;",
            ),
            ("<img id=t border=-1>", ""),
            ("<img id=t border=0>", ""),
            (
                "<object id=t border=2></object>",
                "border-width: 2px; border-style: solid;",
            ),
            (
                "<input id=t type=image border=1>",
                "border-width: 1px; border-style: solid;",
            ),
            (
                "<table id=t bordercolor=red>",
                "border-color: rgb(255, 0, 0);",
            ),
            ("<table id=t bordercolor=transparent>", ""),
            (
                "<img id=t hspace=10%>",
                "margin-left: 10%; margin-right: 10%;",
            ),
            (
                "<embed id=t vspace=3>",
                "margin-top: 3px; margin-bottom: 3px;",
            ),
            ("<img id=t hspace=5 vspace=2>", "margin: 2px 5px;"),
            ("<table cellpadding=6><tr><td id=t>", "padding: 6px;"),
            ("<table cellpadding=6><tr><th id=t>", "padding: 6px;"),
            ("<table cellpadding=6><tr><td><table><tr><td id=t>", ""),
            ("<table cellpadding=x><tr><td id=t>", ""),
            // Alignment.
            ("<div id=t align=center>", "text-align: -moz-center;"),
            ("<p id=t align=RIGHT>", "text-align: -moz-right;"),
            ("<h2 id=t align=left>", "text-align: -moz-left;"),
            ("<p id=t align=justify>", "text-align: justify;"),
            ("<p id=t align=bogus>", ""),
            (
                "<table><tr><td id=t align=center valign=top>",
                "text-align: -moz-center; vertical-align: top;",
            ),
            ("<table><tr id=t valign=bottom>", "vertical-align: bottom;"),
            ("<table id=t align=right>", "float: right;"),
            (
                "<table id=t align=center>",
                "margin-left: auto; margin-right: auto;",
            ),
            ("<div id=t align=middle>", "text-align: -moz-center;"),
            (
                "<table><tr><td id=t align=MIDDLE>",
                "text-align: -moz-center;",
            ),
            ("<table><tr><td id=t align=justify>", "text-align: justify;"),
            (
                "<table><tbody id=t valign=baseline>",
                "vertical-align: baseline;",
            ),
            ("<table><tr id=t valign=sideways>", ""),
            ("<img id=t align=left>", "float: left;"),
            ("<img id=t align=RIGHT>", "float: right;"),
            ("<img id=t align=top>", "vertical-align: top;"),
            ("<img id=t align=baseline>", "vertical-align: baseline;"),
            ("<img id=t align=texttop>", "vertical-align: text-top;"),
            ("<img id=t align=absmiddle>", "vertical-align: middle;"),
            ("<img id=t align=abscenter>", "vertical-align: middle;"),
            ("<img id=t align=bottom>", "vertical-align: bottom;"),
            ("<embed id=t align=left>", "float: left;"),
            ("<iframe id=t align=right></iframe>", "float: right;"),
            ("<object id=t align=top></object>", "vertical-align: top;"),
            (
                "<input id=t type=image align=bottom>",
                "vertical-align: bottom;",
            ),
            ("<input id=t type=text align=left>", ""),
            // Centre and middle need a value Stylo 0.21 cannot parse.
            ("<img id=t align=middle>", ""),
            ("<img id=t align=center>", ""),
            // Text.
            ("<table><tr><td id=t nowrap>", ""),
            (
                "<table><tr><td id=t nowrap width=100>",
                "width: 100px; white-space: normal;",
            ),
            ("<table><tr><td id=t nowrap width=50%>", "width: 50%;"),
            ("<table><tr><td id=t nowrap width=0>", ""),
            ("<table><tr><td id=t width=100>", "width: 100px;"),
            (
                "<!DOCTYPE html><table><tr><td id=t nowrap width=100>",
                "width: 100px;",
            ),
            ("<div id=t nowrap>", ""),
            ("<font id=t size=+2>", "font-size: x-large;"),
            ("<font id=t size=bogus>", ""),
            (
                "<font id=t face='Arial, sans-serif'>",
                "font-family: \"Arial\", sans-serif;",
            ),
            ("<font id=t face='Arial 2'>", "font-family: \"Arial 2\";"),
            (
                "<font id=t face='\"Times New Roman\", SERIF'>",
                "font-family: \"Times New Roman\", serif;",
            ),
            (
                "<font id=t face='a; color: red'>",
                "font-family: \"a; color: red\";",
            ),
            ("<font id=t face='Arial, inherit'>", ""),
            ("<font id=t face=INITIAL>", ""),
            ("<font id=t face='var(--x)'>", ""),
            ("<font id=t face=' , '>", ""),
            (
                "<font id=t color=red size=1 face=serif>",
                "color: rgb(255, 0, 0); font-size: x-small; font-family: serif;",
            ),
            // Lists.
            ("<ol id=t type=1>", "list-style-type: decimal;"),
            ("<ol id=t type=a>", "list-style-type: lower-alpha;"),
            ("<ol id=t type=A>", "list-style-type: upper-alpha;"),
            ("<ol id=t type=i>", "list-style-type: lower-roman;"),
            ("<ol id=t type=I>", "list-style-type: upper-roman;"),
            ("<ol id=t type=disc>", ""),
            ("<ul id=t type=DISC>", "list-style-type: disc;"),
            ("<ul id=t type=Circle>", "list-style-type: circle;"),
            ("<ul id=t type=square>", "list-style-type: square;"),
            ("<ul id=t type=none>", "list-style-type: none;"),
            ("<ul id=t type=1>", ""),
            ("<ul><li id=t type=I>", "list-style-type: upper-roman;"),
            ("<ul><li id=t type=NONE>", "list-style-type: none;"),
            ("<ol><li id=t type=i>", "list-style-type: lower-roman;"),
            ("<ol><li id=t type=bogus>", ""),
            // Rules, line breaks and the body.
            ("<br id=t clear=LEFT>", "clear: left;"),
            ("<br id=t clear=right>", "clear: right;"),
            ("<br id=t clear=all>", "clear: both;"),
            ("<br id=t clear=both>", "clear: both;"),
            ("<br id=t clear=none>", ""),
            (
                "<hr id=t align=left>",
                "margin-left: 0px; margin-right: auto;",
            ),
            (
                "<hr id=t align=right>",
                "margin-left: auto; margin-right: 0px;",
            ),
            (
                "<hr id=t align=center>",
                "margin-left: auto; margin-right: auto;",
            ),
            ("<hr id=t noshade>", "border-style: solid;"),
            (
                "<hr id=t color=red>",
                "border-style: solid; color: rgb(255, 0, 0);",
            ),
            (
                "<hr id=t noshade size=9>",
                "border-style: solid; border-width: 4.5px;",
            ),
            ("<hr id=t size=1>", "border-bottom-width: 0px;"),
            ("<hr id=t size=10>", "height: 8px;"),
            ("<hr id=t size=0>", ""),
            ("<hr id=t size=x>", ""),
            (
                "<body id=t marginwidth=3 marginheight=4>",
                "margin: 4px 3px;",
            ),
            (
                "<body id=t leftmargin=1 topmargin=2 rightmargin=3 bottommargin=4>",
                "margin: 2px 3px 4px 1px;",
            ),
            (
                "<body id=t marginwidth=5 leftmargin=9>",
                "margin-left: 5px; margin-right: 5px;",
            ),
            ("<body id=t marginwidth=x leftmargin=9>", ""),
            // Links take the body's colour; non-links do not.
            ("<body link=blue><a id=t href=x>", "color: rgb(0, 0, 255);"),
            ("<body link=blue><a id=t>", ""),
            ("<body vlink=blue><a id=t href=x>", ""),
            (
                "<body link=blue><map><area id=t href=x>",
                "color: rgb(0, 0, 255);",
            ),
            // Foreign elements get none.
            ("<svg><td id=t bgcolor=red width=5></svg>", ""),
        ];
        for (html, expected) in cases {
            assert_eq!(&hints_of(html), expected, "hints of {html}");
        }
    }

    #[test]
    fn the_background_attribute_becomes_a_quoted_url() {
        assert_eq!(
            hints_of("<body id=t background=' img/a\"b.png '>"),
            r#"background-image: url("img/a\"b.png");"#,
        );
        assert_eq!(hints_of("<body id=t background='  '>"), "");
    }

    #[test]
    fn the_hint_block_is_cached_across_flushes_until_the_document_changes() {
        let mut doc = parse("<body id=t bgcolor=red><p id=other>");
        let node = node_t(&doc);
        let mut engine = Engine::new(&doc);
        let identity = |engine: &Engine, doc: &Document| {
            engine.with_hint_blocks(doc, node, |_, blocks| {
                std::ptr::from_ref(&**blocks[0].source.get()) as usize
            })
        };
        let first = identity(&engine, &doc);
        assert_eq!(identity(&engine, &doc), first);

        set_attribute(&mut doc, node, "bgcolor", "blue");
        engine.flush_prepare(&doc);
        assert_eq!(
            engine.hints(&doc, node),
            "background-color: rgb(0, 0, 255);"
        );
    }

    fn set_attribute(doc: &mut Document, element: NodeId, name: &str, value: &str) {
        doc.mutate(Mutation::SetAttribute {
            element,
            name: QualName::new(atoms::EMPTY, Atom::new(name)),
            value: value.to_owned(),
        })
        .expect("set attribute");
    }

    #[test]
    fn ancestor_attributes_feed_the_hints_of_descendants() {
        let mut doc = parse("<table id=table cellpadding=2><tr><td id=t>x</td></tr></table>");
        let node = node_t(&doc);
        let table = (0..doc.slots() as u32)
            .map(NodeId)
            .find(|&n| doc.element_id(n) == Some("table"))
            .unwrap();
        let mut engine = Engine::new(&doc);
        assert_eq!(engine.hints(&doc, node), "padding: 2px;");
        set_attribute(&mut doc, table, "cellpadding", "9");
        engine.flush_prepare(&doc);
        assert_eq!(engine.hints(&doc, node), "padding: 9px;");

        let mut doc = parse("<body id=body link=red><a id=t href=x>x</a>");
        let node = node_t(&doc);
        let body = doc.body().unwrap();
        let mut engine = Engine::new(&doc);
        assert_eq!(engine.hints(&doc, node), "color: rgb(255, 0, 0);");
        set_attribute(&mut doc, body, "link", "blue");
        engine.flush_prepare(&doc);
        assert_eq!(engine.hints(&doc, node), "color: rgb(0, 0, 255);");
    }

    #[test]
    #[should_panic(expected = "StyleStore::refresh must run")]
    fn reading_hints_without_a_refresh_after_a_mutation_panics() {
        let mut doc = parse("<table id=t bgcolor=red>");
        let node = node_t(&doc);
        let engine = Engine::new(&doc);
        set_attribute(&mut doc, node, "bgcolor", "blue");
        engine.hints(&doc, node);
    }

    #[test]
    fn the_background_url_is_resolved_against_the_document_base() {
        let mut doc = parse("<body id=t background=img/a.png>");
        doc.set_url(Some("http://example.test/dir/page.html".parse().unwrap()));
        let node = node_t(&doc);
        let mut engine = Engine::new(&doc);
        let resolved = |engine: &Engine, doc: &Document| {
            engine.with_hint_blocks(doc, node, |guard, blocks| {
                let declarations = blocks[0].source.read(guard).declarations();
                let PropertyDeclaration::BackgroundImage(images) = &declarations[0] else {
                    panic!("expected a background-image declaration");
                };
                match &images.0[0] {
                    style::values::generics::image::Image::Url(url) => {
                        url.url().unwrap().to_string()
                    }
                    other => panic!("expected a url image: {other:?}"),
                }
            })
        };
        assert_eq!(resolved(&engine, &doc), "http://example.test/dir/img/a.png");

        doc.set_url(Some("http://other.test/x/page.html".parse().unwrap()));
        engine.flush_prepare(&doc);
        assert_eq!(resolved(&engine, &doc), "http://other.test/x/img/a.png");
    }
}
