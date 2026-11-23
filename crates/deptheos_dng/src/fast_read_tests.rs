//! The bridge's fast read paths (native/dng_fast_read.cpp and
//! dng_lossless_jpeg_fast.cpp) give the SDK's own pixels. The file tests
//! write a small linear DNG, read it both ways and compare; the stream
//! tests compare the lossless JPEG decoders on single streams. Which path a
//! read took comes from the test hook `dt_dng_last_read_path`.

use std::path::Path;

use super::*;
use crate::test_dng::{Layout, TestDng};

/// How stage 1 of a read was read (`stage1_read` in native/dng_bridge_internal.h).
#[derive(Debug, PartialEq, Eq)]
enum Stage1 {
    /// The SDK's own reader.
    Sdk,
    /// Uncompressed, copied on several threads.
    Parallel,
    /// Lossless JPEG, every tile by the bridge's decoder.
    FastDecode,
    /// Lossless JPEG, some tiles left to the SDK's decoder.
    FastDecodePartly,
}

/// How a read took its pixels.
#[derive(Debug, PartialEq, Eq)]
struct ReadPath {
    stage1: Stage1,
    /// The black level waited for the copy out.
    deferred_black_level: bool,
}

/// How this thread's last read of linear pixels took them.
fn last_read_path() -> ReadPath {
    let (mut stage1, mut deferred) = (-1, 0);
    // SAFETY: two out-parameters the call writes.
    unsafe { dt_dng_last_read_path(&mut stage1, &mut deferred) };
    let stage1 = match stage1 {
        0 => Stage1::Sdk,
        1 => Stage1::Parallel,
        2 => Stage1::FastDecode,
        3 => Stage1::FastDecodePartly,
        other => panic!("no read path recorded ({other})"),
    };
    ReadPath { stage1, deferred_black_level: deferred != 0 }
}

/// A read that returns the pixels beside the handle.
type Read = fn(&Path, FileCache, crate::Budget<'static>) -> Result<DngRead>;

/// A read's handle and pixels, none for a mosaic.
fn parts(read: DngRead) -> (DngImage, Vec<u16>) {
    match read {
        DngRead::Linear(image, pixels) => (image, pixels),
        DngRead::Mosaic(image) => (image, Vec::new()),
    }
}

/// Read `path` with the SDK's own path and with the fast ones, check they
/// agree, and return how the fast read took its pixels.
fn compare(path: &Path) -> ReadPath {
    compare_with(path, DngImage::read_with_pixels).expect("linear pixels")
}

fn compare_with(path: &Path, read: Read) -> Option<ReadPath> {
    // SAFETY: plain flag setters for this thread.
    unsafe { dt_dng_set_reference_reads(1) };
    let reference = read(path, FileCache::Use, crate::Budget::unlimited());
    unsafe { dt_dng_set_reference_reads(0) };
    let (reference, reference_pixels) = parts(reference.unwrap());
    // Around the OS file cache (Windows), the same pixels by the same path.
    let (bypassed, bypassed_pixels) =
        parts(read(path, FileCache::Bypass, crate::Budget::unlimited()).unwrap());
    let bypassed_path = (bypassed.channels() > 0).then(last_read_path);
    let (fast, fast_pixels) =
        parts(read(path, FileCache::Use, crate::Budget::unlimited()).unwrap());
    let fast_path = (fast.channels() > 0).then(last_read_path);
    assert_eq!(bypassed_path, fast_path);
    assert!(
        bypassed_pixels == fast_pixels,
        "{}: a read around the file cache differs",
        path.display()
    );
    // The metadata that renders and writes the pixels is the same too.
    assert_eq!(fast.render_params().unwrap(), reference.render_params().unwrap());
    assert_eq!(
        (fast.width(), fast.height(), fast.channels()),
        (reference.width(), reference.height(), reference.channels())
    );
    let first_difference = reference_pixels.iter().zip(&fast_pixels).position(|(a, b)| a != b);
    assert_eq!(first_difference, None, "{}: the fast read differs from the SDK's", path.display());
    assert_eq!(reference_pixels.len(), fast_pixels.len());

    // A mosaic is read as metadata only, with no read path.
    fast_path
}

fn frame(width: u32, height: u32, layout: Layout, big_endian: bool) -> TestDng {
    TestDng {
        cfa: false,
        width,
        height,
        big_endian,
        layout,
        black: [1000, 2048, 3000],
        white: [60000, 65535, 62000],
        active: [2, 3, height, width - 2],
        crop: [4, 5, width - 13, height - 9],
    }
}

/// `copied`: whether stage 1 should take the uncompressed fast copy.
fn check(frame: TestDng, copied: bool) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("frame.dng");
    frame.write(&path, &frame.pixels());
    let read = compare(&path);
    assert!(read.deferred_black_level, "black level not deferred: {read:?}");
    assert_eq!(read.stage1 == Stage1::Parallel, copied, "{read:?}");
}

#[test]
fn tiles_with_padded_edges() {
    check(frame(301, 203, Layout::Tiles { width: 64, length: 48 }, false), true);
}

#[test]
fn strips() {
    check(frame(301, 203, Layout::Strips { rows: 37 }, false), true);
}

/// One strip or tile for the whole frame, as Adobe DNG Converter writes:
/// read in runs of rows, straight into the image when the rows line up.
#[test]
fn one_strip_for_the_whole_frame() {
    check(frame(301, 203, Layout::Strips { rows: 203 }, false), true);
}

#[test]
fn one_padded_tile_for_the_whole_frame() {
    check(frame(301, 203, Layout::Tiles { width: 304, length: 208 }, false), true);
}

/// Big-endian samples need swapping: the SDK reads stage 1, and the black
/// level is still deferred.
#[test]
fn big_endian_reads_through_the_sdk() {
    check(frame(301, 203, Layout::Tiles { width: 64, length: 48 }, true), false);
}

/// 36 MB, so the copy out runs in several bands.
#[test]
fn a_large_frame() {
    check(frame(3001, 2003, Layout::Tiles { width: 256, length: 256 }, false), true);
}

/// Real files: set DEPTHEOS_DNG_COMPARE to paths separated by `;` and run
///   cargo test -p deptheos_dng --release -- --ignored real_files
#[test]
#[ignore]
fn real_files() {
    let paths = std::env::var("DEPTHEOS_DNG_COMPARE").expect("DEPTHEOS_DNG_COMPARE");
    for path in paths.split(';').filter(|p| !p.is_empty()) {
        let path = Path::new(path);
        let read = compare_with(path, DngImage::read_with_pixels);
        println!(
            "{}: {}",
            path.display(),
            read.map_or("mosaic, metadata only".into(), |r| format!("{r:?}"))
        );
    }
}

/// A CFA file reads as metadata only, on the fast path as on the SDK's.
#[test]
fn cfa() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cfa.dng");
    let dng = TestDng { cfa: true, ..frame(301, 203, Layout::Strips { rows: 37 }, false) };
    dng.write(&path, &dng.pixels());
    assert!(compare_with(&path, DngImage::read_with_pixels).is_none());
}

/// A frame written by the bridge's writer: lossless JPEG tiles, as the
/// converter writes by default.
fn write_lossless_jpeg(dir: &Path, width: u32, height: u32) -> std::path::PathBuf {
    let source = dir.join("source.dng");
    let f = frame(width, height, Layout::Tiles { width: 256, length: 256 }, false);
    f.write(&source, &f.pixels());
    let (image, pixels) = parts(
        DngImage::read_with_pixels(&source, FileCache::Use, crate::Budget::unlimited()).unwrap(),
    );
    let path = dir.join("compressed.dng");
    image.write(&path, &pixels, image.width(), image.height(), image.channels()).unwrap();
    path
}

/// Decoded by the bridge's decoder inside the SDK's tile loop, on several
/// threads. (The writer makes 144 x 144 tiles of this frame.)
#[test]
fn lossless_jpeg_tiles() {
    let dir = tempfile::tempdir().unwrap();
    let read = compare(&write_lossless_jpeg(dir.path(), 1501, 1003));
    assert_eq!(read.stage1, Stage1::FastDecode);
}

/// The writer writes the path it is given and nothing beside it: moving a
/// save into place is the caller's (`io::write_replacing`).
#[test]
fn write_makes_only_its_file() {
    let dir = tempfile::tempdir().unwrap();
    write_lossless_jpeg(dir.path(), 120, 90);
    let mut names: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, ["compressed.dng", "source.dng"]);
}

/// One tile: the SDK's loop reads it on the calling thread from the file's
/// stream, not a memory stream of the tile.
#[test]
fn lossless_jpeg_one_tile() {
    let dir = tempfile::tempdir().unwrap();
    let read = compare(&write_lossless_jpeg(dir.path(), 120, 90));
    assert_eq!(read.stage1, Stage1::FastDecode);
}

/// A tile the bridge's decoder declines (repeated component ids, which the
/// SDK reads with one table for all) is the SDK's, and the rest still the
/// bridge's.
#[test]
fn lossless_jpeg_some_tiles_declined() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_lossless_jpeg(dir.path(), 1501, 1003);
    let mut bytes = std::fs::read(&path).unwrap();
    let sof = bytes.windows(2).position(|w| w == [0xFF, 0xC3]).expect("a lossless JPEG tile");
    let components = bytes[sof + 9] as usize;
    let first = bytes[sof + 10];
    for c in 1..components {
        bytes[sof + 10 + 3 * c] = first;
    }
    let sos = sof + bytes[sof..].windows(2).position(|w| w == [0xFF, 0xDA]).unwrap();
    for c in 1..components {
        bytes[sos + 5 + 2 * c] = first;
    }
    std::fs::write(&path, &bytes).unwrap();
    assert_eq!(compare(&path).stage1, Stage1::FastDecodePartly);
}

// The lossless JPEG decoder (native/dng_lossless_jpeg_fast.cpp),
// stream by stream against the SDK's.

/// A small deterministic generator.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 =
            self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) as u32
    }
}

fn encode(samples: &[u16], rows: u32, cols: u32, channels: u32, bits: u32) -> Vec<u8> {
    let mut out = vec![0u8; samples.len() * 3 + 4096];
    // SAFETY: `samples` holds rows * cols * channels values; `out` is
    // writable for its length.
    let n = unsafe {
        dt_dng_encode_lossless_jpeg(
            samples.as_ptr(),
            rows,
            cols,
            channels,
            bits,
            out.as_mut_ptr(),
            out.len(),
        )
    };
    assert!(n > 0, "the SDK encoded nothing");
    out.truncate(n);
    out
}

/// 1: both decoders agree; 0: the bridge's declined; -1: it decoded a
/// stream the SDK rejects; -2: they differ.
fn compare_stream(stream: &[u8], samples: usize) -> i32 {
    // SAFETY: a readable buffer of its length.
    unsafe { dt_dng_compare_lossless_jpeg(stream.as_ptr(), stream.len(), samples) }
}

/// Where the scan header's predictor (Ss) and point transform (Al) bytes are.
fn scan_parameters(stream: &[u8]) -> (usize, usize) {
    let sos = stream.windows(2).position(|w| w == [0xFF, 0xDA]).expect("an SOS marker");
    let components = stream[sos + 4] as usize;
    let ss = sos + 5 + 2 * components;
    (ss, ss + 2)
}

/// Smooth gradients with noise and, with `spikes`, rare jumps of every size.
fn image(rows: u32, cols: u32, channels: u32, bits: u32, seed: u64, spikes: bool) -> Vec<u16> {
    let mut rng = Lcg(seed);
    let max = (1u32 << bits) - 1;
    let mut v = Vec::new();
    for y in 0..rows {
        for x in 0..cols {
            for c in 0..channels {
                let base = (x * 37 + y * 11 + c * 1000) % (max + 1);
                let mut s = (base + rng.next() % 8).min(max);
                if spikes && rng.next().is_multiple_of(50) {
                    s = rng.next() % (max + 1);
                }
                v.push(s as u16);
            }
        }
    }
    v
}

#[test]
fn lossless_jpeg_matches_the_sdk_across_shapes() {
    let mut cases = 0;
    for channels in 1..=4 {
        for bits in [8, 12, 14, 16] {
            for (rows, cols) in [(1, 1), (1, 7), (7, 1), (16, 16), (37, 53), (144, 160)] {
                for spikes in [false, true] {
                    let seed = (channels * 100 + bits + rows * cols) as u64;
                    let samples = image(rows, cols, channels, bits, seed, spikes);
                    let stream = encode(&samples, rows, cols, channels, bits);
                    let expected = (rows * cols * channels) as usize;
                    assert_eq!(
                        compare_stream(&stream, expected),
                        1,
                        "{channels} ch, {bits} bits, {rows}x{cols}"
                    );
                    cases += 1;
                }
            }
        }
    }
    assert_eq!(cases, 4 * 4 * 6 * 2);
}

/// Every predictor and some point transforms, by rewriting the scan header
/// of a stream the SDK wrote: the same bits, read with another rule.
#[test]
fn lossless_jpeg_every_predictor_and_point_transform() {
    for channels in [1, 2, 3] {
        let (rows, cols) = (23, 41);
        let samples = image(rows, cols, channels, 16, 7 + channels as u64, true);
        let stream = encode(&samples, rows, cols, channels, 16);
        let (ss, al) = scan_parameters(&stream);
        for predictor in 0..=7u8 {
            for transform in [0u8, 1, 3, 15] {
                let mut s = stream.clone();
                s[ss] = predictor;
                s[al] = (s[al] & 0xF0) | transform;
                assert_eq!(
                    compare_stream(&s, (rows * cols * channels) as usize),
                    1,
                    "{channels} ch, predictor {predictor}, point transform {transform}"
                );
            }
        }
    }
}

/// The longest Huffman code in a stream's tables, and whether any table
/// holds the 16-bit difference symbol.
fn table_extremes(stream: &[u8]) -> (usize, bool) {
    let (mut longest, mut sixteen) = (0, false);
    let mut i = 2;
    while i + 4 <= stream.len() && stream[i] == 0xFF && stream[i + 1] != 0xDA {
        let len = u16::from_be_bytes([stream[i + 2], stream[i + 3]]) as usize;
        if stream[i + 1] == 0xC4 {
            let mut q = i + 4;
            while q < i + 2 + len {
                let counts = &stream[q + 1..q + 17];
                let n: usize = counts.iter().map(|&c| c as usize).sum();
                if let Some(l) = counts.iter().rposition(|&c| c > 0) {
                    longest = longest.max(l + 1);
                }
                sixteen |= stream[q + 17..q + 17 + n].contains(&16);
                q += 17 + n;
            }
        }
        i += 2 + len;
    }
    (longest, sixteen)
}

/// Differences of 16 bits (the -32768 case, from samples alternating 0 and
/// 0x8000) and codes longer than the decoder's 11-bit lookup (nearly all equal
/// samples with rare jumps of every size).
#[test]
fn lossless_jpeg_extreme_differences_and_long_codes() {
    let (rows, cols) = (64u32, 64u32);
    let extremes: Vec<u16> =
        (0..rows * cols * 3).map(|i| if (i / 3) % 2 == 0 { 0 } else { 0x8000 }).collect();
    let stream = encode(&extremes, rows, cols, 3, 16);
    assert!(table_extremes(&stream).1, "no 16-bit differences");
    assert_eq!(compare_stream(&stream, extremes.len()), 1);

    // Differences of every size k (0 to 16 bits), 2^(14 - k) of each (at
    // least one), shuffled, so codes run past 11 bits. (Any steeper and the
    // SDK's encoder falls back to a flat table.)
    let mut differences: Vec<u16> = (0..=16u32)
        .flat_map(|k| {
            let d = if k == 0 { 0 } else { (1u32 << (k - 1)) as u16 };
            std::iter::repeat_n(d, 1usize << 14u32.saturating_sub(k))
        })
        .collect();
    let mut rng = Lcg(99);
    for i in (1..differences.len()).rev() {
        differences.swap(i, rng.next() as usize % (i + 1));
    }
    let cols = 256u32;
    let rows = (differences.len() as u32) / cols;
    let mut skewed = vec![0u16; (rows * cols) as usize];
    for (i, d) in differences.iter().take(skewed.len()).enumerate() {
        // Each sample is its left (or, in the first column, upper) neighbor
        // plus the difference.
        let x = i % cols as usize;
        let prev = if x > 0 {
            skewed[i - 1]
        } else if i >= cols as usize {
            skewed[i - cols as usize]
        } else {
            0
        };
        skewed[i] = prev.wrapping_add(*d);
    }
    let stream = encode(&skewed, rows, cols, 1, 16);
    let (longest, _) = table_extremes(&stream);
    assert!(longest > 11, "longest code {longest} bits");
    assert_eq!(compare_stream(&stream, skewed.len()), 1);
}

/// Damaged streams: the bridge's decoder declines them or agrees with the
/// SDK, and never decodes one the SDK rejects.
#[test]
fn lossless_jpeg_damaged_streams() {
    let (rows, cols, channels) = (19u32, 29u32, 3u32);
    let samples = image(rows, cols, channels, 16, 5, true);
    let stream = encode(&samples, rows, cols, channels, 16);
    let expected = samples.len();
    let mut rng = Lcg(2026);
    let mut outcomes = [0u32; 2];
    for i in 0..3000 {
        let mut s = stream.clone();
        match i % 3 {
            0 => {
                let at = rng.next() as usize % s.len();
                s[at] ^= 1 << (rng.next() % 8);
            }
            1 => {
                let at = rng.next() as usize % s.len();
                s[at] = rng.next() as u8;
            }
            _ => {
                let keep = rng.next() as usize % s.len();
                s.truncate(keep);
            }
        }
        let r = compare_stream(&s, expected);
        assert!(r == 0 || r == 1, "case {i}: {r}");
        outcomes[r as usize] += 1;
    }
    // Both outcomes happen: the test reaches the decoder, not just the header checks.
    assert!(outcomes[0] > 0 && outcomes[1] > 0, "{outcomes:?}");
}

/// Streams sharing some tables, decoded in a shuffled order on one thread:
/// the decoder's per-thread table cache must never hand a stream a table
/// it replaced for another of the same stream's tables.
#[test]
fn lossless_jpeg_table_cache() {
    let (rows, cols, channels) = (12u32, 20u32, 3u32);
    let streams: Vec<Vec<u8>> = (0..12)
        .map(|i| {
            let samples = image(rows, cols, channels, 16, i, i % 2 == 0);
            encode(&samples, rows, cols, channels, 16)
        })
        .collect();
    let expected = (rows * cols * channels) as usize;
    let mut rng = Lcg(7);
    for _ in 0..2000 {
        let s = &streams[rng.next() as usize % streams.len()];
        assert_eq!(compare_stream(s, expected), 1);
    }
}

/// Header variants: those the SDK reads the same way decode alike; the
/// rest the bridge's decoder declines.
#[test]
fn lossless_jpeg_headers() {
    let (rows, cols, channels) = (9u32, 13u32, 3u32);
    let samples = image(rows, cols, channels, 16, 3, true);
    let stream = encode(&samples, rows, cols, channels, 16);
    let expected = samples.len();
    let sof = stream.windows(2).position(|w| w == [0xFF, 0xC3]).unwrap();
    let sos = stream.windows(2).position(|w| w == [0xFF, 0xDA]).unwrap();
    // The stream with `segment` inserted before the frame header.
    let with = |segment: &[u8]| -> Vec<u8> {
        let mut s = stream[..sof].to_vec();
        s.extend_from_slice(segment);
        s.extend_from_slice(&stream[sof..]);
        s
    };
    let alike: [(&str, Vec<u8>); 4] = [
        ("APP1", with(&[0xFF, 0xE1, 0, 4, 1, 2])),
        ("COM", with(&[0xFF, 0xFE, 0, 5, b'h', b'i', b'!'])),
        // Two fill bytes before the frame header's marker.
        ("fill bytes", [&stream[..sof], &[0xFF, 0xFF][..], &stream[sof..]].concat()),
        ("DRI 0", with(&[0xFF, 0xDD, 0, 4, 0, 0])),
    ];
    for (what, s) in &alike {
        assert_eq!(compare_stream(s, expected), 1, "{what}");
    }
    let mut repeated_ids = stream.clone();
    for c in 1..channels as usize {
        repeated_ids[sof + 10 + 3 * c] = repeated_ids[sof + 10];
        repeated_ids[sos + 5 + 2 * c] = repeated_ids[sof + 10];
    }
    let mut scan_order = stream.clone();
    scan_order.swap(sos + 5, sos + 7);
    scan_order.swap(sos + 6, sos + 8);
    let mut sof0 = stream.clone();
    sof0[sof + 1] = 0xC0;
    let mut no_eoi = stream.clone();
    no_eoi.truncate(stream.len() - 2);
    let declined: [(&str, Vec<u8>); 6] = [
        ("DRI 1", with(&[0xFF, 0xDD, 0, 4, 0, 1])),
        ("DQT", with(&[0xFF, 0xDB, 0, 3, 0])),
        ("repeated component ids", repeated_ids),
        ("scan order unlike the frame's", scan_order),
        ("SOF0", sof0),
        ("no EOI", no_eoi),
    ];
    for (what, s) in &declined {
        assert_eq!(compare_stream(s, expected), 0, "{what}");
    }
}
