//! JA4 against FoxIO's published test data.
//!
//! FoxIO ships expected fingerprints for the captures in its repository
//! (`python/test/testdata/*.json` in <https://github.com/FoxIO-LLC/ja4>). Their
//! `JA4_ro` field spells out the cipher, extension and signature-algorithm lists
//! in wire order, which is exactly the input JA4 consumes, so each vector below
//! rebuilds a ClientHello carrying those lists and asserts the published `JA4`
//! and `JA4_r`.
//!
//! The pcap files themselves are not vendored: a rebuilt hello has the same
//! fingerprint-relevant fields but not the same bytes, so these tests cover the
//! algorithm, and `chrome_144_capture.rs` covers parsing real wire bytes.

use tenta_testutil::fingerprint::{
    ClientHello, Extension, ext, ja3, ja3_string, ja4, parse_client_hello,
};

/// The parts of a ClientHello a JA4 vector pins down.
struct Vector {
    /// `legacy_version` on the wire.
    legacy_version: u16,
    /// Contents of `supported_versions`, if the hello carries the extension.
    supported_versions: Option<&'static [u16]>,
    /// Cipher suites in wire order.
    ciphers: &'static [u16],
    /// Extension code points in wire order.
    extensions: &'static [u16],
    /// Contents of `signature_algorithms`.
    sigalgs: &'static [u16],
    /// ALPN protocol names in wire order.
    alpn: &'static [&'static str],
    /// The `server_name` to send; `None` means the extension is still listed but
    /// carries no host name.
    sni: Option<&'static str>,
    /// Contents of `supported_groups`, only needed by JA3.
    groups: &'static [u16],
}

impl Vector {
    /// Serialises the vector as ClientHello wire bytes inside a TLS record.
    fn to_wire(&self) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&self.legacy_version.to_be_bytes());
        body.extend_from_slice(&[0x42; 32]); // random
        body.push(32); // session id length
        body.extend_from_slice(&[0x11; 32]);
        body.extend_from_slice(&u16::try_from(self.ciphers.len() * 2).unwrap().to_be_bytes());
        for c in self.ciphers {
            body.extend_from_slice(&c.to_be_bytes());
        }
        body.extend_from_slice(&[1, 0]); // one compression method: null

        let mut extensions = Vec::new();
        for kind in self.extensions {
            let payload = self.extension_body(*kind);
            extensions.extend_from_slice(&kind.to_be_bytes());
            extensions.extend_from_slice(&u16::try_from(payload.len()).unwrap().to_be_bytes());
            extensions.extend_from_slice(&payload);
        }
        body.extend_from_slice(&u16::try_from(extensions.len()).unwrap().to_be_bytes());
        body.extend_from_slice(&extensions);

        let mut handshake = vec![0x01];
        handshake.extend_from_slice(&u32::try_from(body.len()).unwrap().to_be_bytes()[1..]);
        handshake.extend_from_slice(&body);

        let mut record = vec![0x16, 0x03, 0x01];
        record.extend_from_slice(&u16::try_from(handshake.len()).unwrap().to_be_bytes());
        record.extend_from_slice(&handshake);
        record
    }

    fn extension_body(&self, kind: u16) -> Vec<u8> {
        match kind {
            ext::SERVER_NAME => {
                let Some(host) = self.sni else {
                    return Vec::new();
                };
                let mut entry = vec![0u8];
                entry.extend_from_slice(&u16::try_from(host.len()).unwrap().to_be_bytes());
                entry.extend_from_slice(host.as_bytes());
                let mut out = u16::try_from(entry.len()).unwrap().to_be_bytes().to_vec();
                out.extend_from_slice(&entry);
                out
            }
            ext::ALPN => {
                let mut list = Vec::new();
                for name in self.alpn {
                    list.push(u8::try_from(name.len()).unwrap());
                    list.extend_from_slice(name.as_bytes());
                }
                let mut out = u16::try_from(list.len()).unwrap().to_be_bytes().to_vec();
                out.extend_from_slice(&list);
                out
            }
            ext::SIGNATURE_ALGORITHMS => u16_vector(self.sigalgs),
            ext::SUPPORTED_GROUPS => u16_vector(self.groups),
            ext::SUPPORTED_VERSIONS => {
                let versions = self.supported_versions.unwrap_or(&[]);
                let mut out = vec![u8::try_from(versions.len() * 2).unwrap()];
                for v in versions {
                    out.extend_from_slice(&v.to_be_bytes());
                }
                out
            }
            ext::EC_POINT_FORMATS => vec![1, 0],
            _ => Vec::new(),
        }
    }
}

fn u16_vector(values: &[u16]) -> Vec<u8> {
    let mut out = u16::try_from(values.len() * 2)
        .unwrap()
        .to_be_bytes()
        .to_vec();
    for v in values {
        out.extend_from_slice(&v.to_be_bytes());
    }
    out
}

fn check(vector: &Vector, expected_ja4: &str, expected_ja4_r: &str) -> ClientHello {
    let bytes = vector.to_wire();
    let hello = parse_client_hello(&bytes).expect("parse rebuilt hello");
    assert_eq!(hello.cipher_suites, vector.ciphers, "cipher round trip");
    assert_eq!(
        hello.extensions.iter().map(|e| e.kind).collect::<Vec<_>>(),
        vector.extensions,
        "extension round trip"
    );
    let fingerprint = ja4(&hello);
    assert_eq!(fingerprint.full, expected_ja4, "JA4");
    assert_eq!(fingerprint.raw(), expected_ja4_r, "JA4_r");
    hello
}

/// `pcap/tls12.pcap`: Firefox to `contile.services.mozilla.com`.
const FIREFOX_TLS12_PCAP: Vector = Vector {
    legacy_version: 0x0303,
    supported_versions: Some(&[0x0304, 0x0303]),
    ciphers: &[
        0x1301, 0x1303, 0x1302, 0xc02b, 0xc02f, 0xcca9, 0xcca8, 0xc02c, 0xc030, 0xc00a, 0xc009,
        0xc013, 0xc014, 0x009c, 0x009d, 0x002f, 0x0035,
    ],
    extensions: &[
        0x0000, 0x0017, 0xff01, 0x000a, 0x000b, 0x0023, 0x0010, 0x0005, 0x0022, 0x0033, 0x002b,
        0x000d, 0x002d, 0x001c, 0x0015,
    ],
    sigalgs: &[
        0x0403, 0x0503, 0x0603, 0x0804, 0x0805, 0x0806, 0x0401, 0x0501, 0x0601, 0x0203, 0x0201,
    ],
    alpn: &["h2", "http/1.1"],
    sni: Some("contile.services.mozilla.com"),
    groups: &[0x001d, 0x0017, 0x0018, 0x0019, 0x0100, 0x0101],
};

/// `pcap/tls-alpn-h2.pcap`: a TLS 1.2 client to `www.cloudflare.com`.
const TLS_ALPN_H2_PCAP: Vector = Vector {
    legacy_version: 0x0303,
    supported_versions: None,
    ciphers: &[
        0xc030, 0xc02c, 0xc028, 0xc024, 0xc014, 0xc00a, 0x009f, 0x006b, 0x0039, 0xcca9, 0xcca8,
        0xccaa, 0xff85, 0x00c4, 0x0088, 0x0081, 0x009d, 0x003d, 0x0035, 0x00c0, 0x0084, 0xc02f,
        0xc02b, 0xc027, 0xc023, 0xc013, 0xc009, 0x009e, 0x0067, 0x0033, 0x00be, 0x0045, 0x009c,
        0x003c, 0x002f, 0x00ba, 0x0041, 0xc011, 0xc007, 0x0005, 0x0004, 0xc012, 0xc008, 0x0016,
        0x000a, 0x00ff,
    ],
    extensions: &[0x0000, 0x000b, 0x000a, 0x000d, 0x0010],
    sigalgs: &[
        0x0601, 0x0603, 0xefef, 0x0501, 0x0503, 0x0401, 0x0403, 0xeeee, 0xeded, 0x0301, 0x0303,
        0x0201, 0x0203,
    ],
    alpn: &["h2", "http/1.1"],
    sni: Some("www.cloudflare.com"),
    groups: &[0x0017, 0x0019, 0x001c, 0x001b],
};

/// `pcap/sigalg-grease.pcapng`: Chrome to `optimizationguide-pa.googleapis.com`,
/// with ECH (`fe0d`), the new ALPS code point (`44cd`) and a GREASE extension.
const CHROME_SIGALG_GREASE_PCAPNG: Vector = Vector {
    legacy_version: 0x0303,
    supported_versions: Some(&[0x0304, 0x0303]),
    ciphers: &[
        0x1301, 0x1302, 0x1303, 0xc02b, 0xc02f, 0xc02c, 0xc030, 0xcca9, 0xcca8, 0xc013, 0xc014,
        0x009c, 0x009d, 0x002f, 0x0035,
    ],
    extensions: &[
        0xfe0d, 0x0010, 0x0017, 0x0033, 0x44cd, 0xff01, 0x000a, 0x000d, 0x0023, 0x002b, 0x000b,
        0xca34, 0x002d, 0x0000, 0x0005, 0x001b, 0x0012,
    ],
    sigalgs: &[
        0x0904, 0x0905, 0x0906, 0x0403, 0x0804, 0x0401, 0x0503, 0x0805, 0x0501, 0x0806, 0x0601,
    ],
    alpn: &["h2", "http/1.1"],
    sni: Some("optimizationguide-pa.googleapis.com"),
    groups: &[0x11ec, 0x001d, 0x0017, 0x0018],
};

/// The worked example of FoxIO's `technical_details/JA4.md`, which uses the old
/// ALPS code point (`4469`).
const JA4_SPEC_EXAMPLE: Vector = Vector {
    legacy_version: 0x0303,
    supported_versions: Some(&[0x0304, 0x0303]),
    ciphers: &[
        0x1301, 0x1302, 0x1303, 0xc02b, 0xc02f, 0xc02c, 0xc030, 0xcca9, 0xcca8, 0xc013, 0xc014,
        0x009c, 0x009d, 0x002f, 0x0035,
    ],
    extensions: &[
        0x001b, 0x0000, 0x0033, 0x0010, 0x4469, 0x0017, 0x002d, 0x000d, 0x0005, 0x0023, 0x0012,
        0x002b, 0xff01, 0x000b, 0x000a, 0x0015,
    ],
    sigalgs: &[
        0x0403, 0x0804, 0x0401, 0x0503, 0x0805, 0x0501, 0x0806, 0x0601,
    ],
    alpn: &["h2", "http/1.1"],
    sni: Some("example.com"),
    groups: &[0x001d, 0x0017, 0x0018],
};

#[test]
fn firefox_tls12_pcap_vector() {
    check(
        &FIREFOX_TLS12_PCAP,
        "t13d1715h2_5b57614c22b0_3d5424432f57",
        "t13d1715h2_002f,0035,009c,009d,1301,1302,1303,c009,c00a,c013,c014,c02b,c02c,c02f,c030,cca8,cca9_0005,000a,000b,000d,0015,0017,001c,0022,0023,002b,002d,0033,ff01_0403,0503,0603,0804,0805,0806,0401,0501,0601,0203,0201",
    );
}

#[test]
fn tls12_without_supported_versions_reports_version_12() {
    let hello = check(
        &TLS_ALPN_H2_PCAP,
        "t12d4605h2_85626a9a5f7f_aaf95bb78ec9",
        "t12d4605h2_0004,0005,000a,0016,002f,0033,0035,0039,003c,003d,0041,0045,0067,006b,0081,0084,0088,009c,009d,009e,009f,00ba,00be,00c0,00c4,00ff,c007,c008,c009,c00a,c011,c012,c013,c014,c023,c024,c027,c028,c02b,c02c,c02f,c030,cca8,cca9,ccaa,ff85_000a,000b,000d_0601,0603,efef,0501,0503,0401,0403,eeee,eded,0301,0303,0201,0203",
    );
    assert_eq!(hello.effective_version(), 0x0303);
    assert!(hello.supported_versions().is_none());
}

#[test]
fn chrome_with_ech_and_new_alps_codepoint_vector() {
    let hello = check(
        &CHROME_SIGALG_GREASE_PCAPNG,
        "t13d1517h2_8daaf6152771_cb7bf5808d99",
        "t13d1517h2_002f,0035,009c,009d,1301,1302,1303,c013,c014,c02b,c02c,c02f,c030,cca8,cca9_0005,000a,000b,000d,0012,0017,001b,0023,002b,002d,0033,44cd,ca34,fe0d,ff01_0904,0905,0906,0403,0804,0401,0503,0805,0501,0806,0601",
    );
    assert_eq!(
        hello.encrypted_client_hello(),
        None,
        "GREASE ECH body is empty here"
    );
    assert!(hello.has_extension(ext::ENCRYPTED_CLIENT_HELLO));
    assert!(hello.has_extension(ext::APPLICATION_SETTINGS_NEW));
}

#[test]
fn ja4_specification_worked_example() {
    let hello = check(
        &JA4_SPEC_EXAMPLE,
        "t13d1516h2_8daaf6152771_e5627efa2ab1",
        "t13d1516h2_002f,0035,009c,009d,1301,1302,1303,c013,c014,c02b,c02c,c02f,c030,cca8,cca9_0005,000a,000b,000d,0012,0015,0017,001b,0023,002b,002d,0033,4469,ff01_0403,0804,0401,0503,0805,0501,0806,0601",
    );
    let fingerprint = ja4(&hello);
    assert_eq!(fingerprint.a, "t13d1516h2");
    assert_eq!(fingerprint.b, "8daaf6152771");
    assert_eq!(fingerprint.c, "e5627efa2ab1");
}

#[test]
fn grease_does_not_change_ja4() {
    let plain = parse_client_hello(&JA4_SPEC_EXAMPLE.to_wire()).expect("plain");
    let mut greased = plain.clone();
    greased.cipher_suites.insert(0, 0x8a8a);
    greased.extensions.insert(
        0,
        Extension {
            kind: 0x1a1a,
            body: Vec::new(),
        },
    );
    greased.extensions.push(Extension {
        kind: 0xfafa,
        body: Vec::new(),
    });
    assert_eq!(ja4(&greased).full, ja4(&plain).full);
    assert_eq!(ja3(&greased), ja3(&plain));
}

#[test]
fn extension_permutation_changes_ja3_but_not_ja4() {
    // Chrome reorders its extensions per connection; JA4 sorts, JA3 does not.
    // This is why profiles store JA4 (docs/spec/03-network.md §2).
    let hello = parse_client_hello(&JA4_SPEC_EXAMPLE.to_wire()).expect("parse");
    let mut permuted = hello.clone();
    permuted.extensions.rotate_left(5);
    assert_eq!(ja4(&permuted).full, ja4(&hello).full);
    assert_ne!(ja3(&permuted), ja3(&hello));
}

#[test]
fn ja3_string_and_hash_for_the_spec_example() {
    // `771,ciphers,extensions,groups,formats`, decimal, GREASE removed. The hash
    // is the md5(1) of that exact string, computed outside this crate so the
    // assertion is not `ja3` checking itself.
    let hello = parse_client_hello(&JA4_SPEC_EXAMPLE.to_wire()).expect("parse");
    let expected_string = "771,4865-4866-4867-49195-49199-49196-49200-52393-52392-49171-49172-156-157-47-53,27-0-51-16-17513-23-45-13-5-35-18-43-65281-11-10-21,29-23-24,0";
    assert_eq!(ja3_string(&hello), expected_string);
    assert_eq!(ja3(&hello), "c000e2caf3a25423f9de6c8a4b12a975");
}
