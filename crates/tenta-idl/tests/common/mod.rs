// Fixture loading shared by the integration tests.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use tenta_idl::Model;

/// The workspace's `idl/toy` directory.
pub fn toy_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../idl/toy")
}

/// The directory holding `dom-excerpt.webidl`.
pub fn dom_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Parses the toy fixture.
pub fn toy() -> Model {
    tenta_idl::parse_directory(&toy_dir()).expect("the toy fixture parses")
}

/// Parses the DOM excerpt fixture.
pub fn dom() -> Model {
    tenta_idl::parse_directory(&dom_dir()).expect("the DOM excerpt fixture parses")
}

/// The names of an interface's members, in definition order.
pub fn member_names(interface: &tenta_idl::Interface) -> Vec<&str> {
    interface.members.iter().map(|m| m.name()).collect()
}

/// The property enumeration order of an interface as JavaScript names.
pub fn enumeration_order(interface: &tenta_idl::Interface) -> Vec<String> {
    interface
        .property_enumeration_order()
        .iter()
        .map(|key| key.js_name().to_string())
        .collect()
}

/// The property enumeration order of a namespace as JavaScript names.
pub fn enumeration_order_of_namespace(namespace: &tenta_idl::Namespace) -> Vec<String> {
    namespace
        .property_enumeration_order()
        .iter()
        .map(|key| key.js_name().to_string())
        .collect()
}
