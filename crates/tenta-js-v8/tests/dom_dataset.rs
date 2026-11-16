//! `element.dataset` and `document.currentScript` on the V8 host.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::Document;
use tenta_js_v8::Engine;

fn engine() -> (Engine, Rc<RefCell<Document>>) {
    let mut document = Document::new();
    document.set_url(Some("https://example.com/".parse().expect("test URL")));
    let document = Rc::new(RefCell::new(document));
    let mut engine = Engine::with_document(document.clone());
    engine
        .evaluate(
            "var html = document.createElement('html'); document.appendChild(html); \
             var body = document.createElement('body'); html.appendChild(body); \
             body.innerHTML = '<div id=\"box\" data-id=\"7\" data-user-name=\"ada\" \
               data-Upper=\"lost\" title=\"t\"></div>'; \
             var box = document.getElementById('box'); 'ready'",
        )
        .expect("the test tree");
    (engine, document)
}

fn eval(engine: &mut Engine, source: &str) -> String {
    engine
        .evaluate(source)
        .expect("test script should evaluate without throwing")
}

#[test]
fn dataset_reads_data_attributes_under_camel_cased_names() {
    let (mut engine, _document) = engine();
    assert_eq!(
        eval(
            &mut engine,
            "[box.dataset.id, box.dataset.userName, box.dataset.missing, box.dataset.title, \
              'id' in box.dataset, 'missing' in box.dataset, \
              Object.keys(box.dataset).join(','), JSON.stringify(box.dataset), \
              box.dataset === box.dataset, box.dataset instanceof DOMStringMap, \
              Object.prototype.toString.call(box.dataset)].join('|')",
        ),
        "7|ada|||true|false|id,userName,upper|{\"id\":\"7\",\"userName\":\"ada\",\"upper\":\"lost\"}|\
         true|true|[object DOMStringMap]",
        "the parser lower-cased `data-Upper`, so it is `upper`"
    );
}

#[test]
fn dataset_writes_and_deletes_attributes() {
    let (mut engine, _document) = engine();
    assert_eq!(
        eval(
            &mut engine,
            "box.dataset.fooBar = 'x'; box.dataset.id = 8; box.dataset.empty = ''; \
             delete box.dataset.userName; \
             [box.getAttribute('data-foo-bar'), box.getAttribute('data-id'), \
              box.hasAttribute('data-empty'), box.hasAttribute('data-user-name'), \
              box.dataset.fooBar, Object.keys(box.dataset).sort().join(',')].join('|')",
        ),
        "x|8|true|false|x|empty,fooBar,id,upper"
    );
    assert_eq!(
        eval(
            &mut engine,
            "box.setAttribute('data-late', 'seen'); \
             var refused; try { box.dataset['bad-name'] = 1; refused = 'set'; } \
               catch (error) { refused = error.name; } \
             var invalid; try { box.dataset['a b'] = 1; invalid = 'set'; } \
               catch (error) { invalid = error.name; } \
             [box.dataset.late, refused, invalid, box.hasAttribute('data-bad-name')].join('|')",
        ),
        "seen|SyntaxError|InvalidCharacterError|false"
    );
}

#[test]
fn svg_elements_have_a_dataset_and_other_nodes_do_not() {
    let (mut engine, _document) = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg'); \
             svg.setAttribute('data-kind', 'icon'); \
             [svg.dataset.kind, 'dataset' in document.createTextNode('x'), \
              'dataset' in document].join('|')",
        ),
        "icon|false|false"
    );
}

#[test]
fn current_script_is_null_outside_a_running_script_element() {
    let (mut engine, document) = engine();
    assert_eq!(
        eval(&mut engine, "String(document.currentScript)"),
        "null",
        "an evaluation the host makes is not a script element's script"
    );
    // The page names the element while it runs the element's script.
    let script = {
        let mut doc = document.borrow_mut();
        let body = doc.body().expect("a body");
        let script = doc.create_html_element(tenta_core::atom::atoms::SCRIPT);
        doc.mutate(tenta_dom::Mutation::Append {
            parent: body,
            child: script,
        })
        .expect("attached");
        let reference = doc.node_ref(script);
        doc.set_current_script(Some(reference));
        script
    };
    assert_eq!(
        eval(
            &mut engine,
            "[document.currentScript === document.querySelector('script'), \
              document.currentScript.tagName, \
              document.currentScript.parentElement === body].join('|')",
        ),
        "true|SCRIPT|true"
    );
    document.borrow_mut().set_current_script(None);
    assert_eq!(eval(&mut engine, "String(document.currentScript)"), "null");
    let _ = script;
}
