// Expected rectangles recorded from Chrome 154 (getBoundingClientRect, document coordinates).

use super::Case;

pub(super) const CASES: &[Case] = &[
    Case {
        name: "align_items_baseline",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:60px 60px;align-items:baseline"><div id=a style="font-size:30px;line-height:1">Ag</div><div id=b style="font-size:12px;line-height:1">Ag</div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 800.0, 30.0]),
            ("a", [0.0, 0.0, 60.0, 30.0]),
            ("b", [60.0, 15.0, 60.0, 12.0]),
        ],
    },
    Case {
        name: "align_self_baseline_mixed",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:60px 60px 60px"><div id=a style="align-self:baseline;font-size:24px;line-height:1">Ag</div><div id=b style="align-self:baseline;font-size:12px;line-height:1">Ag</div><div id=d style="height:20px"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 800.0, 24.0]),
            ("a", [0.0, 0.0, 60.0, 24.0]),
            ("b", [60.0, 10.0, 60.0, 12.0]),
            ("d", [120.0, 0.0, 60.0, 20.0]),
        ],
    },
    Case {
        name: "inline_grid_baseline_in_line",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body>x <span id=s><div id=c style="display:inline-grid;grid-template-columns:40px;font-size:20px;line-height:1"><div id=a>Ag</div></div></span> y</body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("s", [12.4531, 3.0, 40.0, 17.0]),
            ("c", [12.4531, 0.0, 40.0, 20.0]),
            ("a", [12.4531, 0.0, 40.0, 20.0]),
        ],
    },
    Case {
        name: "order_reorders_baseline_item",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body>x <span id=s><div id=c style="display:inline-grid;grid-template-columns:40px 40px;line-height:1;"><div id=a style="order:2;font-size:30px">Ag</div><div id=b style="order:1;font-size:12px">Ag</div></div></span> y</body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("s", [12.4531, 0.0, 80.0, 17.0]),
            ("c", [12.4531, 4.0, 80.0, 30.0]),
            ("a", [52.4531, 4.0, 40.0, 30.0]),
            ("b", [12.4531, 4.0, 40.0, 30.0]),
        ],
    },
    Case {
        name: "row_two_item_first_in_dom",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body>x <span id=s><div id=c style="display:inline-grid;grid-template-columns:40px;line-height:1;"><div id=a style="grid-row:2;font-size:30px">Ag</div><div id=b style="grid-row:1;font-size:12px">Ag</div></div></span> y</body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("s", [12.4531, 0.0, 40.0, 17.0]),
            ("c", [12.4531, 4.0, 40.0, 42.0]),
            ("a", [12.4531, 16.0, 40.0, 30.0]),
            ("b", [12.4531, 4.0, 40.0, 12.0]),
        ],
    },
    Case {
        name: "baseline_aligned_preferred_over_first",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body>x <span id=s><div id=c style="display:inline-grid;grid-template-columns:40px 40px;align-items:start;line-height:1;"><div id=a style="font-size:12px">Ag</div><div id=b style="align-self:baseline;font-size:30px">Ag</div></div></span> y</body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("s", [12.4531, 11.0, 80.0, 17.0]),
            ("c", [12.4531, 0.0, 80.0, 30.0]),
            ("a", [12.4531, 0.0, 40.0, 12.0]),
            ("b", [52.4531, 0.0, 40.0, 30.0]),
        ],
    },
    Case {
        name: "end_aligned_first_row_item",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body>x <span id=s><div id=c style="display:inline-grid;grid-template-columns:40px;grid-template-rows:60px 20px;line-height:1;"><div id=a style="align-self:end;font-size:12px">Ag</div><div id=b style="font-size:20px">Ag</div></div></span> y</body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("s", [12.4531, 44.0, 40.0, 17.0]),
            ("c", [12.4531, 0.0, 40.0, 80.0]),
            ("a", [12.4531, 48.0, 40.0, 12.0]),
            ("b", [12.4531, 60.0, 40.0, 20.0]),
        ],
    },
];
