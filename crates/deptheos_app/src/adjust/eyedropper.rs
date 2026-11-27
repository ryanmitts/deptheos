//! The white balance eyedropper: the stored pixels with paint under a
//! click, before any adjustment, set as gray. The click is on the picture
//! as shown, mapped back through the reframe to the stored pixel under it.
//! A camera RAW stack's white is the DNG SDK's for the patch's camera
//! neutral; a standard stack's is the relative value whose matrix draws the
//! patch gray (`deptheos_render::relative_for_gray`).

use deptheos_dng::Temperature;
use deptheos_render::reframe::Size;
use deptheos_render::{Adjustments, Draw, OutputSpace, Reframe, WhiteBalance, slider};

use crate::runs::session::{CompletedStack, StackColor};
use crate::words::{Words, say};
use crate::{Look, Result};

/// The patch's side in stored pixels: a 5 × 5 patch, centered on the pixel
/// clicked, moved inside the picture at its edges.
pub const PATCH: u32 = 5;

/// A code at or above this is clipped: within 1/256 of the top.
const CLIPPED: u16 = 0xFF00;

/// A patch whose darkest channel is below this share of white is too dark
/// to read a color from.
const DARK: f64 = 1.0 / 1024.0;

/// Why a click set no white balance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The click isn't on the picture.
    Outside,
    /// A channel of the patch is clipped, so its color isn't known.
    Clipped,
    /// The patch is too dark to read a color from.
    Dark,
    /// The stack has no white to set, or none makes the patch gray.
    NoWhite,
}

impl Refused {
    /// What the app says.
    pub fn say(self, words: &Words) -> String {
        match self {
            Self::Outside => say!(words, "adjust-eyedrop-outside"),
            Self::Clipped => say!(words, "adjust-eyedrop-clipped"),
            Self::Dark => say!(words, "adjust-eyedrop-dark"),
            Self::NoWhite => say!(words, "adjust-eyedrop-no-white"),
        }
    }
}

/// The white balance that makes the patch under `at` gray: `at` a point on
/// the picture as shown (its pixel centers at index plus a half) under
/// `reframe`, `pixels` the stored picture with its paint.
pub fn eyedrop(
    stack: &CompletedStack,
    pixels: &[u16],
    reframe: Reframe,
    at: (f64, f64),
) -> std::result::Result<WhiteBalance, Refused> {
    let picture = Size { width: stack.width, height: stack.height };
    let (x, y) = reframe.to_stored(picture, at).map_err(|_| Refused::Outside)?;
    let inside = |v: f64, side: u32| v >= 0.0 && v < f64::from(side);
    if !(inside(x, stack.width) && inside(y, stack.height)) {
        return Err(Refused::Outside);
    }
    let patch = patch(stack, pixels, (x as u32, y as u32));
    if patch.iter().flatten().any(|&c| c >= CLIPPED) {
        return Err(Refused::Clipped);
    }
    match &stack.color {
        StackColor::Linear { metadata, render_params } => {
            let black = render_params.black_level;
            let range = (65535.0 - black).max(1.0);
            let mean = mean(&patch, |rgb| rgb.map(|c| (f64::from(c) - black) / range));
            if mean.iter().any(|&c| c < DARK) {
                return Err(Refused::Dark);
            }
            let white = metadata.white_for_neutral(mean).ok().flatten().ok_or(Refused::NoWhite)?;
            let Temperature { kelvin, tint } = Temperature::of(white);
            Ok(WhiteBalance::Kelvin {
                temperature: slider::TEMPERATURE_KELVIN.snap(kelvin),
                tint: slider::TINT_RAW.snap(tint),
            })
        }
        StackColor::Srgb { .. } => {
            let constants = as_stored(stack).map_err(|_| Refused::NoWhite)?;
            let linear = |rgb: [u16; 3]| constants.stored_in_prophoto(rgb).unwrap_or([0.0; 3]);
            let mean = mean(&patch, linear);
            // A NaN counts as dark.
            if !mean.iter().all(|&c| c >= DARK) {
                return Err(Refused::Dark);
            }
            let (temperature, tint) =
                deptheos_render::relative_for_gray(mean).ok_or(Refused::NoWhite)?;
            Ok(WhiteBalance::Relative {
                temperature: slider::TEMPERATURE_RELATIVE.snap(temperature),
                tint: slider::TINT_RELATIVE.snap(tint),
            })
        }
    }
}

/// The [`PATCH`]-sided patch around stored pixel `(x, y)`, inside the
/// picture.
fn patch(stack: &CompletedStack, pixels: &[u16], (x, y): (u32, u32)) -> Vec<[u16; 3]> {
    let start = |v: u32, side: u32| v.saturating_sub(PATCH / 2).min(side.saturating_sub(PATCH));
    let (x0, y0) = (start(x, stack.width), start(y, stack.height));
    let (x1, y1) = ((x0 + PATCH).min(stack.width), (y0 + PATCH).min(stack.height));
    let mut patch = Vec::with_capacity((PATCH * PATCH) as usize);
    for row in y0..y1 {
        for col in x0..x1 {
            let i = ((row * stack.width + col) * 3) as usize;
            patch.push([pixels[i], pixels[i + 1], pixels[i + 2]]);
        }
    }
    patch
}

fn mean(patch: &[[u16; 3]], of: impl Fn([u16; 3]) -> [f64; 3]) -> [f64; 3] {
    let mut sum = [0.0; 3];
    for &rgb in patch {
        let v = of(rgb);
        (0..3).for_each(|c| sum[c] += v[c]);
    }
    sum.map(|s| s / patch.len().max(1) as f64)
}

/// The constants that take a standard stack's stored codes to linear
/// ProPhoto as shot.
fn as_stored(stack: &CompletedStack) -> Result<Box<deptheos_render::dng_render::Constants>> {
    let look = Look::redrawn(stack, &Adjustments::NEUTRAL, Reframe::NONE, OutputSpace::Srgb);
    match look.resolve() {
        Draw::Rendered(constants) => Ok(constants),
        Draw::Stored { .. } => {
            Err(crate::Error::InvalidState("a redrawn look draws on the GPU".into()))
        }
    }
}

#[cfg(test)]
mod tests {
    use deptheos_render::reframe::{Flip, Rect, Turn};

    use super::*;

    const W: u32 = 40;
    const H: u32 = 30;

    /// A standard stack of `background`, with `spot` at stored (30, 7).
    fn standard(background: [u16; 3], spot: [u16; 3]) -> CompletedStack {
        let mut pixels = Vec::new();
        for y in 0..H {
            for x in 0..W {
                let near = x.abs_diff(30) <= 3 && y.abs_diff(7) <= 3;
                pixels.extend(if near { spot } else { background });
            }
        }
        CompletedStack::srgb(W, H, pixels, Vec::new())
    }

    /// What `constants` take stored codes to, as a matrix on the codes'
    /// linear values: its columns are pure red, green and blue at full.
    fn matrix(constants: &deptheos_render::dng_render::Constants) -> [[f64; 3]; 3] {
        let column = |c: usize| {
            let mut rgb = [0u16; 3];
            rgb[c] = 65535;
            constants.stored_in_prophoto(rgb).unwrap()
        };
        let cols = [column(0), column(1), column(2)];
        std::array::from_fn(|r| std::array::from_fn(|c| cols[c][r]))
    }

    fn constants(
        stack: &CompletedStack,
        white_balance: WhiteBalance,
    ) -> Box<deptheos_render::dng_render::Constants> {
        let adjustments = Adjustments { white_balance, ..Adjustments::NEUTRAL };
        let look = Look::redrawn(stack, &adjustments, Reframe::NONE, OutputSpace::Srgb);
        let Draw::Rendered(constants) = look.resolve() else { panic!("drawn on the GPU") };
        constants
    }

    /// sRGB codes of a gray lit by the light `temperature` and `tint`
    /// correct: the codes whose values that white balance draws gray.
    fn lit_gray(stack: &CompletedStack, temperature: f64, tint: f64) -> [u16; 3] {
        let m = matrix(&constants(stack, WhiteBalance::Relative { temperature, tint }));
        let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
        let cof = |i: usize, j: usize| {
            let (i0, i1, j0, j1) = ((i + 1) % 3, (i + 2) % 3, (j + 1) % 3, (j + 2) % 3);
            m[i0][j0] * m[i1][j1] - m[i0][j1] * m[i1][j0]
        };
        let linear: [f64; 3] =
            std::array::from_fn(|r| (0..3).map(|c| cof(c, r) / det * 0.18).sum());
        let encode = |v: f64| {
            let v = v.clamp(0.0, 1.0);
            if v <= 0.0031308 { 12.92 * v } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
        };
        linear.map(|v| (encode(v) * 65535.0).round() as u16)
    }

    #[test]
    fn a_gray_patch_under_a_cast_gives_back_the_cast() {
        for (t, tint) in [(0.0, 0.0), (40.0, 0.0), (-55.0, 20.0), (25.0, -30.0)] {
            let plain = standard([20000; 3], [20000; 3]);
            let spot = lit_gray(&plain, t, tint);
            let stack = standard([20000; 3], spot);
            let got = eyedrop(&stack, &stack.pixels, Reframe::NONE, (30.5, 7.5)).unwrap();
            // Under it the patch draws gray.
            let drawn = constants(&stack, got).stored_in_prophoto(spot).unwrap();
            let spread = drawn.iter().fold(0.0f64, |a, &c| a.max((c / drawn[1] - 1.0).abs()));
            assert!(spread < 0.01, "{t} {tint}: {drawn:?}");
            let WhiteBalance::Relative { temperature, tint: got_tint } = got else {
                panic!("{got:?}")
            };
            assert!(
                (temperature - t).abs() <= 1.0 && (got_tint - tint).abs() <= 1.0,
                "{t} {tint}: {got:?}"
            );
        }
    }

    /// A camera RAW stack of a gray lit by `white`: each pixel the camera
    /// neutral there, at `level`; the stack's own metadata from a plain DNG
    /// of it.
    fn raw_gray(dir: &std::path::Path, white: deptheos_dng::WhiteXy, level: f64) -> CompletedStack {
        use deptheos_dng::test_dng::{Layout, TestDng};
        let flat = vec![20000u16; (W * H * 3) as usize];
        let dng = TestDng {
            width: W,
            height: H,
            cfa: false,
            big_endian: false,
            layout: Layout::Strips { rows: 16 },
            black: [0; 3],
            white: [65535; 3],
            active: [0, 0, H, W],
            crop: [0, 0, W, H],
        };
        let path = dir.join("gray.dng");
        dng.write(&path, &flat);
        let (_, metadata) = crate::decode_linear_with_metadata(&path, Default::default()).unwrap();
        let render_params = metadata.render_params().unwrap();
        let neutral = metadata.render_params_at(Some(white)).unwrap().camera_white;
        let pixel = neutral.map(|c| (f64::from(c) * level * 65535.0).round() as u16);
        let pixels = (0..W * H).flat_map(|_| pixel).collect();
        CompletedStack {
            color: StackColor::Linear { metadata, render_params },
            ..CompletedStack::srgb(W, H, pixels, vec![path])
        }
    }

    #[test]
    fn a_raw_gray_under_a_light_gives_back_its_temperature() {
        let dir = tempfile::tempdir().unwrap();
        for (kelvin, tint) in [(3200.0, 0.0), (5000.0, 12.0), (7500.0, -8.0)] {
            let white = Temperature { kelvin, tint }.white();
            let stack = raw_gray(dir.path(), white, 0.3);
            let got = eyedrop(&stack, &stack.pixels, Reframe::NONE, (10.5, 10.5)).unwrap();
            let WhiteBalance::Kelvin { temperature, tint: got_tint } = got else {
                panic!("{got:?}")
            };
            assert!((temperature - kelvin).abs() <= 50.0, "{kelvin} K: {got:?}");
            assert!((got_tint - tint).abs() <= 1.0, "tint {tint}: {got:?}");
        }
    }

    #[test]
    fn a_clipped_or_dark_patch_or_a_click_off_the_picture_refuses() {
        let clipped = standard([20000; 3], [65535, 30000, 30000]);
        assert_eq!(
            eyedrop(&clipped, &clipped.pixels, Reframe::NONE, (30.5, 7.5)),
            Err(Refused::Clipped)
        );
        let dark = standard([20000; 3], [0, 0, 0]);
        assert_eq!(eyedrop(&dark, &dark.pixels, Reframe::NONE, (30.5, 7.5)), Err(Refused::Dark));
        assert_eq!(eyedrop(&dark, &dark.pixels, Reframe::NONE, (-1.0, 7.5)), Err(Refused::Outside));
        assert_eq!(eyedrop(&dark, &dark.pixels, Reframe::NONE, (40.5, 7.5)), Err(Refused::Outside));
    }

    /// A click on a cropped, flipped and turned view samples the stored
    /// pixel under it.
    #[test]
    fn a_click_on_a_reframed_view_samples_the_stored_pixel_under_it() {
        let stack = standard([20000; 3], [0, 0, 0]);
        let reframe = Reframe {
            crop: Some(Rect { x: 20, y: 2, width: 16, height: 12 }),
            flip: Flip::Horizontal,
            turn: Turn::Quarter,
        };
        let picture = Size { width: W, height: H };
        let (ox, oy) = reframe.to_output(picture, (30.5, 7.5)).unwrap();
        assert_eq!(eyedrop(&stack, &stack.pixels, reframe, (ox, oy)), Err(Refused::Dark));
        // The same output point unframed is on the background.
        assert!(eyedrop(&stack, &stack.pixels, Reframe::NONE, (ox, oy)).is_ok());
    }
}
