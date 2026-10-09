//! The computed style inline layout reads, resolved once per box.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use style::properties::ComputedValues;
use style::properties::longhands::direction::computed_value::T as StyleDirection;
use style::properties::longhands::line_break::computed_value::T as StyleLineBreak;
use style::properties::longhands::overflow_wrap::computed_value::T as StyleOverflowWrap;
use style::properties::longhands::text_wrap_mode::computed_value::T as StyleWrapMode;
use style::properties::longhands::white_space_collapse::computed_value::T as StyleCollapse;
use style::properties::longhands::word_break::computed_value::T as StyleWordBreak;
use style::values::computed::font::{FontFamilyNameSyntax, SingleFontFamily};
use style::values::computed::font::{FontStyle, GenericFontFamily, LineHeight as StyleLineHeight};
use style::values::computed::{
    Length, LengthPercentage, Margin, NonNegativeLengthOrNumber as StyleTabSize,
};
use style::values::generics::box_::GenericBaselineShift as BaselineShift;
use style::values::specified::text::{
    TextAlignKeyword, TextAlignLast, TextOverflowSide, TextTransformCase,
};

use crate::box_tree::{BoxId, BoxTree, LayoutCx};
use crate::font::VerticalMetrics;
use crate::text::shape;
use crate::text::style::{
    FontFeatureSetting, LineBreak, OverflowWrap, Paragraph, TextSpan, TextStyle, WhiteSpace,
    WordBreak,
};
use crate::text::transform::TextTransform;
use crate::types::{Direction, Edges, PaintStyle};

/// How a text item treats white space (`white-space-collapse`).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) enum Handling {
    /// Handling and segment breaks collapse.
    Collapse,
    /// Everything is preserved; segment breaks are forced breaks.
    Preserve,
    /// Handling collapse; segment breaks are forced breaks.
    PreserveBreaks,
    /// Like `Preserve`, but preserved spaces take room and may wrap.
    BreakSpaces,
}

impl Handling {
    /// Whether runs of spaces and tabs collapse.
    pub(super) fn collapses_spaces(self) -> bool {
        matches!(self, Self::Collapse | Self::PreserveBreaks)
    }
}

/// A used `line-height`.
#[derive(Copy, Clone, PartialEq, Debug)]
pub(super) enum LineHeight {
    /// `normal`: the font's own line spacing.
    Normal,
    /// A length, or a number times the font size.
    Fixed(f32),
}

/// `text-align` reduced to what the line builder distinguishes.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) enum Align {
    /// Inline-start edge.
    Start,
    /// Inline-end edge.
    End,
    /// Physical left.
    Left,
    /// Physical right.
    Right,
    /// Centred.
    Center,
    /// Justified; the last line follows `text-align-last`.
    Justify,
}

/// The values a block container contributes to its lines.
#[derive(Copy, Clone, PartialEq, Debug)]
pub(super) struct BlockInfo {
    pub(super) align: Align,
    /// `text-align-last`, `None` for `auto`.
    pub(super) align_last: Option<Align>,
    /// `text-indent` resolved against the containing block, with its keywords.
    pub(super) indent: f32,
    pub(super) indent_hanging: bool,
    pub(super) indent_each_line: bool,
    pub(super) direction: Direction,
    pub(super) ellipsis: bool,
}

/// Everything inline layout needs to know about one box.
pub(super) struct BoxInfo {
    pub(super) text: TextStyle,
    pub(super) collapse: Handling,
    pub(super) wraps: bool,
    pub(super) case: TextTransform,
    /// Distance between tab stops; zero where tabs collapse like spaces.
    pub(super) tab_stop: f32,
    /// Metrics of the primary font at the box's size.
    pub(super) metrics: VerticalMetrics,
    pub(super) line_height: LineHeight,
    /// `vertical-align` is `baseline`.
    pub(super) baseline_aligned: bool,
    pub(super) direction: Direction,
    pub(super) padding: Edges,
    pub(super) border: Edges,
    pub(super) margin: Edges,
    /// The paint style of the box's text fragments.
    pub(super) text_paint: Arc<PaintStyle>,
}

impl BoxInfo {
    /// Whether the box has any horizontal or vertical edge.
    pub(super) fn has_edges(&self) -> bool {
        [self.padding, self.border, self.margin]
            .iter()
            .any(|e| e.horizontal() != 0.0 || e.vertical() != 0.0)
    }
}

/// Per-box info, read lazily and kept for the duration of one layout.
pub(super) struct Infos {
    index: HashMap<u32, usize>,
    list: Vec<BoxInfo>,
    percentage_base: f32,
}

impl Infos {
    /// An empty cache; padding and margin percentages resolve against
    /// `percentage_base`.
    pub(super) fn new(percentage_base: f32) -> Self {
        Self {
            index: HashMap::new(),
            list: Vec::new(),
            percentage_base,
        }
    }

    /// Reads `id` unless it has been read.
    pub(super) fn ensure(&mut self, cx: &mut LayoutCx, tree: &BoxTree, id: BoxId) {
        if self.index.contains_key(&id.0) {
            return;
        }
        let info = read_box(cx, tree, id, self.percentage_base);
        self.index.insert(id.0, self.list.len());
        self.list.push(info);
    }

    /// The info of a box passed to [`Infos::ensure`] earlier.
    pub(super) fn get(&self, id: BoxId) -> &BoxInfo {
        &self.list[self.index[&id.0]]
    }
}

fn px(length: Length) -> f32 {
    length.px()
}

fn resolve(value: &LengthPercentage, base: f32) -> f32 {
    value.resolve(Length::new(base)).px()
}

fn family_list(style: &ComputedValues) -> String {
    let mut out = String::new();
    for family in style.get_font().font_family.families.iter() {
        if !out.is_empty() {
            out.push_str(", ");
        }
        match family {
            SingleFontFamily::FamilyName(name) => {
                let text = name.name.to_string();
                if name.syntax == FontFamilyNameSyntax::Quoted {
                    out.push('"');
                    for ch in text.chars() {
                        if matches!(ch, '"' | '\\') {
                            out.push('\\');
                        }
                        out.push(ch);
                    }
                    out.push('"');
                } else {
                    out.push_str(&text);
                }
            }
            SingleFontFamily::Generic(generic) => out.push_str(match generic {
                GenericFontFamily::Serif => "serif",
                GenericFontFamily::SansSerif => "sans-serif",
                GenericFontFamily::Monospace => "monospace",
                GenericFontFamily::Cursive => "cursive",
                GenericFontFamily::Fantasy => "fantasy",
                GenericFontFamily::SystemUi => "system-ui",
                GenericFontFamily::None => continue,
            }),
        }
    }
    out
}

fn text_style(style: &ComputedValues, size: f32, collapse: Handling, wraps: bool) -> TextStyle {
    let font = style.get_font();
    let inherited = style.get_inherited_text();
    let white_space = match (collapse, wraps) {
        (Handling::Collapse, true) => WhiteSpace::Normal,
        (Handling::Collapse | Handling::PreserveBreaks, false) => WhiteSpace::Nowrap,
        (Handling::Preserve | Handling::BreakSpaces, false) => WhiteSpace::Pre,
        (Handling::Preserve, true) => WhiteSpace::PreWrap,
        (Handling::PreserveBreaks, true) => WhiteSpace::PreLine,
        (Handling::BreakSpaces, true) => WhiteSpace::BreakSpaces,
    };
    let lang = font._x_lang.0.to_string();
    TextStyle {
        family_list: family_list(style),
        size,
        weight: font.font_weight.value().clamp(1.0, 1000.0).round() as u16,
        italic: font.font_style != FontStyle::NORMAL,
        letter_spacing: resolve(&inherited.letter_spacing.0, size),
        word_spacing: resolve(&inherited.word_spacing, size),
        features: font
            .font_feature_settings
            .0
            .iter()
            .map(|f| FontFeatureSetting {
                tag: f.tag.0.to_be_bytes(),
                value: f.value.max(0).min(i32::from(u16::MAX)) as u16,
            })
            .collect(),
        lang: (!lang.is_empty()).then_some(lang),
        white_space,
        line_break: match inherited.line_break {
            StyleLineBreak::Anywhere => LineBreak::Anywhere,
            _ => LineBreak::Auto,
        },
        word_break: match inherited.word_break {
            StyleWordBreak::Normal => WordBreak::Normal,
            StyleWordBreak::BreakAll => WordBreak::BreakAll,
            StyleWordBreak::KeepAll => WordBreak::KeepAll,
        },
        overflow_wrap: match inherited.overflow_wrap {
            StyleOverflowWrap::Normal => OverflowWrap::Normal,
            StyleOverflowWrap::BreakWord => OverflowWrap::BreakWord,
            StyleOverflowWrap::Anywhere => OverflowWrap::Anywhere,
        },
    }
}

/// The text fragments of a box paint only text, so they inherit nothing that
/// would open a stacking context of their own or draw the box twice.
/// A paint style and the text style derived from it.
type Derived = (Arc<PaintStyle>, Arc<PaintStyle>);

fn text_paint(paint: &Arc<PaintStyle>) -> Arc<PaintStyle> {
    thread_local! {
        /// Derived styles by the address of the style they come from. The
        /// source is kept so that its address cannot be reused while it is here.
        static DERIVED: RefCell<HashMap<usize, Derived>> = RefCell::new(HashMap::new());
    }
    const MAX_DERIVED: usize = 8192;
    DERIVED.with_borrow_mut(|derived| {
        let key = Arc::as_ptr(paint) as usize;
        if let Some((_, text)) = derived.get(&key) {
            return text.clone();
        }
        if derived.len() >= MAX_DERIVED {
            derived.clear();
        }
        let text = Arc::new(PaintStyle {
            visibility_hidden: paint.visibility_hidden,
            pointer_events: paint.pointer_events,
            color: paint.color,
            text_decoration: paint.text_decoration,
            text_shadow: paint.text_shadow.clone(),
            ..PaintStyle::default()
        });
        derived.insert(key, (paint.clone(), text.clone()));
        text
    })
}

/// The style of line fragments, which paint nothing. Hit testing looks at
/// fragments that have a node, and lines have none; the two inherited flags
/// are still carried so that the line agrees with its block.
pub(super) fn line_paint(block: &PaintStyle) -> Arc<PaintStyle> {
    thread_local! {
        static LINES: [Arc<PaintStyle>; 4] = std::array::from_fn(|i| {
            Arc::new(PaintStyle {
                pointer_events: i & 1 == 0,
                visibility_hidden: i & 2 != 0,
                ..PaintStyle::default()
            })
        });
    }
    let index = usize::from(!block.pointer_events) | usize::from(block.visibility_hidden) << 1;
    LINES.with(|lines| lines[index].clone())
}

fn edges(style: &ComputedValues, base: f32, paint: &PaintStyle) -> (Edges, Edges, Edges) {
    let p = style.get_padding();
    let padding = Edges {
        top: resolve(&p.padding_top.0, base),
        right: resolve(&p.padding_right.0, base),
        bottom: resolve(&p.padding_bottom.0, base),
        left: resolve(&p.padding_left.0, base),
    };
    let m = style.get_margin();
    let margin_of = |m: &Margin| match m {
        Margin::LengthPercentage(lp) => resolve(lp, base),
        _ => 0.0,
    };
    let margin = Edges {
        top: margin_of(&m.margin_top),
        right: margin_of(&m.margin_right),
        bottom: margin_of(&m.margin_bottom),
        left: margin_of(&m.margin_left),
    };
    (padding, paint.border.widths, margin)
}

fn read_box(cx: &mut LayoutCx, tree: &BoxTree, id: BoxId, percentage_base: f32) -> BoxInfo {
    let b = tree.get(id);
    let style: &ComputedValues = &b.style;
    let inherited = style.get_inherited_text();
    let collapse = match inherited.white_space_collapse {
        StyleCollapse::Collapse => Handling::Collapse,
        StyleCollapse::Preserve => Handling::Preserve,
        StyleCollapse::PreserveBreaks => Handling::PreserveBreaks,
        StyleCollapse::BreakSpaces => Handling::BreakSpaces,
    };
    let wraps = inherited.text_wrap_mode == StyleWrapMode::Wrap;
    let size = style.get_font().font_size.computed_size().px();
    let text = text_style(style, size, collapse, wraps);
    let metrics = cx
        .fonts
        .resolve(&text.family_list, text.weight, text.italic)
        .vertical_metrics(size);
    let line_height = match style.get_font().line_height {
        StyleLineHeight::Normal => LineHeight::Normal,
        StyleLineHeight::Number(n) => LineHeight::Fixed(layout_unit_floor(n.0 * size)),
        StyleLineHeight::Length(l) => LineHeight::Fixed(layout_unit_floor(px(l.0))),
    };
    let case = match style.clone_text_transform().case() {
        TextTransformCase::None => TextTransform::None,
        TextTransformCase::Uppercase => TextTransform::Uppercase,
        TextTransformCase::Lowercase => TextTransform::Lowercase,
        TextTransformCase::Capitalize => TextTransform::Capitalize,
    };
    let tab_stop = if matches!(collapse, Handling::Preserve | Handling::BreakSpaces) {
        let mut space = || space_width(cx, &text) + text.letter_spacing + text.word_spacing;
        match &inherited.tab_size {
            StyleTabSize::Number(n) => n.0 * space(),
            StyleTabSize::Length(l) => px(l.0),
        }
    } else {
        0.0
    };
    let baseline_aligned = {
        let b = style.get_box();
        let shift_zero = matches!(&b.baseline_shift,
            BaselineShift::Length(lp) if lp.to_length().is_some_and(|l| l.px() == 0.0));
        shift_zero && b.alignment_baseline == style::values::computed::AlignmentBaseline::Baseline
    };
    let (padding, border, margin) = edges(style, percentage_base, &b.paint);
    BoxInfo {
        text,
        collapse,
        wraps,
        case,
        tab_stop,
        metrics,
        line_height,
        baseline_aligned,
        direction: direction_of(style),
        padding,
        border,
        margin,
        text_paint: text_paint(&b.paint),
    }
}

/// The advance of U+0020 in `style`'s first available font.
fn space_width(cx: &mut LayoutCx, style: &TextStyle) -> f32 {
    let style = TextStyle {
        white_space: WhiteSpace::Pre,
        letter_spacing: 0.0,
        word_spacing: 0.0,
        ..style.clone()
    };
    let spans = [TextSpan {
        range: 0..1,
        style: &style,
    }];
    let paragraph = Paragraph {
        text: " ",
        spans: &spans,
        direction: Direction::Ltr,
    };
    shape::shape_chunk(cx.fonts, &paragraph).width
}

fn direction_of(style: &ComputedValues) -> Direction {
    match style.get_inherited_box().direction {
        StyleDirection::Ltr => Direction::Ltr,
        StyleDirection::Rtl => Direction::Rtl,
    }
}

/// The distances from the baseline to the top and to the bottom of the
/// inline box that `line-height` makes of a font with `metrics`: the font's
/// ascent and descent plus half of the leading on each side, the half above
/// rounded down as Blink does.
pub(super) fn leading(line_height: LineHeight, metrics: VerticalMetrics) -> (f32, f32) {
    let used = match line_height {
        LineHeight::Normal => metrics.line_spacing(),
        LineHeight::Fixed(value) => value,
    };
    let above = ((used - (metrics.ascent + metrics.descent)) / 2.0).floor();
    (metrics.ascent + above, used - metrics.ascent - above)
}

/// Rounds down to Blink's `LayoutUnit` precision of 1/64 pixel.
pub(super) fn layout_unit_floor(value: f32) -> f32 {
    (value * 64.0).floor() / 64.0
}

fn align_of(keyword: TextAlignKeyword) -> Align {
    match keyword {
        TextAlignKeyword::Start => Align::Start,
        TextAlignKeyword::End => Align::End,
        TextAlignKeyword::Left | TextAlignKeyword::MozLeft => Align::Left,
        TextAlignKeyword::Right | TextAlignKeyword::MozRight => Align::Right,
        TextAlignKeyword::Center | TextAlignKeyword::MozCenter => Align::Center,
        TextAlignKeyword::Justify => Align::Justify,
    }
}

/// Whether `text-overflow` puts an ellipsis at the end side of a line. A single
/// value is stored as the end side; two logical values are start then end, and
/// two physical ones left then right.
fn ends_in_ellipsis(style: &ComputedValues) -> bool {
    let overflow = &style.get_text().text_overflow;
    let end = if !overflow.sides_are_logical && direction_of(style) == Direction::Rtl {
        &overflow.first
    } else {
        &overflow.second
    };
    matches!(end, TextOverflowSide::Ellipsis)
}

/// Reads the block-level properties of the container `root`; `base` is the
/// containing block width that `text-indent` percentages resolve against.
pub(super) fn read_block(tree: &BoxTree, root: BoxId, base: f32) -> BlockInfo {
    let style: &ComputedValues = &tree.get(root).style;
    let inherited = style.get_inherited_text();
    let indent = &inherited.text_indent;
    BlockInfo {
        align: align_of(inherited.text_align),
        align_last: match inherited.text_align_last {
            TextAlignLast::Auto => None,
            TextAlignLast::Start => Some(Align::Start),
            TextAlignLast::End => Some(Align::End),
            TextAlignLast::Left => Some(Align::Left),
            TextAlignLast::Right => Some(Align::Right),
            TextAlignLast::Center => Some(Align::Center),
            TextAlignLast::Justify => Some(Align::Justify),
        },
        indent: resolve(&indent.length, base),
        indent_hanging: indent.hanging,
        indent_each_line: indent.each_line,
        direction: direction_of(style),
        ellipsis: ends_in_ellipsis(style),
    }
}
