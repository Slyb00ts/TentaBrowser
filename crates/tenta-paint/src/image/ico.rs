//! ICO and CUR containers. The entry with the most pixels (then the most bits
//! per pixel) wins; an entry is either a PNG or a headerless bitmap.

use super::bmp;
use super::ctx::Ctx;
use super::decode::RawImage;
use super::error::DecodeError;
use super::format::{ImageFormat, PNG_SIGNATURE};
use super::png;

const FORMAT: ImageFormat = ImageFormat::Ico;
const DIRECTORY_LEN: usize = 6;
const ENTRY_LEN: usize = 16;
/// A hostile directory can list 65535 entries; the best few are enough.
const MAX_ATTEMPTS: usize = 4;

struct Entry {
    pixels: u32,
    bits_per_pixel: u16,
    offset: usize,
    size: usize,
}

pub(super) fn decode(bytes: &[u8], ctx: &Ctx) -> Result<RawImage, DecodeError> {
    let count = bytes
        .get(4..DIRECTORY_LEN)
        .map(|c| usize::from(u16::from_le_bytes([c[0], c[1]])))
        .ok_or_else(truncated)?;
    let mut entries = Vec::with_capacity(count.min(bytes.len() / ENTRY_LEN));
    for i in 0..count {
        let at = DIRECTORY_LEN + i * ENTRY_LEN;
        let Some(e) = bytes.get(at..at + ENTRY_LEN) else {
            break;
        };
        // A stored 0 means 256.
        let dimension = |b: u8| if b == 0 { 256 } else { u32::from(b) };
        let offset = u32::from_le_bytes([e[12], e[13], e[14], e[15]]) as usize;
        let declared = u32::from_le_bytes([e[8], e[9], e[10], e[11]]) as usize;
        if offset >= bytes.len() {
            continue;
        }
        entries.push(Entry {
            pixels: dimension(e[0]) * dimension(e[1]),
            bits_per_pixel: u16::from_le_bytes([e[6], e[7]]),
            offset,
            // Writers get the size wrong often enough that the file's end is
            // the only bound to trust.
            size: declared.min(bytes.len() - offset),
        });
    }
    if entries.is_empty() {
        return Err(DecodeError::malformed(FORMAT, "no image entries"));
    }
    // Stable, so equal entries keep directory order.
    entries.sort_by_key(|e| std::cmp::Reverse((e.pixels, e.bits_per_pixel)));

    // An entry is shrunk afterwards like any other image, never while decoding.
    let ctx = ctx.nested();
    let mut first_error = None;
    for entry in entries.iter().take(MAX_ATTEMPTS) {
        ctx.check()?;
        let data = &bytes[entry.offset..entry.offset + entry.size];
        let decoded = if data.starts_with(&PNG_SIGNATURE) {
            png::decode(data, &ctx)
        } else {
            bmp::decode_dib(data, None, &ctx)
        };
        match decoded {
            Ok(image) => return Ok(image),
            // Stopping is not damage: the next entry would hit the same wall.
            Err(e @ (DecodeError::TimedOut | DecodeError::Cancelled)) => return Err(e),
            Err(e) => {
                first_error.get_or_insert(e);
            }
        }
    }
    Err(first_error.unwrap_or_else(|| DecodeError::malformed(FORMAT, "no decodable entry")))
}

fn truncated() -> DecodeError {
    DecodeError::malformed(FORMAT, "truncated directory")
}
