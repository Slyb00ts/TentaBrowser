// Expected rectangles recorded from Chrome 154: the union of the first line's getClientRects(), in document coordinates.

use super::Case;

pub(super) const CASES: &[Case] = &[
    Case {
        name: "ltr_hebrew_before_parenthesised_hebrew",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'DejaVu Sans'} .r{direction:rtl}</style></head><body id=body><div id=p style='width:300px'><span id=a>ב</span> (<span id=b>ג</span>) <span id=c>a</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 300.0, 19.0]),
            ("a", [24.172, 0.0, 9.25, 19.0]),
            ("b", [6.25, 0.0, 6.594, 19.0]),
            ("c", [38.516, 0.0, 9.812, 19.0]),
        ],
    },
    Case {
        name: "rtl_hebrew_before_parenthesised_hebrew",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'DejaVu Sans'} .r{direction:rtl}</style></head><body id=body><div id=p class=r style='width:300px'><span id=a>ב</span> (<span id=b>ג</span>) <span id=c>a</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 300.0, 19.0]),
            ("a", [290.75, 0.0, 9.25, 19.0]),
            ("b", [272.828, 0.0, 6.594, 19.0]),
            ("c", [251.688, 0.0, 9.812, 19.0]),
        ],
    },
    Case {
        name: "ltr_latin_in_parentheses_after_hebrew",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'DejaVu Sans'} .r{direction:rtl}</style></head><body id=body><div id=p style='width:300px'><span id=a>אבג</span> (<span id=b>abc</span>) <span id=c>דהו</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 300.0, 19.0]),
            ("a", [0.0, 0.0, 26.547, 19.0]),
            ("b", [37.875, 0.0, 28.766, 19.0]),
            ("c", [77.969, 0.0, 23.547, 19.0]),
        ],
    },
    Case {
        name: "ltr_arabic_indic_digits",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'DejaVu Sans'} .r{direction:rtl}</style></head><body id=body><div id=p style='width:300px'><span id=a>abc</span> <span id=n>١٢٣</span> <span id=h>אבג</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 300.0, 19.0]),
            ("a", [0.0, 0.0, 28.766, 19.0]),
            ("n", [65.5, 0.0, 25.781, 19.0]),
            ("h", [33.859, 0.0, 26.547, 19.0]),
        ],
    },
    Case {
        name: "rtl_arabic_indic_digits",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'DejaVu Sans'} .r{direction:rtl}</style></head><body id=body><div id=p class=r style='width:300px'><span id=a>אבג</span> <span id=n>١٢٣</span> <span id=l>abc</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 300.0, 19.0]),
            ("a", [273.453, 0.0, 26.547, 19.0]),
            ("n", [242.578, 0.0, 25.781, 19.0]),
            ("l", [208.719, 0.0, 28.766, 19.0]),
        ],
    },
    Case {
        name: "ltr_arabic_text_with_indic_digits",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'DejaVu Sans'} .r{direction:rtl}</style></head><body id=body><div id=p style='width:300px'><span id=a>مرحبا</span> <span id=n>١٢٣</span> <span id=b>بالعالم</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 300.0, 19.0]),
            ("a", [78.297, 0.0, 37.0, 19.0]),
            ("n", [47.422, 0.0, 25.781, 19.0]),
            ("b", [0.0, 0.0, 42.328, 19.0]),
        ],
    },
    Case {
        name: "ltr_arabic_digits_after_latin",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'DejaVu Sans'} .r{direction:rtl}</style></head><body id=body><div id=p style='width:300px'><span id=a>abc</span> <span id=n>١٢٣</span> <span id=b>def</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 300.0, 19.0]),
            ("a", [0.0, 0.0, 28.766, 19.0]),
            ("n", [33.859, 0.0, 25.781, 19.0]),
            ("b", [64.734, 0.0, 25.641, 19.0]),
        ],
    },
    Case {
        name: "ltr_minus_sign_after_hebrew",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'DejaVu Sans'} .r{direction:rtl}</style></head><body id=body><div id=p style='width:300px'><span id=h>אבג</span> <span id=m>−5</span> <span id=l>abc</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 300.0, 19.0]),
            ("h", [28.688, 0.0, 26.547, 19.0]),
            ("m", [0.0, 0.0, 23.594, 19.0]),
            ("l", [60.328, 0.0, 28.766, 19.0]),
        ],
    },
    Case {
        name: "rtl_minus_sign",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'DejaVu Sans'} .r{direction:rtl}</style></head><body id=body><div id=p class=r style='width:300px'><span id=l>abc</span> <span id=m>−5</span> <span id=h>אבג</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 300.0, 19.0]),
            ("l", [242.547, 0.0, 28.766, 19.0]),
            ("m", [276.406, 0.0, 23.594, 19.0]),
            ("h", [210.906, 0.0, 26.547, 19.0]),
        ],
    },
    Case {
        name: "ltr_minus_sign_between_digits_in_hebrew",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'DejaVu Sans'} .r{direction:rtl}</style></head><body id=body><div id=p style='width:300px'><span id=h>אבג</span> <span id=m>7−5</span> <span id=b>דהו</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 300.0, 19.0]),
            ("h", [67.5, 0.0, 26.547, 19.0]),
            ("m", [28.641, 0.0, 33.766, 19.0]),
            ("b", [0.0, 0.0, 23.547, 19.0]),
        ],
    },
    Case {
        name: "rtl_trailing_minus_sign",
        viewport: (800.0, 600.0),
        html: r#"<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'DejaVu Sans'} .r{direction:rtl}</style></head><body id=body><div id=p class=r style='width:300px'><span id=n>5</span><span id=m>−</span> <span id=h>אבג</span></div></body></html>"#,
        scroll: (800.0, 600.0),
        expect: &[
            ("p", [0.0, 0.0, 300.0, 19.0]),
            ("n", [289.812, 0.0, 10.188, 19.0]),
            ("m", [276.406, 0.0, 13.406, 19.0]),
            ("h", [244.766, 0.0, 26.547, 19.0]),
        ],
    },
];
