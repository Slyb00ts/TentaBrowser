"""Table layout cases recorded against Chrome by record_geometry.py.

Usage: record_geometry.py table_cases.py GROUP out.rs  (GROUP is one name below)
Every group is a list of (name, (viewport width, height), html).
"""


def page(body, css=""):
    return (
        "<!doctype html><html id=html><head><style>html,body{margin:0}"
        "body{font:16px 'Liberation Sans'}" + css + "</style></head><body id=body>"
        + body + "</body></html>"
    )


VP = (800, 600)
TXT = "aa bb cc dd ee ff gg hh ii jj kk ll mm nn oo pp"
BARE = "td,th{padding:0;border:0}table{border-spacing:0}"


def BOX(w, h=10):
    return f"<div style='width:{w}px;height:{h}px'></div>"


def case(name, body, css="", vp=VP):
    return (name, vp, page(body, css))


# --------------------------------------------------------------- structure
STRUCTURE = [
    case("bs_two_by_two", "<table id=t><tr><td id=a>one</td><td id=b>two</td></tr><tr><td id=c>three</td><td id=d>four</td></tr></table>"),
    case("bs_defaults_th", "<table id=t><tr><th id=a>head</th><th id=b>h2</th></tr><tr><td id=c>cell</td><td id=d>cell two</td></tr></table>"),
    case("bs_spacing_none", "<table id=t style='border-spacing:0'><tr><td id=a>one</td><td id=b>two</td></tr></table>"),
    case("bs_spacing_xy", "<table id=t style='border-spacing:5px 9px'><tr><td id=a>one</td><td id=b>two</td></tr><tr><td id=c>3</td><td id=d>4</td></tr></table>"),
    case("bs_cellspacing_attr", "<table id=t cellspacing=6><tr><td id=a>one</td><td id=b>two</td></tr></table>"),
    case("bs_cellpadding_attr", "<table id=t cellpadding=7><tr><td id=a>one</td><td id=b>two</td></tr></table>"),
    case("bs_border_attr", "<table id=t border=3><tr><td id=a>one</td><td id=b>two</td></tr></table>"),
    case("bs_border_padding_table", "<table id=t style='border:4px solid red;padding:6px'><tr><td id=a>one</td><td id=b>two</td></tr></table>"),
    case("bs_sections", "<table id=t><thead id=h><tr><td id=a>head</td></tr></thead><tbody id=b><tr><td id=c>body</td></tr></tbody><tfoot id=f><tr><td id=d>foot</td></tr></tfoot></table>"),
    case("bs_footer_first_in_source", "<table id=t><tfoot id=f><tr><td id=a>foot</td></tr></tfoot><tbody id=b><tr><td id=c>body</td></tr></tbody><thead id=h><tr><td id=d>head</td></tr></thead></table>"),
    case("bs_two_bodies", "<table id=t><tbody id=b1><tr><td id=a>one</td></tr></tbody><tbody id=b2><tr><td id=c>two</td></tr></tbody></table>"),
    case("bs_empty_table", "<table id=t></table><p id=p>after</p>"),
    case("bs_empty_row", "<table id=t><tr id=r></tr><tr><td id=a>x</td></tr></table>"),
    case("bs_empty_cells", "<table id=t><tr><td id=a></td><td id=b></td></tr></table>"),
    case("bs_ragged_rows", "<table id=t><tr><td id=a>1</td><td id=b>2</td><td id=c>3</td></tr><tr><td id=d>4</td></tr></table>"),
    case("bs_colspan", "<table id=t><tr><td id=a>1</td><td id=b>2</td><td id=c>3</td></tr><tr><td id=d colspan=2>wide cell</td><td id=e>5</td></tr></table>"),
    case("bs_colspan_all", "<table id=t><tr><td id=a colspan=3>across</td></tr><tr><td id=b>1</td><td id=c>2</td><td id=d>3</td></tr></table>"),
    case("bs_rowspan", "<table id=t><tr><td id=a rowspan=2>tall</td><td id=b>1</td></tr><tr><td id=c>2</td></tr></table>"),
    case("bs_rowspan_colspan", "<table id=t><tr><td id=a rowspan=2 colspan=2>big</td><td id=b>1</td></tr><tr><td id=c>2</td></tr><tr><td id=d>3</td><td id=e>4</td><td id=f>5</td></tr></table>"),
    case("bs_rowspan_zero", "<table id=t><tr><td id=a rowspan=0>all</td><td id=b>1</td></tr><tr><td id=c>2</td></tr><tr><td id=d>3</td></tr></table>"),
    case("bs_rowspan_past_section", "<table id=t><tbody><tr><td id=a rowspan=5>x</td><td id=b>1</td></tr></tbody><tbody><tr><td id=c>2</td><td id=d>3</td></tr></tbody></table>"),
    case("bs_cols", "<table id=t><colgroup id=g><col id=c1 style='width:60px'><col id=c2 style='width:90px'></colgroup><tr><td id=a>1</td><td id=b>2</td></tr></table>"),
    case("bs_col_span_attr", "<table id=t><col id=c1 span=2 style='width:50px'><tr><td id=a>1</td><td id=b>2</td><td id=c>3</td></tr></table>"),
    case("bs_colgroup_span", "<table id=t><colgroup id=g span=2 style='width:70px'></colgroup><tr><td id=a>1</td><td id=b>2</td></tr></table>"),
    case("bs_valign_attr", "<table id=t style='border-spacing:0'><tr><td id=a valign=top style='height:50px'>t</td><td id=b valign=bottom>b</td><td id=c>m</td></tr></table>"),
    case("bs_nowrap", "<table id=t style='width:100px'><tr><td id=a nowrap>aaaa bbbb cccc dddd</td></tr></table>"),
    case("bs_td_width_attr", "<table id=t><tr><td id=a width=120>a</td><td id=b>b</td></tr></table>"),
    case("bs_table_width_attr", "<table id=t width=300><tr><td id=a>a</td><td id=b>b</td></tr></table>"),
    case("bs_table_align_center", "<table id=t align=center width=200><tr><td id=a>a</td></tr></table>"),
    case("bs_bgcolor", "<table id=t bgcolor=red><tr id=r bgcolor=blue><td id=a bgcolor=green>a</td></tr></table>"),
    case("bs_margin_auto", "<table id=t style='margin:0 auto;width:300px'><tr><td id=a>a</td></tr></table>"),
    case("bs_margins", "<p id=p style='margin:0'>x</p><table id=t style='margin:11px 22px 33px 44px'><tr><td id=a>a</td></tr></table><p id=q style='margin:0'>y</p>"),
]

ANON = [
    case("an_display_table_divs", "<div id=t style='display:table'><div id=r style='display:table-row'><div id=a style='display:table-cell'>one</div><div id=b style='display:table-cell'>two</div></div></div>"),
    case("an_cells_only", "<div id=t style='display:table'><div id=a style='display:table-cell'>one</div><div id=b style='display:table-cell'>two</div></div>"),
    case("an_cells_in_div", "<div id=p><div id=a style='display:table-cell'>one</div><div id=b style='display:table-cell'>two</div></div>"),
    case("an_row_in_div", "<div id=p><div id=r style='display:table-row'><div id=a style='display:table-cell'>x</div></div></div>"),
    case("an_text_in_table", "<div id=t style='display:table'>loose text<div id=r style='display:table-row'><div id=a style='display:table-cell'>x</div></div></div>"),
    case("an_text_in_row", "<div id=t style='display:table'><div id=r style='display:table-row'>text<div id=a style='display:table-cell'>x</div></div></div>"),
    case("an_block_in_row", "<div id=t style='display:table'><div id=r style='display:table-row'><div id=b>block</div><div id=a style='display:table-cell'>x</div></div></div>"),
    case("an_rowgroup_divs", "<div id=t style='display:table'><div id=g style='display:table-row-group'><div style='display:table-row'><div id=a style='display:table-cell'>x</div></div></div></div>"),
    case("an_header_footer_divs", "<div id=t style='display:table'><div style='display:table-footer-group'><div style='display:table-row'><div id=a style='display:table-cell'>foot</div></div></div><div style='display:table-header-group'><div style='display:table-row'><div id=b style='display:table-cell'>head</div></div></div></div>"),
    case("an_whitespace", "<div id=t style='display:table'> <div id=r style='display:table-row'> <div id=a style='display:table-cell'>x</div> <div id=b style='display:table-cell'>y</div> </div> </div>"),
    case("an_inline_table", "before<div id=t style='display:inline-table'><div style='display:table-row'><div id=a style='display:table-cell'>cell</div></div></div>after"),
    case("an_inline_table_el", "x <table id=t style='display:inline-table'><tr><td id=a>a</td></tr></table> y"),
    case("an_table_cell_on_span", "<table id=t><tr><span id=s style='display:table-cell'>x</span><td id=a>y</td></tr></table>"),
    case("an_span_row", "<span id=t style='display:table'><span id=r style='display:table-row'><span id=a style='display:table-cell'>x</span></span></span>"),
    case("an_clearfix_after", "<div id=p style='background:red'><div id=f style='float:left;width:50px;height:50px'></div><div id=c class=cf></div></div>", ".cf:after{content:' ';display:table;clear:both}"),
    case("an_caption_div", "<div id=t style='display:table'><div id=c style='display:table-caption'>cap</div><div style='display:table-row'><div id=a style='display:table-cell'>x</div></div></div>"),
    case("an_col_divs", "<div id=t style='display:table'><div style='display:table-column;width:80px'></div><div style='display:table-row'><div id=a style='display:table-cell'>x</div><div id=b style='display:table-cell'>y</div></div></div>"),
    case("an_cell_padding_div", "<div id=t style='display:table'><div style='display:table-row'><div id=a style='display:table-cell;padding:7px'>x</div></div></div>"),
    case("an_mixed_blocks_between_rows", "<div id=p><div id=a style='display:table-cell'>cell</div><div id=b>block</div><div id=c style='display:table-cell'>cell2</div></div>"),
]

# --------------------------------------------------------------- column widths
COLUMNS = [
    case("cw_auto_text", f"<table id=t><tr><td id=a>short</td><td id=b>{TXT}</td></tr></table>", BARE),
    case("cw_auto_narrow", f"<div style='width:200px'><table id=t><tr><td id=a>short</td><td id=b>{TXT}</td></tr></table></div>", BARE),
    case("cw_auto_two_texts_narrow", f"<div style='width:300px'><table id=t><tr><td id=a>{TXT}</td><td id=b>{TXT} {TXT}</td></tr></table></div>", BARE),
    case("cw_fixed_cell", f"<table id=t><tr><td id=a style='width:100px'>x</td><td id=b>{BOX(30)}</td></tr></table>", BARE),
    case("cw_fixed_cell_text_wraps", f"<table id=t><tr><td id=a style='width:60px'>{TXT}</td><td id=b>x</td></tr></table>", BARE),
    case("cw_fixed_vs_min", f"<table id=t><tr><td id=a style='width:20px'>{BOX(80)}</td><td id=b>x</td></tr></table>", BARE),
    case("cw_two_fixed", "<table id=t><tr><td id=a style='width:50px'>x</td><td id=b style='width:70px'>y</td></tr></table>", BARE),
    case("cw_fixed_in_wide_table", f"<table id=t style='width:500px'><tr><td id=a style='width:50px'>x</td><td id=b>{BOX(40)}</td><td id=c>{BOX(80)}</td></tr></table>", BARE),
    case("cw_percent_cells", f"<table id=t style='width:400px'><tr><td id=a style='width:25%'>x</td><td id=b style='width:50%'>y</td><td id=c>{BOX(30)}</td></tr></table>", BARE),
    case("cw_percent_over_100", "<table id=t style='width:400px'><tr><td id=a style='width:70%'>x</td><td id=b style='width:60%'>y</td></tr></table>", BARE),
    case("cw_percent_auto_table", f"<table id=t><tr><td id=a style='width:50%'>{BOX(40)}</td><td id=b>{BOX(60)}</td></tr></table>", BARE),
    case("cw_percent_narrow_parent", f"<div style='width:200px'><table id=t><tr><td id=a style='width:80%'>{BOX(40)}</td><td id=b>{TXT}</td></tr></table></div>", BARE),
    case("cw_percent_table_width", "<div style='width:600px'><table id=t style='width:50%'><tr><td id=a>x</td><td id=b>y</td></tr></table></div>", BARE),
    case("cw_percent_and_fixed", f"<table id=t style='width:500px'><tr><td id=a style='width:20%'>x</td><td id=b style='width:100px'>y</td><td id=c>{BOX(10)}</td></tr></table>", BARE),
    case("cw_colspan_text", f"<table id=t style='width:300px'><tr><td id=a>aaa</td><td id=b>bbb</td></tr><tr><td id=c colspan=2>{TXT}</td></tr></table>", BARE),
    case("cw_colspan_box", f"<table id=t><tr><td id=a>{BOX(30)}</td><td id=b>{BOX(40)}</td></tr><tr><td id=c colspan=2>{BOX(200)}</td></tr></table>", BARE),
    case("cw_colspan_vs_fixed", f"<table id=t><tr><td id=a style='width:50px'>x</td><td id=b>{BOX(20)}</td></tr><tr><td id=c colspan=2>{BOX(200)}</td></tr></table>", BARE),
    case("cw_colspan_three", f"<table id=t><tr><td id=a>{BOX(10)}</td><td id=b>{BOX(20)}</td><td id=c>{BOX(30)}</td></tr><tr><td id=d colspan=3>{BOX(120)}</td></tr></table>", BARE),
    case("cw_colspan_two_spans", f"<table id=t><tr><td id=a colspan=2>{BOX(100)}</td><td id=b>{BOX(10)}</td></tr><tr><td id=c>{BOX(10)}</td><td id=d colspan=2>{BOX(100)}</td></tr></table>", BARE),
    case("cw_colspan_percent_table", f"<table id=t style='width:400px'><tr><td id=a colspan=2>x</td></tr><tr><td id=b>{BOX(50)}</td><td id=c>{BOX(100)}</td></tr></table>", BARE),
    case("cw_col_width", "<table id=t><col style='width:120px'><tr><td id=a>x</td><td id=b>y</td></tr></table>", BARE),
    case("cw_col_percent", f"<table id=t style='width:400px'><col style='width:30%'><tr><td id=a>x</td><td id=b>{BOX(20)}</td></tr></table>", BARE),
    case("cw_col_vs_cell_content", f"<table id=t><col style='width:40px'><tr><td id=a>{BOX(100)}</td><td id=b>y</td></tr></table>", BARE),
    case("cw_cell_beats_col", "<table id=t><col style='width:40px'><tr><td id=a style='width:90px'>x</td><td id=b>y</td></tr></table>", BARE),
    case("cw_equal_auto", "<table id=t style='width:300px'><tr><td id=a>x</td><td id=b>y</td><td id=c>z</td></tr></table>", BARE),
    case("cw_wide_content_overflow", f"<div style='width:100px'><table id=t><tr><td id=a>{BOX(150)}</td><td id=b>{BOX(80)}</td></tr></table></div>", BARE),
    case("cw_long_word", "<div style='width:100px'><table id=t><tr><td id=a>unbreakableunbreakable</td><td id=b>a b c d e f</td></tr></table></div>", BARE),
    case("cw_padding_border", "<table id=t><tr><td id=a style='padding:5px 9px;border:2px solid'>x</td><td id=b style='padding:3px'>y</td></tr></table>", "table{border-spacing:0}"),
    case("cw_th_bold", "<table id=t><tr><th id=a>Name</th><th id=b>Value</th></tr><tr><td id=c>alpha</td><td id=d>1</td></tr></table>", BARE),
    case("cw_nowrap_long", f"<div style='width:150px'><table id=t><tr><td id=a style='white-space:nowrap'>{TXT}</td><td id=b>x</td></tr></table></div>", BARE),
    case("cw_min_width_cell", "<table id=t><tr><td id=a style='min-width:90px'>x</td><td id=b>y</td></tr></table>", BARE),
    case("cw_cell_max_width_ignored", f"<table id=t><tr><td id=a style='max-width:20px'>{TXT}</td><td id=b>y</td></tr></table>", BARE),
    case("cw_many_columns", "<table id=t><tr>" + "".join(f"<td id=c{i}>{'x'*(i+1)}</td>" for i in range(8)) + "</tr></table>", BARE),
    case("cw_many_columns_narrow", "<div style='width:120px'><table id=t><tr>" + "".join(f"<td id=c{i}>{'x'*(i+1)}</td>" for i in range(8)) + "</tr></table></div>", BARE),
    case("cw_mixed_rows_widths", f"<table id=t><tr><td id=a>{BOX(10)}</td><td id=b>{BOX(90)}</td></tr><tr><td id=c>{BOX(70)}</td><td id=d>{BOX(20)}</td></tr></table>", BARE),
    case("cw_spacing_counts", "<table id=t style='width:200px;border-spacing:10px'><tr><td id=a>x</td><td id=b>y</td></tr></table>", "td{padding:0}"),
]

TABLE_WIDTH = [
    case("tw_auto_shrinks", "<table id=t><tr><td id=a>hello</td></tr></table>"),
    case("tw_width_px", "<table id=t style='width:300px'><tr><td id=a>hello</td></tr></table>"),
    case("tw_width_100", "<table id=t style='width:100%'><tr><td id=a>hello</td><td id=b>x</td></tr></table>"),
    case("tw_width_less_than_min", f"<table id=t style='width:30px'><tr><td id=a>{BOX(80)}</td></tr></table>"),
    case("tw_content_box", "<table id=t style='box-sizing:content-box;width:300px;border:5px solid;padding:7px'><tr><td id=a>hello</td></tr></table>"),
    case("tw_border_box", "<table id=t style='width:300px;border:5px solid;padding:7px'><tr><td id=a>hello</td></tr></table>"),
    case("tw_min_width", "<table id=t style='min-width:250px'><tr><td id=a>hello</td></tr></table>"),
    case("tw_max_width", f"<table id=t style='max-width:100px;width:300px'><tr><td id=a>{BOX(40)}</td></tr></table>"),
    case("tw_max_width_below_min", f"<table id=t style='max-width:20px'><tr><td id=a>{BOX(80)}</td></tr></table>"),
    case("tw_fill_shrink_text", f"<div style='width:300px'><table id=t><tr><td id=a>{TXT} {TXT}</td></tr></table></div>"),
    case("tw_in_inline_block", "<div style='display:inline-block' id=i><table id=t><tr><td id=a>hello</td></tr></table></div>"),
    case("tw_in_float", f"<div id=f style='float:left'><table id=t><tr><td id=a>{TXT}</td></tr></table></div>"),
    case("tw_float_table", "<table id=t style='float:right'><tr><td id=a>hello</td></tr></table><p id=p>text</p>"),
    case("tw_float_table_left_percent", "<table id=t style='float:left;width:30%'><tr><td id=a>hello</td></tr></table>"),
    case("tw_abspos_table", "<div style='position:relative;width:400px;height:100px'><table id=t style='position:absolute;left:10px;top:10px'><tr><td id=a>hello</td></tr></table></div>"),
    case("tw_abspos_left_right", "<div style='position:relative;width:400px;height:100px'><table id=t style='position:absolute;left:10px;right:10px;top:0'><tr><td id=a>hello</td></tr></table></div>"),
    case("tw_beside_float", f"<div id=f style='float:left;width:100px;height:100px'></div><table id=t><tr><td id=a>{TXT}</td></tr></table>"),
    case("tw_beside_float_too_wide", f"<div id=f style='float:left;width:700px;height:50px'></div><table id=t><tr><td id=a>{BOX(200)}</td></tr></table>"),
    case("tw_percent_in_auto_parent", "<div id=p style='float:left'><table id=t style='width:50%'><tr><td id=a>hello</td></tr></table></div>"),
    case("tw_flex_item", "<div style='display:flex'><table id=t><tr><td id=a>hello</td></tr></table><div id=b>x</div></div>"),
    case("tw_flex_item_stretch", "<div style='display:flex;width:500px'><table id=t style='flex:1'><tr><td id=a>hello</td></tr></table></div>"),
    case("tw_grid_item", "<div style='display:grid;grid-template-columns:300px 1fr'><table id=t><tr><td id=a>hello</td></tr></table><div id=b>x</div></div>"),
    case("tw_inline_with_text", "text <table id=t style='display:inline-table'><tr><td id=a>cell</td></tr></table> more"),
    case("tw_inline_table_baseline", "<span id=s>base</span><table id=t style='display:inline-table'><tr><td id=a>cell</td></tr><tr><td id=b>two</td></tr></table>"),
    case("tw_margin_auto_shrink", "<table id=t style='margin:0 auto'><tr><td id=a>centered</td></tr></table>"),
    case("tw_margin_left_auto", "<table id=t style='margin-left:auto'><tr><td id=a>right</td></tr></table>"),
    case("tw_rtl_columns", "<div dir=rtl style='width:300px'><table id=t><tr><td id=a>one</td><td id=b>two</td></tr></table></div>"),
    case("tw_rtl_table_width", "<table id=t dir=rtl style='width:300px'><tr><td id=a>one</td><td id=b>two</td><td id=c>three</td></tr></table>"),
    case("tw_in_cell_percent", "<table id=o style='width:400px'><tr><td id=oa><table id=t style='width:100%'><tr><td id=a>x</td></tr></table></td></tr></table>"),
    case("tw_nested_shrink", "<table id=o><tr><td id=oa><table id=t><tr><td id=a>inner text</td></tr></table></td><td id=ob>side</td></tr></table>"),
    case("tw_height_attr", "<table id=t height=100><tr><td id=a>x</td></tr></table>"),
    case("tw_viewport_overflow", f"<table id=t><tr><td id=a>{BOX(900)}</td></tr></table>"),
    case("tw_body_padding", "<div style='padding:0 30px'><table id=t style='width:100%'><tr><td id=a>x</td></tr></table></div>"),
]

FIXED = [
    case("fx_basic", "<table id=t style='table-layout:fixed;width:300px'><tr><td id=a>one</td><td id=b>two</td><td id=c>three</td></tr></table>", BARE),
    case("fx_first_row_widths", "<table id=t style='table-layout:fixed;width:300px'><tr><td id=a style='width:50px'>x</td><td id=b>y</td></tr><tr><td id=c style='width:200px'>x</td><td id=d>y</td></tr></table>", BARE),
    case("fx_col_widths", "<table id=t style='table-layout:fixed;width:300px'><col style='width:100px'><col style='width:40px'><tr><td id=a>x</td><td id=b>y</td><td id=c>z</td></tr></table>", BARE),
    case("fx_auto_width_is_auto", f"<table id=t style='table-layout:fixed'><tr><td id=a>{BOX(40)}</td><td id=b>{BOX(80)}</td></tr></table>", BARE),
    case("fx_overflow_content", f"<table id=t style='table-layout:fixed;width:100px'><tr><td id=a>{BOX(150)}</td><td id=b>y</td></tr></table>", BARE),
    case("fx_percent", "<table id=t style='table-layout:fixed;width:400px'><tr><td id=a style='width:25%'>x</td><td id=b>y</td></tr></table>", BARE),
    case("fx_sum_exceeds", "<table id=t style='table-layout:fixed;width:100px'><tr><td id=a style='width:80px'>x</td><td id=b style='width:60px'>y</td></tr></table>", BARE),
    case("fx_proportional_growth", "<table id=t style='table-layout:fixed;width:330px'><tr><td id=a style='width:100px'>x</td><td id=b style='width:200px'>y</td></tr></table>", BARE),
    case("fx_colspan_first_row", "<table id=t style='table-layout:fixed;width:300px'><tr><td id=a colspan=2 style='width:120px'>x</td><td id=b>y</td></tr></table>", BARE),
    case("fx_spacing", "<table id=t style='table-layout:fixed;width:300px;border-spacing:10px'><tr><td id=a>x</td><td id=b>y</td></tr></table>", "td{padding:0}"),
    case("fx_border_box_cells", "<table id=t style='table-layout:fixed;width:300px;border-spacing:0'><tr><td id=a style='width:100px;padding:10px;border:2px solid'>x</td><td id=b>y</td></tr></table>"),
    case("fx_collapsed", "<table id=t style='table-layout:fixed;width:300px;border-collapse:collapse'><tr><td id=a style='width:50px;border:2px solid'>x</td><td id=b style='border:2px solid'>y</td></tr></table>", "td{padding:0}"),
    case("fx_text_wraps", f"<table id=t style='table-layout:fixed;width:200px'><tr><td id=a>{TXT}</td><td id=b>{TXT}</td></tr></table>", BARE),
    case("fx_content_box_table", "<table id=t style='table-layout:fixed;box-sizing:content-box;width:300px;border:4px solid;border-spacing:10px 0'><tr><td id=a style='width:50px'>x</td><td id=b>y</td></tr></table>", "td{padding:0}"),
    case("fx_min_content_width_property", f"<table id=t style='table-layout:fixed;width:min-content'><tr><td id=a>{BOX(40)}</td></tr></table>", BARE),
]

ROWS = [
    case("rh_cell_height", "<table id=t><tr id=r><td id=a style='height:60px'>x</td><td id=b>y</td></tr></table>", BARE),
    case("rh_cell_height_less_than_content", "<table id=t><tr id=r><td id=a style='height:5px'>x</td></tr></table>", BARE),
    case("rh_cell_padding_height", "<table id=t><tr id=r><td id=a style='height:40px;padding:6px'>x</td></tr></table>", "table{border-spacing:0}"),
    case("rh_cell_height_border_box", "<table id=t><tr id=r><td id=a style='height:40px;padding:6px;box-sizing:border-box'>x</td></tr></table>", "table{border-spacing:0}"),
    case("rh_row_height", "<table id=t><tr id=r style='height:50px'><td id=a>x</td></tr></table>", BARE),
    case("rh_row_height_two_rows", "<table id=t><tr id=r style='height:50px'><td id=a>x</td></tr><tr id=r2><td id=b>y</td></tr></table>", BARE),
    case("rh_tall_content", f"<table id=t><tr id=r><td id=a>{BOX(20, 70)}</td><td id=b>y</td></tr></table>", BARE),
    case("rh_wrapped_text", f"<table id=t style='width:100px'><tr id=r><td id=a>{TXT}</td><td id=b>y</td></tr></table>", BARE),
    case("rh_rowspan_excess", f"<table id=t><tr id=r1><td id=a rowspan=2>{BOX(20, 90)}</td><td id=b>x</td></tr><tr id=r2><td id=c>y</td></tr></table>", BARE),
    case("rh_rowspan_unequal_rows", f"<table id=t><tr id=r1><td id=a rowspan=2>{BOX(20, 100)}</td><td id=b>{BOX(10, 10)}</td></tr><tr id=r2><td id=c>{BOX(10, 30)}</td></tr></table>", BARE),
    case("rh_rowspan_fits", f"<table id=t><tr id=r1><td id=a rowspan=2>{BOX(20, 10)}</td><td id=b>{BOX(10, 30)}</td></tr><tr id=r2><td id=c>{BOX(10, 30)}</td></tr></table>", BARE),
    case("rh_rowspan_three", f"<table id=t><tr id=r1><td id=a rowspan=3>{BOX(20, 120)}</td><td id=b>x</td></tr><tr id=r2><td id=c>y</td></tr><tr id=r3><td id=d>z</td></tr></table>", BARE),
    case("rh_rowspan_spacing", f"<table id=t style='border-spacing:6px'><tr id=r1><td id=a rowspan=2>{BOX(20, 80)}</td><td id=b>x</td></tr><tr id=r2><td id=c>y</td></tr></table>", "td{padding:0}"),
    case("rh_valign_top", f"<table id=t><tr id=r><td id=a style='vertical-align:top'>x</td><td id=b>{BOX(20, 60)}</td></tr></table>", BARE),
    case("rh_valign_middle", f"<table id=t><tr id=r><td id=a style='vertical-align:middle'>{BOX(10, 10)}</td><td id=b>{BOX(20, 61)}</td></tr></table>", BARE),
    case("rh_valign_bottom", f"<table id=t><tr id=r><td id=a style='vertical-align:bottom'>{BOX(10, 10)}</td><td id=b>{BOX(20, 60)}</td></tr></table>", BARE),
    case("rh_valign_default_baseline", "<table id=t><tr id=r><td id=a style='vertical-align:baseline'>one<br>two</td><td id=b style='font-size:30px;vertical-align:baseline'>big</td></tr></table>", BARE),
    case("rh_baseline_align_text", "<table id=t><tr id=r><td id=a style='font-size:12px'>small</td><td id=b style='font-size:32px'>large</td><td id=c style='font-size:20px'>mid</td></tr></table>", BARE),
    case("rh_baseline_padding", "<table id=t><tr id=r><td id=a style='padding-top:10px'>x</td><td id=b>y</td></tr></table>", "td{padding-left:0}table{border-spacing:0}"),
    case("rh_baseline_vs_middle", "<table id=t><tr id=r><td id=a style='vertical-align:baseline'>x</td><td id=b style='vertical-align:middle;height:50px'>y</td></tr></table>", BARE),
    case("rh_row_valign_inherit", f"<table id=t><tr id=r style='vertical-align:bottom'><td id=a>x</td><td id=b>{BOX(20, 60)}</td></tr></table>", BARE),
    case("rh_tbody_valign_inherit", f"<table id=t><tbody id=g style='vertical-align:top'><tr id=r><td id=a>x</td><td id=b>{BOX(20, 60)}</td></tr></tbody></table>", BARE),
    case("rh_default_middle", f"<table id=t><tbody id=g><tr id=r><td id=a>{BOX(10, 10)}</td><td id=b>{BOX(20, 60)}</td></tr></tbody></table>", BARE),
    case("rh_table_height", "<table id=t style='height:200px'><tr id=r1><td id=a>x</td></tr><tr id=r2><td id=b>y</td></tr></table>", BARE),
    case("rh_table_height_unequal", f"<table id=t style='height:300px'><tr id=r1><td id=a>{BOX(10, 20)}</td></tr><tr id=r2><td id=b>{BOX(10, 40)}</td></tr></table>", BARE),
    case("rh_table_height_spacing", "<table id=t style='height:200px;border-spacing:8px'><tr id=r1><td id=a>x</td></tr><tr id=r2><td id=b>y</td></tr></table>", "td{padding:0}"),
    case("rh_table_height_less", f"<table id=t style='height:10px'><tr id=r1><td id=a>{BOX(10, 40)}</td></tr></table>", BARE),
    case("rh_table_min_height", "<table id=t style='min-height:150px'><tr id=r1><td id=a>x</td></tr></table>", BARE),
    case("rh_table_height_sections", f"<table id=t style='height:250px'><thead id=h><tr id=r1><td id=a>{BOX(10, 20)}</td></tr></thead><tbody id=b><tr id=r2><td id=c>{BOX(10, 40)}</td></tr></tbody></table>", BARE),
    case("rh_table_height_rowspan", "<table id=t style='height:200px'><tr id=r1><td id=a rowspan=2>x</td><td id=b>y</td></tr><tr id=r2><td id=c>z</td></tr></table>", BARE),
    case("rh_row_fixed_and_table_height", "<table id=t style='height:200px'><tr id=r1 style='height:30px'><td id=a>x</td></tr><tr id=r2><td id=b>y</td></tr></table>", BARE),
    case("rh_percent_cell_height_ignored", "<table id=t><tr id=r1><td id=a style='height:50%'>x</td></tr></table>", BARE),
    case("rh_spacing_vertical", "<table id=t style='border-spacing:3px 11px'><tr id=r1><td id=a>x</td></tr><tr id=r2><td id=b>y</td></tr></table>", "td{padding:0}"),
    case("rh_section_geometry", "<table id=t><thead id=h><tr id=r1><td id=a>h</td></tr></thead><tbody id=b><tr id=r2><td id=c>b</td></tr><tr id=r3><td id=d>b</td></tr></tbody><tfoot id=f><tr id=r4><td id=e>f</td></tr></tfoot></table>", BARE),
    case("rh_cell_with_block_children", "<table id=t><tr id=r><td id=a><p id=p style='margin:10px 0'>para</p></td></tr></table>", BARE),
    case("rh_cell_margin_collapse_contained", "<table id=t><tr id=r><td id=a><div id=d style='margin-top:20px'>x</div></td></tr></table>", BARE),
    case("rh_cell_float_contained", "<table id=t><tr id=r><td id=a><div style='float:left;width:20px;height:50px'></div>text</td></tr></table>", BARE),
    case("rh_empty_cell_height", "<table id=t><tr id=r><td id=a></td><td id=b>x</td></tr></table>", BARE),
    case("rh_cell_line_height", "<table id=t><tr id=r><td id=a style='line-height:40px'>x</td></tr></table>", BARE),
    case("rh_nested_table_height", "<table id=o><tr><td id=oa><table id=t><tr><td id=a>inner</td></tr><tr><td id=b>inner2</td></tr></table></td><td id=ob>x</td></tr></table>"),
]

BORDERS = [
    case("bc_basic_1px", "<table id=t><tr><td id=a>a</td><td id=b>bb</td></tr><tr><td id=c>c</td><td id=d>d</td></tr></table>", "table{border-collapse:collapse}td{border:1px solid;padding:0}"),
    case("bc_basic_3px", "<table id=t><tr><td id=a>a</td><td id=b>bb</td></tr><tr><td id=c>c</td><td id=d>d</td></tr></table>", "table{border-collapse:collapse}td{border:3px solid;padding:0}"),
    case("bc_table_border", "<table id=t><tr><td id=a>a</td><td id=b>bb</td></tr></table>", "table{border-collapse:collapse;border:6px solid red}td{border:2px solid;padding:0}"),
    case("bc_table_border_thinner", "<table id=t><tr><td id=a>a</td><td id=b>bb</td></tr></table>", "table{border-collapse:collapse;border:1px solid red}td{border:4px solid;padding:0}"),
    case("bc_padding", "<table id=t><tr><td id=a>a</td><td id=b>bb</td></tr></table>", "table{border-collapse:collapse}td{border:2px solid;padding:5px}"),
    case("bc_wider_wins", "<table id=t><tr><td id=a style='border-right:6px solid'>a</td><td id=b style='border-left:2px solid'>bb</td></tr></table>", "table{border-collapse:collapse}td{padding:0}"),
    case("bc_hidden_wins", "<table id=t><tr><td id=a style='border:5px solid'>a</td><td id=b style='border:5px hidden'>bb</td></tr></table>", "table{border-collapse:collapse}td{padding:0}"),
    case("bc_none_loses", "<table id=t><tr><td id=a style='border:5px none'>a</td><td id=b style='border:4px solid'>bb</td></tr></table>", "table{border-collapse:collapse}td{padding:0}"),
    case("bc_style_priority", "<table id=t><tr><td id=a style='border-right:4px dotted'>a</td><td id=b style='border-left:4px double'>bb</td></tr></table>", "table{border-collapse:collapse}td{padding:0}"),
    case("bc_row_border", "<table id=t><tr id=r style='border:4px solid'><td id=a>a</td><td id=b>bb</td></tr></table>", "table{border-collapse:collapse}td{padding:0}"),
    case("bc_row_vs_cell", "<table id=t><tr id=r style='border-bottom:6px solid'><td id=a style='border-bottom:2px solid'>a</td></tr><tr><td id=b>b</td></tr></table>", "table{border-collapse:collapse}td{padding:0}"),
    case("bc_rowgroup_border", "<table id=t><tbody id=g style='border:4px solid'><tr><td id=a>a</td></tr></tbody></table>", "table{border-collapse:collapse}td{padding:0}"),
    case("bc_col_border", "<table id=t><col id=c1 style='border-right:4px solid'><tr><td id=a>a</td><td id=b>b</td></tr></table>", "table{border-collapse:collapse}td{padding:0}"),
    case("bc_spacing_ignored", "<table id=t style='border-spacing:20px'><tr><td id=a>a</td><td id=b>b</td></tr></table>", "table{border-collapse:collapse}td{border:2px solid;padding:0}"),
    case("bc_colspan", "<table id=t><tr><td id=a colspan=2>wide</td></tr><tr><td id=b>1</td><td id=c>2</td></tr></table>", "table{border-collapse:collapse}td{border:2px solid;padding:0}"),
    case("bc_rowspan", "<table id=t><tr><td id=a rowspan=2>tall</td><td id=b>1</td></tr><tr><td id=c>2</td></tr></table>", "table{border-collapse:collapse}td{border:2px solid;padding:0}"),
    case("bc_width_fixed", "<table id=t style='width:300px'><tr><td id=a>a</td><td id=b>b</td></tr></table>", "table{border-collapse:collapse}td{border:2px solid;padding:0}"),
    case("bc_fractional", "<table id=t><tr><td id=a>a</td><td id=b>bb</td></tr></table>", "table{border-collapse:collapse}td{border:5px solid;padding:0}"),
    case("bc_table_margin", "<table id=t style='margin:10px'><tr><td id=a>a</td></tr></table>", "table{border-collapse:collapse}td{border:2px solid;padding:0}"),
    case("bc_border_attr", "<table id=t border=2 style='border-collapse:collapse'><tr><td id=a>a</td><td id=b>b</td></tr></table>", ""),
    case("bc_outer_only", "<table id=t><tr><td id=a>a</td><td id=b>b</td></tr></table>", "table{border-collapse:collapse;border:3px solid}td{padding:0}"),
    case("bc_header_row_thicker", "<table id=t><tr id=r1 style='border-bottom:5px solid'><td id=a>a</td></tr><tr><td id=b>b</td></tr></table>", "table{border-collapse:collapse}td{border:1px solid;padding:0}"),
    case("bc_inside_cell_content_width", "<table id=t style='width:200px'><tr><td id=a style='border:6px solid'>x</td><td id=b style='border:2px solid'>y</td></tr></table>", "table{border-collapse:collapse}td{padding:0}"),
    case("bc_auto_table_min_widths", "<table id=t><tr><td id=a style='border:6px solid'>xxxxxxxx</td><td id=b style='border:2px solid'>yyyy</td></tr></table>", "table{border-collapse:collapse}td{padding:0}"),
]

CAPTIONS = [
    case("cp_top", "<table id=t><caption id=c>Caption</caption><tr><td id=a>a</td><td id=b>bbbbbbbbbbbbbb</td></tr></table>"),
    case("cp_bottom", "<table id=t><caption id=c style='caption-side:bottom'>Caption</caption><tr><td id=a>a</td><td id=b>bbbbbbbbbbbbbb</td></tr></table>"),
    case("cp_wider_than_table", "<table id=t><caption id=c>A caption that is a good deal wider than the table body</caption><tr><td id=a>a</td></tr></table>"),
    case("cp_wraps_to_table", f"<table id=t><caption id=c>{TXT}</caption><tr><td id=a>{BOX(100)}</td></tr></table>"),
    case("cp_long_word", "<table id=t><caption id=c>Unbreakablecaptionword</caption><tr><td id=a>a</td></tr></table>"),
    case("cp_with_border", "<table id=t style='border:5px solid red'><caption id=c>Caption</caption><tr><td id=a>a</td></tr></table>"),
    case("cp_bottom_with_border", "<table id=t style='border:5px solid red'><caption id=c style='caption-side:bottom'>Caption</caption><tr><td id=a>a</td></tr></table>"),
    case("cp_margins", "<table id=t><caption id=c style='margin:7px 0 13px'>Caption</caption><tr><td id=a>a</td></tr></table>"),
    case("cp_two_captions", "<table id=t><caption id=c1>One</caption><caption id=c2 style='caption-side:bottom'>Two</caption><tr><td id=a>a</td></tr></table>"),
    case("cp_text_align", "<table id=t style='width:300px'><caption id=c style='text-align:left'>Caption</caption><tr><td id=a>a</td></tr></table>"),
    case("cp_default_centered", "<table id=t style='width:300px'><caption id=c>Caption</caption><tr><td id=a>a</td></tr></table>"),
    case("cp_in_float", "<div id=f style='float:left'><table id=t><caption id=c>A caption for the thumbnail that wraps</caption><tr><td id=a style='width:100px;padding:0'><div style='height:60px'></div></td></tr></table></div>"),
    case("cp_collapsed_table", "<table id=t style='border-collapse:collapse;border:4px solid'><caption id=c>Cap</caption><tr><td id=a style='border:2px solid'>a</td></tr></table>"),
    case("cp_table_height", "<table id=t style='height:200px'><caption id=c>Cap</caption><tr><td id=a>a</td></tr></table>"),
    case("cp_width_set", "<table id=t style='width:400px'><caption id=c>Caption</caption><tr><td id=a>a</td></tr></table>"),
    case("cp_div_caption_bottom", "<div id=t style='display:table'><div id=c style='display:table-caption;caption-side:bottom'>Cap</div><div style='display:table-row'><div id=a style='display:table-cell'>x</div></div></div>"),
    case("cp_caption_padding", "<table id=t><caption id=c style='padding:6px;border:2px solid'>Caption</caption><tr><td id=a>a</td></tr></table>"),
    case("cp_below_float", "<div style='float:left;width:50px;height:20px'></div><table id=t><caption id=c>Cap</caption><tr><td id=a>a</td></tr></table>"),
]

CONTEXT = [
    case("ct_table_in_cell", "<table id=o><tr><td id=oa><table id=t><tr><td id=a>inner</td></tr></table></td></tr></table>"),
    case("ct_relative_cell_abspos", "<table id=t><tr><td id=a style='position:relative;height:60px;width:100px'><div id=d style='position:absolute;right:0;bottom:0;width:10px;height:10px'></div></td></tr></table>"),
    case("ct_cell_overflow_hidden", "<table id=t><tr><td id=a style='overflow:hidden;width:50px;height:30px'><div style='width:200px'>x</div></td></tr></table>"),
    case("ct_table_in_flex_wrap", "<div style='display:flex;width:300px'><div id=f style='flex:1'><table id=t style='width:100%'><tr><td id=a>x</td></tr></table></div><div style='width:100px'></div></div>"),
    case("ct_table_after_text", "text before<table id=t><tr><td id=a>x</td></tr></table>text after"),
    case("ct_two_tables", "<table id=t1><tr><td id=a>x</td></tr></table><table id=t2><tr><td id=b>y</td></tr></table>"),
    case("ct_table_in_li", "<ul><li id=l><table id=t><tr><td id=a>x</td></tr></table></li></ul>"),
    case("ct_table_in_overflow", "<div id=d style='overflow:hidden;width:150px'><table id=t><tr><td id=a>some longer cell text</td></tr></table></div>"),
    case("ct_cell_inline_block", "<table id=t><tr><td id=a><span id=ib style='display:inline-block;width:40px;height:30px'></span></td></tr></table>"),
    case("ct_cell_inline_block_text", "<table id=t><tr><td id=a><div style='display:inline-block;width:40px;height:30px'></div> text</td></tr></table>"),
    case("ct_center_tag", "<center><table id=t><tr><td id=a>x</td></tr></table></center>"),
    case("ct_cell_text_align", "<table id=t style='width:200px'><tr><td id=a style='text-align:right'>r</td><td id=b style='text-align:center'>c</td></tr></table>", BARE),
    case("ct_cell_list", "<table id=t><tr><td id=a><ul style='margin:0;padding-left:20px'><li>one</li><li>two</li></ul></td></tr></table>"),
    case("ct_cell_pre", "<table id=t><tr><td id=a><div style='white-space:pre'>line1\nline two</div></td></tr></table>"),
    case("ct_cell_wbr", "<table id=t style='width:100px'><tr><td id=a>aaaa<wbr>bbbb<wbr>cccc<wbr>dddd</td></tr></table>"),
    case("ct_table_clear", "<div id=f style='float:left;width:50px;height:80px'></div><table id=t style='clear:left'><tr><td id=a>x</td></tr></table>"),
    case("ct_deep_nesting", "<table id=t1><tr><td id=a><table id=t2><tr><td id=b><table id=t3><tr><td id=c>deep</td></tr></table></td></tr></table></td></tr></table>"),
    case("ct_abspos_shrink_wrapper", "<div style='position:absolute;left:0;top:0' id=ab><table id=t><tr><td id=a>abs content</td></tr></table></div>"),
    case("ct_nested_percent_widths", "<table id=o style='width:300px'><tr><td id=oa style='width:50%'><table id=t style='width:100%'><tr><td id=a>x</td></tr></table></td><td id=ob>y</td></tr></table>"),
]

# Shapes taken from real pages (Wikipedia infoboxes, periodic table, cladograms,
# Hacker News layout tables) that each exposed a table or inline-sizing bug.
REAL = [
    case("rw_nbsp_label_min_content", "<div style='width:1px'><table id=t><tr><th id=a style='padding:0'>Official&nbsp;languages</th><td id=b style='padding:0'>English</td></tr></table></div>"),
    case("rw_nbsp_in_span_min_content", "<div style='width:1px'><table id=t><tr><th id=a style='padding:0'>Official<span>&nbsp;</span>languages</th><td id=b style='padding:0'>English</td></tr></table></div>"),
    case("rw_soft_hyphen_in_span_min_content", "<div style='width:1px'><table id=t><tr><td id=a style='padding:0'>Lawren<span>&shy;</span>cium</td></tr></table></div>"),
    case("rw_soft_hyphen_min_content", "<div style='width:1px'><table id=t><tr><td id=a style='padding:0'>Lawren&shy;cium</td><td id=b style='padding:0'>Scan&shy;dium</td></tr></table></div>"),
    case("rw_nowrap_block_overflow_wrap", "<table id=t style='table-layout:fixed;width:60px;border-spacing:0'><tr><td id=a style='border:2px solid;padding:0;overflow-wrap:break-word'><span style='white-space:nowrap'><a href='x'><span style='display:block'>Helium</span></a></span></td><td id=b style='overflow-wrap:break-word'>Hello</td></tr></table>"),
    case("rw_display_block_table", "<table id=t style='display:block;overflow:auto;width:100%;border-spacing:1px'><tbody id=g><tr id=r><td id=a>one</td><td id=b>two</td></tr></tbody></table>"),
    case("rw_float_table_in_display_table", "<div id=m style='display:table'><table id=t style='float:left;border:1px solid;padding:5px;margin:1px;border-collapse:collapse'><caption id=c>Caption</caption><tr><th id=h>nuclear</th></tr><tr><td id=a style='padding:10px'>body</td></tr></table></div>"),
    case("rw_flex_row_of_floated_items", "<div id=r style='display:flex;flex-wrap:wrap;justify-content:center;width:267px'><div id=a style='float:left;margin:1px;width:156px'><div style='height:157px'></div></div><div id=b style='float:left;margin:1px;width:107px'><div style='height:157px'></div></div></div>"),
    case("rw_infobox_multi_image_cell", "<table id=t style='float:right;width:200px;border-spacing:3px'><tr><th id=h colspan=2>Cat</th></tr><tr><td id=c colspan=2 style='text-align:center'><div id=mi style='display:flex;flex-direction:column;width:267px'><div id=r1 style='display:flex;flex-wrap:wrap;justify-content:center'><div id=s1 style='margin:1px;float:left;width:156px;height:157px'></div><div id=s2 style='margin:1px;float:left;width:107px;height:157px'></div></div><div id=r2 style='display:flex'><div>Various types</div></div></div></td></tr><tr><td id=k>Kingdom</td><td id=v>Animalia</td></tr></table>"),
    case("rw_hn_layout", "<center><table id=o border=0 cellpadding=0 cellspacing=0 width='85%' bgcolor='#f6f6ef'><tr><td id=hd bgcolor='#ff6600'><table id=i border=0 cellpadding=0 cellspacing=0 width='100%' style='padding:2px'><tr><td id=l style='width:18px;padding-right:4px'><div style='width:18px;height:18px;border:1px solid white'></div></td><td id=m style='line-height:12pt;height:10px'><span><b>Hacker News</b> new | past | comments | ask | show</span></td><td id=r style='text-align:right;padding-right:4px'>login</td></tr></table></td></tr><tr id=sp style='height:10px'></tr><tr><td id=bd><table id=list border=0 cellpadding=0 cellspacing=0><tr><td id=n align=right valign=top>1.</td><td id=v valign=top>&#9650;</td><td id=tt class=title>A story title that is fairly long</td></tr><tr><td colspan=2></td><td id=sub>10 points by someone 2 hours ago</td></tr></table></td></tr></table></center>"),
    case("rw_nested_percent_colspan", "<table id=o style='width:300px;border-spacing:0'><tr><td id=oa colspan=2 style='padding:0'><table id=t style='width:100%;border-spacing:0'><tr><td id=a style='width:30%'>a</td><td id=b>b</td></tr></table></td></tr><tr><td id=c>c</td><td id=d>dd</td></tr></table>"),
    case("rw_sortable_header_widths", "<table id=t style='width:100%;border-collapse:collapse'><tr><th id=a style='width:5%'>#</th><th id=b>Country</th><th id=c style='width:15%'>Population</th><th id=d style='width:10%'>%</th></tr><tr><td id=e>1</td><td id=f>India</td><td id=g>1,428,627,663</td><td id=h>17.7%</td></tr></table>"),
    case("rw_rowspan_with_tall_cell_and_borders", "<table id=t style='border-collapse:collapse'><tr><td id=a rowspan=3 style='border:1px solid;padding:4px'>tall<br>cell<br>text</td><td id=b style='border:1px solid;padding:4px'>1</td></tr><tr><td id=c style='border:1px solid;padding:4px'>2</td></tr><tr><td id=d style='border:1px solid;padding:4px'>3</td></tr></table>"),
]

ALL = STRUCTURE + ANON + COLUMNS + TABLE_WIDTH + FIXED + ROWS + BORDERS + CAPTIONS + CONTEXT + REAL
