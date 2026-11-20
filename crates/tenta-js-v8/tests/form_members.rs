//! Table, select, input, label and other form element members through
//! generated V8 bindings.

use tenta_js_v8::Engine;

// The engine runs its preludes as a page's does, so the scripted members
// below are there.
fn new_engine(document: tenta_dom::SharedDocument) -> Engine {
    let mut engine = Engine::with_document(document);
    engine.install_preludes();
    engine
}

include!("../../tenta-js/tests/support/form_members.rs");

// The scripted members (`tenta_js::conveniences`), which QuickJS lacks.

#[test]
fn option_and_audio_are_legacy_factory_functions() {
    let mut eval = page("<select id=s></select>");
    assert_eq!(
        eval(
            "var plain = new Option(); \
             var full = new Option('Red', 'r', true, false); \
             var picked = new Option('Blue', 'b', false, true); \
             var s = $('s'); s.add(full); s.add(picked); \
             var audio = new Audio('/a.mp3'), silent = new Audio(); \
             [plain instanceof HTMLOptionElement, plain.text, plain.value, plain.childNodes.length, \
              full.text, full.value, full.defaultSelected, picked.selected, s.value, \
              Option.prototype === HTMLOptionElement.prototype, Option.length, Option.name, \
              Object.keys(window).indexOf('Option'), \
              attempt(function () { return Option('x'); }), \
              audio instanceof HTMLAudioElement, audio.getAttribute('src'), audio.preload, \
              silent.hasAttribute('src'), Audio.prototype === HTMLAudioElement.prototype].join('|')"
        ),
        "true|||0|Red|r|true|true|b|true|0|Option|-1|TypeError|true|/a.mp3|auto|false|true"
    );
}

#[test]
fn select_remove_takes_an_index_and_without_one_removes_the_select() {
    let mut eval = page(
        "<div id=box><select id=s><option id=a>A</option><option id=b>B</option>\
         <option id=c>C</option></select></div>",
    );
    assert_eq!(
        eval(
            "var s = $('s'); s.remove(1); var one = tags(s.options); s.remove(9); s.remove(-1); \
             var kept = tags(s.options); s.remove(); \
             [one, kept, s.parentNode, $('box').children.length, HTMLSelectElement.prototype.remove.length, \
              attempt(function () { HTMLSelectElement.prototype.remove.call({}, 0); })].join('|')"
        ),
        "a,c|a,c||0|0|TypeError"
    );
}
