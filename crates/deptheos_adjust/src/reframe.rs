//! The reframe: a rectangle of the stored picture, then a flip and a
//! quarter-turn, applied last as an output window with no resample (on
//! screen it is Crop). Paint, the frame map and compares stay in stored
//! coordinates; this maps between the two.
//!
//! An angle, when it comes, is a field the rectangle is measured after.

/// A picture's size in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

/// A rectangle of the stored picture, in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Quarter-turns clockwise, after the flip.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Turn {
    #[default]
    None,
    Quarter,
    Half,
    ThreeQuarters,
}

/// A mirror left to right, before the turn. A flip top to bottom is this
/// and a half turn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Flip {
    #[default]
    None,
    Horizontal,
}

/// A render's reframe.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Reframe {
    /// The rectangle kept; `None` keeps the whole picture.
    pub crop: Option<Rect>,
    pub flip: Flip,
    pub turn: Turn,
}

/// A crop that isn't inside the picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutsidePicture {
    pub crop: Rect,
    pub picture: Size,
}

impl Turn {
    fn quarters(self) -> u32 {
        match self {
            Self::None => 0,
            Self::Quarter => 1,
            Self::Half => 2,
            Self::ThreeQuarters => 3,
        }
    }

    fn of_quarters(n: u32) -> Self {
        match n % 4 {
            0 => Self::None,
            1 => Self::Quarter,
            2 => Self::Half,
            _ => Self::ThreeQuarters,
        }
    }

    /// This turn followed by `other`.
    pub fn then(self, other: Self) -> Self {
        Self::of_quarters(self.quarters() + other.quarters())
    }
}

impl Reframe {
    pub const NONE: Self = Self { crop: None, flip: Flip::None, turn: Turn::None };

    pub fn is_none(&self) -> bool {
        *self == Self::NONE
    }

    /// The rectangle kept of a `picture`-sized stored picture.
    pub fn window(&self, picture: Size) -> Result<Rect, OutsidePicture> {
        let whole = Rect { x: 0, y: 0, width: picture.width, height: picture.height };
        let Some(crop) = self.crop else { return Ok(whole) };
        let inside = crop.width > 0
            && crop.height > 0
            && u64::from(crop.x) + u64::from(crop.width) <= u64::from(picture.width)
            && u64::from(crop.y) + u64::from(crop.height) <= u64::from(picture.height);
        if inside { Ok(crop) } else { Err(OutsidePicture { crop, picture }) }
    }

    /// The size drawn from a `picture`-sized stored picture.
    pub fn output_size(&self, picture: Size) -> Result<Size, OutsidePicture> {
        let w = self.window(picture)?;
        Ok(match self.turn {
            Turn::None | Turn::Half => Size { width: w.width, height: w.height },
            Turn::Quarter | Turn::ThreeQuarters => Size { width: w.height, height: w.width },
        })
    }

    /// The stored point under output point `(x, y)`, both continuous (a
    /// pixel's center is at its index plus a half).
    pub fn to_stored(
        &self,
        picture: Size,
        (x, y): (f64, f64),
    ) -> Result<(f64, f64), OutsidePicture> {
        let w = self.window(picture)?;
        let (cw, ch) = (f64::from(w.width), f64::from(w.height));
        // Undo the turn, into the flipped window.
        let (fx, fy) = match self.turn {
            Turn::None => (x, y),
            Turn::Quarter => (y, ch - x),
            Turn::Half => (cw - x, ch - y),
            Turn::ThreeQuarters => (cw - y, x),
        };
        let wx = match self.flip {
            Flip::None => fx,
            Flip::Horizontal => cw - fx,
        };
        Ok((wx + f64::from(w.x), fy + f64::from(w.y)))
    }

    /// The output point over stored point `(x, y)`: the inverse of
    /// [`Self::to_stored`].
    pub fn to_output(
        &self,
        picture: Size,
        (x, y): (f64, f64),
    ) -> Result<(f64, f64), OutsidePicture> {
        let w = self.window(picture)?;
        let (cw, ch) = (f64::from(w.width), f64::from(w.height));
        let (wx, wy) = (x - f64::from(w.x), y - f64::from(w.y));
        let fx = match self.flip {
            Flip::None => wx,
            Flip::Horizontal => cw - wx,
        };
        Ok(match self.turn {
            Turn::None => (fx, wy),
            Turn::Quarter => (ch - wy, fx),
            Turn::Half => (cw - fx, ch - wy),
            Turn::ThreeQuarters => (wy, cw - fx),
        })
    }

    /// The stored pixel drawn at output pixel `(x, y)`.
    fn stored_pixel(&self, w: Rect, (x, y): (u32, u32)) -> (u32, u32) {
        let (fx, fy) = match self.turn {
            Turn::None => (x, y),
            Turn::Quarter => (y, w.height - 1 - x),
            Turn::Half => (w.width - 1 - x, w.height - 1 - y),
            Turn::ThreeQuarters => (w.width - 1 - y, x),
        };
        let wx = match self.flip {
            Flip::None => fx,
            Flip::Horizontal => w.width - 1 - fx,
        };
        (wx + w.x, fy + w.y)
    }

    /// `pixels` (a `picture`-sized stored picture of `channels` values a
    /// pixel, rows top first) as this reframe draws them, with their size.
    pub fn apply<T: Copy>(
        &self,
        pixels: &[T],
        picture: Size,
        channels: usize,
    ) -> Result<(Vec<T>, Size), OutsidePicture> {
        let w = self.window(picture)?;
        let size = self.output_size(picture)?;
        let stride = picture.width as usize * channels;
        let mut out = Vec::with_capacity(size.width as usize * size.height as usize * channels);
        for y in 0..size.height {
            for x in 0..size.width {
                let (sx, sy) = self.stored_pixel(w, (x, y));
                let at = sy as usize * stride + sx as usize * channels;
                out.extend_from_slice(&pixels[at..at + channels]);
            }
        }
        Ok((out, size))
    }

    /// The EXIF and DNG Orientation that shows the window as drawn.
    pub fn orientation(&self) -> u16 {
        match (self.flip, self.turn) {
            (Flip::None, Turn::None) => 1,
            (Flip::Horizontal, Turn::None) => 2,
            (Flip::None, Turn::Half) => 3,
            (Flip::Horizontal, Turn::Half) => 4,
            (Flip::Horizontal, Turn::ThreeQuarters) => 5,
            (Flip::None, Turn::Quarter) => 6,
            (Flip::Horizontal, Turn::Quarter) => 7,
            (Flip::None, Turn::ThreeQuarters) => 8,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PICTURE: Size = Size { width: 7, height: 5 };

    fn every() -> Vec<Reframe> {
        let crops = [None, Some(Rect { x: 2, y: 1, width: 4, height: 3 })];
        let turns = [Turn::None, Turn::Quarter, Turn::Half, Turn::ThreeQuarters];
        let mut all = Vec::new();
        for crop in crops {
            for flip in [Flip::None, Flip::Horizontal] {
                for turn in turns {
                    all.push(Reframe { crop, flip, turn });
                }
            }
        }
        all
    }

    /// Each pixel holds its own stored index.
    fn indexed() -> Vec<u32> {
        (0..PICTURE.width * PICTURE.height).collect()
    }

    #[test]
    fn none_draws_the_picture_as_it_is() {
        let (out, size) = Reframe::NONE.apply(&indexed(), PICTURE, 1).unwrap();
        assert_eq!((out, size), (indexed(), PICTURE));
        assert!(Reframe::default().is_none());
    }

    /// The pixel drawn at each output position is the stored pixel whose
    /// center the point mapping sends there, both ways.
    #[test]
    fn the_point_mapping_and_the_window_agree() {
        for reframe in every() {
            let (out, size) = reframe.apply(&indexed(), PICTURE, 1).unwrap();
            assert_eq!(out.len(), (size.width * size.height) as usize);
            for y in 0..size.height {
                for x in 0..size.width {
                    let center = (f64::from(x) + 0.5, f64::from(y) + 0.5);
                    let (sx, sy) = reframe.to_stored(PICTURE, center).unwrap();
                    let stored = (sy.floor() as u32) * PICTURE.width + sx.floor() as u32;
                    assert_eq!(
                        out[(y * size.width + x) as usize],
                        stored,
                        "{reframe:?} ({x}, {y})"
                    );
                    let back = reframe.to_output(PICTURE, (sx, sy)).unwrap();
                    assert!((back.0 - center.0).abs() < 1e-12 && (back.1 - center.1).abs() < 1e-12);
                }
            }
        }
    }

    #[test]
    fn a_turn_swaps_the_sides_and_a_flip_mirrors() {
        let turned = Reframe { turn: Turn::Quarter, ..Reframe::NONE };
        let (out, size) = turned.apply(&indexed(), PICTURE, 1).unwrap();
        assert_eq!(size, Size { width: 5, height: 7 });
        // Clockwise: the stored bottom-left corner lands at the top left.
        assert_eq!(out[0], 4 * 7);
        let flipped = Reframe { flip: Flip::Horizontal, ..Reframe::NONE };
        assert_eq!(flipped.apply(&indexed(), PICTURE, 1).unwrap().0[0], 6);
        // A pixel's channels move together.
        let rgb: Vec<u16> = (0..7 * 5 * 3).collect();
        let (out, _) = turned.apply(&rgb, PICTURE, 3).unwrap();
        assert_eq!(&out[..3], &[84, 85, 86]);
    }

    #[test]
    fn a_crop_outside_the_picture_is_refused() {
        for crop in [
            Rect { x: 0, y: 0, width: 8, height: 5 },
            Rect { x: 4, y: 0, width: 4, height: 1 },
            Rect { x: 0, y: 0, width: 0, height: 5 },
            Rect { x: u32::MAX, y: 0, width: 2, height: 2 },
        ] {
            let reframe = Reframe { crop: Some(crop), ..Reframe::NONE };
            assert_eq!(reframe.window(PICTURE), Err(OutsidePicture { crop, picture: PICTURE }));
        }
    }

    #[test]
    fn every_flip_and_turn_has_its_own_orientation() {
        let mut seen: Vec<u16> = every().iter().take(8).map(Reframe::orientation).collect();
        seen.sort();
        assert_eq!(seen, (1..=8).collect::<Vec<_>>());
        assert_eq!(Turn::ThreeQuarters.then(Turn::Half), Turn::Quarter);
    }
}
