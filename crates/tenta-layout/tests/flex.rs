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

    // The three tests below pin what an answer about a nested container
    // depends on besides the question Taffy asks. A nested container is solved
    // inside its parent's view and again as the root of its own, and the
    // answers are shared between the views, so an answer taken from the wrong
    // one shows up as a wrong size here.

    #[test]
    fn a_percentage_height_resolves_against_the_height_its_container_ends_up_with() {
        // `k` is first sized with its height unknown, where `l`'s percentage
        // is `auto`, and then laid out stretched to 200px, where it is 100px.
        let laid = lay_out(
            r#"<!doctype html><body style="margin:0">
            <div style="display:flex;width:300px;height:200px">
              <div id=k style="display:flex;flex-direction:column;width:100px">
                <div id=l style="display:flex;height:50%"><div id=leaf style="width:10px"></div></div>
              </div>
            </div>"#,
            (800.0, 600.0),
        );
        assert_eq!(laid.rect("k").h, 200.0);
        assert_eq!(laid.rect("l").h, 100.0);
        assert_eq!(laid.rect("leaf").h, 100.0);
    }

    #[test]
    fn a_ratio_only_image_fills_the_container_it_is_in_not_an_outer_one() {
        // The grid column is sized by its content before any width is known,
        // so the image falls back to its container's width: 100px once `k` is
        // laid out, although `k` was measured inside a 400px row first.
        let laid = lay_out(
            r#"<!doctype html><body style="margin:0">
            <div style="display:flex;width:400px;align-items:flex-start">
              <div id=k style="display:grid;grid-template-columns:auto;width:100px">
                <img id=i style="aspect-ratio:2/1">
              </div>
            </div>"#,
            (800.0, 600.0),
        );
        let image = laid.rect("i");
        assert_eq!((image.w, image.h), (100.0, 50.0));
    }

    #[test]
    fn nested_containers_keep_their_layout_at_every_depth() {
        // Rows, grids and columns inside one another with block wrappers in
        // between: every level re-enters the levels below it, so each size
        // here is the product of answers shared across several views.
        let laid = lay_out(
            r#"<!doctype html><body style="margin:0">
            <div id=row style="display:flex;width:600px;gap:20px">
              <div id=col style="flex:1"><div id=grid style="display:grid;grid-template-columns:1fr 1fr;gap:10px">
                <div id=card style="display:flex;flex-direction:column;padding:5px">
                  <div id=head style="display:flex;height:20px"><div id=icon style="width:16px"></div></div>
                  <div><div id=line style="display:flex;gap:4px">
                    <div id=pair style="display:grid;grid-template-columns:1fr 30px;flex:1">
                      <div id=cell style="height:12px"></div><div id=tail style="height:8px"></div>
                    </div>
                  </div></div>
                </div>
                <div id=other style="height:40px"></div>
              </div></div>
              <div id=side style="width:80px"></div>
            </div>"#,
            (800.0, 600.0),
        );
        let rect = |id: &str| {
            let r = laid.rect(id);
            [r.x, r.y, r.w, r.h]
        };
        assert_eq!(rect("col"), [0.0, 0.0, 500.0, 42.0]);
        assert_eq!(rect("grid"), [0.0, 0.0, 500.0, 42.0]);
        assert_eq!(rect("card"), [0.0, 0.0, 245.0, 42.0]);
        assert_eq!(rect("head"), [5.0, 5.0, 235.0, 20.0]);
        assert_eq!(rect("icon"), [5.0, 5.0, 16.0, 20.0]);
        assert_eq!(rect("line"), [5.0, 25.0, 235.0, 12.0]);
        assert_eq!(rect("pair"), [5.0, 25.0, 235.0, 12.0]);
        assert_eq!(rect("cell"), [5.0, 25.0, 205.0, 12.0]);
        assert_eq!(rect("tail"), [210.0, 25.0, 30.0, 8.0]);
        assert_eq!(rect("other"), [255.0, 0.0, 245.0, 40.0]);
        assert_eq!(rect("side"), [520.0, 0.0, 80.0, 42.0]);
    }
}
