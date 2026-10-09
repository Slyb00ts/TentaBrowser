// Expected rectangles recorded from Chrome 154 (getBoundingClientRect, document coordinates).

use super::Case;

pub(super) const CASES: &[Case] = &[
    Case {
        name: "grid_item_default",
        viewport: (800.0, 600.0),
        html: r##"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:300px"><img id=a src="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='200' height='100'%3E%3Crect width='100%25' height='100%25' fill='%23369'/%3E%3C/svg%3E"></div></body></html>"##,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 100.0]),
            ("body", [0.0, 0.0, 800.0, 100.0]),
            ("c", [0.0, 0.0, 800.0, 100.0]),
            ("a", [0.0, 0.0, 200.0, 100.0]),
        ],
    },
    Case {
        name: "grid_item_width_100",
        viewport: (800.0, 600.0),
        html: r##"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:150px"><img id=a style="width:100%" src="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='200' height='100'%3E%3Crect width='100%25' height='100%25' fill='%23369'/%3E%3C/svg%3E"></div></body></html>"##,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 75.0]),
            ("body", [0.0, 0.0, 800.0, 75.0]),
            ("c", [0.0, 0.0, 800.0, 75.0]),
            ("a", [0.0, 0.0, 150.0, 75.0]),
        ],
    },
    Case {
        name: "grid_item_stretch_explicit",
        viewport: (800.0, 600.0),
        html: r##"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:150px;grid-template-rows:150px;justify-items:stretch;align-items:stretch"><img id=a style="width:auto;height:auto" src="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='200' height='100'%3E%3Crect width='100%25' height='100%25' fill='%23369'/%3E%3C/svg%3E"></div></body></html>"##,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 150.0]),
            ("body", [0.0, 0.0, 800.0, 150.0]),
            ("c", [0.0, 0.0, 800.0, 150.0]),
            ("a", [0.0, 0.0, 150.0, 150.0]),
        ],
    },
    Case {
        name: "grid_auto_track_from_image",
        viewport: (800.0, 600.0),
        html: r##"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:auto auto"><img id=a src="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='80' height='40'%3E%3Crect width='100%25' height='100%25' fill='%23369'/%3E%3C/svg%3E"><img id=b src="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='120' height='40'%3E%3Crect width='100%25' height='100%25' fill='%23369'/%3E%3C/svg%3E"></div></body></html>"##,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 40.0]),
            ("body", [0.0, 0.0, 800.0, 40.0]),
            ("c", [0.0, 0.0, 800.0, 40.0]),
            ("a", [0.0, 0.0, 80.0, 40.0]),
            ("b", [380.0, 0.0, 120.0, 40.0]),
        ],
    },
    Case {
        name: "grid_fr_tracks_images",
        viewport: (800.0, 600.0),
        html: r##"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:1fr 1fr;width:400px"><img id=a style="width:100%" src="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='80' height='40'%3E%3Crect width='100%25' height='100%25' fill='%23369'/%3E%3C/svg%3E"><img id=b style="width:100%" src="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='120' height='60'%3E%3Crect width='100%25' height='100%25' fill='%23369'/%3E%3C/svg%3E"></div></body></html>"##,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 100.0]),
            ("body", [0.0, 0.0, 800.0, 100.0]),
            ("c", [0.0, 0.0, 400.0, 100.0]),
            ("a", [0.0, 0.0, 200.0, 100.0]),
            ("b", [200.0, 0.0, 200.0, 100.0]),
        ],
    },
    Case {
        name: "grid_fr_min_content",
        viewport: (800.0, 600.0),
        html: r##"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:1fr 1fr;width:100px"><img id=a src="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='80' height='40'%3E%3Crect width='100%25' height='100%25' fill='%23369'/%3E%3C/svg%3E"><img id=b src="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='120' height='40'%3E%3Crect width='100%25' height='100%25' fill='%23369'/%3E%3C/svg%3E"></div></body></html>"##,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 40.0]),
            ("body", [0.0, 0.0, 800.0, 40.0]),
            ("c", [0.0, 0.0, 100.0, 40.0]),
            ("a", [0.0, 0.0, 80.0, 40.0]),
            ("b", [80.0, 0.0, 120.0, 40.0]),
        ],
    },
    Case {
        name: "grid_align_center",
        viewport: (800.0, 600.0),
        html: r##"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="display:grid;grid-template-columns:200px;grid-template-rows:200px;align-items:center;justify-items:center"><img id=a src="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='200' height='100'%3E%3Crect width='100%25' height='100%25' fill='%23369'/%3E%3C/svg%3E"></div></body></html>"##,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 200.0]),
            ("body", [0.0, 0.0, 800.0, 200.0]),
            ("c", [0.0, 0.0, 800.0, 200.0]),
            ("a", [0.0, 50.0, 200.0, 100.0]),
        ],
    },
];
