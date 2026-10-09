//! Style diffing: which elements changed and how.

use super::types::StyleDiff;

/// What a flush at document generation `current` reports, given the
/// generation `flushed` the styles were last computed for.
///
/// Every style is recomputed from scratch, so there is nothing finer to say
/// than "unchanged" or "everything may have changed". [`StyleDiff::Partial`]
/// needs per-element comparison against Stylo snapshots and restyle hints,
/// which the store does not carry yet, so it is never produced here.
pub(super) fn classify(flushed: Option<u64>, current: u64) -> StyleDiff {
    if flushed == Some(current) {
        StyleDiff::None
    } else {
        StyleDiff::Full
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_flushed_generation_is_unchanged() {
        assert_eq!(classify(Some(7), 7), StyleDiff::None);
        assert_eq!(classify(Some(7), 8), StyleDiff::Full);
        assert_eq!(classify(None, 7), StyleDiff::Full);
    }
}
