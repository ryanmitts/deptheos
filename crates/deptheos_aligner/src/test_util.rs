//! Helpers shared by the unit tests: fixtures, transforms, error measures
//! and a scalar reference warp.

use crate::plane::Plane;
use image::DynamicImage;
use nalgebra::{Affine2, Matrix3, Point2};

/// A fixture image resized to `width`×`height` (triangle filter).
pub fn fixture(name: &str, width: u32, height: u32) -> DynamicImage {
    let path = format!("{}/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    image::open(&path).unwrap_or_else(|e| panic!("{path}: {e}")).resize_exact(
        width,
        height,
        image::imageops::FilterType::Triangle,
    )
}

pub fn translation(tx: f32, ty: f32) -> Affine2<f32> {
    Affine2::from_matrix_unchecked(Matrix3::new(1.0, 0.0, tx, 0.0, 1.0, ty, 0.0, 0.0, 1.0))
}

/// Scale + rotation about `(cx, cy)`, as a backward sampling matrix.
pub fn scale_rotate(scale: f32, degrees: f32, cx: f32, cy: f32) -> Affine2<f32> {
    let (sin, cos) = degrees.to_radians().sin_cos();
    let (a, b, c, d) = (scale * cos, -scale * sin, scale * sin, scale * cos);
    // T(c) · R · S · T(−c)
    let tx = cx - (a * cx + b * cy);
    let ty = cy - (c * cx + d * cy);
    Affine2::from_matrix_unchecked(Matrix3::new(a, b, tx, c, d, ty, 0.0, 0.0, 1.0))
}

/// RMS difference of the 2×3 parts of two affine matrices.
pub fn compute_rms(m1: &Affine2<f32>, m2: &Affine2<f32>) -> f64 {
    let (a, b) = (m1.matrix(), m2.matrix());
    let mut sum = 0.0;
    for i in 0..2 {
        for j in 0..3 {
            let d = (a[(i, j)] - b[(i, j)]) as f64;
            sum += d * d;
        }
    }
    (sum / 6.0).sqrt()
}

/// Largest distance between where the two transforms send the frame's
/// corners and center.
pub fn max_corner_error(a: &Affine2<f32>, b: &Affine2<f32>, w: f32, h: f32) -> f32 {
    [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h), (w / 2.0, h / 2.0)]
        .into_iter()
        .map(|(x, y)| {
            let p = Point2::new(x, y);
            (a.transform_point(&p) - b.transform_point(&p)).norm()
        })
        .fold(0.0, f32::max)
}

/// Scalar bilinear backward warp of same-sized planes, and the valid mask
/// (0 outside the source, with every output 0): the reference for
/// [`crate::image_ops::PackedWarp`], operation for operation.
pub fn warp_planes<const N: usize>(
    src: [&Plane; N],
    transform: &Affine2<f32>,
    width: usize,
    height: usize,
) -> ([Plane; N], Plane) {
    let (src_w, src_h) = (src[0].width, src[0].height);
    let mut out: [Plane; N] = std::array::from_fn(|_| Plane::new(width, height));
    let mut valid = Plane::new(width, height);
    let m = transform.matrix();
    for y in 0..height {
        let dy = y as f32;
        let sx_base = m[(0, 1)] * dy + m[(0, 2)];
        let sy_base = m[(1, 1)] * dy + m[(1, 2)];
        for x in 0..width {
            let dx = x as f32;
            let sx = m[(0, 0)] * dx + sx_base;
            let sy = m[(1, 0)] * dx + sy_base;
            if !(sx >= 0.0 && sy >= 0.0 && sx < src_w as f32 && sy < src_h as f32) {
                continue;
            }
            let (x0, y0) = (sx.floor() as usize, sy.floor() as usize);
            let (x1, y1) = ((x0 + 1).min(src_w - 1), (y0 + 1).min(src_h - 1));
            let (fx, fy) = (sx - x0 as f32, sy - y0 as f32);
            let w = [(1.0 - fy) * (1.0 - fx), (1.0 - fy) * fx, fy * (1.0 - fx), fy * fx];
            let taps = [y0 * src_w + x0, y0 * src_w + x1, y1 * src_w + x0, y1 * src_w + x1];
            let i = y * width + x;
            for (plane, s) in out.iter_mut().zip(&src) {
                plane.data[i] = w[0] * s.data[taps[0]]
                    + w[1] * s.data[taps[1]]
                    + w[2] * s.data[taps[2]]
                    + w[3] * s.data[taps[3]];
            }
            valid.data[i] = 1.0;
        }
    }
    (out, valid)
}

/// `src` warped by `transform` to `width`×`height`, zero outside the source.
pub fn warped(src: &Plane, transform: &Affine2<f32>, width: usize, height: usize) -> Plane {
    let ([out], _) = warp_planes([src], transform, width, height);
    out
}
