//! Lossless JPEG XL for interleaved RGB u16 and gray u16, through the
//! vendored libjxl (`native/jxl_c_api.h`), built with the DNG SDK.

use std::ffi::c_void;
use std::io::{Seek, SeekFrom, Write};

use crate::{Budget, Cause, DngError, DtStatus, MAX_READ_PIXELS, Result, check, check_pixel_count};

/// libjxl's fast-lossless path. Higher efforts (the DNG writer uses 7) are
/// one to two orders of magnitude slower for only a few percent smaller
/// files, so project storage uses this.
pub const FAST_EFFORT: i32 = 1;

/// `DtJxlWrite` and `DtJxlSeek`: the writer an encode goes to.
type WriteFn = unsafe extern "C" fn(ctx: *mut c_void, data: *const u8, size: usize) -> i32;
type SeekFn = unsafe extern "C" fn(ctx: *mut c_void, position: u64) -> i32;

// The C API (`native/jxl_c_api.h`). Every entry point catches C++
// exceptions, so none unwinds into Rust; buffers are passed with their
// exact length, which the C side checks against the image size.
unsafe extern "C" {
    #[allow(clippy::too_many_arguments)]
    fn dt_jxl_encode(
        samples: *const c_void,
        sample_count: usize,
        width: u32,
        height: u32,
        channels: i32,
        bits: i32,
        effort: i32,
        linear: i32,
        threads: i32,
        cancel: *const u8,
        ctx: *mut c_void,
        write: WriteFn,
        seek: SeekFn,
        status: *mut DtStatus,
    ) -> i32;
    fn dt_jxl_info(
        data: *const u8,
        size: usize,
        width: *mut u32,
        height: *mut u32,
        channels: *mut u32,
        status: *mut DtStatus,
    ) -> i32;
    fn dt_jxl_decode(
        data: *const u8,
        size: usize,
        out: *mut c_void,
        sample_count: usize,
        channels: i32,
        bits: i32,
        threads: i32,
        cancel: *const u8,
        status: *mut DtStatus,
    ) -> i32;
}

/// The writer an encode goes to, and the first error it gave.
struct Sink<'a, W> {
    out: &'a mut W,
    error: Option<std::io::Error>,
}

/// `DtJxlWrite` over a [`Sink`].
unsafe extern "C" fn write_to<W: Write + Seek>(
    ctx: *mut c_void,
    data: *const u8,
    size: usize,
) -> i32 {
    // SAFETY: `ctx` is the `Sink` `encode` passed, live for the call, and
    // `data` holds `size` bytes.
    let (sink, data) =
        unsafe { (&mut *ctx.cast::<Sink<W>>(), std::slice::from_raw_parts(data, size)) };
    match sink.out.write_all(data) {
        Ok(()) => 1,
        Err(e) => {
            sink.error.get_or_insert(e);
            0
        }
    }
}

/// `DtJxlSeek` over a [`Sink`].
unsafe extern "C" fn seek_in<W: Write + Seek>(ctx: *mut c_void, position: u64) -> i32 {
    // SAFETY: as in `write_to`.
    let sink = unsafe { &mut *ctx.cast::<Sink<W>>() };
    match sink.out.seek(SeekFrom::Start(position)) {
        Ok(_) => 1,
        Err(e) => {
            sink.error.get_or_insert(e);
            0
        }
    }
}

/// The sample layouts the codec takes.
trait Layout {
    type Sample: Copy + Default;
    const CHANNELS: u32;
    const BITS: i32;
}

/// Interleaved RGB u16.
struct Rgb16;
impl Layout for Rgb16 {
    type Sample = u16;
    const CHANNELS: u32 = 3;
    const BITS: i32 = 16;
}

/// Gray u16.
struct Gray16;
impl Layout for Gray16 {
    type Sample = u16;
    const CHANNELS: u32 = 1;
    const BITS: i32 = 16;
}

/// A call's failure: canceled if `budget`'s flag was set.
fn failed_within(budget: &Budget, what: &'static str, cause: Cause) -> DngError {
    if budget.cancel.load(std::sync::atomic::Ordering::Relaxed) {
        DngError::Canceled
    } else {
        DngError::JxlFailed { what, cause }
    }
}

/// A codestream whose header doesn't fit what it should hold.
fn damaged(what: &'static str) -> DngError {
    DngError::JxlFailed { what, cause: Cause::Damaged }
}

fn threads(budget: &Budget) -> i32 {
    budget.threads.min(i32::MAX as usize) as i32
}

fn encode<L: Layout, W: Write + Seek>(
    out: &mut W,
    samples: &[L::Sample],
    width: u32,
    height: u32,
    effort: i32,
    linear: bool,
    budget: Budget,
) -> Result<()> {
    check_pixel_count(samples.len(), width, height, L::CHANNELS)?;
    let mut sink = Sink { out, error: None };
    let mut status = DtStatus::new();
    // SAFETY: `samples` is a live slice of `samples.len()` elements of the
    // width `L::BITS` names, which the encoder only reads during the call;
    // `sink` and `status` outlive it, and the callbacks take `sink` as the
    // `Sink<W>` it is.
    let ok = unsafe {
        dt_jxl_encode(
            samples.as_ptr().cast(),
            samples.len(),
            width,
            height,
            L::CHANNELS as i32,
            L::BITS,
            effort,
            linear as i32,
            threads(&budget),
            budget.cancel.as_ptr().cast_const().cast(),
            (&raw mut sink).cast(),
            write_to::<W>,
            seek_in::<W>,
            &mut status,
        )
    };
    if let Some(e) = sink.error {
        return Err(DngError::JxlWrite(e));
    }
    check(ok, &status, |cause| failed_within(&budget, "encode", cause))?;
    sink.out.flush().map_err(DngError::JxlWrite)
}

fn decode<L: Layout>(data: &[u8], budget: Budget) -> Result<(Vec<L::Sample>, u32, u32)> {
    let (mut width, mut height, mut channels) = (0, 0, 0);
    let mut status = DtStatus::new();
    // SAFETY: `data` is read for `data.len()` bytes; the out-pointers are
    // live locals.
    let ok = unsafe {
        dt_jxl_info(data.as_ptr(), data.len(), &mut width, &mut height, &mut channels, &mut status)
    };
    check(ok, &status, |cause| DngError::JxlFailed { what: "decode", cause })?;
    if channels != L::CHANNELS {
        return Err(damaged("decode: unexpected channel count"));
    }
    let pixels = u64::from(width) * u64::from(height);
    if pixels == 0 || pixels > MAX_READ_PIXELS {
        return Err(damaged("decode: image size out of range"));
    }
    let sample_count = usize::try_from(pixels * u64::from(channels))
        .map_err(|_| damaged("decode: image size out of range"))?;
    let mut samples = vec![L::Sample::default(); sample_count];
    // SAFETY: `samples` holds `sample_count` elements of the width
    // `L::BITS` names, and the decoder writes no more than that.
    let ok = unsafe {
        dt_jxl_decode(
            data.as_ptr(),
            data.len(),
            samples.as_mut_ptr().cast(),
            samples.len(),
            L::CHANNELS as i32,
            L::BITS,
            threads(&budget),
            budget.cancel.as_ptr().cast_const().cast(),
            &mut status,
        )
    };
    check(ok, &status, |cause| failed_within(&budget, "decode", cause))?;
    Ok((samples, width, height))
}

/// Encode interleaved RGB u16 losslessly into `out` from where it stands,
/// streamed as libjxl finishes each part (it seeks back once for the table
/// of contents). `linear` only tags the transfer for other viewers. On
/// `budget`'s threads; its flag stops it, as [`DngError::Canceled`].
pub fn encode_rgb16(
    out: &mut (impl Write + Seek),
    pixels: &[u16],
    width: u32,
    height: u32,
    effort: i32,
    linear: bool,
    budget: Budget,
) -> Result<()> {
    encode::<Rgb16, _>(out, pixels, width, height, effort, linear, budget)
}

/// Decode a codestream written by [`encode_rgb16`]: `(pixels, width, height)`.
pub fn decode_rgb16(data: &[u8], budget: Budget) -> Result<(Vec<u16>, u32, u32)> {
    decode::<Rgb16>(data, budget)
}

/// Encode 16-bit gray losslessly into `out`, as [`encode_rgb16`], tagged
/// linear (the samples are data, not tones).
pub fn encode_gray16(
    out: &mut (impl Write + Seek),
    pixels: &[u16],
    width: u32,
    height: u32,
    effort: i32,
    budget: Budget,
) -> Result<()> {
    encode::<Gray16, _>(out, pixels, width, height, effort, true, budget)
}

/// Decode a codestream written by [`encode_gray16`]: `(pixels, width, height)`.
pub fn decode_gray16(data: &[u8], budget: Budget) -> Result<(Vec<u16>, u32, u32)> {
    decode::<Gray16>(data, budget)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [`encode_rgb16`] into memory.
    fn rgb16(pixels: &[u16], w: u32, h: u32, linear: bool, budget: Budget) -> Result<Vec<u8>> {
        let mut out = std::io::Cursor::new(Vec::new());
        encode_rgb16(&mut out, pixels, w, h, FAST_EFFORT, linear, budget).map(|()| out.into_inner())
    }

    #[test]
    fn round_trips_exactly() {
        let (width, height) = (67u32, 31u32);
        // Full 16-bit range, including values an 8-bit path would lose.
        let pixels: Vec<u16> =
            (0..width * height * 3).map(|i| i.wrapping_mul(2_654_435_761) as u16).collect();
        for linear in [false, true] {
            let encoded = rgb16(&pixels, width, height, linear, Budget::unlimited()).unwrap();
            let (back, w, h) = decode_rgb16(&encoded, Budget::unlimited()).unwrap();
            assert_eq!((w, h), (width, height));
            assert!(back == pixels, "lossless round trip changed samples (linear: {linear})");
        }
        // 16-bit gray keeps every bit, the high byte included.
        let gray16: Vec<u16> =
            (0..width * height).map(|i| i.wrapping_mul(2_654_435_761) as u16).collect();
        let mut out = std::io::Cursor::new(Vec::new());
        encode_gray16(&mut out, &gray16, width, height, FAST_EFFORT, Budget::unlimited()).unwrap();
        let encoded = out.into_inner();
        assert_eq!(decode_gray16(&encoded, Budget::unlimited()).unwrap(), (gray16, width, height));
        // A gray stream does not decode as RGB.
        assert!(decode_rgb16(&encoded, Budget::unlimited()).is_err());
    }

    /// A codestream cut short fails as damaged: libjxl's error comes back
    /// as the cause the C API's status gives.
    #[test]
    fn a_codestream_cut_short_is_damaged() {
        let (width, height) = (67u32, 31u32);
        let pixels: Vec<u16> =
            (0..width * height * 3).map(|i| i.wrapping_mul(2_654_435_761) as u16).collect();
        let encoded = rgb16(&pixels, width, height, false, Budget::unlimited()).unwrap();
        let cut = &encoded[..encoded.len() / 2];
        let decoded = decode_rgb16(cut, Budget::unlimited());
        assert!(
            matches!(decoded, Err(DngError::JxlFailed { what: "decode", cause: Cause::Damaged })),
            "{:?}",
            decoded.err()
        );
    }

    /// A thread that encoded and decoded exits, and threads start after it:
    /// on Windows a per-thread pool joined at thread exit deadlocks under the
    /// loader lock, and no thread can start or end after. Broken, this test
    /// hangs rather than fails.
    #[test]
    fn a_thread_that_used_the_codec_exits() {
        let (done, finished) = std::sync::mpsc::channel();
        // Started first, so the watch needs no thread made after the codec ran.
        std::thread::spawn(move || {
            let coder = std::thread::spawn(|| {
                let pixels: Vec<u16> = (0..64 * 48 * 3).map(|i| (i * 977) as u16).collect();
                let encoded = rgb16(&pixels, 64, 48, false, Budget::unlimited()).unwrap();
                assert_eq!(decode_rgb16(&encoded, Budget::unlimited()).unwrap().0, pixels);
            });
            coder.join().unwrap();
            std::thread::spawn(|| ()).join().unwrap();
            let _ = done.send(());
        });
        finished
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("a thread that used JPEG XL never exited, or no thread could start after it");
    }

    /// A canceled decode fails as canceled, through the runner: libjxl
    /// turns the runner's error into JXL_DEC_ERROR, which is its code,
    /// not its documented API, so this pins it. Uncanceled, the same
    /// stream decodes.
    #[test]
    fn a_canceled_decode_stops_through_its_runner() {
        use std::sync::atomic::AtomicBool;
        let (width, height) = (512u32, 384u32);
        let pixels: Vec<u16> = (0..width * height * 3).map(|i| (i * 977) as u16).collect();
        let encoded = rgb16(&pixels, width, height, false, Budget::unlimited()).unwrap();
        let canceled = AtomicBool::new(true);
        let budget = Budget { cancel: &canceled, threads: 2, watch: None };
        assert!(matches!(decode_rgb16(&encoded, budget), Err(DngError::Canceled)));
        assert!(matches!(rgb16(&pixels, width, height, false, budget), Err(DngError::Canceled)));
        let going = AtomicBool::new(false);
        let budget = Budget { cancel: &going, threads: 2, watch: None };
        assert_eq!(decode_rgb16(&encoded, budget).unwrap().0, pixels);
    }

    /// A picture of many groups streams: libjxl hands it over a part at a
    /// time and seeks back to fill in the table of contents, and it still
    /// decodes exactly. A writer that fails stops the encode with its
    /// error.
    #[test]
    fn a_large_picture_streams_through_its_writer() {
        let (width, height) = (1100u32, 700u32);
        let pixels: Vec<u16> = (0..width * height * 3)
            .map(|i| {
                ((i / 3 % width) * 40 + (i / 3 / width) * 20 + (i.wrapping_mul(7919) >> 5) % 64)
                    as u16
            })
            .collect();
        let encoded = rgb16(&pixels, width, height, false, Budget::unlimited()).unwrap();
        assert_eq!(
            decode_rgb16(&encoded, Budget::unlimited()).unwrap(),
            (pixels.clone(), width, height)
        );

        struct Full(usize);
        impl Write for Full {
            fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
                if self.0 < data.len() {
                    return Err(std::io::Error::other("full"));
                }
                self.0 -= data.len();
                Ok(data.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl Seek for Full {
            fn seek(&mut self, _: SeekFrom) -> std::io::Result<u64> {
                Ok(0)
            }
        }
        let failed = encode_rgb16(
            &mut Full(1000),
            &pixels,
            width,
            height,
            FAST_EFFORT,
            false,
            Budget::unlimited(),
        );
        assert!(matches!(failed, Err(DngError::JxlWrite(e)) if e.to_string() == "full"));
    }

    #[test]
    fn rejects_bad_input() {
        assert!(matches!(
            rgb16(&[0; 5], 2, 2, false, Budget::unlimited()),
            Err(DngError::PixelCountMismatch { .. })
        ));
        assert!(matches!(
            decode_rgb16(b"not a jxl", Budget::unlimited()),
            Err(DngError::JxlFailed { cause: Cause::Damaged, .. })
        ));
    }
}
