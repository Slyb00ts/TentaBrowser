// Expected rectangles recorded from Chrome 154 (getBoundingClientRect, document coordinates).

use super::Case;

pub(super) const CASES: &[Case] = &[
    Case {
        name: "abs_static",
        viewport: (800.0, 600.0),
        html: r##"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="position:relative;padding:10px"><img id=a style="position:absolute" src="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='200' height='100'%3E%3Crect width='100%25' height='100%25' fill='%23369'/%3E%3C/svg%3E"></div></body></html>"##,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 20.0]),
            ("body", [0.0, 0.0, 800.0, 20.0]),
            ("c", [0.0, 0.0, 800.0, 20.0]),
            ("a", [10.0, 10.0, 200.0, 100.0]),
        ],
    },
    Case {
        name: "abs_left_right",
        viewport: (800.0, 600.0),
        html: r##"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="position:relative;width:400px;height:200px"><img id=a style="position:absolute;left:10px;right:10px" src="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='200' height='100'%3E%3Crect width='100%25' height='100%25' fill='%23369'/%3E%3C/svg%3E"></div></body></html>"##,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 200.0]),
            ("body", [0.0, 0.0, 800.0, 200.0]),
            ("c", [0.0, 0.0, 400.0, 200.0]),
            ("a", [10.0, 0.0, 200.0, 100.0]),
        ],
    },
    Case {
        name: "abs_top_bottom",
        viewport: (800.0, 600.0),
        html: r##"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="position:relative;width:400px;height:300px"><img id=a style="position:absolute;top:10px;bottom:10px" src="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='200' height='100'%3E%3Crect width='100%25' height='100%25' fill='%23369'/%3E%3C/svg%3E"></div></body></html>"##,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 300.0]),
            ("body", [0.0, 0.0, 800.0, 300.0]),
            ("c", [0.0, 0.0, 400.0, 300.0]),
            ("a", [0.0, 10.0, 200.0, 100.0]),
        ],
    },
    Case {
        name: "abs_width_only",
        viewport: (800.0, 600.0),
        html: r##"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="position:relative;width:400px;height:300px"><img id=a style="position:absolute;width:100px;left:5px;top:5px" src="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='200' height='100'%3E%3Crect width='100%25' height='100%25' fill='%23369'/%3E%3C/svg%3E"></div></body></html>"##,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 300.0]),
            ("body", [0.0, 0.0, 800.0, 300.0]),
            ("c", [0.0, 0.0, 400.0, 300.0]),
            ("a", [5.0, 5.0, 100.0, 50.0]),
        ],
    },
    Case {
        name: "abs_height_only",
        viewport: (800.0, 600.0),
        html: r##"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="position:relative;width:400px;height:300px"><img id=a style="position:absolute;height:50px;left:5px;top:5px" src="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='200' height='100'%3E%3Crect width='100%25' height='100%25' fill='%23369'/%3E%3C/svg%3E"></div></body></html>"##,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 300.0]),
            ("body", [0.0, 0.0, 800.0, 300.0]),
            ("c", [0.0, 0.0, 400.0, 300.0]),
            ("a", [5.0, 5.0, 100.0, 50.0]),
        ],
    },
    Case {
        name: "abs_margin_auto_center",
        viewport: (800.0, 600.0),
        html: r##"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="position:relative;width:400px;height:300px"><img id=a style="position:absolute;left:0;right:0;top:0;bottom:0;margin:auto" src="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='200' height='100'%3E%3Crect width='100%25' height='100%25' fill='%23369'/%3E%3C/svg%3E"></div></body></html>"##,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 300.0]),
            ("body", [0.0, 0.0, 800.0, 300.0]),
            ("c", [0.0, 0.0, 400.0, 300.0]),
            ("a", [100.0, 100.0, 200.0, 100.0]),
        ],
    },
    Case {
        name: "abs_percent_width",
        viewport: (800.0, 600.0),
        html: r##"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="position:relative;width:400px;height:300px"><img id=a style="position:absolute;width:50%;left:0;top:0" src="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='200' height='100'%3E%3Crect width='100%25' height='100%25' fill='%23369'/%3E%3C/svg%3E"></div></body></html>"##,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 300.0]),
            ("body", [0.0, 0.0, 800.0, 300.0]),
            ("c", [0.0, 0.0, 400.0, 300.0]),
            ("a", [0.0, 0.0, 200.0, 100.0]),
        ],
    },
    Case {
        name: "abs_max_width",
        viewport: (800.0, 600.0),
        html: r##"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="position:relative;width:400px;height:300px"><img id=a style="position:absolute;max-width:80px;left:0;top:0" src="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='200' height='100'%3E%3Crect width='100%25' height='100%25' fill='%23369'/%3E%3C/svg%3E"></div></body></html>"##,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 300.0]),
            ("body", [0.0, 0.0, 800.0, 300.0]),
            ("c", [0.0, 0.0, 400.0, 300.0]),
            ("a", [0.0, 0.0, 80.0, 40.0]),
        ],
    },
    Case {
        name: "abs_ratio_only",
        viewport: (800.0, 600.0),
        html: r##"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="position:relative;width:400px;height:300px"><img id=a style="position:absolute;left:0;top:0" src="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 40 20'%3E%3Crect width='100%25' height='100%25' fill='%23369'/%3E%3C/svg%3E"></div></body></html>"##,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 300.0]),
            ("body", [0.0, 0.0, 800.0, 300.0]),
            ("c", [0.0, 0.0, 400.0, 300.0]),
            ("a", [0.0, 0.0, 400.0, 200.0]),
        ],
    },
    Case {
        name: "abs_ratio_only_shrink_container",
        viewport: (800.0, 600.0),
        html: r##"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="position:relative;width:240px;height:300px;padding:0 20px"><img id=a style="position:absolute;left:0;top:0" src="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 40 20'%3E%3Crect width='100%25' height='100%25' fill='%23369'/%3E%3C/svg%3E"></div></body></html>"##,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 300.0]),
            ("body", [0.0, 0.0, 800.0, 300.0]),
            ("c", [0.0, 0.0, 280.0, 300.0]),
            ("a", [0.0, 0.0, 280.0, 140.0]),
        ],
    },
    Case {
        name: "fixed_image",
        viewport: (800.0, 600.0),
        html: r##"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><img id=a style="position:fixed;right:10px;bottom:10px" src="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='80' height='40'%3E%3Crect width='100%25' height='100%25' fill='%23369'/%3E%3C/svg%3E"></body></html>"##,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 0.0]),
            ("body", [0.0, 0.0, 800.0, 0.0]),
            ("a", [710.0, 550.0, 80.0, 40.0]),
        ],
    },
    Case {
        name: "abs_right_bottom",
        viewport: (800.0, 600.0),
        html: r##"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="position:relative;width:400px;height:300px"><img id=a style="position:absolute;right:10px;bottom:20px" src="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='200' height='100'%3E%3Crect width='100%25' height='100%25' fill='%23369'/%3E%3C/svg%3E"></div></body></html>"##,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 300.0]),
            ("body", [0.0, 0.0, 800.0, 300.0]),
            ("c", [0.0, 0.0, 400.0, 300.0]),
            ("a", [190.0, 180.0, 200.0, 100.0]),
        ],
    },
    Case {
        name: "abs_all_four_insets",
        viewport: (800.0, 600.0),
        html: r##"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}</style></head><body id=body><div id=c style="position:relative;width:400px;height:300px"><img id=a style="position:absolute;left:10px;right:20px;top:30px;bottom:40px" src="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='200' height='100'%3E%3Crect width='100%25' height='100%25' fill='%23369'/%3E%3C/svg%3E"></div></body></html>"##,
        scroll: (800.0, 600.0),
        expect: &[
            ("html", [0.0, 0.0, 800.0, 300.0]),
            ("body", [0.0, 0.0, 800.0, 300.0]),
            ("c", [0.0, 0.0, 400.0, 300.0]),
            ("a", [10.0, 30.0, 200.0, 100.0]),
        ],
    },
];
