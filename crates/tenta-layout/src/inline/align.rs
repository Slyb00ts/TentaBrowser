//! `text-align`, `text-indent` and justification.

use super::style::{Align, BlockInfo};
use crate::types::Direction;

/// The alignment of a line: the last line of a block and a line that ends in a
/// forced break use `text-align-last`.
pub(super) fn line_align(block: &BlockInfo, last_or_forced: bool) -> Align {
    if !last_or_forced {
        return block.align;
    }
    match (block.align_last, block.align) {
        (Some(last), _) => last,
        (None, Align::Justify) => Align::Start,
        (None, other) => other,
    }
}

/// Whether the line gets the `text-indent` space, given whether it is the
/// first line and whether a forced break precedes it.
pub(super) fn is_indented(block: &BlockInfo, first: bool, after_forced: bool) -> bool {
    let from_start = first || (block.indent_each_line && after_forced);
    from_start != block.indent_hanging
}

/// Rounds towards zero to Blink's `LayoutUnit` precision of 1/64 pixel.
fn quantize(value: f32) -> f32 {
    (value * 64.0).trunc() / 64.0
}

/// The distance from the left edge of a line of width `avail` to the start of
/// `content`, which is `content` wide, with `indent` reserved at the inline
/// start. Content that does not fit starts at the inline start edge whatever
/// the alignment, as in browsers.
pub(super) fn offset(
    align: Align,
    direction: Direction,
    avail: f32,
    indent: f32,
    content: f32,
) -> f32 {
    let rtl = direction == Direction::Rtl;
    let (left_pad, right_pad) = if rtl { (0.0, indent) } else { (indent, 0.0) };
    let free = avail - indent - content;
    let at_start = if rtl {
        avail - right_pad - content
    } else {
        left_pad
    };
    let physical = match align {
        Align::Start => {
            if rtl {
                Align::Right
            } else {
                Align::Left
            }
        }
        Align::End => {
            if rtl {
                Align::Left
            } else {
                Align::Right
            }
        }
        other => other,
    };
    if free < 0.0 && physical != Align::Left {
        return at_start;
    }
    match physical {
        Align::Right => quantize(avail - right_pad - content),
        Align::Center => quantize(left_pad + free / 2.0),
        _ => left_pad,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: f32 = 200.0;

    #[test]
    fn physical_and_logical_alignment() {
        use Align::*;
        use Direction::*;
        assert_eq!(offset(Left, Ltr, W, 0.0, 50.0), 0.0);
        assert_eq!(offset(Right, Ltr, W, 0.0, 50.0), 150.0);
        assert_eq!(offset(Center, Ltr, W, 0.0, 50.0), 75.0);
        assert_eq!(offset(Start, Ltr, W, 0.0, 50.0), 0.0);
        assert_eq!(offset(End, Ltr, W, 0.0, 50.0), 150.0);
        assert_eq!(offset(Start, Rtl, W, 0.0, 50.0), 150.0);
        assert_eq!(offset(End, Rtl, W, 0.0, 50.0), 0.0);
        assert_eq!(offset(Left, Rtl, W, 0.0, 50.0), 0.0);
    }

    #[test]
    fn indent_sits_at_the_inline_start() {
        use Align::*;
        use Direction::*;
        assert_eq!(offset(Start, Ltr, W, 20.0, 50.0), 20.0);
        assert_eq!(offset(Start, Rtl, W, 20.0, 50.0), 130.0);
        assert_eq!(offset(Center, Ltr, W, 20.0, 50.0), 85.0);
        assert_eq!(offset(Right, Ltr, W, 20.0, 50.0), 150.0);
    }

    #[test]
    fn overflowing_content_starts_at_the_start_edge() {
        use Align::*;
        use Direction::*;
        assert_eq!(offset(Center, Ltr, W, 0.0, 250.0), 0.0);
        assert_eq!(offset(Right, Ltr, W, 0.0, 250.0), 0.0);
        assert_eq!(offset(Center, Rtl, W, 0.0, 250.0), -50.0);
    }

    #[test]
    fn the_last_line_follows_text_align_last() {
        let mut block = BlockInfo {
            align: Align::Justify,
            align_last: None,
            indent: 0.0,
            indent_hanging: false,
            indent_each_line: false,
            direction: Direction::Ltr,
            ellipsis: false,
        };
        assert_eq!(line_align(&block, false), Align::Justify);
        assert_eq!(line_align(&block, true), Align::Start);
        block.align_last = Some(Align::Center);
        assert_eq!(line_align(&block, true), Align::Center);
    }

    #[test]
    fn indent_keywords_pick_the_lines() {
        let mut block = BlockInfo {
            align: Align::Start,
            align_last: None,
            indent: 10.0,
            indent_hanging: false,
            indent_each_line: false,
            direction: Direction::Ltr,
            ellipsis: false,
        };
        assert!(is_indented(&block, true, false));
        assert!(!is_indented(&block, false, true));
        block.indent_each_line = true;
        assert!(is_indented(&block, false, true));
        assert!(!is_indented(&block, false, false));
        block.indent_hanging = true;
        assert!(!is_indented(&block, true, false));
        assert!(is_indented(&block, false, false));
    }
}
