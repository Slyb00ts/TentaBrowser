//! Node comparison, element siblings, `insertAdjacent*`, `normalize`, the
//! document's element collections and the scrolling no-ops on the V8 host.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::Document;
use tenta_js_v8::Engine;

fn engine() -> Engine {
    let mut document = Document::new();
    document.set_url(Some(
        "https://example.com/dir/page.html"
            .parse()
            .expect("test URL"),
    ));
    let mut engine = Engine::with_document(Rc::new(RefCell::new(document)));
    engine
        .evaluate(
            r##"var html = document.createElement('html'); document.appendChild(html);
               var body = document.createElement('body'); html.appendChild(body);
               body.innerHTML =
                 '<ul id="list"><li id="a" class="x">one</li> text <li id="b">two</li><!--c--><li id="c">three</li></ul>' +
                 '<form id="f1" name="login"></form><img id="i1" name="pic"><a id="l1" href="/x">x</a>' +
                 '<a id="n1" name="anchor">named</a><area id="ar" href="/y"><embed id="e1">';
               var $ = function (id) { return document.getElementById(id); };
               'ready'"##,
        )
        .expect("the test tree");
    engine
}

fn eval(engine: &mut Engine, source: &str) -> String {
    engine
        .evaluate(source)
        .expect("test script should evaluate without throwing")
}

#[test]
fn element_siblings_skip_text_and_comments() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "[$('a').nextElementSibling.id, $('b').nextElementSibling.id, \
              $('c').nextElementSibling, $('c').previousElementSibling.id, \
              $('a').previousElementSibling, \
              $('a').nextSibling.nextElementSibling.id, \
              $('a').nextSibling.previousElementSibling.id].join('|')",
        ),
        "b|c||b||b|a"
    );
}

#[test]
fn is_equal_node_compares_structure_and_is_same_node_identity() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var copy = $('list').cloneNode(true); \
             var reordered = document.createElement('li'); \
             reordered.setAttribute('class', 'x'); reordered.setAttribute('id', 'a'); \
             reordered.textContent = 'one'; \
             var changed = $('a').cloneNode(true); changed.textContent = 'other'; \
             [$('list').isEqualNode(copy), copy.isEqualNode($('list')), \
              $('a').isEqualNode(reordered), $('a').isEqualNode(changed), \
              $('a').isEqualNode($('b')), $('a').isEqualNode(null), $('a').isEqualNode($('a')), \
              $('a').isSameNode($('a')), $('a').isSameNode(reordered), $('a').isSameNode(null), \
              document.createTextNode('t').isEqualNode(document.createTextNode('t')), \
              document.createTextNode('t').isEqualNode(document.createComment('t'))].join('|')",
        ),
        "true|true|true|false|false|false|true|true|false|false|true|false",
        "attribute order does not matter; text and kind do"
    );
}

#[test]
fn compare_document_position_reports_order_and_containment() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var loose = document.createElement('p'); \
             [$('a').compareDocumentPosition($('a')), \
              $('a').compareDocumentPosition($('b')), $('b').compareDocumentPosition($('a')), \
              $('list').compareDocumentPosition($('a')), $('a').compareDocumentPosition($('list')), \
              ($('a').compareDocumentPosition(loose) & Node.DOCUMENT_POSITION_DISCONNECTED) !== 0, \
              Node.DOCUMENT_POSITION_FOLLOWING, Node.DOCUMENT_POSITION_CONTAINED_BY, \
              $('a').getRootNode() === document, loose.getRootNode() === loose, \
              $('a').getRootNode({ composed: true }) === document].join('|')",
        ),
        "0|4|2|20|10|true|4|16|true|true|true"
    );
}

#[test]
fn normalize_merges_adjacent_text_and_drops_empty_text() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var p = document.createElement('p'); \
             p.appendChild(document.createTextNode('a')); \
             p.appendChild(document.createTextNode('')); \
             p.appendChild(document.createTextNode('b')); \
             var b = document.createElement('b'); p.appendChild(b); \
             b.appendChild(document.createTextNode('')); \
             p.appendChild(document.createTextNode('c')); \
             p.appendChild(document.createTextNode('d')); \
             var before = p.childNodes.length; p.normalize(); \
             [before, p.childNodes.length, p.firstChild.data, p.lastChild.data, \
              b.childNodes.length, p.textContent].join('|')",
        ),
        "6|3|ab|cd|0|abcd"
    );
}

#[test]
fn insert_adjacent_element_and_text_place_nodes_by_position() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "function make(id) { var e = document.createElement('i'); e.id = id; return e; } \
             var b = $('b'); \
             var returned = b.insertAdjacentElement('beforebegin', make('bb')); \
             b.insertAdjacentElement('afterEnd', make('ae')); \
             b.insertAdjacentElement('AFTERBEGIN', make('ab')); \
             b.insertAdjacentElement('beforeend', make('be')); \
             b.insertAdjacentText('beforeend', '!'); \
             var loose = document.createElement('p'); \
             var noParent = loose.insertAdjacentElement('beforebegin', make('np')); \
             var bad; try { b.insertAdjacentElement('sideways', make('x')); bad = 'inserted'; } \
               catch (error) { bad = error.name; } \
             [returned.id, b.previousElementSibling.id, b.nextElementSibling.id, \
              b.firstElementChild.id, b.lastElementChild.id, b.textContent, noParent, bad].join('|')",
        ),
        "bb|bb|ae|ab|be|two!||SyntaxError"
    );
}

#[test]
fn the_document_lists_its_forms_images_links_and_named_elements() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "function ids(list) { return Array.prototype.map.call(list, function (e) { \
               return e.id; }).join(','); } \
             var script = document.createElement('script'); script.id = 's1'; \
             body.appendChild(script); \
             [ids(document.forms), ids(document.images), ids(document.links), \
              ids(document.scripts), ids(document.embeds), ids(document.anchors), \
              ids(document.getElementsByName('login')), \
              document.getElementsByName('nothing').length, \
              document.forms.namedItem('f1') === $('f1'), \
              document.forms instanceof HTMLCollection, \
              document.getElementsByName('pic') instanceof NodeList].join('|')",
        ),
        "f1|i1|l1,ar|s1|e1|n1|f1|0|true|true|true"
    );
}

#[test]
fn a_page_is_visible_and_its_base_uri_is_its_url() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "[document.visibilityState, document.hidden, document.baseURI, $('a').baseURI, \
              $('a').matches('li.x'), $('a').webkitMatchesSelector('li.x'), \
              $('a').webkitMatchesSelector('p')].join('|')",
        ),
        "visible|false|https://example.com/dir/page.html|https://example.com/dir/page.html|\
         true|true|false"
    );
    assert_eq!(
        eval(
            &mut engine,
            "(function () { try { $('a').webkitMatchesSelector('li['); return 'matched'; } \
               catch (error) { return error.name; } })()",
        ),
        "SyntaxError"
    );
}

#[test]
fn import_node_clones_and_adopt_node_detaches() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var shallow = document.importNode($('list')); \
             var deep = document.importNode($('list'), true); \
             var adopted = document.adoptNode($('c')); \
             var refused; try { document.importNode(document); refused = 'imported'; } \
               catch (error) { refused = error.name; } \
             [shallow !== $('list'), shallow.childNodes.length, deep.children.length, \
              deep.parentNode, adopted.id, adopted.parentNode, $('list').children.length, \
              refused].join('|')",
        ),
        "true|0|3||c||2|NotSupportedError"
    );
}

#[test]
fn elements_do_not_scroll() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "$('a').scrollIntoView(); $('a').scrollIntoView({ behavior: 'smooth', block: 'center' }); \
             $('a').scrollIntoView(false); $('list').scrollTo(0, 100); \
             $('list').scrollTo({ top: 10 }); $('list').scroll(1, 2); $('list').scrollBy(0, 5); \
             [typeof $('a').scrollIntoView, $('list').scrollTop].join('|')",
        ),
        "function|0"
    );
}

#[test]
fn a_fragment_answers_queries_over_its_own_subtree() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var frag = document.createDocumentFragment(); \
             var box = document.createElement('div'); box.id = 'a'; box.className = 'in-frag'; \
             var inner = document.createElement('span'); inner.id = 'deep'; \
             box.appendChild(inner); frag.appendChild(box); \
             frag.appendChild(document.createElement('p')); \
             var template = document.createElement('template'); \
             template.innerHTML = '<li class=\"row\">x</li><li class=\"row\">y</li>'; \
             [frag.querySelector('#a') === box, frag.querySelectorAll('div, span, p').length, \
              frag.getElementById('deep') === inner, frag.getElementById('list'), \
              frag.firstElementChild === box, frag.lastElementChild.tagName, \
              frag.childElementCount, frag.children.length, \
              template.content.querySelectorAll('.row').length, \
              Array.from(template.content.querySelectorAll('li')).map(function (li) { \
                return li.textContent; }).join('')].join('|')",
        ),
        "true|3|true||true|P|2|2|2|xy",
        "the document's own `#a` and `#list` are not in the fragment"
    );
}
