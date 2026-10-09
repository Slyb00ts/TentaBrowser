//! Flexbox layout against Chrome: main-axis distribution, cross-axis
//! alignment, wrapping, gaps, ordering, intrinsic sizing and nesting.
//!
//! The expectations in `flex/chrome_cases.rs` are `getBoundingClientRect`
//! values recorded from Chrome 153 for exactly the HTML next to them.

// `FontSystem::new_embedded` exists only with the bundled fonts.
#![cfg(feature = "embedded-fonts")]

#[path = "flex/chrome_cases.rs"]
mod chrome_cases;
#[path = "flex/chrome_more_cases.rs"]
mod chrome_more_cases;
#[path = "flex/chrome_page_cases.rs"]
mod chrome_page_cases;
#[path = "flex/chrome_ratio_cases.rs"]
mod chrome_ratio_cases;
#[path = "flex/chrome_value_cases.rs"]
mod chrome_value_cases;
mod support;

use support::{Case, check_cases};

#[test]
fn rectangles_match_chrome() {
    check_cases(chrome_cases::CASES);
}

#[test]
fn nested_and_content_sized_rectangles_match_chrome() {
    check_cases(chrome_more_cases::CASES);
}

#[test]
fn page_patterns_match_chrome() {
    check_cases(chrome_page_cases::CASES);
}

#[test]
fn floats_and_aspect_ratios_match_chrome() {
    check_cases(chrome_ratio_cases::CASES);
}

#[test]
fn calc_fractions_and_edge_cases_match_chrome() {
    check_cases(chrome_value_cases::CASES);
}

mod behaviour {
    use tenta_layout::FragFlags;

    use super::support::lay_out;

    #[test]
    fn items_paint_atomically_in_order_modified_document_order() {
        let laid = lay_out(
            r#"<!doctype html><body style="margin:0"><div id=c style="display:flex">
            <div id=a style="order:2;width:10px"></div><div id=b style="order:-1;width:10px"></div>
            <div id=d style="width:10px"></div></div>"#,
            (800.0, 600.0),
        );
        let container = laid.frags.get(laid.frag("c"));
        let order: Vec<_> = container
            .children
            .iter()
            .map(|&kid| laid.frags.get(kid).node)
            .collect();
        assert_eq!(order, [laid.node("b"), laid.node("d"), laid.node("a")]);
        for id in ["a", "b", "d"] {
            assert!(
                laid.frags
                    .get(laid.frag(id))
                    .flags
                    .contains(FragFlags::PAINT_ATOMIC)
            );
        }
        let flags = container.flags;
        assert!(flags.contains(FragFlags::ESTABLISHES_BFC));
        assert!(!flags.contains(FragFlags::PAINT_ATOMIC));
    }

    #[test]
    fn positioned_children_neither_size_nor_move_the_items() {
        let laid = lay_out(
            r#"<!doctype html><body style="margin:0"><div id=c style="display:flex;width:300px;position:relative">
            <div id=a style="width:100px;height:20px"></div>
            <div id=x style="position:absolute;width:500px;height:500px"></div>
            <div id=b style="width:100px;height:30px"></div></div>"#,
            (800.0, 600.0),
        );
        assert_eq!(laid.rect("c").h, 30.0);
        assert_eq!(laid.rect("b").x, 100.0);
    }

    #[test]
    fn flex_layout_is_repeatable_on_one_thread() {
        let html = r#"<!doctype html><body style="margin:0;font:16px 'Liberation Sans'">
            <div id=c style="display:flex;flex-wrap:wrap;width:200px">
            <div id=a style="flex:1 1 90px">one two three four</div>
            <div id=b style="flex:1 1 90px"><div style="display:flex"><div id=n>nested text</div></div></div>
            <div id=d style="flex:2 1 50px">tail</div></div>"#;
        let first = lay_out(html, (800.0, 600.0));
        let second = lay_out(html, (800.0, 600.0));
        for id in ["c", "a", "b", "n", "d"] {
            assert_eq!(first.rect(id), second.rect(id), "#{id}");
        }
    }

    #[test]
    fn deeply_nested_flex_containers_do_not_repeat_work_per_level() {
        // Each level lays its content out to measure it and again to place it;
        // without memoised measurements that is exponential in the depth.
        let depth = 60;
        let mut html =
            String::from(r#"<!doctype html><body style="margin:0;font:16px 'Liberation Sans'">"#);
        for level in 0..depth {
            html.push_str(if level % 2 == 0 {
                "<div style='display:flex'><div style='flex:1'>"
            } else {
                "<div style='padding:1px'>"
            });
        }
        html.push_str("<span id=leaf>leaf text</span>");
        for level in (0..depth).rev() {
            html.push_str(if level % 2 == 0 {
                "</div></div>"
            } else {
                "</div>"
            });
        }
        let laid = lay_out(&html, (800.0, 600.0));
        let leaf = laid.rect("leaf");
        assert_eq!((leaf.x, leaf.y), (depth as f32 / 2.0, depth as f32 / 2.0));
    }
}
