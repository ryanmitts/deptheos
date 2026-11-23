//! Brush settings and the pure functions a stroke is built from: the
//! falloff, dab spacing, flow per dab and the tolerance test.

use std::ops::RangeInclusive;

pub const RADIUS_RANGE: RangeInclusive<f32> = 1.0..=2048.0;
// Not 0: the falloff at hardness 0 is zero past the center, so it paints
// nothing.
pub const HARDNESS_RANGE: RangeInclusive<f32> = 0.1..=1.0;
pub const OPACITY_RANGE: RangeInclusive<f32> = 0.01..=1.0;
pub const FLOW_RANGE: RangeInclusive<f32> = 0.01..=1.0;
pub const TOLERANCE_RANGE: RangeInclusive<f32> = 0.0..=1.0;
/// The tolerance a brush starts with once tolerance is turned on.
pub const DEFAULT_TOLERANCE: f32 = 0.1;

/// The width of the tolerance's soft edge, in relative difference.
const TOLERANCE_SOFTNESS: f32 = 0.05;

/// Keeps the tolerance's relative difference finite in the shadows, in u16
/// codes (1/1024 of full scale).
const TOLERANCE_EPS: f32 = 64.0;

/// What a stroke paints with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Brush {
    /// In image pixels.
    pub radius: f32,
    /// Where the falloff turns from its gentle slope to its steep one.
    pub hardness: f32,
    /// The most a stroke moves a pixel towards its source, however often
    /// it passes over it.
    pub opacity: f32,
    /// How much each pass adds, up to the opacity; 1 paints the brush
    /// profile at once.
    pub flow: f32,
    /// Paint only where the source is within this relative difference of
    /// the original; `None` paints everywhere.
    pub tolerance: Option<f32>,
}

impl Default for Brush {
    fn default() -> Self {
        Self { radius: 30.0, hardness: 0.8, opacity: 1.0, flow: 1.0, tolerance: None }
    }
}

impl Brush {
    /// Each setting within its range. A NaN (which passes through `clamp`)
    /// takes the default instead.
    pub fn clamped(self) -> Self {
        let d = Self::default();
        let clamp = |v: f32, r: &RangeInclusive<f32>, default: f32| {
            if v.is_nan() { default } else { v.clamp(*r.start(), *r.end()) }
        };
        Self {
            radius: clamp(self.radius, &RADIUS_RANGE, d.radius),
            hardness: clamp(self.hardness, &HARDNESS_RANGE, d.hardness),
            opacity: clamp(self.opacity, &OPACITY_RANGE, d.opacity),
            flow: clamp(self.flow, &FLOW_RANGE, d.flow),
            tolerance: self.tolerance.map(|t| clamp(t, &TOLERANCE_RANGE, DEFAULT_TOLERANCE)),
        }
    }

    /// The distance between dabs, in image pixels: a quarter of the radius,
    /// at least one pixel.
    pub fn spacing(&self) -> f32 {
        (0.25 * self.radius).max(1.0)
    }

    /// The coverage a dab adds at `dist` from its center: two linear
    /// segments in squared distance, meeting at `hardness` there. Zero
    /// outside the radius.
    pub fn falloff(&self, dist: f32) -> f32 {
        if dist > self.radius {
            return 0.0;
        }
        let rr = dist * dist / (self.radius * self.radius);
        let h = self.hardness;
        let f = if rr <= h {
            1.0 - rr * (1.0 - h) / h.max(0.001)
        } else {
            (1.0 - rr) * h / (1.0 - h).max(0.001)
        };
        f.clamp(0.0, 1.0)
    }

    /// The share of the remaining coverage one dab adds, below flow 1.
    /// Flow is per nominal spacing (a quarter radius), so small brushes
    /// at the 1 px floor build up the same over distance.
    pub(crate) fn flow_per_dab(&self) -> f32 {
        let nominal = 0.25 * self.radius;
        1.0 - (1.0 - self.flow).powf(self.spacing() / nominal)
    }
}

/// How much of a dab lands under tolerance `t`: 1 within, 0 well past, a
/// smooth edge between. The difference is the largest per-channel one,
/// relative to the brighter value, so shadows and highlights read alike.
pub(crate) fn tolerance(t: f32, source: [u16; 3], original: [u16; 3]) -> f32 {
    let delta = (0..3)
        .map(|c| {
            let (a, b) = (f32::from(source[c]), f32::from(original[c]));
            (a - b).abs() / (a.max(b) + TOLERANCE_EPS)
        })
        .fold(0.0, f32::max);
    1.0 - smoothstep(t, t + TOLERANCE_SOFTNESS, delta)
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// One stamp of the brush: its center in continuous image coordinates
/// (pixel centers sit at half-integers) and the pen pressure.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dab {
    pub x: f32,
    pub y: f32,
    pub pressure: f32,
}

/// Turns pointer samples into dabs one spacing apart along the path,
/// starting at the first sample, pressure interpolated.
pub struct Spacer {
    spacing: f32,
    last: Option<Dab>,
    /// Distance traveled along the path since the last dab.
    carry: f32,
}

impl Spacer {
    pub fn new(brush: &Brush) -> Self {
        Self { spacing: brush.spacing(), last: None, carry: 0.0 }
    }

    /// Move the pointer to `to`, appending the dabs it passes to `out`.
    pub fn to(&mut self, to: Dab, out: &mut Vec<Dab>) {
        let Some(from) = self.last.replace(to) else {
            out.push(to);
            return;
        };
        let len = (to.x - from.x).hypot(to.y - from.y);
        // An infinite or NaN gap would never be walked: jump instead.
        if !len.is_finite() {
            self.carry = 0.0;
            out.push(to);
            return;
        }
        let mut at = self.spacing - self.carry;
        while at <= len {
            let t = at / len;
            out.push(Dab {
                x: from.x + (to.x - from.x) * t,
                y: from.y + (to.y - from.y) * t,
                pressure: from.pressure + (to.pressure - from.pressure) * t,
            });
            at += self.spacing;
        }
        self.carry = len - (at - self.spacing);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_softest_brush_still_paints_around_its_center() {
        let brush = Brush { radius: 10.0, hardness: *HARDNESS_RANGE.start(), ..Brush::default() };
        assert!(brush.falloff(2.0) > 0.0);
    }

    #[test]
    fn a_non_finite_sample_is_a_jump_not_a_walk() {
        let mut spacer = Spacer::new(&Brush::default());
        let mut out = Vec::new();
        spacer.to(Dab { x: 0.0, y: 0.0, pressure: 1.0 }, &mut out);
        spacer.to(Dab { x: f32::INFINITY, y: 0.0, pressure: 1.0 }, &mut out);
        spacer.to(Dab { x: f32::NAN, y: 0.0, pressure: 1.0 }, &mut out);
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn the_falloff_is_continuous_at_the_hardness() {
        for hardness in [0.1, 0.5, 0.8] {
            let brush = Brush { radius: 10.0, hardness, ..Brush::default() };
            let knee = 10.0 * hardness.sqrt();
            assert!((brush.falloff(knee - 1e-3) - brush.falloff(knee + 1e-3)).abs() < 1e-3);
            assert!((brush.falloff(knee) - hardness).abs() < 1e-5);
            assert_eq!(brush.falloff(0.0), 1.0);
            assert_eq!(brush.falloff(10.0), 0.0);
            assert_eq!(brush.falloff(10.5), 0.0);
        }
        let hard = Brush { radius: 10.0, hardness: 1.0, ..Brush::default() };
        assert_eq!(hard.falloff(9.99), 1.0);
    }

    #[test]
    fn dabs_are_spaced_along_the_path_from_the_last_dab() {
        let brush = Brush { radius: 8.0, ..Brush::default() };
        let mut spacer = Spacer::new(&brush);
        let mut dabs = Vec::new();
        let at = |x| Dab { x, y: 0.0, pressure: 1.0 };
        spacer.to(at(0.0), &mut dabs);
        spacer.to(at(3.0), &mut dabs);
        spacer.to(at(9.0), &mut dabs);
        let xs: Vec<f32> = dabs.iter().map(|d| d.x).collect();
        assert_eq!(xs, [0.0, 2.0, 4.0, 6.0, 8.0]);
        // A single pixel brush still spaces its dabs a pixel apart.
        let tiny = Spacer::new(&Brush { radius: 1.0, ..Brush::default() }).spacing;
        assert_eq!(tiny, 1.0);
    }

    #[test]
    fn flow_per_dab_is_the_flow_at_nominal_spacing() {
        let brush = Brush { radius: 40.0, flow: 0.3, ..Brush::default() };
        assert!((brush.flow_per_dab() - 0.3).abs() < 1e-6);
        // Spread four times as far apart, each dab adds four dabs' worth.
        let tiny = Brush { radius: 1.0, flow: 0.3, ..Brush::default() };
        assert!((tiny.flow_per_dab() - (1.0 - 0.7f32.powi(4))).abs() < 1e-6);
    }

    #[test]
    fn tolerance_is_relative_and_soft_edged() {
        let gray = |v| [v; 3];
        assert_eq!(tolerance(0.1, gray(30000), gray(30000)), 1.0);
        // 5 % apart, in the shadows and in the highlights alike.
        assert_eq!(tolerance(0.1, gray(1900), gray(2000)), 1.0);
        assert_eq!(tolerance(0.1, gray(57000), gray(60000)), 1.0);
        // 30 % apart is past the edge.
        assert_eq!(tolerance(0.1, gray(42000), gray(60000)), 0.0);
        let edge = tolerance(0.1, gray(52500), gray(60000));
        assert!(edge > 0.0 && edge < 1.0, "{edge}");
    }
}
