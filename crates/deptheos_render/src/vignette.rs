//! Vignette, lightcraft's post-crop vignette as it stands (MIT OR
//! Apache-2.0): its numbers from craft-libs b2d2a1e94
//! `crates/develop/src/finish.rs` (`vignette`), its law per pixel from
//! lightcraft 5bdb4e7a6 `crates/pipeline/src/finish.rs` (`finish_with`).
//! Roundness, Feather, Highlights and the style keep lightcraft's
//! defaults (0, 50, 0, highlight priority), so a vignette is an ellipse
//! halfway to the frame's shape, darkening by a gain or lightening toward
//! white. No other crate, so the port's check reads this file as it is.

/// lightcraft's defaults for what Deptheos doesn't show: Roundness,
/// Feather (craft-libs b2d2a1e94 `crates/develop-model/src/settings.rs`,
/// `Vignette`).
const ROUNDNESS: f64 = 0.0;
const FEATHER: f64 = 50.0;
/// How far a lightening vignette goes toward white.
pub const LIGHTEN: f32 = 0.85;
/// The power of the distance at Roundness 0: an ellipse. The shaders take
/// it as a constant.
pub const POWER: f32 = {
    let r = (ROUNDNESS / 100.0) as f32;
    if r >= 0.0 { 2.0 } else { 2.0 + (-r) * 6.0 }
};

/// A vignette's numbers: lightcraft's `Vig`, but for Highlights and the
/// style, which take no part at their defaults.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vig {
    pub amount: f32,
    pub start: f32,
    pub width: f32,
    pub aspect_mix: f32,
    pub power: f32,
}

impl Vig {
    /// Amount (-100 to 100) and Midpoint (0 to 100), as craft-libs'
    /// `vignette` resolves them; `None` at Amount 0.
    pub fn new(amount: f64, midpoint: f64) -> Option<Self> {
        (amount != 0.0).then(|| {
            let r = (ROUNDNESS / 100.0) as f32;
            Self {
                amount: (amount / 100.0) as f32,
                start: 0.15 + (midpoint / 100.0) as f32 * 0.95,
                width: 0.05 + (FEATHER / 100.0) as f32 * 1.1,
                aspect_mix: ((r + 1.0) / 2.0).clamp(0.0, 1.0),
                power: if r >= 0.0 { 2.0 } else { 2.0 + (-r) * 6.0 },
            }
        })
    }

    /// How much of the vignette pixel (`x`, `y`) of a frame takes, 0 to 1:
    /// `window` is the reframe's rectangle (left, top, width, height) in
    /// the same pixels, which the vignette is centered on and shaped by.
    pub fn reach(&self, window: [f32; 4], (x, y): (f32, f32)) -> f32 {
        let [vx, vy, vw, vh] = window;
        let aspect = vw / vh;
        let u = (x - vx + 0.5) / vw * 2.0 - 1.0;
        let vv = (y - vy + 0.5) / vh * 2.0 - 1.0;
        let sx = 1.0 + (aspect - 1.0) * self.aspect_mix;
        let sy = 1.0 + (1.0 / aspect - 1.0) * self.aspect_mix;
        let (ax, ay) =
            ((u * sx.max(1.0) / sx.max(sy)).abs(), (vv * sy.max(1.0) / sx.max(sy)).abs());
        let dist = (ax.powf(self.power) + ay.powf(self.power)).powf(1.0 / self.power);
        smooth(self.start, self.start + self.width, dist)
    }

    /// Linear values `d` under the vignette where it reaches `t`: darkened
    /// by a gain, or lightened toward white. Highlights at 0 leaves the
    /// gain as it is.
    pub fn apply(&self, d: [f32; 3], t: f32) -> [f32; 3] {
        if t > 0.0 {
            if self.amount < 0.0 {
                let f = 1.0 + self.amount * t;
                d.map(|c| c * f)
            } else {
                d.map(|c| c + (1.0 - c) * self.amount * t * LIGHTEN)
            }
        } else {
            d
        }
    }
}

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_at_amount_0_whatever_the_midpoint() {
        assert_eq!(Vig::new(0.0, 80.0), None);
        let v = Vig::new(-1.0, 50.0).unwrap();
        assert_eq!((v.amount, v.aspect_mix, v.power), (-0.01, 0.5, POWER));
    }

    /// The center takes none; the corners take it all; a higher midpoint
    /// starts it further out.
    #[test]
    fn it_reaches_the_corners_and_not_the_center() {
        let window = [100.0, 50.0, 400.0, 300.0];
        let v = Vig::new(-50.0, 50.0).unwrap();
        assert_eq!(v.reach(window, (299.5, 199.5)), 0.0);
        assert_eq!(v.reach(window, (100.0, 50.0)), 1.0);
        let edge = (480.0, 199.5);
        let later = Vig::new(-50.0, 90.0).unwrap();
        assert!(later.reach(window, edge) < v.reach(window, edge));
        assert_eq!(v.apply([0.5, 0.2, 0.1], 1.0), [0.25, 0.1, 0.05]);
        let lighter = Vig::new(100.0, 50.0).unwrap().apply([0.0, 0.5, 1.0], 1.0);
        assert_eq!(lighter, [0.85, 0.925, 1.0]);
    }
}
