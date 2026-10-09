use crate::{qjs, Ctx, Error, Result, StdString, Value};
use core::{ffi::c_char, mem, ptr::NonNull, slice, str};

/// Rust representation of a JavaScript string.
#[derive(Debug, Clone, PartialEq, Hash)]
#[repr(transparent)]
pub struct String<'js>(pub(crate) Value<'js>);

impl<'js> String<'js> {
    /// Convert the JavaScript string to a Rust string.
    pub fn to_string(&self) -> Result<StdString> {
        let mut len = mem::MaybeUninit::uninit();
        let ptr = unsafe {
            qjs::JS_ToCStringLen(self.0.ctx.as_ptr(), len.as_mut_ptr(), self.0.as_js_value())
        };
        if ptr.is_null() {
            // Might not ever happen but I am not 100% sure
            // so just incase check it.
            return Err(Error::Unknown);
        }
        let len = unsafe { len.assume_init() };
        let bytes: &[u8] = unsafe { slice::from_raw_parts(ptr as _, len as _) };
        let result = str::from_utf8(bytes).map(|s| s.into());
        unsafe { qjs::JS_FreeCString(self.0.ctx.as_ptr(), ptr) };
        Ok(result?)
    }

    /// Borrow the exact UTF-16 code units, including unmatched surrogates.
    pub fn utf16_units(&self) -> Result<Utf16String<'_, 'js>> {
        let mut len = 0;
        // SAFETY: This value is a live string in this context. QuickJS returns
        // a retained, aligned UTF-16 buffer, released by Utf16String::drop.
        let ptr = unsafe {
            qjs::JS_ToCStringLenUTF16(self.0.ctx.as_ptr(), &mut len, self.0.as_js_value())
        };
        if ptr.is_null() {
            return Err(self.0.ctx.raise_exception());
        }
        Ok(Utf16String {
            ctx: &self.0.ctx,
            ptr,
            len: len as usize,
        })
    }

    /// Convert UTF-16 code units to Unicode scalar values for Rust text.
    /// Unpaired surrogates become U+FFFD; valid surrogate pairs stay intact.
    pub fn to_string_lossy(&self) -> Result<StdString> {
        Ok(StdString::from_utf16_lossy(self.utf16_units()?.as_ref()))
    }

    /// Create a JavaScript string from exact UTF-16 code units.
    pub fn from_utf16(ctx: Ctx<'js>, units: &[u16]) -> Result<Self> {
        // SAFETY: The input slice remains live for the synchronous native
        // copy; handle_exception checks the result before taking ownership.
        Ok(unsafe {
            let js_val = qjs::JS_NewStringUTF16(ctx.as_ptr(), units.as_ptr(), units.len() as _);
            let js_val = ctx.handle_exception(js_val)?;
            String::from_js_value(ctx, js_val)
        })
    }

    /// Convert the Javascript string to a Javascript C string.
    pub fn to_cstring(self) -> Result<CString<'js>> {
        CString::from_string(self)
    }

    /// Create a new JavaScript string from an Rust string.
    pub fn from_str(ctx: Ctx<'js>, s: &str) -> Result<Self> {
        let len = s.len();
        let ptr = s.as_ptr();
        Ok(unsafe {
            let js_val = qjs::JS_NewStringLen(ctx.as_ptr(), ptr as _, len as _);
            let js_val = ctx.handle_exception(js_val)?;
            String::from_js_value(ctx, js_val)
        })
    }
}

/// Borrowed UTF-16 code units exported by QuickJS.
/// The native buffer is released when this view is dropped.
pub struct Utf16String<'a, 'js> {
    ctx: &'a Ctx<'js>,
    ptr: *const u16,
    len: usize,
}

impl AsRef<[u16]> for Utf16String<'_, '_> {
    fn as_ref(&self) -> &[u16] {
        // SAFETY: Construction checked the non-null, aligned native buffer.
        // QuickJS retains its immutable contents until JS_FreeCStringUTF16.
        unsafe { slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl Drop for Utf16String<'_, '_> {
    fn drop(&mut self) {
        // SAFETY: This view uniquely owns the exported buffer reference and
        // keeps its originating context live until the matching native free.
        unsafe { qjs::JS_FreeCStringUTF16(self.ctx.as_ptr(), self.ptr) };
    }
}

/// Rust representation of a JavaScript C string.
#[derive(Debug)]
pub struct CString<'js> {
    ptr: NonNull<c_char>,
    len: usize,
    ctx: Ctx<'js>,
}

impl<'js> CString<'js> {
    /// Create a new JavaScript C string from a JavaScript string.
    pub fn from_string(string: String<'js>) -> Result<Self> {
        let mut len = mem::MaybeUninit::uninit();
        // SAFETY: The pointer points to a JSString content which is ref counted
        let ptr = unsafe {
            qjs::JS_ToCStringLen(string.0.ctx.as_ptr(), len.as_mut_ptr(), string.as_raw())
        };
        if ptr.is_null() {
            // Might not ever happen but I am not 100% sure
            // so just incase check it.
            return Err(Error::Unknown);
        }
        let len = unsafe { len.assume_init() };
        let result = Self {
            ptr: unsafe { NonNull::new_unchecked(ptr as *mut _) },
            len,
            ctx: string.0.ctx.clone(),
        };
        // SAFETY: The native export keeps these immutable bytes live until
        // result is dropped, including when UTF-8 validation fails below.
        let bytes = unsafe { slice::from_raw_parts(result.ptr.as_ptr() as *const u8, len) };
        // QuickJS preserves unmatched UTF-16 surrogates as WTF-8. Such bytes
        // cannot back the safe str views exposed by CString.
        str::from_utf8(bytes)?;
        Ok(result)
    }

    /// Converts a `CString` to a raw pointer.
    pub fn as_ptr(&self) -> *const c_char {
        self.ptr.as_ptr() as *const _
    }

    /// Returns the length of this `CString`, in bytes (not chars or graphemes).
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` if this `CString` has a length of zero, and `false` otherwise.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Extracts a string slice containing the entire `CString`.
    pub fn as_str(&self) -> &str {
        // SAFETY: The pointer points to a JSString content which is ref counted
        let bytes = unsafe { slice::from_raw_parts(self.ptr.as_ptr() as *const u8, self.len) };
        // SAFETY: from_string validated these immutable bytes as UTF-8.
        unsafe { str::from_utf8_unchecked(bytes) }
    }
}

impl<'js> Drop for CString<'js> {
    fn drop(&mut self) {
        unsafe { qjs::JS_FreeCString(self.ctx.as_ptr(), self.ptr.as_ptr()) };
    }
}
impl<'js> AsRef<str> for CString<'js> {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}
#[cfg(feature = "std")]
impl<'js> AsRef<std::path::Path> for CString<'js> {
    fn as_ref(&self) -> &std::path::Path {
        std::path::Path::new(self.as_str())
    }
}
impl<'js> core::ops::Deref for CString<'js> {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}
#[cfg(feature = "std")]
impl<'js> AsRef<std::ffi::OsStr> for CString<'js> {
    fn as_ref(&self) -> &std::ffi::OsStr {
        self.as_str().as_ref()
    }
}
impl<'js> AsRef<[u8]> for CString<'js> {
    fn as_ref(&self) -> &[u8] {
        self.as_str().as_ref()
    }
}
impl<'js> PartialEq for CString<'js> {
    fn eq(&self, other: &Self) -> bool {
        PartialEq::eq(self.as_str(), other.as_str())
    }
}
impl<'js> Eq for CString<'js> {}
impl<'js> core::hash::Hash for CString<'js> {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        core::hash::Hash::hash(self.as_str(), state)
    }
}

#[cfg(test)]
mod test {
    use crate::{prelude::*, *};
    #[test]
    fn from_javascript() {
        test_with(|ctx| {
            let s: String = ctx.eval(" 'foo bar baz' ").unwrap();
            assert_eq!(s.to_string().unwrap(), "foo bar baz");
        });
    }

    #[test]
    fn to_javascript() {
        test_with(|ctx| {
            let string = String::from_str(ctx.clone(), "foo").unwrap();
            let func: Function = ctx.eval("x =>  x + 'bar'").unwrap();
            let text: StdString = (string,).apply(&func).unwrap();
            assert_eq!(text, "foobar".to_string());
        });
    }

    #[test]
    fn from_javascript_c() {
        test_with(|ctx| {
            let s: CString = ctx.eval(" 'foo bar baz' ").unwrap();
            assert_eq!(s.as_str(), "foo bar baz");
        });
    }

    #[test]
    fn to_javascript_c() {
        test_with(|ctx| {
            let string = String::from_str(ctx.clone(), "foo")
                .unwrap()
                .to_cstring()
                .unwrap();
            let func: Function = ctx.eval("x =>  x + 'bar'").unwrap();
            let text: StdString = (string,).apply(&func).unwrap();
            assert_eq!(text, "foobar".to_string());
        });
    }

    #[test]
    fn lossy_utf16_preserves_pairs_and_replaces_unpaired_surrogates() {
        test_with(|ctx| {
            let string: String = ctx.eval(r#"'a\uD83D\uDE00\uD800b\uDC00'"#).unwrap();
            assert_eq!(
                string.utf16_units().unwrap().as_ref(),
                &[0x61, 0xD83D, 0xDE00, 0xD800, 0x62, 0xDC00]
            );
            assert_eq!(string.to_string_lossy().unwrap(), "a😀�b�");
            let rebuilt =
                String::from_utf16(ctx.clone(), string.utf16_units().unwrap().as_ref()).unwrap();
            assert_eq!(
                rebuilt.utf16_units().unwrap().as_ref(),
                string.utf16_units().unwrap().as_ref()
            );
        });
    }

    #[test]
    fn cstring_rejects_unpaired_surrogates_before_exposing_str() {
        test_with(|ctx| {
            for source in [r#"'\uD800'"#, r#"'\uDC00'"#, r#"'a\uD800b'"#] {
                let string: String = ctx.eval(source).unwrap();
                assert!(matches!(string.to_cstring(), Err(Error::Utf8(_))));
            }
            assert_eq!(ctx.eval::<i32, _>("6 * 7").unwrap(), 42);
        });
    }

    #[test]
    fn cstring_preserves_valid_unicode_and_embedded_nul() {
        test_with(|ctx| {
            for (source, expected) in [
                ("''", ""),
                (r#"'\uD83D\uDE00'"#, "😀"),
                ("'é'", "é"),
                (r#"'a\0b'"#, "a\0b"),
            ] {
                let string: String = ctx.eval(source).unwrap();
                let native = string.to_cstring().unwrap();
                assert_eq!(native.as_str(), expected);
                assert_eq!(native.len(), expected.len());
            }
        });
    }

    #[test]
    fn exact_utf16_export_handles_empty_latin1_and_ropes() {
        test_with(|ctx| {
            let empty: String = ctx.eval("''").unwrap();
            assert!(empty.utf16_units().unwrap().as_ref().is_empty());
            assert_eq!(empty.to_string_lossy().unwrap(), "");

            let latin1: String = ctx.eval("'é'").unwrap();
            assert_eq!(latin1.utf16_units().unwrap().as_ref(), &[0xE9]);
            assert_eq!(latin1.to_string_lossy().unwrap(), "é");
            let rebuilt = String::from_utf16(ctx.clone(), &[0xE9]).unwrap();
            assert_eq!(rebuilt.utf16_units().unwrap().as_ref(), &[0xE9]);

            let rope: String = ctx.eval("'ab'.repeat(4096) + 'é'").unwrap();
            let units = rope.utf16_units().unwrap();
            assert_eq!(units.as_ref().len(), 8193);
            assert_eq!(&units.as_ref()[..4], &[0x61, 0x62, 0x61, 0x62]);
            assert_eq!(units.as_ref()[8192], 0xE9);
        });
    }

    #[test]
    fn rope_string() {
        test_with(|ctx| {
            let val: Value = ctx
                .eval(
                    r#"
                    let s = "";
                    for (let i = 0; i < 10000; i++) s += "Line " + i + "\n";
                    s
                "#,
                )
                .unwrap();

            assert_eq!(val.type_of(), Type::String);
            assert!(val.is_string());
            assert!(val.as_string().is_some());

            let s: StdString = val.as_string().unwrap().to_string().unwrap();
            assert!(s.starts_with("Line 0\n"));
            assert!(s.contains("Line 999\n"));
            assert!(s.len() > 8000);
        });
    }
}
