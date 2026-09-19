// Duplicate-member diagnostics of the WebIDL front end.
//
// Every case goes through `parse_webidl`, so the tests exercise the same
// lowering a feature crate's build script calls. The sources are merged by
// hand-written snippets rather than the fixtures so that each collision has
// exactly one cause.

mod common;

use tenta_idl::model::Member;
use tenta_idl::{Error, Model};

fn parse(source: &str) -> Model {
    tenta_idl::parse_webidl(source).expect("the source is a consistent model")
}

/// The message of the model error `source` is expected to produce.
fn model_error(source: &str) -> String {
    match tenta_idl::parse_webidl(source) {
        Err(Error::Model(message)) => message,
        other => panic!("expected a model error for {source:?}, got {other:?}"),
    }
}

fn operation<'a>(
    model: &'a Model,
    interface: &str,
    name: &str,
) -> &'a tenta_idl::model::OperationGroup {
    let definition = model
        .interface(interface)
        .unwrap_or_else(|| panic!("the model defines `{interface}`"));
    match definition.member(name) {
        Some(Member::Operation(group)) => group,
        _ => panic!("`{interface}.{name}` is an operation"),
    }
}

#[test]
fn a_duplicate_attribute_across_partials_is_rejected() {
    let message = model_error(
        "interface Widget { attribute long width; };\n\
         partial interface Widget { attribute DOMString width; };",
    );

    assert!(
        message.contains("Widget"),
        "the interface is unnamed: {message}"
    );
    assert!(
        message.contains("`width`"),
        "the attribute is unnamed: {message}"
    );
    assert!(
        message.contains("`partial interface Widget`"),
        "the contributing block is unrecorded: {message}"
    );
}

#[test]
fn a_duplicate_attribute_from_an_included_mixin_is_rejected() {
    let message = model_error(
        "interface Widget { attribute long width; };\n\
         interface mixin Sized { attribute DOMString width; };\n\
         Widget includes Sized;",
    );

    assert!(
        message.contains("Widget"),
        "the interface is unnamed: {message}"
    );
    assert!(
        message.contains("`width`"),
        "the attribute is unnamed: {message}"
    );
    assert!(
        message.contains("`mixin Sized` included by `Widget`"),
        "the mixin that contributed the duplicate is unrecorded: {message}"
    );
}

#[test]
fn two_identical_operations_are_rejected() {
    let message = model_error(
        "interface Widget {\n\
           undefined resize(long width);\n\
           undefined resize(long width);\n\
         };",
    );

    assert!(
        message.contains("Widget"),
        "the interface is unnamed: {message}"
    );
    assert!(
        message.contains("`resize`"),
        "the operation is unnamed: {message}"
    );
    assert!(
        message.contains("`(long)`"),
        "the duplicated argument list is unrecorded: {message}"
    );

    // Overload dispatch sees argument types only, so the same types under
    // different names are just as indistinguishable.
    let message = model_error(
        "interface Widget {\n\
           undefined resize(long width);\n\
           undefined resize(long height);\n\
         };",
    );
    assert!(
        message.contains("`resize`"),
        "the operation is unnamed: {message}"
    );
}

#[test]
fn genuine_overloads_are_accepted() {
    let model = parse(
        "dictionary ScrollOptions { boolean smooth = false; };\n\
         interface Widget {\n\
           undefined resize(long width);\n\
           undefined resize(DOMString selector);\n\
           undefined scroll(optional ScrollOptions options);\n\
           undefined scroll(unrestricted double x, unrestricted double y);\n\
         };",
    );

    let resize = operation(&model, "Widget", "resize");
    assert_eq!(
        resize.overloads.len(),
        2,
        "different argument types overload legally"
    );
    assert_ne!(
        resize.overloads[0].rust_name, resize.overloads[1].rust_name,
        "the two overloads are separate Rust methods"
    );

    let scroll = operation(&model, "Widget", "scroll");
    assert_eq!(
        scroll.overloads.len(),
        2,
        "different argument counts overload legally"
    );
    assert_eq!(scroll.arity_classes(), [0, 1, 2]);
}

#[test]
fn a_mixin_with_differently_named_members_still_merges() {
    let model = parse(
        "interface Widget { attribute long width; };\n\
         interface mixin Sized {\n\
           readonly attribute long height;\n\
           undefined measure();\n\
         };\n\
         Widget includes Sized;",
    );

    let widget = model.interface("Widget").expect("the Widget interface");
    assert_eq!(widget.included_mixins, ["Sized"]);
    assert_eq!(
        common::member_names(widget),
        ["width", "height", "measure"],
        "the mixin's members are appended after the interface's own"
    );
}
