//! Tests for Phase 4d-3: collections are iterable.
//!
//! `NodeList`, `HTMLCollection` and `DOMTokenList` declare `iterable<>` in the
//! DOM spec, so `for...of`, spread and array destructuring work on them.
//! Application code reaches for `for (const el of root.querySelectorAll(...))`
//! far more often than for `list.item(i)`, and a missing `@@iterator` shows up
//! as a bare `TypeError: object is not iterable` deep inside a framework.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_js_v8::Engine;

/// A document with `<html><body></body></html>`; tests build their own markup.
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
fn a_node_list_is_iterable_with_for_of() {
    assert_eq!(
        eval(
            r#"
            document.body.innerHTML = '<p id="a"></p><p id="b"></p><p id="c"></p>';
            const seen = [];
            for (const p of document.querySelectorAll('p')) {
                seen.push(p.id);
            }
            seen.join(',');
        "#
        ),
        "a,b,c"
    );
}

#[test]
fn a_node_list_spreads_and_destructures() {
    assert_eq!(
        eval(
            r#"
            document.body.innerHTML = '<p id="a"></p><p id="b"></p>';
            const list = document.querySelectorAll('p');
            const [first, second] = list;
            const ids = [...list].map((p) => p.id);
            [first.id, second.id, ids.join('+'), Array.from(list).length].join('|');
        "#
        ),
        "a|b|a+b|2"
    );
}

#[test]
fn an_html_collection_is_iterable() {
    assert_eq!(
        eval(
            r#"
            document.body.innerHTML = '<span id="x"></span><span id="y"></span>';
            const seen = [];
            for (const child of document.body.children) {
                seen.push(child.id);
            }
            seen.join(',');
        "#
        ),
        "x,y"
    );
}

#[test]
fn a_live_collection_reiterates_its_current_contents() {
    // `children` is live, so a collection taken once and spread twice reports
    // the element appended in between. This is about re-iteration; mutation
    // *during* one walk is covered by the next test.
    assert_eq!(
        eval(
            r#"
            document.body.innerHTML = '<span id="x"></span>';
            const children = document.body.children;
            const before = [...children].length;
            document.body.appendChild(document.createElement('span')).id = 'z';
            [before, [...children].map((c) => c.id).join('+')].join('|');
        "#
        ),
        "1|x+z"
    );
}

#[test]
fn growing_a_live_collection_mid_walk_is_observed_by_the_iterator() {
    // `%ArrayIteratorPrototype%.next` re-reads `length` on every step, so an
    // element appended while a `for...of` is running is still visited. The
    // walk is bounded here so a regression loops forever rather than silently
    // passing.
    assert_eq!(
        eval(
            r#"
            document.body.innerHTML = '<span id="a"></span>';
            const children = document.body.children;
            const seen = [];
            for (const child of children) {
                seen.push(child.id);
                if (seen.length === 1) {
                    document.body.appendChild(document.createElement('span')).id = 'b';
                }
                if (seen.length > 8) { break; }
            }
            seen.join(',');
        "#
        ),
        "a,b"
    );
}

#[test]
fn shrinking_a_live_collection_mid_walk_ends_the_iteration_early() {
    assert_eq!(
        eval(
            r#"
            document.body.innerHTML =
                '<span id="a"></span><span id="b"></span><span id="c"></span>';
            const children = document.body.children;
            const seen = [];
            for (const child of children) {
                seen.push(child.id);
                document.body.removeChild(document.body.lastElementChild);
                if (seen.length > 8) { break; }
            }
            seen.join(',');
        "#
        ),
        "a,b"
    );
}

#[test]
fn an_unsupported_index_is_absent_while_item_still_returns_null() {
    // The indexed getter declines, so the property is absent; the `item`
    // method keeps the WebIDL `Node?` return and reports null.
    assert_eq!(
        eval(
            r#"
            document.body.innerHTML = '<p></p>';
            const list = document.querySelectorAll('p');
            [
                typeof list[99],
                String(list.item(99)),
                99 in list,
                Object.prototype.hasOwnProperty.call(list, 99),
            ].join('|');
        "#
        ),
        "undefined|null|false|false"
    );
}

/// The indexed getter is installed without a `query` callback, so V8 answers
/// `HasProperty` from the getter alone: a value the getter returns is present,
/// a declined one (`Intercepted::kNo`) falls through to absent. That is what
/// makes `%Array.prototype%`'s `forEach` and the array iterators visit exactly
/// the in-range indices of a collection.
#[test]
fn an_in_range_index_is_present_to_the_in_operator() {
    assert_eq!(
        eval(
            r#"
            document.body.innerHTML = '<p id="a"></p><p id="b"></p>';
            const list = document.querySelectorAll('p');
            [
                0 in list,
                1 in list,
                typeof list[0],
                list[0].id,
                2 in list,
                99 in list,
            ].join('|');
        "#
        ),
        "true|true|object|a|false|false"
    );
}

/// A value iterator's `entries`, `keys`, `values` and `forEach` are
/// `%Array.prototype%`'s own function objects, installed non-enumerable, and
/// `@@iterator` is *the same function object* as `values`.
#[test]
fn a_value_iterable_installs_the_array_prototype_members() {
    assert_eq!(
        eval(
            r#"
            const list = document.querySelectorAll('p');
            const prototype = Object.getPrototypeOf(list);
            const nonEnumerable = (name) =>
                !Object.prototype.propertyIsEnumerable.call(prototype, name);
            [
                list.entries === Array.prototype.entries,
                list.keys === Array.prototype.keys,
                list.values === Array.prototype.values,
                list.forEach === Array.prototype.forEach,
                list[Symbol.iterator] === list.values,
                ['entries', 'keys', 'values', 'forEach'].every(nonEnumerable),
            ].join('|');
        "#
        ),
        "true|true|true|true|true|true"
    );
}

#[test]
fn for_each_visits_every_item_with_its_index_and_the_collection() {
    // `%Array.prototype.forEach%` receives the collection as its receiver and
    // passes it back as the callback's third argument.
    assert_eq!(
        eval(
            r#"
            document.body.innerHTML = '<p id="a"></p><p id="b"></p><p id="c"></p>';
            const list = document.querySelectorAll('p');
            const seen = [];
            let receiver = null;
            list.forEach(function (value, index, collection) {
                seen.push(index + ':' + value.id);
                receiver = collection;
            });
            [seen.join(','), receiver === list].join('|');
        "#
        ),
        "0:a,1:b,2:c|true"
    );
}

#[test]
fn keys_and_entries_yield_indices_and_pairs() {
    assert_eq!(
        eval(
            r#"
            document.body.innerHTML = '<p id="a"></p><p id="b"></p>';
            const list = document.querySelectorAll('p');
            const keyed = [...list.keys()].join(',');
            const paired = [...list.entries()]
                .map(([index, node]) => index + ':' + node.id)
                .join(',');
            [keyed, paired].join('|');
        "#
        ),
        "0,1|0:a,1:b"
    );
}

#[test]
fn the_generated_collection_members_read_the_live_collection() {
    // The members are `%Array.prototype%` functions, so they read `length` and
    // the indexed properties on every call instead of snapshotting: a mutation
    // between two calls is observed by the second one.
    assert_eq!(
        eval(
            r#"
            document.body.innerHTML = '<span id="a"></span>';
            const children = document.body.children;
            const before = [...children.values()].map((child) => child.id);
            document.body.appendChild(document.createElement('span')).id = 'b';
            [
                before.join(','),
                [...children.values()].map((child) => child.id).join(','),
            ].join('|');
        "#
        ),
        "a|a,b"
    );
}

#[test]
fn a_token_list_is_iterable_and_indexable() {
    assert_eq!(
        eval(
            r#"
            const div = document.createElement('div');
            div.className = 'one two three';
            const seen = [];
            for (const token of div.classList) {
                seen.push(token);
            }
            // `join` would render an out-of-range `undefined` as "", so the
            // absent index is reported through `typeof` instead.
            [
                seen.join(','),
                div.classList[0],
                div.classList[2],
                typeof div.classList[9],
            ].join('|');
        "#
        ),
        "one,two,three|one|three|undefined"
    );
}

#[test]
fn an_empty_collection_iterates_zero_times() {
    assert_eq!(
        eval(
            r#"
            const empty = document.querySelectorAll('article');
            let count = 0;
            for (const _ of empty) { count += 1; }
            [count, [...empty].length, [...document.createElement('div').classList].length].join('|');
        "#
        ),
        "0|0|0"
    );
}

#[test]
fn the_iterator_is_the_shared_array_values_intrinsic() {
    // The generated `@@iterator` is `%Array.prototype.values%`, so it is the
    // same function object on every collection and on plain arrays.
    assert_eq!(
        eval(
            r#"
            const list = document.querySelectorAll('p');
            const children = document.body.children;
            const tokens = document.createElement('div').classList;
            [
                list[Symbol.iterator] === Array.prototype.values,
                children[Symbol.iterator] === Array.prototype.values,
                tokens[Symbol.iterator] === Array.prototype.values,
                Object.prototype.propertyIsEnumerable.call(
                    Object.getPrototypeOf(list), Symbol.iterator),
            ].join('|');
        "#
        ),
        "true|true|true|false"
    );
}

/// An interface with an indexed getter and a `length` iterates as an array
/// does even though it declares no `iterable<>`; it gets `@@iterator` and none
/// of `forEach`, `keys`, `values` or `entries`.
#[test]
fn lists_with_an_indexed_getter_iterate_without_declaring_it() {
    assert_eq!(
        eval(
            r#"
            document.body.innerHTML =
                '<style>a { color: red } @media print { b { color: blue } }</style>' +
                '<style>c { float: left }</style><p style="color: red; float: left"></p>';
            const sheets = [...document.styleSheets];
            const kinds = [];
            for (const sheet of document.styleSheets) {
                for (const rule of sheet.cssRules) kinds.push(rule.constructor.name);
            }
            const [first] = sheets[0].cssRules;
            const media = sheets[0].cssRules[1].media;
            const style = document.querySelector('p').style;
            [
                sheets.length,
                kinds.join(','),
                first.selectorText,
                [...media].join(','),
                [...style].join(','),
                [...first.style].join(','),
                CSSRuleList.prototype[Symbol.iterator] === Array.prototype.values,
                Object.getOwnPropertyDescriptor(CSSRuleList.prototype, Symbol.iterator).enumerable,
                typeof document.styleSheets.forEach,
                typeof sheets[0].cssRules.entries,
                Array.from(sheets[1].cssRules, (rule) => rule.cssText).join(''),
            ].join('|');
        "#
        ),
        "2|CSSStyleRule,CSSMediaRule,CSSStyleRule|a|print|color,float|color|true|false|\
         undefined|undefined|c { float: left; }"
    );
}
