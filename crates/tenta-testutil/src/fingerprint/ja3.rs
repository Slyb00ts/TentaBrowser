//! JA3 (Salesforce, 2017): the MD5 of a comma-separated string of decimal
//! ClientHello fields.
//!
//! Chrome permutes its extension order per connection, so JA3 is not stable for
//! a Chrome profile. It exists here for the deterministic test mode with
//! permutation disabled and for comparison against third-party tooling
//! (`docs/spec/03-network.md` §2).

use super::hex;
use super::tls::{ClientHello, is_grease};

/// Computes the JA3 hash of a ClientHello.
#[must_use]
pub fn ja3(hello: &ClientHello) -> String {
    use md5::{Digest, Md5};
    hex(&Md5::digest(ja3_string(hello).as_bytes()))
}

/// The pre-hash JA3 string:
/// `version,ciphers,extensions,groups,ec_point_formats`, decimal and
/// `-`-separated, GREASE removed.
#[must_use]
pub fn ja3_string(hello: &ClientHello) -> String {
    let ciphers = join_decimal(&hello.ciphers_without_grease());
    let extensions = join_decimal(&hello.extensions_without_grease());
    let groups = join_decimal(
        &hello
            .supported_groups()
            .unwrap_or_default()
            .into_iter()
            .filter(|g| !is_grease(*g))
            .collect::<Vec<_>>(),
    );
    let formats = hello
        .ec_point_formats()
        .unwrap_or_default()
        .iter()
        .map(u8::to_string)
        .collect::<Vec<_>>()
        .join("-");
    format!(
        "{},{ciphers},{extensions},{groups},{formats}",
        hello.legacy_version
    )
}

fn join_decimal(values: &[u16]) -> String {
    values
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join("-")
}
