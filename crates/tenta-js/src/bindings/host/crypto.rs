//! The Window Crypto object's synchronous random-value operation.

use super::*;
use crate::bindings::{ArrayBufferViewRef, BufferKind, BufferMetadata, Crypto, CryptoOps};
use tenta_core::NativeMemoryComponent;

const GET_RANDOM_VALUES_QUOTA: usize = 65_536;

fn is_integer_view(kind: BufferKind) -> bool {
    matches!(
        kind,
        BufferKind::Int8
            | BufferKind::Uint8
            | BufferKind::Uint8Clamped
            | BufferKind::Int16
            | BufferKind::Uint16
            | BufferKind::Int32
            | BufferKind::Uint32
            | BufferKind::BigInt64
            | BufferKind::BigUint64
    )
}

fn fill_random_values(
    script: &dyn crate::dom::events::ScriptHost,
    memory: &NativeMemoryBudget,
    array: ArrayBufferViewRef,
    entropy: impl FnOnce(&mut [u8]) -> Result<(), ()>,
) -> Result<ArrayBufferViewRef, JsError> {
    let BufferMetadata {
        kind, byte_length, ..
    } = script.array_buffer_view_info(array)?;
    if !is_integer_view(kind) {
        return Err(JsError::dom_exception(
            "TypeMismatchError",
            "getRandomValues requires an integer typed array",
        ));
    }
    if byte_length > GET_RANDOM_VALUES_QUOTA {
        return Err(JsError::dom_exception(
            "QuotaExceededError",
            "the requested random byte length exceeds 65536",
        ));
    }

    script.check_native_buffer_work()?;
    let mut reservation = memory.begin(NativeMemoryComponent::WebCrypto);
    if !reservation.charge(byte_length) {
        return Err(script.native_buffer_memory_failure());
    }
    // Declare the bytes after their charge: Rust drops them before releasing
    // the reservation on success, error, termination and panic unwinds.
    let mut bytes = Vec::new();
    if bytes.try_reserve_exact(byte_length).is_err() {
        reservation.fail_allocation();
        return Err(script.native_buffer_memory_failure());
    }
    if bytes.capacity() > byte_length && !reservation.charge(bytes.capacity() - byte_length) {
        return Err(script.native_buffer_memory_failure());
    }
    bytes.resize(byte_length, 0);

    if byte_length != 0 {
        script.check_native_buffer_work()?;
        let filled = entropy(&mut bytes);
        script.check_native_buffer_work()?;
        if filled.is_err() {
            return Err(JsError::dom_exception(
                "OperationError",
                "secure randomness is unavailable",
            ));
        }
    }
    script.check_native_buffer_work()?;
    script.write_array_buffer_view(array, &bytes)?;
    // A watchdog can arrive during the final native write. Check again before
    // returning the view to page code, even when the destination is short.
    script.check_native_buffer_work()?;
    Ok(array)
}

impl HostObject {
    /// The canonical Crypto object for this page Window.
    pub fn crypto(&self) -> Result<Handle<Crypto>, JsError> {
        let HostObject::Window { ctx } = self else {
            return Err(JsError::type_error("the receiver is not Window"));
        };
        if ctx.slot != 0 {
            return Err(JsError::type_error("this document has no Window"));
        }
        if let Some(key) = *ctx.crypto_key.borrow()
            && matches!(ctx.host_records.borrow().get(key), Some(HostRecord::Crypto))
        {
            return Ok(Handle::from_bits(encode_host(ctx.slot, key)));
        }
        let key = ctx.host_records.borrow_mut().insert(HostRecord::Crypto);
        *ctx.crypto_key.borrow_mut() = Some(key);
        Ok(Handle::from_bits(encode_host(ctx.slot, key)))
    }
}

impl CryptoOps for HostObject {
    fn get_random_values(&self, array: ArrayBufferViewRef) -> Result<ArrayBufferViewRef, JsError> {
        let HostObject::Host { ctx, key } = self else {
            return Err(JsError::type_error("the receiver is not Crypto"));
        };
        if !matches!(
            ctx.host_records.borrow().get(*key),
            Some(HostRecord::Crypto)
        ) {
            return Err(JsError::type_error("the Crypto object is gone"));
        }
        let script = ctx
            .script
            .borrow()
            .clone()
            .ok_or_else(|| JsError::type_error("script context is unavailable"))?;
        fill_random_values(&*script, &ctx.native_memory, array, |bytes| {
            getrandom::fill(bytes).map_err(|_| ())
        })
    }

    fn random_uuid(&self) -> Result<String, JsError> {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).map_err(|_| {
            JsError::dom_exception("OperationError", "the random source is unavailable")
        })?;
        Ok(format_uuid_v4(bytes))
    }
}

/// `bytes` as a version 4, variant 1 UUID in its canonical lowercase form
/// (RFC 9562 §5.4): the version and variant bits replace six of the random
/// ones.
fn format_uuid_v4(mut bytes: [u8; 16]) -> String {
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = |range: std::ops::Range<usize>| {
        bytes[range]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    format!(
        "{}-{}-{}-{}-{}",
        hex(0..4),
        hex(4..6),
        hex(6..8),
        hex(8..10),
        hex(10..16)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom::events::{CallbackRef, HostValue, PromiseOutcome, PromiseRef, ScriptHost};
    use crate::fetch::FetchStartError;
    use crate::xhr::{XhrHandle, XhrRequest};
    use std::cell::{Cell, RefCell};

    struct BufferHost {
        info: BufferMetadata,
        written: RefCell<Vec<u8>>,
        writes: Cell<usize>,
    }

    impl BufferHost {
        fn new(kind: BufferKind, byte_length: usize) -> Self {
            Self {
                info: BufferMetadata {
                    kind,
                    byte_offset: 0,
                    byte_length,
                },
                written: RefCell::new(Vec::new()),
                writes: Cell::new(0),
            }
        }
    }

    impl ScriptHost for BufferHost {
        fn invoke_listener(&self, _: CallbackRef, _: u64, _: u64) -> bool {
            unreachable!()
        }
        fn release_value(&self, _: u64) {
            unreachable!()
        }
        fn retain_existing_target(&self, _: u64) -> Result<Option<u64>, JsError> {
            unreachable!()
        }
        fn build_value(&self, _: HostValue<'_>) -> Option<u64> {
            unreachable!()
        }
        fn throw_value(&self, _: u64) {
            unreachable!()
        }
        fn new_promise(&self) -> Option<PromiseRef> {
            unreachable!()
        }
        fn start_xhr(&self, _: XhrRequest, _: XhrHandle) -> Result<(), FetchStartError> {
            unreachable!()
        }
        fn abort_xhr(&self, _: XhrHandle) -> bool {
            unreachable!()
        }
        fn schedule_background_task(&self, _: u64, _: Box<dyn FnOnce()>) {
            unreachable!()
        }
        fn settle_promise(&self, _: u64, _: PromiseOutcome<'_>) -> bool {
            unreachable!()
        }
        fn array_buffer_view_info(&self, _: ArrayBufferViewRef) -> Result<BufferMetadata, JsError> {
            Ok(self.info)
        }
        fn write_array_buffer_view(
            &self,
            _: ArrayBufferViewRef,
            bytes: &[u8],
        ) -> Result<BufferMetadata, JsError> {
            self.written.replace(bytes.to_vec());
            self.writes.set(self.writes.get() + 1);
            Ok(self.info)
        }
        fn check_native_buffer_work(&self) -> Result<(), JsError> {
            Ok(())
        }
        fn native_buffer_memory_failure(&self) -> JsError {
            crate::bindings::pending_js_exception_error()
        }
    }

    fn token() -> ArrayBufferViewRef {
        ArrayBufferViewRef::new(17, 1)
    }

    #[test]
    fn exact_injected_bytes_and_zero_length_entropy_bypass_release_charge() {
        let budget = NativeMemoryBudget::with_limits(100, 100, 100, 100);
        let host = BufferHost::new(BufferKind::Uint8, 8);
        let result = fill_random_values(&host, &budget, token(), |bytes| {
            bytes.copy_from_slice(&[3, 1, 4, 1, 5, 9, 2, 6]);
            Ok(())
        });
        assert_eq!(result, Ok(token()));
        assert_eq!(*host.written.borrow(), [3, 1, 4, 1, 5, 9, 2, 6]);
        assert_eq!(budget.used(NativeMemoryComponent::WebCrypto), 0);

        let empty = BufferHost::new(BufferKind::Uint8, 0);
        assert_eq!(
            fill_random_values(&empty, &budget, token(), |_| panic!(
                "empty input needs no entropy"
            )),
            Ok(token())
        );
        assert_eq!(
            empty.writes.get(),
            1,
            "zero length still validates the destination"
        );
        assert_eq!(budget.total_used(), 0);
    }

    #[test]
    fn type_and_quota_reject_before_entropy_or_write() {
        for (kind, length, name) in [
            (BufferKind::DataView, 65_537, "TypeMismatchError"),
            (BufferKind::Float64, 65_537, "TypeMismatchError"),
            (BufferKind::Uint8, 65_537, "QuotaExceededError"),
        ] {
            let host = BufferHost::new(kind, length);
            let budget = NativeMemoryBudget::with_limits(100_000, 100_000, 100_000, 100_000);
            let error =
                fill_random_values(&host, &budget, token(), |_| panic!("entropy must not run"))
                    .unwrap_err();
            assert_eq!(error.dom_name(), Some(name));
            assert_eq!(host.writes.get(), 0);
            assert_eq!(budget.total_used(), 0);
        }
    }

    #[test]
    fn entropy_failure_and_panic_do_not_write_or_retain_charge() {
        let budget = NativeMemoryBudget::with_limits(100, 100, 100, 100);
        let host = BufferHost::new(BufferKind::Uint8, 8);
        let error = fill_random_values(&host, &budget, token(), |bytes| {
            bytes[0] = 99;
            Err(())
        })
        .unwrap_err();
        assert_eq!(error.dom_name(), Some("OperationError"));
        assert_eq!(host.writes.get(), 0);
        assert_eq!(budget.total_used(), 0);
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = fill_random_values(&host, &budget, token(), |_| panic!("entropy panic"));
        }));
        assert!(panic.is_err());
        assert_eq!(host.writes.get(), 0);
        assert_eq!(budget.total_used(), 0);
    }

    #[test]
    fn ledger_failure_preserves_web_crypto_component() {
        let budget = NativeMemoryBudget::with_limits(100, 100, 100, 4);
        let host = BufferHost::new(BufferKind::Uint8, 8);
        let _ = fill_random_values(&host, &budget, token(), |_| panic!("entropy must not run"));
        assert_eq!(host.writes.get(), 0);
        assert_eq!(budget.total_used(), 0);
        assert_eq!(
            budget.first_failure().unwrap().component,
            NativeMemoryComponent::WebCrypto
        );
    }
}
