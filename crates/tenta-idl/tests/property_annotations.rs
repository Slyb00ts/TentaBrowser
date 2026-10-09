//! Attribute forwarding and annotated string lowering used by CSSOM.

use tenta_idl::{Error, model::Member};

#[test]
fn put_forwards_requires_a_readonly_instance_interface_attribute() {
    for declaration in [
        "[PutForwards=value] static readonly attribute Child child;",
        "[PutForwards=value] attribute Child child;",
        "[PutForwards=value] readonly attribute DOMString child;",
        "[PutForwards=value] readonly attribute Child? child;",
        "[PutForwards] readonly attribute Child child;",
    ] {
        let source = format!(
            "interface Child {{ attribute DOMString value; }}; interface Parent {{ {declaration} }};"
        );
        assert!(
            matches!(tenta_idl::parse_webidl(&source), Err(Error::Model(_))),
            "invalid forwarding must fail while lowering: {declaration}"
        );
    }
}

#[test]
fn forwarding_and_string_annotations_generate_valid_rust_for_both_engines() {
    let model = tenta_idl::parse_webidl(
        r#"
        interface Child { attribute DOMString value; };
        interface Parent {
            [SameObject, PutForwards=value] readonly attribute Child child;
            attribute [LegacyNullToEmptyString] DOMString text;
            attribute [LegacyNullToEmptyString] USVString scalarText;
            undefined accept([LegacyNullToEmptyString] DOMString text,
                             [LegacyNullToEmptyString] USVString scalarText);
        };
        dictionary Options {
            [LegacyNullToEmptyString] DOMString text;
            required [LegacyNullToEmptyString] USVString scalarText;
        };
        "#,
    )
    .unwrap();
    let parent = model.interface("Parent").unwrap();
    let Some(Member::Attribute(child)) = parent.member("child") else {
        panic!("the forwarding attribute is retained");
    };
    assert_eq!(child.put_forwards(), Some("value"));
    for name in ["text", "scalarText"] {
        let Some(Member::Attribute(attribute)) = parent.member(name) else {
            panic!("the annotated attribute is retained");
        };
        assert!(attribute.attrs.has("LegacyNullToEmptyString"));
    }
    for member in &model.dictionary("Options").unwrap().members {
        assert!(member.attrs.has("LegacyNullToEmptyString"));
    }
    for file in tenta_idl::generate(&model) {
        syn::parse_file(&file.contents)
            .unwrap_or_else(|error| panic!("{} must be valid Rust: {error}", file.name));
    }
}

#[test]
fn null_string_annotation_rejects_nonstring_nullable_and_argument_forms() {
    for annotated in [
        "[LegacyNullToEmptyString] long",
        "[LegacyNullToEmptyString] ByteString",
        "[LegacyNullToEmptyString] DOMString?",
        "[LegacyNullToEmptyString] USVString?",
        "[LegacyNullToEmptyString=Null] DOMString",
    ] {
        for source in [
            format!("interface Probe {{ attribute {annotated} value; }};"),
            format!("interface Probe {{ undefined accept({annotated} value); }};"),
            format!("dictionary Probe {{ {annotated} value; }};"),
            format!("dictionary Probe {{ required {annotated} value; }};"),
        ] {
            assert!(
                matches!(tenta_idl::parse_webidl(&source), Err(Error::Model(_))),
                "{source}"
            );
        }
    }
}

#[test]
fn throwing_primitive_getter_keeps_a_fallible_host_contract() {
    use tenta_idl::model::{FastApi, NotFastBecause};
    let model = tenta_idl::parse_webidl(
        "interface Probe { [Throws] readonly attribute unsigned long length; };",
    )
    .unwrap();
    let Some(Member::Attribute(attribute)) = model.interface("Probe").unwrap().member("length")
    else {
        panic!("missing attribute")
    };
    assert_eq!(
        attribute.fast_getter,
        FastApi::Ineligible(NotFastBecause::Throws)
    );
    let generated = tenta_idl::generate(&model);
    let ops = generated
        .iter()
        .find(|file| file.name == "probe_ops.rs")
        .unwrap();
    assert!(
        ops.contents
            .contains("fn length(&self) -> Result<u32, JsError>")
    );
}
