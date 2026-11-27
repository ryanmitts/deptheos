//! What a render's adjustments become when a render is made, pasted onto
//! or reset: pure functions both apps share (ADR-144), with the version
//! rules of ADR-152.

use deptheos_render::{Adjustments, Reframe, WhiteBalance};

use super::RenderAdjustments;
use crate::runs::session::CompletedStack;

/// How a new render's stored pixels relate to its base render's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Geometry {
    /// The same size, each pixel on the same point: a reframe still fits.
    Same,
    /// Anything else; a reframe isn't carried over.
    Differs,
}

/// How far a mapped corner may land from its own place and still count
/// as the same pixel.
const SAME_WITHIN_PX: f32 = 0.01;

impl Geometry {
    /// How `new`'s stored pixels relate to `base`'s: the same when both
    /// are the same size and every corner of `new` lands on itself in
    /// `base` through the renders' line-up (ADR-147).
    pub fn between(new: &CompletedStack, base: &CompletedStack) -> Self {
        if (new.width, new.height) != (base.width, base.height) {
            return Self::Differs;
        }
        let Some(alignment) = new.alignment_to(base) else { return Self::Differs };
        // `alignment` takes `new`'s full-frame coordinates to `base`'s
        // pixels; `new`'s own pixels start at its crop's origin.
        let origin = new.geometry.crop.map_or((0.0, 0.0), |r| (r.x as f32, r.y as f32));
        let (w, h) = (new.width as f32, new.height as f32);
        let same = [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)].into_iter().all(|(x, y)| {
            let full = nalgebra::Point2::new(x + origin.0, y + origin.1);
            let there = alignment.transform.transform_point(&full);
            (there.x - x).abs() <= SAME_WITHIN_PX && (there.y - y).abs() <= SAME_WITHIN_PX
        });
        if same { Self::Same } else { Self::Differs }
    }
}

/// Which white balance a stack's sliders take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StackKind {
    /// Kelvins and the DNG SDK's tint.
    CameraRaw,
    /// The relative scale.
    Standard,
}

/// What a new render starts with: its base render's sliders, in that
/// render's process version, and its reframe when the geometry is the
/// same. With no base (a first render, a batch's) it starts neutral in
/// the latest process.
pub fn inherited(base: Option<&RenderAdjustments>, geometry: Geometry) -> RenderAdjustments {
    let Some(base) = base else { return RenderAdjustments::NONE };
    let reframe = match geometry {
        Geometry::Same => base.reframe,
        Geometry::Differs => Reframe::NONE,
    };
    RenderAdjustments { adjustments: base.adjustments.clone(), reframe }
}

/// `copied` pasted onto `onto`, a render of `kind`: every slider and the
/// vignette. The crop, turn and flip stay `onto`'s, and so does its
/// process version. A white balance of the other kind's scale keeps
/// `onto`'s own.
pub fn pasted(
    copied: &Adjustments,
    onto: &RenderAdjustments,
    kind: StackKind,
) -> RenderAdjustments {
    let fits = match (kind, copied.white_balance) {
        (_, WhiteBalance::AsShot) => true,
        (StackKind::CameraRaw, WhiteBalance::Kelvin { .. }) => true,
        (StackKind::Standard, WhiteBalance::Relative { .. }) => true,
        (StackKind::CameraRaw, WhiteBalance::Relative { .. })
        | (StackKind::Standard, WhiteBalance::Kelvin { .. }) => false,
    };
    let white_balance = if fits { copied.white_balance } else { onto.adjustments.white_balance };
    let adjustments =
        Adjustments { white_balance, ..copied.clone() }.in_process(onto.adjustments.process());
    RenderAdjustments { adjustments, reframe: onto.reframe }
}

/// Every slider back to neutral, in the latest process. The reframe has
/// its own reset, the crop tool's, so it stays.
pub fn reset_all(onto: &RenderAdjustments) -> RenderAdjustments {
    RenderAdjustments { adjustments: Adjustments::NEUTRAL, reframe: onto.reframe }
}

#[cfg(test)]
mod tests {
    use deptheos_render::reframe::{Flip, Rect, Turn};
    use deptheos_render::{Sharpening, Vignette};

    use super::*;

    /// Sliders set and a reframe.
    fn set() -> RenderAdjustments {
        RenderAdjustments {
            adjustments: Adjustments {
                exposure: 0.7,
                white_balance: WhiteBalance::Kelvin { temperature: 5200.0, tint: 8.0 },
                contrast: 20.0,
                clarity: 15.0,
                sharpening: Sharpening { amount: 40.0, ..Sharpening::NONE },
                vignette: Vignette { amount: -30.0, midpoint: 40.0 },
                ..Adjustments::NEUTRAL
            },
            reframe: Reframe {
                crop: Some(Rect { x: 10, y: 20, width: 300, height: 200 }),
                turn: Turn::Quarter,
                flip: Flip::Horizontal,
            },
        }
    }

    #[test]
    fn a_new_render_starts_from_its_base() {
        let base = set();
        let cases = [
            (None, Geometry::Same, RenderAdjustments::NONE),
            (None, Geometry::Differs, RenderAdjustments::NONE),
            (Some(&base), Geometry::Same, base.clone()),
            (
                Some(&base),
                Geometry::Differs,
                RenderAdjustments { reframe: Reframe::NONE, ..base.clone() },
            ),
        ];
        for (from, geometry, want) in cases {
            assert_eq!(inherited(from, geometry), want, "{from:?} {geometry:?}");
        }
        assert_eq!(
            inherited(Some(&base), Geometry::Same).adjustments.process(),
            base.adjustments.process()
        );
    }

    #[test]
    fn a_reframe_fits_only_the_same_pixels() {
        use std::path::PathBuf;

        use deptheos_core::transform::FrameAlignment;

        let paths: Vec<PathBuf> = (0..3).map(|i| PathBuf::from(format!("f{i}.jpg"))).collect();
        let stack = |w, h, paths: &[PathBuf]| {
            CompletedStack::srgb(w, h, vec![0; (w * h * 3) as usize], paths.to_vec())
        };
        let base = stack(8, 6, &paths);
        assert_eq!(Geometry::between(&stack(8, 6, &paths), &base), Geometry::Same);
        // A frame added keeps the pixels where they were.
        let more: Vec<PathBuf> = paths.iter().cloned().chain([PathBuf::from("f3.jpg")]).collect();
        assert_eq!(Geometry::between(&stack(8, 6, &more), &base), Geometry::Same);
        assert_eq!(Geometry::between(&stack(8, 5, &paths), &base), Geometry::Differs);
        // No frame in common: nothing says where the pixels are.
        let others: Vec<PathBuf> = (5..8).map(|i| PathBuf::from(format!("f{i}.jpg"))).collect();
        assert_eq!(Geometry::between(&stack(8, 6, &others), &base), Geometry::Differs);
        // Aligned the same way: the same; shifted by a pixel: not.
        let aligned = |dx: f32| {
            let mut s = stack(8, 6, &paths);
            let shift = nalgebra::Affine2::from_matrix_unchecked(nalgebra::Matrix3::new(
                1.0, 0.0, dx, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0,
            ));
            s.geometry.transforms = vec![Some(FrameAlignment::from_transform(shift)); 3];
            s
        };
        assert_eq!(Geometry::between(&aligned(0.0), &aligned(0.0)), Geometry::Same);
        assert_eq!(Geometry::between(&aligned(1.0), &aligned(0.0)), Geometry::Differs);
    }

    #[test]
    fn paste_takes_the_sliders_and_keeps_the_reframe_and_version() {
        let copied = set().adjustments;
        let onto = RenderAdjustments {
            adjustments: Adjustments { exposure: -1.0, ..Adjustments::NEUTRAL },
            reframe: Reframe { crop: None, turn: Turn::Half, flip: Flip::None },
        };
        let pasted = pasted(&copied, &onto, StackKind::CameraRaw);
        assert_eq!(pasted.reframe, onto.reframe);
        assert_eq!(pasted.adjustments.process(), onto.adjustments.process());
        assert_eq!(pasted.adjustments, copied);
    }

    #[test]
    fn paste_keeps_a_white_balance_of_the_other_scale_out() {
        let kelvin = WhiteBalance::Kelvin { temperature: 5200.0, tint: 8.0 };
        let relative = WhiteBalance::Relative { temperature: 30.0, tint: -5.0 };
        let own = WhiteBalance::Relative { temperature: -10.0, tint: 0.0 };
        let raw_own = WhiteBalance::Kelvin { temperature: 4000.0, tint: 0.0 };
        let cases = [
            (StackKind::Standard, kelvin, own, own),
            (StackKind::Standard, relative, own, relative),
            (StackKind::Standard, WhiteBalance::AsShot, own, WhiteBalance::AsShot),
            (StackKind::CameraRaw, relative, raw_own, raw_own),
            (StackKind::CameraRaw, kelvin, raw_own, kelvin),
            (StackKind::CameraRaw, WhiteBalance::AsShot, raw_own, WhiteBalance::AsShot),
        ];
        for (kind, copied, own, want) in cases {
            let copied =
                Adjustments { white_balance: copied, exposure: 1.0, ..Adjustments::NEUTRAL };
            let onto = RenderAdjustments {
                adjustments: Adjustments { white_balance: own, ..Adjustments::NEUTRAL },
                reframe: Reframe::NONE,
            };
            let got = pasted(&copied, &onto, kind);
            assert_eq!(got.adjustments.white_balance, want, "{kind:?} {copied:?}");
            assert_eq!(got.adjustments.exposure, 1.0);
        }
    }

    #[test]
    fn reset_all_is_neutral_in_the_latest_process_and_keeps_the_reframe() {
        let onto = set();
        let reset = reset_all(&onto);
        assert!(reset.adjustments.is_neutral());
        assert_eq!(reset.adjustments.process(), deptheos_render::ProcessVersion::LATEST);
        assert_eq!(reset.reframe, onto.reframe);
    }
}
