//! A render's turn and flip at the hand-off: every picture an app is
//! handed of a render (the result, Retouch's canvas and its patches, a
//! source frame, the depth map, Adjust's pictures) is turned and flipped
//! as its reframe says, while the crop is the result's alone, drawn by the
//! app as a window (ADR-136, ADR-149). Pointers go back through
//! `view::Framing`.

use deptheos_render::Reframe;
use deptheos_render::reframe::{Flip, Size, Turn};

use crate::engine::present::{CpuImage, PixelRect, PresentedImage};

/// The turn and flip of a `picture`-sized stored picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Orient {
    reframe: Reframe,
    picture: Size,
}

impl Orient {
    /// `reframe`'s turn and flip; its crop is left to the app.
    pub fn of(reframe: Reframe, picture: Size) -> Self {
        Self { reframe: Reframe { crop: None, ..reframe }, picture }
    }

    /// Turned and flipped nowhere.
    pub fn is_none(&self) -> bool {
        self.reframe.flip == Flip::None && self.reframe.turn == Turn::None
    }

    /// The size shown.
    pub fn size(&self) -> Size {
        self.reframe.output_size(self.picture).unwrap_or(self.picture)
    }

    /// The shown rect over stored rect `r`.
    pub fn to_shown(&self, r: PixelRect) -> PixelRect {
        self.map(r, |p| self.reframe.to_output(self.picture, p))
    }

    /// The stored rect under shown rect `r`.
    pub fn to_stored(&self, r: PixelRect) -> PixelRect {
        self.map(r, |p| self.reframe.to_stored(self.picture, p))
    }

    fn map(
        &self,
        r: PixelRect,
        f: impl Fn((f64, f64)) -> Result<(f64, f64), deptheos_render::reframe::OutsidePicture>,
    ) -> PixelRect {
        let corner = |x: u32, y: u32| f((f64::from(x), f64::from(y))).unwrap_or((0.0, 0.0));
        let (a, b) = (corner(r.x, r.y), corner(r.right(), r.bottom()));
        let (x0, x1) = (a.0.min(b.0).round() as u32, a.0.max(b.0).round() as u32);
        let (y0, y1) = (a.1.min(b.1).round() as u32, a.1.max(b.1).round() as u32);
        PixelRect { x: x0, y: y0, width: x1 - x0, height: y1 - y0 }
    }

    /// `image`, a rect of the stored picture (or the whole of it),
    /// turned and flipped.
    pub fn image(&self, image: &CpuImage) -> CpuImage {
        if self.is_none() {
            return image.clone();
        }
        let size = Size { width: image.width, height: image.height };
        let mut pixels = vec![0; image.pixels.len()];
        self.write(&image.pixels, size, &mut pixels);
        let turned = self.turned(size);
        CpuImage { width: turned.width, height: turned.height, pixels }
    }

    /// The size of a `size`-sized part of the stored picture once turned.
    pub fn turned(&self, size: Size) -> Size {
        Reframe { crop: None, ..self.reframe }.output_size(size).unwrap_or(size)
    }

    /// `pixels`, four bytes a pixel of a `size`-sized part of the stored
    /// picture, turned and flipped into `out` (as long).
    pub fn write(&self, pixels: &[u8], size: Size, out: &mut [u8]) {
        let turned = self.turned(size);
        let w = size.width as usize;
        for oy in 0..turned.height as usize {
            for ox in 0..turned.width as usize {
                let (sx, sy) = self.stored_index(size, ox, oy);
                let (from, to) = ((sy * w + sx) * 4, (oy * turned.width as usize + ox) * 4);
                out[to..to + 4].copy_from_slice(&pixels[from..from + 4]);
            }
        }
    }

    /// The pixel of a `size`-sized part drawn at turned pixel `(x, y)`.
    fn stored_index(&self, size: Size, x: usize, y: usize) -> (usize, usize) {
        let (w, h) = (size.width as usize, size.height as usize);
        let (fx, fy) = match self.reframe.turn {
            Turn::None => (x, y),
            Turn::Quarter => (y, h - 1 - x),
            Turn::Half => (w - 1 - x, h - 1 - y),
            Turn::ThreeQuarters => (w - 1 - y, x),
        };
        match self.reframe.flip {
            Flip::None => (fx, fy),
            Flip::Horizontal => (w - 1 - fx, fy),
        }
    }

    /// `image` (of any size) turned and flipped, its levels made again
    /// from its first.
    pub fn presented(&self, image: PresentedImage) -> PresentedImage {
        if self.is_none() {
            return image;
        }
        let Some(first) = image.levels.first() else { return image };
        let first = self.image(&first.assemble());
        let whole = self.turned(Size { width: image.width(), height: image.height() });
        PresentedImage::from_first(
            first,
            image.levels.len(),
            image.first_level,
            whole,
            image.space(),
        )
    }
}

#[cfg(test)]
mod tests {
    use deptheos_render::reframe::Rect;

    use super::*;

    const PICTURE: Size = Size { width: 700, height: 600 };

    fn picture() -> CpuImage {
        let (w, h) = (PICTURE.width, PICTURE.height);
        let pixels = (0..w * h).flat_map(|i| [(i % 251) as u8, (i / 251 % 241) as u8, 7, 255]);
        CpuImage { width: w, height: h, pixels: pixels.collect() }
    }

    fn every() -> Vec<Reframe> {
        let turns = [Turn::None, Turn::Quarter, Turn::Half, Turn::ThreeQuarters];
        let flips = [Flip::None, Flip::Horizontal];
        let crop = Some(Rect { x: 10, y: 20, width: 100, height: 50 });
        turns
            .iter()
            .flat_map(|&turn| flips.iter().map(move |&flip| Reframe { crop, flip, turn }))
            .collect()
    }

    #[test]
    fn a_picture_turned_is_its_pixels_turned_and_its_levels_made_again() {
        for reframe in every() {
            let orient = Orient::of(reframe, PICTURE);
            let whole =
                PresentedImage::from_first(picture(), 3, 0, PICTURE, crate::ScreenSpace::Srgb);
            let turned = orient.presented(whole);
            let want = orient.image(&picture());
            assert_eq!((turned.width(), turned.height()), (want.width, want.height), "{reframe:?}");
            assert!(turned.levels[0].assemble().pixels == want.pixels, "{reframe:?}");
            let again =
                PresentedImage::from_first(want, 3, 0, orient.size(), crate::ScreenSpace::Srgb);
            for (a, b) in turned.levels.iter().zip(&again.levels) {
                assert!(a.assemble().pixels == b.assemble().pixels, "{reframe:?}: levels");
            }
        }
    }

    #[test]
    fn turning_matches_the_reframes_own_mapping() {
        let stored = picture();
        for reframe in every() {
            let orient = Orient::of(reframe, PICTURE);
            let (want, _) =
                Reframe { crop: None, ..reframe }.apply(&stored.pixels, PICTURE, 4).unwrap();
            assert!(orient.image(&stored).pixels == want, "{reframe:?}");
        }
    }

    #[test]
    fn a_rect_turned_holds_its_pixels_turned() {
        let stored = picture();
        let rect = PixelRect { x: 30, y: 40, width: 90, height: 70 };
        let cut = |image: &CpuImage, r: PixelRect| {
            let mut pixels = Vec::new();
            for y in r.y..r.bottom() {
                let row = (y * image.width + r.x) as usize * 4;
                pixels.extend_from_slice(&image.pixels[row..row + r.width as usize * 4]);
            }
            CpuImage { width: r.width, height: r.height, pixels }
        };
        for reframe in every() {
            let orient = Orient::of(reframe, PICTURE);
            let shown = orient.to_shown(rect);
            assert_eq!(orient.to_stored(shown), rect, "{reframe:?}");
            let whole = orient.image(&stored);
            let part = orient.image(&cut(&stored, rect));
            assert!(cut(&whole, shown).pixels == part.pixels, "{reframe:?}");
        }
    }
}
