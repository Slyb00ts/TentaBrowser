//! Message digests from the process's one libcrypto.
//!
//! BoringSSL is linked here for TLS, and nothing else in the workspace may
//! bring a second implementation of the same primitives
//! (`docs/spec/03-network.md` §1.1). A crate above this one that has to hash
//! asks for it here instead of depending on a hashing crate of its own; Web
//! Crypto's `digest()` is the first.

use boring::hash::{Hasher, MessageDigest};

/// The digest algorithms this module computes: the SHA family Web Crypto
/// registers for `digest()`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum DigestAlgorithm {
    Sha1,
    Sha256,
    Sha384,
    Sha512,
}

impl DigestAlgorithm {
    /// Every algorithm, in the order of their output lengths.
    pub const ALL: [DigestAlgorithm; 4] = [
        DigestAlgorithm::Sha1,
        DigestAlgorithm::Sha256,
        DigestAlgorithm::Sha384,
        DigestAlgorithm::Sha512,
    ];

    /// The algorithm's registered name, as Web Crypto spells it.
    pub const fn name(self) -> &'static str {
        match self {
            DigestAlgorithm::Sha1 => "SHA-1",
            DigestAlgorithm::Sha256 => "SHA-256",
            DigestAlgorithm::Sha384 => "SHA-384",
            DigestAlgorithm::Sha512 => "SHA-512",
        }
    }

    /// The algorithm `name` registers, compared ASCII case-insensitively as
    /// Web Crypto's algorithm normalization does.
    pub fn from_name(name: &str) -> Option<DigestAlgorithm> {
        Self::ALL
            .into_iter()
            .find(|algorithm| algorithm.name().eq_ignore_ascii_case(name))
    }

    /// The length of a digest in bytes.
    pub const fn output_len(self) -> usize {
        match self {
            DigestAlgorithm::Sha1 => 20,
            DigestAlgorithm::Sha256 => 32,
            DigestAlgorithm::Sha384 => 48,
            DigestAlgorithm::Sha512 => 64,
        }
    }

    fn message_digest(self) -> MessageDigest {
        match self {
            DigestAlgorithm::Sha1 => MessageDigest::sha1(),
            DigestAlgorithm::Sha256 => MessageDigest::sha256(),
            DigestAlgorithm::Sha384 => MessageDigest::sha384(),
            DigestAlgorithm::Sha512 => MessageDigest::sha512(),
        }
    }
}

/// The library refused to hash. BoringSSL's digests fail only when an
/// allocation does, so there is nothing more to say about why.
#[derive(Clone, Copy, PartialEq, Eq, Debug, thiserror::Error)]
#[error("the digest could not be computed")]
pub struct DigestError;

/// A digest being computed over input that arrives in pieces, so that a
/// caller hashing a large buffer can look at its deadline between them.
pub struct Digest {
    hasher: Hasher,
}

impl Digest {
    pub fn new(algorithm: DigestAlgorithm) -> Result<Digest, DigestError> {
        Hasher::new(algorithm.message_digest())
            .map(|hasher| Digest { hasher })
            .map_err(|_| DigestError)
    }

    /// Feeds the next piece of the input.
    pub fn update(&mut self, bytes: &[u8]) -> Result<(), DigestError> {
        self.hasher.update(bytes).map_err(|_| DigestError)
    }

    /// The digest of everything fed so far.
    pub fn finish(mut self) -> Result<Vec<u8>, DigestError> {
        self.hasher
            .finish()
            .map(|bytes| bytes.to_vec())
            .map_err(|_| DigestError)
    }
}

/// The digest of `bytes`.
pub fn digest(algorithm: DigestAlgorithm, bytes: &[u8]) -> Result<Vec<u8>, DigestError> {
    let mut digest = Digest::new(algorithm)?;
    digest.update(bytes)?;
    digest.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    /// FIPS 180-4's own examples: the empty message and "abc".
    #[test]
    fn the_standard_vectors() {
        let cases: [(DigestAlgorithm, &str, &str); 4] = [
            (
                DigestAlgorithm::Sha1,
                "da39a3ee5e6b4b0d3255bfef95601890afd80709",
                "a9993e364706816aba3e25717850c26c9cd0d89d",
            ),
            (
                DigestAlgorithm::Sha256,
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
            (
                DigestAlgorithm::Sha384,
                "38b060a751ac96384cd9327eb1b1e36a21fdb71114be07434c0cc7bf63f6e1da\
                 274edebfe76f65fbd51ad2f14898b95b",
                "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed\
                 8086072ba1e7cc2358baeca134c825a7",
            ),
            (
                DigestAlgorithm::Sha512,
                "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce\
                 47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e",
                "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a\
                 2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
            ),
        ];
        for (algorithm, empty, abc) in cases {
            assert_eq!(
                hex(&digest(algorithm, b"").unwrap()),
                empty,
                "{algorithm:?}"
            );
            let hashed = digest(algorithm, b"abc").unwrap();
            assert_eq!(hex(&hashed), abc, "{algorithm:?}");
            assert_eq!(hashed.len(), algorithm.output_len());
        }
    }

    #[test]
    fn pieces_hash_as_the_whole() {
        let input: Vec<u8> = (0..10_000u32).map(|n| (n % 251) as u8).collect();
        for algorithm in DigestAlgorithm::ALL {
            let mut pieces = Digest::new(algorithm).unwrap();
            for piece in input.chunks(977) {
                pieces.update(piece).unwrap();
            }
            assert_eq!(
                pieces.finish().unwrap(),
                digest(algorithm, &input).unwrap(),
                "{algorithm:?}"
            );
        }
    }

    #[test]
    fn a_name_is_matched_whatever_its_ascii_case_and_nothing_looser() {
        assert_eq!(
            DigestAlgorithm::from_name("sha-256"),
            Some(DigestAlgorithm::Sha256)
        );
        assert_eq!(
            DigestAlgorithm::from_name("Sha-1"),
            Some(DigestAlgorithm::Sha1)
        );
        for name in ["SHA256", "SHA-224", " SHA-256", "\u{17f}HA-256", ""] {
            assert_eq!(DigestAlgorithm::from_name(name), None, "{name:?}");
        }
    }
}
