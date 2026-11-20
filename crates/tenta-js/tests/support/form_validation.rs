// Included by V8 and QuickJS adapters: constraint validation of form
// controls (`tenta_dom::validation`) and the refusal of an invalid
// submission.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::Document;

const HELPERS: &str = "var $ = function (id) { return document.getElementById(id); }; \
    var log = []; \
    var make = function (html) { var holder = document.createElement('div'); \
      holder.innerHTML = html; document.body.append(holder); return holder.firstElementChild; }; \
    var flags = function (el) { var v = el.validity; return ['valueMissing','typeMismatch', \
      'patternMismatch','tooLong','tooShort','rangeUnderflow','rangeOverflow','stepMismatch', \
      'badInput','customError'].filter(function (name) { return v[name]; }).join('+') \
      || (v.valid ? 'valid' : '?'); };";

/// `text` as a JavaScript string literal.
fn quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

fn page(body: &str) -> (Rc<RefCell<Document>>, impl FnMut(&str) -> String) {
    let mut document = Document::new();
    document.set_url(Some(
        "https://example.com/app/form.html".parse().expect("test URL"),
    ));
    let document = Rc::new(RefCell::new(document));
    // QuickJS evaluates through `&self`, V8 through `&mut self`.
    #[allow(unused_mut)]
    let mut engine = new_engine(document.clone());
    engine
        .evaluate(&format!(
            "var html = document.createElement('html'); document.appendChild(html); \
             var body = document.createElement('body'); html.appendChild(body); \
             body.innerHTML = {}; {HELPERS} 'ready'",
            quoted(body)
        ))
        .expect("set up the page");
    (document, move |source: &str| {
        engine.evaluate(source).expect("validation script")
    })
}

#[test]
fn each_constraint_is_reported_by_its_flag() {
    let (_, mut eval) = page("");
    assert_eq!(
        eval(
            "[ \
               '<input required>', '<input required value=x>', '<input type=checkbox required>', \
               '<input type=checkbox required checked>', '<textarea required></textarea>', \
               '<select required><option value=\"\">Choose</option><option>a</option></select>', \
               '<select required><option>a</option></select>', \
               '<input type=email value=nobody>', '<input type=email value=a@b.example>', \
               '<input type=email multiple value=\"a@b, c@d\">', \
               '<input type=url value=/relative>', '<input type=url value=https://x.example/>', \
               '<input pattern=\"[0-9]{3}\" value=12>', '<input pattern=\"[0-9]{3}\" value=123>', \
               '<input pattern=\"(?=x)\" value=y>', \
               '<input type=number min=1 max=5 value=0>', '<input type=number max=5 value=9>', \
               '<input type=number step=0.5 min=0 value=1.25>', '<input type=number step=0.1 value=0.3>', \
               '<input type=number step=any value=1.2345>', '<input type=number value=abc>', \
               '<input type=number min=1 value=2 step=2>', \
               '<input type=date min=2024-01-10 value=2024-01-09>', \
               '<input type=date step=2 min=2024-01-01 value=2024-01-02>', \
               '<input type=time min=22:00 max=06:00 value=12:00>', \
               '<input type=time min=22:00 max=06:00 value=23:00>', \
               '<input type=month max=2024-05 value=2024-06>', \
               '<input type=week value=2021-W53>', '<input type=range min=0 max=10 value=50>', \
               '<input type=hidden required>' \
             ].map(function (html) { return flags(make(html)); }).join(' ')"
        ),
        "valueMissing valid valueMissing valid valueMissing valueMissing valid \
         typeMismatch valid valid typeMismatch valid patternMismatch valid valid \
         rangeUnderflow rangeOverflow stepMismatch valid valid badInput stepMismatch \
         rangeUnderflow stepMismatch rangeUnderflow+rangeOverflow valid rangeOverflow \
         badInput valid valid"
    );
}

#[test]
fn a_radio_group_is_missing_its_value_until_one_button_is_checked() {
    let (_, mut eval) = page(
        "<form><input type=radio name=size value=s id=s required>\
         <input type=radio name=size value=m id=m></form>\
         <input type=radio name=size id=outside>",
    );
    assert_eq!(
        eval(
            "var before = [flags($('s')), flags($('m')), flags($('outside'))].join(','); \
             $('m').checked = true; \
             [before, flags($('s')), flags($('m')), $('s').validationMessage].join('|')"
        ),
        "valueMissing,valueMissing,valid|valid|valid|"
    );
}

#[test]
fn too_short_needs_a_value_that_was_changed() {
    let (_, mut eval) = page(
        "<input id=a minlength=4 value=ab><textarea id=t minlength=3></textarea>",
    );
    assert_eq!(
        eval(
            "var before = [flags($('a')), flags($('t'))].join(','); \
             $('a').value = 'abc'; $('t').value = 'x'; \
             var after = [flags($('a')), flags($('t')), $('a').validationMessage, \
               $('t').validationMessage, $('t').minLength].join(','); \
             $('a').value = ''; [before, after, flags($('a'))].join('|')"
        ),
        "valid,valid|tooShort,tooShort,\
         Please lengthen this text to 4 characters or more (you are currently using 3 characters).,\
         Please lengthen this text to 3 characters or more (you are currently using 1 character).,\
         3|valid"
    );
}

#[test]
fn barred_controls_do_not_validate_and_validity_is_one_live_object() {
    let (_, mut eval) = page(
        "<form id=f>\
         <input id=hidden type=hidden required><input id=ro required readonly>\
         <input id=off required disabled>\
         <fieldset disabled><input id=inset required></fieldset>\
         <datalist><input id=listed required></datalist>\
         <button id=submit></button><button id=reset type=reset></button>\
         <button id=plain type=button></button><input id=check type=checkbox required readonly checked>\
         <select id=sel required><option value=''>-</option></select>\
         </form>",
    );
    assert_eq!(
        eval(
            "['hidden','ro','off','inset','listed','submit','reset','plain','check','sel'] \
               .map(function (id) { return $(id).willValidate; }).join(',')"
        ),
        "false,false,false,false,false,true,false,false,true,true"
    );
    assert_eq!(
        eval(
            "var v = $('sel').validity; \
             var shape = [v === $('sel').validity, v instanceof ValidityState, \
               Object.prototype.toString.call(v), v.valid].join(','); \
             $('sel').required = false; \
             var live = v.valid; \
             $('ro').setCustomValidity('Not now'); \
             [shape, live, $('ro').validity.customError, $('ro').checkValidity(), \
              $('ro').validationMessage === '', $('f').checkValidity()].join('|')"
        ),
        "true,true,[object ValidityState],false|true|true|true|true|true"
    );
}

#[test]
fn custom_validity_and_messages() {
    let (_, mut eval) = page(
        "<input id=name required><input id=mail type=email value=me>\
         <input id=box type=checkbox required><select id=pick required></select>\
         <input id=n type=number min=2 max=8 step=3 value=6><button id=b></button>",
    );
    assert_eq!(
        eval(
            "var messages = ['name','mail','box','pick','n'].map(function (id) { \
               return $(id).validationMessage; }); \
             $('n').value = '1'; messages.push($('n').validationMessage); \
             $('n').value = '9'; messages.push($('n').validationMessage); \
             $('b').setCustomValidity('Pick a plan first'); \
             messages.push(flags($('b')), $('b').validationMessage); \
             $('b').setCustomValidity(''); messages.push(flags($('b'))); \
             messages.join('|')"
        ),
        "Please fill out this field.\
         |Please include an '@' in the email address. 'me' is missing an '@'.\
         |Please check this box if you want to proceed.\
         |Please select an item in the list.\
         |Please enter a valid value. The two nearest valid values are 5 and 8.\
         |Value must be greater than or equal to 2.\
         |Value must be less than or equal to 8.\
         |customError|Pick a plan first|valid"
    );
}

#[test]
fn check_and_report_validity_fire_invalid_and_report_focuses() {
    let (_, mut eval) = page(
        "<form id=f><input id=a required><input id=b required><input id=c value=ok></form>",
    );
    assert_eq!(
        eval(
            "['a','b','c','f'].forEach(function (id) { \
               $(id).addEventListener('invalid', function (event) { \
                 log.push(id + ':' + event.target.id + ':' + event.bubbles + ':' + event.cancelable); \
               }); }); \
             var results = [$('c').checkValidity(), $('a').checkValidity(), $('f').checkValidity()]; \
             var fired = log.join(' '); log = []; \
             $('a').addEventListener('invalid', function (event) { event.preventDefault(); }); \
             var reported = $('f').reportValidity(); \
             [results.join(','), fired, reported, log.join(' '), document.activeElement.id, \
              $('a').reportValidity(), document.activeElement.id].join('|')"
        ),
        "true,false,false|a:a:false:true a:a:false:true b:b:false:true\
         |false|a:a:false:true b:b:false:true|b|false|b"
    );
}

#[test]
fn an_invalid_form_is_not_submitted_unless_validation_is_turned_off() {
    let (document, mut eval) = page(
        "<form id=f action=/send method=post><input id=q name=q required>\
         <button id=go>Go</button><button id=skip formnovalidate>Skip</button></form>",
    );
    let taken = || document.borrow_mut().take_navigation_request().is_some();
    eval(
        "$('f').addEventListener('submit', function () { log.push('submit'); }); \
         $('q').addEventListener('invalid', function () { log.push('invalid'); }); \
         $('f').requestSubmit(); $('go').click(); 'tried'",
    );
    assert!(!taken(), "an invalid form is not submitted");
    assert_eq!(eval("log.join(' ')"), "invalid invalid");

    eval("$('f').requestSubmit($('skip')); 'skipped'");
    assert!(taken(), "formnovalidate submits");
    eval("$('f').noValidate = true; $('f').requestSubmit(); $('f').noValidate = false; 'nv'");
    assert!(taken(), "novalidate submits");
    eval("$('f').submit(); 'plain'");
    assert!(taken(), "submit() does not validate");

    eval("log = []; $('q').value = 'tenta'; $('go').click(); 'valid now'");
    assert!(taken(), "a valid form is submitted");
    assert_eq!(
        eval("[log.join(' '), $('go').formNoValidate, $('skip').formNoValidate].join('|')"),
        "submit|false|true"
    );
}
