//! Frame alignment transforms at the stacker boundary.
//!
//! Transforms map reference coordinates → frame coordinates (the backward
//! sampling matrix), as produced by the aligners in `deptheos_aligner`.

pub use nalgebra::Affine2;

/// One frame's alignment: the backward sampling matrix and the exposure
/// factor to frame 0's brightness, applied wherever the frame is warped for
/// blending (the depth stacker's focus metric ignores it).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameAlignment {
    pub transform: Affine2<f32>,
    pub brightness: f32,
}

impl FrameAlignment {
    pub fn new(transform: Affine2<f32>, brightness: f32) -> Self {
        Self { transform, brightness }
    }

    /// A transform with no exposure correction.
    pub fn from_transform(transform: Affine2<f32>) -> Self {
        Self { transform, brightness: 1.0 }
    }

    pub fn identity() -> Self {
        Self::from_transform(Affine2::identity())
    }

    /// Magnification against the reference, `sqrt(|det A|)`: above 1 the
    /// frame is zoomed in.
    pub fn magnification(&self) -> f32 {
        let m = self.transform.matrix();
        (m[(0, 0)] * m[(1, 1)] - m[(0, 1)] * m[(1, 0)]).abs().sqrt()
    }
}

impl From<Affine2<f32>> for FrameAlignment {
    fn from(transform: Affine2<f32>) -> Self {
        Self::from_transform(transform)
    }
}

// A module so the allow covers the encase derive's layout-check fns.
mod params {
    #![allow(dead_code)]

    /// `WarpParams` in the shared `deptheos::warp` shader module
    /// (`shaders/warp_common.wgsl`).
    #[derive(Clone, Copy, encase::ShaderType)]
    pub(crate) struct WarpParams {
        pub x_axis: glam::Vec4,
        pub y_axis: glam::Vec4,
        pub z_axis: glam::Vec4,
        /// 1 to sRGB-encode a linear source (focus metrics, luma, the
        /// Laplacian pyramid).
        pub apply_srgb: u32,
        /// Exposure factor on the sampled color, before any sRGB step; 1.0
        /// for none.
        pub brightness: f32,
    }
}
pub(crate) use params::WarpParams;

impl WarpParams {
    pub fn from_transform(t: &Affine2<f32>, apply_srgb: bool) -> Self {
        let m = t.matrix();
        Self {
            x_axis: glam::Vec4::new(m[(0, 0)], m[(1, 0)], 0.0, 0.0),
            y_axis: glam::Vec4::new(m[(0, 1)], m[(1, 1)], 0.0, 0.0),
            z_axis: glam::Vec4::new(m[(0, 2)], m[(1, 2)], 1.0, 0.0),
            apply_srgb: apply_srgb as u32,
            brightness: 1.0,
        }
    }

    pub fn from_alignment(a: &FrameAlignment, apply_srgb: bool) -> Self {
        Self { brightness: a.brightness, ..Self::from_transform(&a.transform, apply_srgb) }
    }

    /// The same transform on a grid at `ratio` frame pixels per grid pixel,
    /// mapped `frame = (grid + ½)·ratio − ½` (the depth map grid). With
    /// `S = diag(ratio)`, `c = (½, ½)`, `[A | t]` becomes
    /// `[S⁻¹AS | S⁻¹(t + (A − I)(S − I)c)]`; the identity stays exact.
    pub fn on_grid(t: &Affine2<f32>, ratio: [f32; 2], apply_srgb: bool) -> Self {
        let m = t.matrix().cast::<f64>();
        let [sx, sy] = ratio.map(f64::from);
        let a = [[m[(0, 0)], m[(0, 1)]], [m[(1, 0)], m[(1, 1)]]];
        let lin = [[a[0][0], a[0][1] * sy / sx], [a[1][0] * sx / sy, a[1][1]]];
        // (A − I)(S − I)c
        let (cx, cy) = (0.5 * (sx - 1.0), 0.5 * (sy - 1.0));
        let shift = [(a[0][0] - 1.0) * cx + a[0][1] * cy, a[1][0] * cx + (a[1][1] - 1.0) * cy];
        let tx = (m[(0, 2)] + shift[0]) / sx;
        let ty = (m[(1, 2)] + shift[1]) / sy;
        Self {
            x_axis: glam::Vec4::new(lin[0][0] as f32, lin[1][0] as f32, 0.0, 0.0),
            y_axis: glam::Vec4::new(lin[0][1] as f32, lin[1][1] as f32, 0.0, 0.0),
            z_axis: glam::Vec4::new(tx as f32, ty as f32, 1.0, 0.0),
            apply_srgb: apply_srgb as u32,
            brightness: 1.0,
        }
    }

    /// Identity transform; `apply_srgb` still applies.
    pub fn identity(apply_srgb: bool) -> Self {
        Self::from_transform(&Affine2::identity(), apply_srgb)
    }

    pub fn for_frame(alignment: Option<&FrameAlignment>, apply_srgb: bool) -> Self {
        match alignment {
            Some(a) => Self::from_alignment(a, apply_srgb),
            None => Self::identity(apply_srgb),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The grid transform maps a grid point to where the full-resolution
    /// transform sends the frame point it stands for, on odd ratios too.
    #[test]
    fn on_grid_commutes_with_the_grid_mapping() {
        let (s, c) = 0.03f32.sin_cos();
        let t = Affine2::from_matrix_unchecked(nalgebra::Matrix3::new(
            1.01 * c,
            -s,
            7.25,
            s,
            0.99 * c,
            -3.5,
            0.0,
            0.0,
            1.0,
        ));
        let ratio = [301.0f32 / 150.0, 203.0 / 101.0];
        let p = WarpParams::on_grid(&t, ratio, false);
        let to_full = |g: f32, r: f32| (g + 0.5) * r - 0.5;
        let to_grid = |f: f32, r: f32| (f + 0.5) / r - 0.5;
        for (gx, gy) in [(0.0f32, 0.0f32), (149.0, 100.0), (37.0, 61.0)] {
            let full = t * nalgebra::Point2::new(to_full(gx, ratio[0]), to_full(gy, ratio[1]));
            let want = (to_grid(full.x, ratio[0]), to_grid(full.y, ratio[1]));
            let got = (
                p.x_axis.x * gx + p.y_axis.x * gy + p.z_axis.x,
                p.x_axis.y * gx + p.y_axis.y * gy + p.z_axis.y,
            );
            assert!((got.0 - want.0).abs() < 1e-3 && (got.1 - want.1).abs() < 1e-3, "{got:?}");
        }
        // The identity stays exact, so the aligned path matches the
        // unaligned one bit for bit.
        let id = WarpParams::on_grid(&Affine2::identity(), ratio, false);
        assert_eq!(
            (id.x_axis, id.y_axis, id.z_axis),
            (glam::Vec4::X, glam::Vec4::Y, glam::Vec4::Z)
        );
    }

    /// The fields of `struct name { … }` in `source`, as (name, type) in
    /// order, comments dropped and `pub` stripped.
    fn struct_fields(source: &str, name: &str) -> Vec<(String, String)> {
        let start = source.find(&format!("struct {name} {{")).expect("struct present");
        let body = &source[start..];
        let body = &body[body.find('{').unwrap() + 1..body.find('}').unwrap()];
        body.lines()
            .map(|l| l.split("//").next().unwrap().trim().trim_end_matches(','))
            .filter(|l| !l.is_empty())
            .map(|l| {
                let l = match l.strip_prefix("pub(") {
                    Some(rest) => rest.split_once(") ").expect("pub(…) field").1,
                    None => l.trim_start_matches("pub "),
                };
                let (n, t) = l.split_once(':').expect("name: type");
                (n.trim().to_owned(), t.trim().to_owned())
            })
            .collect()
    }

    /// `rust`, a Rust field type of a uniform, as WGSL writes it.
    fn wgsl_type(rust: &str) -> &'static str {
        match rust {
            "glam::Vec4" | "Vec4" => "vec4f",
            "UVec4" => "vec4u",
            "u32" => "u32",
            "f32" => "f32",
            other => panic!("no WGSL type for {other}"),
        }
    }

    /// Every other uniform mirrored in WGSL matches its Rust struct as
    /// `WarpParams` does: same fields, same order, same types.
    #[test]
    fn shader_uniforms_match_their_rust_structs() {
        let pairs: &[(&str, &str, &[&str])] = &[
            (include_str!("accum.rs"), include_str!("shaders/accum.wgsl"), &["FinishParams"]),
            (include_str!("upload.rs"), include_str!("upload.wgsl"), &["UploadParams"]),
            (
                include_str!("contribution.rs"),
                include_str!("contribution.wgsl"),
                &["ContributionParams"],
            ),
            (
                include_str!("focus.rs"),
                include_str!("shaders/focus.wgsl"),
                &["GrayParams", "BandpassParams"],
            ),
            (include_str!("focus.rs"), include_str!("shaders/metric.wgsl"), &["BlurParams"]),
            (include_str!("map.rs"), include_str!("map.wgsl"), &["MapParams"]),
            (include_str!("scalar.rs"), include_str!("scalar.wgsl"), &["FillParams"]),
            (
                include_str!("pyramid/mod.rs"),
                include_str!("pyramid/pyramid.wgsl"),
                &["LevelParams", "FrameParams"],
            ),
            (
                include_str!("depth/mod.rs"),
                include_str!("depth/depth.wgsl"),
                &[
                    "ConfidenceParams",
                    "DepthWtaParams",
                    "IirParams",
                    "DepthFeatherParams",
                    "BlendParams",
                ],
            ),
            (
                include_str!("dng_render/mod.rs"),
                include_str!("dng_render/dng_render.wgsl"),
                &["DngColorParams"],
            ),
            (
                include_str!("wavelet/mod.rs"),
                include_str!("wavelet/wavelet.wgsl"),
                &[
                    "WaveletDims",
                    "WtaParams",
                    "LiveParams",
                    "LlParams",
                    "LevelDims",
                    "DenoiseParams",
                    "ReassignParams",
                ],
            ),
        ];
        for (rust, wgsl, names) in pairs {
            for name in *names {
                let want: Vec<(String, String)> = struct_fields(rust, name)
                    .into_iter()
                    .map(|(n, t)| (n, wgsl_type(&t).to_owned()))
                    .collect();
                assert!(!want.is_empty(), "{name} has fields");
                assert_eq!(struct_fields(wgsl, name), want, "{name}");
            }
        }
    }

    /// The shader's `WarpParams` (`deptheos::warp`) matches the Rust struct,
    /// and no shader declares its own copy.
    #[test]
    fn shader_warp_params_matches_the_rust_struct() {
        let want: Vec<(String, String)> = struct_fields(include_str!("transform.rs"), "WarpParams")
            .into_iter()
            .map(|(n, t)| (n, wgsl_type(&t).to_owned()))
            .collect();
        assert_eq!(want.len(), 5);
        let shared = include_str!("shaders/warp_common.wgsl");
        assert_eq!(
            struct_fields(shared, "WarpParams"),
            want,
            "WarpParams in shaders/warp_common.wgsl"
        );
        for (path, source) in [
            ("warp.wgsl", include_str!("warp.wgsl")),
            ("pyramid/pyramid.wgsl", include_str!("pyramid/pyramid.wgsl")),
            ("depth/depth.wgsl", include_str!("depth/depth.wgsl")),
            ("wavelet/wavelet.wgsl", include_str!("wavelet/wavelet.wgsl")),
        ] {
            assert!(!source.contains("struct WarpParams"), "{path} declares its own WarpParams");
        }
    }
}
