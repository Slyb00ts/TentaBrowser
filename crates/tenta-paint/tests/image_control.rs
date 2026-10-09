//! Deadlines and cancellation: a decode stops with a typed error between rows,
//! scans, frames and entries instead of running to the end.

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use support::{GifFrame, IconEntry, bmp, corpus, gif, ico, png_rgba8};
use tenta_paint::{
    DecodeControl, DecodeError, DecodeLimits, DecodeParams, ImageFormat,
    decode_format_with_control, decode_with_control,
};

fn run(
    format: ImageFormat,
    bytes: &[u8],
    control: &DecodeControl,
) -> Result<tenta_paint::DecodedImage, DecodeError> {
    decode_format_with_control(
        format,
        bytes,
        &DecodeParams::default(),
        &DecodeLimits::default(),
        control,
    )
}

#[test]
fn an_expired_deadline_stops_every_format() {
    let expired = DecodeControl::new().with_deadline(Instant::now());
    for (format, bytes) in corpus() {
        assert_eq!(
            run(format, &bytes, &expired).map(|i| i.width),
            Err(DecodeError::TimedOut),
            "{format}"
        );
    }
}

#[test]
fn a_raised_cancel_flag_stops_every_format() {
    let flag = Arc::new(AtomicBool::new(true));
    let control = DecodeControl::new().with_cancel(flag);
    for (format, bytes) in corpus() {
        assert_eq!(
            run(format, &bytes, &control).map(|i| i.width),
            Err(DecodeError::Cancelled),
            "{format}"
        );
    }
}

#[test]
fn a_generous_deadline_and_a_lowered_flag_change_nothing() {
    let flag = Arc::new(AtomicBool::new(false));
    let control = DecodeControl::new()
        .with_timeout(Duration::from_secs(600))
        .with_cancel(flag.clone());
    for (format, bytes) in corpus() {
        let plain = run(format, &bytes, &DecodeControl::new()).expect("decodes");
        assert_eq!(
            run(format, &bytes, &control).as_ref(),
            Ok(&plain),
            "{format}"
        );
    }
    assert!(!flag.load(Ordering::Relaxed));
    let sniffed = decode_with_control(
        &png_rgba8(),
        &DecodeParams::default(),
        &DecodeLimits::default(),
        &control,
    );
    assert!(sniffed.is_ok());
}

/// How long `decode` takes with no control, and how long it takes to give up
/// when the deadline is a tenth of that.
fn time_to_stop(format: ImageFormat, bytes: &[u8]) -> (Duration, Duration) {
    let started = Instant::now();
    run(format, bytes, &DecodeControl::new()).expect("decodes");
    let full = started.elapsed();

    let control = DecodeControl::new().with_timeout(full / 10);
    let started = Instant::now();
    let result = run(format, bytes, &control);
    let stopped = started.elapsed();
    assert_eq!(result.map(|i| i.width), Err(DecodeError::TimedOut));
    (full, stopped)
}

#[test]
fn a_png_stops_between_rows() {
    let side = 3000u32;
    let bytes = support::encode_png(
        &support::PngSpec {
            width: side,
            height: side,
            color: png::ColorType::Rgba,
            depth: png::BitDepth::Eight,
            palette: None,
            trns: None,
            gamma: None,
        },
        &vec![0x55u8; side as usize * side as usize * 4],
    );
    let (full, stopped) = time_to_stop(ImageFormat::Png, &bytes);
    assert!(
        stopped < full / 2,
        "stopped after {stopped:?}, a full decode takes {full:?}"
    );
}

#[test]
fn a_bitmap_stops_between_rows() {
    let side = 3000;
    let bytes = bmp(
        side,
        side,
        24,
        0,
        &[],
        &[],
        &vec![7; side as usize * side as usize * 3],
    );
    let (full, stopped) = time_to_stop(ImageFormat::Bmp, &bytes);
    assert!(
        stopped < full / 2,
        "stopped after {stopped:?}, a full decode takes {full:?}"
    );
}

#[test]
fn a_run_length_bitmap_stops_between_operations() {
    let side = 2000;
    let mut data = Vec::new();
    for _ in 0..side {
        for _ in 0..side {
            data.extend_from_slice(&[1, 1]);
        }
        data.extend_from_slice(&[0, 0]);
    }
    data.extend_from_slice(&[0, 1]);
    let bytes = bmp(side, side, 8, 1, &[], &support::bmp_palette(), &data);
    let (full, stopped) = time_to_stop(ImageFormat::Bmp, &bytes);
    assert!(
        stopped < full / 2,
        "stopped after {stopped:?}, a full decode takes {full:?}"
    );
}

/// A deadline that has passed when the icon directory is read stops the decode
/// instead of trying the remaining entries.
#[test]
fn an_icon_stops_instead_of_trying_the_next_entry() {
    let broken = |side: u8| IconEntry {
        width: side,
        height: side,
        bpp: 32,
        data: vec![0xAB; 64],
    };
    let bytes = ico(&[broken(48), broken(32), broken(16)]);
    let plain = run(ImageFormat::Ico, &bytes, &DecodeControl::new());
    assert!(
        matches!(plain, Err(DecodeError::Malformed { .. })),
        "{plain:?}"
    );
    let expired = DecodeControl::new().with_deadline(Instant::now());
    assert_eq!(
        run(ImageFormat::Ico, &bytes, &expired).map(|i| i.width),
        Err(DecodeError::TimedOut)
    );
}

#[test]
fn a_gif_checks_before_its_frame_is_decoded() {
    let bytes = gif((4, 4), &[GifFrame::new(0, 0, 4, 4, vec![0; 16])]);
    let flag = Arc::new(AtomicBool::new(false));
    let control = DecodeControl::new().with_cancel(flag.clone());
    assert!(run(ImageFormat::Gif, &bytes, &control).is_ok());
    flag.store(true, Ordering::Relaxed);
    assert_eq!(
        run(ImageFormat::Gif, &bytes, &control).map(|i| i.width),
        Err(DecodeError::Cancelled)
    );
}
