//! WebP via `image-webp`: lossy, lossless and alpha. An animated file yields
//! its first frame.

use std::io::Cursor;

use image_webp::WebPDecoder;

use super::ctx::Ctx;
use super::decode::RawImage;
use super::error::DecodeError;
use super::format::ImageFormat;
use super::orient::Orientation;
use super::pixels::Pixels;

const FORMAT: ImageFormat = ImageFormat::WebP;

/// Working memory beyond the RGBA output, measured with the counting allocator
/// on 1 to 9 Mpx images (`docs/dev/p3-plan.md`, IM1).
///
/// A lossy decode holds its YUV planes, the alpha plane when there is one, and
/// two copies of its token partitions, so it also scales with the input size.
/// A lossless decode keeps a 32-bit pixel array and transform buffers when
/// there is no alpha channel (the output buffer doubles as that array when
/// there is), and copies none of its input.
const LOSSY_EXTRA_QUARTER_BYTES_PER_PIXEL: u64 = 9;
const LOSSY_ALPHA_EXTRA_BYTES_PER_PIXEL: u64 = 8;
const LOSSLESS_OPAQUE_EXTRA_BYTES_PER_PIXEL: u64 = 6;
const LOSSLESS_ALPHA_EXTRA_BYTES_PER_PIXEL: u64 = 2;
const LOSSY_INPUT_COPIES: u64 = 2;

pub(super) fn decode(bytes: &[u8], ctx: &Ctx) -> Result<RawImage, DecodeError> {
    // Pinned to the `image-webp = "=0.2.4"` requirement in Cargo.toml: this
    // pre-scan exists only because that version sizes its planes from the VP8
    // bitstream before comparing them with the container. Revisit it, and
    // delete it if the crate gains a real limit, when the pin is lifted.
    check_vp8_frames(bytes.get(12..).unwrap_or(&[]), false, ctx)?;
    let mut decoder =
        WebPDecoder::new(Cursor::new(bytes)).map_err(|e| DecodeError::malformed(FORMAT, e))?;
    // In 0.2.4 this only bounds the metadata chunks (ICC, EXIF, XMP) that the
    // accessors read. It does not limit pixel memory; the checks below do.
    decoder.set_memory_limit(ctx.limits.max_decoded_bytes);
    let (width, height) = decoder.dimensions();
    let plan = ctx.plan(FORMAT, width, height, false)?;
    let input_copies = LOSSY_INPUT_COPIES * bytes.len() as u64;
    let extra = match (decoder.is_lossy(), decoder.has_alpha()) {
        (true, false) => plan.pixels * LOSSY_EXTRA_QUARTER_BYTES_PER_PIXEL / 4 + input_copies,
        (true, true) => plan.pixels * LOSSY_ALPHA_EXTRA_BYTES_PER_PIXEL + input_copies,
        (false, false) => plan.pixels * LOSSLESS_OPAQUE_EXTRA_BYTES_PER_PIXEL,
        (false, true) => plan.pixels * LOSSLESS_ALPHA_EXTRA_BYTES_PER_PIXEL,
    };
    ctx.admit(
        &plan,
        plan.source_bytes() + extra + plan.finish_bytes(false),
    )?;

    let size = decoder
        .output_buffer_size()
        .ok_or(DecodeError::TooLarge { width, height })?;
    let pixels = width as usize * height as usize;
    // The decoder writes RGB or RGBA into the front of the buffer; RGB is then
    // widened in place, last pixel first, so no second image is allocated.
    let mut rgba = Pixels::zeroed(pixels * 4)?;
    let out = rgba
        .get_mut(..size)
        .ok_or_else(|| DecodeError::malformed(FORMAT, "unexpected output size"))?;
    // image-webp cannot be interrupted, so the pixel limit above is what
    // bounds the time spent inside this call.
    ctx.check()?;
    decoder
        .read_image(out)
        .map_err(|e| DecodeError::malformed(FORMAT, e))?;
    if size != pixels * 4 {
        for i in (0..pixels).rev() {
            rgba.copy_within(i * 3..i * 3 + 3, i * 4);
            rgba[i * 4 + 3] = 255;
        }
    }
    Ok(RawImage::straight(
        width,
        height,
        rgba,
        Orientation::Identity,
    ))
}

/// Bytes of a VP8 key frame before its first partition: the frame tag, the
/// start code and the two dimensions.
const VP8_FRAME_HEADER: usize = 10;

/// Refuses a VP8 frame whose token partitions claim more bytes than the frame
/// holds.
///
/// `image-webp` allocates each partition from its 24-bit declared size before
/// reading it, so a tiny frame could make it reserve 16 MiB per partition. The
/// partition count sits in the bool-coded frame header (RFC 6386 sections 9.2
/// to 9.6); this reads that header just far enough to find it, then checks the
/// size table that follows the first partition.
fn check_vp8_partitions(frame: &[u8]) -> Result<(), DecodeError> {
    let too_short = || DecodeError::malformed(FORMAT, "VP8 frame shorter than its partitions");
    let tag = u32::from_le_bytes([frame[0], frame[1], frame[2], 0]);
    let first_partition = (tag >> 5) as usize;
    let header = frame
        .get(VP8_FRAME_HEADER..)
        .and_then(|rest| rest.get(..first_partition))
        .ok_or_else(too_short)?;
    let count = 1usize << partition_count_log2(header);

    let table_at = VP8_FRAME_HEADER + first_partition;
    let table = frame
        .get(table_at..table_at + 3 * (count - 1))
        .ok_or_else(too_short)?;
    let mut remaining = frame.len() - table_at - table.len();
    for size in table.as_chunks::<3>().0 {
        let size = usize::from(size[0]) | usize::from(size[1]) << 8 | usize::from(size[2]) << 16;
        remaining = remaining.checked_sub(size).ok_or_else(too_short)?;
    }
    Ok(())
}

/// `log2` of the number of token partitions, read from a key frame's first
/// partition. Reads past the end yield zeros, as the decoder's own do.
fn partition_count_log2(first_partition: &[u8]) -> u32 {
    let mut b = BoolReader::new(first_partition);
    // Colour space and clamping type.
    b.literal(2);
    // Segment-based adjustments.
    if b.flag() {
        let update_map = b.flag();
        if b.flag() {
            // Quantizer and loop-filter values for the four segments.
            b.flag();
            for _ in 0..4 {
                b.optional_signed(7);
            }
            for _ in 0..4 {
                b.optional_signed(6);
            }
        }
        if update_map {
            for _ in 0..3 {
                if b.flag() {
                    b.literal(8);
                }
            }
        }
    }
    // Filter type, level and sharpness.
    b.literal(1 + 6 + 3);
    // Loop-filter delta adjustments.
    if b.flag() && b.flag() {
        for _ in 0..8 {
            b.optional_signed(6);
        }
    }
    b.literal(2)
}

/// The boolean entropy decoder of RFC 6386 section 7.
struct BoolReader<'a> {
    data: &'a [u8],
    next: usize,
    value: u32,
    range: u32,
    bit_count: u32,
}

impl<'a> BoolReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        let byte = |i: usize| u32::from(data.get(i).copied().unwrap_or(0));
        Self {
            data,
            next: 2,
            value: byte(0) << 8 | byte(1),
            range: 255,
            bit_count: 0,
        }
    }

    fn bool(&mut self, probability: u32) -> bool {
        let split = 1 + (((self.range - 1) * probability) >> 8);
        let big_split = split << 8;
        let bit = if self.value >= big_split {
            self.range -= split;
            self.value -= big_split;
            true
        } else {
            self.range = split;
            false
        };
        while self.range < 128 {
            self.value <<= 1;
            self.range <<= 1;
            self.bit_count += 1;
            if self.bit_count == 8 {
                self.bit_count = 0;
                self.value |= u32::from(self.data.get(self.next).copied().unwrap_or(0));
                self.next += 1;
            }
        }
        bit
    }

    fn flag(&mut self) -> bool {
        self.bool(128)
    }

    /// An unsigned `bits`-wide value, most significant bit first.
    fn literal(&mut self, bits: u32) -> u32 {
        (0..bits).fold(0, |v, _| v << 1 | u32::from(self.flag()))
    }

    /// A flag, then if set a magnitude of `bits` and a sign.
    fn optional_signed(&mut self, bits: u32) {
        if self.flag() {
            self.literal(bits);
            self.flag();
        }
    }
}

/// Bounds what `image-webp` allocates from sizes the file declares.
///
/// It allocates its YUV planes from the dimensions inside the VP8 bitstream
/// and compares them with the container's only afterwards, so a small file can
/// make it reserve up to 16383x16383 pixels; and it allocates every chunk it
/// reads from the chunk's declared size before reading it, so a 30-byte file
/// can ask for 4 GiB. Walking the RIFF chunks (and the sub-chunks of animation
/// frames) first, refusing a chunk that claims more than the file holds and
/// checking each VP8 frame header, keeps both inside the limits.
fn check_vp8_frames(mut chunks: &[u8], in_frame: bool, ctx: &Ctx) -> Result<(), DecodeError> {
    const ANMF_HEADER: usize = 16;
    while let Some(header) = chunks.get(..8) {
        let size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
        let body = &chunks[8..];
        if size > body.len() {
            return Err(DecodeError::malformed(
                FORMAT,
                "chunk larger than the data that follows",
            ));
        }
        let payload = &body[..size];
        match &header[..4] {
            b"VP8 " => {
                // Frame tag (3 bytes), start code, then two 14-bit dimensions.
                if let Some(frame) = payload.get(..10).filter(|f| f[3..6] == [0x9D, 0x01, 0x2A]) {
                    let width = u32::from(u16::from_le_bytes([frame[6], frame[7]]) & 0x3FFF);
                    let height = u32::from(u16::from_le_bytes([frame[8], frame[9]]) & 0x3FFF);
                    ctx.plan(FORMAT, width, height, false)?;
                    check_vp8_partitions(payload)?;
                }
            }
            // Frames do not nest; refusing to recurse keeps the stack flat.
            b"ANMF" if !in_frame => {
                check_vp8_frames(payload.get(ANMF_HEADER..).unwrap_or(&[]), true, ctx)?
            }
            _ => {}
        }
        // Chunks are padded to an even size.
        chunks = body.get(size.saturating_add(size & 1)..).unwrap_or(&[]);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The boolean entropy encoder of RFC 6386 section 7.3.
    struct BoolWriter {
        out: Vec<u8>,
        range: u32,
        bottom: u32,
        bit_count: u32,
    }

    impl BoolWriter {
        fn new() -> Self {
            Self {
                out: Vec::new(),
                range: 255,
                bottom: 0,
                bit_count: 24,
            }
        }

        fn carry(&mut self) {
            for byte in self.out.iter_mut().rev() {
                if *byte == 255 {
                    *byte = 0;
                } else {
                    *byte += 1;
                    break;
                }
            }
        }

        fn bool(&mut self, probability: u32, value: bool) {
            let split = 1 + (((self.range - 1) * probability) >> 8);
            if value {
                self.bottom = self.bottom.wrapping_add(split);
                self.range -= split;
            } else {
                self.range = split;
            }
            while self.range < 128 {
                self.range <<= 1;
                if self.bottom & (1 << 31) != 0 {
                    self.carry();
                }
                self.bottom <<= 1;
                self.bit_count -= 1;
                if self.bit_count == 0 {
                    self.out.push((self.bottom >> 24) as u8);
                    self.bottom &= (1 << 24) - 1;
                    self.bit_count = 8;
                }
            }
        }

        fn flag(&mut self, value: bool) {
            self.bool(128, value);
        }

        fn literal(&mut self, value: u32, bits: u32) {
            for i in (0..bits).rev() {
                self.flag(value >> i & 1 == 1);
            }
        }

        fn optional_signed(&mut self, bits: u32) {
            self.flag(true);
            self.literal(5, bits);
            self.flag(true);
        }

        fn finish(mut self) -> Vec<u8> {
            let mut c = self.bit_count;
            let mut v = self.bottom;
            if v & (1 << (32 - c)) != 0 {
                self.carry();
            }
            v <<= c & 7;
            c >>= 3;
            for _ in 0..c {
                v <<= 8;
            }
            for _ in 0..4 {
                self.out.push((v >> 24) as u8);
                v <<= 8;
            }
            self.out
        }
    }

    /// A first partition holding a key-frame header with optional segment and
    /// loop-filter adjustments, ending in `log2` partitions.
    fn first_partition(segments: bool, deltas: bool, log2: u32) -> Vec<u8> {
        let mut w = BoolWriter::new();
        w.literal(0, 2);
        w.flag(segments);
        if segments {
            w.flag(true);
            w.flag(true);
            w.flag(true);
            for _ in 0..4 {
                w.optional_signed(7);
            }
            for _ in 0..4 {
                w.optional_signed(6);
            }
            for _ in 0..3 {
                w.flag(true);
                w.literal(77, 8);
            }
        }
        w.literal(0b10_1010_1011, 10);
        w.flag(deltas);
        if deltas {
            w.flag(true);
            for _ in 0..8 {
                w.optional_signed(6);
            }
        }
        w.literal(log2, 2);
        w.literal(0xA5, 8);
        w.finish()
    }

    /// A VP8 key frame with `1 << log2` partitions, the first `sizes.len()`
    /// declared as given and the rest of `data` as the last partition.
    fn frame(first: &[u8], log2: u32, sizes: &[usize], data: usize) -> Vec<u8> {
        let tag = (first.len() as u32) << 5 | 1 << 4;
        let mut out = tag.to_le_bytes()[..3].to_vec();
        out.extend_from_slice(&[0x9D, 0x01, 0x2A, 16, 0, 16, 0]);
        out.extend_from_slice(first);
        assert_eq!(sizes.len(), (1 << log2) - 1);
        for size in sizes {
            out.extend_from_slice(&size.to_le_bytes()[..3]);
        }
        out.resize(out.len() + data, 0);
        out
    }

    #[test]
    fn the_partition_count_is_found_behind_every_optional_header_part() {
        for (segments, deltas) in [(false, false), (true, false), (false, true), (true, true)] {
            for log2 in 0..4 {
                let first = first_partition(segments, deltas, log2);
                assert_eq!(
                    partition_count_log2(&first),
                    log2,
                    "segments {segments}, deltas {deltas}"
                );
            }
        }
    }

    #[test]
    fn partitions_that_fit_the_frame_pass() {
        for (segments, log2) in [(false, 0), (true, 1), (false, 3), (true, 3)] {
            let first = first_partition(segments, segments, log2);
            let sizes = vec![100; (1 << log2) - 1];
            let frame = frame(&first, log2, &sizes, 100 * sizes.len() + 7);
            assert_eq!(check_vp8_partitions(&frame), Ok(()), "{log2} partitions");
        }
    }

    #[test]
    fn partitions_larger_than_the_frame_are_refused() {
        let first = first_partition(true, true, 3);
        // Seven declared partitions of 16 MiB each, in a frame of a few bytes.
        let huge = frame(&first, 3, &[0xFF_FFFF; 7], 12);
        assert!(matches!(
            check_vp8_partitions(&huge),
            Err(DecodeError::Malformed { .. })
        ));
        // Each fits alone but together they do not.
        let sum = frame(&first, 3, &[100; 7], 699);
        assert!(check_vp8_partitions(&sum).is_err());
        // A size table cut off by the end of the frame.
        let mut cut = frame(&first, 3, &[1; 7], 0);
        cut.truncate(VP8_FRAME_HEADER + first.len() + 5);
        assert!(check_vp8_partitions(&cut).is_err());
        // A first partition that claims more than the frame.
        let mut long = frame(&first, 0, &[], 4);
        long[0] = 0xFF;
        long[1] = 0xFF;
        assert!(check_vp8_partitions(&long).is_err());
    }
}
