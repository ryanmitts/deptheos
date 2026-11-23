//! The stackers' focus blurs: the IIR radius → σ mapping behind every focus
//! blur's width, the Gaussian's uniform, and the host side of the depth
//! stacker's focus metric (`shaders/focus.wgsl`).

use deptheos_gpu::GpuContext;
use deptheos_gpu::gpu_utils::{
    create_bind_group, create_compute_pipeline, create_tiled_compute_pipeline, dispatch_compute,
    tex,
};
use wgpu::{ComputePipeline, Texture};

/// Gain of the bandpass metric. The same for every frame, it only sets where
/// the metric saturates at 1: at very hard edges.
pub(crate) const BANDPASS_GAIN: f32 = 1.1;

// --- Exponential IIR blur mapping -------------------------------------------
//
// Blur radii are defined through the two-sided first-order IIR
// `r[n] = k1·r[n-1] + k2·(in[n-1] + in[n])`, `k1 + 2·k2 = 1`, that the
// depth-map smoothing runs. At `k1 → 0` it floors at `[¼, ½, ¼]`
// (σ = 1/√2). `k1` is quantized to 12 bits so every GPU blurs alike.

/// `C` of [`iir_k1`].
const IIR_K1_BASE: f32 = 0.045_999_825;
/// [`iir_k1`] quantizes `k1` to this many steps (12 fractional bits).
const IIR_QUANTIZATION: f32 = 4096.0;

/// Feedback coefficient for blur radius `r`: `k1 = C^(1.5/r)`, as an even
/// 12-bit `w1` (at least 2) so `k2 = (4096 − w1)/2` stays integral.
pub(crate) fn iir_k1(r: f32) -> f32 {
    let k1 = if r > 0.0 { IIR_K1_BASE.powf(1.0 / (2.0 * (r / 3.0))) } else { 0.0 };
    let w1 = (k1 * IIR_QUANTIZATION) as u32;
    let w1 = (w1 & !1).max(2);
    w1 as f32 / IIR_QUANTIZATION
}

/// Standard deviation of the two-sided exponential kernel with feedback `k1`:
/// `σ = (1 + k1) / ((1 − k1)·√2)`.
pub(crate) fn iir_sigma(k1: f32) -> f32 {
    (1.0 + k1) / ((1.0 - k1) * std::f32::consts::SQRT_2)
}

/// [`iir_sigma`] for blur radius `r`: the Gaussian width the focus blurs use
/// (with lighter tails than the exponential at wide σ).
pub(crate) fn iir_blur_sigma(r: f32) -> f32 {
    iir_sigma(iir_k1(r))
}

/// Gaussian kernels cover ±this many σ.
pub(crate) const BLUR_HALF_WIDTH_SIGMAS: f32 = 3.0;

/// Half-width of a Gaussian kernel, at least one tap.
pub(crate) fn blur_half_width(sigma: f32) -> u32 {
    ((sigma * BLUR_HALF_WIDTH_SIGMAS).ceil() as u32).max(1)
}

/// The widest half-width `focus.wgsl`'s 512-weight table (`MAX_BLUR_TAPS`)
/// holds.
pub(crate) const MAX_BLUR_HALF_WIDTH: u32 = 255;

/// The blur uniform for `sigma`. Panics past [`MAX_BLUR_HALF_WIDTH`]: that
/// would read unwritten weights, a bug in the radius range.
pub(crate) fn blur_params(sigma: f32) -> BlurParams {
    let half_width = blur_half_width(sigma);
    assert!(
        half_width <= MAX_BLUR_HALF_WIDTH,
        "blur σ {sigma} needs half-width {half_width}, past the shader's {MAX_BLUR_HALF_WIDTH}"
    );
    BlurParams { sigma, half_width }
}

mod params {
    #![allow(dead_code)] // encase derive emits layout-check fns flagged as unused

    #[derive(Clone, Copy, encase::ShaderType)]
    pub(super) struct GrayParams {
        pub(super) apply_srgb: u32,
    }

    /// A Gaussian blur's uniform (`deptheos::metric`'s `BlurParams`).
    #[derive(Clone, Copy, encase::ShaderType)]
    pub(crate) struct BlurParams {
        pub(crate) sigma: f32,
        pub(crate) half_width: u32,
    }

    #[derive(Clone, Copy, encase::ShaderType)]
    pub(super) struct BandpassParams {
        pub(super) amplifier: f32,
    }
}
pub(crate) use params::BlurParams;
use params::{BandpassParams, GrayParams};

/// The metric's uniforms for one pair of blur widths and source encoding.
pub(crate) struct FocusUniforms {
    gray: wgpu::Buffer,
    pre_blur: wgpu::Buffer,
    post_blur: wgpu::Buffer,
    bandpass: wgpu::Buffer,
}

impl FocusUniforms {
    /// Sigmas are in map pixels; `is_linear` sRGB-encodes the source first.
    pub fn new(ctx: &GpuContext, pre_sigma: f32, post_sigma: f32, is_linear: bool) -> Self {
        let blur = |sigma: f32, label| ctx.create_uniform_buffer(&blur_params(sigma), label);
        Self {
            gray: ctx
                .create_uniform_buffer(&GrayParams { apply_srgb: is_linear as u32 }, "focus_gray"),
            pre_blur: blur(pre_sigma, "focus_pre_blur"),
            post_blur: blur(post_sigma, "focus_post_blur"),
            bandpass: ctx.create_uniform_buffer(
                &BandpassParams { amplifier: BANDPASS_GAIN },
                "focus_bandpass",
            ),
        }
    }
}

/// The metric's five kernels, from a module that imports `deptheos::focus`
/// and declares its entry points.
pub(crate) struct FocusKernels {
    gray_h: ComputePipeline,
    gray_v: ComputePipeline,
    blur_h: ComputePipeline,
    blur_v: ComputePipeline,
    bandpass: ComputePipeline,
}

/// One metric's bind groups over a source, the gray `rows` (map width ×
/// source height) and three map-grid scratch maps; it ends in `blurred`.
pub(crate) struct FocusBindings {
    gray_h: wgpu::BindGroup,
    gray_v: wgpu::BindGroup,
    /// The source's height, the horizontal pass's grid.
    rows_height: u32,
    pre_h: wgpu::BindGroup,
    pre_v: wgpu::BindGroup,
    bandpass: wgpu::BindGroup,
    post_h: wgpu::BindGroup,
    post_v: wgpu::BindGroup,
}

impl FocusKernels {
    pub fn new(ctx: &GpuContext, module: &wgpu::ShaderModule) -> Self {
        let make = |name| create_compute_pipeline(&ctx.device, module, name);
        let make_tiled = |name| create_tiled_compute_pipeline(&ctx.device, module, name);
        Self {
            gray_h: make("gray_downscale_h"),
            gray_v: make("gray_downscale_v"),
            blur_h: make_tiled("gaussian_blur_h"),
            blur_v: make_tiled("gaussian_blur_v"),
            bandpass: make("bandpass_diff"),
        }
    }

    /// Bind `source` and the scratch maps, once for every frame.
    pub fn bind(
        &self,
        ctx: &GpuContext,
        uniforms: &FocusUniforms,
        source: &Texture,
        rows: &Texture,
        [gray, focus, blurred]: [&Texture; 3],
    ) -> FocusBindings {
        let view = |t: &Texture| t.create_view(&Default::default());
        let rows_height = rows.height();
        let (source, rows) = (view(source), view(rows));
        let (gray, focus, blurred) = (view(gray), view(focus), view(blurred));
        let group = |pipeline, label, views: &[&wgpu::TextureView], params: &wgpu::Buffer| {
            let mut resources: Vec<_> = views.iter().map(|v| tex(v)).collect();
            resources.push(params.as_entire_binding());
            create_bind_group(&ctx.device, pipeline, 0, label, &resources)
        };
        // gray: source → rows → gray; pre-blur: → focus → blurred;
        // bandpass: gray, blurred → focus; post-blur: → gray → blurred.
        let u = uniforms;
        let gray_v = create_bind_group(
            &ctx.device,
            &self.gray_v,
            0,
            "focus_gray_v",
            &[tex(&rows), tex(&gray)],
        );
        FocusBindings {
            gray_h: group(&self.gray_h, "focus_gray_h", &[&source, &rows], &u.gray),
            gray_v,
            rows_height,
            pre_h: group(&self.blur_h, "focus_pre_blur_h", &[&gray, &focus], &u.pre_blur),
            pre_v: group(&self.blur_v, "focus_pre_blur_v", &[&focus, &blurred], &u.pre_blur),
            bandpass: group(
                &self.bandpass,
                "focus_bandpass",
                &[&gray, &blurred, &focus],
                &u.bandpass,
            ),
            post_h: group(&self.blur_h, "focus_post_blur_h", &[&focus, &gray], &u.post_blur),
            post_v: group(&self.blur_v, "focus_post_blur_v", &[&gray, &blurred], &u.post_blur),
        }
    }

    /// Encode the seven passes over the map grid (the first over the rows).
    pub fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        b: &FocusBindings,
        width: u32,
        height: u32,
    ) {
        dispatch_compute(encoder, &self.gray_h, &b.gray_h, "focus_gray_h", width, b.rows_height);
        for (pipeline, group, label) in [
            (&self.gray_v, &b.gray_v, "focus_gray_v"),
            (&self.blur_h, &b.pre_h, "focus_pre_blur_h"),
            (&self.blur_v, &b.pre_v, "focus_pre_blur_v"),
            (&self.bandpass, &b.bandpass, "focus_bandpass"),
            (&self.blur_h, &b.post_h, "focus_post_blur_h"),
            (&self.blur_v, &b.post_v, "focus_post_blur_v"),
        ] {
            dispatch_compute(encoder, pipeline, group, label, width, height);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reference points of the k1 curve and its equivalent σ.
    #[test]
    fn iir_k1_mapping_table() {
        for (r, k1, sigma) in [
            (0.5, 0.0005, 0.708),
            (1.0, 0.0098, 0.721),
            (2.0, 0.0991, 0.863),
            (5.0, 0.3970, 1.638),
            (10.0, 0.6299, 3.114),
            (20.0, 0.7935, 6.140),
            (100.0, 0.9546, 30.436),
        ] {
            assert!((iir_k1(r) - k1).abs() < 5e-4, "r={r}: k1={}", iir_k1(r));
            assert!((iir_blur_sigma(r) - sigma).abs() < 5e-3, "r={r}: σ={}", iir_blur_sigma(r));
        }
        // r → 0 collapses to the [¼,½,¼] floor, never below.
        assert!((iir_blur_sigma(0.0) - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3);
    }

    /// The table size the half-width limit assumes is the shader's.
    #[test]
    fn blur_limit_matches_the_shader_table() {
        let shader = include_str!("shaders/focus.wgsl");
        assert!(shader.contains("const MAX_BLUR_TAPS: u32 = 512u;"));
        assert!(shader.contains("array<f32, MAX_BLUR_TAPS>"));
        const { assert!(2 * MAX_BLUR_HALF_WIDTH < 512) };
    }

    /// A blur wider than the table fails loudly instead of being clamped.
    #[test]
    #[should_panic(expected = "past the shader's")]
    fn blur_past_the_table_panics() {
        blur_params(255.0);
    }
}
