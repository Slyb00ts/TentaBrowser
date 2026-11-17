//! `Request` (<https://fetch.spec.whatwg.org/#request-class>) and the steps
//! `fetch(input, init)` shares with its constructor.
//!
//! The standard defines `fetch(input, init)` as "construct a `Request` from
//! `input` and `init`, then fetch it", so the two are one algorithm here:
//! [`request_parts`] is the constructor's steps over plain data, the
//! constructor stores what it returns in a record, and `fetch()` turns it
//! straight into the request a driver is handed.
//!
//! # What a request record holds
//!
//! The URL, the method, the four modes, `keepalive`, the body as a byte
//! sequence (the stand-in `Response` uses, for the same reason: there are no
//! streams), a `Headers` record under guard `"request"` that the request owns
//! the way a response owns its headers, and a signal.
//!
//! # The signal
//!
//! The standard gives every request a signal of its own that *follows* the one
//! the initializer named. There is no "follow" between signals in this engine,
//! so a request that was given a signal answers `signal` with **that signal**,
//! kept alive by one stored reference to its wrapper, and a request that was
//! given none owns a signal nothing can abort. `request.signal === init.signal`
//! is therefore `true` here and `false` in a browser; what a page does with
//! the signal — read `aborted`, listen for `abort`, pass the request to
//! `fetch()` and abort it — is the same.

use super::*;

/// The prefix of a failure of the `Request` constructor's own steps.
const REQUEST_CONTEXT: &str = "Failed to construct 'Request'";

/// A `Request` record.
#[derive(Debug)]
pub struct RequestRecord {
    /// The request's URL, already resolved against the document.
    pub url: Url,
    /// The method, validated and normalized.
    pub method: http::Method,
    /// The header list: a `Headers` record under guard `"request"` that this
    /// record owns.
    pub headers: HostKey,
    /// The body, or `None` for a null body.
    pub body: Option<Vec<u8>>,
    /// Whether the body has been read. A request with no body is never
    /// disturbed.
    pub body_used: bool,
    /// The request's mode.
    pub mode: RequestMode,
    /// The credentials mode.
    pub credentials: RequestCredentials,
    /// The cache mode.
    pub cache: RequestCache,
    /// The redirect mode.
    pub redirect: RequestRedirect,
    /// The keepalive flag, stored and read back; a request does not outlive
    /// its page here.
    pub keepalive: bool,
    /// The signal `signal` answers with.
    pub signal: RequestSignal,
}

/// The signal of a request; see the module documentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestSignal {
    /// A signal record this request owns, as an `AbortController` owns its
    /// signal: it lives while the request does, or while its own wrapper does.
    Own(HostKey),
    /// The signal the initializer named.
    Given {
        /// The signal's handle bits.
        handle: u64,
        /// The stored reference to the signal's wrapper that keeps it, and so
        /// its record, alive. Released when the request's record is freed.
        stored: Option<u64>,
    },
}

impl RequestRecord {
    /// The signal record this request owns, which its reclamation frees.
    pub(super) fn own_signal(&self) -> Option<HostKey> {
        match self.signal {
            RequestSignal::Own(key) => Some(key),
            RequestSignal::Given { .. } => None,
        }
    }

    /// The stored value this record owns and gives back when it is freed.
    pub(super) fn stored_value(&self) -> Option<u64> {
        match self.signal {
            RequestSignal::Given { stored, .. } => stored,
            RequestSignal::Own(_) => None,
        }
    }
}

/// A request as the `Request` constructor's steps leave it, before it is
/// either stored in a record or handed to a driver.
#[derive(Debug, Clone)]
pub struct RequestParts {
    /// The parsed URL.
    pub url: Url,
    /// The validated, normalized method.
    pub method: http::Method,
    /// The header list, already filled under guard `"request"`.
    pub headers: HeaderList,
    /// The body, or `None` for a null body.
    pub body: Option<Vec<u8>>,
    /// The request's mode.
    pub mode: RequestMode,
    /// The credentials mode.
    pub credentials: RequestCredentials,
    /// The cache mode.
    pub cache: RequestCache,
    /// The redirect mode.
    pub redirect: RequestRedirect,
    /// The keepalive flag.
    pub keepalive: bool,
    /// The handle bits of the signal the request was given, if it was given
    /// one whose wrapper is known to be alive.
    pub signal: Option<u64>,
}

impl RequestParts {
    /// The request a bare URL is: a `GET` with every mode at its default.
    pub(super) fn new(url: Url) -> RequestParts {
        RequestParts {
            url,
            method: http::Method::GET,
            headers: Vec::new(),
            body: None,
            mode: RequestMode::Cors,
            credentials: RequestCredentials::SameOrigin,
            cache: RequestCache::Default,
            redirect: RequestRedirect::Follow,
            keepalive: false,
            signal: None,
        }
    }

    /// The request a driver is handed for these parts, made by the document
    /// at `initiator` with provenance `origin`.
    ///
    /// Every mode [`refused_request_init_member`] lets through has an arm; the
    /// ones it refuses fall to the default they never reach.
    pub fn into_fetch_request(self, initiator: Url, origin: Origin) -> FetchRequest {
        let credentials = match self.credentials {
            RequestCredentials::Omit => crate::fetch::Credentials::Omit,
            RequestCredentials::Include => crate::fetch::Credentials::Include,
            RequestCredentials::SameOrigin => crate::fetch::Credentials::SameOrigin,
        };
        let mode = match self.mode {
            RequestMode::SameOrigin => crate::fetch::Mode::SameOrigin,
            RequestMode::NoCors => crate::fetch::Mode::NoCors,
            _ => crate::fetch::Mode::Cors,
        };
        let redirect = match self.redirect {
            RequestRedirect::Error => crate::fetch::Redirect::Error,
            _ => crate::fetch::Redirect::Follow,
        };
        let bypass_cache = matches!(
            self.cache,
            RequestCache::NoStore | RequestCache::Reload | RequestCache::NoCache
        );
        FetchRequest {
            url: self.url,
            method: self.method,
            headers: header_pairs(self.headers),
            body: self.body,
            initiator,
            origin,
            credentials,
            mode,
            redirect,
            bypass_cache,
        }
    }
}

/// The `Request` constructor's steps for `input` and `init`
/// (<https://fetch.spec.whatwg.org/#dom-request>), which are also the first
/// step of `fetch(input, init)`. `context` prefixes the messages, so each
/// caller names the call that failed.
///
/// A string `input` is parsed against the document's URL and starts as
/// [`RequestParts::new`]. A `Request` `input` starts as a copy of that request
/// — its URL, method, modes and a copy of its header list — and then `init`
/// is applied over either. When `init` names no body the new request takes the
/// input request's body, which disturbs the input.
///
/// The caller has already refused an initializer the engine cannot serve
/// ([`refused_request_init_member`]).
pub fn request_parts(
    ctx: &SharedContext,
    input: RequestInfo,
    init: RequestInit,
    context: &str,
) -> Result<RequestParts, JsError> {
    let (parts, source) = match input {
        RequestInfo::UsvString(input) => {
            let url =
                ctx.doc.borrow().resolve_url(&input).ok_or_else(|| {
                    JsError::type_error(format!("Failed to parse URL from {input}"))
                })?;
            (RequestParts::new(url), None)
        }
        RequestInfo::Request(handle) => {
            let decoded = decode(handle.bits());
            let records = ctx.host_records.borrow();
            let Some(HostRecord::Request(record)) = host_record_of(&records, handle.bits()) else {
                return Err(JsError::type_error(format!(
                    "{context}: the input is not the Request it was converted as"
                )));
            };
            let headers = match records.get(record.headers) {
                Some(HostRecord::Headers(headers)) => headers.list.clone(),
                _ => Vec::new(),
            };
            let parts = RequestParts {
                url: record.url.clone(),
                method: record.method.clone(),
                headers,
                // The body moves, if it moves at all, after `init` is known.
                body: None,
                mode: record.mode,
                credentials: record.credentials,
                cache: record.cache,
                redirect: record.redirect,
                keepalive: record.keepalive,
                // A signal the input owns has no wrapper to keep until script
                // reads it, and nothing can abort it, so it is not inherited.
                signal: match record.signal {
                    RequestSignal::Given { handle, .. } => Some(handle),
                    RequestSignal::Own(_) => None,
                },
            };
            (parts, Some(decoded.host_key()))
        }
    };
    apply_request_init(ctx, parts, source, init, context)
}

/// Applies `init` over `parts`, in the standard's order: the method, the
/// modes, the headers, then the body. `source` is the record of the request
/// `parts` was copied from, whose body the new request takes when `init`
/// names none.
///
/// 1. `init["method"]`: a value that is not a *method* is a `TypeError`, one
///    that is a *forbidden method* is a `TypeError`, and a value that is
///    either is then normalised.
/// 2. `init["headers"]`: the request's header list is *filled* from them under
///    guard `"request"`, replacing a list copied from an input request — so a
///    *forbidden request-header* is dropped silently.
/// 3. The body: a non-null body with a `GET` or a `HEAD` method is a
///    `TypeError`; an `init["body"]` is *extracted with type* and the type is
///    written as `Content-Type` when the list does not already name one.
pub(super) fn apply_request_init(
    ctx: &SharedContext,
    mut parts: RequestParts,
    source: Option<HostKey>,
    init: RequestInit,
    context: &str,
) -> Result<RequestParts, JsError> {
    // `method_token` is the `method = token` production of RFC 9110 §5.6.2,
    // which the XHR `open()` steps 2 to 4 also run: the same standard defines
    // both, so the two share the checker.
    if let Some(bytes) = &init.method {
        let Some(mut normalized) = method_token(bytes) else {
            return Err(JsError::type_error(format!(
                "{context}: the method is not a valid HTTP method token"
            )));
        };
        if FORBIDDEN_METHODS
            .iter()
            .any(|forbidden| bytes.eq_ignore_ascii_case(forbidden))
        {
            return Err(JsError::type_error(format!(
                "{context}: the method is forbidden"
            )));
        }
        if NORMALIZED_METHODS
            .iter()
            .any(|known| bytes.eq_ignore_ascii_case(known))
        {
            normalized.make_ascii_uppercase();
        }
        // `method_token` admits exactly HTTP's `tchar`s, which `http` also
        // accepts, so the parse cannot fail on a value that got this far.
        parts.method = http::Method::from_bytes(normalized.as_bytes()).map_err(|_| {
            JsError::type_error(format!(
                "{context}: the method is not a valid HTTP method token"
            ))
        })?;
    }

    if let Some(credentials) = init.credentials {
        parts.credentials = credentials;
    }
    if let Some(mode) = init.mode {
        parts.mode = mode;
    }
    if let Some(redirect) = init.redirect {
        parts.redirect = redirect;
    }
    if let Some(cache) = init.cache {
        parts.cache = cache;
    }
    if let Some(keepalive) = init.keepalive {
        parts.keepalive = keepalive;
    }
    // An explicit `null` is "no signal", which also drops an inherited one.
    match init.signal {
        Some(Some(signal)) => parts.signal = Some(signal.bits()),
        Some(None) => parts.signal = None,
        None => {}
    }
    // The constructor's `no-cors` step: only a CORS-safelisted method.
    if parts.mode == RequestMode::NoCors
        && !matches!(
            parts.method,
            http::Method::GET | http::Method::HEAD | http::Method::POST
        )
    {
        return Err(JsError::type_error(format!(
            "{context}: '{}' is unsupported in no-cors mode",
            parts.method
        )));
    }

    if let Some(headers) = init.headers {
        parts.headers = fill_header_list(context, headers, HeadersGuard::Request)?;
    }

    // The outer `None` is an absent member and the inner one the IDL's
    // nullable `body`; both leave the input request's body, if there is one.
    let init_body = init.body.flatten();
    let source_has_body = source.is_some_and(|source| {
        matches!(
            ctx.host_records.borrow().get(source),
            Some(HostRecord::Request(record)) if record.body.is_some()
        )
    });
    if (init_body.is_some() || source_has_body)
        && (parts.method == http::Method::GET || parts.method == http::Method::HEAD)
    {
        return Err(JsError::type_error(format!(
            "{context}: a request with a body cannot be a GET or a HEAD"
        )));
    }
    match (init_body, source) {
        (Some(body), _) => {
            let (bytes, body_type) = extract_body(ctx, body)?;
            set_inferred_content_type(&mut parts.headers, body_type.as_deref());
            parts.body = Some(bytes);
        }
        (None, Some(source)) if source_has_body => {
            // "If inputBody is body and input is unusable, then throw a
            // TypeError", and otherwise the body is the new request's and the
            // input is disturbed.
            let mut records = ctx.host_records.borrow_mut();
            if let Some(HostRecord::Request(record)) = records.get_mut(source) {
                if record.body_used {
                    return Err(JsError::type_error(format!(
                        "{context}: the input request's body has already been read"
                    )));
                }
                record.body_used = true;
                parts.body = record.body.clone();
            }
        }
        _ => {}
    }
    Ok(parts)
}

/// Inserts the request record for `parts`, with the headers record it owns
/// and its signal, and returns the request's key.
fn insert_request(ctx: &SharedContext, parts: RequestParts) -> Result<HostKey, JsError> {
    // The signal the request was given is kept by one stored reference to its
    // wrapper, taken before any record is inserted so that a failure leaves
    // nothing behind.
    let given = match parts.signal {
        Some(handle)
            if matches!(
                host_record_of(&ctx.host_records.borrow(), handle),
                Some(HostRecord::Signal(_))
            ) =>
        {
            let script = ctx.script.borrow().as_ref().map(Rc::clone);
            let stored = match script {
                Some(script) => script.retain_existing_target(handle)?,
                None => None,
            };
            Some(RequestSignal::Given { handle, stored })
        }
        _ => None,
    };

    let mut records = ctx.host_records.borrow_mut();
    let headers = records.insert(HostRecord::Headers(HeadersRecord {
        list: parts.headers,
        guard: HeadersGuard::Request,
        response: None,
        has_wrapper: false,
    }));
    let signal = given.unwrap_or_else(|| {
        RequestSignal::Own(records.insert(HostRecord::Signal(SignalRecord {
            reason: None,
            controller: None,
            has_wrapper: false,
        })))
    });
    let key = records.insert(HostRecord::Request(RequestRecord {
        url: parts.url,
        method: parts.method,
        headers,
        body: parts.body,
        body_used: false,
        mode: parts.mode,
        credentials: parts.credentials,
        cache: parts.cache,
        redirect: parts.redirect,
        keepalive: parts.keepalive,
        signal,
    }));
    // Both records the request owns point back at it, which is what their own
    // reclamation reads: each stays while the request's record is alive.
    if let Some(HostRecord::Headers(record)) = records.get_mut(headers) {
        record.response = Some(key);
    }
    if let RequestSignal::Own(signal) = signal
        && let Some(HostRecord::Signal(record)) = records.get_mut(signal)
    {
        record.controller = Some(key);
    }
    Ok(key)
}

impl HostObject {
    /// Reads the `Request` this host stands for, or returns `default` when the
    /// host is not one or its record has been reclaimed.
    fn with_request<R>(&self, default: R, f: impl FnOnce(&RequestRecord) -> R) -> R {
        if let HostObject::Host { ctx, key } = self
            && let Some(HostRecord::Request(record)) = ctx.host_records.borrow().get(*key)
        {
            return f(record);
        }
        default
    }
}

impl RequestConstructor for HostObject {
    fn construct(
        realm: &Realm,
        input: RequestInfo,
        init: Option<RequestInit>,
    ) -> Result<Self, JsError> {
        let ctx = realm_context(realm)?;
        let init = init.unwrap_or_default();
        if let Some(member) = refused_request_init_member(&init) {
            return Err(JsError::type_error(format!(
                "{REQUEST_CONTEXT}: the RequestInit member '{member}' is not implemented"
            )));
        }
        let parts = request_parts(&ctx, input, init, REQUEST_CONTEXT)?;
        let key = insert_request(&ctx, parts)?;
        Ok(HostObject::host_record(ctx, key))
    }
}

impl RequestOps for HostObject {
    fn method(&self) -> Result<Vec<u8>, JsError> {
        Ok(self.with_request(Vec::new(), |record| {
            record.method.as_str().as_bytes().to_vec()
        }))
    }

    fn url(&self) -> Result<String, JsError> {
        Ok(self.with_request(String::new(), |record| record.url.as_str().to_owned()))
    }

    fn headers(&self) -> Result<Handle<Headers>, JsError> {
        // `[SameObject]`: the key was stored by the constructor.
        self.with_request(None, |record| Some(record.headers))
            .map(|headers| Handle::from_bits(encode_host(self.slot(), headers)))
            .ok_or_else(|| JsError::type_error("headers: the receiver is not a Request"))
    }

    fn destination(&self) -> Result<RequestDestination, JsError> {
        // A request script constructs has the empty destination.
        Ok(RequestDestination::EmptyString)
    }

    fn referrer(&self) -> Result<String, JsError> {
        // The only referrer an initializer may name is the default, "client",
        // which serializes as this.
        Ok("about:client".to_owned())
    }

    fn referrer_policy(&self) -> Result<ReferrerPolicy, JsError> {
        Ok(ReferrerPolicy::EmptyString)
    }

    fn mode(&self) -> Result<RequestMode, JsError> {
        Ok(self.with_request(RequestMode::Cors, |record| record.mode))
    }

    fn credentials(&self) -> Result<RequestCredentials, JsError> {
        Ok(self.with_request(RequestCredentials::SameOrigin, |record| record.credentials))
    }

    fn cache(&self) -> Result<RequestCache, JsError> {
        Ok(self.with_request(RequestCache::Default, |record| record.cache))
    }

    fn redirect(&self) -> Result<RequestRedirect, JsError> {
        Ok(self.with_request(RequestRedirect::Follow, |record| record.redirect))
    }

    fn integrity(&self) -> Result<String, JsError> {
        // A non-empty `integrity` is refused by name, so this is all it can be.
        Ok(String::new())
    }

    fn keepalive(&self) -> bool {
        self.with_request(false, |record| record.keepalive)
    }

    fn signal(&self) -> Result<Handle<AbortSignal>, JsError> {
        self.with_request(None, |record| Some(record.signal))
            .map(|signal| match signal {
                RequestSignal::Own(key) => Handle::from_bits(encode_host(self.slot(), key)),
                RequestSignal::Given { handle, .. } => Handle::from_bits(handle),
            })
            .ok_or_else(|| JsError::type_error("signal: the receiver is not a Request"))
    }

    fn duplex(&self) -> Result<RequestDuplex, JsError> {
        Ok(RequestDuplex::Half)
    }

    fn clone(&self) -> Result<Handle<Request>, JsError> {
        // "If this is unusable, then throw a TypeError." The clone has a header
        // list of its own and the same body: the body is a byte sequence held
        // whole, so the standard's tee is a copy of it.
        let HostObject::Host { ctx, key } = self else {
            return Err(JsError::type_error("clone: the receiver is not a Request"));
        };
        let parts = {
            let records = ctx.host_records.borrow();
            let Some(HostRecord::Request(record)) = records.get(*key) else {
                return Err(JsError::type_error("clone: the receiver is not a Request"));
            };
            if record.body.is_some() && record.body_used {
                return Err(JsError::type_error(
                    "Failed to execute 'clone' on 'Request': the body has already been read",
                ));
            }
            RequestParts {
                url: record.url.clone(),
                method: record.method.clone(),
                headers: match records.get(record.headers) {
                    Some(HostRecord::Headers(headers)) => headers.list.clone(),
                    _ => Vec::new(),
                },
                body: record.body.clone(),
                mode: record.mode,
                credentials: record.credentials,
                cache: record.cache,
                redirect: record.redirect,
                keepalive: record.keepalive,
                signal: match record.signal {
                    RequestSignal::Given { handle, .. } => Some(handle),
                    RequestSignal::Own(_) => None,
                },
            }
        };
        let key = insert_request(ctx, parts)?;
        Ok(Handle::from_bits(encode_host(ctx.slot, key)))
    }

    fn body_used(&self) -> bool {
        self.body_is_used()
    }

    fn array_buffer(&self) -> Result<Promise<Vec<u8>>, JsError> {
        self.body_array_buffer()
    }

    fn blob(&self) -> Result<Promise<Handle<Blob>>, JsError> {
        self.body_blob()
    }

    fn bytes(&self) -> Result<Promise<Vec<u8>>, JsError> {
        self.body_bytes()
    }

    fn json(&self) -> Result<Promise<AnyValue>, JsError> {
        self.body_json()
    }

    fn text(&self) -> Result<Promise<String>, JsError> {
        self.body_text()
    }
}
