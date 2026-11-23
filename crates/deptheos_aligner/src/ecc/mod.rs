//! ECC alignment of each frame against its predecessor, coarse to fine
//! (~256 px doubling to ~2048 px), giving up early when a level's
//! correlation is too low to refine.

use crate::FEATURELESS_STD;
use crate::Step;
use crate::error::{AlignmentError, Motion};
use crate::image_ops::resize_plane;
use crate::plane::Plane;
use crate::rect::rescale_affine;
use nalgebra::Affine2;
use tracing::debug;

mod solve;

use solve::{Criteria, EccInput, find_transform_ecc};

/// Configuration for the coarse-to-fine ECC pyramid.
#[derive(Debug, Clone, Copy)]
pub struct EccConfig {
    /// Max dimension of the coarsest level.
    pub coarse_max_dimension: u32,
    /// Max dimension of the finest level.
    pub fine_max_dimension: u32,
    /// Most ECC iterations at the coarsest level.
    pub coarse_max_iters: i32,
    /// Most ECC iterations at each level between the coarsest and finest.
    pub mid_max_iters: i32,
    /// Most ECC iterations at the finest level.
    pub fine_max_iters: i32,
    /// [`Criteria::max_step`], in level pixels.
    pub max_step: f64,
    /// Pre-ECC Gaussian blur at the coarse level (resize already low-passed it).
    pub coarse_blur_sigma: f32,
    /// Pre-ECC Gaussian blur at mid/fine levels.
    pub blur_sigma: f32,
    /// Fail the frame when the final correlation at any level is below
    /// this: a frame in the wrong basin still converges, at a low ρ. Real
    /// stacks reach ρ ≥ 0.93 at every level; wrong answers usually stay
    /// ≤ 0.58 (a mirrored smooth scene can reach 0.67, caught by the
    /// plausibility limits).
    pub min_correlation: f64,
    /// Plausibility limits on the frame-to-frame transform: center shift
    /// as a percentage of width/height, scale (and shear) as a percentage,
    /// rotation in degrees. Real stacks move ≤ 0.5 %, ≤ 1 % and ≤ 0.03° per
    /// frame; a well-correlated wrong optimum on repetitive texture passes
    /// the ρ floor but not these.
    pub max_shift_pct: f64,
    pub max_scale_pct: f64,
    pub max_rotation_deg: f64,
}

impl Default for EccConfig {
    fn default() -> Self {
        Self {
            coarse_max_dimension: 256,
            fine_max_dimension: 2048,
            coarse_max_iters: 25,
            mid_max_iters: 30,
            fine_max_iters: 50,
            max_step: 0.02,
            coarse_blur_sigma: 0.0,
            blur_sigma: 0.8,
            min_correlation: 0.6,
            max_shift_pct: 5.0,
            max_scale_pct: 5.0,
            max_rotation_deg: 5.0,
        }
    }
}

impl EccConfig {
    /// Level resolutions: coarse, doubling, capped at the fine maximum
    /// (or the source size for small images).
    pub(crate) fn level_resolutions(&self, src_max_dimension: u32) -> Vec<u32> {
        let top = self.fine_max_dimension.min(src_max_dimension).max(1);
        // A zero coarse size would never double up to `top`.
        let mut levels = vec![self.coarse_max_dimension.clamp(1, top)];
        while levels.last().unwrap().saturating_mul(2) < top {
            levels.push(levels.last().unwrap().saturating_mul(2));
        }
        if *levels.last().unwrap() != top {
            levels.push(top);
        }
        levels
    }

    /// When ECC stops at `level` (0 the coarsest) of `num_levels`.
    pub(crate) fn criteria_for_level(&self, level: usize, num_levels: usize) -> Criteria {
        let max_iter = if level == 0 {
            self.coarse_max_iters
        } else if level == num_levels - 1 {
            self.fine_max_iters
        } else {
            self.mid_max_iters
        };
        Criteria { max_step: self.max_step, max_iter }
    }

    /// The pre-ECC blur's `σ` at `level` (0 the coarsest; ≤ 0 is none).
    pub(crate) fn blur_for_level(&self, level: usize) -> f32 {
        if level == 0 { self.coarse_blur_sigma } else { self.blur_sigma }
    }

    /// Why a frame-to-frame transform is not plausible camera motion, or
    /// `None` when it is.
    fn implausibility(&self, t: &Affine2<f32>, width: u32, height: u32) -> Option<Motion> {
        let m = t.matrix().cast::<f64>();
        if m.iter().any(|v| !v.is_finite()) {
            return Some(Motion::NotFinite);
        }
        let (a, b, c, d) = (m[(0, 0)], m[(0, 1)], m[(1, 0)], m[(1, 1)]);
        let det = a * d - b * c;
        if det.is_nan() || det <= 0.0 {
            return Some(Motion::Flips);
        }
        // The closest similarity's scale and rotation, and the remainder
        // (shear, anisotropic scale) relative to that scale.
        let scale = det.sqrt();
        let rotation = (c - b).atan2(a + d).to_degrees();
        let remainder = (a - d).abs().max((b + c).abs()) / (2.0 * scale);
        // The center's shift, so a scale about it is not also a shift.
        let (cx, cy) = (width as f64 / 2.0, height as f64 / 2.0);
        let dx = 100.0 * (a * cx + b * cy + m[(0, 2)] - cx).abs() / width as f64;
        let dy = 100.0 * (c * cx + d * cy + m[(1, 2)] - cy).abs() / height as f64;
        let scale_pct = 100.0 * (scale - 1.0).abs();
        let remainder_pct = 100.0 * remainder;
        if [dx, dy, scale_pct, remainder_pct, rotation].iter().any(|v| !v.is_finite()) {
            return Some(Motion::NotFinite);
        }
        if dx > self.max_shift_pct || dy > self.max_shift_pct {
            Some(Motion::Shift { dx, dy, max: self.max_shift_pct })
        } else if scale_pct > self.max_scale_pct {
            Some(Motion::Scale { percent: scale_pct, max: self.max_scale_pct })
        } else if remainder_pct > self.max_scale_pct {
            Some(Motion::Shear { percent: remainder_pct, max: self.max_scale_pct })
        } else if rotation.abs() > self.max_rotation_deg {
            Some(Motion::Rotation { degrees: rotation.abs(), max: self.max_rotation_deg })
        } else {
            None
        }
    }
}

/// One frame's ECC inputs at every level of the schedule, coarse to fine
/// ([`EccAligner::prepare`]).
pub(crate) struct EccLevels(pub(crate) Vec<EccInput>);

/// ECC against the previous frame, coarse to fine; [`crate::Aligner`]
/// keeps the previous frame's levels as the templates.
pub(crate) struct EccAligner {
    config: EccConfig,
}

impl EccAligner {
    pub fn new() -> Self {
        Self { config: EccConfig::default() }
    }

    /// A frame resized to every level, blurred, with its gradients. Depends
    /// on the frame alone, so it runs ahead of the chain on any thread.
    ///
    /// Only the finest level comes from the full frame (`finest`); each
    /// coarser one is resampled from the level above, which saves reading
    /// the whole frame per level.
    pub fn prepare(
        config: &EccConfig,
        width: usize,
        height: usize,
        finest: impl FnOnce(usize, usize) -> Result<Plane, AlignmentError>,
    ) -> Result<EccLevels, AlignmentError> {
        let src_max = width.max(height) as u32;
        let sizes: Vec<(usize, usize)> = config
            .level_resolutions(src_max)
            .into_iter()
            .map(|resolution| {
                let scale = (resolution as f64 / src_max as f64).min(1.0);
                // An extreme aspect ratio's minor axis would round to 0.
                let w = ((width as f64 * scale).round() as usize).max(1);
                let h = ((height as f64 * scale).round() as usize).max(1);
                (w, h)
            })
            .collect();
        let mut levels = Vec::with_capacity(sizes.len());
        let (&(w, h), coarser) = sizes.split_last().expect("at least one level");
        let mut source = finest(w, h)?;
        levels.push(EccInput::new(&source, config.blur_for_level(coarser.len())));
        for (i, &(w, h)) in coarser.iter().enumerate().rev() {
            source = resize_plane(&source, w, h)?;
            levels.push(EccInput::new(&source, config.blur_for_level(i)));
        }
        levels.reverse();
        Ok(EccLevels(levels))
    }

    /// The schedule and limits, which its preparer needs too.
    pub fn config(&self) -> &EccConfig {
        &self.config
    }

    /// Frame 0, whose levels become the first templates. A frame with no
    /// contrast is an error, so the next frame is tried as frame 0: ECC's
    /// correlation is 0/0 against a flat template.
    pub fn first(&self, frame: &EccLevels) -> Result<(), AlignmentError> {
        let finest = &frame.0.last().expect("at least one level").blurred;
        let std = finest.std();
        if std < FEATURELESS_STD {
            return Err(AlignmentError::Featureless { std });
        }
        debug!("ECC: {} levels", frame.0.len());
        Ok(())
    }

    /// The transform from the previous frame (`template`) to this one, in
    /// full-resolution pixels.
    pub fn step(
        &self,
        template: &EccLevels,
        frame: &EccLevels,
        width: u32,
        height: u32,
    ) -> Result<Step, AlignmentError> {
        let (transform, correlations) = pairwise(&self.config, template, frame, width, height)?;
        debug!(
            "ECC: step translation=({:.2}, {:.2}), ρ by level {}",
            transform[(0, 2)],
            transform[(1, 2)],
            format_correlations(&correlations)
        );
        Ok(Step { transform, brightness: 1.0 })
    }
}

/// Coarse-to-fine ECC of `frame` against `template`, from the identity.
fn pairwise(
    config: &EccConfig,
    template: &EccLevels,
    frame: &EccLevels,
    width: u32,
    height: u32,
) -> PairResult {
    let num_levels = template.0.len();
    if frame.0.len() != num_levels {
        return Err(AlignmentError::ProcessingFailed(format!(
            "frame prepared at {} levels, the chain has {num_levels}",
            frame.0.len()
        )));
    }
    let mut transform = Affine2::identity();
    let mut correlations = Vec::with_capacity(num_levels);

    for i in 0..num_levels {
        let (input, level) = (&frame.0[i], &template.0[i]);
        let criteria = config.criteria_for_level(i, num_levels);
        let (level_w, level_h) = (level.blurred.width, level.blurred.height);

        // Seed with the coarser result, carried to this level's grid.
        let (sx, sy) = (level_w as f64 / width as f64, level_h as f64 / height as f64);
        let seeded = rescale_affine(&transform, sx, sy);

        let started = std::time::Instant::now();
        let result = find_transform_ecc(&level.blurred, input, Some(&seeded), criteria)?;

        debug!(
            "ECC level {i} ({level_w}x{level_h}): correlation {:.4}, converged {}, {} iterations, {:.2} ms",
            result.correlation,
            result.converged,
            result.iterations,
            started.elapsed().as_secs_f64() * 1e3
        );
        correlations.push(result.correlation);

        // Fail fast rather than polish a bad solution at finer levels.
        if result.correlation < config.min_correlation {
            return Err(AlignmentError::LowCorrelation {
                correlation: result.correlation,
                min: config.min_correlation,
                level: (level_w as u32, level_h as u32),
                by_level: format_correlations(&correlations),
            });
        }

        // Back to full-resolution coordinates.
        transform = rescale_affine(&result.transform, 1.0 / sx, 1.0 / sy);
    }

    if let Some(motion) = config.implausibility(&transform, width, height) {
        return Err(AlignmentError::Implausible {
            motion,
            by_level: format_correlations(&correlations),
        });
    }

    Ok((transform, correlations))
}

/// A pair's transform and the final correlation at each level run.
type PairResult = Result<(Affine2<f32>, Vec<f64>), AlignmentError>;

fn format_correlations(correlations: &[f64]) -> String {
    let parts: Vec<String> = correlations.iter().map(|r| format!("{r:.3}")).collect();
    format!("[{}]", parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_ops::image_to_grayscale;
    use crate::test_util::{
        compute_rms, fixture, max_corner_error, scale_rotate, translation, warped,
    };
    use crate::{AlignMode, Aligner};

    const MAX_RMS: f64 = 0.25;

    /// Large enough for several levels and proportionally small borders.
    fn spider_gray() -> (Plane, u32, u32) {
        let gray = image_to_grayscale(&fixture("spider.jpg", 1600, 1200)).unwrap();
        (gray, 1600, 1200)
    }

    #[test]
    fn first_image_is_identity() {
        let (gray, w, h) = spider_gray();
        let mut chain = Aligner::new(AlignMode::Ecc);
        let transform = chain.add_gray(&gray).map(|f| f.transform).expect("first image");

        assert_eq!(compute_rms(&transform, &Affine2::identity()), 0.0);
        let area = chain.crop().unwrap();
        assert_eq!((area.width, area.height), (w, h));
    }

    /// A flat frame 0 is an error that leaves the chain empty, so the next
    /// frame becomes frame 0 and the chain aligns from there.
    #[test]
    fn flat_first_frame_is_rejected_and_the_next_becomes_frame_0() {
        let (gray, w, h) = spider_gray();
        let (wu, hu) = (w as usize, h as usize);
        let mut chain = Aligner::new(AlignMode::Ecc);
        for v in [0.0, 0.5] {
            let flat = Plane::from_vec(wu, hu, vec![v; wu * hu]);
            let err = chain.add_gray(&flat).expect_err("flat frame 0");
            assert!(matches!(err, AlignmentError::Featureless { .. }), "{err}");
        }
        let first = chain.add_gray(&gray).expect("frame 0").transform;
        assert_eq!(compute_rms(&first, &Affine2::identity()), 0.0);

        let shifted = warped(&gray, &translation(15.0, 12.0), wu, hu);
        let r = chain.add_gray(&shifted).expect("frame 1").transform;
        let rms = compute_rms(&r, &translation(-15.0, -12.0));
        assert!(rms < MAX_RMS, "frame 1: RMS {rms:.4} exceeds {MAX_RMS}");
    }

    #[test]
    fn translation_chain() {
        let (gray, w, h) = spider_gray();
        let (w, h) = (w as usize, h as usize);

        // Sampled at +t, so the reference → frame map is −t.
        let warped1 = warped(&gray, &translation(15.0, 12.0), w, h);
        let warped2 = warped(&gray, &translation(25.0, 30.0), w, h);
        let expected1 = translation(-15.0, -12.0);
        let expected2 = translation(-25.0, -30.0);

        let mut chain = Aligner::new(AlignMode::Ecc);
        chain.add_gray(&gray).map(|f| f.transform).expect("frame 0");

        let r1 = chain.add_gray(&warped1).map(|f| f.transform).expect("frame 1");
        let rms1 = compute_rms(&r1, &expected1);
        assert!(rms1 < MAX_RMS, "frame 1: RMS {rms1:.4} exceeds {MAX_RMS}");

        let r2 = chain.add_gray(&warped2).map(|f| f.transform).expect("frame 2");
        let rms2 = compute_rms(&r2, &expected2);
        assert!(rms2 < MAX_RMS, "frame 2: RMS {rms2:.4} exceeds {MAX_RMS}");
    }

    /// Warp an oversize source and take the central `w`×`h` window, so no
    /// border; returns the frame and the warp in window coordinates.
    fn warp_and_crop(
        src: &Plane,
        warp: &Affine2<f32>,
        w: usize,
        h: usize,
    ) -> (Plane, Affine2<f32>) {
        let (src_h, src_w) = (src.height, src.width);
        let (ox, oy) = ((src_w - w) / 2, (src_h - h) / 2);
        let frame = warped(src, warp, src_w, src_h).crop(ox, oy, w, h);
        let shift =
            nalgebra::Matrix3::new_translation(&nalgebra::Vector2::new(ox as f32, oy as f32));
        let unshift =
            nalgebra::Matrix3::new_translation(&nalgebra::Vector2::new(-(ox as f32), -(oy as f32)));
        let local = Affine2::from_matrix_unchecked(unshift * warp.matrix() * shift);
        (frame, local)
    }

    /// A scale/rotation frame then a shifted one checks the composition
    /// order, since only translations commute.
    #[test]
    fn non_commuting_chain() {
        let big_gray = image_to_grayscale(&fixture("spider.jpg", 2000, 1500)).unwrap();
        let (w, h) = (1600usize, 1200usize);
        let (cx, cy) = (1000.0, 750.0);

        let (reference, _) = warp_and_crop(&big_gray, &Affine2::identity(), w, h);
        let (frame1, warp1) = warp_and_crop(&big_gray, &scale_rotate(1.03, 3.0, cx, cy), w, h);
        let (frame2, warp2) = warp_and_crop(&big_gray, &translation(20.0, 25.0), w, h);

        // frame_k(x) = ref(W_k x), so the reference → frame_k map is W_k⁻¹.
        let expected1 = warp1.try_inverse().unwrap();
        let expected2 = warp2.try_inverse().unwrap();

        let mut chain = Aligner::new(AlignMode::Ecc);
        chain.add_gray(&reference).map(|f| f.transform).expect("frame 0");

        let r1 = chain.add_gray(&frame1).map(|f| f.transform).expect("frame 1");
        let rms1 = compute_rms(&r1, &expected1);
        assert!(rms1 < MAX_RMS, "frame 1: RMS {rms1:.4} exceeds {MAX_RMS}\n{:?}", r1.matrix());

        let r2 = chain.add_gray(&frame2).map(|f| f.transform).expect("frame 2");
        let rms2 = compute_rms(&r2, &expected2);
        assert!(rms2 < MAX_RMS, "frame 2: RMS {rms2:.4} exceeds {MAX_RMS}\n{:?}", r2.matrix());
    }

    /// Large single-step motions the coarse level must carry most of the
    /// way (a test on ρ instead of the step leaves the last 76 px off).
    #[test]
    fn large_motion_converges() {
        let big_gray = image_to_grayscale(&fixture("spider.jpg", 2400, 1800)).unwrap();
        let (w, h) = (1600usize, 1200usize);
        let (cx, cy) = (1200.0, 900.0);
        let (reference, _) = warp_and_crop(&big_gray, &Affine2::identity(), w, h);
        for (scale, degrees, tx, ty) in
            [(1.02, 1.0, 40.0, -30.0), (0.97, -2.0, -55.0, 35.0), (1.04, 3.0, 20.0, 60.0)]
        {
            let motion = translation(tx, ty) * scale_rotate(scale, degrees, cx, cy);
            let (frame, warp) = warp_and_crop(&big_gray, &motion, w, h);
            let mut chain = Aligner::new(AlignMode::Ecc);
            chain.add_gray(&reference).unwrap();
            let r = chain.add_gray(&frame).expect("frame 1");
            let expected = warp.try_inverse().unwrap();
            let e = max_corner_error(&r.transform, &expected, w as f32, h as f32);
            assert!(e < 0.05, "{scale} {degrees}° ({tx}, {ty}): corner error {e:.3} px");
        }
    }

    /// Focus breathing: each frame magnified 0.5 % more, with jitter. The
    /// chain must stay on the truth (counting the zero border drifts ~4×).
    #[test]
    fn breathing_chain_does_not_drift() {
        let src = image_to_grayscale(&fixture("spider.jpg", 2000, 1500)).unwrap();
        let (w, h) = (1400usize, 1050usize);
        let jitter = [(0.0, 0.0), (1.2, -0.7), (-0.9, 1.4), (0.4, 0.8), (-1.3, -0.2), (0.7, -1.1)];
        // S_k: frame-k pixel → source pixel.
        let sampling = |k: usize| -> nalgebra::Matrix3<f64> {
            let m = 1.005f64.powi(k as i32);
            let (jx, jy) = jitter[k % jitter.len()];
            nalgebra::Matrix3::new(
                1.0 / m,
                0.0,
                1000.0 + jx - 700.0 / m,
                0.0,
                1.0 / m,
                750.0 + jy - 525.0 / m,
                0.0,
                0.0,
                1.0,
            )
        };
        let mut chain = Aligner::new(AlignMode::Ecc);
        let mut error = 0.0f64;
        for k in 0..24 {
            let s_k = Affine2::from_matrix_unchecked(sampling(k).cast::<f32>());
            let frame = warped(&src, &s_k, w, h);
            let r = chain.add_gray(&frame).map(|f| f.transform).expect("frame");
            // Truth: reference pixel → frame-k pixel = S_k⁻¹ · S_0.
            let truth = sampling(k).try_inverse().unwrap() * sampling(0);
            let got = r.matrix().cast::<f64>();
            for (x, y) in [(0.0, 0.0), (w as f64, 0.0), (0.0, h as f64), (w as f64, h as f64)] {
                let p = nalgebra::Vector3::new(x, y, 1.0);
                error = error.max(((got * p) - (truth * p)).xy().norm());
            }
        }
        assert!(error < 0.2, "chained corner error {error:.3} px after 24 frames");
    }

    /// A shift past the coarse level's capture range is an error, not a
    /// wrong transform.
    #[test]
    fn shift_beyond_basin_is_an_error() {
        let (gray, w, h) = spider_gray();
        for shift in [100.0, 200.0] {
            let frame = warped(&gray, &translation(shift, shift * 0.6), w as usize, h as usize);
            let mut chain = Aligner::new(AlignMode::Ecc);
            chain.add_gray(&gray).map(|f| f.transform).unwrap();
            let result = chain.add_gray(&frame).map(|f| f.transform);
            assert!(result.is_err(), "shift {shift}: wrong transform accepted: {result:?}");
        }
    }

    /// A frame no affine maps onto the reference (rotated 180°) is an error.
    #[test]
    fn uncorrelated_frame_is_an_error() {
        let img = fixture("spider.jpg", 1600, 1200);
        let gray = image_to_grayscale(&img).unwrap();
        let turned = image_to_grayscale(&img.rotate180()).unwrap();

        let mut chain = Aligner::new(AlignMode::Ecc);
        chain.add_gray(&gray).map(|f| f.transform).unwrap();
        let result = chain.add_gray(&turned).map(|f| f.transform);
        assert!(result.is_err(), "uncorrelated frame accepted: {result:?}");
    }

    /// A failed frame leaves the templates and the cumulative transform
    /// alone, so the next frame aligns against the last good one.
    #[test]
    fn failed_frame_does_not_become_the_template() {
        let (gray, w, h) = spider_gray();
        let far = warped(&gray, &translation(200.0, 120.0), w as usize, h as usize);
        let near = warped(&gray, &translation(15.0, 12.0), w as usize, h as usize);

        let mut chain = Aligner::new(AlignMode::Ecc);
        chain.add_gray(&gray).map(|f| f.transform).unwrap();
        assert!(chain.add_gray(&far).map(|f| f.transform).is_err());
        let r = chain.add_gray(&near).map(|f| f.transform).expect("frame after a failure");
        let rms = compute_rms(&r, &translation(-15.0, -12.0));
        assert!(rms < MAX_RMS, "RMS {rms:.4} exceeds {MAX_RMS}");
    }

    #[test]
    fn implausible_motion_is_named() {
        let config = EccConfig::default();
        let (w, h) = (1600, 1200);
        let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
        // About the center, so it is not also a shift.
        let shear = Affine2::from_matrix_unchecked(nalgebra::Matrix3::new(
            1.0,
            0.15,
            -0.15 * cy,
            0.0,
            1.0,
            0.0,
            0.0,
            0.0,
            1.0,
        ));
        let flip = Affine2::from_matrix_unchecked(nalgebra::Matrix3::new(
            -1.0, 0.0, 1600.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0,
        ));
        let nan = Affine2::from_matrix_unchecked(nalgebra::Matrix3::new(
            1.0,
            0.0,
            f32::NAN,
            0.0,
            1.0,
            0.0,
            0.0,
            0.0,
            1.0,
        ));
        let cases = [
            (translation(100.0, 0.0), "shift"),
            (translation(0.0, 70.0), "shift"),
            (scale_rotate(1.08, 0.0, cx, cy), "scale"),
            (scale_rotate(1.0, 8.0, cx, cy), "rotation"),
            (shear, "shear"),
            (flip, "flips"),
            (nan, "finite"),
        ];
        for (t, word) in cases {
            let reason = config.implausibility(&t, w, h).expect(word).to_string();
            assert!(reason.contains(word), "{word}: {reason}");
        }
        // A scale about the center must not count as a shift.
        let ok = [Affine2::identity(), translation(40.0, -30.0), scale_rotate(1.03, 3.0, cx, cy)];
        for t in ok {
            assert_eq!(config.implausibility(&t, w, h), None, "{:?}", t.matrix());
        }
    }

    #[test]
    fn valid_area_shrinks() {
        let (gray, w, h) = spider_gray();
        let frame = warped(&gray, &translation(15.0, 10.0), w as usize, h as usize);

        let mut chain = Aligner::new(AlignMode::Ecc);
        chain.add_gray(&gray).map(|f| f.transform).unwrap();
        chain.add_gray(&frame).map(|f| f.transform).unwrap();

        let area = chain.crop().unwrap();
        assert!(area.width < w, "valid width {} should be < {}", area.width, w);
        assert!(area.height < h, "valid height {} should be < {}", area.height, h);
    }
}
