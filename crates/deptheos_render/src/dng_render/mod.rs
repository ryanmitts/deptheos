//! GPU color rendering. A linear stack: camera RGB → sRGB with the
//! source's own color science, matching the DNG SDK's CPU renderer to 60
//! dB PSNR, 55 dB with a camera-matching profile's sRGB-encoded look table
//! (`tests/integration/dng_render.rs`; ADR-130). An adjusted standard
//! stack: its stored values through its profile (`standard.rs`; ADR-135).

use deptheos_gpu::GpuContext;
use glam::{Mat3, UVec4, Vec4};

use deptheos_core::pack::{QUANTIZE_MODULE, threshold_bind_group};
use deptheos_core::pipelines::COLOR_MODULE;

pub use deptheos_dng::{HueSatMap, RenderParams};
pub use draw::{NeighborStage, Region, Source, TILE};
pub(crate) use stages::{Exposure, LocalTone, Stages};
pub use standard::Linearizer;

mod draw;
#[cfg(test)]
mod reference;
mod stages;
mod standard;

mod place {
    #![allow(dead_code)] // encase derive emits layout-check fns flagged as unused

    use glam::UVec4;

    /// Mirror of the WGSL `Placement` uniform: one tile of a draw.
    #[derive(Clone, Copy, Debug, encase::ShaderType)]
    pub(super) struct Placement {
        pub(super) origins: UVec4,
        pub(super) size: UVec4,
    }
}

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
        /// ProPhoto to the output's linear values; `c0.w` its encoding:
        /// 0 sRGB's curve, else the power `1 / gamma`.
        pub(super) pro_to_output_c0: Vec4,
        pub(super) pro_to_output_c1: Vec4,
        pub(super) pro_to_output_c2: Vec4,
        pub(super) camera_white: Vec4,
        pub(super) flags: UVec4,
        pub(super) hsm_dims: UVec4,
        pub(super) lt_dims: UVec4,
        pub(super) exposure_gain_knee: Vec4,
        pub(super) late: Vec4,
        pub(super) local_tone: Vec4,
        pub(super) base_exposure: Vec4,
    }
}

/// Number of samples the 257-point tone curve is upsampled to: 16 per
/// curve interval.
const TONE_CURVE_SAMPLES: usize = 4097;

pub struct DngRenderPipeline {
    linear: Shader,
    standard: Shader,
}

/// One kind's compiled shader.
struct Shader {
    pipeline: wgpu::ComputePipeline,
    /// Only the quantizing (16-bit) variant reads the threshold table.
    thresholds: Option<wgpu::BindGroup>,
    /// The pass that writes log brightness before exposure, which
    /// the local tone's planes are taken of.
    brightness: wgpu::ComputePipeline,
}

impl Shader {
    /// `source` with the curve stage appended, its tables at binding
    /// `curve_binding`.
    fn new(ctx: &GpuContext, source: &str, entry: &str, curve_binding: u32) -> Self {
        let source = format!(
            "{source}\n@group(0) @binding({curve_binding}) var<storage, read> curve_luts: array<f32>;\n{}",
            include_str!("curve.wgsl")
        );
        let source = source.as_str();
        let constants = stages::constants_module();
        let modules = [
            QUANTIZE_MODULE,
            COLOR_MODULE,
            (constants.as_str(), stages::CONSTANTS_MODULE),
            (include_str!("adjust.wgsl"), "deptheos::adjust"),
        ];
        let module = ctx.create_shader_module_composed(source, entry, &modules);
        let pipeline =
            deptheos_gpu::gpu_utils::create_compute_pipeline(&ctx.device, &module, entry);
        let thresholds =
            ctx.format_support.rgba16unorm_storage.then(|| threshold_bind_group(ctx, &pipeline));
        let brightness = deptheos_gpu::gpu_utils::create_compute_pipeline(
            &ctx.device,
            &module,
            "log_brightness",
        );
        Self { pipeline, thresholds, brightness }
    }
}

impl DngRenderPipeline {
    pub fn new(ctx: &GpuContext) -> Self {
        Self {
            linear: Shader::new(ctx, include_str!("dng_render.wgsl"), "dng_render", 7),
            standard: Shader::new(ctx, include_str!("standard.wgsl"), "standard_render", 5),
        }
    }

    /// Render a whole texture through `constants` to a new one in
    /// `ctx.source_format()`; as Rgba16Unorm it holds exactly the codes
    /// `io::quantize_u16` would give the float render. The input is what
    /// [`Constants::linearizer`] says.
    pub fn render(
        &self,
        ctx: &GpuContext,
        input: &wgpu::Texture,
        constants: &Constants,
    ) -> wgpu::Texture {
        let (width, height) = (input.width(), input.height());
        let output = Self::create_output(ctx, width, height);
        let source = Source::whole(input);
        let prepared = self.with_planes(ctx, PreparedRender::new(ctx, constants), source, height);
        self.draw(ctx, &prepared, source, Region::whole(width, height), &output);
        output
    }

    /// [`Self::render`] a band of `band` rows at a time, each handed to
    /// `sink` with its region, its texture reused for the next: what a
    /// whole picture needs on a device that can't hold it twice.
    pub fn render_bands(
        &self,
        ctx: &GpuContext,
        input: &wgpu::Texture,
        constants: &Constants,
        band: u32,
        mut sink: impl FnMut(Region, &wgpu::Texture),
    ) {
        let (width, height) = (input.width(), input.height());
        let band = band.clamp(1, height);
        let source = Source::whole(input);
        let prepared = self.with_planes(ctx, PreparedRender::new(ctx, constants), source, band);
        let output = Self::create_output(ctx, width, band);
        for y in (0..height).step_by(band as usize) {
            let region = Region { x: 0, y, width, height: band.min(height - y) };
            self.draw(ctx, &prepared, source, region, &output);
            sink(region, &output);
        }
    }

    /// A texture [`Self::draw`] can write: `ctx.source_format()`,
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
}

/// What the shader is given for one draw, on the CPU. Made by
/// [`crate::resolve`]; equal values draw equal pictures.
#[derive(Clone, Debug, PartialEq)]
pub struct Constants(Kind);

#[derive(Clone, Debug, PartialEq)]
enum Kind {
    /// A linear stack: the uniform, the composed tone curve and the
    /// profile's tables.
    Linear {
        uniform: params::DngColorParams,
        tone: Vec<f32>,
        /// Empty when the profile has none (or an incomplete one).
        hsm: Vec<f32>,
        look: Vec<f32>,
        /// Contrast, Whites and Blacks.
        curve: Option<crate::curve::Curve>,
        /// sRGB or a wide space.
        output: crate::OutputSpace,
    },
    Standard(standard::StandardConstants),
}

impl Constants {
    /// A linear stack's constants from its render parameters, through
    /// `stages` (none: the SDK's render exactly), into `output`: a wide
    /// space, else sRGB.
    pub(crate) fn linear(rp: &RenderParams, stages: &Stages, output: crate::OutputSpace) -> Self {
        let lut = |map: Option<&HueSatMap>| match map {
            Some(map) if lut_is_complete(map) => map.data.clone(),
            _ => Vec::new(),
        };
        // The ramp and the darkening curve take the exposure the stage
        // leaves them, else the baseline.
        let ramp = stages.exposure.map_or(rp.baseline_exposure, |e| e.ramp);
        let (output, to_output) = output_encoding(output);
        let mut uniform = pack_params(rp, ramp, to_output);
        if let Some(exposure) = stages.exposure {
            uniform.flags.z = 1;
            uniform.exposure_gain_knee = exposure.gain_knee();
        }
        if let Some(local_tone) = stages.local_tone {
            uniform.flags.z |= local_tone.terms();
            uniform.local_tone = local_tone.packed();
            uniform.base_exposure = pack_params(rp, rp.baseline_exposure, to_output).exposure;
        }
        (uniform.flags.w, uniform.late) = stages.late();
        Self(Kind::Linear {
            output,
            curve: stages.curve.clone(),
            uniform,
            tone: precompose_tone_curve(&rp.tone_curve, exposure_tone(ramp), TONE_CURVE_SAMPLES),
            hsm: lut(rp.hue_sat_map.as_ref()),
            look: lut(rp.look_table.as_ref()),
        })
    }

    /// A standard stack's constants: stored values in `profile` (`None`:
    /// sRGB) drawn into `output` through `stages`.
    pub(crate) fn standard(
        profile: Option<&deptheos_core::io::IccProfile>,
        output: crate::OutputSpace,
        stages: &Stages,
    ) -> Self {
        Self(Kind::Standard(standard::StandardConstants::new(profile, output, stages)))
    }

    /// These constants with exposure's shoulder a clamp at white, as the
    /// SDK's ramp clips: for the tests that match the SDK.
    #[doc(hidden)]
    pub fn without_shoulder(mut self) -> Self {
        match &mut self.0 {
            Kind::Linear { uniform, .. } => uniform.exposure_gain_knee.y = 1.0,
            Kind::Standard(standard) => standard.uniform.exposure_gain_knee.y = 1.0,
        }
        self
    }

    /// The space the drawn values are in: sRGB but for a standard stack
    /// drawn in its own profile.
    pub fn output(&self) -> crate::OutputSpace {
        match &self.0 {
            Kind::Linear { output, .. } => *output,
            Kind::Standard(standard) => standard.output,
        }
    }

    /// Whether the draw has a slider that reads neighbors, so it needs
    /// planes of the whole frame ([`DngRenderPipeline::with_planes`]).
    pub fn has_local_tone(&self) -> bool {
        self.local_needs().is_some()
    }

    /// The planes the draw's local tone reads, if it has any.
    fn local_needs(&self) -> Option<crate::local_tone::Needs> {
        let z = match &self.0 {
            Kind::Linear { uniform, .. } => uniform.flags.z,
            Kind::Standard(standard) => standard.uniform.flags.z,
        };
        let needs = stages::needs(z);
        (needs != crate::local_tone::Needs::default()).then_some(needs)
    }

    /// How the stored pixels go to the GPU for this draw: as they are
    /// (`None`), or linearized on the CPU first, uploaded as Rgba32Float.
    pub fn linearizer(&self) -> Option<&Linearizer> {
        match &self.0 {
            Kind::Linear { .. } => None,
            Kind::Standard(standard) => standard.linearizer.as_ref(),
        }
    }
}

/// One [`Constants`]' GPU buffers, built once and reused by every render
/// with those constants, and the local tone's planes of the whole frame
/// when they have it.
pub struct PreparedRender {
    buffers: Prepared,
    planes: Planes,
}

/// Lifecycle: the local tone's planes of the whole frame. `None` for a
/// draw without local tone; `Needed` until
/// [`DngRenderPipeline::with_planes`] makes them `Built`, which a draw with
/// local tone requires.
enum Planes {
    None,
    Needed(crate::local_tone::Needs),
    Built(crate::local_tone::Planes),
}

enum Prepared {
    Linear {
        params: wgpu::Buffer,
        tone: wgpu::Buffer,
        hsm: wgpu::Buffer,
        look: wgpu::Buffer,
        curve: wgpu::Buffer,
    },
    Standard {
        params: wgpu::Buffer,
        decode: wgpu::Buffer,
        curve: wgpu::Buffer,
    },
}

impl PreparedRender {
    pub fn new(ctx: &GpuContext, constants: &Constants) -> Self {
        let buffers = Self::buffers(ctx, constants);
        let planes = constants.local_needs().map_or(Planes::None, Planes::Needed);
        Self { buffers, planes }
    }

    fn buffers(ctx: &GpuContext, constants: &Constants) -> Prepared {
        match &constants.0 {
            Kind::Linear { uniform, tone, hsm, look, curve, output: _ } => Prepared::Linear {
                params: ctx.create_uniform_buffer(uniform, "dng_color_params"),
                tone: storage_buffer(ctx, tone, "dng_tone_curve"),
                hsm: lut_buffer(ctx, hsm, "dng_hsm"),
                look: lut_buffer(ctx, look, "dng_look"),
                curve: lut_buffer(ctx, &stages::pack_curve(curve.as_ref()).0, "dng_curve"),
            },
            Kind::Standard(standard) => Prepared::Standard {
                params: ctx.create_uniform_buffer(&standard.uniform, "standard_params"),
                decode: lut_buffer(ctx, &standard.decode, "standard_decode"),
                curve: lut_buffer(
                    ctx,
                    &stages::pack_curve(standard.curve.as_ref()).0,
                    "standard_curve",
                ),
            },
        }
    }
}

/// Pack render params into the shader uniform, the ramp at `ramp` stops.
fn pack_params(rp: &RenderParams, ramp: f64, to_output: (Mat3, f32)) -> params::DngColorParams {
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
    let exposure = ramp as f32;
    let white = 1.0 / 2f32.powf(exposure.max(0.0));
    let shadows = 5.0f32;
    let black = (shadows * rp.shadow_scale as f32 * 0.001).min(0.99 * white);
    let slope = 1.0 / (white - black);
    let radius = (0.5 * black).min((1.0 / 16.0) / slope);
    // With radius 0 the quadratic branch is unreachable: 0, not +inf.
    let q_scale = if radius > 0.0 { slope / (4.0 * radius) } else { 0.0 };

    let cam = Mat3::from_cols_array(&rp.camera_to_prophoto).transpose(); // row-major input

    let (hsm_flag, hsm_dims) = lut_dims(rp.hue_sat_map.as_ref());
    let (lt_flag, lt_dims) = lut_dims(rp.look_table.as_ref());

    params::DngColorParams {
        range_inv: Vec4::new(range_scale, range_scale, range_scale, s3bl),
        cam_to_prophoto_c0: (cam.x_axis, 0.0).into(),
        cam_to_prophoto_c1: (cam.y_axis, 0.0).into(),
        cam_to_prophoto_c2: (cam.z_axis, 0.0).into(),
        exposure: Vec4::new(black, slope, radius, q_scale),
        pro_to_output_c0: (to_output.0.x_axis, to_output.1).into(),
        pro_to_output_c1: (to_output.0.y_axis, 0.0).into(),
        pro_to_output_c2: (to_output.0.z_axis, 0.0).into(),
        camera_white,
        flags: UVec4::new(hsm_flag, lt_flag, 0, 0),
        hsm_dims,
        lt_dims,
        exposure_gain_knee: Vec4::new(1.0, 1.0, 0.0, 0.0),
        late: Vec4::ZERO,
        local_tone: Vec4::ZERO,
        base_exposure: Vec4::ZERO,
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

/// The space a linear stack draws into for `output`, its matrix from
/// ProPhoto, and its encoding (0: sRGB's curve; else `1 / gamma`): sRGB's
/// own, or a wide space's profile, its matrix adapted through D50 and its
/// curve a pure power (Display P3's is sRGB's).
fn output_encoding(output: crate::OutputSpace) -> (crate::OutputSpace, (Mat3, f32)) {
    let Some(profile) = output.wide_profile() else {
        return (crate::OutputSpace::Srgb, (prophoto_to_srgb(), 0.0));
    };
    let m = moxcms::ColorProfile::new_pro_photo_rgb().transform_matrix(&profile).v;
    let matrix =
        Mat3::from_cols_array(&std::array::from_fn(|i| m[i / 3][i % 3] as f32)).transpose();
    let power = match profile.red_trc {
        // A one-entry curve is a gamma in u8Fixed8.
        Some(moxcms::ToneReprCurve::Lut(ref v)) if v.len() == 1 => 256.0 / f32::from(v[0]),
        _ => 0.0,
    };
    (output, (matrix, power))
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

/// The exposure-tone pre-curve, applied only for a negative exposure of
/// `stops` (the SDK's `dng_function_exposure_tone`).
fn exposure_tone(stops: f64) -> Vec4 {
    let exposure = stops as f32;
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
/// (the shaders gate reads on `flags`).
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
            &["DngColorParams", "Placement"],
        );
        deptheos_gpu::wgsl_mirror::assert_mirrors(
            include_str!("standard.rs"),
            include_str!("standard.wgsl"),
            &["StandardParams"],
        );
        deptheos_gpu::wgsl_mirror::assert_mirrors(
            include_str!("mod.rs"),
            include_str!("standard.wgsl"),
            &["Placement"],
        );
    }
}
