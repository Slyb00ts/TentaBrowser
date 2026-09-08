//! Browser fingerprint profiles (`docs/spec/03-network.md` §2).
//!
//! A profile is data: `profiles/<browser>_<version>_<os>.toml` describes every
//! observable wire property of a browser — the TLS ClientHello, the HTTP/2
//! preface and the request headers with their order and casing. The transport
//! reads it; nothing in the transport knows a browser's name.

use std::fmt;
use std::path::Path;
use std::sync::Arc;

use serde::Deserialize;
use tenta_h2::fingerprint::{H2Fingerprint, HeadersPriority, PseudoId};

/// Profiles compiled into the library so a client needs no filesystem access.
pub const BUILTIN_PROFILES: &[(&str, &str)] = &[
    (
        "chrome_144_macos",
        include_str!("../../../profiles/chrome_144_macos.toml"),
    ),
    (
        "chrome_144_linux",
        include_str!("../../../profiles/chrome_144_linux.toml"),
    ),
    (
        "chrome_144_windows",
        include_str!("../../../profiles/chrome_144_windows.toml"),
    ),
];

/// What went wrong while loading a profile.
#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("no built-in profile named {0:?}")]
    UnknownBuiltin(String),
    #[error("reading {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("parsing {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: toml::de::Error,
    },
    #[error("profile {name}: {reason}")]
    Invalid { name: String, reason: String },
}

/// Which wire image a client presents.
#[derive(Clone, Default)]
pub enum Profile {
    /// No fingerprint: library defaults (BoringSSL's own ClientHello, stock
    /// `h2` framing, headers in the order the caller supplied them). Used by
    /// WPT, fixtures and non-stealth scraping.
    #[default]
    None,
    /// A browser profile.
    Browser(Arc<ClientProfile>),
}

impl Profile {
    /// The profile's name, or `"none"`.
    pub fn name(&self) -> &str {
        match self {
            Profile::None => "none",
            Profile::Browser(profile) => &profile.meta.name,
        }
    }

    pub fn client_profile(&self) -> Option<&ClientProfile> {
        match self {
            Profile::None => None,
            Profile::Browser(profile) => Some(profile),
        }
    }
}

impl fmt::Debug for Profile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Profile({})", self.name())
    }
}

impl PartialEq for Profile {
    fn eq(&self, other: &Self) -> bool {
        self.name() == other.name()
    }
}

impl Eq for Profile {}

impl From<ClientProfile> for Profile {
    fn from(profile: ClientProfile) -> Self {
        Profile::Browser(Arc::new(profile))
    }
}

/// A complete browser profile.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ClientProfile {
    pub meta: Meta,
    pub tls: TlsProfile,
    pub h2: H2Profile,
    pub headers: HeaderProfile,
}

/// Identity of the profile and the fingerprints it is expected to produce.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Meta {
    pub name: String,
    pub browser: String,
    pub version: String,
    pub os: String,
    /// Measured JA4 of the ClientHello this profile produces. Empty until a
    /// capture from the real browser has been taken.
    #[serde(default)]
    pub ja4: String,
    /// Expected Akamai HTTP/2 fingerprint string. Derived from `[h2]`, stored
    /// so a typo in the settings list fails the conformance test.
    #[serde(default)]
    pub akamai_h2: String,
    /// `JA4_r`: the same fingerprint with the cipher and extension lists left
    /// unhashed, so a JA4 mismatch says which list moved.
    #[serde(default)]
    pub ja4_r: String,
    /// ISO date the capture behind this profile was taken.
    #[serde(default)]
    pub captured: String,
    /// Full version string the captured browser reported, e.g.
    /// `Google Chrome 144.0.7559.60`.
    #[serde(default)]
    pub chrome_build: String,
}

/// The ClientHello.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TlsProfile {
    /// Cipher suites in the browser's order, as IANA names or `0x1301`-style
    /// identifiers. GREASE values are not listed; see `grease`.
    pub ciphers: Vec<String>,
    /// The canonical extension list (identifiers) the browser sends, in its
    /// unpermuted order. Used by the fingerprint conformance test.
    pub extensions: Vec<u16>,
    /// Chrome ≥ 110 shuffles the extension order per connection.
    #[serde(default)]
    pub permute_extensions: bool,
    /// Supported groups in order, as BoringSSL group names.
    pub groups: Vec<String>,
    /// Signature algorithms in order, as BoringSSL sigalg names.
    pub sigalgs: Vec<String>,
    /// ALPN protocol list in order.
    pub alpn: Vec<String>,
    /// Protocols offered through ALPS (extension 17513).
    #[serde(default)]
    pub alps: Vec<String>,
    /// Whether ALPS uses the new codepoint (17613) instead of 17513.
    #[serde(default)]
    pub alps_new_codepoint: bool,
    /// Certificate compression algorithms to advertise (`brotli`, `zlib`, `zstd`).
    #[serde(default)]
    pub cert_compression: Vec<String>,
    /// Whether GREASE values are inserted (RFC 8701).
    #[serde(default)]
    pub grease: bool,
    /// Whether to ask for OCSP stapling (`status_request`).
    #[serde(default)]
    pub ocsp_stapling: bool,
    /// Whether to ask for signed certificate timestamps.
    #[serde(default)]
    pub signed_cert_timestamps: bool,
    /// Whether TLS session tickets and resumption are enabled.
    #[serde(default)]
    pub session_tickets: bool,
    /// Whether to send the `encrypted_client_hello` extension (65037). Chrome
    /// sends a GREASE ECH on every TLS 1.3 connection.
    #[serde(default)]
    pub ech: bool,
}

/// The HTTP/2 preface.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct H2Profile {
    /// SETTINGS identifiers and values in the order they are sent.
    pub settings: Vec<(u16, u32)>,
    /// Connection-level WINDOW_UPDATE increment sent after the preface.
    #[serde(default)]
    pub window_update: Option<u32>,
    /// Request pseudo-header order, e.g. `["method", "authority", "scheme", "path"]`.
    pub pseudo_header_order: Vec<String>,
    /// Priority fields written into request HEADERS frames.
    #[serde(default)]
    pub headers_priority: Option<ProfileHeadersPriority>,
    /// Header names the HPACK encoder must never index.
    #[serde(default)]
    pub never_index: Vec<String>,
    /// When the HPACK encoder Huffman-codes a literal: `"always"` (RFC 7541's
    /// usual choice and stock `h2`) or `"when_shorter"` (Chromium, which
    /// leaves a literal plain unless Huffman saves an octet).
    #[serde(default)]
    pub huffman: HuffmanLiterals,
}

/// `[h2] huffman`.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HuffmanLiterals {
    #[default]
    Always,
    WhenShorter,
}

/// `[h2.headers_priority]`.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProfileHeadersPriority {
    pub exclusive: bool,
    pub dep: u32,
    /// Wire weight (RFC 7540 weight minus one); Chrome sends 255.
    pub weight: u8,
}

/// Request headers: order, casing and the values a browser always sends.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HeaderProfile {
    /// Header names of a *navigation*, in the order the browser sends them and
    /// with the casing it uses on HTTP/1.1. HTTP/2 lowercases them.
    pub order: Vec<String>,
    /// The same for a sub-resource, which Chrome orders differently: the
    /// client hints move around `User-Agent` and the navigation-only names are
    /// gone. Empty means "use `order`".
    #[serde(default)]
    pub subresource_order: Vec<String>,
    pub user_agent: String,
    pub accept: String,
    pub accept_language: String,
    pub accept_encoding: String,
    /// What the browser offers on an origin that is not potentially
    /// trustworthy, where Chrome drops the content codings it only advertises
    /// over TLS. Empty means "same as `accept_encoding`".
    #[serde(default)]
    pub insecure_accept_encoding: String,
    #[serde(default)]
    pub sec_ch_ua: String,
    #[serde(default)]
    pub sec_ch_ua_mobile: String,
    #[serde(default)]
    pub sec_ch_ua_platform: String,
    #[serde(default)]
    pub upgrade_insecure_requests: String,
    #[serde(default)]
    pub sec_fetch_site: String,
    #[serde(default)]
    pub sec_fetch_mode: String,
    #[serde(default)]
    pub sec_fetch_user: String,
    #[serde(default)]
    pub sec_fetch_dest: String,
    /// `Sec-Fetch-Storage-Access`, which Chrome 144 sends on cross-site
    /// sub-resources that have no storage access.
    #[serde(default)]
    pub sec_fetch_storage_access: String,
    /// The RFC 9218 `Priority` header per resource type, sent on HTTP/2 only.
    #[serde(default)]
    pub priority: PriorityProfile,
}

/// `[headers.priority]`: the `priority` header value per resource type.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PriorityProfile {
    #[serde(default)]
    pub document: String,
    #[serde(default)]
    pub stylesheet: String,
    #[serde(default)]
    pub script: String,
    #[serde(default)]
    pub blocking_script: String,
    #[serde(default)]
    pub font: String,
    #[serde(default)]
    pub image: String,
    #[serde(default)]
    pub media: String,
    #[serde(default)]
    pub xhr: String,
    #[serde(default)]
    pub fetch: String,
    #[serde(default)]
    pub prefetch: String,
    #[serde(default)]
    pub other: String,
}

impl PriorityProfile {
    /// The value for one resource type, or `""` when the profile has none.
    pub fn for_resource(&self, resource_type: crate::request::ResourceType) -> &str {
        use crate::request::ResourceType;
        match resource_type {
            ResourceType::Document => &self.document,
            ResourceType::Stylesheet => &self.stylesheet,
            ResourceType::Script => &self.script,
            ResourceType::BlockingScript => &self.blocking_script,
            ResourceType::Font => &self.font,
            ResourceType::Image => &self.image,
            ResourceType::Media => &self.media,
            ResourceType::Xhr => &self.xhr,
            ResourceType::Fetch => &self.fetch,
            ResourceType::Prefetch => &self.prefetch,
            ResourceType::Other => &self.other,
        }
    }
}

impl HeaderProfile {
    /// The wire order for a request of this kind.
    pub fn order_for(&self, resource_type: crate::request::ResourceType) -> &[String] {
        if resource_type == crate::request::ResourceType::Document
            || self.subresource_order.is_empty()
        {
            &self.order
        } else {
            &self.subresource_order
        }
    }
}

impl ClientProfile {
    /// Loads one of the profiles compiled into the library.
    pub fn builtin(name: &str) -> Result<Self, ProfileError> {
        let (_, text) = BUILTIN_PROFILES
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .ok_or_else(|| ProfileError::UnknownBuiltin(name.to_owned()))?;
        Self::from_toml(text, name)
    }

    /// Reads a profile from a TOML file.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ProfileError> {
        let path = path.as_ref();
        let display = path.display().to_string();
        let text = std::fs::read_to_string(path).map_err(|source| ProfileError::Io {
            path: display.clone(),
            source,
        })?;
        Self::from_toml(&text, &display)
    }

    /// Parses a profile from TOML text. `origin` only appears in errors.
    pub fn from_toml(text: &str, origin: &str) -> Result<Self, ProfileError> {
        let profile: ClientProfile =
            toml::from_str(text).map_err(|source| ProfileError::Parse {
                path: origin.to_owned(),
                source,
            })?;
        profile.validate()?;
        Ok(profile)
    }

    fn validate(&self) -> Result<(), ProfileError> {
        let invalid = |reason: String| ProfileError::Invalid {
            name: self.meta.name.clone(),
            reason,
        };
        if self.tls.ciphers.is_empty() {
            return Err(invalid("[tls] ciphers is empty".into()));
        }
        if self.tls.alpn.is_empty() {
            return Err(invalid("[tls] alpn is empty".into()));
        }
        for cipher in &self.tls.ciphers {
            cipher_id(cipher).ok_or_else(|| invalid(format!("unknown cipher {cipher:?}")))?;
        }
        self.pseudo_order()?;
        // `H2Fingerprint::with_settings` asserts these ranges, and it runs
        // inside the connection task where a panic is a lost connection rather
        // than a load error.
        for (id, value) in &self.h2.settings {
            let limit = match id {
                // SETTINGS_ENABLE_PUSH and SETTINGS_ENABLE_CONNECT_PROTOCOL
                // are flags.
                2 | 8 => 1,
                // SETTINGS_INITIAL_WINDOW_SIZE (RFC 9113 §6.5.2).
                4 => i32::MAX as u32,
                _ => u32::MAX,
            };
            if *value > limit {
                return Err(invalid(format!(
                    "[h2] setting {id} value {value} is above the protocol maximum {limit}"
                )));
            }
            // SETTINGS_MAX_FRAME_SIZE has a floor as well.
            if *id == 5 && !(16_384..=16_777_215).contains(value) {
                return Err(invalid(format!(
                    "[h2] SETTINGS_MAX_FRAME_SIZE {value} is outside 16384..=16777215"
                )));
            }
        }
        if let Some(window) = self.h2.window_update
            && window > i32::MAX as u32
        {
            return Err(invalid(format!(
                "[h2] window_update {window} is above the protocol maximum"
            )));
        }
        if !self.meta.akamai_h2.is_empty() && self.meta.akamai_h2 != self.akamai_h2_string() {
            return Err(invalid(format!(
                "[meta] akamai_h2 does not match [h2]: stored {:?}, derived {:?}",
                self.meta.akamai_h2,
                self.akamai_h2_string()
            )));
        }
        Ok(())
    }

    fn pseudo_order(&self) -> Result<[PseudoId; 4], ProfileError> {
        let invalid = |reason: String| ProfileError::Invalid {
            name: self.meta.name.clone(),
            reason,
        };
        if self.h2.pseudo_header_order.len() != 4 {
            return Err(invalid(
                "[h2] pseudo_header_order needs exactly four entries".into(),
            ));
        }
        let mut order = [PseudoId::Method; 4];
        for (slot, name) in order.iter_mut().zip(&self.h2.pseudo_header_order) {
            *slot = match name.trim_start_matches(':') {
                "method" => PseudoId::Method,
                "authority" => PseudoId::Authority,
                "scheme" => PseudoId::Scheme,
                "path" => PseudoId::Path,
                other => return Err(invalid(format!("unknown pseudo-header {other:?}"))),
            };
        }
        for id in [
            PseudoId::Method,
            PseudoId::Authority,
            PseudoId::Scheme,
            PseudoId::Path,
        ] {
            if !order.contains(&id) {
                return Err(invalid(format!(
                    "[h2] pseudo_header_order is not a permutation, {id:?} is missing"
                )));
            }
        }
        Ok(order)
    }

    /// The HTTP/2 fingerprint this profile drives.
    pub fn h2_fingerprint(&self) -> H2Fingerprint {
        let order = self
            .pseudo_order()
            .expect("profile was validated at construction");
        let never_index: Vec<http::HeaderName> = self
            .h2
            .never_index
            .iter()
            .filter_map(|name| http::HeaderName::try_from(name.as_str()).ok())
            .collect();
        H2Fingerprint::upstream()
            .with_settings(self.h2.settings.clone())
            .with_window_update(self.h2.window_update)
            .with_pseudo_order(order)
            .with_headers_priority(self.h2.headers_priority.map(|p| HeadersPriority {
                exclusive: p.exclusive,
                dependency: p.dep,
                weight: p.weight,
            }))
            .with_never_index(never_index)
            .with_huffman(match self.h2.huffman {
                HuffmanLiterals::Always => tenta_h2::fingerprint::HuffmanPolicy::Always,
                HuffmanLiterals::WhenShorter => tenta_h2::fingerprint::HuffmanPolicy::WhenShorter,
            })
    }

    /// The Akamai HTTP/2 fingerprint string derived from `[h2]`:
    /// `settings|window_update|priority_frames|pseudo_header_order`.
    pub fn akamai_h2_string(&self) -> String {
        let settings = self
            .h2
            .settings
            .iter()
            .map(|(id, value)| format!("{id}:{value}"))
            .collect::<Vec<_>>()
            .join(";");
        let window_update = self.h2.window_update.unwrap_or(0);
        // Chrome sends no PRIORITY frames; the priority information rides in
        // HEADERS, which the Akamai string does not cover.
        let priority_frames = "0";
        let pseudo = self
            .h2
            .pseudo_header_order
            .iter()
            .map(|name| {
                name.trim_start_matches(':')
                    .chars()
                    .next()
                    .map(String::from)
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join(",");
        format!("{settings}|{window_update}|{priority_frames}|{pseudo}")
    }

    /// The cipher suite identifiers in profile order.
    pub fn cipher_ids(&self) -> Vec<u16> {
        self.tls
            .ciphers
            .iter()
            .filter_map(|name| cipher_id(name))
            .collect()
    }

    /// The BoringSSL cipher list string for the suites below TLS 1.3.
    ///
    /// TLS 1.3 suites are not configurable through `SSL_CTX_set_cipher_list`
    /// in BoringSSL; it always offers AES-128-GCM, AES-256-GCM and
    /// ChaCha20-Poly1305 in that order, which is Chrome's order.
    pub fn boring_cipher_list(&self) -> String {
        self.cipher_ids()
            .iter()
            .filter_map(|id| boring_cipher_name(*id))
            .collect::<Vec<_>>()
            .join(":")
    }
}

/// Maps an IANA cipher name or `0x1301`-style identifier to its number.
pub fn cipher_id(name: &str) -> Option<u16> {
    let name = name.trim();
    if let Some(hex) = name.strip_prefix("0x").or_else(|| name.strip_prefix("0X")) {
        return u16::from_str_radix(hex, 16).ok();
    }
    CIPHERS
        .iter()
        .find(|(_, iana, _)| *iana == name)
        .map(|(id, _, _)| *id)
}

/// The BoringSSL/OpenSSL name of a cipher suite, if it has one below TLS 1.3.
fn boring_cipher_name(id: u16) -> Option<&'static str> {
    CIPHERS
        .iter()
        .find(|(candidate, _, _)| *candidate == id)
        .and_then(|(_, _, openssl)| (!openssl.is_empty()).then_some(*openssl))
}

/// (identifier, IANA name, BoringSSL name). An empty BoringSSL name means the
/// suite is a TLS 1.3 suite, which BoringSSL selects on its own.
const CIPHERS: &[(u16, &str, &str)] = &[
    (0x1301, "TLS_AES_128_GCM_SHA256", ""),
    (0x1302, "TLS_AES_256_GCM_SHA384", ""),
    (0x1303, "TLS_CHACHA20_POLY1305_SHA256", ""),
    (
        0xc02b,
        "TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256",
        "ECDHE-ECDSA-AES128-GCM-SHA256",
    ),
    (
        0xc02f,
        "TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256",
        "ECDHE-RSA-AES128-GCM-SHA256",
    ),
    (
        0xc02c,
        "TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384",
        "ECDHE-ECDSA-AES256-GCM-SHA384",
    ),
    (
        0xc030,
        "TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384",
        "ECDHE-RSA-AES256-GCM-SHA384",
    ),
    (
        0xcca9,
        "TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256",
        "ECDHE-ECDSA-CHACHA20-POLY1305",
    ),
    (
        0xcca8,
        "TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256",
        "ECDHE-RSA-CHACHA20-POLY1305",
    ),
    (
        0xc013,
        "TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA",
        "ECDHE-RSA-AES128-SHA",
    ),
    (
        0xc014,
        "TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA",
        "ECDHE-RSA-AES256-SHA",
    ),
    (
        0xc009,
        "TLS_ECDHE_ECDSA_WITH_AES_128_CBC_SHA",
        "ECDHE-ECDSA-AES128-SHA",
    ),
    (
        0xc00a,
        "TLS_ECDHE_ECDSA_WITH_AES_256_CBC_SHA",
        "ECDHE-ECDSA-AES256-SHA",
    ),
    (
        0x009c,
        "TLS_RSA_WITH_AES_128_GCM_SHA256",
        "AES128-GCM-SHA256",
    ),
    (
        0x009d,
        "TLS_RSA_WITH_AES_256_GCM_SHA384",
        "AES256-GCM-SHA384",
    ),
    (0x002f, "TLS_RSA_WITH_AES_128_CBC_SHA", "AES128-SHA"),
    (0x0035, "TLS_RSA_WITH_AES_256_CBC_SHA", "AES256-SHA"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_huffman_policy_reaches_the_encoder() {
        // The fingerprint is built from `upstream()`, whose policy is
        // `Always`, so the profile's value has to be applied explicitly or
        // every literal goes out Huffman-coded like stock `h2`.
        for (name, _) in BUILTIN_PROFILES {
            let profile = ClientProfile::builtin(name).expect("profile");
            assert_eq!(profile.h2.huffman, HuffmanLiterals::WhenShorter);
            assert_eq!(
                profile.h2_fingerprint().huffman_policy(),
                tenta_h2::fingerprint::HuffmanPolicy::WhenShorter,
                "{name}: the profile's Huffman policy did not reach the fingerprint"
            );
        }
    }

    #[test]
    fn every_builtin_profile_parses_and_validates() {
        for (name, _) in BUILTIN_PROFILES {
            let profile = ClientProfile::builtin(name).expect("profile parses");
            assert_eq!(&profile.meta.name, name);
            assert!(!profile.headers.user_agent.is_empty());
            assert!(profile.tls.alpn.contains(&"h2".to_owned()));
        }
    }

    #[test]
    fn chrome_profile_drives_the_h2_fingerprint() {
        let profile = ClientProfile::builtin("chrome_144_macos").expect("profile");
        let fingerprint = profile.h2_fingerprint();
        assert_eq!(fingerprint.settings(), profile.h2.settings.as_slice());
        assert_eq!(fingerprint.window_update(), profile.h2.window_update);
        assert_eq!(
            fingerprint.pseudo_order(),
            [
                PseudoId::Method,
                PseudoId::Authority,
                PseudoId::Scheme,
                PseudoId::Path
            ]
        );
        assert_eq!(
            profile.akamai_h2_string(),
            "1:65536;2:0;4:6291456;6:262144|15663105|0|m,a,s,p"
        );
    }

    #[test]
    fn cipher_names_and_ids_round_trip() {
        assert_eq!(cipher_id("0x1301"), Some(0x1301));
        assert_eq!(cipher_id("TLS_AES_128_GCM_SHA256"), Some(0x1301));
        assert_eq!(cipher_id("nonsense"), None);
        let profile = ClientProfile::builtin("chrome_144_macos").expect("profile");
        assert_eq!(profile.cipher_ids().len(), profile.tls.ciphers.len());
        assert!(
            profile
                .boring_cipher_list()
                .starts_with("ECDHE-ECDSA-AES128-GCM-SHA256")
        );
    }

    #[test]
    fn a_stored_akamai_string_that_disagrees_with_the_settings_is_rejected() {
        let mut text = BUILTIN_PROFILES[0].1.to_owned();
        text = text.replace(
            "akamai_h2 = \"1:65536;2:0;4:6291456;6:262144|15663105|0|m,a,s,p\"",
            "akamai_h2 = \"1:65536|0|0|m,a,s,p\"",
        );
        let err = ClientProfile::from_toml(&text, "test").expect_err("must not validate");
        assert!(err.to_string().contains("akamai_h2"), "{err}");
    }

    #[test]
    fn profile_none_is_the_default() {
        assert_eq!(Profile::default().name(), "none");
        assert!(Profile::default().client_profile().is_none());
    }
}
