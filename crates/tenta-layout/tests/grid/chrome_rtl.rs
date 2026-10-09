// Expected rectangles recorded from Chrome 154 (getBoundingClientRect, document coordinates).

use super::Case;

pub(super) const CASES: &[Case] = &[
    Case {
        name: "rtl_columns",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:50px 100px;direction:rtl;width:300px"><div id=a style="height:10px"></div><div id=b style="height:10px"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 300.0, 10.0]),
            ("a", [250.0, 0.0, 50.0, 10.0]),
            ("b", [150.0, 0.0, 100.0, 10.0]),
        ],
    },
    Case {
        name: "rtl_explicit_placement",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:50px 50px 50px;direction:rtl;width:300px"><div id=a style="grid-column:1;height:10px"></div><div id=b style="grid-column:3;height:10px"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 300.0, 10.0]),
            ("a", [250.0, 0.0, 50.0, 10.0]),
            ("b", [150.0, 0.0, 50.0, 10.0]),
        ],
    },
    Case {
        name: "rtl_justify_items_start",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:100px 100px;direction:rtl;justify-items:start;width:300px"><div id=a style="width:30px;height:10px"></div><div id=b style="width:30px;height:10px"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 300.0, 10.0]),
            ("a", [270.0, 0.0, 30.0, 10.0]),
            ("b", [170.0, 0.0, 30.0, 10.0]),
        ],
    },
    Case {
        name: "rtl_justify_content_end",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:50px 50px;direction:rtl;justify-content:end;width:300px"><div id=a style="height:10px"></div><div id=b style="height:10px"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 300.0, 10.0]),
            ("a", [50.0, 0.0, 50.0, 10.0]),
            ("b", [0.0, 0.0, 50.0, 10.0]),
        ],
    },
    Case {
        name: "rtl_justify_content_left",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:50px 50px;direction:rtl;justify-content:left;width:300px"><div id=a style="height:10px"></div><div id=b style="height:10px"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 300.0, 10.0]),
            ("a", [50.0, 0.0, 50.0, 10.0]),
            ("b", [0.0, 0.0, 50.0, 10.0]),
        ],
    },
    Case {
        name: "rtl_justify_self_left",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:100px;direction:rtl;width:300px"><div id=a style="width:30px;height:10px;justify-self:left"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 300.0, 10.0]),
            ("a", [200.0, 0.0, 30.0, 10.0]),
        ],
    },
    Case {
        name: "rtl_gap_and_span",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:40px 40px 40px;gap:10px;direction:rtl;width:300px"><div id=a style="grid-column:1/3;height:10px"></div><div id=b style="height:10px"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 300.0, 10.0]),
            ("a", [210.0, 0.0, 90.0, 10.0]),
            ("b", [160.0, 0.0, 40.0, 10.0]),
        ],
    },
    Case {
        name: "rtl_inherited_by_items",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:repeat(2,80px);direction:rtl;width:300px"><div id=a style="height:10px"></div><div id=b style="height:10px;direction:ltr"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 300.0, 10.0]),
            ("a", [220.0, 0.0, 80.0, 10.0]),
            ("b", [140.0, 0.0, 80.0, 10.0]),
        ],
    },
    Case {
        name: "rtl_auto_flow_column",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-rows:20px;grid-auto-flow:column;grid-auto-columns:50px;direction:rtl;width:300px"><div id=a></div><div id=b></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 300.0, 20.0]),
            ("a", [250.0, 0.0, 50.0, 20.0]),
            ("b", [200.0, 0.0, 50.0, 20.0]),
        ],
    },
    Case {
        name: "rtl_negative_lines",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:50px 50px 50px;direction:rtl;width:300px"><div id=a style="grid-column:-2/-1;height:10px"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 300.0, 10.0]),
            ("a", [150.0, 0.0, 50.0, 10.0]),
        ],
    },
    Case {
        name: "rtl_shrink_inline_grid",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=w style="width:400px;direction:rtl"><div id=c style="display:inline-grid;grid-template-columns:50px 50px"><div id=a style="height:10px"></div><div id=b style="height:10px"></div></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("w", [0.0, 0.0, 400.0, 18.0]),
            ("c", [300.0, 4.0, 100.0, 10.0]),
            ("a", [350.0, 4.0, 50.0, 10.0]),
            ("b", [300.0, 4.0, 50.0, 10.0]),
        ],
    },
];
