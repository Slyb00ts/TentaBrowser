// Expected rectangles recorded from Chrome 154 (getBoundingClientRect, document coordinates).

use super::Case;

pub(super) const CASES: &[Case] = &[
    Case {
        name: "static_first_child",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="padding:10px;margin:20px"><div id=a style="position:absolute;width:50px;height:30px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 760.0, 20.0]),
            ("a", [30.0, 30.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "static_after_block_sibling",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px"><div id=s style="height:40px"></div><div id=a style="position:absolute;width:50px;height:30px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 760.0, 40.0]),
            ("s", [20.0, 20.0, 760.0, 40.0]),
            ("a", [20.0, 60.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "static_before_block_sibling",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px"><div id=a style="position:absolute;width:50px;height:30px"></div><div id=s style="height:40px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 760.0, 40.0]),
            ("a", [20.0, 20.0, 50.0, 30.0]),
            ("s", [20.0, 20.0, 760.0, 40.0]),
        ],
    },
    Case {
        name: "static_between_siblings_with_margins",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px"><div id=s style="height:40px;margin-bottom:25px"></div><div id=a style="position:absolute;width:50px;height:30px"></div><div id=t style="height:10px;margin-top:15px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 760.0, 75.0]),
            ("s", [20.0, 20.0, 760.0, 40.0]),
            ("a", [20.0, 85.0, 50.0, 30.0]),
            ("t", [20.0, 85.0, 760.0, 10.0]),
        ],
    },
    Case {
        name: "static_own_margins_add",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px"><div id=s style="height:40px"></div><div id=a style="position:absolute;width:50px;height:30px;margin:7px 0 0 9px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 760.0, 40.0]),
            ("s", [20.0, 20.0, 760.0, 40.0]),
            ("a", [29.0, 67.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "static_with_parent_padding_border",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px;padding:6px;border:3px solid"><div id=s style="height:40px"></div><div id=a style="position:absolute;width:50px;height:30px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 760.0, 58.0]),
            ("s", [29.0, 29.0, 742.0, 40.0]),
            ("a", [29.0, 69.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "static_left_given_top_auto",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px"><div id=s style="height:40px"></div><div id=a style="position:absolute;left:12px;width:50px;height:30px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 760.0, 40.0]),
            ("s", [20.0, 20.0, 760.0, 40.0]),
            ("a", [12.0, 60.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "static_top_given_left_auto",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px"><div id=s style="height:40px;width:90px"></div><div id=a style="position:absolute;top:12px;width:50px;height:30px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 760.0, 40.0]),
            ("s", [20.0, 20.0, 90.0, 40.0]),
            ("a", [20.0, 12.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "static_right_given_left_auto_width_set",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px;width:300px"><div id=a style="position:absolute;right:10px;width:50px;height:30px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 0.0]),
            ("a", [740.0, 20.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "static_rtl_block",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px;width:300px;direction:rtl"><div id=s style="height:40px"></div><div id=a style="position:absolute;width:50px;height:30px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 40.0]),
            ("s", [20.0, 20.0, 300.0, 40.0]),
            ("a", [270.0, 60.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "static_rtl_block_shrink",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px;width:300px;direction:rtl"><div id=a style="position:absolute;height:30px"><div style="width:80px;height:5px"></div></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 300.0, 0.0]),
            ("a", [240.0, 20.0, 80.0, 30.0]),
        ],
    },
    Case {
        name: "static_nested_blocks",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div style="margin:10px;padding:10px"><div style="margin:20px;padding:5px"><div id=s style="height:30px"></div><div id=a style="position:absolute;width:50px;height:30px"></div></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("s", [45.0, 45.0, 710.0, 30.0]),
            ("a", [45.0, 75.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "static_collapsed_parent_margin",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:30px"><div id=m style="margin-top:20px;height:10px"></div><div id=a style="position:absolute;width:50px;height:30px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [30.0, 30.0, 740.0, 10.0]),
            ("m", [30.0, 30.0, 740.0, 10.0]),
            ("a", [30.0, 40.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "static_first_child_margin_collapse_through",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin-top:30px"><div id=a style="position:absolute;width:50px;height:30px"></div><div id=m style="margin-top:20px;height:10px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 30.0, 800.0, 10.0]),
            ("a", [0.0, 30.0, 50.0, 30.0]),
            ("m", [0.0, 30.0, 800.0, 10.0]),
        ],
    },
    Case {
        name: "static_inside_inline_block",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="display:inline-block;margin:20px;width:200px;padding:10px"><div id=s style="height:40px"></div><div id=a style="position:absolute;width:50px;height:30px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 220.0, 60.0]),
            ("s", [30.0, 30.0, 200.0, 40.0]),
            ("a", [30.0, 70.0, 50.0, 30.0]),
        ],
    },
    Case {
        name: "static_shrink_available_from_static_left",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="position:relative;margin:20px;width:200px"><div id=s style="height:10px"></div><div id=a style="position:absolute;height:20px">Hello world and some more words to wrap</div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 200.0, 10.0]),
            ("s", [20.0, 20.0, 200.0, 10.0]),
            ("a", [20.0, 30.0, 200.0, 20.0]),
        ],
    },
    Case {
        name: "static_does_not_take_space",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px"><div id=s style="height:40px"></div><div id=a style="position:absolute;width:50px;height:300px"></div><div id=t style="height:10px"></div></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 760.0, 50.0]),
            ("s", [20.0, 20.0, 760.0, 40.0]),
            ("a", [20.0, 60.0, 50.0, 300.0]),
            ("t", [20.0, 60.0, 760.0, 10.0]),
        ],
    },
    Case {
        name: "static_parent_height_auto_with_only_abs",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=p style="margin:20px"><div id=a style="position:absolute;width:50px;height:300px"></div></div><div id=t style="height:10px"></div>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [20.0, 20.0, 760.0, 0.0]),
            ("a", [20.0, 20.0, 50.0, 300.0]),
            ("t", [0.0, 20.0, 800.0, 10.0]),
        ],
    },
];
