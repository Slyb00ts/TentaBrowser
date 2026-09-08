//! "hello" ladder binary for the single-libcrypto link test and the size and
//! RSS baselines (`docs/spec/03-network.md` §1.1, `docs/spec/21-project-operations.md` §4).
//!
//! Every rung of the ladder does real work so that the linker keeps the code:
//! `base` opens a loopback listener with hyper, does one HTTPS handshake to
//! itself through BoringSSL with an `rcgen` certificate and resolves
//! `localhost` with hickory; `rtc` builds a rustls client config on the `ring`
//! provider and creates a str0m offer; `codecs` opens a VP9 decoder, an Opus
//! encoder and a rav1d decoder; `v8` initialises the platform, creates one
//! isolate from the default snapshot and evaluates `1+1`.

use std::error::Error;
use std::time::Instant;

type BoxError = Box<dyn Error + Send + Sync>;

fn main() -> Result<(), BoxError> {
    let started = Instant::now();
    let budgets = tenta_core::Budgets::platform_default();
    println!(
        "tenta-core: platform budgets, page memory {} MB",
        budgets.page_memory_mb
    );

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(net::hello_https())?;
    runtime.block_on(net::hello_dns());
    drop(runtime);

    #[cfg(feature = "rtc")]
    rtc::hello()?;
    #[cfg(feature = "codecs")]
    codecs::hello()?;
    #[cfg(feature = "v8")]
    js::hello()?;

    println!("features: {}", enabled_features().join(","));
    println!("startup: {:.3} ms", started.elapsed().as_secs_f64() * 1e3);
    println!("ok");
    Ok(())
}

fn enabled_features() -> Vec<&'static str> {
    let mut features = vec!["base"];
    if cfg!(feature = "rtc") {
        features.push("rtc");
    }
    if cfg!(feature = "codecs") {
        features.push("codecs");
    }
    if cfg!(feature = "v8") {
        features.push("v8");
    }
    features
}

mod net {
    use super::BoxError;

    use boring::pkey::PKey;
    use boring::ssl::{SslAcceptor, SslConnector, SslMethod};
    use boring::x509::X509;
    use bytes::Bytes;
    use http_body_util::{BodyExt, Empty, Full};
    use hyper::{Request, Response};
    use hyper_util::rt::TokioIo;
    use tokio::net::{TcpListener, TcpStream};

    /// Serves one request over TLS on a loopback port and fetches it with
    /// hyper's client through the same BoringSSL build, verifying the
    /// self-signed certificate against a store that contains only it.
    pub async fn hello_https() -> Result<(), BoxError> {
        let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])?;
        let cert = X509::from_pem(certified.cert.pem().as_bytes())?;
        let key = PKey::private_key_from_pem(certified.signing_key.serialize_pem().as_bytes())?;

        let mut acceptor = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls())?;
        acceptor.set_certificate(&cert)?;
        acceptor.set_private_key(&key)?;
        let acceptor = acceptor.build();

        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await?;
            let tls = tokio_boring::accept(&acceptor, stream)
                .await
                .map_err(|e| e.to_string())?;
            hyper::server::conn::http1::Builder::new()
                .serve_connection(
                    TokioIo::new(tls),
                    hyper::service::service_fn(|_req| async {
                        Ok::<_, std::convert::Infallible>(Response::new(Full::new(
                            Bytes::from_static(b"ok"),
                        )))
                    }),
                )
                .await?;
            Ok::<(), BoxError>(())
        });

        let mut connector = SslConnector::builder(SslMethod::tls())?;
        connector.cert_store_mut().add_cert(cert)?;
        let config = connector.build().configure()?;
        let stream = TcpStream::connect(addr).await?;
        let tls = tokio_boring::connect(config, "localhost", stream)
            .await
            .map_err(|e| e.to_string())?;
        let version = tls.ssl().version_str().to_string();
        let cipher = tls
            .ssl()
            .current_cipher()
            .map(|c| c.name().to_string())
            .unwrap_or_default();

        let (mut sender, connection) =
            hyper::client::conn::http1::handshake(TokioIo::new(tls)).await?;
        let connection = tokio::spawn(connection);
        let request = Request::builder()
            .uri("/")
            .header("host", "localhost")
            .body(Empty::<Bytes>::new())?;
        let response = sender.send_request(request).await?;
        let status = response.status();
        let body = response.into_body().collect().await?.to_bytes();
        drop(sender);
        connection.await??;
        server.await??;

        if status != 200 || body.as_ref() != b"ok" {
            return Err(format!("unexpected response {status} {body:?}").into());
        }
        println!(
            "https: {addr} {version} {cipher} boringssl {}",
            boring::version::version()
        );
        Ok(())
    }

    /// Resolves `localhost` through hickory with the system configuration.
    /// A failure is reported but not fatal: the hosts-file lookup can be
    /// blocked in sandboxed environments and the binary's job is the link.
    pub async fn hello_dns() {
        let lookup = async {
            let resolver = hickory_resolver::Resolver::builder_tokio()?.build()?;
            resolver.lookup_ip("localhost.").await
        };
        match lookup.await {
            Ok(ips) => println!("dns: localhost -> {} address(es)", ips.iter().count()),
            Err(e) => println!("dns: lookup failed ({e})"),
        }
    }
}

#[cfg(feature = "rtc")]
mod rtc {
    use super::BoxError;

    use std::sync::Arc;
    use std::time::Instant;

    use str0m::media::{Direction, MediaKind};

    pub fn hello() -> Result<(), BoxError> {
        let provider = rustls::crypto::ring::default_provider();
        let suites = provider.cipher_suites.len();
        let config = rustls::ClientConfig::builder_with_provider(Arc::new(provider))
            .with_safe_default_protocol_versions()?
            .with_root_certificates(rustls::RootCertStore::empty())
            .with_no_client_auth();
        println!(
            "rustls: ring provider, {suites} cipher suites, {} protocol versions",
            config.crypto_provider().cipher_suites.len().min(2)
        );

        let mut rtc = str0m::Rtc::new(Instant::now());
        let mut change = rtc.sdp_api();
        let mid = change.add_media(MediaKind::Video, Direction::SendRecv, None, None, None);
        let (offer, _pending) = change
            .apply()
            .ok_or("str0m produced no offer for a new media line")?;
        println!(
            "str0m: offer for mid {mid} ({} bytes of SDP)",
            offer.to_sdp_string().len()
        );
        Ok(())
    }
}

#[cfg(feature = "codecs")]
mod codecs {
    use super::BoxError;

    use std::ffi::CStr;
    use std::mem::MaybeUninit;
    use std::ptr::NonNull;

    use rav1d::include::dav1d::dav1d::{Dav1dContext, Dav1dSettings};
    use rav1d::src::lib::{dav1d_close, dav1d_default_settings, dav1d_open, dav1d_version};

    pub fn hello() -> Result<(), BoxError> {
        let vp9 = shiguredo_libvpx::Decoder::new(shiguredo_libvpx::DecoderConfig::new(
            shiguredo_libvpx::DecoderCodec::Vp9,
        ))?;
        println!(
            "libvpx: VP9 decoder ready, {} codecs, libvpx {}",
            shiguredo_libvpx::supported_codecs().len(),
            shiguredo_libvpx::BUILD_VERSION
        );
        drop(vp9);

        let encoder = opus::Encoder::new(48_000, opus::Channels::Stereo, opus::Application::Audio)?;
        println!("opus: {} (48 kHz stereo encoder)", opus::version());
        drop(encoder);

        // rav1d exports the dav1d C API; the settings are filled by the
        // library itself, so uninitialised storage is passed to it first.
        let mut settings = MaybeUninit::<Dav1dSettings>::uninit();
        let settings_ptr = NonNull::new(settings.as_mut_ptr()).ok_or("null settings")?;
        let mut context: Option<Dav1dContext> = None;
        // SAFETY: valid pointers to live storage; the context is closed before
        // it is dropped.
        let rc = unsafe {
            dav1d_default_settings(settings_ptr);
            dav1d_open(Some(NonNull::from(&mut context)), Some(settings_ptr))
        };
        if rc.0 != 0 || context.is_none() {
            return Err(format!("dav1d_open failed with {}", rc.0).into());
        }
        unsafe { dav1d_close(Some(NonNull::from(&mut context))) };
        let version = unsafe { CStr::from_ptr(dav1d_version()) };
        println!(
            "rav1d: dav1d {} decoder opened and closed",
            version.to_string_lossy()
        );
        Ok(())
    }
}

#[cfg(feature = "v8")]
mod js {
    use super::BoxError;

    use std::time::Instant;

    pub fn hello() -> Result<(), BoxError> {
        let t0 = Instant::now();
        let platform = v8::new_default_platform(0, false).make_shared();
        v8::V8::initialize_platform(platform);
        v8::V8::initialize();
        let platform_init = t0.elapsed();

        let t1 = Instant::now();
        let isolate = &mut v8::Isolate::new(v8::CreateParams::default());
        let isolate_create = t1.elapsed();

        let result = {
            v8::scope!(let handle_scope, isolate);
            let context = v8::Context::new(handle_scope, Default::default());
            let scope = &v8::ContextScope::new(handle_scope, context);
            let code = v8::String::new(scope, "1+1").ok_or("v8 string")?;
            let script = v8::Script::compile(scope, code, None).ok_or("v8 compile")?;
            let value = script.run(scope).ok_or("v8 run")?;
            value.to_rust_string_lossy(scope)
        };
        if result != "2" {
            return Err(format!("v8 evaluated 1+1 to {result:?}").into());
        }
        println!(
            "v8: {} platform init {:.3} ms, first isolate {:.3} ms, 1+1 = {result}",
            v8::VERSION_STRING,
            platform_init.as_secs_f64() * 1e3,
            isolate_create.as_secs_f64() * 1e3
        );
        Ok(())
    }
}
