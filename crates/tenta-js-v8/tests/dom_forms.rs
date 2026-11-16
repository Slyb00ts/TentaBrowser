//! The script interface of forms on the V8 host: control state, the text
//! selection, `<select>`, and submission, which leaves a navigation request
//! on the document for the page (`tenta_dom::NavigationRequest`).

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::Document;
use tenta_js_v8::Engine;

const PAGE: &str = "https://example.com/app/form.html";

const FORM: &str = r##"
<form id="f" action="/login" method="post">
  <input id="user" name="user" value="ada" maxlength="20" required>
  <input id="pass" name="pass" type="PASSWORD">
  <input id="keep" name="keep" type="checkbox" checked>
  <input id="odd" type="frobnicate">
  <textarea id="note" name="note" rows="4">hello</textarea>
  <select id="color" name="color">
    <option value="r">Red</option><option value="g" selected>Green</option><option>Blue</option>
  </select>
  <select id="tags" name="tags" multiple><option>a</option><option selected>b</option></select>
  <button id="go" name="action" value="go">Go</button>
  <button id="clear" type="reset">Clear</button>
  <button id="plain" type="button">Plain</button>
</form>
<input id="stray" name="stray" value="s" form="f">
<a id="next" href="/next?x=1">Next</a>
<a id="anchor" href="#part">Part</a>
<a id="popup" href="/popup" target="_blank">Popup</a>
"##;

fn engine() -> (Engine, Rc<RefCell<Document>>) {
    let mut document = Document::new();
    document.set_url(Some(PAGE.parse().expect("test URL")));
    let document = Rc::new(RefCell::new(document));
    let mut engine = Engine::with_document(document.clone());
    engine
        .evaluate(&format!(
            "var html = document.createElement('html'); document.appendChild(html); \
             var body = document.createElement('body'); html.appendChild(body); \
             body.innerHTML = {FORM:?}; \
             var $ = function (id) {{ return document.getElementById(id); }}; \
             globalThis.log = []; 'ready'"
        ))
        .expect("the test tree");
    (engine, document)
}

fn eval(engine: &mut Engine, source: &str) -> String {
    engine
        .evaluate(source)
        .expect("test script should evaluate without throwing")
}

#[test]
fn inputs_report_their_type_form_and_reflected_attributes() {
    let (mut engine, _document) = engine();
    assert_eq!(
        eval(
            &mut engine,
            "[$('user').type, $('pass').type, $('keep').type, $('odd').type, $('note').type, \
              $('go').type, $('clear').type, $('color').type, $('tags').type].join(' ')",
        ),
        "text password checkbox text textarea submit reset select-one select-multiple"
    );
    assert_eq!(
        eval(
            &mut engine,
            "[$('user').form === $('f'), $('stray').form === $('f'), $('go').form === $('f'), \
              $('color').options[0].form === $('f'), document.createElement('input').form, \
              $('user').required, $('pass').required, $('user').maxLength, $('pass').maxLength, \
              $('user').defaultValue, $('keep').defaultChecked, $('note').rows, $('note').cols, \
              $('note').defaultValue, $('note').textLength].join('|')",
        ),
        "true|true|true|true||true|false|20|-1|ada|true|4|20|hello|5"
    );
    assert_eq!(
        eval(
            &mut engine,
            "$('user').value = 'grace'; $('user').readOnly = true; $('user').type = 'email'; \
             [$('user').value, $('user').defaultValue, $('user').getAttribute('value'), \
              $('user').hasAttribute('readonly'), $('user').type].join('|')",
        ),
        "grace|ada|ada|true|email"
    );
}

#[test]
fn text_controls_have_a_selection_and_others_do_not() {
    let (mut engine, _document) = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var u = $('user'); \
             var initial = [u.selectionStart, u.selectionEnd, u.selectionDirection].join(','); \
             u.setSelectionRange(1, 2, 'backward'); \
             var ranged = [u.selectionStart, u.selectionEnd, u.selectionDirection].join(','); \
             u.selectionStart = 3; \
             var moved = [u.selectionStart, u.selectionEnd].join(','); \
             u.setSelectionRange(2, 99); \
             var clamped = [u.selectionStart, u.selectionEnd, u.selectionDirection].join(','); \
             u.select(); \
             var all = [u.selectionStart, u.selectionEnd].join(','); \
             u.value = 'longer value'; \
             var afterValue = [u.selectionStart, u.selectionEnd].join(','); \
             [initial, ranged, moved, clamped, all, afterValue].join(' | ')",
        ),
        "3,3,none | 1,2,backward | 3,3 | 2,3,none | 0,3 | 12,12",
        "a caret at the end by default and after `value` is set; ranges are clamped"
    );
    assert_eq!(
        eval(
            &mut engine,
            "var n = $('note'); n.setSelectionRange(0, 2); \
             var k = $('keep'); \
             var refused; try { k.setSelectionRange(0, 1); refused = 'no error'; } \
               catch (error) { refused = error.name; } \
             var assigned; try { k.selectionStart = 0; assigned = 'no error'; } \
               catch (error) { assigned = error.name; } \
             k.select(); \
             [n.selectionStart, n.selectionEnd, k.selectionStart, k.selectionEnd, \
              k.selectionDirection, refused, assigned, 'selectionStart' in k].join('|')",
        ),
        "0|2||||InvalidStateError|InvalidStateError|true"
    );
}

#[test]
fn a_select_reports_and_changes_its_selected_option() {
    let (mut engine, _document) = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var c = $('color'); \
             var before = [c.value, c.selectedIndex, c.length, c.options.length, \
                           c.selectedOptions.length, c.selectedOptions[0].text, \
                           c.options[2].value, c.options[2].index, c.options[1].selected, \
                           c.options[1].defaultSelected, c.options[0].label].join(','); \
             c.value = 'Blue'; \
             var byValue = [c.value, c.selectedIndex, c.options[1].selected, \
                            c.options[1].defaultSelected].join(','); \
             c.selectedIndex = 0; \
             var byIndex = [c.value, c.options[2].selected].join(','); \
             c.options[1].selected = true; \
             var byOption = [c.value, c.options[0].selected].join(','); \
             c.value = 'no such value'; \
             var none = [c.value, c.selectedIndex, c.selectedOptions.length].join(','); \
             [before, byValue, byIndex, byOption, none].join(' | ')",
        ),
        "g,1,3,3,1,Green,Blue,2,true,true,Red | Blue,2,false,true | r,false | g,false | ,-1,0"
    );
    assert_eq!(
        eval(
            &mut engine,
            "var t = $('tags'); t.options[0].selected = true; \
             [t.value, t.selectedOptions.length, t.multiple, t.options[1].selected].join(',')",
        ),
        "a,2,true,true",
        "a multiple select keeps the other options selected"
    );
}

#[test]
fn elements_lists_the_forms_controls_and_finds_them_by_name() {
    let (mut engine, _document) = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var e = $('f').elements; \
             [e.length, $('f').length, e[0] === $('user'), e.namedItem('pass') === $('pass'), \
              e[e.length - 1] === $('stray'), $('f').method, $('f').enctype, $('f').encoding, \
              $('f').action, $('f').target, $('f').noValidate].join('|')",
        ),
        "11|11|true|true|true|post|application/x-www-form-urlencoded|\
         application/x-www-form-urlencoded|https://example.com/login||false"
    );
}

#[test]
fn submit_leaves_a_post_request_with_the_current_values() {
    let (mut engine, document) = engine();
    eval(
        &mut engine,
        "$('f').addEventListener('submit', function () { log.push('submit'); }); \
         $('user').value = 'grace hopper'; $('pass').value = 'p&ss'; $('keep').checked = false; \
         $('color').value = 'r'; $('f').submit(); 'submitted'",
    );
    let request = document
        .borrow_mut()
        .take_navigation_request()
        .expect("a navigation request");
    assert_eq!(request.url.as_str(), "https://example.com/login");
    let post = request.post.expect("a POST body");
    assert_eq!(post.content_type, "application/x-www-form-urlencoded");
    assert_eq!(
        String::from_utf8(post.body).unwrap(),
        "user=grace+hopper&pass=p%26ss&note=hello&color=r&tags=b&stray=s",
        "no button contributes to `submit()`"
    );
    assert_eq!(
        eval(&mut engine, "log.length"),
        "0",
        "`submit()` fires no submit event"
    );
}

#[test]
fn request_submit_fires_submit_and_a_listener_can_cancel_it() {
    let (mut engine, document) = engine();
    eval(
        &mut engine,
        "$('f').addEventListener('submit', function (event) { \
           log.push(event.type + ':' + event.cancelable + ':' + event.bubbles); \
         }); \
         $('f').requestSubmit($('go')); 'done'",
    );
    let request = document
        .borrow_mut()
        .take_navigation_request()
        .expect("a request");
    let body = String::from_utf8(request.post.expect("a body").body).unwrap();
    assert!(body.ends_with("&action=go&stray=s"), "{body}");
    assert_eq!(eval(&mut engine, "log.join(' ')"), "submit:true:true");

    eval(
        &mut engine,
        "$('f').addEventListener('submit', function (event) { event.preventDefault(); }); \
         $('f').requestSubmit(); 'done'",
    );
    assert_eq!(
        document.borrow_mut().take_navigation_request(),
        None,
        "a cancelled submission does not navigate"
    );

    assert_eq!(
        eval(
            &mut engine,
            "[$('plain'), $('user'), document.createElement('button')].map(function (candidate) { \
               try { $('f').requestSubmit(candidate); return 'accepted'; } \
               catch (error) { return error.name; } \
             }).join('|')",
        ),
        "TypeError|TypeError|NotFoundError"
    );
}

#[test]
fn clicking_a_submit_button_submits_and_a_reset_button_resets() {
    let (mut engine, document) = engine();
    eval(
        &mut engine,
        "$('user').value = 'changed'; $('go').click(); 'clicked'",
    );
    let request = document
        .borrow_mut()
        .take_navigation_request()
        .expect("the click submitted the form");
    let body = String::from_utf8(request.post.expect("a body").body).unwrap();
    assert!(
        body.starts_with("user=changed&") && body.contains("action=go"),
        "{body}"
    );

    assert_eq!(
        eval(
            &mut engine,
            "$('keep').checked = false; $('color').value = 'r'; $('clear').click(); \
             [$('user').value, $('keep').checked, $('color').value].join('|')",
        ),
        "ada|true|g"
    );
    assert_eq!(document.borrow_mut().take_navigation_request(), None);

    eval(
        &mut engine,
        "$('go').addEventListener('click', function (event) { event.preventDefault(); }); \
         $('go').click(); $('plain').click(); $('go').disabled = true; $('go').click(); 'done'",
    );
    assert_eq!(
        document.borrow_mut().take_navigation_request(),
        None,
        "a cancelled click, a plain button and a disabled button submit nothing"
    );
}

#[test]
fn clicking_a_link_navigates_and_a_fragment_link_stays() {
    let (mut engine, document) = engine();
    eval(&mut engine, "$('next').click(); 'clicked'");
    let request = document
        .borrow_mut()
        .take_navigation_request()
        .expect("the link was followed");
    assert_eq!(request.url.as_str(), "https://example.com/next?x=1");
    assert_eq!(request.post, None);

    eval(
        &mut engine,
        "addEventListener('hashchange', function (event) { log.push(event.newURL); }); \
         $('anchor').click(); $('popup').click(); \
         $('next').addEventListener('click', function (event) { event.preventDefault(); }); \
         $('next').click(); 'clicked'",
    );
    assert_eq!(
        document.borrow_mut().take_navigation_request(),
        None,
        "a fragment link stays, a `_blank` link opens nothing, a cancelled click is not followed"
    );
    engine.run_event_loop(16);
    assert_eq!(
        eval(
            &mut engine,
            "log.join(' ') + '|' + location.hash + '|' + history.length"
        ),
        format!("{PAGE}#part|#part|2")
    );
}

#[test]
fn html_elements_reflect_hidden_title_lang_and_dir() {
    let (mut engine, _document) = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var p = document.createElement('p'); body.appendChild(p); \
             var before = [p.hidden, p.title, p.lang, p.dir, p.inert, p.accessKey].join('/'); \
             p.hidden = true; p.title = 'tip'; p.lang = 'pl'; p.dir = 'RTL'; p.inert = true; \
             var attrs = [p.hasAttribute('hidden'), p.getAttribute('title'), \
                          p.getAttribute('lang'), p.getAttribute('dir'), p.dir, \
                          p.hasAttribute('inert')].join('/'); \
             p.hidden = false; p.setAttribute('dir', 'sideways'); \
             [before, attrs, p.hasAttribute('hidden'), p.dir === '', \
              'hidden' in document.createElementNS('http://www.w3.org/2000/svg', 'g')].join(' | ')",
        ),
        "false////false/ | true/tip/pl/RTL/rtl/true | false | true | false"
    );
}

#[test]
fn inner_text_reads_laid_out_text_and_writes_lines() {
    let (mut engine, _document) = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var box = document.createElement('div'); body.appendChild(box); \
             box.innerHTML = '<p>one <b>two</b></p><p hidden>secret</p><div>three</div>' + \
                             '<script>var x = 1;</' + 'script>'; \
             var read = JSON.stringify(box.innerText); \
             var loose = document.createElement('div'); \
             loose.innerHTML = '<p>a</p><p hidden>b</p>'; \
             var detached = loose.innerText; \
             box.innerText = 'first\\nsecond\\r\\n\\nlast'; \
             var written = box.innerHTML; \
             box.innerText = ''; \
             [read, detached, written, box.childNodes.length].join(' | ')",
        ),
        "\"one two\\nthree\" | ab | first<br>second<br><br>last | 0"
    );
}
