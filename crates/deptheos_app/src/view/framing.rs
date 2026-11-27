//! How a pane shows the stored picture under a render's reframe (ADR-149):
//! the result reframed, every other view turned and flipped as the result
//! is but uncropped, the crop marked where it applies. Pointers map back
//! to stored pixels through it, so Sharpest Frame, paint and the frame map
//! stay in stored coordinates, and switching between the two keeps the
//! stored point under the pointer.

use deptheos_render::Reframe;
use deptheos_render::reframe::{Rect, Size};

use crate::runs::session::CompletedStack;

/// What a pane shows of a render.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    /// The result in the viewer or Adjust: adjusted and reframed.
    Result,
    /// The result in Adjust's crop tool, or either side of a compare:
    /// adjusted, uncropped, the crop marked.
    Uncropped,
    /// Retouch's canvas: the stored picture with paint, no adjustments,
    /// uncropped, the crop marked.
    Retouch,
    /// A source frame or the depth map: the stored picture, uncropped.
    Stored,
}

impl Pane {
    /// Whether it draws the render's adjustments.
    pub fn adjusted(self) -> bool {
        matches!(self, Self::Result | Self::Uncropped)
    }

    /// Whether the crop is drawn as a window rather than marked.
    fn cropped(self) -> bool {
        self == Self::Result
    }

    /// Whether the crop is marked over it.
    fn marks_crop(self) -> bool {
        matches!(self, Self::Uncropped | Self::Retouch)
    }
}

/// A pane's picture: the stored picture (`picture` pixels) through the
/// part of `reframe` the pane draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Framing {
    picture: Size,
    /// The render's reframe, a crop outside the picture dropped.
    reframe: Reframe,
    pane: Pane,
}

impl Framing {
    pub fn of(picture: Size, reframe: Reframe, pane: Pane) -> Self {
        let reframe = match reframe.window(picture) {
            Ok(_) => reframe,
            Err(outside) => {
                tracing::warn!("crop {:?} outside {:?}: shown uncropped", outside.crop, picture);
                Reframe { crop: None, ..reframe }
            }
        };
        Self { picture, reframe, pane }
    }

    pub fn pane(&self) -> Pane {
        self.pane
    }

    /// What is applied at the hand-off: the crop only for the result.
    pub fn drawn(&self) -> Reframe {
        if self.pane.cropped() { self.reframe } else { Reframe { crop: None, ..self.reframe } }
    }

    /// The size shown.
    pub fn size(&self) -> Size {
        self.drawn().output_size(self.picture).unwrap_or(self.picture)
    }

    /// The stored point under shown point `p` (continuous coordinates).
    pub fn to_stored(&self, p: (f64, f64)) -> (f64, f64) {
        self.drawn().to_stored(self.picture, p).unwrap_or(p)
    }

    /// The shown point over stored point `p`.
    pub fn to_shown(&self, p: (f64, f64)) -> (f64, f64) {
        self.drawn().to_output(self.picture, p).unwrap_or(p)
    }

    /// The stored pixel under shown point `p`, if `p` is on the picture.
    pub fn stored_pixel(&self, p: (f64, f64)) -> Option<(u32, u32)> {
        let size = self.size();
        let on = (0.0..f64::from(size.width)).contains(&p.0)
            && (0.0..f64::from(size.height)).contains(&p.1);
        if !on {
            return None;
        }
        let (x, y) = self.to_stored(p);
        let (x, y) = (x.floor(), y.floor());
        let inside = x >= 0.0
            && y >= 0.0
            && x < f64::from(self.picture.width)
            && y < f64::from(self.picture.height);
        inside.then_some((x as u32, y as u32))
    }

    /// The crop marked over the picture, in shown coordinates (left, top,
    /// width, height); `None` where it isn't marked or there is none.
    pub fn crop_mark(&self) -> Option<Rect> {
        let crop = self.reframe.crop.filter(|_| self.pane.marks_crop())?;
        let corners = [(crop.x, crop.y), (crop.x + crop.width, crop.y + crop.height)]
            .map(|(x, y)| self.to_shown((f64::from(x), f64::from(y))));
        let (x0, x1) = (corners[0].0.min(corners[1].0), corners[0].0.max(corners[1].0));
        let (y0, y1) = (corners[0].1.min(corners[1].1), corners[0].1.max(corners[1].1));
        Some(Rect { x: x0 as u32, y: y0 as u32, width: (x1 - x0) as u32, height: (y1 - y0) as u32 })
    }

    /// The frame with the largest share of the stored pixel under shown
    /// point `p` (Sharpest Frame), an index into `stack.paths`.
    pub fn frame_at(&self, stack: &CompletedStack, p: (f64, f64)) -> Option<usize> {
        let (x, y) = self.stored_pixel(p)?;
        stack.frame_at(x, y)
    }
}

/// Where a pane's picture sits on screen: screen pixels per shown pixel,
/// and the screen point of the picture's top left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub zoom: f64,
    pub origin: (f64, f64),
}

impl Placement {
    /// The shown point under screen point `s`.
    pub fn shown_at(&self, s: (f64, f64)) -> (f64, f64) {
        ((s.0 - self.origin.0) / self.zoom, (s.1 - self.origin.1) / self.zoom)
    }

    /// The placement of `to` at the same zoom that keeps the stored point
    /// under screen point `s` where `from` (this placement) had it: the
    /// switch between the reframed and the uncropped view.
    pub fn keeping(&self, from: &Framing, to: &Framing, s: (f64, f64)) -> Self {
        let stored = from.to_stored(self.shown_at(s));
        let (x, y) = to.to_shown(stored);
        Self { zoom: self.zoom, origin: (s.0 - x * self.zoom, s.1 - y * self.zoom) }
    }
}

#[cfg(test)]
mod tests {
    use deptheos_render::reframe::{Flip, Turn};

    use super::*;

    const PICTURE: Size = Size { width: 40, height: 30 };
    const CROP: Rect = Rect { x: 10, y: 5, width: 20, height: 12 };
    const TURNED: Reframe =
        Reframe { crop: Some(CROP), flip: Flip::Horizontal, turn: Turn::Quarter };

    #[test]
    fn each_pane_draws_its_part_of_the_reframe() {
        let rows = [
            // pane, adjusted, size, crop marked
            (Pane::Result, true, (12, 20), None),
            (Pane::Uncropped, true, (30, 40), Some(())),
            (Pane::Retouch, false, (30, 40), Some(())),
            (Pane::Stored, false, (30, 40), None),
        ];
        for (pane, adjusted, (w, h), marked) in rows {
            let f = Framing::of(PICTURE, TURNED, pane);
            assert_eq!(pane.adjusted(), adjusted, "{pane:?}");
            assert_eq!(f.size(), Size { width: w, height: h }, "{pane:?}");
            assert_eq!(f.crop_mark().map(|_| ()), marked, "{pane:?}");
            assert_eq!(f.drawn().turn, Turn::Quarter, "{pane:?}: turned in every view");
            assert_eq!(f.drawn().flip, Flip::Horizontal, "{pane:?}: flipped in every view");
        }
    }

    #[test]
    fn the_crop_mark_covers_the_crop_turned() {
        let f = Framing::of(PICTURE, TURNED, Pane::Uncropped);
        let mark = f.crop_mark().unwrap();
        assert_eq!((mark.width, mark.height), (12, 20), "turned a quarter");
        // Every corner of the mark maps back onto the crop's.
        let back = |x: u32, y: u32| f.to_stored((f64::from(x), f64::from(y)));
        let (a, b) = (back(mark.x, mark.y), back(mark.x + mark.width, mark.y + mark.height));
        let xs = (a.0.min(b.0), a.0.max(b.0));
        let ys = (a.1.min(b.1), a.1.max(b.1));
        assert_eq!(xs, (10.0, 30.0));
        assert_eq!(ys, (5.0, 17.0));
    }

    #[test]
    fn a_pointer_maps_back_to_its_stored_pixel() {
        for pane in [Pane::Result, Pane::Uncropped, Pane::Retouch, Pane::Stored] {
            let f = Framing::of(PICTURE, TURNED, pane);
            let size = f.size();
            for y in 0..size.height {
                for x in 0..size.width {
                    let p = (f64::from(x) + 0.5, f64::from(y) + 0.5);
                    let (sx, sy) = f.stored_pixel(p).unwrap();
                    let back = f.to_shown((f64::from(sx) + 0.5, f64::from(sy) + 0.5));
                    assert_eq!(back, p, "{pane:?} at {p:?}");
                }
            }
            assert_eq!(f.stored_pixel((-0.5, 0.0)), None);
            assert_eq!(f.stored_pixel((f64::from(size.width), 0.0)), None);
        }
        // Turned a quarter clockwise, the result's top left is the flipped
        // crop's bottom left: the crop's bottom right.
        let f = Framing::of(PICTURE, TURNED, Pane::Result);
        assert_eq!(f.stored_pixel((0.5, 0.5)), Some((29, 16)));
    }

    #[test]
    fn sharpest_frame_picks_the_frame_under_a_turned_flipped_pointer() {
        // A 4 × 2 map: frame 0 on the left column, frame 1 elsewhere.
        let mut stack = CompletedStack::srgb(
            4,
            2,
            vec![0; 4 * 2 * 3],
            ["a.png", "b.png"].map(std::path::PathBuf::from).to_vec(),
        );
        let map = vec![0, 65535, 65535, 65535, 0, 65535, 65535, 65535];
        stack.frame_map = Some(crate::Gray16Image::from_raw(4, 2, map).unwrap());
        let picture = Size { width: 4, height: 2 };
        let reframe = Reframe { crop: None, flip: Flip::Horizontal, turn: Turn::Quarter };
        let f = Framing::of(picture, reframe, Pane::Result);
        assert_eq!(f.size(), Size { width: 2, height: 4 });
        // Flipped, the left column is on the right; turned a quarter
        // clockwise, the right column is the bottom row.
        for x in 0..2 {
            let bottom = (f64::from(x) + 0.5, 3.5);
            assert_eq!(f.frame_at(&stack, bottom), Some(0), "bottom row, x {x}");
            let top = (f64::from(x) + 0.5, 0.5);
            assert_eq!(f.frame_at(&stack, top), Some(1), "top row, x {x}");
        }
        assert_eq!(f.frame_at(&stack, (2.5, 0.5)), None, "off the picture");
    }

    #[test]
    fn switching_views_keeps_the_stored_point_under_the_pointer() {
        let reframed = Framing::of(PICTURE, TURNED, Pane::Result);
        let uncropped = Framing::of(PICTURE, TURNED, Pane::Uncropped);
        let start = Placement { zoom: 3.0, origin: (100.0, 50.0) };
        for s in [(100.0, 50.0), (130.5, 91.25), (160.0, 110.0)] {
            let stored = reframed.to_stored(start.shown_at(s));
            let there = start.keeping(&reframed, &uncropped, s);
            assert_eq!(there.zoom, 3.0);
            let after = uncropped.to_stored(there.shown_at(s));
            assert!((after.0 - stored.0).abs() < 1e-9 && (after.1 - stored.1).abs() < 1e-9);
            // And back again lands where it started.
            let back = there.keeping(&uncropped, &reframed, s);
            assert!((back.origin.0 - start.origin.0).abs() < 1e-9);
            assert!((back.origin.1 - start.origin.1).abs() < 1e-9);
        }
    }

    #[test]
    fn a_crop_outside_the_picture_shows_uncropped() {
        let outside = Reframe { crop: Some(Rect { x: 30, y: 0, width: 20, height: 10 }), ..TURNED };
        let f = Framing::of(PICTURE, outside, Pane::Result);
        assert_eq!(f.size(), Size { width: 30, height: 40 });
        assert_eq!(f.crop_mark(), None);
    }
}
