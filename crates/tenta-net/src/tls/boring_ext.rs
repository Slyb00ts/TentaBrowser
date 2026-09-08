//! The BoringSSL calls `boring`'s safe API does not expose.
//!
//! This is the only file in `tenta-net` with `unsafe`
//! (`docs/spec/13-performance.md` §5, `03-network.md` §1): ALPS (TLS
//! extension 17513 / 17613), which Chrome sends on every HTTPS connection and
//! which no safe wrapper covers, plus TLS session resumption, whose
//! `SSL_set_session` binding is `unsafe` in `boring` because the session must
//! come from the same `SSL_CTX`.
//!
//! Every call documents the invariant that makes it sound. All of them share
//! one: `SslRef` is a live, exclusively borrowed `SSL *` owned by the caller,
//! and the pointers passed in are borrowed for the duration of the call only
//! (BoringSSL copies what it keeps).

use boring::error::ErrorStack;
use boring::ssl::{SslRef, SslSession};
use foreign_types::ForeignTypeRef;

/// Enables ALPS for one ALPN protocol, offering `settings` as the client's
/// application settings (empty for HTTP/2, which Chrome sends as an empty
/// SETTINGS payload).
///
/// Must be called before the handshake starts.
pub fn add_application_settings(
    ssl: &mut SslRef,
    protocol: &[u8],
    settings: &[u8],
) -> Result<(), ErrorStack> {
    // SAFETY: `ssl.as_ptr()` is a live `SSL *` borrowed exclusively for this
    // call. `protocol` and `settings` are read-only for the duration of the
    // call: BoringSSL copies both into the `SSL`'s own storage
    // (`ssl_add_application_settings` in ssl_lib.cc). Empty slices are passed
    // as a dangling-but-aligned pointer with length 0, which the callee never
    // dereferences.
    let result = unsafe {
        boring_sys::SSL_add_application_settings(
            ssl.as_ptr(),
            protocol.as_ptr(),
            protocol.len(),
            settings.as_ptr(),
            settings.len(),
        )
    };
    if result == 1 {
        Ok(())
    } else {
        Err(ErrorStack::get())
    }
}

/// Selects the ALPS codepoint: the new 17613 when `use_new` is true, the
/// original 17513 otherwise. Chrome ≥ 141 uses the new one.
pub fn set_alps_use_new_codepoint(ssl: &mut SslRef, use_new: bool) {
    // SAFETY: `ssl.as_ptr()` is a live `SSL *` borrowed exclusively for this
    // call; the callee only stores a flag on it and returns nothing.
    unsafe {
        boring_sys::SSL_set_alps_use_new_codepoint(ssl.as_ptr(), i32::from(use_new));
    }
}

/// Offers a cached session for resumption.
///
/// `der` must be a session produced by a connection of the *same*
/// `SslContext`; sessions from another context have incompatible internal
/// state. Callers key their cache by connector identity to guarantee that.
pub fn set_session_der(ssl: &mut SslRef, der: &[u8]) -> Result<(), ErrorStack> {
    let session = SslSession::from_der(der)?;
    // SAFETY: `SSL_set_session` requires a session created by the same
    // `SSL_CTX` as `ssl` (documented on `boring`'s wrapper); the caller's
    // cache is keyed per connector, so `der` was serialised from a session of
    // this context. The reference is only borrowed: BoringSSL takes its own
    // reference on the session before returning.
    unsafe { ssl.set_session(&session) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use boring::ssl::{Ssl, SslContext, SslMethod};

    fn ssl() -> Ssl {
        let ctx = SslContext::builder(SslMethod::tls())
            .expect("context builder")
            .build();
        Ssl::new(&ctx).expect("ssl")
    }

    #[test]
    fn alps_settings_are_accepted_for_h2() {
        let mut ssl = ssl();
        add_application_settings(&mut ssl, b"h2", b"").expect("ALPS for h2");
        set_alps_use_new_codepoint(&mut ssl, true);
        set_alps_use_new_codepoint(&mut ssl, false);
    }

    #[test]
    fn a_malformed_session_is_an_error_not_a_crash() {
        let mut ssl = ssl();
        let err = set_session_der(&mut ssl, b"not a der session").expect_err("must fail");
        assert!(!err.errors().is_empty());
    }
}
