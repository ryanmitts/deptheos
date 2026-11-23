//! One type over the stackers, which the core crate keeps independent (no
//! shared trait), matched by hand. Pyramid is single-pass; depth and
//! wavelet add a blend pass reading the frames again by accumulation index.

use std::sync::Arc;

use deptheos_core::depth::{DepthConfig, DepthStacker};

use crate::words::{Words, say};
use deptheos_aligner::AlignMode;
use deptheos_core::pyramid::{PyramidConfig, PyramidStacker};
use deptheos_core::wavelet::{WaveletConfig, WaveletStacker};
use deptheos_core::{FrameAlignment, FrameDepth, FrameInput, Result, StackInfo};
use deptheos_gpu::GpuContext;

/// A stack setting with a one-sentence help line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SettingHelp {
    Smoothness,
    Smoothing,
    Denoise,
    Radius,
    BlendRange,
    Aligner,
    Crop,
    Decoder,
}

impl SettingHelp {
    pub const ALL: [Self; 8] = [
        Self::Smoothness,
        Self::Smoothing,
        Self::Denoise,
        Self::Radius,
        Self::BlendRange,
        Self::Aligner,
        Self::Crop,
        Self::Decoder,
    ];

    /// What moving it does, one sentence.
    pub fn help(self, words: &Words) -> String {
        match self {
            Self::Smoothness => say!(words, "setting-smoothness-help"),
            Self::Smoothing => say!(words, "setting-smoothing-help"),
            Self::Denoise => say!(words, "setting-denoise-help"),
            Self::Radius => say!(words, "setting-radius-help"),
            Self::BlendRange => say!(words, "setting-blend-range-help"),
            Self::Aligner => say!(words, "setting-aligner-help"),
            Self::Crop => say!(words, "setting-crop-help"),
            Self::Decoder => say!(words, "setting-decoder-help"),
        }
    }
}

/// Which algorithm stacks, without its settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum StackerKind {
    #[default]
    Pyramid,
    Depth,
    Wavelet,
}

impl StackerKind {
    pub const ALL: [Self; 3] = [Self::Pyramid, Self::Depth, Self::Wavelet];

    /// What the command line, the preferences and benchmarks call it.
    pub fn key(self) -> &'static str {
        match self {
            Self::Pyramid => "pyramid",
            Self::Depth => "depth",
            Self::Wavelet => "wavelet",
        }
    }

    /// What the method's choice calls it.
    pub fn label(self, words: &Words) -> String {
        match self {
            Self::Pyramid => say!(words, "method-pyramid"),
            Self::Depth => say!(words, "method-depth"),
            Self::Wavelet => say!(words, "method-wavelet"),
        }
    }

    /// Its English name, for file metadata and benchmark records.
    pub fn english_name(self) -> &'static str {
        match self {
            Self::Pyramid => "Pyramid",
            Self::Depth => "Depth",
            Self::Wavelet => "Wavelet",
        }
    }

    /// When to use it, in a line or two, under the method's choice.
    pub fn description(self, words: &Words) -> String {
        match self {
            Self::Pyramid => say!(words, "method-pyramid-description"),
            Self::Depth => say!(words, "method-depth-description"),
            Self::Wavelet => say!(words, "method-wavelet-description"),
        }
    }

    /// The kind `key` names, in any case.
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.key().eq_ignore_ascii_case(key))
    }

    /// The algorithm with its default settings.
    pub fn default_config(self) -> StackerConfig {
        match self {
            Self::Pyramid => StackerConfig::Pyramid(PyramidConfig::default()),
            Self::Depth => StackerConfig::Depth(DepthConfig::default()),
            Self::Wavelet => StackerConfig::Wavelet(WaveletConfig::default()),
        }
    }
}

/// What the alignment choice calls `mode`: "ECC (affine)".
pub fn align_label(mode: AlignMode, words: &Words) -> String {
    match mode {
        AlignMode::Ecc => say!(words, "align-ecc"),
        AlignMode::Grid => say!(words, "align-grid"),
    }
}

/// Algorithm choice plus its parameters.
#[derive(Clone, Copy, Debug)]
pub enum StackerConfig {
    Pyramid(PyramidConfig),
    Depth(DepthConfig),
    Wavelet(WaveletConfig),
}

impl StackerConfig {
    pub fn kind(&self) -> StackerKind {
        match self {
            Self::Pyramid(_) => StackerKind::Pyramid,
            Self::Depth(_) => StackerKind::Depth,
            Self::Wavelet(_) => StackerKind::Wavelet,
        }
    }

    /// Whether this algorithm re-reads every frame in a second pass.
    pub fn needs_blend(&self) -> bool {
        !matches!(self, Self::Pyramid(_))
    }

    /// The settings as the sliders read them: "Depth · R8 S2.5 B3".
    pub fn summary(&self, words: &Words) -> String {
        match self {
            Self::Pyramid(c) => {
                say!(words, "method-pyramid-summary", smoothness = short(c.smoothness))
            }
            Self::Depth(c) => say!(
                words,
                "method-depth-summary",
                radius = short(c.radius),
                smoothing = short(c.smoothing),
                blend = short(c.blend_range)
            ),
            Self::Wavelet(c) if c.consistency > 0 => say!(
                words,
                "method-wavelet-consistent-summary",
                denoise = short(c.denoise),
                consistency = c.consistency.to_string()
            ),
            Self::Wavelet(c) => say!(words, "method-wavelet-summary", denoise = short(c.denoise)),
        }
    }
}

/// A slider value to one decimal, whole numbers without a point.
pub(crate) fn short(value: f32) -> String {
    let rounded = (value * 10.0).round() / 10.0;
    if rounded.fract() == 0.0 { format!("{rounded:.0}") } else { format!("{rounded:.1}") }
}

// One instance per session, so the size spread between variants is harmless.
#[allow(clippy::large_enum_variant)]
pub(crate) enum Stacker {
    Pyramid(PyramidStacker),
    Depth(DepthStacker),
    Wavelet(WaveletStacker),
}

impl Stacker {
    /// A stacker for `total_frames` frames, in linear camera RGB when
    /// `is_linear`.
    pub(crate) fn new(
        ctx: Arc<GpuContext>,
        config: StackerConfig,
        total_frames: u32,
        is_linear: bool,
    ) -> Self {
        let info = StackInfo { total_frames, is_linear };
        match config {
            StackerConfig::Pyramid(c) => Self::Pyramid(PyramidStacker::new(ctx, c, info)),
            StackerConfig::Depth(c) => Self::Depth(DepthStacker::new(ctx, c, info)),
            StackerConfig::Wavelet(c) => Self::Wavelet(WaveletStacker::new(ctx, c, info)),
        }
    }

    /// The peak GPU memory of a stack of `config` (`PyramidStacker::gpu_bytes`).
    pub(crate) fn gpu_bytes(
        ctx: &GpuContext,
        config: StackerConfig,
        width: u32,
        height: u32,
        depth: FrameDepth,
    ) -> u64 {
        match config {
            StackerConfig::Pyramid(_) => PyramidStacker::gpu_bytes(ctx, width, height, depth),
            StackerConfig::Depth(_) => DepthStacker::gpu_bytes(ctx, width, height, depth),
            StackerConfig::Wavelet(c) => WaveletStacker::gpu_bytes(ctx, &c, width, height, depth),
        }
    }

    /// Size the stacker for frames of `depth` before the first frame
    /// arrives (see `PyramidStacker::allocate`).
    pub(crate) fn allocate(&mut self, width: u32, height: u32, depth: FrameDepth) -> Result<()> {
        match self {
            Self::Pyramid(s) => s.allocate(width, height, depth),
            Self::Depth(s) => s.allocate(width, height, depth),
            Self::Wavelet(s) => s.allocate(width, height, depth),
        }
    }

    pub(crate) fn accumulate<'a>(
        &mut self,
        frame: impl Into<FrameInput<'a>>,
        alignment: Option<&FrameAlignment>,
    ) -> Result<()> {
        match self {
            Self::Pyramid(s) => s.accumulate(frame, alignment),
            Self::Depth(s) => s.accumulate(frame, alignment),
            Self::Wavelet(s) => s.accumulate(frame, alignment),
        }
    }

    /// Leave the next frame out, keeping its index (and plane).
    pub(crate) fn skip_frame(&mut self) -> Result<()> {
        match self {
            Self::Pyramid(s) => s.skip_frame(),
            Self::Depth(s) => s.skip_frame(),
            Self::Wavelet(s) => s.skip_frame(),
        }
    }

    /// End the first pass: pyramid's final step, depth's and wavelet's
    /// preparation for their second.
    pub(crate) fn end_pass_one(&mut self) -> Result<()> {
        match self {
            Self::Pyramid(s) => s.finalize(),
            Self::Depth(s) => s.end_pass_one(),
            Self::Wavelet(s) => s.end_pass_one(),
        }
    }

    /// Feed a second-pass frame by its accumulation index, which depth's
    /// planes count in.
    pub(crate) fn blend_frame<'a>(
        &mut self,
        frame: impl Into<FrameInput<'a>>,
        frame_index: u32,
        alignment: Option<&FrameAlignment>,
    ) -> Result<()> {
        match self {
            Self::Pyramid(_) => Ok(()),
            Self::Depth(s) => s.blend_frame(frame, frame_index, alignment),
            Self::Wavelet(s) => s.blend_frame(frame, frame_index, alignment),
        }
    }

    /// Finish after the blend pass; a no-op for pyramid.
    pub(crate) fn finalize(&mut self) -> Result<()> {
        match self {
            Self::Pyramid(_) => Ok(()),
            Self::Depth(s) => s.finalize(),
            Self::Wavelet(s) => s.finalize(),
        }
    }

    /// The stack so far (pyramid only, `PyramidStacker::collapse`); the
    /// next frame overwrites the texture.
    pub(crate) fn collapse(&self) -> Result<&wgpu::Texture> {
        match self {
            Self::Pyramid(s) => s.collapse(),
            _ => Err(deptheos_core::CoreError::InvalidState(
                "only a pyramid stack has a picture partway through".into(),
            )),
        }
    }

    /// The finalized result texture (rgba32float), if finalization has run.
    pub(crate) fn result_texture(&self) -> Option<&wgpu::Texture> {
        match self {
            Self::Pyramid(s) => s.result_texture(),
            Self::Depth(s) => s.result_texture(),
            Self::Wavelet(s) => s.result_texture(),
        }
    }

    /// The frame map as it forms, 8-bit in `format`, at most `max_side`
    /// long, for every stacker ([`crate::engine::present::present_map`]
    /// colors it). `Err` before the first stacked frame.
    pub(crate) fn render_live(
        &self,
        max_side: u32,
        format: wgpu::TextureFormat,
    ) -> Result<wgpu::Texture> {
        match self {
            Self::Pyramid(s) => s.render_live(max_side, format),
            Self::Depth(s) => s.render_live(max_side, format),
            Self::Wavelet(s) => s.render_live(max_side, format),
        }
    }

    /// The finished frame map (r32float, full size, each pixel its frame's
    /// plane); `None` before finalization.
    pub(crate) fn frame_map(&self) -> Option<&wgpu::Texture> {
        match self {
            Self::Pyramid(s) => s.frame_map(),
            Self::Depth(s) => s.frame_map(),
            Self::Wavelet(s) => s.frame_map(),
        }
    }
}

#[cfg(test)]
mod summary_tests {
    use super::*;

    #[test]
    fn each_help_line_is_one_sentence() {
        let w = &Words::english(crate::Platform::Desktop);
        for setting in SettingHelp::ALL {
            let help = setting.help(w);
            assert!(help.ends_with('.'), "{setting:?}: {help}");
            assert_eq!(help.matches(". ").count(), 0, "{setting:?}: {help}");
        }
    }

    #[test]
    fn summaries_read_like_the_sliders() {
        let w = &Words::english(crate::Platform::Desktop);
        let pyramid = StackerConfig::Pyramid(PyramidConfig::default());
        assert_eq!(pyramid.summary(w), "Pyramid · S4");
        let depth =
            StackerConfig::Depth(DepthConfig { radius: 8.0, smoothing: 2.5, blend_range: 3.0 });
        assert_eq!(depth.summary(w), "Depth · R8 S2.5 B3");
        let wavelet = StackerConfig::Wavelet(WaveletConfig { denoise: 1.0, consistency: 2 });
        assert_eq!(wavelet.summary(w), "Wavelet · D1 C2");
    }
}
