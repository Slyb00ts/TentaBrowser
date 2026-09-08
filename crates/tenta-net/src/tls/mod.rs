//! TLS: BoringSSL driven by a [`Profile`](crate::profile::Profile).
//!
//! `docs/spec/03-network.md` §1 and §7. The connector is built once per
//! client; every connection gets its own `Ssl` so that per-connection state
//! (SNI, ALPS, session tickets, GREASE and extension permutation) is fresh.

pub mod boring_ext;

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use boring::ssl::{
    ErrorCode, Ssl, SslContext, SslContextBuilder, SslMethod, SslMode, SslOptions,
    SslSessionCacheMode, SslVerifyMode, SslVersion,
};
use boring::x509::X509;
use boring::x509::store::{X509Store, X509StoreBuilder};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::error::{ErrorName, NetError};
use crate::policy::session_cache::SessionCache;
use crate::profile::{ClientProfile, Profile};

/// A TLS stream over any byte stream.
pub type TlsStream<S> = tokio_boring::SslStream<S>;

/// Per-client TLS settings that are not part of the fingerprint.
#[derive(Clone)]
pub struct TlsOptions {
    /// Playwright's `ignoreHTTPSErrors`: the certificate is still parsed and
    /// reported, but a verification failure does not fail the connection.
    pub ignore_https_errors: bool,
    /// Extra trust anchors in PEM form (corporate proxies).
    pub extra_root_cas: Vec<Vec<u8>>,
    /// Replaces the profile's ALPN list; `Profile::None` defaults to
    /// `h2, http/1.1`.
    pub alpn_override: Option<Vec<String>>,
    /// Where session tickets live between connections and between
    /// processes; ephemeral by default (`docs/spec/03-network.md` §4).
    pub session_cache: SessionCache,
    /// How long a stored ticket may be reused.
    pub session_ttl: Duration,
    /// Encrypted Client Hello. Off by default: the GREASE extension is part
    /// of the profile, a real ECH configuration is not.
    pub enable_ech: bool,
}

impl Default for TlsOptions {
    fn default() -> Self {
        Self {
            ignore_https_errors: false,
            extra_root_cas: Vec::new(),
            alpn_override: None,
            session_cache: SessionCache::ephemeral(),
            session_ttl: Duration::from_secs(6 * 60 * 60),
            enable_ech: false,
        }
    }
}

impl std::fmt::Debug for TlsOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TlsOptions")
            .field("ignore_https_errors", &self.ignore_https_errors)
            .field("extra_root_cas", &self.extra_root_cas.len())
            .field("alpn_override", &self.alpn_override)
            .field("enable_ech", &self.enable_ech)
            .finish_non_exhaustive()
    }
}

/// What the handshake negotiated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TlsInfo {
    pub alpn: Option<String>,
    pub version: String,
    pub cipher: String,
    pub resumed: bool,
}

/// A configured BoringSSL client context.
pub struct TlsConnector {
    context: SslContext,
    profile: Option<Arc<ClientProfile>>,
    options: TlsOptions,
    host_index: boring::ex_data::Index<boring::ssl::Ssl, String>,
}

impl std::fmt::Debug for TlsConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TlsConnector")
            .field(
                "profile",
                &self.profile.as_ref().map(|p| p.meta.name.as_str()),
            )
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

impl TlsConnector {
    /// Builds the connector for a profile.
    pub fn new(profile: &Profile, options: TlsOptions) -> Result<Self, NetError> {
        // `SslConnector::builder` is not used: it loads the platform's
        // default verify paths, which costs about ten milliseconds per client
        // and is thrown away by `install_roots` anyway. The options below are
        // the ones it would have set.
        let mut builder = SslContextBuilder::new(SslMethod::tls()).map_err(ssl_setup_err)?;
        let host_index = Ssl::new_ex_index::<String>().map_err(ssl_setup_err)?;
        let mut options_mask = SslOptions::ALL
            | SslOptions::NO_COMPRESSION
            | SslOptions::NO_SSLV2
            | SslOptions::NO_SSLV3
            | SslOptions::SINGLE_DH_USE
            | SslOptions::SINGLE_ECDH_USE;
        options_mask &= !SslOptions::DONT_INSERT_EMPTY_FRAGMENTS;
        builder.set_options(options_mask);
        builder.set_mode(
            SslMode::AUTO_RETRY
                | SslMode::ACCEPT_MOVING_WRITE_BUFFER
                | SslMode::ENABLE_PARTIAL_WRITE,
        );
        builder.set_verify(SslVerifyMode::PEER);

        install_roots(&mut builder, &options)?;
        builder
            .set_min_proto_version(Some(SslVersion::TLS1_2))
            .map_err(ssl_setup_err)?;
        builder
            .set_max_proto_version(Some(SslVersion::TLS1_3))
            .map_err(ssl_setup_err)?;
        if options.ignore_https_errors {
            builder.set_verify(SslVerifyMode::NONE);
        }

        let client_profile = profile.client_profile();
        let alpn = options.alpn_override.clone().unwrap_or_else(|| {
            client_profile
                .map(|p| p.tls.alpn.clone())
                .unwrap_or_else(|| vec!["h2".to_owned(), "http/1.1".to_owned()])
        });
        builder
            .set_alpn_protos(&encode_alpn(&alpn))
            .map_err(ssl_setup_err)?;

        if let Some(profile) = client_profile {
            configure_from_profile(&mut builder, profile)?;
        }

        // The session ticket arrives after the handshake (TLS 1.3), so the
        // cache is filled from BoringSSL's new-session callback. The internal
        // store is disabled: ours is the only one, keyed by `host:port`.
        builder.set_session_cache_mode(
            SslSessionCacheMode::CLIENT | SslSessionCacheMode::NO_INTERNAL_STORE,
        );
        let sessions = options.session_cache.clone();
        let ttl = options.session_ttl;
        builder.set_new_session_callback(move |ssl, session| {
            let Some(endpoint) = ssl.ex_data(host_index) else {
                return;
            };
            if let Ok(der) = session.to_der() {
                sessions.put_tls(endpoint, der, ttl, SystemTime::now());
            }
        });

        Ok(Self {
            context: builder.build(),
            profile: client_profile.map(|p| Arc::new(p.clone())),
            options,
            host_index,
        })
    }

    /// Performs the handshake for `host` over `stream`.
    pub async fn connect<S>(
        &self,
        host: &str,
        port: u16,
        stream: S,
    ) -> Result<TlsStream<S>, NetError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let endpoint = crate::proxy::format_authority(host, port);
        let mut ssl = Ssl::new(&self.context).map_err(ssl_setup_err)?;
        // RFC 6066 §3: an IP literal is not a host name, so it carries no
        // `server_name` extension. Sending one changes the extension count and
        // flips JA4's first field from `i` to `d` on every IP-addressed
        // connection, which no browser does.
        let is_ip_literal = host.parse::<std::net::IpAddr>().is_ok();
        if !is_ip_literal {
            ssl.set_hostname(host).map_err(ssl_setup_err)?;
        }
        if !self.options.ignore_https_errors {
            // What `ConnectConfiguration::into_ssl` does: the name (or the
            // literal address) the certificate has to match.
            let param = ssl.param_mut();
            param.set_hostflags(boring::x509::verify::X509CheckFlags::NO_PARTIAL_WILDCARDS);
            match host.parse::<std::net::IpAddr>() {
                Ok(ip) => param.set_ip(ip).map_err(ssl_setup_err)?,
                Err(_) => param.set_host(host).map_err(ssl_setup_err)?,
            }
        }
        ssl.set_ex_data(self.host_index, endpoint.clone());

        if let Some(profile) = &self.profile {
            if profile.tls.permute_extensions {
                ssl.set_permute_extensions(true);
            }
            for protocol in &profile.tls.alps {
                boring_ext::add_application_settings(&mut ssl, protocol.as_bytes(), b"")
                    .map_err(ssl_setup_err)?;
            }
            if !profile.tls.alps.is_empty() {
                boring_ext::set_alps_use_new_codepoint(&mut ssl, profile.tls.alps_new_codepoint);
            }
            // Chrome always offers a GREASE ECH extension; a real ECH
            // configuration is a separate, opt-in feature.
            if profile.tls.extensions.contains(&0xfe0d) && !self.options.enable_ech {
                ssl.set_enable_ech_grease(true);
            }
        }
        if self.options.enable_ech {
            ssl.set_enable_ech_grease(false);
        }

        // A ticket from an earlier connection (or an earlier process) turns
        // the handshake into one round trip.
        if let Some(der) = self.options.session_cache.tls(&endpoint, SystemTime::now()) {
            let _ = boring_ext::set_session_der(&mut ssl, &der);
        }

        let builder = tokio_boring::SslStreamBuilder::new(ssl, stream);
        match builder.connect().await {
            Ok(stream) => Ok(stream),
            Err(err) => Err(handshake_error(&err)),
        }
    }

    /// What the profile asks the connector to present, for diagnostics.
    pub fn profile_name(&self) -> &str {
        self.profile
            .as_ref()
            .map(|p| p.meta.name.as_str())
            .unwrap_or("none")
    }

    /// The User-Agent string from the profile, used for CONNECT tunnels (MINOR-8).
    pub fn user_agent(&self) -> Option<&str> {
        self.profile.as_ref().map(|p| p.headers.user_agent.as_str())
    }
}

/// Reads the negotiated parameters off a finished stream.
pub fn tls_info<S>(stream: &TlsStream<S>) -> TlsInfo {
    let ssl = stream.ssl();
    TlsInfo {
        alpn: ssl
            .selected_alpn_protocol()
            .map(|p| String::from_utf8_lossy(p).into_owned()),
        version: ssl.version_str().to_owned(),
        cipher: ssl
            .current_cipher()
            .map(|c| c.name().to_owned())
            .unwrap_or_default(),
        resumed: ssl.session_reused(),
    }
}

fn configure_from_profile(
    builder: &mut SslContextBuilder,
    profile: &ClientProfile,
) -> Result<(), NetError> {
    let ciphers = profile.boring_cipher_list();
    if !ciphers.is_empty() {
        builder.set_cipher_list(&ciphers).map_err(|_| {
            NetError::new(
                ErrorName::SslVersionOrCipherMismatch,
                format!("profile {}: cipher list rejected", profile.meta.name),
            )
        })?;
    }
    let groups = profile.tls.groups.join(":");
    if !groups.is_empty() {
        builder.set_curves_list(&groups).map_err(|_| {
            NetError::new(
                ErrorName::SslVersionOrCipherMismatch,
                format!(
                    "profile {}: BoringSSL rejected the group list {groups:?}",
                    profile.meta.name
                ),
            )
        })?;
    }
    let sigalgs = profile.tls.sigalgs.join(":");
    if !sigalgs.is_empty() {
        builder.set_sigalgs_list(&sigalgs).map_err(|_| {
            NetError::new(
                ErrorName::SslVersionOrCipherMismatch,
                format!(
                    "profile {}: BoringSSL rejected the sigalg list {sigalgs:?}",
                    profile.meta.name
                ),
            )
        })?;
    }

    builder.set_grease_enabled(profile.tls.grease);
    builder.set_permute_extensions(profile.tls.permute_extensions);
    if profile.tls.ocsp_stapling {
        builder.enable_ocsp_stapling();
    }
    if profile.tls.signed_cert_timestamps {
        builder.enable_signed_cert_timestamps();
    }
    if !profile.tls.session_tickets {
        builder.set_options(SslOptions::NO_TICKET);
    }
    for algorithm in &profile.tls.cert_compression {
        add_cert_compression(builder, algorithm, &profile.meta.name)?;
    }
    Ok(())
}

/// Certificate decompression (RFC 8879). Only decompression is registered:
/// a client advertises the algorithms it can decompress, and we never
/// compress our own (client certificates are rare and Chrome does not).
fn add_cert_compression(
    builder: &mut SslContextBuilder,
    algorithm: &str,
    profile_name: &str,
) -> Result<(), NetError> {
    use boring::ssl::CertificateCompressor;

    /// Brotli decompression for the `compress_certificate` extension.
    struct BrotliCertificateCompressor;

    impl CertificateCompressor for BrotliCertificateCompressor {
        const ALGORITHM: boring::ssl::CertificateCompressionAlgorithm =
            boring::ssl::CertificateCompressionAlgorithm::BROTLI;
        const CAN_COMPRESS: bool = false;
        const CAN_DECOMPRESS: bool = true;

        fn compress<W>(&self, _input: &[u8], _output: &mut W) -> std::io::Result<()>
        where
            W: std::io::Write,
        {
            Err(std::io::Error::other(
                "certificate compression is not offered",
            ))
        }

        fn decompress<W>(&self, input: &[u8], output: &mut W) -> std::io::Result<()>
        where
            W: std::io::Write,
        {
            use std::io::Write;

            let mut writer = brotli::DecompressorWriter::new(output, 4096);
            writer.write_all(input)?;
            writer.flush()?;
            Ok(())
        }
    }

    match algorithm {
        "brotli" => builder
            .add_certificate_compression_algorithm(BrotliCertificateCompressor)
            .map_err(|_| {
                NetError::new(
                    ErrorName::SslProtocolError,
                    format!("profile {profile_name}: brotli certificate compression rejected"),
                )
            }),
        other => Err(NetError::new(
            ErrorName::SslProtocolError,
            format!("profile {profile_name}: unsupported certificate compression {other:?}"),
        )),
    }
}

/// Installs the trust anchors: the Mozilla bundle plus any extra roots.
///
/// Platform-native root loading is deferred (`docs/spec/03-network.md` §7):
/// BoringSSL has no keychain/schannel bridge, so the bundle is the default on
/// every platform and corporate roots are added per client.
fn install_roots(builder: &mut SslContextBuilder, options: &TlsOptions) -> Result<(), NetError> {
    if options.extra_root_cas.is_empty() {
        // The common case: every client shares one reference-counted store,
        // because filling a fresh one with the whole bundle costs more than
        // the client's entire construction budget.
        builder.set_cert_store_ref(shared_root_store()?);
        return Ok(());
    }
    let mut store = X509StoreBuilder::new().map_err(ssl_setup_err)?;
    // Parsing the bundle costs milliseconds, and every client would pay it;
    // `X509` is reference-counted, so the parsed certificates are shared and
    // only the store insert is per client.
    for certificate in bundled_roots()? {
        store.add_cert(certificate.clone()).map_err(ssl_setup_err)?;
    }
    for pem in &options.extra_root_cas {
        for certificate in X509::stack_from_pem(pem)
            .map_err(|_| NetError::new(ErrorName::CertInvalid, "custom root CA is not valid PEM"))?
        {
            store.add_cert(certificate).map_err(ssl_setup_err)?;
        }
    }
    builder.set_cert_store_builder(store);
    Ok(())
}

/// The store holding the Mozilla bundle, built once per process and shared
/// by every client that adds no roots of its own.
fn shared_root_store() -> Result<&'static X509Store, NetError> {
    static STORE: std::sync::OnceLock<X509Store> = std::sync::OnceLock::new();
    if let Some(store) = STORE.get() {
        return Ok(store);
    }
    let mut builder = X509StoreBuilder::new().map_err(ssl_setup_err)?;
    for certificate in bundled_roots()? {
        builder
            .add_cert(certificate.clone())
            .map_err(ssl_setup_err)?;
    }
    Ok(STORE.get_or_init(|| builder.build()))
}

/// The Mozilla bundle, parsed once per process.
fn bundled_roots() -> Result<&'static [X509], NetError> {
    static ROOTS: std::sync::OnceLock<Vec<X509>> = std::sync::OnceLock::new();
    if let Some(roots) = ROOTS.get() {
        return Ok(roots);
    }
    let mut parsed = Vec::with_capacity(webpki_root_certs::TLS_SERVER_ROOT_CERTS.len());
    for cert in webpki_root_certs::TLS_SERVER_ROOT_CERTS {
        parsed.push(X509::from_der(cert).map_err(ssl_setup_err)?);
    }
    Ok(ROOTS.get_or_init(|| parsed))
}

fn encode_alpn(protocols: &[String]) -> Vec<u8> {
    let mut wire = Vec::new();
    for protocol in protocols {
        wire.push(protocol.len() as u8);
        wire.extend_from_slice(protocol.as_bytes());
    }
    wire
}

fn ssl_setup_err(err: impl std::fmt::Display) -> NetError {
    NetError::new(ErrorName::SslProtocolError, err.to_string())
}

/// Maps a failed handshake to a Chromium error name.
///
/// The transport cause comes first. `SSL_get_verify_result` is not `X509_V_OK`
/// on a connection that was reset before verification ever ran, so asking it
/// first turns every TCP reset into `ERR_CERT_INVALID` and hides the retryable
/// class the caller needs (`docs/spec/03-network.md` §7). A certificate error
/// is reported only when BoringSSL itself failed the handshake in the TLS
/// layer (`SSL_ERROR_SSL`) *and* verification has a reason to give.
fn handshake_error<S>(err: &tokio_boring::HandshakeError<S>) -> NetError {
    let message = err.to_string();
    if let Some(io) = err.as_io_error() {
        return NetError::from_stream_io(io);
    }
    match err.code() {
        Some(ErrorCode::SSL) => match err.ssl().map(|ssl| ssl.verify_result()) {
            Some(Err(verify)) => NetError::new(
                certificate_error(verify.as_raw()),
                format!("certificate verification failed: {}", verify.error_string()),
            ),
            _ => NetError::new(ErrorName::SslProtocolError, message),
        },
        // The peer went away mid-handshake without a TLS alert: a reset or a
        // half-close, not anything the certificate did.
        Some(ErrorCode::SYSCALL) | Some(ErrorCode::ZERO_RETURN) => {
            NetError::new(ErrorName::ConnectionClosed, message)
        }
        _ => NetError::new(ErrorName::SslProtocolError, message),
    }
}

/// X509 verification codes to `net::ERR_CERT_*` (`docs/spec/03-network.md` §7).
fn certificate_error(code: i32) -> ErrorName {
    if code == boring_sys::X509_V_ERR_CERT_HAS_EXPIRED
        || code == boring_sys::X509_V_ERR_CERT_NOT_YET_VALID
        || code == boring_sys::X509_V_ERR_CRL_HAS_EXPIRED
        || code == boring_sys::X509_V_ERR_CRL_NOT_YET_VALID
    {
        ErrorName::CertDateInvalid
    } else if code == boring_sys::X509_V_ERR_HOSTNAME_MISMATCH {
        ErrorName::CertCommonNameInvalid
    } else if code == boring_sys::X509_V_ERR_CERT_REVOKED {
        ErrorName::CertRevoked
    } else if code == boring_sys::X509_V_ERR_DEPTH_ZERO_SELF_SIGNED_CERT
        || code == boring_sys::X509_V_ERR_SELF_SIGNED_CERT_IN_CHAIN
        || code == boring_sys::X509_V_ERR_UNABLE_TO_GET_ISSUER_CERT
        || code == boring_sys::X509_V_ERR_UNABLE_TO_GET_ISSUER_CERT_LOCALLY
        || code == boring_sys::X509_V_ERR_UNABLE_TO_VERIFY_LEAF_SIGNATURE
        || code == boring_sys::X509_V_ERR_CERT_UNTRUSTED
    {
        ErrorName::CertAuthorityInvalid
    } else {
        ErrorName::CertInvalid
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chrome() -> Profile {
        Profile::Browser(Arc::new(
            ClientProfile::builtin("chrome_144_macos").expect("profile"),
        ))
    }

    #[test]
    fn the_chrome_profile_configures_boringssl() {
        // Every knob the profile sets must be accepted by the bundled
        // BoringSSL; a rejected group or sigalg name would silently change the
        // ClientHello, so this is the guard for profile refreshes.
        TlsConnector::new(&chrome(), TlsOptions::default()).expect("connector builds");
    }

    #[test]
    fn x25519mlkem768_is_available_in_the_bundled_boringssl() {
        let mut builder = SslContextBuilder::new(SslMethod::tls()).expect("context");
        builder
            .set_curves_list("X25519MLKEM768:X25519:P-256:P-384")
            .expect("BoringSSL knows X25519MLKEM768");
    }

    #[test]
    fn profile_none_still_offers_h2() {
        let connector =
            TlsConnector::new(&Profile::None, TlsOptions::default()).expect("connector builds");
        assert_eq!(connector.profile_name(), "none");
    }

    #[test]
    fn alpn_is_encoded_with_length_prefixes() {
        assert_eq!(
            encode_alpn(&["h2".to_owned(), "http/1.1".to_owned()]),
            b"\x02h2\x08http/1.1".to_vec()
        );
    }

    #[test]
    fn certificate_codes_map_to_chromium_names() {
        assert_eq!(
            certificate_error(boring_sys::X509_V_ERR_CERT_HAS_EXPIRED),
            ErrorName::CertDateInvalid
        );
        assert_eq!(
            certificate_error(boring_sys::X509_V_ERR_DEPTH_ZERO_SELF_SIGNED_CERT),
            ErrorName::CertAuthorityInvalid
        );
        assert_eq!(
            certificate_error(boring_sys::X509_V_ERR_HOSTNAME_MISMATCH),
            ErrorName::CertCommonNameInvalid
        );
        assert_eq!(
            certificate_error(boring_sys::X509_V_ERR_CERT_REVOKED),
            ErrorName::CertRevoked
        );
    }

    #[test]
    fn an_ephemeral_session_cache_keeps_nothing() {
        let options = TlsOptions::default();
        assert!(options.session_cache.is_ephemeral());
        options.session_cache.put_tls(
            "example.com:443",
            vec![1, 2, 3],
            Duration::from_secs(60),
            SystemTime::now(),
        );
        assert!(
            options
                .session_cache
                .tls("example.com:443", SystemTime::now())
                .is_none()
        );
    }
}
