//! The stackers' GPU memory estimates (`gpu_bytes`) against what Metal
//! reports allocated at the stack's peak and on completion; elsewhere the
//! test skips.
//!
//! A binary of its own, with one test: every wgpu device on a Mac shares one
//! Metal device, so a test beside it would count too.

use deptheos_core::depth::{DepthConfig, DepthStacker};
use deptheos_core::pyramid::{PyramidConfig, PyramidStacker};
use deptheos_core::upload::StagedUpload;
use deptheos_core::wavelet::{WaveletConfig, WaveletStacker};
use deptheos_core::{FrameDepth, StackInfo};
use deptheos_gpu::{GpuContext, StagedFrames};
use image::{DynamicImage, RgbImage};

const WIDTH: u32 = 3000;
const HEIGHT: u32 = 2000;
const FRAMES: u32 = 3;

/// Frame `i` at `depth`: 8-bit RGB, or the same values widened to 16.
fn frame(i: u32, depth: FrameDepth) -> DynamicImage {
    let rgb8 = DynamicImage::ImageRgb8(RgbImage::from_fn(WIDTH, HEIGHT, |x, y| {
        let v = ((x / (8 + i) + y / 8) % 2 * 200) as u8;
        image::Rgb([v, v / 2, 255 - v])
    }));
    match depth {
        FrameDepth::Eight => rgb8,
        FrameDepth::Sixteen => DynamicImage::ImageRgb16(rgb8.to_rgb16()),
    }
}

/// `image` staged as the prefetch threads stage it: the buffer is the
/// session's to count, not the stacker's.
fn staged(ctx: &GpuContext, image: &DynamicImage) -> StagedUpload {
    StagedUpload::with_pool(ctx, image, None).unwrap()
}

/// The stacker's own upload buffer, which `gpu_bytes` leaves to the
/// caller: none where staged frames are read in place.
fn upload_buffer(ctx: &GpuContext, depth: FrameDepth) -> u64 {
    match ctx.staged_frames() {
        StagedFrames::Copied => depth.rgb_bytes(u64::from(WIDTH) * u64::from(HEIGHT)),
        StagedFrames::InPlace => 0,
    }
}

/// Bytes allocated now, once the device has freed what was dropped.
fn allocated(ctx: &GpuContext) -> u64 {
    let _ = ctx.device.poll(wgpu::PollType::wait_indefinitely());
    ctx.allocated_bytes().expect("Metal reports its allocations")
}

/// `estimate` covers `measured`, padding and all, and is not far above it,
/// so a stack that would fit is not refused.
fn assert_covers(what: &str, estimate: u64, measured: u64) {
    let ratio = estimate as f64 / measured as f64;
    eprintln!(
        "{what}: estimate {} MB, measured {} MB ({ratio:.3})",
        estimate >> 20,
        measured >> 20
    );
    assert!((1.0..1.15).contains(&ratio), "{what}: estimate/measured = {ratio:.3}");
}

/// What completing holds (`finished` plus the readbacks: rgba32f result,
/// r32float map and its 16-bit copy) is under the stacking estimate, the
/// only phase the session counts.
fn assert_completion_fits(what: &str, estimate: u64, finished: u64) {
    let readbacks = u64::from(WIDTH) * u64::from(HEIGHT) * (16 + 4 + 2);
    let completing = finished + readbacks;
    eprintln!("{what}: completing {} MB, estimate {} MB", completing >> 20, estimate >> 20);
    assert!(completing < estimate, "{what}: completing holds more than stacking");
}

/// What a stacker allocates past what was allocated before it (`base`):
/// the most at any point noted (`peak`).
struct Measured {
    base: u64,
    peak: u64,
}

impl Measured {
    fn new(ctx: &GpuContext) -> Self {
        Self { base: allocated(ctx), peak: 0 }
    }

    /// Note what is allocated now.
    fn note(&mut self, ctx: &GpuContext) {
        self.peak = self.peak.max(allocated(ctx) - self.base);
    }

    /// What is allocated now.
    fn now(&self, ctx: &GpuContext) -> u64 {
        allocated(ctx) - self.base
    }
}

/// The three stackers, as the test drives them.
trait Stack {
    /// Whether frames are read again in a blend pass.
    const BLENDS: bool;
    fn accumulate(&mut self, frame: &StagedUpload);
    fn end_pass_one(&mut self);
    fn blend_frame(&mut self, frame: &StagedUpload, index: u32);
    fn finalize(&mut self);
}

impl Stack for PyramidStacker {
    const BLENDS: bool = false;
    fn accumulate(&mut self, frame: &StagedUpload) {
        PyramidStacker::accumulate(self, frame, None).unwrap();
    }
    fn end_pass_one(&mut self) {
        unreachable!("the pyramid has no blend pass")
    }
    fn blend_frame(&mut self, _: &StagedUpload, _: u32) {
        unreachable!("the pyramid has no blend pass")
    }
    fn finalize(&mut self) {
        PyramidStacker::finalize(self).unwrap();
    }
}

/// A stacker with a blend pass.
macro_rules! blending_stack {
    ($stacker:ty) => {
        impl Stack for $stacker {
            const BLENDS: bool = true;
            fn accumulate(&mut self, frame: &StagedUpload) {
                <$stacker>::accumulate(self, frame, None).unwrap();
            }
            fn end_pass_one(&mut self) {
                <$stacker>::end_pass_one(self).unwrap();
            }
            fn blend_frame(&mut self, frame: &StagedUpload, index: u32) {
                <$stacker>::blend_frame(self, frame, index, None).unwrap();
            }
            fn finalize(&mut self) {
                <$stacker>::finalize(self).unwrap();
            }
        }
    };
}
blending_stack!(DepthStacker);
blending_stack!(WaveletStacker);

/// Stack `frames`, noting allocations after each step, and check them
/// against `estimate`.
fn stack_and_measure<S: Stack>(
    ctx: &GpuContext,
    what: &str,
    mut stacker: S,
    frames: &[DynamicImage],
    estimate: u64,
) {
    let mut m = Measured::new(ctx);
    for f in frames {
        stacker.accumulate(&staged(ctx, f));
        m.note(ctx);
    }
    if S::BLENDS {
        stacker.end_pass_one();
        m.note(ctx);
        for (i, f) in frames.iter().enumerate() {
            stacker.blend_frame(&staged(ctx, f), i as u32);
            m.note(ctx);
        }
    }
    stacker.finalize();
    m.note(ctx);
    assert_covers(what, estimate, m.peak);
    assert_completion_fits(what, estimate, m.now(ctx));
}

#[test]
fn estimates_match_the_measured_peak() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    if ctx.allocated_bytes().is_none() {
        eprintln!("skipping: this backend does not report its allocations");
        return;
    }
    let info = StackInfo { total_frames: FRAMES, is_linear: false };
    for depth in [FrameDepth::Eight, FrameDepth::Sixteen] {
        let frames: Vec<_> = (0..FRAMES).map(|i| frame(i, depth)).collect();
        let upload = upload_buffer(&ctx, depth);

        // Pyramid: everything is allocated up front, and finalize reuses it.
        let stacker = PyramidStacker::new(ctx.clone(), PyramidConfig::default(), info);
        let estimate = PyramidStacker::gpu_bytes(&ctx, WIDTH, HEIGHT, depth) + upload;
        stack_and_measure(&ctx, &format!("pyramid, {depth:?}-bit"), stacker, &frames, estimate);

        // Depth: the peak is the blend pass.
        let stacker = DepthStacker::new(ctx.clone(), DepthConfig::default(), info);
        let estimate = DepthStacker::gpu_bytes(&ctx, WIDTH, HEIGHT, depth) + upload;
        stack_and_measure(&ctx, &format!("depth, {depth:?}-bit"), stacker, &frames, estimate);

        // Wavelet: the peak is the accumulate phase, with the runner-up
        // the consistency filter reads or without.
        for consistency in [0, 2] {
            let config = WaveletConfig { consistency, ..WaveletConfig::default() };
            let stacker = WaveletStacker::new(ctx.clone(), config, info);
            let estimate = WaveletStacker::gpu_bytes(&ctx, &config, WIDTH, HEIGHT, depth) + upload;
            let what = format!("wavelet, consistency {consistency}, {depth:?}-bit");
            stack_and_measure(&ctx, &what, stacker, &frames, estimate);
        }
    }
}
