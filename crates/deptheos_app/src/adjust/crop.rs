//! The crop tool's geometry (ADR-136): the crop as drawn over the
//! uncropped picture (turned and flipped as the render is), its handles,
//! a drag on one under an aspect, and the crop stored from it. Pure, so
//! both apps drag alike.

use deptheos_render::Reframe;
use deptheos_render::reframe::{Rect, Size};

use crate::view::Pane;
use crate::view::framing::Framing;

/// The smallest crop side, in stored pixels.
pub const MIN_SIDE: f64 = 16.0;

/// The crop's shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Aspect {
    /// Any shape.
    Free,
    /// The picture's own, as shown.
    Original,
    /// `w`:`h`, as shown: wide when `w` > `h`.
    Ratio(u32, u32),
}

impl Aspect {
    /// The choices, in the panel's order; each ratio has its turned pair
    /// ([`Self::turned`]).
    pub const CHOICES: [Self; 6] = [
        Self::Original,
        Self::Free,
        Self::Ratio(1, 1),
        Self::Ratio(4, 5),
        Self::Ratio(3, 2),
        Self::Ratio(16, 9),
    ];

    /// The same shape turned: 4:5 is 5:4; Free and Original stay.
    pub fn turned(self) -> Self {
        match self {
            Self::Ratio(w, h) => Self::Ratio(h, w),
            other => other,
        }
    }

    /// The same choice, whichever way it is turned.
    pub fn same_choice(self, other: Self) -> bool {
        self == other || self == other.turned()
    }

    /// Width over height for a picture shown `shown`; `None` for Free.
    pub fn ratio(self, shown: Size) -> Option<f64> {
        match self {
            Self::Free => None,
            Self::Original => Some(f64::from(shown.width) / f64::from(shown.height.max(1))),
            Self::Ratio(w, h) => Some(f64::from(w) / f64::from(h.max(1))),
        }
    }
}

/// What a press on the crop holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
    /// Inside: the crop moves.
    Inside,
}

impl Handle {
    /// Which edges it moves: left, top, right, bottom.
    fn edges(self) -> [bool; 4] {
        match self {
            Self::TopLeft => [true, true, false, false],
            Self::Top => [false, true, false, false],
            Self::TopRight => [false, true, true, false],
            Self::Right => [false, false, true, false],
            Self::BottomRight => [false, false, true, true],
            Self::Bottom => [false, false, false, true],
            Self::BottomLeft => [true, false, false, true],
            Self::Left => [true, false, false, false],
            Self::Inside => [true, true, true, true],
        }
    }

    /// Whether it is a corner.
    fn corner(self) -> bool {
        matches!(self, Self::TopLeft | Self::TopRight | Self::BottomRight | Self::BottomLeft)
    }
}

/// A rectangle on the uncropped picture as shown, in its pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

impl Area {
    pub fn width(&self) -> f64 {
        self.right - self.left
    }

    pub fn height(&self) -> f64 {
        self.bottom - self.top
    }

    fn whole(shown: Size) -> Self {
        Self { left: 0.0, top: 0.0, right: f64::from(shown.width), bottom: f64::from(shown.height) }
    }
}

/// The crop tool's picture: a `picture`-sized stored picture under
/// `reframe`'s turn and flip, uncropped.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CropFrame {
    framing: Framing,
    reframe: Reframe,
}

impl CropFrame {
    pub fn of(picture: Size, reframe: Reframe) -> Self {
        Self { framing: Framing::of(picture, reframe, Pane::Uncropped), reframe }
    }

    /// The uncropped picture's size as shown.
    pub fn shown(&self) -> Size {
        self.framing.size()
    }

    /// The crop as drawn: the whole picture when there is none.
    pub fn area(&self) -> Area {
        let shown = self.shown();
        match self.framing.crop_mark() {
            Some(m) => Area {
                left: f64::from(m.x),
                top: f64::from(m.y),
                right: f64::from(m.x + m.width),
                bottom: f64::from(m.y + m.height),
            },
            None => Area::whole(shown),
        }
    }

    /// The handle under shown point `p`, within `reach` shown pixels of an
    /// edge; inside it otherwise, or `None` outside.
    pub fn handle_at(&self, area: Area, p: (f64, f64), reach: f64) -> Option<Handle> {
        let near = |a: f64, b: f64| (a - b).abs() <= reach;
        let (x, y) = p;
        let inside_x = x >= area.left - reach && x <= area.right + reach;
        let inside_y = y >= area.top - reach && y <= area.bottom + reach;
        if !(inside_x && inside_y) {
            return None;
        }
        let (l, t, r, b) =
            (near(x, area.left), near(y, area.top), near(x, area.right), near(y, area.bottom));
        Some(match (l, t, r, b) {
            (true, true, _, _) => Handle::TopLeft,
            (_, true, true, _) => Handle::TopRight,
            (_, _, true, true) => Handle::BottomRight,
            (true, _, _, true) => Handle::BottomLeft,
            (_, true, _, _) => Handle::Top,
            (_, _, true, _) => Handle::Right,
            (_, _, _, true) => Handle::Bottom,
            (true, _, _, _) => Handle::Left,
            _ => Handle::Inside,
        })
    }

    /// `from` dragged by `handle` through `by` shown pixels, kept on the
    /// picture, at least [`MIN_SIDE`], and of `aspect`'s shape.
    pub fn drag(&self, from: Area, handle: Handle, by: (f64, f64), aspect: Aspect) -> Area {
        let whole = Area::whole(self.shown());
        if handle == Handle::Inside {
            let dx = by.0.clamp(whole.left - from.left, whole.right - from.right);
            let dy = by.1.clamp(whole.top - from.top, whole.bottom - from.bottom);
            return Area {
                left: from.left + dx,
                top: from.top + dy,
                right: from.right + dx,
                bottom: from.bottom + dy,
            };
        }
        let [l, t, r, b] = handle.edges();
        let mut a = from;
        if l {
            a.left = (from.left + by.0).clamp(whole.left, from.right - MIN_SIDE);
        }
        if r {
            a.right = (from.right + by.0).clamp(from.left + MIN_SIDE, whole.right);
        }
        if t {
            a.top = (from.top + by.1).clamp(whole.top, from.bottom - MIN_SIDE);
        }
        if b {
            a.bottom = (from.bottom + by.1).clamp(from.top + MIN_SIDE, whole.bottom);
        }
        let Some(ratio) = aspect.ratio(self.shown()) else { return a };
        // The edge dragged leads; the other side follows, anchored at the
        // opposite corner or edge's middle, shrunk to fit the picture.
        let (mut w, mut h) = (a.width(), a.height());
        if handle.corner() {
            if w / h > ratio { w = h * ratio } else { h = w / ratio }
        } else if l || r {
            h = w / ratio;
        } else {
            w = h * ratio;
        }
        let ax = if l && !r {
            from.right
        } else if r && !l {
            from.left
        } else {
            (from.left + from.right) / 2.0
        };
        let ay = if t && !b {
            from.bottom
        } else if b && !t {
            from.top
        } else {
            (from.top + from.bottom) / 2.0
        };
        // Room from the anchor toward where the crop grows.
        let room_w = match (l, r) {
            (true, false) => ax - whole.left,
            (false, true) => whole.right - ax,
            _ => 2.0 * (ax - whole.left).min(whole.right - ax),
        };
        let room_h = match (t, b) {
            (true, false) => ay - whole.top,
            (false, true) => whole.bottom - ay,
            _ => 2.0 * (ay - whole.top).min(whole.bottom - ay),
        };
        let fit = (room_w / w).min(room_h / h).min(1.0);
        let (w, h) = (w * fit, h * fit);
        let (left, right) = match (l, r) {
            (true, false) => (ax - w, ax),
            (false, true) => (ax, ax + w),
            _ => (ax - w / 2.0, ax + w / 2.0),
        };
        let (top, bottom) = match (t, b) {
            (true, false) => (ay - h, ay),
            (false, true) => (ay, ay + h),
            _ => (ay - h / 2.0, ay + h / 2.0),
        };
        Area { left, top, right, bottom }
    }

    /// The largest area of `aspect`'s shape inside `area`, centered on it:
    /// a shape just chosen.
    pub fn shaped(&self, area: Area, aspect: Aspect) -> Area {
        let Some(ratio) = aspect.ratio(self.shown()) else { return area };
        let (w, h) = (area.width(), area.height());
        let (w, h) = if w / h > ratio { (h * ratio, h) } else { (w, w / ratio) };
        let (cx, cy) = ((area.left + area.right) / 2.0, (area.top + area.bottom) / 2.0);
        Area { left: cx - w / 2.0, top: cy - h / 2.0, right: cx + w / 2.0, bottom: cy + h / 2.0 }
    }

    /// The reframe with `area` as its crop, in whole stored pixels; `None`
    /// for the whole picture.
    pub fn reframed(&self, area: Area) -> Reframe {
        let picture = self.framing.picture();
        let corners =
            [(area.left, area.top), (area.right, area.bottom)].map(|p| self.framing.to_stored(p));
        let x0 = corners[0].0.min(corners[1].0).round().clamp(0.0, f64::from(picture.width));
        let x1 = corners[0].0.max(corners[1].0).round().clamp(0.0, f64::from(picture.width));
        let y0 = corners[0].1.min(corners[1].1).round().clamp(0.0, f64::from(picture.height));
        let y1 = corners[0].1.max(corners[1].1).round().clamp(0.0, f64::from(picture.height));
        let crop =
            Rect { x: x0 as u32, y: y0 as u32, width: (x1 - x0) as u32, height: (y1 - y0) as u32 };
        let whole = crop == Rect { x: 0, y: 0, width: picture.width, height: picture.height };
        Reframe {
            crop: (!whole && crop.width > 0 && crop.height > 0).then_some(crop),
            ..self.reframe
        }
    }
}

#[cfg(test)]
mod tests {
    use deptheos_render::reframe::{Flip, Turn};

    use super::*;

    const PICTURE: Size = Size { width: 400, height: 300 };

    fn every() -> Vec<Reframe> {
        let crop = Some(Rect { x: 40, y: 30, width: 200, height: 120 });
        [Turn::None, Turn::Quarter, Turn::Half, Turn::ThreeQuarters]
            .into_iter()
            .flat_map(|turn| {
                [Flip::None, Flip::Horizontal].map(|flip| Reframe { crop, flip, turn })
            })
            .collect()
    }

    #[test]
    fn the_crop_drawn_is_stored_back_as_it_was() {
        for reframe in every() {
            let frame = CropFrame::of(PICTURE, reframe);
            assert_eq!(frame.reframed(frame.area()), reframe, "{reframe:?}");
        }
        let none = CropFrame::of(PICTURE, Reframe::NONE);
        assert_eq!(none.area(), Area { left: 0.0, top: 0.0, right: 400.0, bottom: 300.0 });
        assert_eq!(none.reframed(none.area()), Reframe::NONE, "the whole picture is no crop");
    }

    #[test]
    fn handles_are_found_at_the_edges_and_corners() {
        let frame = CropFrame::of(PICTURE, Reframe::NONE);
        let a = Area { left: 100.0, top: 100.0, right: 200.0, bottom: 200.0 };
        let at = |x, y| frame.handle_at(a, (x, y), 5.0);
        assert_eq!(at(101.0, 99.0), Some(Handle::TopLeft));
        assert_eq!(at(150.0, 203.0), Some(Handle::Bottom));
        assert_eq!(at(198.0, 150.0), Some(Handle::Right));
        assert_eq!(at(150.0, 150.0), Some(Handle::Inside));
        assert_eq!(at(300.0, 150.0), None);
    }

    #[test]
    fn a_drag_keeps_the_crop_on_the_picture_and_its_shape() {
        let frame = CropFrame::of(PICTURE, Reframe::NONE);
        let a = Area { left: 100.0, top: 100.0, right: 200.0, bottom: 200.0 };
        // Moved past the edge: stopped at it, its size kept.
        let moved = frame.drag(a, Handle::Inside, (500.0, -500.0), Aspect::Free);
        assert_eq!(moved, Area { left: 300.0, top: 0.0, right: 400.0, bottom: 100.0 });
        // A free corner goes where it is dragged, not past the opposite one.
        let free = frame.drag(a, Handle::BottomRight, (50.0, 20.0), Aspect::Free);
        assert_eq!(free, Area { left: 100.0, top: 100.0, right: 250.0, bottom: 220.0 });
        let shrunk = frame.drag(a, Handle::Left, (500.0, 0.0), Aspect::Free);
        assert_eq!(shrunk.width(), MIN_SIDE);
        // Under a ratio, every drag keeps it, anchored opposite, on the picture.
        for aspect in [Aspect::Ratio(3, 2), Aspect::Ratio(4, 5), Aspect::Original] {
            let ratio = aspect.ratio(frame.shown()).unwrap();
            let start = frame.shaped(a, aspect);
            for handle in [Handle::TopLeft, Handle::Right, Handle::Bottom, Handle::BottomRight] {
                for by in [(40.0, 10.0), (-30.0, 60.0), (900.0, 900.0)] {
                    let d = frame.drag(start, handle, by, aspect);
                    assert!(
                        (d.width() / d.height() - ratio).abs() < 1e-6,
                        "{aspect:?} {handle:?} {by:?}: {d:?}"
                    );
                    assert!(d.left >= -1e-9 && d.top >= -1e-9, "{d:?}");
                    assert!(d.right <= 400.0 + 1e-9 && d.bottom <= 300.0 + 1e-9, "{d:?}");
                }
            }
        }
    }

    #[test]
    fn a_ratio_turned_is_its_pair() {
        assert_eq!(Aspect::Ratio(4, 5).turned(), Aspect::Ratio(5, 4));
        assert!(Aspect::Ratio(16, 9).same_choice(Aspect::Ratio(9, 16)));
        assert_eq!(Aspect::Free.turned(), Aspect::Free);
        let turned = CropFrame::of(PICTURE, Reframe { turn: Turn::Quarter, ..Reframe::NONE });
        assert_eq!(Aspect::Original.ratio(turned.shown()), Some(300.0 / 400.0), "as shown");
    }
}
