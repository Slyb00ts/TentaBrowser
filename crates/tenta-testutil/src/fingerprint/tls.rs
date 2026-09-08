//! ClientHello parsing: TLS record layer, handshake header and every extension
//! whose contents a fingerprint profile records.

use super::{ParseError, Reader, Result, u16_list};

/// Extension code points a fingerprint profile cares about.
///
/// Values are IANA `TLS ExtensionType` registrations except where noted.
pub mod ext {
    /// `server_name` (RFC 6066).
    pub const SERVER_NAME: u16 = 0;
    /// `status_request`, OCSP stapling (RFC 6066).
    pub const STATUS_REQUEST: u16 = 5;
    /// `supported_groups` (RFC 8422, RFC 7919).
    pub const SUPPORTED_GROUPS: u16 = 10;
    /// `ec_point_formats` (RFC 8422).
    pub const EC_POINT_FORMATS: u16 = 11;
    /// `signature_algorithms` (RFC 8446).
    pub const SIGNATURE_ALGORITHMS: u16 = 13;
    /// `application_layer_protocol_negotiation` (RFC 7301).
    pub const ALPN: u16 = 16;
    /// `signed_certificate_timestamp` (RFC 6962).
    pub const SIGNED_CERTIFICATE_TIMESTAMP: u16 = 18;
    /// `padding` (RFC 7685).
    pub const PADDING: u16 = 21;
    /// `extended_master_secret` (RFC 7627).
    pub const EXTENDED_MASTER_SECRET: u16 = 23;
    /// `compress_certificate` (RFC 8879).
    pub const COMPRESS_CERTIFICATE: u16 = 27;
    /// `session_ticket` (RFC 5077).
    pub const SESSION_TICKET: u16 = 35;
    /// `pre_shared_key` (RFC 8446).
    pub const PRE_SHARED_KEY: u16 = 41;
    /// `supported_versions` (RFC 8446).
    pub const SUPPORTED_VERSIONS: u16 = 43;
    /// `psk_key_exchange_modes` (RFC 8446).
    pub const PSK_KEY_EXCHANGE_MODES: u16 = 45;
    /// `key_share` (RFC 8446).
    pub const KEY_SHARE: u16 = 51;
    /// ALPS, original code point (`draft-vvv-tls-alps`); Chrome sends this one
    /// unless it has been switched to the new one.
    pub const APPLICATION_SETTINGS: u16 = 17513;
    /// ALPS, new code point; Chrome sends both while the migration runs.
    pub const APPLICATION_SETTINGS_NEW: u16 = 17613;
    /// `encrypted_client_hello` (`draft-ietf-tls-esni`).
    pub const ENCRYPTED_CLIENT_HELLO: u16 = 65037;
    /// `renegotiation_info` (RFC 5746).
    pub const RENEGOTIATION_INFO: u16 = 0xff01;
}

/// True for a GREASE code point (`draft-ietf-tls-grease`, RFC 8701).
///
/// GREASE values are the sixteen `0x?a?a` patterns with both bytes equal. They
/// are excluded from every fingerprint list and count.
#[must_use]
pub fn is_grease(value: u16) -> bool {
    (value & 0x0f0f) == 0x0a0a && (value >> 8) == (value & 0x00ff)
}

/// One extension exactly as it appeared on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extension {
    /// The extension code point.
    pub kind: u16,
    /// The extension body, without the type and length prefix.
    pub body: Vec<u8>,
}

/// The ALPS extension: which code point carried it and which protocols it named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplicationSettings {
    /// [`ext::APPLICATION_SETTINGS`] or [`ext::APPLICATION_SETTINGS_NEW`].
    pub codepoint: u16,
    /// The advertised protocols, in wire order.
    pub protocols: Vec<Vec<u8>>,
}

impl ApplicationSettings {
    /// True when the extension used the new (17613) code point.
    #[must_use]
    pub fn uses_new_codepoint(&self) -> bool {
        self.codepoint == ext::APPLICATION_SETTINGS_NEW
    }
}

/// A parsed ClientHello.
///
/// Every list is kept in wire order and keeps its GREASE values; the fingerprint
/// algorithms filter them, so a caller can still see what the client really sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientHello {
    /// `legacy_version`, the two bytes at the top of the handshake body.
    pub legacy_version: u16,
    /// `random`, including the 32-bit timestamp of pre-TLS-1.3 clients.
    pub random: [u8; 32],
    /// `legacy_session_id`.
    pub session_id: Vec<u8>,
    /// Offered cipher suites in wire order.
    pub cipher_suites: Vec<u16>,
    /// Offered compression methods; TLS 1.3 clients send only `null` (0).
    pub compression_methods: Vec<u8>,
    /// Extensions in wire order with their raw bodies.
    pub extensions: Vec<Extension>,
}

/// Parses a ClientHello from a TLS record, a sequence of records, or a bare
/// handshake message.
///
/// A capture straight off a socket starts with a `0x16` record header; a capture
/// taken from a TLS callback starts with the `0x01` handshake type. Both are
/// accepted, and handshake fragments spread over several records are rejoined.
///
/// # Errors
///
/// Returns [`ParseError`] when the bytes are truncated, when a length prefix does
/// not match its contents, or when the message is not a ClientHello.
pub fn parse_client_hello(bytes: &[u8]) -> Result<ClientHello> {
    let handshake = if bytes.first() == Some(&0x16) {
        reassemble_handshake(bytes)?
    } else {
        bytes.to_vec()
    };

    let mut r = Reader::new(&handshake);
    let msg_type = r.u8("handshake type")?;
    if msg_type != 0x01 {
        return Err(ParseError::Unexpected {
            what: "handshake type",
            found: u32::from(msg_type),
        });
    }
    let body_len = r.u24("handshake length")? as usize;
    let body = r.take(body_len, "handshake body")?;

    let mut r = Reader::new(body);
    let legacy_version = r.u16("legacy_version")?;
    let mut random = [0u8; 32];
    random.copy_from_slice(r.take(32, "random")?);
    let session_id = r.u8_prefixed("legacy_session_id")?.to_vec();
    let cipher_suites = u16_list(r.u16_prefixed("cipher_suites")?, "cipher_suites")?;
    let compression_methods = r.u8_prefixed("legacy_compression_methods")?.to_vec();

    // Pre-TLS-1.2 hellos may stop here.
    let extensions = if r.is_empty() {
        Vec::new()
    } else {
        parse_extensions(r.u16_prefixed("extensions")?)?
    };

    Ok(ClientHello {
        legacy_version,
        random,
        session_id,
        cipher_suites,
        compression_methods,
        extensions,
    })
}

/// Concatenates the payloads of the handshake records at the front of `bytes`.
fn reassemble_handshake(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut r = Reader::new(bytes);
    let mut out = Vec::with_capacity(bytes.len());
    while r.remaining() >= 5 {
        let content_type = r.u8("record type")?;
        if content_type != 0x16 {
            break;
        }
        let _version = r.u16("record version")?;
        let payload = r.u16_prefixed("record payload")?;
        out.extend_from_slice(payload);
    }
    if out.is_empty() {
        return Err(ParseError::Missing {
            what: "handshake record",
        });
    }
    Ok(out)
}

fn parse_extensions(bytes: &[u8]) -> Result<Vec<Extension>> {
    let mut r = Reader::new(bytes);
    let mut out = Vec::new();
    while !r.is_empty() {
        let kind = r.u16("extension type")?;
        let body = r.u16_prefixed("extension body")?.to_vec();
        out.push(Extension { kind, body });
    }
    Ok(out)
}

impl ClientHello {
    /// The first extension with this code point, if any.
    #[must_use]
    pub fn extension(&self, kind: u16) -> Option<&Extension> {
        self.extensions.iter().find(|e| e.kind == kind)
    }

    /// True when the extension is present, whatever its body.
    #[must_use]
    pub fn has_extension(&self, kind: u16) -> bool {
        self.extension(kind).is_some()
    }

    /// Cipher suites with GREASE removed, in wire order.
    #[must_use]
    pub fn ciphers_without_grease(&self) -> Vec<u16> {
        self.cipher_suites
            .iter()
            .copied()
            .filter(|c| !is_grease(*c))
            .collect()
    }

    /// Extension code points with GREASE removed, in wire order.
    #[must_use]
    pub fn extensions_without_grease(&self) -> Vec<u16> {
        self.extensions
            .iter()
            .map(|e| e.kind)
            .filter(|k| !is_grease(*k))
            .collect()
    }

    /// True when the hello carries GREASE anywhere a profile would place it.
    #[must_use]
    pub fn uses_grease(&self) -> bool {
        self.cipher_suites.iter().copied().any(is_grease)
            || self.extensions.iter().any(|e| is_grease(e.kind))
            || self
                .supported_groups()
                .is_some_and(|g| g.iter().copied().any(is_grease))
            || self
                .supported_versions()
                .is_some_and(|v| v.iter().copied().any(is_grease))
    }

    /// `supported_versions` in wire order, GREASE included.
    #[must_use]
    pub fn supported_versions(&self) -> Option<Vec<u16>> {
        let body = &self.extension(ext::SUPPORTED_VERSIONS)?.body;
        let mut r = Reader::new(body);
        let list = r.u8_prefixed("supported_versions").ok()?;
        u16_list(list, "supported_versions").ok()
    }

    /// The version a fingerprint reports: the highest non-GREASE
    /// `supported_versions` entry, or `legacy_version` when the extension is
    /// absent.
    ///
    /// TLS over TCP only; DTLS numbers versions downwards.
    #[must_use]
    pub fn effective_version(&self) -> u16 {
        self.supported_versions()
            .and_then(|v| v.into_iter().filter(|x| !is_grease(*x)).max())
            .unwrap_or(self.legacy_version)
    }

    /// `supported_groups` in wire order, GREASE included.
    #[must_use]
    pub fn supported_groups(&self) -> Option<Vec<u16>> {
        let body = &self.extension(ext::SUPPORTED_GROUPS)?.body;
        let mut r = Reader::new(body);
        let list = r.u16_prefixed("supported_groups").ok()?;
        u16_list(list, "supported_groups").ok()
    }

    /// `signature_algorithms` in wire order, GREASE included.
    #[must_use]
    pub fn signature_algorithms(&self) -> Option<Vec<u16>> {
        let body = &self.extension(ext::SIGNATURE_ALGORITHMS)?.body;
        let mut r = Reader::new(body);
        let list = r.u16_prefixed("signature_algorithms").ok()?;
        u16_list(list, "signature_algorithms").ok()
    }

    /// The first `host_name` in `server_name`.
    ///
    /// Returns `None` when the extension is absent or carries no host name;
    /// use [`ClientHello::has_extension`] to distinguish "absent" from "empty".
    #[must_use]
    pub fn server_name(&self) -> Option<String> {
        let body = &self.extension(ext::SERVER_NAME)?.body;
        let mut r = Reader::new(body);
        let list = r.u16_prefixed("server_name_list").ok()?;
        let mut r = Reader::new(list);
        while !r.is_empty() {
            let name_type = r.u8("name_type").ok()?;
            let name = r.u16_prefixed("host_name").ok()?;
            if name_type == 0 {
                return String::from_utf8(name.to_vec()).ok();
            }
        }
        None
    }

    /// ALPN protocol names in wire order; they are byte strings, not text,
    /// because GREASE ALPN values are not printable.
    #[must_use]
    pub fn alpn(&self) -> Option<Vec<Vec<u8>>> {
        parse_protocol_name_list(&self.extension(ext::ALPN)?.body)
    }

    /// ALPS, from whichever of the two code points the client used.
    #[must_use]
    pub fn application_settings(&self) -> Option<ApplicationSettings> {
        for codepoint in [ext::APPLICATION_SETTINGS_NEW, ext::APPLICATION_SETTINGS] {
            if let Some(e) = self.extension(codepoint)
                && let Some(protocols) = parse_protocol_name_list(&e.body)
            {
                return Some(ApplicationSettings {
                    codepoint,
                    protocols,
                });
            }
        }
        None
    }

    /// Group ids of the `key_share` entries, in wire order; the keys themselves
    /// are not fingerprint material and are dropped.
    #[must_use]
    pub fn key_share_groups(&self) -> Option<Vec<u16>> {
        let body = &self.extension(ext::KEY_SHARE)?.body;
        let mut r = Reader::new(body);
        let shares = r.u16_prefixed("client_shares").ok()?;
        let mut r = Reader::new(shares);
        let mut out = Vec::new();
        while !r.is_empty() {
            out.push(r.u16("key_share group").ok()?);
            r.u16_prefixed("key_exchange").ok()?;
        }
        Some(out)
    }

    /// `psk_key_exchange_modes`.
    #[must_use]
    pub fn psk_key_exchange_modes(&self) -> Option<Vec<u8>> {
        let body = &self.extension(ext::PSK_KEY_EXCHANGE_MODES)?.body;
        let mut r = Reader::new(body);
        Some(r.u8_prefixed("ke_modes").ok()?.to_vec())
    }

    /// `compress_certificate` algorithm ids (RFC 8879: 1 = zlib, 2 = brotli,
    /// 3 = zstd).
    #[must_use]
    pub fn certificate_compression_algorithms(&self) -> Option<Vec<u16>> {
        let body = &self.extension(ext::COMPRESS_CERTIFICATE)?.body;
        let mut r = Reader::new(body);
        let list = r.u8_prefixed("compress_certificate").ok()?;
        u16_list(list, "compress_certificate").ok()
    }

    /// `ec_point_formats` (0 = uncompressed).
    #[must_use]
    pub fn ec_point_formats(&self) -> Option<Vec<u8>> {
        let body = &self.extension(ext::EC_POINT_FORMATS)?.body;
        let mut r = Reader::new(body);
        Some(r.u8_prefixed("ec_point_formats").ok()?.to_vec())
    }

    /// The `renegotiation_info` payload; empty for a first handshake.
    #[must_use]
    pub fn renegotiation_info(&self) -> Option<&[u8]> {
        let body = &self.extension(ext::RENEGOTIATION_INFO)?.body;
        let mut r = Reader::new(body);
        r.u8_prefixed("renegotiated_connection").ok()
    }

    /// The `session_ticket` body; empty when the client has no ticket.
    #[must_use]
    pub fn session_ticket(&self) -> Option<&[u8]> {
        Some(&self.extension(ext::SESSION_TICKET)?.body)
    }

    /// The `certificate_status_type` of `status_request` (1 = OCSP).
    #[must_use]
    pub fn status_request_type(&self) -> Option<u8> {
        self.extension(ext::STATUS_REQUEST)?.body.first().copied()
    }

    /// True when `signed_certificate_timestamp` is offered.
    #[must_use]
    pub fn signed_certificate_timestamp(&self) -> bool {
        self.has_extension(ext::SIGNED_CERTIFICATE_TIMESTAMP)
    }

    /// True when `extended_master_secret` is offered.
    #[must_use]
    pub fn extended_master_secret(&self) -> bool {
        self.has_extension(ext::EXTENDED_MASTER_SECRET)
    }

    /// The `ClientHelloType` of `encrypted_client_hello` (0 = outer, which is
    /// what GREASE ECH sends; 1 = inner).
    #[must_use]
    pub fn encrypted_client_hello(&self) -> Option<u8> {
        self.extension(ext::ENCRYPTED_CLIENT_HELLO)?
            .body
            .first()
            .copied()
    }

    /// Length of the `padding` extension body, if present.
    #[must_use]
    pub fn padding_len(&self) -> Option<usize> {
        Some(self.extension(ext::PADDING)?.body.len())
    }
}

/// Parses the `ProtocolNameList` shape shared by ALPN and ALPS: a two-byte
/// length, then one-byte-prefixed names.
fn parse_protocol_name_list(body: &[u8]) -> Option<Vec<Vec<u8>>> {
    let mut r = Reader::new(body);
    let list = r.u16_prefixed("protocol_name_list").ok()?;
    let mut r = Reader::new(list);
    let mut out = Vec::new();
    while !r.is_empty() {
        out.push(r.u8_prefixed("protocol_name").ok()?.to_vec());
    }
    Some(out)
}

/// JA4's two-character label for a TLS or DTLS version number.
#[must_use]
pub fn version_label(version: u16) -> &'static str {
    match version {
        0x0304 => "13",
        0x0303 => "12",
        0x0302 => "11",
        0x0301 => "10",
        0x0300 => "s3",
        0x0002 => "s2",
        0xfeff => "d1",
        0xfefd => "d2",
        0xfefc => "d3",
        _ => "00",
    }
}
