//! `vertical-align` and its effect on line-box height.
//!
//! Line building places every fragment of a line with its baseline on the
//! line's baseline. This module then shifts the boxes whose `vertical-align`
//! says otherwise (CSS 2.1 section 10.8.1, with Blink's rounding), grows the
//! line box to contain them, and aligns `top` and `bottom` boxes to the edges
//! of the finished line box.

use std::collections::HashMap;

use smallvec::SmallVec;
use style::properties::ComputedValues;
use style::values::computed::{AlignmentBaseline, Length};
use style::values::generics::box_::{BaselineShift, BaselineShiftKeyword};

use super::style::{BoxInfo, Infos, LineHeight, layout_unit_floor, leading};
use crate::box_tree::{BoxId, BoxTree};
use crate::font::{ResolvedFont, VerticalMetrics};
use crate::fragment::{FragId, FragmentBuilder};
use crate::types::Rect;

/// What the aligner must know about a fragment that is an inline box or an
/// atomic inline; text fragments take part through their box.
pub(super) enum Member {
    /// A piece of an inline box.
    Inline {
        box_id: BoxId,
        /// The box contributes its line-height box to the line (quirks mode skips empty
        /// boxes).
        counts: bool,
        /// Metrics of fallback fonts whose glyphs sit under the box.
        fallback: SmallVec<[VerticalMetrics; 1]>,
    },
    /// An atomic inline, with how far its margin box reaches above and below
    /// its baseline.
    Atomic {
        box_id: BoxId,
        above: f32,
        below: f32,
    },
}

/// A line whose fragments sit on one baseline.
pub(super) struct LineBox<'a> {
    pub(super) tree: &'a BoxTree,
    pub(super) infos: &'a Infos,
    /// The block container whose strut the line carries.
    pub(super) root: BoxId,
    /// Ascent and descent of the strut and of the fallback fonts under it, or
    /// `None` when the line has no strut.
    pub(super) strut: Option<(f32, f32)>,
    /// Where the fragments currently put the baseline, from the line top.
    pub(super) baseline: f32,
    pub(super) members: Vec<(FragId, Member)>,
}

/// The aligned line box.
pub(super) struct Aligned {
    pub(super) ascent: f32,
    pub(super) descent: f32,
    /// Bounds of every fragment on the line, in the line's coordinates.
    pub(super) extent: Rect,
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum Align {
    Baseline,
    Sub,
    Super,
    TextTop,
    TextBottom,
    Middle,
    Top,
    Bottom,
    /// Raised by this much (negative lowers).
    Raise(f32),
}

fn align_of(style: &ComputedValues, line_height: f32) -> Align {
    let b = style.get_box();
    match &b.baseline_shift {
        BaselineShift::Keyword(BaselineShiftKeyword::Sub) => Align::Sub,
        BaselineShift::Keyword(BaselineShiftKeyword::Super) => Align::Super,
        BaselineShift::Keyword(BaselineShiftKeyword::Top) => Align::Top,
        BaselineShift::Keyword(BaselineShiftKeyword::Bottom) => Align::Bottom,
        BaselineShift::Keyword(BaselineShiftKeyword::Center) => Align::Middle,
        BaselineShift::Length(lp) => {
            let raised = lp.resolve(Length::new(line_height)).px();
            if raised != 0.0 {
                Align::Raise(raised)
            } else {
                match b.alignment_baseline {
                    AlignmentBaseline::Middle => Align::Middle,
                    AlignmentBaseline::TextTop => Align::TextTop,
                    AlignmentBaseline::TextBottom => Align::TextBottom,
                    _ => Align::Baseline,
                }
            }
        }
    }
}

/// Where a box sits relative to its parent's baseline.
#[derive(Copy, Clone, Debug, PartialEq)]
enum Place {
    /// Shifted down by this much (negative raises).
    Baseline(f32),
    Top,
    Bottom,
}

struct Node {
    frag: Option<FragId>,
    parent: usize,
    box_id: BoxId,
    align: Align,
    /// Distances from the node's baseline to the top and bottom of what it
    /// contributes; negative infinity when it contributes nothing.
    ascent: f32,
    descent: f32,
    place: Place,
    /// Baseline position from the line top once aligned.
    baseline: f32,
}

impl Node {
    fn contributes(&self) -> bool {
        self.ascent.is_finite()
    }

    fn unite(&mut self, ascent: f32, descent: f32) {
        self.ascent = self.ascent.max(ascent);
        self.descent = self.descent.max(descent);
    }
}

/// The primary font's x-height, which `middle` aligns to.
fn x_height(info: &BoxInfo) -> f32 {
    let text = &info.text;
    ResolvedFont::resolve(&text.family_list, text.weight, text.italic)
        .metrics_data(text.size)
        .x_height
        .unwrap_or(text.size / 2.0)
}

/// The downward shift of a box with the given extent from its parent's
/// baseline, or `None` for the boxes aligned to the line box itself.
fn shift_of(align: Align, ascent: f32, descent: f32, parent: &BoxInfo) -> Option<f32> {
    Some(match align {
        Align::Baseline => 0.0,
        Align::Sub => layout_unit_floor(parent.text.size / 5.0) + 1.0,
        Align::Super => -(layout_unit_floor(parent.text.size / 3.0) + 1.0),
        Align::TextTop => ascent - parent.metrics.ascent,
        Align::TextBottom => parent.metrics.descent - descent,
        Align::Middle => {
            layout_unit_floor((ascent - descent) / 2.0) - layout_unit_floor(x_height(parent) / 2.0)
        }
        Align::Raise(by) => -by,
        Align::Top | Align::Bottom => return None,
    })
}

/// The distances from the baseline to the top and bottom of the line-height
/// box of `info`, grown to hold the line-height boxes of the fallback fonts
/// that `line-height: normal` accounts for.
pub(super) fn strut(info: &BoxInfo, fallback: &[VerticalMetrics]) -> (f32, f32) {
    let (mut ascent, mut descent) = leading(info.line_height, info.metrics);
    for metrics in fallback {
        let (a, d) = leading(LineHeight::Normal, *metrics);
        ascent = ascent.max(a);
        descent = descent.max(d);
    }
    (ascent, descent)
}

/// Aligns the fragments `children` of `line` and returns the line box.
///
/// Every fragment of `children` (and the boxes inside inline boxes) is expected
/// at the baseline `line.baseline`; the line box that results can be taller
/// than the one it was placed in, and its baseline lower.
pub(super) fn align_line(
    line: &LineBox,
    children: &[FragId],
    out: &mut FragmentBuilder,
) -> Aligned {
    let by_frag: HashMap<FragId, &Member> = line.members.iter().map(|(f, m)| (*f, m)).collect();
    let (strut_ascent, strut_descent) =
        line.strut.unwrap_or((f32::NEG_INFINITY, f32::NEG_INFINITY));
    let mut nodes = vec![Node {
        frag: None,
        parent: 0,
        box_id: line.root,
        align: Align::Baseline,
        ascent: strut_ascent,
        descent: strut_descent,
        place: Place::Baseline(0.0),
        baseline: 0.0,
    }];

    // Boxes in tree order: a parent always precedes its boxes.
    let mut pending: Vec<(FragId, usize)> = children.iter().rev().map(|&f| (f, 0)).collect();
    while let Some((frag, parent)) = pending.pop() {
        let Some(member) = by_frag.get(&frag) else {
            continue;
        };
        let (box_id, ascent, descent) = match member {
            Member::Inline {
                box_id,
                counts,
                fallback,
            } => {
                let (ascent, descent) = if *counts {
                    strut(line.infos.get(*box_id), fallback)
                } else {
                    (f32::NEG_INFINITY, f32::NEG_INFINITY)
                };
                pending.extend(
                    out.get(frag)
                        .children
                        .iter()
                        .rev()
                        .map(|&c| (c, nodes.len())),
                );
                (*box_id, ascent, descent)
            }
            Member::Atomic {
                box_id,
                above,
                below,
            } => (*box_id, *above, *below),
        };
        let info = line.infos.get(box_id);
        let used = match info.line_height {
            LineHeight::Normal => info.metrics.line_spacing(),
            LineHeight::Fixed(v) => v,
        };
        nodes.push(Node {
            frag: Some(frag),
            parent,
            box_id,
            align: align_of(&line.tree.get(box_id).style, used),
            ascent,
            descent,
            place: Place::Baseline(0.0),
            baseline: 0.0,
        });
    }

    // Children before parents: a box's extent is final when it is merged into
    // its parent.
    for i in (1..nodes.len()).rev() {
        let Node {
            parent,
            align,
            ascent,
            descent,
            ..
        } = nodes[i];
        let parent_info = line.infos.get(nodes[parent].box_id);
        match shift_of(align, ascent, descent, parent_info) {
            Some(shift) => {
                nodes[i].place = Place::Baseline(shift);
                if nodes[i].contributes() {
                    nodes[parent].unite(ascent - shift, descent + shift);
                }
            }
            None => {
                nodes[i].place = if align == Align::Top {
                    Place::Top
                } else {
                    Place::Bottom
                };
            }
        }
    }

    // A box aligned to the top grows the line downwards, one aligned to the
    // bottom upwards.
    if !nodes[0].contributes() {
        nodes[0].ascent = 0.0;
        nodes[0].descent = 0.0;
    }
    let (mut ascent, mut descent) = (nodes[0].ascent, nodes[0].descent);
    for node in &nodes[1..] {
        if !node.contributes() {
            continue;
        }
        let height = node.ascent + node.descent;
        match node.place {
            Place::Top if ascent + descent < height => descent = height - ascent,
            Place::Bottom if ascent + descent < height => ascent = height - descent,
            _ => {}
        }
    }
    let total = ascent + descent;

    for node in &mut nodes {
        if !node.contributes() {
            node.ascent = 0.0;
            node.descent = 0.0;
        }
    }
    nodes[0].baseline = ascent;
    for i in 1..nodes.len() {
        nodes[i].baseline = match nodes[i].place {
            Place::Baseline(shift) => nodes[nodes[i].parent].baseline + shift,
            Place::Top => nodes[i].ascent,
            Place::Bottom => total - nodes[i].descent,
        };
    }
    let moved = |i: usize| nodes[i].baseline - line.baseline;
    for (i, node) in nodes.iter().enumerate().skip(1) {
        let Some(frag) = node.frag else { continue };
        let relative = if node.parent == 0 {
            moved(i)
        } else {
            moved(i) - moved(node.parent)
        };
        out.get_mut(frag).rect.y += relative;
    }
    // Text sitting directly on the line moves with the line's baseline.
    for &child in children {
        if !by_frag.contains_key(&child) {
            out.get_mut(child).rect.y += moved(0);
        }
    }

    Aligned {
        ascent,
        descent,
        extent: extent_of(children, &by_frag, out),
    }
}

/// The union of the border boxes of the fragments under `children`, not
/// looking inside atomic inlines, in the line's coordinates.
fn extent_of(
    children: &[FragId],
    members: &HashMap<FragId, &Member>,
    out: &FragmentBuilder,
) -> Rect {
    let mut extent = Rect::default();
    let mut pending: Vec<(FragId, (f32, f32))> =
        children.iter().map(|&f| (f, (0.0, 0.0))).collect();
    while let Some((frag, origin)) = pending.pop() {
        let f = out.get(frag);
        let rect = f.rect.translated(origin.0, origin.1);
        extent = extent.union(&rect);
        if !matches!(members.get(&frag), Some(Member::Atomic { .. })) {
            pending.extend(f.children.iter().map(|&c| (c, (rect.x, rect.y))));
        }
    }
    extent
}
