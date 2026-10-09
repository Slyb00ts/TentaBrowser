//! Identifiers in the shape Chromium uses: 32 upper-case hex digits.

use std::fmt::Write;

/// A fresh identifier from 128 bits of OS randomness, so it cannot be
/// guessed. Ids still are not credentials; the token is.
pub(crate) fn random_id() -> String {
    let mut bytes = [0_u8; 16];
    // Without an OS random source nothing here is safe to hand out.
    getrandom::fill(&mut bytes).expect("the OS random source is unavailable");
    bytes
        .iter()
        .fold(String::with_capacity(32), |mut id, byte| {
            let _ = write!(id, "{byte:02X}");
            id
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_32_hex_digits_and_distinct() {
        let a = random_id();
        let b = random_id();
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }
}
