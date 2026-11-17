//! The tree walk that turns elements into boxes and inline item lists.
//!
//! The walk is iterative (an explicit work stack) so a deeply nested document
//! cannot overflow the native stack. Each block container being built owns a
//! [`Frame`] collecting its children in document order; when the container
//! closes, [`Builder::finish`] decides between "one inline formatting
//! context", "block children" and the mixed case that needs anonymous block
//! wrappers (CSS 2.1 section 9.2.1.1).

use std::sync::Arc;

use smallvec::SmallVec;
use style::computed_values::white_space_collapse::T as WhiteSpaceCollapse;
use style::values::specified::box_::{Display, DisplayInside, DisplayOutside};
use tenta_core::NodeId;
use tenta_core::atom::atoms;
use tenta_css::cascade::{PseudoKind, ResolveError, Style};

use super::anonymous::AnonymousStyles;
use super::counters::Counters;
use super::paint::paint_style;
use super::replaced::{self, ReplacedInfo};
use crate::box_tree::{
    BoxFlags, BoxId, BoxKind, BoxTree, ImageState, InlineFormattingContext, InlineItem, LayoutBox,
    LayoutCx, TextSource,
};
use crate::types::{DecorationPaint, MarkerKind, TableKind};

mod table;

/// How a finished box joins its parent's child list.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) enum Level {
    Block,
    Inline,
    Float,
    AbsPos,
}

enum Entry {
    Inline(InlineItem),
    Block(BoxId),
    OutOfFlow(BoxId, Level),
}

struct InlineCtx {
    box_id: BoxId,
    decoration: DecorationPaint,
    /// An `OpenInline` for this box was emitted before, so the next one resumes it.
    started: bool,
}

/// A block container whose children are being collected.
struct Frame {
    box_id: BoxId,
    style: Style,
    decoration: DecorationPaint,
    level: Level,
    entries: Vec<Entry>,
    /// Inline boxes enclosing the current position, outermost first.
    inlines: Vec<InlineCtx>,
    /// How many of `inlines` currently have an `OpenInline` in `entries`. A
    /// block child closes them and the next inline content reopens them, which
    /// is how an inline box is split around a block.
    opened: usize,
}

impl Frame {
    fn open_all(&mut self) {
        while self.opened < self.inlines.len() {
            let ctx = &mut self.inlines[self.opened];
            let item = InlineItem::OpenInline {
                box_id: ctx.box_id,
                resumes: ctx.started,
            };
            ctx.started = true;
            self.entries.push(Entry::Inline(item));
            self.opened += 1;
        }
    }

    /// Ends the pieces of every open inline box because a block-level child
    /// interrupts them.
    fn close_all(&mut self) {
        while self.opened > 0 {
            self.opened -= 1;
            let box_id = self.inlines[self.opened].box_id;
            self.entries.push(Entry::Inline(InlineItem::CloseInline {
                box_id,
                interrupted: true,
            }));
        }
    }

    fn push_item(&mut self, item: InlineItem) {
        self.open_all();
        self.entries.push(Entry::Inline(item));
    }

    fn pop_inline(&mut self) {
        let Some(ctx) = self.inlines.last() else {
            return;
        };
        let (started, box_id) = (ctx.started, ctx.box_id);
        if started && self.opened < self.inlines.len() {
            // A block interrupted the box and nothing resumed it. The end edge
            // still has to belong to some piece, so emit an empty last one.
            self.open_all();
        }
        if self.opened == self.inlines.len() {
            self.opened -= 1;
            self.entries.push(Entry::Inline(InlineItem::CloseInline {
                box_id,
                interrupted: false,
            }));
        }
        self.inlines.pop();
    }

    fn style_box(&self) -> BoxId {
        self.inlines.last().map_or(self.box_id, |c| c.box_id)
    }

    fn decoration(&self) -> DecorationPaint {
        self.inlines
            .last()
            .map_or(self.decoration, |c| c.decoration)
    }
}

enum Task {
    Visit(NodeId),
    Pseudo(NodeId, Style),
    Leave { kind: Leave, counter_mark: usize },
}

#[derive(Copy, Clone)]
enum Leave {
    /// An inline box or a `display: contents` element.
    Inline,
    /// A block container.
    Frame,
}

/// What an element becomes.
enum Shape {
    Replaced(ReplacedInfo),
    InlineBox,
    Container,
}

pub(super) struct Builder<'a, 'cx> {
    pub(super) cx: &'a LayoutCx<'cx>,
    pub(super) tree: BoxTree,
    frames: Vec<Frame>,
    work: Vec<Task>,
    pub(super) counters: Counters,
    /// Elements entered and not yet left; the scope of new counters.
    pub(super) depth: u32,
    pub(super) quote_depth: usize,
    /// An author or user sheet has a `::marker` rule.
    author_marker_rules: bool,
    /// Default marker styles by the address of the list item's style; the
    /// styles stay alive in the tree for the whole build.
    marker_cache:
        std::collections::HashMap<*const style::properties::ComputedValues, Option<Style>>,
    #[cfg(all(test, feature = "embedded-fonts"))]
    pub(super) marker_resolves: usize,
    pub(super) anon: AnonymousStyles,
}

fn is_floated(style: &style::properties::ComputedValues) -> bool {
    !matches!(
        style.get_box().float,
        style::computed_values::float::T::None
    )
}

fn is_abspos(style: &style::properties::ComputedValues) -> bool {
    use style::computed_values::position::T;
    matches!(style.get_box().position, T::Absolute | T::Fixed)
}

fn is_whitespace_only(text: &str) -> bool {
    text.bytes()
        .all(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0c))
}

impl<'a, 'cx> Builder<'a, 'cx> {
    pub(super) fn new(cx: &'a LayoutCx<'cx>) -> Self {
        Self {
            cx,
            tree: BoxTree::new(cx.doc.mutation_generation()),
            frames: Vec::new(),
            work: Vec::new(),
            counters: Counters::default(),
            depth: 0,
            quote_depth: 0,
            author_marker_rules: cx.styles.has_author_rules(PseudoKind::Marker),
            marker_cache: std::collections::HashMap::new(),
            #[cfg(all(test, feature = "embedded-fonts"))]
            marker_resolves: 0,
            anon: AnonymousStyles::new(),
        }
    }

    pub(super) fn run(&mut self) -> BoxTree {
        let doc = self.cx.doc;
        let root_element = doc.document_element();
        let direct = root_element
            .and_then(|el| self.cx.styles.style(el))
            .is_some_and(|s| {
                let d = s.get_box().display;
                !d.is_none()
                    && !d.is_contents()
                    && d.outside() == DisplayOutside::Block
                    && !is_floated(&s)
                    && !is_abspos(&s)
            });
        if !direct {
            self.push_anonymous_root();
        }
        if let Some(el) = root_element {
            self.work.push(Task::Visit(el));
        }
        while let Some(task) = self.work.pop() {
            self.step(task);
        }
        if let Some(frame) = self.frames.pop() {
            self.finish(frame);
        }
        std::mem::replace(&mut self.tree, BoxTree::new(0))
    }

    fn push_anonymous_root(&mut self) {
        let style = self.anon.initial();
        let decoration = DecorationPaint::default();
        let (paint, paint_flags) = paint_style(&style, &decoration);
        let paint = Arc::new(paint);
        let mut root = LayoutBox::new(
            NodeId::NONE,
            BoxId::NONE,
            BoxKind::FlowRoot,
            style.clone(),
            paint,
        );
        root.flags = BoxFlags::ANONYMOUS | BoxFlags::ESTABLISHES_BFC | paint_flags;
        let box_id = self.tree.push(root);
        self.frames.push(Frame {
            box_id,
            style,
            decoration,
            level: Level::Block,
            entries: Vec::new(),
            inlines: Vec::new(),
            opened: 0,
        });
    }

    fn step(&mut self, task: Task) {
        match task {
            Task::Visit(node) => self.visit(node),
            Task::Pseudo(node, style) => self.visit_pseudo(node, &style),
            Task::Leave { kind, counter_mark } => {
                self.depth -= 1;
                self.counters.truncate(counter_mark);
                match kind {
                    Leave::Inline => {
                        if let Some(frame) = self.frames.last_mut() {
                            frame.pop_inline();
                        }
                    }
                    Leave::Frame => {
                        if let Some(frame) = self.frames.pop() {
                            let level = frame.level;
                            let id = self.finish(frame);
                            self.attach(id, level);
                        }
                    }
                }
            }
        }
    }

    fn visit(&mut self, node: NodeId) {
        let doc = self.cx.doc;
        match doc.kind(node) {
            tenta_dom::NodeKind::Element => {
                let Some(style) = self.cx.styles.style(node) else {
                    return;
                };
                let alt = self.broken_image_alt(node);
                self.enter(node, style, alt);
            }
            tenta_dom::NodeKind::Text | tenta_dom::NodeKind::CData => self.visit_text(node),
            _ => {}
        }
    }

    /// The text a broken `<img>` with a non-empty `alt` is laid out as: an
    /// inline box holding it, with no icon and no frame (the fallback Chrome
    /// shows for an image without usable dimensions).
    fn broken_image_alt(&self, node: NodeId) -> Option<String> {
        let doc = self.cx.doc;
        if doc.tag(node) != Some(atoms::IMG)
            || doc.namespace(node) != Some(atoms::NS_HTML)
            || self.cx.images.state(node) != ImageState::Broken
        {
            return None;
        }
        let alt = doc.attribute(node, atoms::ALT)?.trim();
        (!alt.is_empty()).then(|| alt.to_owned())
    }

    fn visit_text(&mut self, node: NodeId) {
        let Some(len) = self.cx.doc.text_data(node).map(str::len) else {
            return;
        };
        let Some(frame) = self.frames.last_mut() else {
            return;
        };
        if len == 0 {
            return;
        }
        let style_box = frame.style_box();
        frame.push_item(InlineItem::Text {
            source: TextSource::Node {
                node,
                range: 0..len as u32,
            },
            style_box,
        });
    }

    /// Appends text the builder made up (generated content, marker strings).
    pub(super) fn push_owned_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if let Some(frame) = self.frames.last_mut() {
            let style_box = frame.style_box();
            frame.push_item(InlineItem::Text {
                source: TextSource::Owned(Arc::from(text)),
                style_box,
            });
        }
    }

    fn visit_pseudo(&mut self, node: NodeId, style: &Style) {
        if let Some(text) = self.generated_content(node, style) {
            self.enter(node, style.clone(), Some(text));
        }
    }

    /// The cascaded style of a list item's `::marker`, including the
    /// user-agent `::marker` rule. Without author `::marker` rules the answer
    /// depends only on the item's own style, so it is resolved once per
    /// distinct item style; otherwise every item is resolved on its own.
    pub(super) fn marker_style(&mut self, node: NodeId, item_style: &Style) -> Option<Style> {
        if self.author_marker_rules {
            return self.resolve_marker(node);
        }
        let key = std::ptr::from_ref::<style::properties::ComputedValues>(item_style);
        if let Some(cached) = self.marker_cache.get(&key) {
            return cached.clone();
        }
        let resolved = self.resolve_marker(node);
        self.marker_cache.insert(key, resolved.clone());
        resolved
    }

    fn resolve_marker(&mut self, node: NodeId) -> Option<Style> {
        #[cfg(all(test, feature = "embedded-fonts"))]
        {
            self.marker_resolves += 1;
        }
        match self
            .cx
            .styles
            .resolve_pseudo_style(self.cx.doc, node, PseudoKind::Marker)
        {
            Ok(style) => style,
            Err(ResolveError::Stale) => {
                debug_assert!(false, "the style engine was not flushed for this document");
                None
            }
            // The marker then inherits from the list item, losing only the
            // `::marker` rules.
            Err(ResolveError::Failed) => None,
        }
    }

    fn current_decoration(&self) -> DecorationPaint {
        self.frames
            .last()
            .map_or_else(DecorationPaint::default, Frame::decoration)
    }

    fn plan(
        &self,
        node: NodeId,
        style: &style::properties::ComputedValues,
        generated: bool,
        level: Level,
    ) -> (BoxKind, Shape) {
        let display = style.get_box().display;
        if !generated && let Some(info) = replaced::classify(self.cx.doc, node) {
            return (BoxKind::Replaced(info.kind), Shape::Replaced(info));
        }
        let inline_level = level == Level::Inline;
        if display.outside() == DisplayOutside::TableCaption {
            return (BoxKind::Table(TableKind::Caption), Shape::Container);
        }
        let kind = match display.inside() {
            DisplayInside::Flex => BoxKind::Flex,
            DisplayInside::Grid => BoxKind::Grid,
            DisplayInside::Table => BoxKind::Table(TableKind::Table),
            DisplayInside::TableRowGroup
            | DisplayInside::TableHeaderGroup
            | DisplayInside::TableFooterGroup => BoxKind::Table(TableKind::RowGroup),
            DisplayInside::TableRow => BoxKind::Table(TableKind::Row),
            DisplayInside::TableCell => BoxKind::Table(TableKind::Cell),
            DisplayInside::TableColumn => BoxKind::Table(TableKind::Column),
            DisplayInside::TableColumnGroup => BoxKind::Table(TableKind::ColumnGroup),
            DisplayInside::FlowRoot if inline_level => BoxKind::InlineBlock,
            DisplayInside::FlowRoot => BoxKind::FlowRoot,
            _ if display.is_list_item() => BoxKind::ListItem { marker: None },
            _ if inline_level => return (BoxKind::Inline, Shape::InlineBox),
            _ => BoxKind::Block,
        };
        (kind, Shape::Container)
    }

    /// Creates the box (or boxes) for one element or generated pseudo-element
    /// and schedules its children.
    pub(super) fn enter(&mut self, node: NodeId, style: Style, generated: Option<String>) {
        let display: Display = style.get_box().display;
        if display.is_none() {
            return;
        }
        let is_generated = generated.is_some();
        if display.is_contents() {
            if is_generated || replaced::classify(self.cx.doc, node).is_some() {
                return;
            }
            self.enter_contents(node, &style);
            return;
        }

        if !is_generated
            && self.cx.doc.namespace(node) == Some(atoms::NS_HTML)
            && self.cx.doc.tag(node) == Some(atoms::WBR)
        {
            self.push_inline_item(InlineItem::SoftBreak);
            return;
        }

        if !is_generated {
            self.apply_counters(node, &style, false);
        }
        let counter_mark = self.counters.len();

        let level = if is_floated(&style) {
            Level::Float
        } else if is_abspos(&style) {
            Level::AbsPos
        } else if display.outside() == DisplayOutside::Inline {
            Level::Inline
        } else {
            Level::Block
        };
        let (mut kind, mut shape) = self.plan(node, &style, is_generated, level);
        if matches!(level, Level::Float | Level::AbsPos) && matches!(shape, Shape::InlineBox) {
            kind = BoxKind::FlowRoot;
            shape = Shape::Container;
        }

        let root = self.frames.is_empty();
        let overflow = {
            let b = style.get_box();
            [b.overflow_x, b.overflow_y]
        };
        let scrolls = overflow.iter().any(|o| {
            !matches!(
                o,
                style::values::specified::box_::Overflow::Visible
                    | style::values::specified::box_::Overflow::Clip
            )
        });
        let is_container = matches!(shape, Shape::Container);
        let in_flow_item = is_container
            && !matches!(level, Level::Float | Level::AbsPos)
            && self
                .frames
                .last()
                .is_some_and(|f| self.is_flex_or_grid(f.box_id));
        let contained = is_container
            && style.get_box().contain.intersects(
                style::values::specified::box_::Contain::LAYOUT
                    | style::values::specified::box_::Contain::PAINT,
            );
        let fieldset = is_container
            && !is_generated
            && self.cx.doc.namespace(node) == Some(atoms::NS_HTML)
            && self.cx.doc.tag(node) == Some(atoms::FIELDSET);
        let bfc = root
            || scrolls
            || in_flow_item
            || contained
            || fieldset
            || matches!(level, Level::Float | Level::AbsPos)
            || matches!(
                kind,
                BoxKind::InlineBlock
                    | BoxKind::FlowRoot
                    | BoxKind::Flex
                    | BoxKind::Grid
                    | BoxKind::Table(TableKind::Table | TableKind::Cell | TableKind::Caption)
            );
        if bfc && kind == BoxKind::Block {
            kind = BoxKind::FlowRoot;
        }

        let mut flags = BoxFlags::empty();
        if bfc && !matches!(shape, Shape::Replaced(_)) {
            flags |= BoxFlags::ESTABLISHES_BFC;
        }
        match level {
            Level::Inline => flags |= BoxFlags::INLINE_LEVEL,
            Level::Float => flags |= BoxFlags::OUT_OF_FLOW | BoxFlags::FLOAT,
            Level::AbsPos => flags |= BoxFlags::OUT_OF_FLOW | BoxFlags::ABSPOS,
            Level::Block => {}
        }
        if matches!(shape, Shape::Replaced(_)) {
            flags |= BoxFlags::REPLACED;
        }
        if is_generated {
            flags |= BoxFlags::GENERATED;
        }

        // Decorations are drawn through a box's in-flow descendants, but not
        // through floats, positioned boxes, atomic inlines or replaced content.
        let propagates = matches!(shape, Shape::InlineBox)
            || (level == Level::Block
                && !matches!(kind, BoxKind::InlineBlock | BoxKind::Replaced(_)));
        let ancestor = if propagates {
            self.current_decoration()
        } else {
            DecorationPaint::default()
        };
        let (mut paint, paint_flags) = paint_style(&style, &ancestor);
        paint.generated = is_generated;
        let paint = Arc::new(paint);
        let decoration = paint.text_decoration;
        let enclosing = self
            .frames
            .last()
            .and_then(|f| f.inlines.last())
            .map(|c| c.box_id);
        let parent = match shape {
            Shape::InlineBox => enclosing.unwrap_or(BoxId::NONE),
            _ => BoxId::NONE,
        };
        let mut layout_box = LayoutBox::new(node, parent, kind, style.clone(), paint);
        layout_box.flags = flags | paint_flags;
        let box_id = self.tree.push(layout_box);

        match shape {
            Shape::Replaced(info) => {
                if let Some(text) = info.text.filter(|t| !t.is_empty()) {
                    self.tree.get_mut(box_id).ifc = Some(Box::new(InlineFormattingContext {
                        items: vec![InlineItem::Text {
                            source: TextSource::Owned(Arc::from(text)),
                            style_box: box_id,
                        }],
                        root: box_id,
                    }));
                }
                self.attach(box_id, level);
            }
            Shape::InlineBox if self.is_line_break(node, is_generated) => {
                self.push_inline_item(InlineItem::ForcedBreak { box_id });
            }
            Shape::InlineBox => {
                if let Some(frame) = self.frames.last_mut() {
                    frame.inlines.push(InlineCtx {
                        box_id,
                        decoration,
                        started: false,
                    });
                    frame.open_all();
                }
                self.depth += 1;
                self.schedule_children(node, Leave::Inline, counter_mark, generated);
            }
            Shape::Container => {
                self.frames.push(Frame {
                    box_id,
                    style: style.clone(),
                    decoration,
                    level,
                    entries: Vec::new(),
                    inlines: Vec::new(),
                    opened: 0,
                });
                if !is_generated && display.is_list_item() {
                    self.push_marker(node, box_id, &style);
                }
                self.depth += 1;
                self.schedule_children(node, Leave::Frame, counter_mark, generated);
            }
        }
    }

    fn is_line_break(&self, node: NodeId, generated: bool) -> bool {
        !generated
            && self.cx.doc.namespace(node) == Some(atoms::NS_HTML)
            && self.cx.doc.tag(node) == Some(atoms::BR)
    }

    /// A `display: contents` element has no box, yet its text must still be
    /// styled by it, so it contributes an anonymous inline box that only
    /// appears in the item list when content actually sits inside it.
    fn enter_contents(&mut self, node: NodeId, style: &Style) {
        self.apply_counters(node, style, false);
        let counter_mark = self.counters.len();
        let decoration = self.current_decoration();
        let anon_style = self.anon.derive(style, false);
        let (paint, paint_flags) = paint_style(&anon_style, &decoration);
        let paint = Arc::new(paint);
        let decoration = paint.text_decoration;
        let mut layout_box = LayoutBox::new(
            NodeId::NONE,
            self.frames
                .last()
                .and_then(|f| f.inlines.last())
                .map_or(BoxId::NONE, |c| c.box_id),
            BoxKind::Inline,
            anon_style,
            paint,
        );
        layout_box.flags = BoxFlags::ANONYMOUS | BoxFlags::INLINE_LEVEL | paint_flags;
        let box_id = self.tree.push(layout_box);
        if let Some(frame) = self.frames.last_mut() {
            frame.inlines.push(InlineCtx {
                box_id,
                decoration,
                started: false,
            });
        }
        self.depth += 1;
        self.schedule_children(node, Leave::Inline, counter_mark, None);
    }

    fn schedule_children(
        &mut self,
        node: NodeId,
        leave: Leave,
        counter_mark: usize,
        generated: Option<String>,
    ) {
        if let Some(text) = generated {
            self.push_owned_text(&text);
            self.work.push(Task::Leave {
                kind: leave,
                counter_mark,
            });
            return;
        }
        self.work.push(Task::Leave {
            kind: leave,
            counter_mark,
        });
        let styles = self.cx.styles;
        if let Some(style) = styles.pseudo_style(node, PseudoKind::After) {
            self.work.push(Task::Pseudo(node, style));
        }
        let start = self.work.len();
        // The flat tree is what is rendered: a shadow host's boxes are those
        // of its shadow tree, and a slot's those of the nodes assigned to it.
        match self.cx.doc.flat_children(node) {
            Some(children) => self.work.extend(children.into_iter().map(Task::Visit)),
            None => self
                .work
                .extend(self.cx.doc.children(node).map(Task::Visit)),
        }
        self.work[start..].reverse();
        if let Some(style) = styles.pseudo_style(node, PseudoKind::Before) {
            self.work.push(Task::Pseudo(node, style));
        }
    }

    /// Joins a finished box to the container being built.
    pub(super) fn attach(&mut self, id: BoxId, level: Level) {
        let Some(frame) = self.frames.last_mut() else {
            return;
        };
        match level {
            Level::Block => {
                frame.close_all();
                frame.entries.push(Entry::Block(id));
                let parent = frame.box_id;
                self.tree.get_mut(id).parent = parent;
            }
            Level::Inline => frame.push_item(InlineItem::Atomic { box_id: id }),
            Level::Float | Level::AbsPos => frame.entries.push(Entry::OutOfFlow(id, level)),
        }
    }

    /// Pushes an inline-level item into the container being built.
    pub(super) fn push_inline_item(&mut self, item: InlineItem) {
        if let Some(frame) = self.frames.last_mut() {
            frame.push_item(item);
        }
    }

    /// Adds a list marker box. An outside marker keeps its text in its own
    /// formatting context; an inside one is inline content of the list item.
    pub(super) fn push_marker_box(
        &mut self,
        marker: LayoutBox,
        text: Option<&str>,
        inside: bool,
    ) -> BoxId {
        let kind_is_text = matches!(marker.kind, BoxKind::Marker(MarkerKind::Text));
        let id = self.tree.push(marker);
        if inside {
            if kind_is_text {
                self.push_inline_item(InlineItem::OpenInline {
                    box_id: id,
                    resumes: false,
                });
                if let Some(text) = text
                    && let Some(frame) = self.frames.last_mut()
                {
                    frame.push_item(InlineItem::Text {
                        source: TextSource::Owned(Arc::from(text)),
                        style_box: id,
                    });
                }
                self.push_inline_item(InlineItem::CloseInline {
                    box_id: id,
                    interrupted: false,
                });
            } else {
                self.push_inline_item(InlineItem::Atomic { box_id: id });
            }
        } else if let Some(text) = text {
            self.tree.get_mut(id).ifc = Some(Box::new(InlineFormattingContext {
                items: vec![InlineItem::Text {
                    source: TextSource::Owned(Arc::from(text)),
                    style_box: id,
                }],
                root: id,
            }));
        }
        id
    }

    // ------------------------------------------------------------ closing

    /// Closes a container: settles its children into an inline formatting
    /// context, block children, or anonymous wrappers around inline runs.
    fn finish(&mut self, frame: Frame) -> BoxId {
        let container = frame.box_id;
        if let BoxKind::Table(
            kind @ (TableKind::Table
            | TableKind::RowGroup
            | TableKind::Row
            | TableKind::Column
            | TableKind::ColumnGroup),
        ) = self.tree.get(container).kind
        {
            return self.finish_table_part(frame, kind);
        }
        // `float` does not apply to the children of a flex or grid container:
        // they are items, so a float there is a block-level child even when
        // nothing else is.
        let items_float = self.is_flex_or_grid(container);
        let has_block = frame.entries.iter().any(|e| {
            matches!(e, Entry::Block(_))
                || (items_float && matches!(e, Entry::OutOfFlow(_, Level::Float)))
        });
        if !has_block {
            let items: Vec<InlineItem> = frame
                .entries
                .into_iter()
                .filter_map(|e| match e {
                    Entry::Inline(item) => Some(item),
                    Entry::OutOfFlow(box_id, Level::Float) => Some(InlineItem::Float { box_id }),
                    Entry::OutOfFlow(box_id, _) => Some(InlineItem::AbsPos { box_id }),
                    Entry::Block(_) => None,
                })
                .collect();
            if items.is_empty() {
                // Controls keep the value text they were given at creation.
                return container;
            }
            self.adopt_items(&items, container);
            self.tree.get_mut(container).ifc = Some(Box::new(InlineFormattingContext {
                items,
                root: container,
            }));
            return container;
        }

        // Floats and positioned boxes stay in the run they interrupt: they
        // are placeholders, and splitting a paragraph around one would break
        // its lines. Only block-level children end a run.
        let mut children: SmallVec<[BoxId; 4]> = SmallVec::new();
        let mut run: Vec<InlineItem> = Vec::new();
        for entry in frame.entries {
            match entry {
                Entry::Inline(item) => run.push(item),
                Entry::OutOfFlow(box_id, Level::Float) if items_float => {
                    self.flush_run(
                        &mut run,
                        &mut children,
                        &frame.style,
                        &frame.decoration,
                        container,
                    );
                    self.tree.get_mut(box_id).parent = container;
                    children.push(box_id);
                }
                Entry::OutOfFlow(box_id, Level::Float) => run.push(InlineItem::Float { box_id }),
                Entry::OutOfFlow(box_id, _) => run.push(InlineItem::AbsPos { box_id }),
                Entry::Block(id) => {
                    self.flush_run(
                        &mut run,
                        &mut children,
                        &frame.style,
                        &frame.decoration,
                        container,
                    );
                    children.push(id);
                }
            }
        }
        self.flush_run(
            &mut run,
            &mut children,
            &frame.style,
            &frame.decoration,
            container,
        );
        self.wrap_stray_parts(&mut children, container, &frame.style, &frame.decoration);
        self.tree.get_mut(container).children = children;
        container
    }

    /// Turns a run of inline items between block children into an anonymous
    /// block, or drops it when it holds nothing but collapsible white space.
    fn flush_run(
        &mut self,
        run: &mut Vec<InlineItem>,
        children: &mut SmallVec<[BoxId; 4]>,
        container_style: &Style,
        decoration: &DecorationPaint,
        container: BoxId,
    ) {
        if run.is_empty() {
            return;
        }
        if self.is_droppable(run) {
            // Out-of-flow boxes do not count towards the decision; the ones in
            // a dropped run become direct children so they are not lost.
            for item in run.drain(..) {
                if let InlineItem::Float { box_id } | InlineItem::AbsPos { box_id } = item {
                    self.tree.get_mut(box_id).parent = container;
                    children.push(box_id);
                }
            }
            return;
        }
        let style = self.anon.derive(container_style, true);
        let (paint, paint_flags) = paint_style(&style, decoration);
        let paint = Arc::new(paint);
        let mut wrapper = LayoutBox::new(
            NodeId::NONE,
            container,
            BoxKind::AnonymousBlock,
            style,
            paint,
        );
        wrapper.flags = BoxFlags::ANONYMOUS | paint_flags;
        if self.is_flex_or_grid(container) {
            wrapper.flags |= BoxFlags::ESTABLISHES_BFC;
        }
        let wrapper_id = self.tree.push(wrapper);
        let mut items = std::mem::take(run);
        for item in &mut items {
            if let InlineItem::Text { style_box, .. } = item
                && *style_box == container
            {
                *style_box = wrapper_id;
            }
        }
        self.adopt_items(&items, wrapper_id);
        self.tree.get_mut(wrapper_id).ifc = Some(Box::new(InlineFormattingContext {
            items,
            root: wrapper_id,
        }));
        children.push(wrapper_id);
    }

    fn is_droppable(&self, run: &[InlineItem]) -> bool {
        run.iter().all(|item| match item {
            // An empty inline box with margin, border or padding still takes
            // part in layout and paint.
            InlineItem::OpenInline { box_id, .. } | InlineItem::CloseInline { box_id, .. } => {
                !self.has_inline_edges(*box_id)
            }
            InlineItem::Float { .. } | InlineItem::AbsPos { .. } => true,
            InlineItem::Text { source, style_box } => {
                let collapses = matches!(
                    self.tree
                        .get(*style_box)
                        .style
                        .get_inherited_text()
                        .white_space_collapse,
                    WhiteSpaceCollapse::Collapse
                );
                let blank = match source {
                    TextSource::Node { node, range } => self
                        .cx
                        .doc
                        .text_data(*node)
                        .and_then(|t| t.get(range.start as usize..range.end as usize))
                        .is_some_and(is_whitespace_only),
                    TextSource::Owned(text) => is_whitespace_only(text),
                };
                collapses && blank
            }
            _ => false,
        })
    }

    fn has_inline_edges(&self, id: BoxId) -> bool {
        let style = &self.tree.get(id).style;
        let nonzero = |lp: &style::values::computed::LengthPercentage| {
            lp.to_length().is_none_or(|l| l.px() != 0.0)
        };
        let padding = style.get_padding();
        let margin = style.get_margin();
        let margin_nonzero = |m: &style::values::computed::Margin| match m {
            style::values::computed::Margin::LengthPercentage(lp) => nonzero(lp),
            _ => false,
        };
        let border = &self.tree.get(id).paint.border.widths;
        border.horizontal() + border.vertical() > 0.0
            || [
                &padding.padding_top.0,
                &padding.padding_right.0,
                &padding.padding_bottom.0,
                &padding.padding_left.0,
            ]
            .into_iter()
            .any(nonzero)
            || margin_nonzero(&margin.margin_left)
            || margin_nonzero(&margin.margin_right)
    }

    pub(super) fn is_flex_or_grid(&self, id: BoxId) -> bool {
        matches!(self.tree.get(id).kind, BoxKind::Flex | BoxKind::Grid)
    }

    /// Gives boxes that were created without a known owner their formatting
    /// context's block container as parent.
    fn adopt_items(&mut self, items: &[InlineItem], owner: BoxId) {
        for item in items {
            let id = match item {
                InlineItem::OpenInline { box_id, .. }
                | InlineItem::Atomic { box_id }
                | InlineItem::ForcedBreak { box_id }
                | InlineItem::Float { box_id }
                | InlineItem::AbsPos { box_id } => *box_id,
                InlineItem::CloseInline { .. }
                | InlineItem::SoftBreak
                | InlineItem::Text { .. } => continue,
            };
            let b = self.tree.get_mut(id);
            if b.parent == BoxId::NONE {
                b.parent = owner;
            }
        }
    }
}
