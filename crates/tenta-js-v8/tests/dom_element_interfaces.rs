//! Tests for Phase 4d: per-tag HTML element interfaces.
//!
//! Each interactive/common tag gets its own interface (and global constructor)
//! so `instanceof HTMLInputElement` and friends work, with the prototype chain
//! HTMLXxxElement -> HTMLElement -> Element -> Node -> EventTarget. Only the
//! safe reflected attributes are wired: `name`, `placeholder` (DOMString) and
//! `disabled` (boolean); `HTMLLabelElement.htmlFor` reflects the `for`
//! attribute. The enumerated and URL-valued attributes are intentionally
//! absent; the dirty value model lives in `dom_form_value.rs`.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_js_v8::Engine;

/// A document with `<html><body></body></html>`; tests create their own nodes.
fn document() -> Rc<RefCell<Document>> {
    let mut d = Document::new();
    let html = d.create_html_element(atoms::HTML);
    let body = d.create_html_element(atoms::BODY);
    d.mutate(Mutation::Append {
        parent: d.document_node(),
        child: html,
    })
    .unwrap();
    d.mutate(Mutation::Append {
        parent: html,
        child: body,
    })
    .unwrap();
    Rc::new(RefCell::new(d))
}

fn eval(source: &str) -> String {
    let mut engine = Engine::with_document(document());
    engine
        .evaluate(source)
        .expect("script should evaluate without throwing")
}

#[test]
fn per_tag_elements_get_their_own_interface_prototype() {
    assert_eq!(
        eval(
            r#"
            const mk = (t) => document.createElement(t);
            [
                mk('div')      instanceof HTMLDivElement,
                mk('span')     instanceof HTMLSpanElement,
                mk('p')        instanceof HTMLParagraphElement,
                mk('a')        instanceof HTMLAnchorElement,
                mk('img')      instanceof HTMLImageElement,
                mk('input')    instanceof HTMLInputElement,
                mk('button')   instanceof HTMLButtonElement,
                mk('select')   instanceof HTMLSelectElement,
                mk('textarea') instanceof HTMLTextAreaElement,
                mk('option')   instanceof HTMLOptionElement,
                mk('form')     instanceof HTMLFormElement,
                mk('label')    instanceof HTMLLabelElement,
            ].join(',');
        "#
        ),
        "true,true,true,true,true,true,true,true,true,true,true,true"
    );
}

#[test]
fn the_prototype_chain_reaches_htmlelement_and_element() {
    assert_eq!(
        eval(
            r#"
            const input = document.createElement('input');
            [
                input instanceof HTMLElement,
                input instanceof Element,
                input instanceof Node,
                Object.getPrototypeOf(HTMLInputElement.prototype) === HTMLElement.prototype,
                typeof input.click,   // inherited from HTMLElement
            ].join(',');
        "#
        ),
        "true,true,true,true,function"
    );
}

#[test]
fn a_wrong_tag_is_not_an_instance_of_a_sibling_interface() {
    assert_eq!(
        eval(
            r#"
            const div = document.createElement('div');
            const input = document.createElement('input');
            [
                div instanceof HTMLInputElement,
                input instanceof HTMLDivElement,
                div instanceof HTMLElement,
            ].join(',');
        "#
        ),
        "false,false,true"
    );
}

#[test]
fn name_reflects_as_a_string_attribute() {
    assert_eq!(
        eval(
            r#"
            const input = document.createElement('input');
            const before = input.name;              // absent -> ""
            input.name = 'email';
            [before, input.name, input.getAttribute('name')].join('|');
        "#
        ),
        "|email|email"
    );
}

#[test]
fn placeholder_reflects_on_input_and_textarea() {
    assert_eq!(
        eval(
            r#"
            const input = document.createElement('input');
            input.setAttribute('placeholder', 'Type here');
            const area = document.createElement('textarea');
            area.placeholder = 'Body';
            [input.placeholder, area.placeholder, area.getAttribute('placeholder')].join('|');
        "#
        ),
        "Type here|Body|Body"
    );
}

#[test]
fn disabled_reflects_as_a_boolean_attribute() {
    assert_eq!(
        eval(
            r#"
            const b = document.createElement('button');
            const initial = b.disabled;             // absent -> false
            b.disabled = true;
            const present = b.getAttribute('disabled');   // "" when set
            const on = b.disabled;
            b.disabled = false;
            const off = b.disabled;
            const gone = b.hasAttribute('disabled');
            [initial, present, on, off, gone].join('|');
        "#
        ),
        "false||true|false|false"
    );
}

#[test]
fn disabled_is_true_for_any_present_value() {
    assert_eq!(
        eval(
            r#"
            const s = document.createElement('select');
            s.setAttribute('disabled', 'disabled');
            String(s.disabled);
        "#
        ),
        "true"
    );
}

#[test]
fn label_html_for_reflects_the_for_attribute() {
    assert_eq!(
        eval(
            r#"
            const label = document.createElement('label');
            const before = label.htmlFor;
            label.htmlFor = 'email';
            [before, label.htmlFor, label.getAttribute('for')].join('|');
        "#
        ),
        "|email|email"
    );
}

#[test]
fn form_name_reflects() {
    assert_eq!(
        eval(
            r#"
            const form = document.createElement('form');
            form.name = 'login';
            [form.name, form.getAttribute('name'), form instanceof HTMLFormElement].join('|');
        "#
        ),
        "login|login|true"
    );
}

#[test]
fn constructors_are_present_on_the_global() {
    assert_eq!(
        eval(
            r#"
            [
                typeof HTMLInputElement,
                typeof HTMLDivElement,
                typeof HTMLFormElement,
                typeof HTMLLabelElement,
            ].join(',');
        "#
        ),
        "function,function,function,function"
    );
}

// ---------------------------------------------------------------------------
// EL1: every HTML element has the interface the standard gives its name
// ---------------------------------------------------------------------------

#[test]
fn every_html_element_name_has_its_interface() {
    // Each pair is a tag and the interface its wrapper must be an instance of
    // and report as its constructor. The interfaces are all globals, so a page
    // naming one in an `instanceof` does not get a `ReferenceError`.
    assert_eq!(
        eval(
            r#"
            const expected = {
                html: 'HTMLHtmlElement', head: 'HTMLHeadElement', title: 'HTMLTitleElement',
                base: 'HTMLBaseElement', body: 'HTMLBodyElement',
                h1: 'HTMLHeadingElement', h2: 'HTMLHeadingElement', h3: 'HTMLHeadingElement',
                h4: 'HTMLHeadingElement', h5: 'HTMLHeadingElement', h6: 'HTMLHeadingElement',
                hr: 'HTMLHRElement', pre: 'HTMLPreElement', listing: 'HTMLPreElement',
                xmp: 'HTMLPreElement', blockquote: 'HTMLQuoteElement', q: 'HTMLQuoteElement',
                ol: 'HTMLOListElement', ul: 'HTMLUListElement', menu: 'HTMLMenuElement',
                li: 'HTMLLIElement', dl: 'HTMLDListElement', data: 'HTMLDataElement',
                time: 'HTMLTimeElement', br: 'HTMLBRElement', ins: 'HTMLModElement',
                del: 'HTMLModElement', picture: 'HTMLPictureElement', embed: 'HTMLEmbedElement',
                object: 'HTMLObjectElement', track: 'HTMLTrackElement', map: 'HTMLMapElement',
                area: 'HTMLAreaElement', table: 'HTMLTableElement',
                caption: 'HTMLTableCaptionElement', col: 'HTMLTableColElement',
                colgroup: 'HTMLTableColElement', thead: 'HTMLTableSectionElement',
                tbody: 'HTMLTableSectionElement', tfoot: 'HTMLTableSectionElement',
                tr: 'HTMLTableRowElement', td: 'HTMLTableCellElement',
                th: 'HTMLTableCellElement', fieldset: 'HTMLFieldSetElement',
                legend: 'HTMLLegendElement', optgroup: 'HTMLOptGroupElement',
                datalist: 'HTMLDataListElement', output: 'HTMLOutputElement',
                progress: 'HTMLProgressElement', meter: 'HTMLMeterElement',
                details: 'HTMLDetailsElement', param: 'HTMLParamElement',
                font: 'HTMLFontElement', dir: 'HTMLDirectoryElement',
                frame: 'HTMLFrameElement', frameset: 'HTMLFrameSetElement',
                marquee: 'HTMLMarqueeElement',
            };
            const wrong = [];
            for (const [tag, name] of Object.entries(expected)) {
                const element = document.createElement(tag);
                const Interface = globalThis[name];
                if (typeof Interface !== 'function' || !(element instanceof Interface)
                    || !(element instanceof HTMLElement) || element.constructor !== Interface
                    || Object.prototype.toString.call(element) !== '[object ' + name + ']'
                    || Object.getPrototypeOf(Interface.prototype) !== HTMLElement.prototype) {
                    wrong.push(tag);
                }
            }
            [Object.keys(expected).length, wrong.join(',')].join('|');
            "#
        ),
        "57|"
    );
}

#[test]
fn an_element_with_no_interface_of_its_own_and_an_unknown_one_are_told_apart() {
    assert_eq!(
        eval(
            r#"
            const name = tag => document.createElement(tag).constructor.name;
            [
                name('section'), name('em'), name('summary'), name('main'), name('code'),
                name('my-widget'), name('x-y-z'),
                name('blink'), name('foo'), name('customelement'),
                document.createElement('foo') instanceof HTMLElement,
                document.createElement('section') instanceof HTMLUnknownElement,
                // The document's own elements, not only ones script made.
                document.documentElement.constructor.name, document.body.constructor.name,
                document.body instanceof HTMLBodyElement,
            ].join('|');
            "#
        ),
        "HTMLElement|HTMLElement|HTMLElement|HTMLElement|HTMLElement|HTMLElement|HTMLElement|\
         HTMLUnknownElement|HTMLUnknownElement|HTMLUnknownElement|true|false|HTMLHtmlElement|\
         HTMLBodyElement|true"
    );
}

#[test]
fn the_interfaces_cannot_be_constructed_and_have_no_members_of_their_own() {
    assert_eq!(
        eval(
            r#"
            const attempt = f => { try { f(); return 'made'; } catch (e) { return e.constructor.name; } };
            [
                attempt(() => new HTMLBodyElement()), attempt(() => new HTMLTableElement()),
                attempt(() => HTMLHeadingElement()),
                // `HTMLTableElement` has had members since FM2; its caption's
                // interface is still a shell.
                Object.keys(HTMLTableCaptionElement.prototype).length,
                Object.keys(HTMLLIElement.prototype).length,
                HTMLUListElement.name, HTMLUnknownElement.length,
            ].join('|');
            "#
        ),
        "TypeError|TypeError|TypeError|0|0|HTMLUListElement|0"
    );
}
