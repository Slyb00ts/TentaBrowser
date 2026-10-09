// Expected rectangles recorded from Chrome 154 (getBoundingClientRect, document coordinates).

use super::Case;

pub(super) const CASES: &[Case] = &[
    Case {
        name: "line_numbers",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:repeat(4,50px);grid-template-rows:repeat(3,20px)"><div id=a style="grid-column:2/4;grid-row:1/3"></div><div id=b style="grid-column:1;grid-row:3"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 800.0, 60.0]),
            ("a", [50.0, 0.0, 100.0, 40.0]),
            ("b", [0.0, 40.0, 50.0, 20.0]),
        ],
    },
    Case {
        name: "negative_lines",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:repeat(4,50px);grid-template-rows:repeat(3,20px)"><div id=a style="grid-column:-3/-1;grid-row:-2/-1"></div><div id=b style="grid-column:1/-2;grid-row:1"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 800.0, 60.0]),
            ("a", [100.0, 40.0, 100.0, 20.0]),
            ("b", [0.0, 0.0, 150.0, 20.0]),
        ],
    },
    Case {
        name: "span_end_only",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:repeat(4,50px);grid-template-rows:20px"><div id=a style="grid-column:2/span 2"></div><div id=b style="grid-column:span 2/-1"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 800.0, 20.0]),
            ("a", [50.0, 0.0, 100.0, 20.0]),
            ("b", [100.0, 20.0, 100.0, 0.0]),
        ],
    },
    Case {
        name: "start_only_span",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:repeat(4,50px);grid-template-rows:20px"><div id=a style="grid-column-start:3;grid-column-end:span 2"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 800.0, 20.0]),
            ("a", [100.0, 0.0, 100.0, 20.0]),
        ],
    },
    Case {
        name: "reversed_lines_swap",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:repeat(4,50px);grid-template-rows:20px"><div id=a style="grid-column:4/2"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 800.0, 20.0]),
            ("a", [50.0, 0.0, 100.0, 20.0]),
        ],
    },
    Case {
        name: "same_start_end",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:repeat(4,50px);grid-template-rows:20px"><div id=a style="grid-column:2/2"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 800.0, 20.0]),
            ("a", [50.0, 0.0, 50.0, 20.0]),
        ],
    },
    Case {
        name: "named_lines",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:[a] 50px [b] 60px [c] 70px [d];grid-template-rows:20px"><div id=a style="grid-column:b/d"></div><div id=b style="grid-column:a/c;grid-row:2"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 800.0, 20.0]),
            ("a", [50.0, 0.0, 130.0, 20.0]),
            ("b", [0.0, 20.0, 110.0, 0.0]),
        ],
    },
    Case {
        name: "named_line_index",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:[x] 40px [x] 40px [x] 40px [x];grid-template-rows:20px"><div id=a style="grid-column:x 2/x 4"></div><div id=b style="grid-column:x -1/span 1;grid-row:2"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 800.0, 20.0]),
            ("a", [40.0, 0.0, 80.0, 20.0]),
            ("b", [120.0, 20.0, 680.0, 0.0]),
        ],
    },
    Case {
        name: "named_span",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:[s] 40px [e] 40px [s] 40px [e] 40px;grid-template-rows:20px"><div id=a style="grid-column:s/span 2"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 800.0, 20.0]),
            ("a", [0.0, 0.0, 80.0, 20.0]),
        ],
    },
    Case {
        name: "implicit_named_lines_from_areas",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-areas:'h h' 'm s';grid-template-columns:60px 40px;grid-template-rows:20px 30px"><div id=a style="grid-column:h-start/h-end;grid-row:h-start/h-end"></div><div id=b style="grid-area:m"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 800.0, 50.0]),
            ("a", [0.0, 0.0, 100.0, 20.0]),
            ("b", [0.0, 20.0, 60.0, 30.0]),
        ],
    },
    Case {
        name: "overlapping_items",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:repeat(3,50px);grid-template-rows:repeat(2,20px)"><div id=a style="grid-column:1/3;grid-row:1/3"></div><div id=b style="grid-column:2/4;grid-row:2"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 800.0, 40.0]),
            ("a", [0.0, 0.0, 100.0, 40.0]),
            ("b", [50.0, 20.0, 100.0, 20.0]),
        ],
    },
    Case {
        name: "placement_beyond_explicit_creates_tracks",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:50px;grid-template-rows:20px;grid-auto-columns:30px;grid-auto-rows:15px"><div id=a style="grid-column:4;grid-row:3"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 800.0, 50.0]),
            ("a", [110.0, 35.0, 30.0, 15.0]),
        ],
    },
    Case {
        name: "negative_beyond_explicit_start",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:50px 50px;grid-auto-columns:30px;grid-template-rows:20px"><div id=a style="grid-column:-4/-3"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 800.0, 20.0]),
            ("a", [0.0, 0.0, 30.0, 20.0]),
        ],
    },
    Case {
        name: "grid_area_shorthand",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:repeat(4,50px);grid-template-rows:repeat(4,20px)"><div id=a style="grid-area:2/2/4/4"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 800.0, 80.0]),
            ("a", [50.0, 20.0, 100.0, 40.0]),
        ],
    },
    Case {
        name: "grid_row_span_content_sized",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:100px 100px"><div id=a style="grid-row:1/span 2;height:60px"></div><div id=b style="height:10px"></div><div id=d style="height:10px"></div></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("c", [0.0, 0.0, 800.0, 60.0]),
            ("a", [0.0, 0.0, 100.0, 60.0]),
            ("b", [100.0, 0.0, 100.0, 10.0]),
            ("d", [100.0, 30.0, 100.0, 10.0]),
        ],
    },
];
