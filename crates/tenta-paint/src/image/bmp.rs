//! Windows bitmaps: OS/2 and Windows headers up to V5, 1 to 32 bits per pixel,
//! palettes, `BITFIELDS` masks and RLE4/RLE8. The DIB reader is shared with ICO,
//! whose entries are headerless bitmaps followed by a 1-bit transparency mask.

use super::ctx::Ctx;
use super::decode::RawImage;
use super::error::DecodeError;
use super::format::ImageFormat;
use super::orient::Orientation;
use super::pixels::Pixels;

const FORMAT: ImageFormat = ImageFormat::Bmp;
const FILE_HEADER_LEN: usize = 14;
const CORE_HEADER_LEN: usize = 12;
const INFO_HEADER_LEN: usize = 40;

const BI_RGB: u32 = 0;
const BI_RLE8: u32 = 1;
const BI_RLE4: u32 = 2;
const BI_BITFIELDS: u32 = 3;
const BI_JPEG: u32 = 4;
const BI_PNG: u32 = 5;
const BI_ALPHABITFIELDS: u32 = 6;

pub(super) fn decode(bytes: &[u8], ctx: &Ctx) -> Result<RawImage, DecodeError> {
    if bytes.len() < FILE_HEADER_LEN + CORE_HEADER_LEN {
        return Err(truncated());
    }
    let offset = le32(bytes, 10)? as usize;
    let pixel_start = offset
        .checked_sub(FILE_HEADER_LEN)
        .ok_or_else(|| DecodeError::malformed(FORMAT, "pixel offset inside the file header"))?;
    decode_dib(&bytes[FILE_HEADER_LEN..], Some(pixel_start), ctx)
}

/// Decodes a bitmap without file header. `pixel_start` is the offset of the
/// pixel data inside `dib`; `None` marks an icon entry, where the pixels follow
/// the colour table, the height counts the mask rows and a mask follows.
pub(super) fn decode_dib(
    dib: &[u8],
    pixel_start: Option<usize>,
    ctx: &Ctx,
) -> Result<RawImage, DecodeError> {
    ctx.check()?;
    let icon = pixel_start.is_none();
    let header = Header::parse(dib, icon)?;
    // The RGBA buffer is all there is; a shrink or the ICO mask adds nothing.
    let plan = ctx.plan(FORMAT, header.width, header.height, false)?;
    ctx.admit(&plan, plan.source_bytes() + plan.finish_bytes(false))?;

    let pixel_start = match pixel_start {
        Some(start) => start,
        None => header.table_end(),
    };
    let data = dib.get(pixel_start..).ok_or_else(truncated)?;
    let (w, h) = (header.width as usize, header.height as usize);

    let mut rgba = match header.compression {
        BI_RLE8 | BI_RLE4 => decode_rle(data, &header, ctx)?,
        _ => decode_rows(data, &header, ctx)?,
    };

    let has_alpha_channel = header.masks.a != 0;
    let translucent = has_alpha_channel && rgba.as_chunks::<4>().0.iter().any(|px| px[3] != 0);
    if has_alpha_channel && !translucent {
        // An alpha channel that is zero everywhere is a writer that left the
        // byte unused, not a fully transparent picture.
        for px in rgba.as_chunks_mut::<4>().0 {
            px[3] = 255;
        }
    }
    if icon && !translucent && !matches!(header.compression, BI_RLE8 | BI_RLE4) {
        let mask_start = pixel_start.saturating_add(header.stride().saturating_mul(h));
        apply_icon_mask(&mut rgba, w, h, dib.get(mask_start..).unwrap_or(&[]), ctx)?;
    }

    Ok(RawImage::straight(
        header.width,
        header.height,
        rgba,
        Orientation::Identity,
    ))
}

struct Header {
    size: usize,
    width: u32,
    height: u32,
    top_down: bool,
    bpp: u32,
    compression: u32,
    palette: Vec<[u8; 3]>,
    /// Bytes between the end of the header and the pixel data.
    table_len: usize,
    masks: Masks,
}

struct Masks {
    r: u32,
    g: u32,
    b: u32,
    a: u32,
}

impl Header {
    fn parse(dib: &[u8], icon: bool) -> Result<Self, DecodeError> {
        let size = le32(dib, 0)? as usize;
        if size > dib.len() {
            return Err(truncated());
        }
        let (width, raw_height, bpp, compression, colors_used) = if size == CORE_HEADER_LEN {
            (
                i32::from(le16(dib, 4)?),
                i32::from(le16(dib, 6)?),
                u32::from(le16(dib, 10)?),
                BI_RGB,
                0,
            )
        } else if size >= INFO_HEADER_LEN {
            (
                le32(dib, 4)? as i32,
                le32(dib, 8)? as i32,
                u32::from(le16(dib, 14)?),
                le32(dib, 16)?,
                le32(dib, 32)? as usize,
            )
        } else {
            return Err(DecodeError::malformed(FORMAT, "unknown header size"));
        };
        if width <= 0 || raw_height == 0 || raw_height == i32::MIN {
            return Err(DecodeError::malformed(FORMAT, "invalid dimensions"));
        }
        let top_down = raw_height < 0;
        let mut height = raw_height.unsigned_abs();
        if icon {
            height /= 2;
        }

        match (compression, bpp) {
            (BI_RGB, 1 | 4 | 8 | 16 | 24 | 32) => {}
            (BI_RLE8, 8) | (BI_RLE4, 4) => {
                if top_down {
                    return Err(DecodeError::malformed(FORMAT, "top-down RLE"));
                }
            }
            (BI_BITFIELDS | BI_ALPHABITFIELDS, 16 | 32) => {}
            (BI_JPEG | BI_PNG, _) => {
                return Err(DecodeError::Unsupported {
                    format: FORMAT,
                    feature: "embedded JPEG or PNG compression",
                });
            }
            _ => {
                return Err(DecodeError::malformed(
                    FORMAT,
                    "unsupported depth or compression",
                ));
            }
        }

        let bitfields = matches!(compression, BI_BITFIELDS | BI_ALPHABITFIELDS);
        let mut table_at = size;
        let mut masks = match bpp {
            16 => Masks {
                r: 0x7C00,
                g: 0x03E0,
                b: 0x001F,
                a: 0,
            },
            32 => Masks {
                r: 0x00FF_0000,
                g: 0x0000_FF00,
                b: 0x0000_00FF,
                a: 0xFF00_0000,
            },
            _ => Masks {
                r: 0,
                g: 0,
                b: 0,
                a: 0,
            },
        };
        if bitfields {
            // Plain 40-byte headers carry the masks after themselves; larger
            // headers embed them.
            let (at, with_alpha) = if size >= 52 {
                (40, size >= 56)
            } else {
                table_at += if compression == BI_ALPHABITFIELDS {
                    16
                } else {
                    12
                };
                (size, compression == BI_ALPHABITFIELDS)
            };
            masks = Masks {
                r: le32(dib, at)?,
                g: le32(dib, at + 4)?,
                b: le32(dib, at + 8)?,
                a: if with_alpha { le32(dib, at + 12)? } else { 0 },
            };
        }

        let entry_len = if size == CORE_HEADER_LEN { 3 } else { 4 };
        let colors = if bpp <= 8 {
            let full = 1usize << bpp;
            if colors_used == 0 {
                full
            } else {
                colors_used.min(full)
            }
        } else {
            // Unused, but present in some files; it still occupies the space.
            colors_used.min(256)
        };
        let palette = if bpp <= 8 {
            dib.get(table_at..)
                .and_then(|t| t.get(..colors * entry_len))
                .ok_or_else(truncated)?
                .chunks_exact(entry_len)
                .map(|e| [e[2], e[1], e[0]])
                .collect()
        } else {
            Vec::new()
        };

        Ok(Self {
            size,
            width: width as u32,
            height,
            top_down,
            bpp,
            compression,
            palette,
            table_len: table_at - size + colors * entry_len,
            masks,
        })
    }

    fn table_end(&self) -> usize {
        self.size + self.table_len
    }

    /// Bytes per uncompressed pixel row, padded to 32 bits.
    fn stride(&self) -> usize {
        (self.width as usize * self.bpp as usize).div_ceil(32) * 4
    }

    fn out_row(&self, file_row: usize) -> usize {
        if self.top_down {
            file_row
        } else {
            self.height as usize - 1 - file_row
        }
    }
}

fn decode_rows(data: &[u8], header: &Header, ctx: &Ctx) -> Result<Pixels, DecodeError> {
    let (w, h) = (header.width as usize, header.height as usize);
    let stride = header.stride();
    if data.len() < stride.saturating_mul(h) {
        return Err(truncated());
    }
    let channel = |mask: u32, default: u8| Channel::new(mask, default);
    let (r, g, b, a) = (
        channel(header.masks.r, 0),
        channel(header.masks.g, 0),
        channel(header.masks.b, 0),
        channel(header.masks.a, 255),
    );
    let mut rgba = Pixels::zeroed(w * h * 4)?;
    for file_row in 0..h {
        ctx.checkpoint(file_row, w * 4)?;
        let row = &data[file_row * stride..(file_row + 1) * stride];
        let out = &mut rgba[header.out_row(file_row) * w * 4..][..w * 4];
        for (x, px) in out.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            match header.bpp {
                1 | 4 | 8 => {
                    let bit = x * header.bpp as usize;
                    let shift = 8 - header.bpp as usize - bit % 8;
                    let index = usize::from(row[bit / 8] >> shift) & ((1 << header.bpp) - 1);
                    let [pr, pg, pb] = header.palette.get(index).copied().unwrap_or([0; 3]);
                    px.copy_from_slice(&[pr, pg, pb, 255]);
                }
                24 => px.copy_from_slice(&[row[x * 3 + 2], row[x * 3 + 1], row[x * 3], 255]),
                16 => {
                    let v = u32::from(u16::from_le_bytes([row[x * 2], row[x * 2 + 1]]));
                    px.copy_from_slice(&[r.get(v), g.get(v), b.get(v), a.get(v)]);
                }
                _ => {
                    let v = u32::from_le_bytes([
                        row[x * 4],
                        row[x * 4 + 1],
                        row[x * 4 + 2],
                        row[x * 4 + 3],
                    ]);
                    px.copy_from_slice(&[r.get(v), g.get(v), b.get(v), a.get(v)]);
                }
            }
        }
    }
    Ok(rgba)
}

/// One colour component selected by a bit mask and scaled to 8 bits.
struct Channel {
    mask: u32,
    shift: u32,
    max: u64,
    default: u8,
}

impl Channel {
    fn new(mask: u32, default: u8) -> Self {
        let shift = if mask == 0 { 0 } else { mask.trailing_zeros() };
        let bits = (mask >> shift).trailing_ones();
        Self {
            mask,
            shift,
            max: (1u64 << bits) - 1,
            default,
        }
    }

    fn get(&self, pixel: u32) -> u8 {
        if self.mask == 0 {
            return self.default;
        }
        let value = u64::from((pixel & self.mask) >> self.shift).min(self.max);
        ((value * 255 + self.max / 2) / self.max) as u8
    }
}

/// RLE bitmaps leave skipped pixels untouched, and those stay transparent.
fn decode_rle(data: &[u8], header: &Header, ctx: &Ctx) -> Result<Pixels, DecodeError> {
    let (w, h) = (header.width as usize, header.height as usize);
    let nibbles = header.compression == BI_RLE4;
    let mut rgba = Pixels::zeroed(w * h * 4)?;
    let (mut x, mut y) = (0usize, 0usize);
    let mut put = |x: usize, y: usize, index: u8| {
        if x < w && y < h {
            let [r, g, b] = header
                .palette
                .get(usize::from(index))
                .copied()
                .unwrap_or([0; 3]);
            let at = (header.out_row(y) * w + x) * 4;
            rgba[at..at + 4].copy_from_slice(&[r, g, b, 255]);
        }
    };
    let mut pos = 0;
    let mut operations = 0usize;
    while y < h {
        // A run is at most 255 pixels, so this bounds the time between checks.
        ctx.checkpoint(operations, 255)?;
        operations += 1;
        let (Some(&count), Some(&value)) = (data.get(pos), data.get(pos + 1)) else {
            break;
        };
        pos += 2;
        if count > 0 {
            for i in 0..usize::from(count) {
                let index = match (nibbles, i % 2) {
                    (false, _) => value,
                    (true, 0) => value >> 4,
                    (true, _) => value & 15,
                };
                put(x, y, index);
                x += 1;
            }
            continue;
        }
        match value {
            0 => {
                x = 0;
                y += 1;
            }
            1 => break,
            2 => {
                let (Some(&dx), Some(&dy)) = (data.get(pos), data.get(pos + 1)) else {
                    break;
                };
                pos += 2;
                x += usize::from(dx);
                y += usize::from(dy);
            }
            run => {
                let run = usize::from(run);
                let bytes = if nibbles { run.div_ceil(2) } else { run };
                let Some(literal) = data.get(pos..pos + bytes) else {
                    break;
                };
                for i in 0..run {
                    let index = match (nibbles, i % 2) {
                        (false, _) => literal[i],
                        (true, 0) => literal[i / 2] >> 4,
                        (true, _) => literal[i / 2] & 15,
                    };
                    put(x, y, index);
                    x += 1;
                }
                // Literal runs are padded to a 16-bit boundary.
                pos += bytes + bytes % 2;
            }
        }
    }
    Ok(rgba)
}

/// Clears alpha where the icon's 1-bit mask is set. A missing mask is opaque.
fn apply_icon_mask(
    rgba: &mut [u8],
    w: usize,
    h: usize,
    mask: &[u8],
    ctx: &Ctx,
) -> Result<(), DecodeError> {
    let stride = w.div_ceil(32) * 4;
    if mask.len() < stride * h {
        return Ok(());
    }
    for file_row in 0..h {
        ctx.checkpoint(file_row, w)?;
        let out_row = h - 1 - file_row;
        for x in 0..w {
            if (mask[file_row * stride + x / 8] >> (7 - x % 8)) & 1 == 1 {
                rgba[(out_row * w + x) * 4 + 3] = 0;
            }
        }
    }
    Ok(())
}

fn truncated() -> DecodeError {
    DecodeError::malformed(FORMAT, "truncated data")
}

fn le16(data: &[u8], at: usize) -> Result<u16, DecodeError> {
    let bytes = data.get(at..at + 2).ok_or_else(truncated)?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn le32(data: &[u8], at: usize) -> Result<u32, DecodeError> {
    let bytes = data.get(at..at + 4).ok_or_else(truncated)?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}
