//! Shadow trees in the document model: attachment, connectedness across the
//! shadow boundary, slot assignment and the flat tree extraction reads.

use tenta_core::atom::atoms;
use tenta_core::{Atom, NodeId};
use tenta_dom::{Document, LifecycleRecord, Mutation, ShadowRootMode, extract};
use tenta_html::{ParseStatus, Parser, ParserOptions, QualName};

fn parse(html: &str) -> Document {
    let document = Document::with_url("https://example.com/".parse().expect("the test URL"));
    let mut parser = Parser::new(document, ParserOptions::default());
    assert_eq!(parser.feed(html.as_bytes()), ParseStatus::Continue);
    parser.finish()
}

fn element(doc: &mut Document, name: &str, attributes: &[(&str, &str)]) -> NodeId {
    let node = doc.create_html_element(Atom::from(name));
    for (name, value) in attributes {
        doc.mutate(Mutation::SetAttribute {
            element: node,
            name: QualName::new(atoms::EMPTY, Atom::from(*name)),
            value: (*value).to_owned(),
        })
        .expect("set an attribute");
    }
    node
}

fn text(doc: &mut Document, parent: NodeId, data: &str) -> NodeId {
    let node = doc.create_text(data);
    append(doc, parent, node);
    node
}

fn append(doc: &mut Document, parent: NodeId, child: NodeId) {
    doc.mutate(Mutation::Append { parent, child })
        .expect("append");
}

#[test]
fn a_shadow_root_has_a_mode_and_a_host_and_is_not_in_the_document_tree() {
    let mut doc = parse("<!doctype html><body><div id=a></div><div id=b></div>");
    let a = doc.get_element_by_id("a").expect("a");
    let b = doc.get_element_by_id("b").expect("b");
    let open = doc.attach_shadow_root(a).expect("attach");
    let closed = doc
        .attach_shadow_root_with_mode(b, ShadowRootMode::Closed)
        .expect("attach closed");
    assert_eq!(doc.shadow_root_mode(open), Some(ShadowRootMode::Open));
    assert_eq!(doc.shadow_root_mode(closed), Some(ShadowRootMode::Closed));
    assert_eq!(doc.shadow_root_mode(a), None);
    assert_eq!(doc.shadow_host(open), Some(a));
    assert_eq!(doc.shadow_host(a), None);

    let inner = element(&mut doc, "span", &[("id", "inner")]);
    append(&mut doc, open, inner);
    assert_eq!(doc.containing_shadow_root(inner), Some(open));
    assert_eq!(doc.containing_shadow_root(a), None);
    // Connected, but not in the document tree: the id index does not see it.
    assert!(doc.is_composed_connected(inner));
    assert!(!doc.is_connected(inner));
    assert_eq!(doc.get_element_by_id("inner"), None);

    doc.mutate(Mutation::Remove { child: a }).expect("remove");
    assert!(!doc.is_composed_connected(inner));
    assert!(!doc.is_composed_connected(a));
}

#[test]
fn slots_take_the_host_children_that_name_them() {
    let mut doc = parse(
        "<!doctype html><body><div id=host>lead<b slot=title>T</b><i>one</i>\
         <u slot=missing>lost</u><i>two</i></div>",
    );
    let host = doc.get_element_by_id("host").expect("host");
    let root = doc.attach_shadow_root(host).expect("attach");
    let title = element(&mut doc, "slot", &[("name", "title")]);
    let default = element(&mut doc, "slot", &[]);
    let second_default = element(&mut doc, "slot", &[]);
    let empty = element(&mut doc, "slot", &[("name", "empty")]);
    for slot in [title, default, second_default, empty] {
        append(&mut doc, root, slot);
    }
    let children: Vec<NodeId> = {
        let mut out = Vec::new();
        let mut child = doc.first_child(host);
        while child.is_some() {
            out.push(child);
            child = doc.next_sibling(child);
        }
        out
    };
    let (lead, b, one, lost, two) = (
        children[0],
        children[1],
        children[2],
        children[3],
        children[4],
    );
    assert_eq!(doc.assigned_nodes(title), vec![b]);
    assert_eq!(doc.assigned_nodes(default), vec![lead, one, two]);
    // Only the first slot of a name is assigned to.
    assert!(doc.assigned_nodes(second_default).is_empty());
    assert!(doc.assigned_nodes(empty).is_empty());
    assert_eq!(doc.assigned_slot(b), Some(title));
    assert_eq!(doc.assigned_slot(one), Some(default));
    assert_eq!(doc.assigned_slot(lost), None);
    assert_eq!(doc.assigned_slot(host), None);
    // A slot outside a shadow tree assigns nothing.
    let stray = element(&mut doc, "slot", &[]);
    append(&mut doc, host, stray);
    assert!(doc.assigned_nodes(stray).is_empty());
}

#[test]
fn extraction_reads_the_flat_tree() {
    let mut doc = parse(
        "<!doctype html><body><p>before</p>\
         <div id=host><span slot=name>Ada</span><em>unslotted</em></div><p>after</p>",
    );
    let host = doc.get_element_by_id("host").expect("host");
    let root = doc.attach_shadow_root(host).expect("attach");
    let heading = element(&mut doc, "h2", &[]);
    append(&mut doc, root, heading);
    text(&mut doc, heading, "Hello, ");
    let slot = element(&mut doc, "slot", &[("name", "name")]);
    append(&mut doc, heading, slot);
    text(&mut doc, slot, "nobody");
    let fallback = element(&mut doc, "slot", &[("name", "role")]);
    append(&mut doc, root, fallback);
    text(&mut doc, fallback, "guest");

    let body = doc.body().expect("body");
    let extracted = extract::text(&doc, body);
    let lines: Vec<&str> = extracted.lines().map(str::trim).collect();
    assert_eq!(lines, ["before", "Hello, Ada", "guest", "after"]);
    // The node tree is unchanged: the host's own text is its light children.
    assert_eq!(doc.text_content(host), "Adaunslotted");
}

#[test]
fn the_lifecycle_log_follows_elements_into_shadow_trees() {
    let mut doc = parse("<!doctype html><body><div id=host></div>");
    doc.enable_lifecycle_log();
    let host = doc.get_element_by_id("host").expect("host");
    let root = doc.attach_shadow_root(host).expect("attach");
    let card = element(&mut doc, "x-card", &[]);
    let card_ref = doc.node_ref(card);
    doc.take_lifecycle_log();

    append(&mut doc, root, card);
    assert_eq!(
        doc.take_lifecycle_log(),
        vec![LifecycleRecord::Connected(card_ref)]
    );
    doc.mutate(Mutation::Remove { child: card })
        .expect("remove from the shadow tree");
    assert_eq!(
        doc.take_lifecycle_log(),
        vec![LifecycleRecord::Disconnected(card_ref)]
    );

    // A shadow tree whose host is not connected connects nothing.
    doc.mutate(Mutation::Remove { child: host })
        .expect("remove the host");
    doc.take_lifecycle_log();
    append(&mut doc, root, card);
    assert!(doc.take_lifecycle_log().is_empty());

    // The shadow-including subtree lists a shadow tree before the children.
    let light = element(&mut doc, "p", &[]);
    append(&mut doc, host, light);
    assert_eq!(
        doc.shadow_including_subtree(host),
        vec![host, root, card, light]
    );
}

fn parse_declaring(html: &str) -> Document {
    let document = Document::with_url("https://example.com/".parse().expect("the test URL"));
    let mut parser = Parser::new(
        document,
        ParserOptions {
            declarative_shadow_roots: true,
            ..ParserOptions::default()
        },
    );
    assert_eq!(parser.feed(html.as_bytes()), ParseStatus::Continue);
    parser.finish()
}

#[test]
fn a_template_with_a_shadow_root_mode_becomes_its_parents_shadow_root() {
    let doc = parse_declaring(
        "<!doctype html><body><div id=host>\
         <template shadowrootmode=open><p id=inner>in the shadow <slot></slot></p></template>\
         <b id=light>light</b></div>\
         <section id=sealed><template shadowrootmode=CLOSED><i>x</i></template></section>",
    );
    let host = doc.get_element_by_id("host").expect("host");
    let root = doc.shadow_root(host).expect("a declared shadow root");
    assert_eq!(doc.shadow_root_mode(root), Some(ShadowRootMode::Open));
    // The template is not in the tree: the host's only child is the light one.
    let light = doc.get_element_by_id("light").expect("light");
    assert_eq!(doc.children(host).collect::<Vec<_>>(), [light]);
    assert!(doc.get_elements_by_tag_name("template").is_empty());
    let inner = doc.first_child(root);
    assert_eq!(doc.attribute(inner, atoms::ID), Some("inner"));
    assert_eq!(doc.containing_shadow_root(inner), Some(root));
    // The light child is slotted into the declared tree.
    assert!(extract::text(&doc, host).contains("in the shadow light"));

    let sealed = doc.get_element_by_id("sealed").expect("sealed");
    let closed = doc.shadow_root(sealed).expect("a closed root");
    assert_eq!(doc.shadow_root_mode(closed), Some(ShadowRootMode::Closed));
}

#[test]
fn a_template_that_cannot_declare_a_shadow_root_stays_a_template() {
    // Not a valid host, an unknown mode, and a second declaration on one host.
    let doc = parse_declaring(
        "<!doctype html><body><ul id=list><template shadowrootmode=open><li>x</li></template></ul>\
         <div id=odd><template shadowrootmode=ajar>y</template></div>\
         <div id=twice><template shadowrootmode=open>first</template>\
         <template shadowrootmode=open>second</template></div>",
    );
    let list = doc.get_element_by_id("list").expect("list");
    assert_eq!(doc.shadow_root(list), None);
    let odd = doc.get_element_by_id("odd").expect("odd");
    assert_eq!(doc.shadow_root(odd), None);
    let twice = doc.get_element_by_id("twice").expect("twice");
    let root = doc.shadow_root(twice).expect("the first declaration");
    assert_eq!(doc.text_data(doc.first_child(root)), Some("first"));
    let templates = doc.get_elements_by_tag_name("template");
    assert_eq!(templates.len(), 3);
    assert_eq!(doc.parent(templates[2]), twice);
    let contents = doc.template_contents(templates[2]).expect("contents");
    assert_eq!(doc.text_data(doc.first_child(contents)), Some("second"));

    // Without the option nothing is declared: `innerHTML` and `DOMParser`.
    let doc = parse("<!doctype html><body><div id=host><template shadowrootmode=open>x</template>");
    let host = doc.get_element_by_id("host").expect("host");
    assert_eq!(doc.shadow_root(host), None);
    assert_eq!(doc.get_elements_by_tag_name("template").len(), 1);
}

#[test]
fn a_declared_shadow_root_is_claimed_once_and_emptied() {
    let mut doc = parse_declaring(
        "<!doctype html><body><div id=host><template shadowrootmode=open><p>server</p></template></div>",
    );
    let host = doc.get_element_by_id("host").expect("host");
    let root = doc.shadow_root(host).expect("declared");
    assert_eq!(
        doc.claim_declarative_shadow_root(host, ShadowRootMode::Closed),
        None,
        "the other mode does not claim it"
    );
    assert!(doc.first_child(root).is_some());
    assert_eq!(
        doc.claim_declarative_shadow_root(host, ShadowRootMode::Open),
        Some(root)
    );
    assert!(!doc.first_child(root).is_some());
    assert_eq!(
        doc.claim_declarative_shadow_root(host, ShadowRootMode::Open),
        None,
        "claimed roots are ordinary ones"
    );
}
