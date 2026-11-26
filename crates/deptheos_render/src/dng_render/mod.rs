//! GPU DNG color rendering: linear camera RGB → sRGB with the source's own
//! color science, matching the DNG SDK's CPU renderer to 60 dB PSNR, 55 dB
//! with a camera-matching profile's sRGB-encoded look table
//! (`tests/integration/dng_render.rs`; ADR-130).

use deptheos_gpu::GpuContext;
use deptheos_gpu::gpu_utils::{WORKGROUP_SIZE, create_bind_group, dispatch_compute_with, tex};
use glam::{Mat3, UVec4, Vec4};

use deptheos_core::pack::{QUANTIZE_MODULE, threshold_bind_group};
use deptheos_core::pipelines::COLOR_MODULE;

pub use deptheos_dng::{HueSatMap, RenderParams};

mod params {
    #![allow(dead_code)] // encase derive emits layout-check fns flagged as unused

    use glam::{UVec4, Vec4};

    /// Mirror of the WGSL `DngColorParams` uniform.
    #[derive(Clone, Copy, Debug, PartialEq, encase::ShaderType)]
    pub(super) struct DngColorParams {
        pub(super) range_inv: Vec4,
        pub(super) cam_to_prophoto_c0: Vec4,
        pub(super) cam_to_prophoto_c1: Vec4,
        pub(super) cam_to_prophoto_c2: Vec4,
        pub(super) exposure: Vec4,
        pub(super) pro_to_srgb_c0: Vec4,
        pub(super) pro_to_srgb_c1: Vec4,
        pub(super) pro_to_srgb_c2: Vec4,
        pub(super) camera_white: Vec4,
        pub(super) flags: UVec4,
        pub(super) hsm_dims: UVec4,
        pub(super) lt_dims: UVec4,
    }
}

/// Number of samples the 257-point tone curve is upsampled to: 16 per
/// curve interval.
const TONE_CURVE_SAMPLES: usize = 4097;

pub struct DngRenderPipeline {
    pipeline: wgpu::ComputePipeline,
    /// Only the quantizing (16-bit) variant reads the threshold table.
    thresholds: Option<wgpu::BindGroup>,
}

impl DngRenderPipeline {
    pub fn new(ctx: &GpuContext) -> Self {
        let module = ctx.create_shader_module_composed(
            include_str!("dng_render.wgsl"),
            "dng_render",
            &[QUANTIZE_MODULE, COLOR_MODULE],
        );
        let pipeline =
            deptheos_gpu::gpu_utils::create_compute_pipeline(&ctx.device, &module, "dng_render");
        let thresholds =
            ctx.format_support.rgba16unorm_storage.then(|| threshold_bind_group(ctx, &pipeline));
        Self { pipeline, thresholds }
    }

    /// Render a linear camera-RGB texture to a new sRGB-encoded one in
    /// `ctx.source_format()`; as Rgba16Unorm it holds exactly the codes
    /// `io::quantize_u16` would give the float render.
    pub fn render(
        &self,
        ctx: &GpuContext,
        input: &wgpu::Texture,
        constants: &Constants,
    ) -> wgpu::Texture {
        let (width, height) = (input.width(), input.height());
        let output = Self::create_output(ctx, width, height);
        let prepared = PreparedRender::new(ctx, constants);
        self.render_into(ctx, &prepared, input, &output, [width, height]);
        output
    }

    /// A texture [`Self::render_into`] can write: `ctx.source_format()`,
    /// readable back.
    pub fn create_output(ctx: &GpuContext, width: u32, height: u32) -> wgpu::Texture {
        ctx.create_texture(
            width,
            height,
            ctx.source_format(),
            wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            "dng_render_out",
        )
    }

    /// Render the top-left `extent` of `input` into the same pixels of
    /// `output`. Both may be larger (pooled); the render is per pixel, so a
    /// rect renders as it would within the whole image.
    pub fn render_into(
        &self,
        ctx: &GpuContext,
        prepared: &PreparedRender,
        input: &wgpu::Texture,
        output: &wgpu::Texture,
        [width, height]: [u32; 2],
    ) {
        let mut encoder = ctx.encoder("dng_render");
        let (in_view, out_view) =
            (input.create_view(&Default::default()), output.create_view(&Default::default()));
        let group0 = create_bind_group(
            &ctx.device,
            &self.pipeline,
            0,
            "dng_render",
            &[
                tex(&in_view),                       // in_tex
                tex(&out_view),                      // out_tex
                prepared.params.as_entire_binding(), // params
                prepared.tone.as_entire_binding(),   // tone_curve
                prepared.hsm.as_entire_binding(),    // hsm_lut
                prepared.look.as_entire_binding(),   // look_lut
            ],
        );
        let groups: Vec<&wgpu::BindGroup> =
            std::iter::once(&group0).chain(self.thresholds.as_ref()).collect();
        dispatch_compute_with(
            &mut encoder,
            &self.pipeline,
            &groups,
            "dng_render",
            width,
            height,
            WORKGROUP_SIZE,
        );
        ctx.submit(encoder);
    }
}

/// What the shader is given for one draw, on the CPU: the uniform, the
/// composed tone curve and the profile's tables. Made by
/// [`crate::resolve`]; equal values draw equal pictures.
#[derive(Clone, Debug, PartialEq)]
pub struct Constants {
    uniform: params::DngColorParams,
    tone: Vec<f32>,
    /// Empty when the profile has none (or an incomplete one).
    hsm: Vec<f32>,
    look: Vec<f32>,
}

impl Constants {
    /// A linear stack's constants from its render parameters.
    pub(crate) fn linear(rp: &RenderParams) -> Self {
        let lut = |map: Option<&HueSatMap>| match map {
            Some(map) if lut_is_complete(map) => map.data.clone(),
            _ => Vec::new(),
        };
        Self {
            uniform: pack_params(rp),
            tone: precompose_tone_curve(&rp.tone_curve, exposure_tone(rp), TONE_CURVE_SAMPLES),
            hsm: lut(rp.hue_sat_map.as_ref()),
            look: lut(rp.look_table.as_ref()),
        }
    }
}

/// One [`Constants`]' GPU buffers, built once and reused by every render
/// with those constants.
pub struct PreparedRender {
    params: wgpu::Buffer,
    tone: wgpu::Buffer,
    hsm: wgpu::Buffer,
    look: wgpu::Buffer,
}

impl PreparedRender {
    pub fn new(ctx: &GpuContext, constants: &Constants) -> Self {
        Self {
            params: ctx.create_uniform_buffer(&constants.uniform, "dng_color_params"),
            tone: storage_buffer(ctx, &constants.tone, "dng_tone_curve"),
            hsm: lut_buffer(ctx, &constants.hsm, "dng_hsm"),
            look: lut_buffer(ctx, &constants.look, "dng_look"),
        }
    }
}

/// Pack render params into the shader uniform.
fn pack_params(rp: &RenderParams) -> params::DngColorParams {
    // Stage3 black level, normalized; the shader subtracts then rescales.
    let s3bl = (rp.black_level / 65535.0) as f32;
    let range_scale = if s3bl < 1.0 { 1.0 / (1.0 - s3bl) } else { 1.0 };

    // Camera white normalized so the max channel clips at exactly 1.0. No
    // separate white balance: camera_to_prophoto (the SDK's CameraToPCS)
    // already maps the camera white to the PCS white for both profile
    // kinds, and the SDK's reference render is clip → matrix → clamp.
    let cw = rp.camera_white;
    let max_white = cw[0].max(cw[1]).max(cw[2]);
    // The shader clips to this white; with no usable white (all zero, or
    // not finite) nothing is clipped, rather than everything to black.
    let camera_white = if max_white > 1e-10 && max_white.is_finite() {
        Vec4::new(cw[0] / max_white, cw[1] / max_white, cw[2] / max_white, 0.0)
    } else {
        Vec4::ONE
    };

    // Exposure ramp constants (the DNG SDK's dng_render defaults).
    let exposure = rp.baseline_exposure as f32;
    let white = 1.0 / 2f32.powf(exposure.max(0.0));
    let shadows = 5.0f32;
    let black = (shadows * rp.shadow_scale as f32 * 0.001).min(0.99 * white);
    let slope = 1.0 / (white - black);
    let radius = (0.5 * black).min((1.0 / 16.0) / slope);
    // With radius 0 the quadratic branch is unreachable: 0, not +inf.
    let q_scale = if radius > 0.0 { slope / (4.0 * radius) } else { 0.0 };

    let cam = Mat3::from_cols_array(&rp.camera_to_prophoto).transpose(); // row-major input
    let pro_to_srgb = prophoto_to_srgb();

    let (hsm_flag, hsm_dims) = lut_dims(rp.hue_sat_map.as_ref());
    let (lt_flag, lt_dims) = lut_dims(rp.look_table.as_ref());

    params::DngColorParams {
        range_inv: Vec4::new(range_scale, range_scale, range_scale, s3bl),
        cam_to_prophoto_c0: (cam.x_axis, 0.0).into(),
        cam_to_prophoto_c1: (cam.y_axis, 0.0).into(),
        cam_to_prophoto_c2: (cam.z_axis, 0.0).into(),
        exposure: Vec4::new(black, slope, radius, q_scale),
        pro_to_srgb_c0: (pro_to_srgb.x_axis, 0.0).into(),
        pro_to_srgb_c1: (pro_to_srgb.y_axis, 0.0).into(),
        pro_to_srgb_c2: (pro_to_srgb.z_axis, 0.0).into(),
        camera_white,
        flags: UVec4::new(hsm_flag, lt_flag, 0, 0),
        hsm_dims,
        lt_dims,
    }
}

fn lut_dims(lut: Option<&HueSatMap>) -> (u32, UVec4) {
    match lut {
        Some(map) if lut_is_complete(map) => {
            let srgb = u32::from(map.srgb_encoded);
            (1, UVec4::new(map.hue_divisions, map.sat_divisions, map.val_divisions, srgb))
        }
        _ => (0, UVec4::ZERO),
    }
}

/// Whether the LUT's data covers its grid (three floats per entry); the
/// shader indexes by the dims alone, so a short table counts as absent.
fn lut_is_complete(map: &HueSatMap) -> bool {
    let entries = (map.hue_divisions as usize)
        .saturating_mul(map.sat_divisions as usize)
        .saturating_mul(map.val_divisions as usize);
    entries > 0 && map.data.len() >= entries.saturating_mul(3)
}

/// ProPhoto RGB → linear sRGB (both D50 PCS): inverse(sRGB→XYZ) ·
/// (ProPhoto→XYZ).
fn prophoto_to_srgb() -> Mat3 {
    let prophoto_to_xyz = Mat3::from_cols_array(&[
        0.7977, 0.1352, 0.0313, //
        0.2880, 0.7119, 0.0001, //
        0.0000, 0.0000, 0.8249,
    ])
    .transpose();
    let srgb_to_xyz = Mat3::from_cols_array(&[
        0.4361, 0.3851, 0.1431, //
        0.2225, 0.7169, 0.0606, //
        0.0139, 0.0971, 0.7141,
    ])
    .transpose();
    srgb_to_xyz.inverse() * prophoto_to_xyz
}

/// The exposure-tone pre-curve, applied only for negative baseline
/// exposures (the SDK's `dng_function_exposure_tone`).
fn exposure_tone(rp: &RenderParams) -> Vec4 {
    let exposure = rp.baseline_exposure as f32;
    if exposure < 0.0 {
        let slope = 2f32.powf(exposure);
        let a = (16.0 / 9.0) * (1.0 - slope);
        let b = slope - 0.5 * a;
        let c = 1.0 - a - b;
        Vec4::new(slope, a, b, c)
    } else {
        Vec4::ZERO
    }
}

/// The exposure-tone function then the profile curve, as `count` uniform
/// samples. A curve of fewer than two points is the identity.
fn precompose_tone_curve(raw_curve: &[f32], tone: Vec4, count: usize) -> Vec<f32> {
    let mut result = vec![0.0f32; count];
    let n = (count.max(2) - 1) as f32;
    let curve_n = raw_curve.len().saturating_sub(1);
    for (i, out) in result.iter_mut().enumerate() {
        let mut x = i as f32 / n;
        if tone.x != 0.0 {
            let (slope, a, b, c) = (tone.x, tone.y, tone.z, tone.w);
            x = if x <= 0.25 { x * slope } else { (a * x + b) * x + c };
        }
        *out = if curve_n == 0 {
            x
        } else {
            let y = x * curve_n as f32;
            let idx = (y as usize).min(curve_n - 1);
            let fract = y - idx as f32;
            raw_curve[idx] * (1.0 - fract) + raw_curve[idx + 1] * fract
        };
    }
    result
}

fn storage_buffer(ctx: &GpuContext, data: &[f32], label: &str) -> wgpu::Buffer {
    ctx.create_buffer_with_data(label, bytemuck::cast_slice(data), wgpu::BufferUsages::STORAGE)
}

/// LUT storage buffer; an absent (empty) LUT binds a one-float placeholder
/// (the shader gates reads on `flags`).
fn lut_buffer(ctx: &GpuContext, lut: &[f32], label: &str) -> wgpu::Buffer {
    storage_buffer(ctx, if lut.is_empty() { &[0.0] } else { lut }, label)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A profile curve of fewer than two points is the identity.
    #[test]
    fn short_tone_curve_is_identity() {
        for curve in [&[][..], &[0.5][..]] {
            let samples = precompose_tone_curve(curve, Vec4::ZERO, 5);
            assert_eq!(samples, vec![0.0, 0.25, 0.5, 0.75, 1.0]);
        }
        // Two points interpolate linearly.
        let samples = precompose_tone_curve(&[0.0, 2.0], Vec4::ZERO, 3);
        assert_eq!(samples, vec![0.0, 1.0, 2.0]);
    }

    /// A LUT whose data does not cover its grid is absent (flag 0, zero dims).
    #[test]
    fn short_lut_is_absent() {
        let full = HueSatMap {
            hue_divisions: 2,
            sat_divisions: 2,
            val_divisions: 1,
            data: vec![0.0; 12],
            srgb_encoded: false,
        };
        let short = HueSatMap { data: vec![0.0; 11], ..full.clone() };
        let empty_grid = HueSatMap { hue_divisions: 0, ..full.clone() };
        assert_eq!(lut_dims(Some(&full)), (1, UVec4::new(2, 2, 1, 0)));
        let srgb = HueSatMap { srgb_encoded: true, ..full.clone() };
        assert_eq!(lut_dims(Some(&srgb)), (1, UVec4::new(2, 2, 1, 1)));
        assert_eq!(lut_dims(Some(&short)), (0, UVec4::ZERO));
        assert_eq!(lut_dims(Some(&empty_grid)), (0, UVec4::ZERO));
        assert_eq!(lut_dims(None), (0, UVec4::ZERO));
    }

    /// The shader's `DngColorParams` has the Rust struct's fields.
    #[test]
    fn the_uniform_matches_its_wgsl() {
        deptheos_gpu::wgsl_mirror::assert_mirrors(
            include_str!("mod.rs"),
            include_str!("dng_render.wgsl"),
            &["DngColorParams"],
        );
    }
}
