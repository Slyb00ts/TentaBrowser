"""Cases for `record_script_api.py`: script-visible geometry and getComputedStyle.

Every case is (name, (viewport width, height), html). The html defines
`collect()` returning a JSON-serialisable object; the recorder stores what
Chrome returns and `crates/tenta-browser/tests/script_api_chrome.rs` compares
this engine's answers with it. Text uses 'Liberation Sans' (the recorder
declares the face) so advances match the engine's bundled font.
"""

PRELUDE = """
function R(id){var r=document.getElementById(id).getBoundingClientRect();return [r.x,r.y,r.width,r.height];}
function M(id){var e=document.getElementById(id);return [e.offsetLeft,e.offsetTop,e.offsetWidth,e.offsetHeight,
  e.clientLeft,e.clientTop,e.clientWidth,e.clientHeight,e.scrollWidth,e.scrollHeight,e.offsetParent?(e.offsetParent.id||e.offsetParent.localName):null];}
function CS(id,props,pseudo){var cs=getComputedStyle(document.getElementById(id),pseudo||null);var o={};props.forEach(function(p){o[p]=cs.getPropertyValue(p);});return o;}
"""

def page(style, body, script, doctype=True):
    dt = "<!doctype html>" if doctype else ""
    return (f"{dt}<html><head><style>html,body{{margin:0}}body{{font:16px/20px 'Liberation Sans'}}{style}</style></head>"
            f"<body>{body}<script>{PRELUDE}{script}</script></body></html>")

GEOMETRY = [
 ("g-block-box", (500, 300), page(
   "#a{width:120px;height:30px;margin:10px 0 0 20px;padding:5px;border:2px solid red}",
   "<div id=a>box</div>",
   "function collect(){return {r:R('a'),m:M('a')};}")),
 ("g-offset-parent-chain", (500, 300), page(
   "#p{position:relative;left:30px;top:40px;width:200px;height:100px;border:3px solid #000;padding:7px}"
   "#c{position:absolute;left:11px;top:13px;width:50px;height:20px}#s{margin:5px}",
   "<div id=p><div id=s><div id=c></div></div></div>",
   "function collect(){return {p:R('p'),c:R('c'),mp:M('p'),mc:M('c'),ms:M('s')};}")),
 ("g-inline-wrapped", (500, 300), page(
   "#w{width:100px}#s{background:#ccc}",
   "<div id=w>aaa bbb <span id=s>ccc ddd eee fff ggg</span> hhh</div>",
   "function collect(){var s=document.getElementById('s');var rs=s.getClientRects();"
   "return {n:rs.length,h:[].map.call(rs,function(r){return r.height;}),y:[].map.call(rs,function(r){return r.y;}),w:R('w')};}")),
 ("g-lines-height", (500, 300), page(
   "#p{width:90px;line-height:20px;margin:0}",
   "<p id=p>one two three four five six seven</p>",
   "function collect(){return {r:R('p'),m:M('p')};}")),
 ("g-transform", (500, 300), page(
   "#t{width:100px;height:50px;transform:translate(10px,20px) scale(2);margin:30px}",
   "<div id=t></div>",
   "function collect(){return {r:R('t'),m:M('t'),n:document.getElementById('t').getClientRects().length};}")),
 ("g-fixed", (500, 300), page(
   "#f{position:fixed;left:15px;top:25px;width:60px;height:70px}",
   "<div id=f></div><div style='height:900px'></div>",
   "function collect(){return {r:R('f'),m:M('f')};}")),
 ("g-display-none", (500, 300), page(
   "#n{display:none;width:50px;height:50px}",
   "<div id=n><span id=k>x</span></div>",
   "function collect(){return {r:R('n'),m:M('n'),k:R('k'),n:document.getElementById('n').getClientRects().length};}")),
 ("g-display-contents", (500, 300), page(
   "#c{display:contents}#d{width:30px;height:30px}",
   "<div id=c><div id=d></div></div>",
   "function collect(){return {c:R('c'),n:document.getElementById('c').getClientRects().length,d:R('d')};}")),
 ("g-visibility-hidden", (500, 300), page(
   "#v{visibility:hidden;width:40px;height:40px;margin:5px}",
   "<div id=v></div>",
   "function collect(){return {r:R('v'),m:M('v')};}")),
 ("g-overflow-scroll", (500, 300), page(
   "#o{width:100px;height:60px;overflow:scroll;border:2px solid #000}#i{width:300px;height:200px}",
   "<div id=o><div id=i></div></div>",
   "function collect(){return {m:M('o'),i:R('i')};}")),
 ("g-overflow-hidden", (500, 300), page(
   "#o{width:100px;height:60px;overflow:hidden;padding:4px}#i{width:250px;height:90px}",
   "<div id=o><div id=i></div></div>",
   "function collect(){return {m:M('o')};}")),
 ("g-flex-row", (500, 300), page(
   "#f{display:flex;width:300px;height:80px}#a{flex:1}#b{flex:2}#c{width:50px}",
   "<div id=f><div id=a></div><div id=b></div><div id=c></div></div>",
   "function collect(){return {a:R('a'),b:R('b'),c:R('c'),mb:M('b')};}")),
 ("g-grid", (500, 300), page(
   "#g{display:grid;grid-template-columns:100px 1fr;grid-gap:10px;width:300px}#a{height:20px}#b{height:30px}",
   "<div id=g><div id=a></div><div id=b></div></div>",
   "function collect(){return {a:R('a'),b:R('b'),g:R('g')};}")),
 ("g-float", (500, 300), page(
   "#f{float:left;width:80px;height:50px;margin:5px}#t{height:100px}",
   "<div id=f></div><div id=t></div>",
   "function collect(){return {f:R('f'),mf:M('f'),t:R('t')};}")),
 ("g-table", (500, 300), page(
   "table{border-collapse:separate;border-spacing:2px}td{width:40px;height:20px;padding:3px}",
   "<table id=tb><tr><td id=a></td><td id=b></td></tr></table>",
   "function collect(){return {a:R('a'),b:R('b'),tb:R('tb'),ma:M('a'),mtb:M('tb')};}")),
 ("g-body-margin", (500, 300), page(
   "html,body{margin:8px}#a{height:50px}",
   "<div id=a></div>",
   "function collect(){var b=document.body,d=document.documentElement;return {a:R('a'),b:[b.offsetTop,b.offsetLeft,b.offsetWidth,b.offsetHeight,b.clientWidth,b.clientHeight,b.scrollHeight],"
   "d:[d.clientWidth,d.clientHeight,d.scrollWidth,d.scrollHeight,d.offsetWidth,d.offsetHeight],se:document.scrollingElement.localName,ma:M('a')};}")),
 ("g-tall-document", (500, 300), page(
   "#t{height:900px;width:700px}",
   "<div id=t></div>",
   "function collect(){var d=document.documentElement,b=document.body;return {d:[d.clientWidth,d.clientHeight,d.scrollWidth,d.scrollHeight],b:[b.clientWidth,b.clientHeight,b.scrollWidth,b.scrollHeight],t:R('t')};}")),
 ("g-quirks-body", (500, 300), page(
   "#t{height:900px}",
   "<div id=t></div>",
   "function collect(){var d=document.documentElement,b=document.body;return {d:[d.clientWidth,d.clientHeight,d.scrollHeight],b:[b.clientWidth,b.clientHeight,b.scrollHeight],se:document.scrollingElement.localName,m:document.compatMode};}",
   doctype=False)),
 ("g-image-sized", (500, 300), page(
   "",
   "<img id=i width=40 height=30 style='border:2px solid #000;padding:1px'>",
   "function collect(){return {r:R('i'),m:M('i')};}")),
 ("g-input", (500, 300), page(
   "input{font:16px 'Liberation Sans'}",
   "<input id=t type=text style='width:100px;height:24px;box-sizing:border-box'>",
   "function collect(){return {r:R('t'),m:M('t')};}")),
 ("g-relative-offset", (500, 300), page(
   "#r{position:relative;left:12px;top:7px;width:50px;height:50px;margin-left:20px}",
   "<div id=r></div>",
   "function collect(){return {r:R('r'),m:M('r')};}")),
 ("g-empty-block", (500, 300), page(
   "#e{}",
   "<div id=e></div><div id=f style='width:10px'></div>",
   "function collect(){return {e:R('e'),f:R('f'),me:M('e')};}")),
 ("g-fractional", (500, 300), page(
   ".c{width:100px}#a{width:33.333%;height:10.5px}",
   "<div class=c><div id=a></div></div>",
   "function collect(){return {a:R('a'),m:M('a')};}")),
 ("g-border-box", (500, 300), page(
   "#a{box-sizing:border-box;width:100px;height:60px;padding:10px;border:5px solid #000}",
   "<div id=a></div>",
   "function collect(){return {a:R('a'),m:M('a')};}")),
 ("g-inline-block", (500, 300), page(
   "#a,#b{display:inline-block;width:60px;height:30px;margin:0 4px;vertical-align:top}",
   "<div><span id=a></span><span id=b></span></div>",
   "function collect(){return {a:R('a'),b:R('b'),ma:M('a')};}")),
 ("g-element-from-point", (500, 300), page(
   "#a{position:absolute;left:10px;top:10px;width:100px;height:100px;background:#f00}"
   "#b{position:absolute;left:50px;top:50px;width:100px;height:100px;background:#0f0;z-index:2}#c{pointer-events:none;position:absolute;left:0;top:200px;width:50px;height:50px}",
   "<div id=a></div><div id=b></div><div id=c>t</div>",
   "function id(e){return e?(e.id||e.localName):null;}"
   "function collect(){var f=function(x,y){return id(document.elementFromPoint(x,y));};"
   "return {a:f(20,20),ab:f(60,60),b:f(140,140),none:f(300,250),out:f(-1,5),out2:f(400,5),out3:f(5,300),c:f(10,210),bg:f(250,100)};}")),
 ("g-detached", (500, 300), page(
   "",
   "<div id=a></div>",
   "function collect(){var e=document.createElement('div');var r=e.getBoundingClientRect();"
   "return {r:[r.x,r.y,r.width,r.height],o:[e.offsetWidth,e.offsetHeight,e.clientWidth,e.scrollHeight],p:e.offsetParent,n:e.getClientRects().length};}")),
 ("g-window-metrics", (500, 300), page(
   "",
   "<div id=a></div>",
   "function collect(){return {iw:innerWidth,ih:innerHeight,dpr:devicePixelRatio,ow:outerWidth>0,sx:scrollX,sy:scrollY};}")),
 ("g-mutation", (500, 300), page(
   "#a{width:50px;height:20px}",
   "<div id=a></div><div id=b style='height:10px'></div>",
   "function collect(){var a=document.getElementById('a');var o=[a.offsetHeight,R('b')[1]];"
   "a.style.height='60px';var o2=[a.offsetHeight,R('b')[1]];"
   "a.className='x';var st=document.createElement('style');st.textContent='.x{height:90px}';document.head.appendChild(st);var o3=[a.offsetHeight,R('b')[1]];"
   "a.parentNode.removeChild(a);var o4=[R('b')[1]];return {o:o,o2:o2,o3:o3,o4:o4};}")),
 ("g-abs-percent", (500, 300), page(
   "#p{position:relative;width:200px;height:100px;margin:10px}#c{position:absolute;left:10%;top:20%;width:50%;height:30%}",
   "<div id=p><div id=c></div></div>",
   "function collect(){return {c:R('c'),mc:M('c')};}")),
 ("g-min-max", (500, 300), page(
   "#a{width:10px;min-width:60px;height:200px;max-height:40px}",
   "<div id=a></div>",
   "function collect(){return {a:R('a')};}")),
 ("g-scroll-offsets", (500, 300), page(
   "#o{width:50px;height:50px;overflow:auto}#i{height:300px}",
   "<div id=o><div id=i></div></div>",
   "function collect(){var o=document.getElementById('o');o.scrollTop=20;return {t:o.scrollTop,l:o.scrollLeft,sh:o.scrollHeight};}")),
 ("g-rotate-bounding", (500, 300), page(
   "#t{width:100px;height:40px;transform:rotate(90deg);margin:60px}",
   "<div id=t></div>",
   "function collect(){return {r:R('t'),m:M('t')};}")),
 ("g-list-item", (500, 300), page(
   "ul{margin:0;padding-left:40px}li{height:20px}",
   "<ul id=u><li id=a>a</li><li id=b>b</li></ul>",
   "function collect(){return {u:R('u'),a:R('a'),b:R('b'),ma:M('a')};}")),
 ("g-rect-api", (500, 300), page(
   "",
   "<div id=a></div>",
   "function collect(){var r=new DOMRect(1,2,-30,40);var ro=new DOMRectReadOnly(5,6,7,8);var f=DOMRect.fromRect({x:3,width:9});"
   "return {r:[r.x,r.y,r.width,r.height,r.top,r.right,r.bottom,r.left],ro:[ro.x,ro.right,ro.bottom],f:[f.x,f.y,f.width,f.height],j:JSON.stringify(f),"
   "i:[r instanceof DOMRect,r instanceof DOMRectReadOnly,ro instanceof DOMRect,Object.prototype.toString.call(r)]};}")),
]

GEOMETRY += [
 ("g-root-border-origin", (500, 300), page(
   "html{margin:5px;padding:2px;border:1px solid #000}body{margin:11px;padding:4px;border:3px solid #00f}#a{margin:3px 0 0 6px;height:20px}",
   "<div id=a></div>",
   "function collect(){var b=document.body;return {a:R('a'),ma:M('a'),mb:[b.offsetLeft,b.offsetTop,b.offsetWidth,b.clientLeft,b.clientTop]};}")),
 ("g-positioned-body", (500, 300), page(
   "html{margin:5px}body{margin:11px;position:relative}#a{margin:3px 0 0 6px;height:20px}",
   "<div id=a></div>",
   "function collect(){var b=document.body;return {a:R('a'),ma:M('a'),mb:[b.offsetLeft,b.offsetTop]};}")),
 ("g-fixed-table-scroller", (500, 300), page(
   "html{margin:5px;padding:2px;border:1px solid #000}body{margin:11px}#f{position:fixed;left:7px;top:9px;width:10px;height:10px}"
   "table{margin:4px;border:2px solid #000;border-spacing:3px}td{padding:1px;width:30px;height:10px}",
   "<div id=f></div><table id=t><tr><td id=a></td></tr></table><div id=s style='width:50px;height:20px;overflow:auto;padding:3px'><div id=i style='width:30px;height:10px'></div></div>",
   "function collect(){var d=document.documentElement;return {html:[d.offsetLeft,d.offsetTop,d.offsetWidth,d.offsetHeight,d.clientLeft,d.clientTop,d.clientWidth,d.clientHeight],f:M('f'),a:M('a'),ra:R('a'),t:M('t'),s:M('s'),rt:R('t'),si:M('i')};}")),
]

GEOMETRY += [
 ("g-pseudo-block", (500, 300), page(
   "#a{width:200px;padding:3px}#a::before{content:'';display:block;height:7px;margin:2px}#a::after{content:'';display:block;height:9px}",
   "<div id=a>text</div>",
   "function collect(){return {r:R('a'),m:M('a'),n:document.getElementById('a').getClientRects().length,"
   "cs:CS('a',['width','height']),b:CS('a',['width','height','display'],'::before')};}")),
 ("g-pseudo-inline-icon", (500, 300), page(
   "#a::before{content:'';display:inline-block;width:7px;height:7px}#a{padding:0 5px}",
   "<div><span id=a>word</span></div>",
   "function collect(){return {n:document.getElementById('a').getClientRects().length,m:M('a'),r:R('a')};}")),
 ("g-pseudo-clearfix", (500, 300), page(
   "#a::after{content:'';display:table;clear:both}#f{float:left;width:50px;height:40px}#a{width:300px}",
   "<div id=a><div id=f></div></div>",
   "function collect(){return {r:R('a'),m:M('a'),n:document.getElementById('a').getClientRects().length,cs:CS('a',['width','height'])};}")),
 ("g-pseudo-absolute", (500, 300), page(
   "#a{position:relative;width:120px;height:30px}#a::after{content:'';position:absolute;left:5px;top:6px;width:10px;height:11px}",
   "<div id=a></div>",
   "function collect(){return {r:R('a'),m:M('a'),n:document.getElementById('a').getClientRects().length,sw:document.getElementById('a').scrollWidth};}")),
]

STYLE_PROPS = [
 "color","background-color","display","position","width","height","margin-top","margin-left","padding-top","padding-right",
 "border-top-width","border-top-style","border-top-color","border-left-width","font-size","font-weight","font-family","font-style",
 "line-height","text-align","text-decoration-line","opacity","overflow-x","z-index","float","box-sizing","visibility","cursor",
 "flex-grow","flex-direction","transform","white-space","letter-spacing","text-transform","vertical-align","border-top-left-radius",
 "top","left","min-width","max-width","content","list-style-type","outline-style","text-indent","word-spacing","pointer-events",
]

STYLE = [
 ("s-box-all", (500, 300), page(
   "#a{color:rgb(10,20,30);background-color:#abcdef;width:120px;height:30px;margin:10px 5px 0 20px;padding:3px 4px 5px 6px;"
   "border:2px dashed rgba(255,0,0,.5);font-size:14px;font-weight:bold;font-style:italic;font-family:Arial,sans-serif;line-height:1.5;"
   "text-align:center;text-decoration:underline;opacity:.5;overflow-x:hidden;z-index:3;position:relative;float:none;box-sizing:border-box;"
   "visibility:hidden;cursor:pointer;flex-grow:2;transform:rotate(10deg);white-space:nowrap;letter-spacing:1px;text-transform:uppercase;"
   "vertical-align:middle;border-radius:4px 8px;top:5px;left:-3px;min-width:10%;max-width:300px;list-style-type:square;outline:1px solid red;"
   "text-indent:10px;word-spacing:2px;pointer-events:none}",
   "<div id=a>x</div>",
   "var P=" + repr(STYLE_PROPS).replace("'", '"') + ";function collect(){return CS('a',P);}")),
 ("s-defaults", (500, 300), page(
   "",
   "<div id=a>x</div>",
   "var P=" + repr(STYLE_PROPS).replace("'", '"') + ";function collect(){return CS('a',P);}")),
 ("s-inherited-and-inline", (500, 300), page(
   "#p{color:green;font:italic 20px/30px serif;text-align:right}",
   "<div id=p><span id=a style='color:rgb(0 0 255 / 0.5)'>x</span></div>",
   "var P=" + repr(STYLE_PROPS).replace("'", '"') + ";function collect(){return {a:CS('a',P),p:CS('p',P)};}")),
 ("s-pseudo-and-custom", (500, 300), page(
   "#a{--main:  12px ;--empty:;color:red}#a::before{content:'>';color:blue;display:inline-block;width:10px}#a::after{content:attr(data-x)}",
   "<div id=a data-x=tail>x</div>",
   "function collect(){return {main:CS('a',['--main','--empty','--missing']),before:CS('a',['content','color','display','width'],'::before'),"
   "after:CS('a',['content'],'::after'),bad:CS('a',['color'],'::nonsense'),len:getComputedStyle(document.getElementById('a')).length>100,"
   "text:getComputedStyle(document.getElementById('a')).cssText}; }")),
 ("s-layout-dependent", (500, 300), page(
   "#p{width:200px;height:100px;position:relative}#a{width:50%;height:20px;margin:0 auto;padding:1em 10%;"
   "position:absolute;left:10px;bottom:5px}#b{display:inline;width:50px;height:50px;margin:3px}#n{display:none;width:33%}"
   "#r{position:relative;top:10%;left:-5px}#t{transform-origin:left top;width:80px;height:40px}#u{width:80px;height:40px;transform-origin:50% 25%}",
   "<div id=p><div id=a>x</div><span id=b>i</span><div id=n></div><div id=r></div><div id=t></div><div id=u></div></div>",
   "var W=['width','height','margin-left','margin-right','margin-top','padding-left','padding-top','top','left','bottom','right','display','transform-origin'];"
   "function collect(){return {a:CS('a',W),b:CS('b',W),n:CS('n',W),r:CS('r',W),t:CS('t',W),u:CS('u',W)};}")),
 ("s-property-access", (500, 300), page(
   "#a{background-color:red;margin-top:7px}",
   "<div id=a></div>",
   "function collect(){var cs=getComputedStyle(document.getElementById('a'));var err=null;try{cs.color='red';}catch(e){err=e.name;}"
   "var e2=null;try{cs.setProperty('color','red');}catch(e){e2=e.name;}var e3=null;try{cs.removeProperty('color');}catch(e){e3=e.name;}"
   "return {camel:cs.backgroundColor,dashed:cs['background-color'],gp:cs.getPropertyValue('BACKGROUND-COLOR'),mt:cs.marginTop,"
   "prio:cs.getPropertyPriority('margin-top'),float:cs.cssFloat,item0:typeof cs.item(0),itemOob:cs.item(100000),pr:cs.parentRule,"
   "setErr:e2,rmErr:e3,same:getComputedStyle(document.getElementById('a'))===cs,proto:Object.prototype.toString.call(cs),unknown:cs.getPropertyValue('nope'),"
   "lenOK:cs.length>50,live:(function(){var a=document.getElementById('a');var c=getComputedStyle(a);var before=c.marginTop;a.style.marginTop='9px';return [before,c.marginTop];})()};}")),
]
