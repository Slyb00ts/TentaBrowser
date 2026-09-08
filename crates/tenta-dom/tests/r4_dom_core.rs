//! Regression coverage for the round-4 DOM findings: the class/id index
//! position map (B-1), `outer_html` on empty raw-text elements (B-2),
//! document-rooted queries and connectivity (B-3), the quirks-mode folded
//! key leak on the reindex path (M-2), hidden-content extraction cost and
//! agreement (M-7, m-3), and `:scope` (M-1).

mod support;

use tenta_core::NodeId;
use tenta_core::atom::atoms;
use tenta_dom::{Document, Mutation};
use tenta_html::QualName;

fn append(document: &mut Document, parent: NodeId, child: NodeId) {
    document
        .mutate(Mutation::Append { parent, child })
        .expect("append");
}

fn remove(document: &mut Document, child: NodeId) {
    document.mutate(Mutation::Remove { child }).expect("remove");
}

fn set(document: &mut Document, element: NodeId, name: tenta_core::Atom, value: &str) {
    document
        .mutate(Mutation::SetAttribute {
            element,
            name: QualName::new(atoms::EMPTY, name),
            value: value.to_owned(),
        })
        .expect("set attribute");
}

fn ids_of(document: &Document, nodes: &[NodeId]) -> Vec<String> {
    nodes
        .iter()
        .map(|node| {
            document
                .attribute(*node, atoms::ID)
                .unwrap_or("")
                .to_owned()
        })
        .collect()
}

/// The class members a naive tree walk finds, in tree order.
fn walk_class(document: &Document, class: &str) -> Vec<NodeId> {
    document
        .descendants(document.document_node())
        .filter(|node| document.is_element(*node) && document.has_class(*node, class))
        .collect()
}

fn sorted(mut nodes: Vec<NodeId>) -> Vec<NodeId> {
    nodes.sort_by_key(|node| node.raw());
    nodes
}

/// xorshift64*: deterministic, no dependency.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 33) as usize % n
    }
}

// ------------------------------------------------------------------- B-1

#[test]
fn a_class_bucket_shrinking_below_the_position_map_threshold_stays_exact() {
    // The critic's sequence: nine members build a position map on the first
    // removal, the bucket shrinks to eight (linear path), grows again, and
    // the next removal must not read a stale position.
    let mut document = Document::new();
    let body = document.create_html_element(atoms::BODY);
    let root = document.document_node();
    append(&mut document, root, body);
    let mut members = Vec::new();
    for i in 0..9 {
        let el = document.create_html_element(atoms::DIV);
        set(&mut document, el, atoms::ID, &format!("e{i}"));
        set(&mut document, el, atoms::CLASS, "c");
        append(&mut document, body, el);
        members.push(el);
    }
    remove(&mut document, members[3]);
    let x = document.create_html_element(atoms::DIV);
    set(&mut document, x, atoms::ID, "x");
    set(&mut document, x, atoms::CLASS, "c");
    append(&mut document, body, x);
    remove(&mut document, members[7]);

    let by_index = sorted(document.get_elements_by_class_name("c"));
    let by_walk = sorted(walk_class(&document, "c"));
    assert_eq!(
        ids_of(&document, &by_index),
        ids_of(&document, &by_walk),
        "index and tree walk must agree"
    );
    assert!(by_index.contains(&x), "the live member x is listed");
    assert!(!by_index.contains(&members[7]), "the removed e7 is not");
    let by_query = sorted(
        document
            .query_selector_all(document.document_node(), ".c")
            .expect("query"),
    );
    assert_eq!(by_query, by_walk);
}

#[test]
fn random_class_membership_churn_around_the_threshold_matches_a_naive_walk() {
    let mut document = Document::new();
    let body = document.create_html_element(atoms::BODY);
    let root = document.document_node();
    append(&mut document, root, body);
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut pool: Vec<NodeId> = Vec::new();
    for step in 0..20_000 {
        match rng.below(4) {
            0 | 1 => {
                let el = document.create_html_element(atoms::SPAN);
                set(&mut document, el, atoms::ID, &format!("n{step}"));
                set(&mut document, el, atoms::CLASS, "c");
                append(&mut document, body, el);
                pool.push(el);
            }
            2 if !pool.is_empty() => {
                let victim = pool.swap_remove(rng.below(pool.len()));
                remove(&mut document, victim);
            }
            _ if !pool.is_empty() => {
                // Rewrite the class attribute: leaves and re-enters the bucket.
                let el = pool[rng.below(pool.len())];
                set(&mut document, el, atoms::CLASS, "other");
                set(&mut document, el, atoms::CLASS, "c");
            }
            _ => {}
        }
        if step % 97 == 0 {
            document.sweep_detached();
        }
        let expected = sorted(walk_class(&document, "c"));
        assert_eq!(
            sorted(document.get_elements_by_class_name("c")),
            expected,
            "index diverged from the tree at step {step}"
        );
    }
}

// ------------------------------------------------------------------- B-2

#[test]
fn outer_html_of_empty_raw_text_elements_does_not_panic() {
    for (html, expected) in [
        (
            "<!doctype html><body><textarea></textarea>",
            "<textarea></textarea>",
        ),
        ("<!doctype html><body><pre></pre>", "<pre></pre>"),
        (
            "<!doctype html><body><listing></listing>",
            "<listing></listing>",
        ),
        (
            "<!doctype html><body><textarea name=c></textarea><p>after</p>",
            "<textarea name=\"c\"></textarea><p>after</p>",
        ),
        // The tokenizer drops the first LF, so the element is empty.
        ("<!doctype html><body><pre>\n</pre>", "<pre></pre>"),
        (
            "<!doctype html><body><template><pre></pre></template>",
            "<template><pre></pre></template>",
        ),
    ] {
        let document = support::parse(html);
        let body = document
            .query_selector(document.document_node(), "body")
            .expect("query")
            .expect("body");
        let html = document.outer_html(body);
        assert_eq!(html, format!("<body>{expected}</body>"), "input {html:?}");
        // The whole-document form is what `--dump html` uses.
        let _ = document.outer_html(document.document_node());
    }
    // The non-empty case keeps its leading newline through a re-parse.
    let document = support::parse("<!doctype html><body><pre>\n\nx</pre>");
    let pre = document
        .query_selector(document.document_node(), "pre")
        .expect("query")
        .expect("pre");
    let text = document.first_child(pre);
    assert_eq!(
        document.text_data(text),
        Some("\nx"),
        "the parser drops one LF"
    );
    assert_eq!(document.outer_html(pre), "<pre>\n\nx</pre>");
}

// ------------------------------------------------------------------- B-3

#[test]
fn document_rooted_queries_only_see_connected_nodes() {
    let mut document = support::parse(
        "<!doctype html><body><p id=m class=k>m</p>\
         <template><p id=t class=k>t</p></template><div id=host></div>",
    );
    let root = document.document_node();
    let qsa = |document: &Document, root: NodeId, selector: &str| -> Vec<String> {
        let found = document.query_selector_all(root, selector).expect("query");
        ids_of(document, &found)
    };
    assert_eq!(qsa(&document, root, "p"), ["m"]);
    assert_eq!(qsa(&document, root, ".k"), ["m"]);
    assert!(
        document
            .query_selector(root, "#t")
            .expect("query")
            .is_none(),
        "template contents are not in the document"
    );
    assert!(document.get_element_by_id("t").is_none());

    // A shadow tree is not reachable from the document either.
    let host = document.get_element_by_id("host").expect("host");
    let shadow = document.attach_shadow_root(host).expect("shadow root");
    let span = document.create_html_element(atoms::SPAN);
    set(&mut document, span, atoms::ID, "s");
    set(&mut document, span, atoms::CLASS, "k");
    append(&mut document, shadow, span);
    assert_eq!(qsa(&document, root, ".k"), ["m"]);
    assert!(
        document
            .query_selector(root, "#s")
            .expect("query")
            .is_none()
    );

    // Removed but not yet swept: gone from the document's point of view.
    let m = document.get_element_by_id("m").expect("m");
    remove(&mut document, m);
    assert!(
        document
            .query_selector(root, "#m")
            .expect("query")
            .is_none()
    );
    assert!(qsa(&document, root, "p").is_empty());
    assert!(qsa(&document, root, ".k").is_empty());

    // A detached subtree is only visible from its own root.
    let detached = document.create_html_element(atoms::DIV);
    let inner = document.create_html_element(atoms::P);
    set(&mut document, inner, atoms::ID, "d");
    set(&mut document, inner, atoms::CLASS, "k");
    append(&mut document, detached, inner);
    assert!(qsa(&document, root, "p").is_empty());
    assert_eq!(qsa(&document, detached, "p"), ["d"]);

    // Re-attaching brings the node back.
    let body = document
        .query_selector(root, "body")
        .expect("query")
        .expect("body");
    append(&mut document, body, detached);
    assert_eq!(qsa(&document, root, "p.k"), ["d"]);
}

#[test]
fn a_query_from_a_removed_element_still_sees_its_own_subtree() {
    let mut document = support::parse(
        "<!doctype html><body><div id=box><p id=a class=k>a</p><p id=b class=k>b</p></div>",
    );
    let boxed = document.get_element_by_id("box").expect("box");
    remove(&mut document, boxed);
    let found = document.query_selector_all(boxed, ".k").expect("query");
    assert_eq!(ids_of(&document, &found), ["a", "b"]);
    assert!(
        document
            .query_selector_all(document.document_node(), ".k")
            .expect("query")
            .is_empty()
    );
}

// ------------------------------------------------------------------- M-1

#[test]
fn scope_matches_the_query_root_and_only_it() {
    let document = support::parse(
        "<!doctype html><body><div id=r><p id=a>a</p><p id=b><span id=c>c</span></p></div>",
    );
    let root = document.document_node();
    let r = document.get_element_by_id("r").expect("r");
    let b = document.get_element_by_id("b").expect("b");
    let c = document.get_element_by_id("c").expect("c");
    let qsa = |root: NodeId, selector: &str| {
        ids_of(
            &document,
            &document.query_selector_all(root, selector).expect("query"),
        )
    };
    assert_eq!(qsa(r, ":scope > p"), ["a", "b"]);
    assert_eq!(qsa(r, ":scope > #a"), ["a"]);
    assert_eq!(qsa(r, ":scope p"), ["a", "b"]);
    assert!(
        qsa(r, ":scope").is_empty(),
        "the root is not its own descendant"
    );
    assert_eq!(qsa(b, ":scope > span"), ["c"]);
    assert!(qsa(b, ":scope > p").is_empty());
    assert!(document.matches(r, ":scope").expect("matches"));
    assert!(document.matches(c, ":scope").expect("matches"));
    assert_eq!(document.closest(c, ":scope").expect("closest"), Some(c));
    // From the document, :scope is the root element.
    assert_eq!(
        document
            .query_selector(root, ":scope")
            .expect("query")
            .map(|node| document.tag(node)),
        Some(Some(atoms::HTML))
    );
    assert_eq!(qsa(root, ":scope > body > div"), ["r"]);
}

// ------------------------------------------------------------------- M-2

#[test]
fn quirks_mode_id_and_class_rewrites_do_not_grow_the_name_tables() {
    let mut document = Document::new();
    document.set_quirks_mode(tenta_html::QuirksMode::Quirks);
    let body = document.create_html_element(atoms::BODY);
    let root = document.document_node();
    append(&mut document, root, body);
    let el = document.create_html_element(atoms::DIV);
    append(&mut document, body, el);
    let rewrite = |document: &mut Document, round: usize| {
        set(document, el, atoms::ID, &format!("Item{round}"));
        set(
            document,
            el,
            atoms::CLASS,
            &format!("Row{round} Alt{round}"),
        );
    };
    for round in 0..64 {
        rewrite(&mut document, round);
    }
    let baseline = document.memory_report().names;
    for round in 64..20_064 {
        rewrite(&mut document, round);
    }
    let after = document.memory_report().names;
    assert!(
        after <= baseline + 4096,
        "quirks-mode name tables grew from {baseline} to {after} bytes over 20 000 rewrites"
    );
    // getElementById stays case-sensitive in quirks mode; selectors fold.
    assert_eq!(document.get_element_by_id("Item20063"), Some(el));
    assert_eq!(document.get_element_by_id("item20063"), None);
    assert_eq!(
        document.query_selector(root, "#item20063").expect("query"),
        Some(el),
        "the last id still matches case-insensitively"
    );
    assert_eq!(
        document.query_selector(root, ".ALT20063").expect("query"),
        Some(el),
        "the last class still matches case-insensitively"
    );
    assert_eq!(document.get_elements_by_class_name("ROW20063"), vec![el]);
    assert!(document.get_element_by_id("item20062").is_none());
}

// ------------------------------------------------------------------- M-7 / m-3

fn body_of(document: &Document) -> NodeId {
    document
        .query_selector(document.document_node(), "body")
        .expect("query")
        .expect("body")
}

#[test]
fn hidden_content_agrees_across_extractors_for_the_subtree_rules() {
    let cases: &[(&str, &[&str], &[&str])] = &[
        (
            "<details><summary>Sum</summary><p>Body</p></details>",
            &["Sum"],
            &["Body"],
        ),
        (
            "<details open><summary>Sum</summary><p>Body</p></details>",
            &["Sum", "Body"],
            &[],
        ),
        (
            "<details><p>Lead</p><summary>Sum</summary><p>Body</p></details>",
            &["Sum"],
            &["Lead", "Body"],
        ),
        ("<dialog><p>Modal</p></dialog>", &[], &["Modal"]),
        ("<dialog open><p>Modal</p></dialog>", &["Modal"], &[]),
        (
            "<video><p>Fallback</p></video><p>After</p>",
            &["After"],
            &["Fallback"],
        ),
        (
            "<p style=\"DISPLAY: NONE !important\">Gone</p><p>Kept</p>",
            &["Kept"],
            &["Gone"],
        ),
        (
            "<p style=\"display:none; display:block\">Shown</p>",
            &["Shown"],
            &[],
        ),
        (
            "<div aria-hidden=\" TRUE \"><p>Aria</p></div><p>Kept</p>",
            &["Kept"],
            &["Aria"],
        ),
        (
            "<div hidden=until-found><p>Found</p></div><p>Kept</p>",
            &["Kept"],
            &["Found"],
        ),
        (
            "<table><tr><td hidden>Cell</td><td>Shown</td></tr></table>",
            &["Shown"],
            &["Cell"],
        ),
    ];
    for (fragment, visible, hidden) in cases {
        let document = support::parse(&format!("<!doctype html><body>{fragment}"));
        let body = body_of(&document);
        let text = document.text();
        let markdown = document.markdown();
        let snapshot = serde_json::to_string(&document.snapshot()).expect("json");
        for word in *visible {
            assert!(
                text.contains(word),
                "text of {fragment:?} lacks {word:?}: {text:?}"
            );
            assert!(
                markdown.contains(word),
                "markdown of {fragment:?} lacks {word:?}: {markdown:?}"
            );
            assert!(
                snapshot.contains(word),
                "snapshot of {fragment:?} lacks {word:?}: {snapshot}"
            );
        }
        for word in *hidden {
            assert!(
                !text.contains(word),
                "text of {fragment:?} shows {word:?}: {text:?}"
            );
            assert!(
                !markdown.contains(word),
                "markdown of {fragment:?} shows {word:?}: {markdown:?}"
            );
            assert!(
                !snapshot.contains(word),
                "snapshot of {fragment:?} shows {word:?}: {snapshot}"
            );
        }
        // The single-node predicate agrees with the walkers.
        for node in document.descendants(body) {
            if let Some(chunk) = document.text_data(node) {
                let chunk = chunk.trim();
                if chunk.is_empty() {
                    continue;
                }
                let expect_hidden = hidden.iter().any(|word| chunk.contains(word));
                assert_eq!(
                    tenta_dom::extract::is_hidden_from_text(&document, node),
                    expect_hidden,
                    "is_hidden_from_text disagrees on {chunk:?} in {fragment:?}"
                );
            }
        }
    }
}

#[test]
fn text_of_a_deep_tree_costs_linear_time() {
    // 200 000 nested <div>x: the hidden verdict is taken once per element,
    // never by walking ancestors, so doubling the depth doubles the time.
    let build = |depth: usize| {
        let mut html = String::with_capacity(depth * 8 + 32);
        html.push_str("<!doctype html><body>");
        for _ in 0..depth {
            html.push_str("<div>x");
        }
        support::parse(&html)
    };
    let time = |document: &Document| {
        let start = std::time::Instant::now();
        let text = document.text();
        (start.elapsed(), text.len())
    };
    let small = build(50_000);
    let large = build(200_000);
    let (_, small_len) = time(&small);
    let (_, large_len) = time(&large);
    assert_eq!(large_len, 4 * small_len);
    let best = |document: &Document| (0..3).map(|_| time(document).0).min().expect("runs");
    let small_time = best(&small);
    let large_time = best(&large);
    let ratio = large_time.as_secs_f64() / small_time.as_secs_f64().max(1e-6);
    assert!(
        ratio < 8.0,
        "text() on 4x the depth took {ratio:.1}x the time ({small_time:?} vs {large_time:?})"
    );
}

// ------------------------------------------------------------------- M-6

#[test]
fn toggling_an_empty_attribute_does_not_grow_the_attribute_records() {
    // Boolean attributes carry no value bytes, so a byte-only garbage count
    // would never trigger compaction: 40 000 `hidden` toggles must not leave
    // 40 000 dead records behind (round-4 M-6).
    let mut document = Document::new();
    let body = document.create_html_element(atoms::BODY);
    let root = document.document_node();
    append(&mut document, root, body);
    let el = document.create_html_element(atoms::DIV);
    append(&mut document, body, el);
    for _ in 0..64 {
        set(&mut document, el, atoms::HIDDEN, "");
        document
            .mutate(Mutation::RemoveAttribute {
                element: el,
                name: QualName::new(atoms::EMPTY, atoms::HIDDEN),
            })
            .expect("remove");
    }
    let baseline = document.memory_report().attributes;
    for _ in 0..40_000 {
        set(&mut document, el, atoms::HIDDEN, "");
        document
            .mutate(Mutation::RemoveAttribute {
                element: el,
                name: QualName::new(atoms::EMPTY, atoms::HIDDEN),
            })
            .expect("remove");
    }
    let after = document.memory_report().attributes;
    assert!(
        after <= baseline + 64 * 1024,
        "attribute records grew from {baseline} to {after} bytes over 40 000 empty toggles"
    );
    assert!(!document.has_attribute(el, atoms::HIDDEN));
}

// ------------------------------------------------------------------- m-8

#[test]
fn a_control_in_a_disabled_fieldsets_first_legend_is_not_disabled() {
    let document = support::parse(
        "<!doctype html><body><fieldset disabled>\
         <legend><input id=in-legend></legend>\
         <legend><input id=in-second-legend></legend>\
         <input id=in-body>\
         <fieldset><legend><input id=in-nested-legend></legend></fieldset>\
         </fieldset>",
    );
    let root = document.document_node();
    let disabled = |id: &str| {
        let node = document.get_element_by_id(id).expect(id);
        document.matches(node, ":disabled").expect("matches")
    };
    assert!(!disabled("in-legend"), "the first legend is exempt");
    assert!(disabled("in-second-legend"), "only the first legend is");
    assert!(disabled("in-body"));
    assert!(
        disabled("in-nested-legend"),
        "the outer disabled fieldset still applies through a nested one's legend"
    );
    let listed: Vec<String> = ids_of(
        &document,
        &document
            .query_selector_all(root, ":disabled")
            .expect("query"),
    );
    assert!(!listed.iter().any(|id| id == "in-legend"), "{listed:?}");
}
