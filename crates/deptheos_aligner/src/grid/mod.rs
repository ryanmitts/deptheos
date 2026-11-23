//! Grid-search alignment.
//!
//! Each frame is aligned against its predecessor with a similarity
//! transform (shift, rotation, isotropic magnification) found by exhaustive
//! search over a shrinking grid, coarse-to-fine down a box pyramid of a
//! half-resolution gray map. Each round scores at most
//! `MAX_STEPS_PER_AXIS^4` candidates by the standard deviation of the pixel
//! difference, which ignores an exposure offset; the frame is prescaled by
//! the mean brightness ratio so a gain change costs nothing either.
//!
//! The search always finds a best candidate, so the result is judged
//! afterwards: a best difference not well below what unrelated images give
//! ([`MAX_MATCH_RATIO`]) is an error, and the next frame aligns against the
//! last good one. After a featureless frame there is nothing to judge
//! against, and the search's result is kept.
//!
//! Runs on the CPU with rayon over candidates, leaving the GPU to stack.

use crate::error::AlignmentError;
use crate::image_ops::GrayRows;
use crate::plane::Plane;
use crate::rect::rescale_affine;
use crate::{FEATURELESS_STD, Step};
use nalgebra::{Affine2, Matrix3};
use rayon::prelude::*;
use tracing::debug;

mod score;

use score::{Match, difference_scores, match_quality, score_task_size};

/// The grid aligner's search limits and sampling. Rotation is on by
/// default: a focus-rail stack rolls a little.
#[derive(Clone, Copy, Debug)]
pub struct GridConfig {
    /// Horizontal shift limit, percent of the frame's longer side.
    pub horizontal_pct: f32,
    /// Vertical shift limit, percent of the frame's longer side.
    pub vertical_pct: f32,
    /// Rotation limit in degrees; 0 disables the rotation axis.
    pub rotation_deg: f32,
    /// Magnification limit in percent; 0 disables the scale axis.
    pub magnification_pct: f32,
    /// Report the brightness ratio as the frame's factor, so the stackers
    /// scale the frame to frame 0's exposure. Off by default: on a
    /// constant-exposure stack the ratio measures vignetting and
    /// focus-dependent brightness, and applying it shifts the whole blend.
    pub apply_brightness: bool,
    /// Approximate number of sample positions for the difference metric.
    pub sample_points: usize,
}

impl Default for GridConfig {
    fn default() -> Self {
        Self {
            horizontal_pct: 5.0,
            vertical_pct: 5.0,
            rotation_deg: 1.0,
            magnification_pct: 3.0,
            apply_brightness: false,
            sample_points: 10_000,
        }
    }
}

impl GridConfig {
    /// The ranges the app offers for the limits, inclusive, and its steps.
    /// Both shift limits share one range.
    pub const SHIFT_PCT_RANGE: (f32, f32) = (0.0, 25.0);
    pub const SHIFT_PCT_STEP: f32 = 0.5;
    pub const ROTATION_DEG_RANGE: (f32, f32) = (0.0, 10.0);
    pub const ROTATION_DEG_STEP: f32 = 0.1;
    pub const MAGNIFICATION_PCT_RANGE: (f32, f32) = (0.0, 15.0);
    pub const MAGNIFICATION_PCT_STEP: f32 = 0.5;

    fn is_disabled(&self) -> bool {
        self.horizontal_pct == 0.0
            && self.vertical_pct == 0.0
            && self.rotation_deg == 0.0
            && self.magnification_pct == 0.0
    }
}

/// Scale of the gray map the search runs on, relative to the frame.
pub(crate) const MAP_SCALE: f32 = 0.5;
/// Widest map; a wider frame gets a smaller map scale.
pub(crate) const MAP_MAX_DIMENSION: f32 = 6000.0;
/// Smallest pyramid layer side; the search starts at the coarsest layer.
pub(crate) const MIN_LAYER_SIZE: usize = 32;
/// Where the samples are taken: this rectangle of the reference map
/// (relative `x0, y0, x1, y1`), carried into the frame.
pub(crate) const DETECTION_RECT: (f32, f32, f32, f32) = (0.1, 0.1, 0.9, 0.9);
/// At most this many grid positions per axis per round (odd, so the
/// current point is one of them).
pub(crate) const MAX_STEPS_PER_AXIS: usize = 7;
/// Finest grid spacing per round on the map layer (layer 0), in layer
/// pixels at the layer's edge.
pub(crate) const FINEST_LAYER_GRANULARITY: f32 = 0.2;
/// The same on every coarser layer, which only seeds the next.
pub(crate) const COARSE_LAYER_GRANULARITY: f32 = 0.5;
/// Positions the brightness ratio averages over, about.
const BRIGHTNESS_SAMPLES: f32 = 240_000.0;
/// The largest exposure change one step may correct, either way (two stops).
const MAX_BRIGHTNESS_RATIO: f32 = 4.0;
/// A frame is rejected when its best difference score is above this
/// fraction of what two unrelated images would score. Measured: neighbors
/// in real stacks score up to 0.05, frames 25 apart 0.23, a neighbor under
/// a 30 px blur 0.28; black, gray and noise frames 0.99–1.0.
pub(crate) const MAX_MATCH_RATIO: f32 = 0.5;
/// Guard for the per-layer loop, which normally ends in a handful of rounds.
pub(crate) const MAX_ROUNDS_PER_LAYER: usize = 64;

/// A similarity transform about a pair of centers:
/// `F(p) = dst + m·R(−rotation)·(p − src)` maps reference coordinates to
/// coordinates in the frame; centers are in the pixels of whichever layer
/// the params currently describe.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Similarity {
    pub src_cx: f32,
    pub src_cy: f32,
    pub dst_cx: f32,
    pub dst_cy: f32,
    /// Degrees.
    pub rotation: f32,
    pub magnification: f32,
}

impl Similarity {
    fn identity(width: usize, height: usize) -> Self {
        let cx = width as f32 / 2.0;
        let cy = height as f32 / 2.0;
        Self { src_cx: cx, src_cy: cy, dst_cx: cx, dst_cy: cy, rotation: 0.0, magnification: 1.0 }
    }

    /// The same transform on a grid scaled by `s`: only the centers scale.
    /// This drops the box pyramid's `(s − 1)/2` center term, which only
    /// seeds the next layer and does not change the result.
    fn scaled(&self, s: f32) -> Self {
        Self {
            src_cx: self.src_cx * s,
            src_cy: self.src_cy * s,
            dst_cx: self.dst_cx * s,
            dst_cy: self.dst_cy * s,
            ..*self
        }
    }

    /// `(sin, cos)` of the internal angle `−rotation` in radians.
    /// From `libm`, not `f32::sin_cos`, so every platform gets the same last
    /// bit and searches identically.
    fn sin_cos(&self) -> (f32, f32) {
        let x = -self.rotation.to_radians();
        (libm::sinf(x), libm::cosf(x))
    }

    /// `1 / magnification`, kept finite for a zero magnification.
    fn inv_magnification(&self) -> f32 {
        if self.magnification != 0.0 { 1.0 / self.magnification } else { 1e6 }
    }

    /// Reference → frame.
    fn forward(&self, x: f32, y: f32) -> (f32, f32) {
        let (sn, cs) = self.sin_cos();
        let m = self.magnification;
        let dx = x - self.src_cx;
        let dy = y - self.src_cy;
        (dx * cs * m - dy * sn * m + self.dst_cx, dx * sn * m + dy * cs * m + self.dst_cy)
    }

    /// Frame → reference.
    #[inline]
    fn back(&self, x: f32, y: f32, sn: f32, cs: f32, inv_m: f32) -> (f32, f32) {
        let dx = x - self.dst_cx;
        let dy = y - self.dst_cy;
        ((dx * cs + dy * sn) * inv_m + self.src_cx, (-dx * sn + dy * cs) * inv_m + self.src_cy)
    }

    fn to_affine(self) -> Affine2<f32> {
        let (sn, cs) = self.sin_cos();
        let m = self.magnification;
        let (a, b, c, d) = (cs * m, -sn * m, sn * m, cs * m);
        let tx = self.dst_cx - (a * self.src_cx + b * self.src_cy);
        let ty = self.dst_cy - (c * self.src_cx + d * self.src_cy);
        Affine2::from_matrix_unchecked(Matrix3::new(a, b, tx, c, d, ty, 0.0, 0.0, 1.0))
    }
}

/// Pyramid layers are gray planes in `[0, 1]`.
impl Plane {
    /// A 2×2 box average to `ceil(dim/2)`. The right edge drops the
    /// missing column and divides by 2; the bottom edge re-reads the last
    /// row so the divisor stays 4.
    fn box_down(&self) -> Plane {
        let width = self.width.div_ceil(2);
        let height = self.height.div_ceil(2);
        let mut data = vec![0.0f32; width * height];
        data.par_chunks_mut(width).enumerate().for_each(|(y, row)| {
            let sy = 2 * y;
            let sy2 = (sy + 1).min(self.height - 1);
            for (x, out) in row.iter_mut().enumerate() {
                let sx = 2 * x;
                let sx2 = sx + 1;
                *out = if sx2 >= self.width {
                    (self.at(sx, sy) + self.at(sx, sy2)) * 0.5
                } else {
                    (self.at(sx, sy) + self.at(sx2, sy) + self.at(sx, sy2) + self.at(sx2, sy2))
                        * 0.25
                };
            }
        });
        Plane { width, height, data }
    }

    /// Bilinear read; `None` outside `[0, w−1] × [0, h−1]`.
    #[inline]
    fn bilinear(&self, x: f32, y: f32) -> Option<f32> {
        let max_x = (self.width - 1) as f32;
        let max_y = (self.height - 1) as f32;
        if !(x >= 0.0 && y >= 0.0 && x <= max_x && y <= max_y) {
            return None;
        }
        let x0 = x as usize;
        let y0 = y as usize;
        let fx = x - x0 as f32;
        let fy = y - y0 as f32;
        let x1 = (x0 + 1).min(self.width - 1);
        let y1 = (y0 + 1).min(self.height - 1);
        let top = self.at(x0, y0) + (self.at(x1, y0) - self.at(x0, y0)) * fx;
        let bottom = self.at(x0, y1) + (self.at(x1, y1) - self.at(x0, y1)) * fx;
        Some(top + (bottom - top) * fy)
    }
}

/// Box pyramid of a gray map, layer 0 the map itself, each further layer
/// `ceil(dim/2)` until a side would drop below `min_layer_size`.
pub(crate) struct Pyramid {
    pub(crate) layers: Vec<Plane>,
}

impl Pyramid {
    const MAX_LAYERS: usize = 32;

    fn build(base: Plane, min_layer_size: usize) -> Self {
        let mut layers = vec![base];
        while layers.len() < Self::MAX_LAYERS {
            let last = layers.last().unwrap();
            let next = (last.width.div_ceil(2), last.height.div_ceil(2));
            if next.0 < min_layer_size || next.1 < min_layer_size {
                break;
            }
            let down = last.box_down();
            layers.push(down);
        }
        Self { layers }
    }
}

/// Integer half-open rectangle `[x0, x1) × [y0, y1)`.
#[derive(Clone, Copy, Debug)]
struct IRect {
    x0: i64,
    y0: i64,
    x1: i64,
    y1: i64,
}

impl IRect {
    fn clamped(&self, width: usize, height: usize) -> (usize, usize, usize, usize) {
        let x0 = self.x0.clamp(0, width as i64) as usize;
        let y0 = self.y0.clamp(0, height as i64) as usize;
        let x1 = self.x1.clamp(x0 as i64, width as i64) as usize;
        let y1 = self.y1.clamp(y0 as i64, height as i64) as usize;
        (x0, y0, x1, y1)
    }

    /// `rect · s`, truncated.
    fn scaled(&self, s: f32) -> Self {
        Self {
            x0: (self.x0 as f32 * s) as i64,
            y0: (self.y0 as f32 * s) as i64,
            x1: (self.x1 as f32 * s) as i64,
            y1: (self.y1 as f32 * s) as i64,
        }
    }
}

/// Axis order of the search dimensions.
const SHIFT_X: usize = 0;
const SHIFT_Y: usize = 1;
const SCALE: usize = 2;
const ROTATION: usize = 3;

/// Grid-search aligner; [`crate::Aligner`] keeps the reference pyramid.
pub(crate) struct GridAligner {
    config: GridConfig,
}

impl GridAligner {
    pub fn new(config: GridConfig) -> Self {
        Self { config }
    }

    /// [`MAP_SCALE`], capped so a huge frame's map stays bounded.
    pub(crate) fn map_scale_for(width: usize) -> f32 {
        MAP_SCALE.min(MAP_MAX_DIMENSION / width.max(1) as f32)
    }

    /// A frame's pyramid. Depends on the frame alone, so it runs ahead of
    /// the search on any thread.
    pub fn prepare(gray: GrayRows<'_>) -> Result<Pyramid, AlignmentError> {
        let map_scale = Self::map_scale_for(gray.size().0);
        let map = crate::image_ops::resize_keys_scaled(gray, map_scale)?;
        Ok(Pyramid::build(map, MIN_LAYER_SIZE))
    }

    /// Frame 0, which becomes the first reference. A frame with no contrast
    /// is an error, so the next frame is tried as frame 0: a black
    /// reference would leave nothing to judge against and skew every
    /// brightness ratio.
    pub fn first(&self, pyramid: &Pyramid) -> Result<(), AlignmentError> {
        let std = pyramid.layers[0].std();
        if std < FEATURELESS_STD {
            return Err(AlignmentError::Featureless { std });
        }
        let (mw, mh) = (pyramid.layers[0].width, pyramid.layers[0].height);
        debug!("GridAligner: map {mw}x{mh}, {} layers", pyramid.layers.len());
        Ok(())
    }

    /// This frame's step from the previous frame (`reference`). `sequence`
    /// is frame 0 → previous frame in full-resolution pixels, which places
    /// the samples. A frame that does not match is an error.
    pub fn step(
        &self,
        reference: &Pyramid,
        pyramid: &Pyramid,
        sequence: &Affine2<f32>,
        width: u32,
    ) -> Result<Step, AlignmentError> {
        let (step, quality) = self.step_between(reference, pyramid, sequence, width);
        match quality {
            Some(q) => {
                if let Some(e) = q.rejection() {
                    return Err(e);
                }
            }
            // No sample landed on the previous frame; a disabled search has
            // no verdict by design.
            None if !self.config.is_disabled() => {
                return Err(AlignmentError::NoOverlap);
            }
            None => {}
        }
        Ok(step)
    }

    /// The step from `reference` (the previous frame) to `pyramid`.
    fn step_between(
        &self,
        reference: &Pyramid,
        pyramid: &Pyramid,
        sequence: &Affine2<f32>,
        width: u32,
    ) -> (Step, Option<Match>) {
        if self.config.is_disabled() {
            return (Step { transform: Affine2::identity(), brightness: 1.0 }, None);
        }
        let map_scale = Self::map_scale_for(width as usize) as f64;
        let (mw, mh) = (pyramid.layers[0].width, pyramid.layers[0].height);

        // Samples sit where this frame overlaps frame 0's center.
        let rect =
            self.detection_rect_through(mw, mh, &rescale_affine(sequence, map_scale, map_scale));
        // The metric's gain, at the search's identity start.
        let start = Similarity::identity(mw, mh);
        let ratio =
            brightness_ratio(&reference.layers[0], &pyramid.layers[0], rect, &start).unwrap_or(1.0);

        let params = self.search(reference, pyramid, rect, ratio);
        let points = sample_points(&pyramid.layers[0], rect, self.config.sample_points, ratio);
        let quality = match_quality(&reference.layers[0], &params, &points);
        if let Some(q) = quality {
            debug!(
                "GridAligner: difference {:.4} of unrelated images (previous frame's contrast {:.4})",
                q.score / q.unrelated.max(f32::MIN_POSITIVE),
                q.reference_std
            );
        }

        // The reported ratio uses the geometry just found.
        let brightness = if self.config.apply_brightness {
            let rect = self.detection_rect(mw, mh, &params);
            brightness_ratio(&reference.layers[0], &pyramid.layers[0], rect, &params).unwrap_or(1.0)
        } else {
            1.0
        };
        debug!(
            "GridAligner: step shift ({:.2}, {:.2}) map px, rotation {:.3}°, magnification {:.4}{}",
            params.dst_cx - params.src_cx,
            params.dst_cy - params.src_cy,
            params.rotation,
            params.magnification,
            if self.config.apply_brightness {
                format!(", brightness ratio {brightness:.3} (applied)")
            } else {
                String::new()
            }
        );

        let inv_scale = 1.0 / map_scale;
        let transform = rescale_affine(&params.to_affine(), inv_scale, inv_scale);
        (Step { transform, brightness }, quality)
    }

    /// [`DETECTION_RECT`] of the map, mapped forward through `params`
    /// (reference → this frame), reduced to the inscribed axis-aligned
    /// rectangle and rounded inward.
    fn detection_rect(&self, mw: usize, mh: usize, params: &Similarity) -> IRect {
        detection_rect_with(mw, mh, |x, y| params.forward(x, y))
    }

    /// `detection_rect` through a map-pixel affine (the sequence transform).
    fn detection_rect_through(&self, mw: usize, mh: usize, affine: &Affine2<f32>) -> IRect {
        detection_rect_with(mw, mh, |x, y| {
            let p = affine.transform_point(&nalgebra::Point2::new(x, y));
            (p.x, p.y)
        })
    }

    /// Search every layer, coarsest first; `step` carries across layers.
    fn search(&self, reference: &Pyramid, new: &Pyramid, rect: IRect, ratio: f32) -> Similarity {
        let c = &self.config;
        let mut step = [
            c.horizontal_pct / 100.0,
            c.vertical_pct / 100.0,
            c.magnification_pct / 100.0,
            c.rotation_deg,
        ];
        let (ref_layers, new_layers) = (&reference.layers, &new.layers);
        let depth = ref_layers.len().min(new_layers.len());
        let base = &new_layers[0];
        let mut point = Similarity::identity(base.width, base.height);

        for layer in (0..depth).rev() {
            let s = 0.5f32.powi(layer as i32);
            let layer_point = point.scaled(s);
            let layer_rect = rect.scaled(s);
            let out = self.align_one_layer(
                &ref_layers[layer],
                &new_layers[layer],
                &mut step,
                layer_point,
                s,
                layer_rect,
                ratio,
            );
            point = out.scaled(1.0 / s);
            debug!(
                "grid layer {layer} ({}x{}): shift ({:.2}, {:.2}) rot {:.3} mag {:.4} step {:?}",
                new_layers[layer].width,
                new_layers[layer].height,
                point.dst_cx - point.src_cx,
                point.dst_cy - point.src_cy,
                point.rotation,
                point.magnification,
                step
            );
        }
        point
    }

    /// Rounds of grid search on one layer (`s` its scale relative to the
    /// map) from `start`, contracting `step` until every searched axis is
    /// down to the layer's granularity.
    #[allow(clippy::too_many_arguments)]
    fn align_one_layer(
        &self,
        reference: &Plane,
        new: &Plane,
        step: &mut [f32; 4],
        start: Similarity,
        s: f32,
        rect: IRect,
        ratio: f32,
    ) -> Similarity {
        let max_radius = (new.width.max(new.height) / 2) as f32;
        let granularity =
            if s == 1.0 { FINEST_LAYER_GRANULARITY } else { COARSE_LAYER_GRANULARITY };
        // `granularity` pixels at the layer's edge, per axis.
        let unit = [
            granularity / (2.0 * max_radius),
            granularity / (2.0 * max_radius),
            granularity / max_radius,
            granularity * (1.0 / max_radius).asin().to_degrees(),
        ];

        let points = sample_points(new, rect, self.config.sample_points, ratio);
        let mut point = start;
        if points.is_empty() {
            return point;
        }

        for _round in 0..MAX_ROUNDS_PER_LAYER {
            let grid = Grid::new(step, &unit);
            let total = grid.total();
            if total <= 1 {
                break;
            }

            let candidates = grid.candidates(&point, total, max_radius);
            let mut scores = vec![0.0f32; total];
            let per_task = score_task_size(total);
            scores.par_chunks_mut(per_task).zip(candidates.par_chunks(per_task)).for_each(
                |(scores, candidates)| difference_scores(reference, candidates, &points, scores),
            );

            let center_index = grid.center();
            let best = best_from(center_index, &scores);
            // No candidate could be scored.
            if scores[best] == f32::MAX {
                break;
            }
            point = candidates[best];
            let n = grid.decode(best);
            grid.refine(&mut point, n, &scores, best, max_radius);
            grid.contract(step, n);
        }
        point
    }
}

/// One round's search grid: how many candidates each axis takes (odd, so
/// the current point is the center) and their spacing.
struct Grid {
    nm: [usize; 4],
    dist: [f32; 4],
}

impl Grid {
    /// The grid spanning ±`step` per axis at about one `unit` apart. An
    /// axis whose unit is not positive (NaN on a 1 px layer) is not searched.
    fn new(step: &[f32; 4], unit: &[f32; 4]) -> Self {
        let mut nm = [1usize; 4];
        let mut dist = [0.0f32; 4];
        for i in 0..4 {
            if !(unit[i].is_finite() && unit[i] > 0.0) {
                continue;
            }
            nm[i] = ((2.0 * (step[i] / unit[i]).trunc() + 1.0) as usize).min(MAX_STEPS_PER_AXIS);
            dist[i] = if nm[i] == 1 { 0.0 } else { 2.0 * step[i] / (nm[i] - 1) as f32 };
        }
        Self { nm, dist }
    }

    fn total(&self) -> usize {
        self.nm.iter().product()
    }

    /// Candidate `inx`'s place on each axis.
    fn decode(&self, mut inx: usize) -> [usize; 4] {
        let mut n = [0usize; 4];
        for (n, &m) in n.iter_mut().zip(&self.nm) {
            *n = inx % m;
            inx /= m;
        }
        n
    }

    /// The `total` candidates around `point`, in index order.
    fn candidates(&self, point: &Similarity, total: usize, max_radius: f32) -> Vec<Similarity> {
        let (nm, dist) = (self.nm, self.dist);
        (0..total)
            .map(|inx| {
                let n = self.decode(inx);
                let offset: [f32; 4] =
                    std::array::from_fn(|i| dist[i] * (n[i] as f32 - (nm[i] - 1) as f32 * 0.5));
                Similarity {
                    dst_cx: point.dst_cx + offset[SHIFT_X] * max_radius * 2.0,
                    dst_cy: point.dst_cy + offset[SHIFT_Y] * max_radius * 2.0,
                    rotation: point.rotation + offset[ROTATION],
                    magnification: point.magnification + offset[SCALE],
                    ..*point
                }
            })
            .collect()
    }

    /// The center candidate's index: the current point (every `nm` is odd).
    fn center(&self) -> usize {
        let mut best = 0;
        let mut stride = 1;
        for &m in &self.nm {
            best += (m - 1) / 2 * stride;
            stride *= m;
        }
        best
    }

    /// Sub-grid refinement: a parabola through the winner and its two
    /// neighbors along each axis. Without it, steps quantized to the grid
    /// under-read a slow magnification drift over a long stack.
    fn refine(
        &self,
        point: &mut Similarity,
        n: [usize; 4],
        scores: &[f32],
        best: usize,
        max_radius: f32,
    ) {
        let (nm, dist) = (self.nm, self.dist);
        let mut stride = 1;
        for i in 0..4 {
            if nm[i] >= 3 && n[i] > 0 && n[i] < nm[i] - 1 {
                let (s_lo, s_mid, s_hi) =
                    (scores[best - stride], scores[best], scores[best + stride]);
                let denom = s_lo - 2.0 * s_mid + s_hi;
                if denom > 0.0 && s_lo < f32::MAX && s_hi < f32::MAX {
                    let frac = (0.5 * (s_lo - s_hi) / denom).clamp(-0.5, 0.5);
                    match i {
                        SHIFT_X => point.dst_cx += frac * dist[i] * max_radius * 2.0,
                        SHIFT_Y => point.dst_cy += frac * dist[i] * max_radius * 2.0,
                        SCALE => point.magnification += frac * dist[i],
                        _ => point.rotation += frac * dist[i],
                    }
                }
            }
            stride *= nm[i];
        }
    }

    /// Pattern search: an axis whose winner (at `n`) is on its grid's edge
    /// keeps its step; an interior winner halves it. Always contracting
    /// would make a coarse-layer mistake unrecoverable.
    fn contract(&self, step: &mut [f32; 4], n: [usize; 4]) {
        let (nm, dist) = (self.nm, self.dist);
        for i in 0..4 {
            let interior = n[i] != 0 && n[i] != nm[i] - 1;
            if nm[i] > 1 && interior {
                step[i] = (0.5 * step[i]).min(dist[i]);
            }
        }
    }
}

/// The lowest of `scores`; exact ties keep the center candidate.
fn best_from(center: usize, scores: &[f32]) -> usize {
    let mut best = center;
    for (i, &score) in scores.iter().enumerate() {
        if score < scores[best] {
            best = i;
        }
    }
    best
}

fn detection_rect_with(mw: usize, mh: usize, forward: impl Fn(f32, f32) -> (f32, f32)) -> IRect {
    let (rx0, ry0, rx1, ry1) = DETECTION_RECT;
    let x0 = (rx0 * mw as f32).trunc();
    let y0 = (ry0 * mh as f32).trunc();
    let x1 = (rx1 * mw as f32).ceil();
    let y1 = (ry1 * mh as f32).ceil();
    let tl = forward(x0, y0);
    let tr = forward(x1, y0);
    let bl = forward(x0, y1);
    let br = forward(x1, y1);
    IRect {
        x0: tl.0.max(bl.0).ceil() as i64,
        y0: tl.1.max(tr.1).ceil() as i64,
        x1: tr.0.min(br.0).floor() as i64,
        y1: bl.1.min(br.1).floor() as i64,
    }
}

/// Ratio of mean brightness, reference over this frame, over the same scene
/// content: positions that leave the reference through `params` are
/// skipped on both sides. `None` when either side is black; otherwise
/// bounded to [`MAX_BRIGHTNESS_RATIO`], since it compounds over the sequence.
fn brightness_ratio(
    reference: &Plane,
    new: &Plane,
    rect: IRect,
    params: &Similarity,
) -> Option<f32> {
    let (x0, y0, x1, y1) = rect.clamped(new.width, new.height);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let area = (x1 - x0) * (y1 - y0);
    let step = ((area as f32 / BRIGHTNESS_SAMPLES).sqrt().floor() as usize).max(1);
    let (sn, cs) = params.sin_cos();
    let inv_m = params.inv_magnification();
    // Summed in row order, independent of rayon's scheduling.
    let rows: Vec<(f64, f64)> = (y0..y1)
        .into_par_iter()
        .step_by(step)
        .map(|y| {
            let new_row = new.row(y);
            let (mut r, mut n) = (0.0f64, 0.0f64);
            for x in (x0..x1).step_by(step) {
                let (sx, sy) = params.back(x as f32, y as f32, sn, cs, inv_m);
                if let Some(v) = reference.bilinear(sx, sy) {
                    r += v as f64;
                    n += new_row[x] as f64;
                }
            }
            (r, n)
        })
        .collect();
    let (ref_sum, new_sum) = rows.iter().fold((0.0, 0.0), |a, b| (a.0 + b.0, a.1 + b.1));
    (ref_sum > 1e-6 && new_sum > 1e-6).then(|| {
        ((ref_sum / new_sum) as f32).clamp(1.0 / MAX_BRIGHTNESS_RATIO, MAX_BRIGHTNESS_RATIO)
    })
}

/// The difference metric's samples, one array per field for vector loads:
/// frame positions and values prescaled by the brightness ratio.
#[derive(Clone, Debug, Default)]
struct Samples {
    xs: Vec<f32>,
    ys: Vec<f32>,
    values: Vec<f32>,
}

impl Samples {
    fn len(&self) -> usize {
        self.xs.len()
    }

    fn is_empty(&self) -> bool {
        self.xs.is_empty()
    }

    fn iter(&self) -> impl Iterator<Item = (f32, f32, f32)> + '_ {
        self.xs.iter().zip(&self.ys).zip(&self.values).map(|((&x, &y), &v)| (x, y, v))
    }
}

/// About `target` evenly spaced samples of `rect`; none when it misses the
/// layer.
fn sample_points(layer: &Plane, rect: IRect, target: usize, ratio: f32) -> Samples {
    let (x0, y0, x1, y1) = rect.clamped(layer.width, layer.height);
    if x1 <= x0 || y1 <= y0 {
        return Samples::default();
    }
    let area = ((x1 - x0) * (y1 - y0)) as f32;
    let target = target.max(1) as f32;
    let step_x = ((area / target).sqrt().floor() as usize).max(1);
    let step_y = ((area / target / step_x as f32) as usize).max(1);
    let n = (x1 - x0).div_ceil(step_x) * (y1 - y0).div_ceil(step_y);
    let mut points = Samples {
        xs: Vec::with_capacity(n),
        ys: Vec::with_capacity(n),
        values: Vec::with_capacity(n),
    };
    for y in (y0..y1).step_by(step_y) {
        for x in (x0..x1).step_by(step_x) {
            points.xs.push(x as f32);
            points.ys.push(y as f32);
            points.values.push(layer.at(x, y) * ratio);
        }
    }
    points
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AlignedFrame;
    use crate::image_ops::image_to_gray_mean;
    use crate::test_util::{fixture, max_corner_error, scale_rotate, translation, warped};
    use crate::{AlignMode, Aligner};
    use nalgebra::Point2;

    const W: u32 = 1600;
    const H: u32 = 1200;

    fn spider_gray() -> Plane {
        image_to_gray_mean(&fixture("spider.jpg", W, H)).expect("gray")
    }

    fn corner_error(a: &Affine2<f32>, b: &Affine2<f32>) -> f32 {
        max_corner_error(a, b, W as f32, H as f32)
    }

    #[test]
    fn first_frame_is_identity() {
        let gray = spider_gray();
        let mut aligner = Aligner::new(AlignMode::Grid);
        let frame = aligner.add_gray(&gray).unwrap();
        assert_eq!(frame, AlignedFrame::identity());
        let area = aligner.crop().unwrap();
        assert_eq!((area.width, area.height), (W, H));
    }

    #[test]
    fn recovers_translation_chained() {
        let gray = spider_gray();
        let (w, h) = (W as usize, H as usize);
        // frame(x) = ref(W x), so the reference → frame map is W⁻¹.
        let w1 = translation(15.0, 12.0);
        let w2 = translation(-25.0, 30.0);
        let frame1 = warped(&gray, &w1, w, h);
        let frame2 = warped(&gray, &w2, w, h);

        let mut aligner = Aligner::new(AlignMode::Grid);
        aligner.add_gray(&gray).unwrap();
        let r1 = aligner.add_gray(&frame1).unwrap();
        let r2 = aligner.add_gray(&frame2).unwrap();

        // Finest spacing 0.4 full-res px, composed twice.
        let e1 = corner_error(&r1.transform, &w1.try_inverse().unwrap());
        let e2 = corner_error(&r2.transform, &w2.try_inverse().unwrap());
        assert!(e1 < 1.2, "frame 1 error {e1}\n{:?}", r1.transform.matrix());
        assert!(e2 < 1.2, "frame 2 error {e2}\n{:?}", r2.transform.matrix());
        assert!((r1.brightness - 1.0).abs() < 0.02, "brightness {}", r1.brightness);

        let area = aligner.crop().unwrap();
        assert!(area.width < W && area.height < H, "valid area {area:?}");
    }

    #[test]
    fn recovers_scale_and_rotation() {
        let gray = spider_gray();
        let warp = scale_rotate(1.02, 1.5, W as f32 / 2.0, H as f32 / 2.0);
        let frame = warped(&gray, &warp, W as usize, H as usize);

        let mut aligner = Aligner::with_grid_config(
            AlignMode::Grid,
            GridConfig { rotation_deg: 3.0, ..Default::default() },
        );
        aligner.add_gray(&gray).unwrap();
        let r = aligner.add_gray(&frame).unwrap();

        let e = corner_error(&r.transform, &warp.try_inverse().unwrap());
        assert!(e < 1.0, "corner error {e}\n{:?}", r.transform.matrix());
    }

    #[test]
    fn brightness_ratio_restores_exposure() {
        let gray = spider_gray();
        let darker = Plane { data: gray.data.iter().map(|v| v * 0.8).collect(), ..gray.clone() };

        let mut aligner = Aligner::with_grid_config(
            AlignMode::Grid,
            GridConfig { apply_brightness: true, ..Default::default() },
        );
        aligner.add_gray(&gray).unwrap();
        let r = aligner.add_gray(&darker).unwrap();

        assert!((r.brightness - 1.25).abs() < 0.01, "brightness {}", r.brightness);

        let mut off = Aligner::new(AlignMode::Grid);
        off.add_gray(&gray).unwrap();
        assert_eq!(off.add_gray(&darker).unwrap().brightness, 1.0);
        let e = corner_error(&r.transform, &Affine2::identity());
        assert!(e < 0.75, "exposure change moved the frame by {e}");
    }

    /// A featureless pair is the identity step, not an error.
    #[test]
    fn featureless_frame_is_the_identity() {
        let flat = Plane::from_vec(W as usize, H as usize, vec![0.5; (W * H) as usize]);
        let pyramid = GridAligner::prepare(crate::image_ops::GrayRows::Plane(&flat)).unwrap();
        let aligner = GridAligner::new(GridConfig::default());
        let r = aligner.step(&pyramid, &pyramid, &Affine2::identity(), W).unwrap();
        assert!(corner_error(&r.transform, &Affine2::identity()) < 1e-3, "{:?}", r.transform);
    }

    /// A 1×1 pair is the identity step, not a panic.
    #[test]
    fn a_one_pixel_pair_is_the_identity() {
        let one = Plane::from_vec(1, 1, vec![0.5]);
        let pyramid = Pyramid::build(one, MIN_LAYER_SIZE);
        let aligner = GridAligner::new(GridConfig::default());
        let (step, _) = aligner.step_between(&pyramid, &pyramid, &Affine2::identity(), 1);
        assert_eq!(step.transform, Affine2::identity());
    }

    /// A flat frame 0 is an error that leaves the sequence empty, so the
    /// next frame becomes frame 0.
    #[test]
    fn flat_first_frame_is_rejected_and_the_next_becomes_frame_0() {
        let gray = spider_gray();
        let (w, h) = (W as usize, H as usize);
        let mut aligner = Aligner::with_grid_config(
            AlignMode::Grid,
            GridConfig { apply_brightness: true, ..Default::default() },
        );
        for v in [0.0, 0.5] {
            let flat = Plane::from_vec(w, h, vec![v; w * h]);
            let err = aligner.add_gray(&flat).expect_err("flat frame 0");
            assert!(matches!(err, AlignmentError::Featureless { .. }), "{err}");
            assert!(aligner.crop().is_none(), "a flat frame 0 started the sequence");
        }
        assert_eq!(aligner.add_gray(&gray).expect("frame 0"), AlignedFrame::identity());

        let shift = translation(15.0, 12.0);
        let r = aligner.add_gray(&warped(&gray, &shift, w, h)).expect("frame 1");
        let e = corner_error(&r.transform, &shift.try_inverse().unwrap());
        assert!(e < 1.2, "frame 1 off by {e}\n{:?}", r.transform.matrix());
        assert!((r.brightness - 1.0).abs() < 0.02, "brightness {}", r.brightness);
    }

    /// A black reference gives no ratio rather than 0, and a ratio is
    /// bounded, since it compounds over the sequence.
    #[test]
    fn brightness_ratio_refuses_black_and_is_bounded() {
        let (w, h) = (64, 64);
        let flat = |v: f32| Plane::from_vec(w, h, vec![v; w * h]);
        let rect = IRect { x0: 0, y0: 0, x1: w as i64, y1: h as i64 };
        let start = Similarity::identity(w, h);
        let ratio = |r: &Plane, n: &Plane| brightness_ratio(r, n, rect, &start);
        assert_eq!(ratio(&flat(0.0), &flat(0.5)), None);
        assert_eq!(ratio(&flat(0.5), &flat(0.0)), None);
        let r = ratio(&flat(0.4), &flat(0.5)).unwrap();
        assert!((r - 0.8).abs() < 1e-4, "{r}");
        assert_eq!(ratio(&flat(1.0), &flat(0.01)), Some(MAX_BRIGHTNESS_RATIO));
        assert_eq!(ratio(&flat(0.01), &flat(1.0)), Some(1.0 / MAX_BRIGHTNESS_RATIO));
    }

    /// A step no sample could judge (frame 0's center carried outside this
    /// frame) is an error, not an unchecked step.
    #[test]
    fn unjudged_step_is_rejected() {
        let gray = spider_gray();
        let gray_rows = || crate::image_ops::GrayRows::Plane(&gray);
        let aligner = GridAligner::new(GridConfig::default());
        let reference = GridAligner::prepare(gray_rows()).unwrap();
        aligner.first(&reference).unwrap();
        let far = translation(10.0 * W as f32, 10.0 * H as f32);
        let frame = GridAligner::prepare(gray_rows()).unwrap();
        let Err(err) = aligner.step(&reference, &frame, &far, W) else {
            panic!("an unjudged step was accepted");
        };
        assert!(matches!(&err, AlignmentError::NoOverlap), "{err}");
    }

    /// An unrelated frame is an error; the next aligns against the last good
    /// one.
    #[test]
    fn unrelated_frames_are_rejected() {
        let gray = spider_gray();
        let (w, h) = (W as usize, H as usize);
        let mut seed = 12345u32;
        let noise = Plane::from_vec(
            w,
            h,
            (0..w * h)
                .map(|_| {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    (seed >> 8) as f32 / (1u32 << 24) as f32
                })
                .collect(),
        );
        let flat = |v: f32| Plane::from_vec(w, h, vec![v; w * h]);
        let shift = translation(15.0, 12.0);
        let good = warped(&gray, &shift, w, h);

        for (name, bad) in [("black", flat(0.0)), ("gray", flat(0.5)), ("noise", noise)] {
            let mut aligner = Aligner::new(AlignMode::Grid);
            aligner.add_gray(&gray).unwrap();
            let err = aligner.add_gray(&bad).expect_err(name);
            assert!(
                matches!(err, AlignmentError::NoMatch { .. } | AlignmentError::NoOverlap),
                "{name}: {err}"
            );
            let area = aligner.crop().unwrap();
            assert_eq!((area.width, area.height), (W, H), "{name} narrowed the crop");

            let r = aligner.add_gray(&good).expect("the next good frame aligns");
            let e = corner_error(&r.transform, &shift.try_inverse().unwrap());
            assert!(e < 1.2, "{name}: next frame off by {e}\n{:?}", r.transform.matrix());
        }
    }

    /// A badly defocused neighbor (a 32× box blur) still matches.
    #[test]
    fn a_blurred_neighbor_is_kept() {
        let gray = spider_gray();
        let small = Pyramid::build(gray.clone(), 8).layers.swap_remove(5);
        let up = Affine2::from_matrix_unchecked(Matrix3::new(
            1.0 / 32.0,
            0.0,
            0.0,
            0.0,
            1.0 / 32.0,
            0.0,
            0.0,
            0.0,
            1.0,
        ));
        let blurred = warped(&small, &up, W as usize, H as usize);
        let mut aligner = Aligner::new(AlignMode::Grid);
        aligner.add_gray(&gray).unwrap();
        aligner.add_gray(&blurred).expect("a blurred neighbor is kept");
    }

    #[test]
    fn box_down_edges() {
        // 3×3: the right column is averaged over two rows only, the bottom
        // row re-reads itself.
        let layer = Plane { width: 3, height: 3, data: (1..=9).map(|v| v as f32).collect() };
        let down = layer.box_down();
        assert_eq!((down.width, down.height), (2, 2));
        assert_eq!(down.at(0, 0), (1.0 + 2.0 + 4.0 + 5.0) / 4.0);
        assert_eq!(down.at(1, 0), (3.0 + 6.0) / 2.0);
        assert_eq!(down.at(0, 1), (7.0 + 8.0 + 7.0 + 8.0) / 4.0);
        assert_eq!(down.at(1, 1), (9.0 + 9.0) / 2.0);
    }

    #[test]
    fn pyramid_stops_at_min_layer() {
        let base = Plane { width: 300, height: 200, data: vec![0.0; 300 * 200] };
        let pyramid = Pyramid::build(base, 32);
        let sizes: Vec<_> = pyramid.layers.iter().map(|l| (l.width, l.height)).collect();
        assert_eq!(sizes, vec![(300, 200), (150, 100), (75, 50)]);
    }

    #[test]
    fn params_round_trip() {
        let p = Similarity {
            src_cx: 10.0,
            src_cy: 20.0,
            dst_cx: 13.0,
            dst_cy: 18.0,
            rotation: 2.0,
            magnification: 1.03,
        };
        let (sn, cs) = p.sin_cos();
        let (fx, fy) = p.forward(31.0, 7.0);
        let (bx, by) = p.back(fx, fy, sn, cs, p.inv_magnification());
        assert!((bx - 31.0).abs() < 1e-4 && (by - 7.0).abs() < 1e-4);
        let m = p.to_affine().transform_point(&Point2::new(31.0, 7.0));
        assert!((m.x - fx).abs() < 1e-4 && (m.y - fy).abs() < 1e-4);
    }
}
