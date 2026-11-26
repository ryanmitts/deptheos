//! A standard stack's constants for `standard.wgsl`: the decode table of
//! its profile's curves, the matrices through linear ProPhoto, and how the
//! output is encoded. A table-based profile has no curves to tabulate, so
//! its pixels are linearized on the CPU by moxcms ([`Linearizer`]).

use std::fmt;
use std::sync::Arc;

use deptheos_core::io::IccProfile;
use deptheos_gpu::GpuContext;
use glam::{UVec4, Vec4};
use moxcms::{ColorProfile, Matrix3d, ToneReprCurve};

use crate::OutputSpace;

/// Codes a 16-bit channel holds, each with its entry in the decode table.
pub(super) const CODES: usize = 1 << 16;

pub(super) mod params {
    #![allow(dead_code)] // encase derive emits layout-check fns flagged as unused

    use glam::{UVec4, Vec4};

    /// Mirror of the WGSL `StandardParams` uniform.
    #[derive(Clone, Copy, Debug, PartialEq, encase::ShaderType)]
    pub(crate) struct StandardParams {
        pub(crate) to_prophoto_c0: Vec4,
        pub(crate) to_prophoto_c1: Vec4,
        pub(crate) to_prophoto_c2: Vec4,
        pub(crate) from_prophoto_c0: Vec4,
        pub(crate) from_prophoto_c1: Vec4,
        pub(crate) from_prophoto_c2: Vec4,
        pub(crate) flags: UVec4,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StandardConstants {
    pub(crate) uniform: params::StandardParams,
    /// Each channel's linear value for every code, red's, green's then
    /// blue's; empty when the input is linearized.
    pub(crate) decode: Vec<f32>,
    pub(crate) linearizer: Option<Linearizer>,
    /// The space the drawn values are in.
    pub(crate) output: OutputSpace,
}

impl StandardConstants {
    /// Stored values in `profile` (`None`, or one moxcms can't read: sRGB)
    /// drawn into `output`. A table-based profile draws in sRGB: its own
    /// space has no curves to encode back through.
    pub(crate) fn new(profile: Option<&IccProfile>, output: OutputSpace) -> Self {
        let prophoto = ColorProfile::new_pro_photo_rgb();
        let srgb = ColorProfile::new_srgb();
        let parsed = profile.and_then(|p| Some((p, ColorProfile::new_from_slice(p.bytes()).ok()?)));
        let input = match parsed {
            Some((profile, parsed)) if is_table_based(&parsed) => {
                match Linearizer::new(profile.clone(), &parsed) {
                    Some(linearizer) => Input::Linearized(linearizer),
                    None => Input::Curves(Box::new(srgb.clone())),
                }
            }
            Some((_, parsed)) => Input::Curves(Box::new(parsed)),
            None => Input::Curves(Box::new(srgb.clone())),
        };
        let (to_prophoto, decode, own) = match &input {
            Input::Curves(source) => {
                (source.transform_matrix(&prophoto), decode_table(source), Some(source))
            }
            Input::Linearized(_) => (Matrix3d::IDENTITY, Vec::new(), None),
        };
        let (output, from_prophoto, own_curves) = match (output, own) {
            (OutputSpace::StackProfile, Some(source)) => {
                (output, prophoto.transform_matrix(source), true)
            }
            (OutputSpace::StackProfile | OutputSpace::Srgb, _) => {
                (OutputSpace::Srgb, prophoto.transform_matrix(&srgb), false)
            }
        };
        let [t0, t1, t2] = columns(&to_prophoto);
        let [f0, f1, f2] = columns(&from_prophoto);
        let uniform = params::StandardParams {
            to_prophoto_c0: t0,
            to_prophoto_c1: t1,
            to_prophoto_c2: t2,
            from_prophoto_c0: f0,
            from_prophoto_c1: f1,
            from_prophoto_c2: f2,
            flags: UVec4::new(u32::from(decode.is_empty()), u32::from(own_curves), 0, 0),
        };
        let linearizer = match input {
            Input::Linearized(linearizer) => Some(linearizer),
            Input::Curves(_) => None,
        };
        Self { uniform, decode, linearizer, output }
    }
}

/// What the shader reads: stored values through these curves, or values
/// linearized on the CPU.
enum Input {
    Curves(Box<ColorProfile>),
    Linearized(Linearizer),
}

/// A profile drawn through moxcms on the CPU: one with lookup tables, or
/// without a matrix and three curves.
fn is_table_based(profile: &ColorProfile) -> bool {
    !profile.is_matrix_shaper()
        || profile.lut_a_to_b_colorimetric.is_some()
        || profile.lut_a_to_b_perceptual.is_some()
}

/// Every code of each channel through its curve. A curve moxcms can't
/// evaluate is taken as linear.
fn decode_table(profile: &ColorProfile) -> Vec<f32> {
    let curves = [&profile.red_trc, &profile.green_trc, &profile.blue_trc];
    let mut table = Vec::with_capacity(3 * CODES);
    for curve in curves {
        let evaluator = curve.as_ref().and_then(|c| c.make_linear_evaluator().ok());
        table.extend((0..CODES).map(|code| {
            let v = code as f32 / 65535.0;
            evaluator.as_ref().map_or(v, |e| e.evaluate_value(v))
        }));
    }
    table
}

/// A row-major matrix's columns, for a WGSL `mat3x3f`.
fn columns(m: &Matrix3d) -> [Vec4; 3] {
    std::array::from_fn(|c| Vec4::new(m.v[0][c] as f32, m.v[1][c] as f32, m.v[2][c] as f32, 0.0))
}

/// A table-based profile's pixels to linear ProPhoto, on the CPU.
#[derive(Clone)]
pub struct Linearizer {
    profile: IccProfile,
    transform: Arc<moxcms::TransformF32Executor>,
}

impl PartialEq for Linearizer {
    fn eq(&self, other: &Self) -> bool {
        self.profile == other.profile
    }
}

impl fmt::Debug for Linearizer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Linearizer({:?})", self.profile)
    }
}

/// Pixels linearized at a time, bounding the scratch rows.
const LINEARIZE_CHUNK: usize = 1 << 20;

impl Linearizer {
    fn new(profile: IccProfile, parsed: &ColorProfile) -> Option<Self> {
        let mut linear = ColorProfile::new_pro_photo_rgb();
        let identity = ToneReprCurve::Lut(Vec::new());
        linear.red_trc = Some(identity.clone());
        linear.green_trc = Some(identity.clone());
        linear.blue_trc = Some(identity);
        let options = moxcms::TransformOptions {
            rendering_intent: moxcms::RenderingIntent::RelativeColorimetric,
            ..Default::default()
        };
        let layout = moxcms::Layout::Rgb;
        match parsed.create_transform_f32(layout, &linear, layout, options) {
            Ok(transform) => Some(Self { profile, transform }),
            Err(e) => {
                tracing::warn!("can't linearize {:?}, drawing it as sRGB: {e}", profile.name());
                None
            }
        }
    }

    /// `rgb16` (`width`×`height` interleaved RGB) linearized into a new
    /// Rgba32Float texture, a band of rows at a time.
    pub fn upload(
        &self,
        ctx: &GpuContext,
        rgb16: &[u16],
        width: u32,
        height: u32,
    ) -> wgpu::Texture {
        let texture = ctx.create_texture(
            width,
            height,
            wgpu::TextureFormat::Rgba32Float,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            "linearized",
        );
        let band = (LINEARIZE_CHUNK as u32 / width.max(1)).max(1);
        for top in (0..height).step_by(band as usize) {
            let rows = band.min(height - top);
            let (start, end) = ((top * width * 3) as usize, ((top + rows) * width * 3) as usize);
            let rgba = self.linearize(&rgb16[start..end]);
            ctx.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: 0, y: top, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                bytemuck::cast_slice(&rgba),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(width * 16),
                    rows_per_image: Some(rows),
                },
                wgpu::Extent3d { width, height: rows, depth_or_array_layers: 1 },
            );
        }
        texture
    }

    /// `rgb16` (interleaved RGB) in linear ProPhoto, as interleaved RGBA
    /// with alpha 1.
    pub fn linearize(&self, rgb16: &[u16]) -> Vec<f32> {
        let mut out = Vec::with_capacity(rgb16.len() / 3 * 4);
        let (mut src, mut dst) = (Vec::new(), Vec::new());
        for chunk in rgb16.chunks(LINEARIZE_CHUNK * 3) {
            src.clear();
            src.extend(chunk.iter().map(|&v| f32::from(v) / 65535.0));
            dst.resize(src.len(), 0.0);
            if self.transform.transform(&src, &mut dst).is_err() {
                dst.copy_from_slice(&src);
            }
            out.extend(dst.as_chunks::<3>().0.iter().flat_map(|&[r, g, b]| [r, g, b, 1.0]));
        }
        out
    }
}
