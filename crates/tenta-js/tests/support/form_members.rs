// Included by V8 and QuickJS adapters: the members of tables, selects,
// inputs, labels and the small form-associated elements (FM2).

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::Document;

/// `text` as a JavaScript string literal.
fn quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

fn page(body: &str) -> impl FnMut(&str) -> String {
    let mut document = Document::new();
    document.set_url(Some(
        "https://example.com/app/page.html".parse().expect("test URL"),
    ));
    let document = Rc::new(RefCell::new(document));
    // QuickJS evaluates through `&self`, V8 through `&mut self`.
    #[allow(unused_mut)]
    let mut engine = new_engine(document);
    engine
        .evaluate(&format!(
            "var html = document.createElement('html'); document.appendChild(html); \
             var body = document.createElement('body'); html.appendChild(body); \
             body.innerHTML = {}; \
             var $ = function (id) {{ return document.getElementById(id); }}; \
             var tags = function (list) {{ return Array.prototype.map.call(list, \
               function (el) {{ return el.id || el.localName; }}).join(','); }}; \
             var attempt = function (f) {{ try {{ return String(f()); }} \
               catch (e) {{ return e.name; }} }}; 'ready'",
            quoted(body)
        ))
        .expect("set up the page");
    move |source: &str| {
        engine
            .evaluate(source)
            .unwrap_or_else(|error| panic!("{error:?} in {source}"))
    }
}

#[test]
fn a_table_lists_and_inserts_its_rows_and_sections() {
    let mut eval = page(
        "<table id=t><caption id=cap>C</caption>\
         <tfoot id=foot><tr id=f1></tr></tfoot>\
         <tbody id=b1><tr id=r1><td id=c1></td><th id=c2></th></tr></tbody>\
         <thead id=head><tr id=h1></tr></thead>\
         <tbody id=b2><tr id=r2></tr></tbody></table>",
    );
    assert_eq!(
        eval(
            "var t = $('t');              [tags(t.rows), tags(t.tBodies), t.caption.id, t.tHead.id, t.tFoot.id, \
              $('r1').rowIndex, $('r2').rowIndex, $('f1').rowIndex, $('r2').sectionRowIndex, \
              tags($('r1').cells), $('c2').cellIndex, tags($('b1').rows)].join('|')"
        ),
        "h1,r1,r2,f1|b1,b2|cap|head|foot|1|2|3|0|c1,c2|1|r1"
    );
    assert_eq!(
        eval(
            "var t = $('t'); \
             var last = t.insertRow(); last.id = 'new-last'; \
             var first = t.insertRow(0); first.id = 'new-first'; \
             var inBody = $('b1').insertRow(0); inBody.id = 'b1-first'; \
             var refused = [attempt(function () { t.insertRow(99); }), \
               attempt(function () { t.insertRow(-2); }), \
               attempt(function () { t.deleteRow(99); })].join(','); \
             var lastParent = last.parentNode.id; t.deleteRow(-1); \
             [tags(t.rows), first.parentNode.id, lastParent, last.parentNode, refused].join('|')"
        ),
        "new-first,h1,b1-first,r1,r2,f1|head|foot||IndexSizeError,IndexSizeError,IndexSizeError"
    );
    assert_eq!(
        eval(
            "var row = $('r1'); \
             var cell = row.insertCell(1); cell.id = 'mid'; row.insertCell().id = 'end'; \
             row.deleteCell(0); \
             [tags(row.cells), cell.localName, attempt(function () { row.insertCell(9); })] \
               .join('|')"
        ),
        "mid,c2,end|td|IndexSizeError"
    );
}

#[test]
fn an_empty_table_grows_its_parts() {
    let mut eval = page("<table id=t><colgroup id=cg></colgroup></table>");
    assert_eq!(
        eval(
            "var t = $('t'); \
             var row = t.insertRow(); \
             var head = t.createTHead(), again = t.createTHead(); \
             var foot = t.createTFoot(), caption = t.createCaption(); \
             var body = t.createTBody(); body.id = 'second'; \
             var shape = tags(t.children); \
             t.deleteCaption(); t.deleteTHead(); t.deleteTFoot(); \
             [shape, head === again, row.parentNode.localName, tags(t.children), \
              t.caption, t.tHead].join('|')"
        ),
        "caption,cg,thead,tbody,second,tfoot|true|tbody|cg,tbody,second||"
    );
}

#[test]
fn table_cells_reflect_their_spans() {
    let mut eval = page("<table><tr><td id=c colspan=0 rowspan=70000 scope=ROW></td></tr></table>");
    assert_eq!(
        eval(
            "var c = $('c'); var read = [c.colSpan, c.rowSpan, c.scope].join(','); \
             c.colSpan = 3; c.abbr = 'x'; c.headers = 'h1 h2'; \
             [read, c.getAttribute('colspan'), c.abbr, c.headers, \
              document.createElement('td').cellIndex].join('|')"
        ),
        "1,65534,row|3|x|h1 h2|-1"
    );
}

#[test]
fn a_select_adds_and_finds_its_options() {
    let mut eval = page(
        "<label id=l1 for=s>Pick</label>\
         <select id=s><option id=a>A</option><optgroup id=g><option id=b name=bee>B</option>\
         </optgroup></select><label id=l2><select id=inner></select></label>",
    );
    assert_eq!(
        eval(
            "var s = $('s'); \
             var x = document.createElement('option'); x.id = 'x'; s.add(x); \
             var y = document.createElement('option'); y.id = 'y'; s.add(y, 0); \
             var z = document.createElement('option'); z.id = 'z'; s.add(z, $('b')); \
             var w = document.createElement('option'); w.id = 'w'; s.add(w, 99); \
             var refused = [attempt(function () { s.add(document.createElement('div')); }), \
               attempt(function () { s.add(document.createElement('option'), \
                 document.createElement('option')); })].join(','); \
             [tags(s.options), $('z').parentNode.id, s.item(1).id, s.item(9), \
              s.namedItem('bee').id, s.namedItem('x').id, refused, tags(s.labels), \
              tags($('inner').labels)].join('|')"
        ),
        "y,a,z,b,x,w|g|a||b|x|TypeError,NotFoundError|l1|l2"
    );
}

#[test]
fn an_input_has_its_value_as_a_number_and_steps() {
    let mut eval = page(
        "<input id=n type=number min=0 max=10 step=2 value=3>\
         <input id=d type=date value=2024-02-29><input id=t type=text value=7>\
         <input id=tm type=time value=01:30><input id=any type=number step=any>\
         <label id=lab for=n>N</label><label id=wrap><input id=w></label>",
    );
    assert_eq!(
        eval(
            "var n = $('n'), d = $('d'); \
             var read = [n.valueAsNumber, d.valueAsNumber, $('t').valueAsNumber, \
               $('tm').valueAsNumber].join(','); \
             n.stepUp(); var up = n.value; n.stepUp(); var again = n.value; \
             n.stepUp(9); var clamped = n.value; n.stepDown(2); var down = n.value; \
             d.valueAsNumber = 86400000; var date = d.value; d.stepUp(3); \
             var refused = [attempt(function () { $('t').valueAsNumber = 1; }), \
               attempt(function () { $('t').stepUp(); }), \
               attempt(function () { $('any').stepUp(); }), \
               attempt(function () { n.valueAsNumber = Infinity; })].join(','); \
             n.valueAsNumber = NaN; \
             [read, up, again, clamped, down, date, d.value, refused, JSON.stringify(n.value)] \
               .join('|')"
        ),
        "3,1709164800000,NaN,5400000|4|6|10|6|1970-01-02|1970-01-05\
         |InvalidStateError,InvalidStateError,InvalidStateError,TypeError|\"\""
    );
    assert_eq!(
        eval(
            "var c = document.createElement('input'); c.type = 'checkbox'; \
             var before = c.indeterminate; c.indeterminate = true; \
             [before, c.indeterminate, c.hasAttribute('indeterminate'), tags($('n').labels), \
              tags($('w').labels), $('lab').control.id, $('wrap').control.id, \
              $('lab').form].join('|')"
        ),
        "false|true|false|lab|wrap|n|w|"
    );
}

#[test]
fn output_progress_meter_and_details_reflect_their_state() {
    let mut eval = page(
        "<form id=f><output id=o name=total>5</output></form>\
         <progress id=p max=4 value=6></progress><progress id=busy></progress>\
         <meter id=m min=10 max=20 value=25 low=5></meter><details id=d></details>",
    );
    assert_eq!(
        eval(
            "var o = $('o'), p = $('p'), m = $('m'), d = $('d'); \
             var read = [o.value, o.type, o.name, o.form.id, p.value, p.max, p.position, \
               $('busy').position, m.value, m.low, m.high, m.optimum, d.open].join(','); \
             o.value = '7'; p.value = 1; m.value = 12; d.open = true; \
             [read, o.textContent, p.position, p.getAttribute('value'), m.value, \
              d.hasAttribute('open')].join('|')"
        ),
        "5,output,total,f,4,4,1,-1,20,10,20,15,false|7|0.25|1|12|true"
    );
}
