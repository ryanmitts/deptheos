//! Separable Gaussian blur, ECC's pre-blur.
//!
//! Both passes sum a tap at a time across pixels, which lets LLVM
//! vectorize them; each pixel still adds its taps in order, so every
//! instruction set gives the same values.

use super::rows::{add_weighted_row, par_rows};
use crate::plane::Plane;

/// Separable Gaussian blur, `2·ceil(3σ) + 1` sampled taps normalized to 1,
/// edges clamped.
pub(crate) fn gaussian_blur(src: &Plane, sigma: f32) -> Plane {
    let (w, h) = (src.width, src.height);
    let r = (3.0 * sigma).ceil() as usize;
    let mut kernel: Vec<f32> = (0..=2 * r)
        .map(|i| {
            let d = i as f32 - r as f32;
            (-d * d / (2.0 * sigma * sigma)).exp()
        })
        .collect();
    let total: f32 = kernel.iter().sum();
    kernel.iter_mut().for_each(|k| *k /= total);
    if w == 0 || h == 0 {
        return src.clone();
    }

    // Horizontal: no index is clamped over the interior `r..w − r`, so the
    // sum goes a tap at a time across it.
    let clamped = |row: &[f32], x: usize| -> f32 {
        kernel.iter().enumerate().map(|(i, k)| k * row[(x + i).saturating_sub(r).min(w - 1)]).sum()
    };
    let mut tmp = Plane::new(w, h);
    par_rows(
        &mut tmp.data,
        w,
        #[inline(always)]
        |y, out| {
            let row = src.row(y);
            if w > 2 * r {
                let n = w - 2 * r;
                for (i, &k) in kernel.iter().enumerate() {
                    add_weighted_row(&mut out[r..r + n], &row[i..i + n], k);
                }
                for x in (0..r).chain(w - r..w) {
                    out[x] = clamped(row, x);
                }
            } else {
                for (x, o) in out.iter_mut().enumerate() {
                    *o = clamped(row, x);
                }
            }
        },
    );

    // Vertical: a whole row per tap, row index clamped.
    let mut out = Plane::new(w, h);
    par_rows(
        &mut out.data,
        w,
        #[inline(always)]
        |y, out_row| {
            for (i, &k) in kernel.iter().enumerate() {
                add_weighted_row(out_row, tmp.row((y + i).saturating_sub(r).min(h - 1)), k);
            }
        },
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The blur keeps a constant image constant (edges included) and
    /// spreads an impulse symmetrically without losing mass.
    #[test]
    fn blur_preserves_mass_and_symmetry() {
        let flat = gaussian_blur(&Plane::from_vec(9, 7, vec![0.3; 63]), 0.8);
        assert!(flat.data.iter().all(|v| (v - 0.3).abs() < 1e-6));
        let mut impulse = Plane::new(21, 21);
        impulse.data[10 * 21 + 10] = 1.0;
        let b = gaussian_blur(&impulse, 0.8);
        assert!((b.data.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        assert!(
            (b.at(7, 10) - b.at(13, 10)).abs() < 1e-7 && (b.at(10, 8) - b.at(10, 12)).abs() < 1e-7
        );
        assert_eq!(b.at(6, 10), 0.0, "7 taps at σ = 0.8");
    }
}
