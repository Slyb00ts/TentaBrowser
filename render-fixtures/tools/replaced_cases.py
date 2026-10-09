import urllib.parse

def img(w=None, h=None, vb=None, color="369"):
    """An SVG data URL with the given natural size and/or view box."""
    attrs = "xmlns='http://www.w3.org/2000/svg'"
    if w is not None:
        attrs += f" width='{w}'"
    if h is not None:
        attrs += f" height='{h}'"
    if vb is not None:
        attrs += f" viewBox='0 0 {vb[0]} {vb[1]}'"
    inner = "<rect width='100%25' height='100%25' fill='%23" + color + "'/>"
    return "data:image/svg+xml," + "%3Csvg " + attrs + "%3E" + inner.replace("<", "%3C").replace(">", "%3E") + "%3C/svg%3E"

HEAD = "<!doctype html><html id=html><head><meta charset=utf-8><style>html,body{margin:0}body{font:16px 'Liberation Sans'}%s</style></head><body id=body>%s</body></html>"

def page(body, css=""):
    return HEAD % (css, body)

def I(w=200, h=100, **kw):
    return img(w, h, **kw)

BLOCK = []
def b(name, body, css="", vp=(800, 600)):
    BLOCK.append((name, vp, page(body, css)))

# ---- block-level replaced boxes
b("block_natural", f'<img id=a style="display:block" src="{I()}">')
b("block_width_only", f'<img id=a style="display:block;width:50px" src="{I()}">')
b("block_height_only", f'<img id=a style="display:block;height:40px" src="{I()}">')
b("block_both", f'<img id=a style="display:block;width:30px;height:70px" src="{I()}">')
b("block_percent_width", f'<div id=c style="width:400px"><img id=a style="display:block;width:50%" src="{I()}"></div>')
b("block_max_width_percent", f'<div id=c style="width:150px"><img id=a style="display:block;max-width:100%" src="{I()}"></div>')
b("block_max_height", f'<img id=a style="display:block;max-height:30px" src="{I()}">')
b("block_min_width", f'<img id=a style="display:block;min-width:300px" src="{I()}">')
b("block_width_over_max", f'<img id=a style="display:block;width:300px;max-width:100px" src="{I()}">')
b("block_table_10_4_a", f'<img id=a style="display:block;min-width:300px;max-height:50px" src="{I()}">')
b("block_table_10_4_b", f'<img id=a style="display:block;max-width:100px;min-height:80px" src="{I()}">')
b("block_table_10_4_c", f'<img id=a style="display:block;max-width:100px;max-height:30px" src="{I()}">')
b("block_table_10_4_d", f'<img id=a style="display:block;min-width:300px;min-height:200px" src="{I()}">')
b("block_auto_margins", f'<div id=c style="width:400px"><img id=a style="display:block;margin:0 auto" src="{I()}"></div>')
b("block_padding_border", f'<img id=a style="display:block;padding:5px;border:3px solid red;width:50px" src="{I()}">')
b("block_border_box", f'<img id=a style="display:block;box-sizing:border-box;padding:5px;border:3px solid red;width:50px" src="{I()}">')
b("block_height_percent_auto_parent", f'<div id=c style="width:300px"><img id=a style="display:block;height:50%" src="{I()}"></div>')
b("block_height_percent_definite", f'<div id=c style="width:300px;height:200px"><img id=a style="display:block;height:50%" src="{I()}"></div>')
b("block_ratio_only_fills_width", f'<div id=c style="width:300px"><img id=a style="display:block" src="{img(None, None, vb=(40, 20))}"></div>')
b("block_ratio_only_width", f'<img id=a style="display:block;width:100px" src="{img(None, None, vb=(40, 20))}">')
b("block_ratio_only_height", f'<img id=a style="display:block;height:100px" src="{img(None, None, vb=(40, 20))}">')
b("block_css_aspect_ratio", f'<img id=a style="display:block;width:100px;aspect-ratio:1/1" src="{I()}">')
b("block_attr_width_height", f'<img id=a style="display:block" width=60 height=30 src="{I()}">')
b("block_attr_width_only", f'<img id=a style="display:block" width=60 src="{I()}">')
b("block_attr_width_css_height_auto", f'<img id=a style="display:block;height:auto" width=60 height=60 src="{I()}">')
b("block_object_fit_keeps_box", f'<img id=a style="display:block;width:100px;height:100px;object-fit:cover" src="{I()}">')
b("block_zero_width", f'<img id=a style="display:block;width:0" src="{I()}">')
b("block_zero_height", f'<img id=a style="display:block;height:0" src="{I()}">')
b("block_fit_content_wrapper", f'<div id=c style="width:fit-content"><img id=a style="display:block" src="{I()}"></div>')
b("block_max_height_percent_auto_parent", f'<div id=c style="width:300px"><img id=a style="display:block;max-height:50%" src="{I()}"></div>')

INLINE = []
def n(name, body, css="", vp=(800, 600)):
    INLINE.append((name, vp, page(body, css)))

n("inline_in_text", f'<div id=c>&#8203;<img id=a src="{I(40, 30)}">&#8203;</div>')
n("inline_alone", f'<div id=c><img id=a src="{I(40, 30)}"></div>')
n("inline_valign_middle", f'<div id=c>&#8203;<img id=a style="vertical-align:middle" src="{I(40, 30)}">&#8203;</div>')
n("inline_valign_top", f'<div id=c style="line-height:50px">&#8203;<img id=a style="vertical-align:top" src="{I(40, 30)}">&#8203;</div>')
n("inline_valign_bottom", f'<div id=c style="line-height:50px">&#8203;<img id=a style="vertical-align:bottom" src="{I(40, 30)}">&#8203;</div>')
n("inline_valign_text_top", f'<div id=c>&#8203;<img id=a style="vertical-align:text-top" src="{I(40, 30)}">&#8203;</div>')
n("inline_valign_length", f'<div id=c>&#8203;<img id=a style="vertical-align:-10px" src="{I(40, 30)}">&#8203;</div>')
n("inline_two_images", f'<div id=c><img id=a src="{I(40, 30)}"><img id=b src="{I(50, 20)}"></div>')
n("inline_wraps", f'<div id=c style="width:100px"><img id=a src="{I(60, 30)}"><img id=b src="{I(60, 20)}"></div>')
n("inline_percent_width", f'<div id=c style="width:200px"><img id=a style="width:50%" src="{I(40, 30)}"></div>')
n("inline_ratio_only", f'<div id=c style="width:200px"><img id=a src="{img(None, None, vb=(40, 20))}"></div>')
n("inline_max_width", f'<div id=c style="width:100px"><img id=a style="max-width:100%" src="{I(300, 150)}"></div>')
n("inline_block_wrapper", f'<span id=c style="display:inline-block"><img id=a src="{I(80, 40)}"></span>')
n("inline_block_wrapper_percent", f'<span id=c style="display:inline-block"><img id=a style="width:50%" src="{I(80, 40)}"></span>')
n("inline_block_image", f'<div id=c><img id=a style="display:inline-block;padding:4px" src="{I(80, 40)}"></div>')

FLEX = []
def f(name, body, css="", vp=(800, 600)):
    FLEX.append((name, vp, page(body, css)))

f("flex_row_item", f'<div id=c style="display:flex"><img id=a src="{I()}"><div id=d style="width:50px;height:150px"></div></div>')
f("flex_row_align_center", f'<div id=c style="display:flex;align-items:center"><img id=a src="{I()}"><div id=d style="width:50px;height:150px"></div></div>')
f("flex_row_align_stretch_explicit", f'<div id=c style="display:flex;align-items:stretch"><img id=a src="{I()}"><div id=d style="width:50px;height:150px"></div></div>')
f("flex_row_shrink", f'<div id=c style="display:flex;width:150px"><img id=a src="{I()}"><img id=b src="{I()}"></div>')
f("flex_row_shrink_height_auto", f'<div id=c style="display:flex;width:150px;align-items:flex-start"><img id=a style="height:auto" src="{I()}"></div>')
f("flex_column_item", f'<div id=c style="display:flex;flex-direction:column;width:300px"><img id=a src="{I()}"></div>')
f("flex_column_stretch_explicit", f'<div id=c style="display:flex;flex-direction:column;width:300px;align-items:stretch"><img id=a style="width:auto" src="{I()}"></div>')
f("flex_basis", f'<div id=c style="display:flex"><img id=a style="flex:0 0 100px" src="{I()}"></div>')
f("flex_grow_images", f'<div id=c style="display:flex;width:500px"><img id=a style="flex:1" src="{I(100, 100)}"><img id=b style="flex:2" src="{I(100, 100)}"></div>')
f("flex_percent_height", f'<div id=c style="display:flex;height:200px"><img id=a style="height:50%" src="{I()}"></div>')
f("flex_item_ratio_only", f'<div id=c style="display:flex;width:300px"><img id=a src="{img(None, None, vb=(40, 20))}"></div>')
f("flex_item_ratio_only_wide_container", f'<div id=c style="display:flex;width:500px"><img id=a src="{img(None, None, vb=(40, 20))}"></div>')
f("flex_column_ratio_only", f'<div id=c style="display:flex;flex-direction:column;width:400px"><img id=a src="{img(None, None, vb=(40, 20))}"></div>')
f("flex_wrap_images", f'<div id=c style="display:flex;flex-wrap:wrap;width:250px"><img id=a src="{I(100, 50)}"><img id=b src="{I(100, 50)}"><img id=d src="{I(100, 50)}"></div>')

GRID = []
def g(name, body, css="", vp=(800, 600)):
    GRID.append((name, vp, page(body, css)))

g("grid_item_default", f'<div id=c style="display:grid;grid-template-columns:300px"><img id=a src="{I()}"></div>')
g("grid_item_width_100", f'<div id=c style="display:grid;grid-template-columns:150px"><img id=a style="width:100%" src="{I()}"></div>')
g("grid_item_stretch_explicit", f'<div id=c style="display:grid;grid-template-columns:150px;grid-template-rows:150px;justify-items:stretch;align-items:stretch"><img id=a style="width:auto;height:auto" src="{I()}"></div>')
g("grid_auto_track_from_image", f'<div id=c style="display:grid;grid-template-columns:auto auto"><img id=a src="{I(80, 40)}"><img id=b src="{I(120, 40)}"></div>')
g("grid_fr_tracks_images", f'<div id=c style="display:grid;grid-template-columns:1fr 1fr;width:400px"><img id=a style="width:100%" src="{I(80, 40)}"><img id=b style="width:100%" src="{I(120, 60)}"></div>')
g("grid_fr_min_content", f'<div id=c style="display:grid;grid-template-columns:1fr 1fr;width:100px"><img id=a src="{I(80, 40)}"><img id=b src="{I(120, 40)}"></div>')
g("grid_align_center", f'<div id=c style="display:grid;grid-template-columns:200px;grid-template-rows:200px;align-items:center;justify-items:center"><img id=a src="{I()}"></div>')

ABS = []
def a(name, body, css="", vp=(800, 600)):
    ABS.append((name, vp, page(body, css)))

a("abs_static", f'<div id=c style="position:relative;padding:10px"><img id=a style="position:absolute" src="{I()}"></div>')
a("abs_left_right", f'<div id=c style="position:relative;width:400px;height:200px"><img id=a style="position:absolute;left:10px;right:10px" src="{I()}"></div>')
a("abs_top_bottom", f'<div id=c style="position:relative;width:400px;height:300px"><img id=a style="position:absolute;top:10px;bottom:10px" src="{I()}"></div>')
a("abs_width_only", f'<div id=c style="position:relative;width:400px;height:300px"><img id=a style="position:absolute;width:100px;left:5px;top:5px" src="{I()}"></div>')
a("abs_height_only", f'<div id=c style="position:relative;width:400px;height:300px"><img id=a style="position:absolute;height:50px;left:5px;top:5px" src="{I()}"></div>')
a("abs_margin_auto_center", f'<div id=c style="position:relative;width:400px;height:300px"><img id=a style="position:absolute;left:0;right:0;top:0;bottom:0;margin:auto" src="{I()}"></div>')
a("abs_percent_width", f'<div id=c style="position:relative;width:400px;height:300px"><img id=a style="position:absolute;width:50%;left:0;top:0" src="{I()}"></div>')
a("abs_max_width", f'<div id=c style="position:relative;width:400px;height:300px"><img id=a style="position:absolute;max-width:80px;left:0;top:0" src="{I()}"></div>')
a("abs_ratio_only", f'<div id=c style="position:relative;width:400px;height:300px"><img id=a style="position:absolute;left:0;top:0" src="{img(None, None, vb=(40, 20))}"></div>')
a("abs_ratio_only_shrink_container", f'<div id=c style="position:relative;width:240px;height:300px;padding:0 20px"><img id=a style="position:absolute;left:0;top:0" src="{img(None, None, vb=(40, 20))}"></div>')
a("fixed_image", f'<img id=a style="position:fixed;right:10px;bottom:10px" src="{I(80, 40)}">')
a("abs_right_bottom", f'<div id=c style="position:relative;width:400px;height:300px"><img id=a style="position:absolute;right:10px;bottom:20px" src="{I()}"></div>')
a("abs_all_four_insets", f'<div id=c style="position:relative;width:400px;height:300px"><img id=a style="position:absolute;left:10px;right:20px;top:30px;bottom:40px" src="{I()}"></div>')

MISC = []
def m(name, body, css="", vp=(800, 600)):
    MISC.append((name, vp, page(body, css)))

m("broken_attrs_box", '<img id=a style="display:block" width=60 height=30 src="gone.png">')
m("broken_attrs_ratio_height_auto", '<img id=a style="display:block;width:100px;height:auto" width=60 height=30 src="gone.png">')
m("broken_alt_empty", '<div id=c>&#8203;<img id=a alt="" src="gone.png">&#8203;</div>')
m("grid_ratio_only_item", f'<div id=c style="display:grid;grid-template-columns:200px"><img id=a src="{img(None, None, vb=(40, 20))}"></div>')
m("body_with_two_blocks", f'<img id=a style="display:block" src="{I(100, 30)}"><img id=b style="display:block" src="{I(50, 60)}">')
m("img_in_li", f'<ul id=c><li id=l><img id=a src="{I(20, 20)}"></li></ul>')
