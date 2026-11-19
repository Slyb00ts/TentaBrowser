//! `SubtleCrypto` and `crypto.subtle` (Web Cryptography API,
//! <https://w3c.github.io/webcrypto/>), as a script the engine builds the
//! first time a page asks for one of them, over one native function that
//! hashes.
//!
//! # Why a script
//!
//! Every operation of `SubtleCrypto` returns a promise, and WebIDL turns
//! whatever goes wrong in such an operation - a missing argument, a value of
//! the wrong type, a getter that throws - into a rejection of that promise,
//! never into an exception of the call. The generated bindings throw
//! (`docs/dev/idl.md`, "the separate Promise rejection policy"), and a page
//! that writes `crypto.subtle.digest(…).catch(…)` relies on the difference.
//! A script method gets it from one `try`.
//!
//! # What is here
//!
//!  - `crypto.subtle`: one `SubtleCrypto` per realm, the same object at every
//!    read.
//!  - `digest(algorithm, data)` for `SHA-1`, `SHA-256`, `SHA-384` and
//!    `SHA-512`, the algorithm given as a name or as an object with a `name`.
//!    The digest is computed by the host, from the process's one libcrypto
//!    (`tenta_net::digest`), over a copy of the data taken during the call;
//!    the promise is resolved from a task, as the standard's "crypto task
//!    source" has it.
//!
//! # What is not, and what differs
//!
//!  - **Nothing but `digest()`.** No keys: `importKey()`, `generateKey()`,
//!    `exportKey()`, `sign()`, `verify()`, `encrypt()`, `decrypt()`,
//!    `deriveBits()`, `deriveKey()`, `wrapKey()` and `unwrapKey()` are absent
//!    rather than present and failing, and so is `CryptoKey`.
//!  - **The digest is computed during the call**, not in parallel with the
//!    page: a page that hashes a large buffer waits for it.
//!  - **Data the page's memory budget cannot hold twice** is refused with an
//!    `OperationError`, where a browser would hash it.
//!  - **No secure-context test.** The standard exposes `subtle` in secure
//!    contexts only; this engine does not model them, so an `http:` page has
//!    it too.
//!
//! # How it is built
//!
//! The source evaluates to a function of two arguments: `digestBytes`, the
//! host's `(algorithm, data) -> ArrayBuffer`, which does the standard's
//! argument conversions and its algorithm normalization in their order and
//! throws what they throw; and `queueTask`, the host's way to run a function
//! as a task. It returns the exports. The script must not read `SubtleCrypto`
//! from the global, which is the lazy name that runs it.

/// The script. Evaluates to `function (digestBytes, queueTask)` returning
/// `{ SubtleCrypto, subtle() }`.
pub const SUBTLE_PRELUDE_SOURCE: &str = r#"
(function (digestBytes, queueTask) {
  'use strict';
  const PromiseConstructor = Promise;
  let constructing = false;
  let instance = null;

  class SubtleCrypto {
    constructor() {
      if (!constructing) throw new TypeError('Illegal constructor');
    }

    digest(algorithm, data) {
      let digest;
      // A promise-returning operation: what goes wrong before the work is
      // done rejects the promise.
      try {
        if (instance === null || this !== instance) throw new TypeError('Illegal invocation');
        if (arguments.length < 2) {
          throw new TypeError("Failed to execute 'digest' on 'SubtleCrypto': 2 arguments " +
            'required, but only ' + arguments.length + ' present.');
        }
        digest = digestBytes(algorithm, data);
      } catch (error) {
        return PromiseConstructor.reject(error);
      }
      return new PromiseConstructor(function (resolve) {
        queueTask(function () { resolve(digest); });
      });
    }
  }

  Object.defineProperty(SubtleCrypto.prototype, 'digest', { enumerable: true });
  Object.defineProperty(SubtleCrypto.prototype, Symbol.toStringTag, {
    value: 'SubtleCrypto', configurable: true,
  });

  return {
    SubtleCrypto: SubtleCrypto,
    subtle: function () {
      if (instance === null) {
        constructing = true;
        try {
          instance = new SubtleCrypto();
        } finally {
          constructing = false;
        }
      }
      return instance;
    },
  };
})
"#;
