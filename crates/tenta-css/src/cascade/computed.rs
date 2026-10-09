//! Serialisation of computed values for `getComputedStyle`.
//!
//! Values are the *computed* ones, written by Stylo's serialiser, with the
//! spellings browsers report. Properties whose resolved value is a used value
//! (`width`, `height`, margins and `top`/`left` of positioned boxes, a
//! percentage `transform`, `min-width: auto`, ...) come out as their computed
//! keyword, percentage or function here; the host substitutes the used value
//! after layout.

use cssparser::serialize_string;
use style::properties::{
    ComputedValues, Importance, LonghandId, NonCustomPropertyId, PropertyDeclarationBlock,
    PropertyDeclarationId, PropertyId, ShorthandId,
};
use style::values::generics::counters::{Content, ContentItem};
use style::values::resolved;
use style_traits::ToCss;
use tenta_core::NodeId;
use tenta_core::atom::{Atom as TentaAtom, atoms};
use tenta_dom::Document;

use super::engine::StyleEngine;
use super::pseudo::ResolveError;
use super::types::PseudoKind;
use crate::cssom::configure_stylo;

impl StyleEngine {
    /// The computed value of `property` for `node` (or one of its pseudo
    /// elements) as CSS text, from the styles of the last flush.
    ///
    /// `None` for an unknown property, for a property Stylo does not implement
    /// (a host reading `getPropertyValue` maps that to `""`), and for a node
    /// with no stored style: a non-element, a detached node, a descendant of a
    /// `display: none` element, or a pseudo-element that generates no box.
    /// This form has no document, so it cannot see the document's staleness or
    /// resolve `attr()`; [`StyleEngine::computed_value_in`] does both and
    /// resolves the nodes above.
    pub fn computed_value(
        &self,
        node: NodeId,
        pseudo: Option<PseudoKind>,
        property: &str,
    ) -> Option<String> {
        let style = match pseudo {
            None => self.style(node)?,
            Some(pseudo) => self.pseudo_style(node, pseudo)?,
        };
        serialize(&style, property)
    }

    /// Like [`StyleEngine::computed_value`], but resolves styles that are not
    /// stored: elements under `display: none`, and the pseudo-elements
    /// `getComputedStyle` answers for even though they generate no box. It also
    /// resolves `attr()` in the `content` of `::before` and `::after`, which
    /// the stored style (and so [`StyleEngine::computed_value`]) keeps as
    /// written.
    ///
    /// [`ResolveError::Stale`] when `doc` is not what the last flush styled,
    /// checked before anything else; `Ok(None)` for an unknown property, a
    /// property Stylo does not implement, and the cases in which
    /// [`StyleEngine::resolve_style`] and [`StyleEngine::resolve_pseudo_style`]
    /// find no style.
    pub fn computed_value_in(
        &self,
        doc: &Document,
        node: NodeId,
        pseudo: Option<PseudoKind>,
        property: &str,
    ) -> Result<Option<String>, ResolveError> {
        let style = match pseudo {
            None => self.resolve_style(doc, node)?,
            Some(pseudo) => self.resolve_pseudo_style(doc, node, pseudo)?,
        };
        let Some(style) = style else {
            return Ok(None);
        };
        let generated = matches!(pseudo, Some(PseudoKind::Before | PseudoKind::After));
        if property == "content"
            && generated
            && let Some(content) = resolved_content(doc, node, &style)
        {
            return Ok(Some(content));
        }
        Ok(serialize(&style, property))
    }

    /// The longhand properties `getComputedStyle` enumerates, sorted by name.
    /// Shorthands and aliases are excluded; custom properties depend on the
    /// element and are not listed.
    pub fn computed_property_names() -> Vec<&'static str> {
        // Which longhands are enabled depends on the process-wide prefs.
        configure_stylo();
        let mut names: Vec<&'static str> = NonCustomPropertyId::iter()
            .map(NonCustomPropertyId::name)
            .filter(|&name| {
                matches!(
                    PropertyId::parse_enabled_for_all_content(name).as_ref().map(PropertyId::as_shorthand),
                    Ok(Err(PropertyDeclarationId::Longhand(longhand))) if longhand.name() == name
                )
            })
            .collect();
        names.sort_unstable();
        names.dedup();
        names
    }
}

/// `property` of `style` as `getComputedStyle().getPropertyValue` reports it.
fn serialize(style: &ComputedValues, property: &str) -> Option<String> {
    let id = PropertyId::parse_enabled_for_all_content(property).ok()?;
    let value = match id.as_shorthand() {
        Ok(shorthand) => shorthand_value(style, shorthand),
        Err(PropertyDeclarationId::Longhand(LonghandId::Transform)) => transform_value(style),
        Err(declaration) => style.computed_value_to_string(declaration),
    };
    Some(web_spelling(&id, value))
}

fn longhand(style: &ComputedValues, longhand: LonghandId) -> String {
    style.computed_value_to_string(PropertyDeclarationId::Longhand(longhand))
}

/// A shorthand as browsers report it for computed values.
///
/// Stylo's serialiser follows CSSOM and leaves out components that equal their
/// initial value, which suits declarations. Browsers write every component of
/// the shorthands below in `getComputedStyle`, so those are joined from the
/// longhands here, in the order each grammar defines. Every other shorthand
/// uses Stylo's text, empty when its longhands cannot be expressed as one.
fn shorthand_value(style: &ComputedValues, shorthand: ShorthandId) -> String {
    use LonghandId as L;
    let join = |ids: &[LonghandId]| -> String {
        ids.iter()
            .map(|&id| longhand(style, id))
            .collect::<Vec<_>>()
            .join(" ")
    };
    match shorthand {
        ShorthandId::Border => {
            let sides = [
                [L::BorderTopWidth, L::BorderTopStyle, L::BorderTopColor],
                [
                    L::BorderRightWidth,
                    L::BorderRightStyle,
                    L::BorderRightColor,
                ],
                [
                    L::BorderBottomWidth,
                    L::BorderBottomStyle,
                    L::BorderBottomColor,
                ],
                [L::BorderLeftWidth, L::BorderLeftStyle, L::BorderLeftColor],
            ];
            let top = join(&sides[0]);
            if sides[1..].iter().all(|side| join(side) == top) {
                top
            } else {
                String::new()
            }
        }
        ShorthandId::BorderTop => join(&[L::BorderTopWidth, L::BorderTopStyle, L::BorderTopColor]),
        ShorthandId::BorderRight => join(&[
            L::BorderRightWidth,
            L::BorderRightStyle,
            L::BorderRightColor,
        ]),
        ShorthandId::BorderBottom => join(&[
            L::BorderBottomWidth,
            L::BorderBottomStyle,
            L::BorderBottomColor,
        ]),
        ShorthandId::BorderLeft => {
            join(&[L::BorderLeftWidth, L::BorderLeftStyle, L::BorderLeftColor])
        }
        ShorthandId::Outline => join(&[L::OutlineColor, L::OutlineStyle, L::OutlineWidth]),
        ShorthandId::ListStyle => {
            join(&[L::ListStylePosition, L::ListStyleImage, L::ListStyleType])
        }
        ShorthandId::FlexFlow => join(&[L::FlexDirection, L::FlexWrap]),
        ShorthandId::Background => background(style),
        // The colour stays `currentcolor` (and so is left out) unless the
        // page set one: resolving it first would always write it.
        ShorthandId::TextDecoration => stylo_shorthand(style, shorthand, false),
        _ => stylo_shorthand(style, shorthand, true),
    }
}

/// Stylo's CSSOM serialisation of `shorthand` from the computed (`resolve`
/// false) or resolved longhands of `style`.
fn stylo_shorthand(style: &ComputedValues, shorthand: ShorthandId, resolve: bool) -> String {
    let mut block = PropertyDeclarationBlock::new();
    for longhand in shorthand.longhands() {
        let mut context = resolved::Context {
            style,
            for_property: PropertyId::NonCustom(longhand.into()),
            current_longhand: Some(longhand),
        };
        block.push(
            style.computed_or_resolved_declaration(longhand, resolve.then_some(&mut context)),
            Importance::Normal,
        );
    }
    let mut out = String::new();
    match block.shorthand_to_css(shorthand, &mut out) {
        Ok(()) => out,
        Err(_) => String::new(),
    }
}

/// `background` with every component of every layer, the colour on the last
/// layer: `<color> <image> <repeat> <attachment> <position> / <size> <origin>
/// <clip>`. Lists shorter than the image list repeat, as the cascade defines.
fn background(style: &ComputedValues) -> String {
    use LonghandId as L;
    let images = longhand(style, L::BackgroundImage);
    let images = split_list(&images);
    let lists = [
        L::BackgroundRepeat,
        L::BackgroundAttachment,
        L::BackgroundPositionX,
        L::BackgroundPositionY,
        L::BackgroundSize,
        L::BackgroundOrigin,
        L::BackgroundClip,
    ]
    .map(|id| longhand(style, id));
    let lists = lists.each_ref().map(|list| split_list(list));
    let layers = images.len();
    (0..layers)
        .map(|index| {
            let [repeat, attachment, x, y, size, origin, clip] =
                lists.each_ref().map(|list| list[index % list.len()]);
            let colour = if index + 1 == layers {
                format!("{} ", longhand(style, L::BackgroundColor))
            } else {
                String::new()
            };
            format!(
                "{colour}{} {repeat} {attachment} {x} {y} / {size} {origin} {clip}",
                images[index]
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Splits a comma-separated computed list at its top-level commas, those
/// outside parentheses and quotes (`url("a,b")`, gradients).
fn split_list(list: &str) -> Vec<&str> {
    let mut items = Vec::new();
    let (mut depth, mut quote, mut escaped, mut start) = (0usize, None, false, 0usize);
    for (at, ch) in list.char_indices() {
        match (quote, ch) {
            (Some(_), _) if escaped => escaped = false,
            (Some(_), '\\') => escaped = true,
            (Some(open), ch) if ch == open => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(ch),
            (None, '(') => depth += 1,
            (None, ')') => depth = depth.saturating_sub(1),
            (None, ',') if depth == 0 => {
                items.push(list[start..at].trim());
                start = at + 1;
            }
            _ => {}
        }
    }
    items.push(list[start..].trim());
    items
}

/// `transform` as the matrix browsers report, `matrix()` for a 2D transform
/// and `matrix3d()` otherwise. A transform that needs the reference box
/// (percentages) keeps its computed text for the host to resolve.
fn transform_value(style: &ComputedValues) -> String {
    let computed = longhand(style, LonghandId::Transform);
    let transform = style.get_box().clone_transform();
    if transform.0.is_empty() {
        return computed;
    }
    let Ok((m, is_3d)) = transform.to_transform_3d_matrix_f64(None) else {
        return computed;
    };
    let numbers = if is_3d {
        vec![
            m.m11, m.m12, m.m13, m.m14, m.m21, m.m22, m.m23, m.m24, m.m31, m.m32, m.m33, m.m34,
            m.m41, m.m42, m.m43, m.m44,
        ]
    } else {
        vec![m.m11, m.m12, m.m21, m.m22, m.m41, m.m42]
    };
    let text: Vec<String> = numbers.into_iter().map(matrix_number).collect();
    format!(
        "{}({})",
        if is_3d { "matrix3d" } else { "matrix" },
        text.join(", ")
    )
}

/// A matrix entry with at most six decimals and no trailing zeros, the
/// precision browsers print.
fn matrix_number(value: f64) -> String {
    let text = format!("{value:.6}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".to_owned()
    } else {
        text.to_owned()
    }
}

/// `content` of a generated box with `attr()` replaced by the attribute of the
/// originating element, as browsers report it: runs of strings and attribute
/// values become one quoted string, counters and quotes stay as written.
/// `None` when `content` is not an item list.
fn resolved_content(doc: &Document, node: NodeId, style: &ComputedValues) -> Option<String> {
    let Content::Items(items) = &style.get_counters().content else {
        return None;
    };
    let mut parts: Vec<String> = Vec::new();
    let mut text: Option<String> = None;
    for item in items.items.iter() {
        let piece = match item {
            ContentItem::String(string) => Some(string.to_string()),
            ContentItem::Attr(attr) => {
                // HTML attribute names are matched case-insensitively on HTML
                // elements of HTML documents only.
                let html = doc.is_html_document() && doc.namespace(node) == Some(atoms::NS_HTML);
                let name = if html {
                    attr.attribute.to_string().to_ascii_lowercase()
                } else {
                    attr.attribute.to_string()
                };
                let name = TentaAtom::new(&name);
                Some(
                    doc.attribute(node, name)
                        .unwrap_or(&attr.fallback)
                        .to_owned(),
                )
            }
            _ => None,
        };
        match piece {
            Some(piece) => text.get_or_insert_with(String::new).push_str(&piece),
            None => {
                parts.extend(text.take().map(quoted));
                parts.push(item.to_css_string());
            }
        }
    }
    parts.extend(text.take().map(quoted));
    Some(parts.join(" "))
}

fn quoted(text: String) -> String {
    let mut out = String::new();
    serialize_string(&text, &mut out).expect("writing to a String cannot fail");
    out
}

/// Spellings where Stylo's differ from what pages see: the Gecko-prefixed
/// `-moz-center`, `-moz-left` and `-moz-right` of `text-align`, which
/// `<center>` and `align=` compute to, are reported as `-webkit-center`,
/// `-webkit-left` and `-webkit-right`, the only names browsers other than
/// Firefox know (and the names the UA sheet is written against).
fn web_spelling(id: &PropertyId, value: String) -> String {
    if !matches!(
        id.as_shorthand(),
        Err(PropertyDeclarationId::Longhand(LonghandId::TextAlign))
    ) {
        return value;
    }
    match value.as_str() {
        "-moz-center" => "-webkit-center".to_owned(),
        "-moz-left" => "-webkit-left".to_owned(),
        "-moz-right" => "-webkit-right".to_owned(),
        _ => value,
    }
}
