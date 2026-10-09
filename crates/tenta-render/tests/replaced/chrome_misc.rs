// Expected rectangles recorded from Chrome 154 (getBoundingClientRect, document coordinates).

use super::Case;

pub(super) const CASES: &[Case] = &[
    Case {
        name: "broken_attrs_box",
        viewport: (800.0, 600.0),
        html: r##"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><img id=a style="display:block" width=60 height=30 src="gone.png"></body></html>"##,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 30.0]),
            ("body", [0.0, 0.0, 800.0, 30.0]),
            ("a", [0.0, 0.0, 60.0, 30.0]),
        ],
    },
    Case {
        name: "broken_attrs_ratio_height_auto",
        viewport: (800.0, 600.0),
        html: r##"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><img id=a style="display:block;width:100px;height:auto" width=60 height=30 src="gone.png"></body></html>"##,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 50.0]),
            ("body", [0.0, 0.0, 800.0, 50.0]),
            ("a", [0.0, 0.0, 100.0, 50.0]),
        ],
    },
    Case {
        name: "broken_alt_empty",
        viewport: (800.0, 600.0),
        html: r##"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c>&#8203;<img id=a alt="" src="gone.png">&#8203;</div></body></html>"##,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 18.0]),
            ("body", [0.0, 0.0, 800.0, 18.0]),
            ("c", [0.0, 0.0, 800.0, 18.0]),
            ("a", [0.0, 14.0, 0.0, 0.0]),
        ],
    },
    Case {
        name: "grid_ratio_only_item",
        viewport: (800.0, 600.0),
        html: r##"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:200px"><img id=a src="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 40 20'%3E%3Crect width='100%25' height='100%25' fill='%23369'/%3E%3C/svg%3E"></div></body></html>"##,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 100.0]),
            ("body", [0.0, 0.0, 800.0, 100.0]),
            ("c", [0.0, 0.0, 800.0, 100.0]),
            ("a", [0.0, 0.0, 200.0, 100.0]),
        ],
    },
    Case {
        name: "body_with_two_blocks",
        viewport: (800.0, 600.0),
        html: r##"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><img id=a style="display:block" src="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='100' height='30'%3E%3Crect width='100%25' height='100%25' fill='%23369'/%3E%3C/svg%3E"><img id=b style="display:block" src="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='50' height='60'%3E%3Crect width='100%25' height='100%25' fill='%23369'/%3E%3C/svg%3E"></body></html>"##,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 90.0]),
            ("body", [0.0, 0.0, 800.0, 90.0]),
            ("a", [0.0, 0.0, 100.0, 30.0]),
            ("b", [0.0, 30.0, 50.0, 60.0]),
        ],
    },
    Case {
        name: "img_in_li",
        viewport: (800.0, 600.0),
        html: r##"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><ul id=c><li id=l><img id=a src="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='20' height='20'%3E%3Crect width='100%25' height='100%25' fill='%23369'/%3E%3C/svg%3E"></li></ul></body></html>"##,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 56.0]),
            ("body", [0.0, 16.0, 800.0, 24.0]),
            ("c", [0.0, 16.0, 800.0, 24.0]),
            ("l", [40.0, 16.0, 760.0, 24.0]),
            ("a", [40.0, 16.0, 20.0, 20.0]),
        ],
    },
];
