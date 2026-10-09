// Expected rectangles recorded from Chrome 153 (getBoundingClientRect, document coordinates).

use super::Case;

pub(super) const CASES: &[Case] = &[
    Case {
        name: "empty_item_reserves_a_line",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} ul{margin:0;padding-left:40px;} body{font:16px LS}</style></head><body id=body><ul><li id=a></li><li id=b>x</li></ul>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("a", [40.0, 0.0, 760.0, 18.0]),
            ("b", [40.0, 18.0, 760.0, 18.0]),
        ],
    },
    Case {
        name: "empty_item_with_height_zero",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} ul{margin:0;padding-left:40px;} body{font:16px LS}</style></head><body id=body><ul><li id=a style="height:0"></li><li id=b>x</li></ul>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("a", [40.0, 0.0, 760.0, 0.0]),
            ("b", [40.0, 0.0, 760.0, 18.0]),
        ],
    },
    Case {
        name: "item_with_empty_block_child",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} ul{margin:0;padding-left:40px;} body{font:16px LS}</style></head><body id=body><ul><li id=a><div id=c></div></li><li id=b>x</li></ul>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("a", [40.0, 0.0, 760.0, 18.0]),
            ("c", [40.0, 0.0, 760.0, 0.0]),
            ("b", [40.0, 18.0, 760.0, 18.0]),
        ],
    },
    Case {
        name: "item_with_tall_block_child",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} ul{margin:0;padding-left:40px;} body{font:16px LS}</style></head><body id=body><ul><li id=a><div id=c style="height:30px"></div></li><li id=b>x</li></ul>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("a", [40.0, 0.0, 760.0, 30.0]),
            ("c", [40.0, 0.0, 760.0, 30.0]),
            ("b", [40.0, 30.0, 760.0, 18.0]),
        ],
    },
    Case {
        name: "empty_item_strut_uses_line_height",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} ul{margin:0;padding-left:40px;} body{font:16px LS}</style></head><body id=body><ul><li id=a style="padding:3px 0;line-height:30px"></li><li id=b>x</li></ul>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("a", [40.0, 0.0, 760.0, 36.0]),
            ("b", [40.0, 36.0, 760.0, 18.0]),
        ],
    },
    Case {
        name: "empty_item_without_marker",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} ul{margin:0;padding-left:40px;} body{font:16px LS}</style></head><body id=body><ul><li id=a style="list-style:none"></li><li id=b>x</li></ul>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("a", [40.0, 0.0, 760.0, 0.0]),
            ("b", [40.0, 0.0, 760.0, 18.0]),
        ],
    },
    Case {
        name: "empty_item_min_height",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} ul{margin:0;padding-left:40px;} body{font:16px LS}</style></head><body id=body><ul><li id=a style="min-height:5px"></li><li id=b>x</li></ul>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("a", [40.0, 0.0, 760.0, 18.0]),
            ("b", [40.0, 18.0, 760.0, 18.0]),
        ],
    },
    Case {
        name: "empty_item_margins",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} ul{margin:0;padding-left:40px;} body{font:16px LS}</style></head><body id=body><ul><li id=a style="margin:10px 0"></li><li id=b>x</li></ul>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("a", [40.0, 10.0, 760.0, 18.0]),
            ("b", [40.0, 38.0, 760.0, 18.0]),
        ],
    },
    Case {
        name: "ordered_list_items",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><style>html,body{margin:0;} ul{margin:0;padding-left:40px;} body{font:16px LS}</style></head><body id=body><ol><li id=a>x</li><li id=b>y</li></ol>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("a", [40.0, 16.0, 760.0, 18.0]),
            ("b", [40.0, 34.0, 760.0, 18.0]),
        ],
    },
];
