//! The crop tool's geometry for the app (`deptheos_app::adjust::crop`,
//! ADR-136): the crop over the uncropped picture as shown, its handles, a
//! drag under an aspect, and the reframe stored from it, plus the turns
//! and flips as shown. The app only draws what these say.

use std::sync::Arc;

use deptheos_app::adjust::crop::{self, Area};
use deptheos_render::Reframe;
use deptheos_render::reframe::Size;

/// The crop's shape (`deptheos_app::adjust::crop::Aspect`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum CropAspect {
    Free,
    /// The picture's own, as shown.
    Original,
    /// `width`:`height`, as shown.
    Ratio {
        width: u32,
        height: u32,
    },
}

impl From<CropAspect> for crop::Aspect {
    fn from(a: CropAspect) -> Self {
        match a {
            CropAspect::Free => Self::Free,
            CropAspect::Original => Self::Original,
            CropAspect::Ratio { width, height } => Self::Ratio(width, height),
        }
    }
}

impl From<crop::Aspect> for CropAspect {
    fn from(a: crop::Aspect) -> Self {
        match a {
            crop::Aspect::Free => Self::Free,
            crop::Aspect::Original => Self::Original,
            crop::Aspect::Ratio(width, height) => Self::Ratio { width, height },
        }
    }
}

/// What a press on the crop holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum CropHandle {
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

impl From<crop::Handle> for CropHandle {
    fn from(h: crop::Handle) -> Self {
        match h {
            crop::Handle::TopLeft => Self::TopLeft,
            crop::Handle::Top => Self::Top,
            crop::Handle::TopRight => Self::TopRight,
            crop::Handle::Right => Self::Right,
            crop::Handle::BottomRight => Self::BottomRight,
            crop::Handle::Bottom => Self::Bottom,
            crop::Handle::BottomLeft => Self::BottomLeft,
            crop::Handle::Left => Self::Left,
            crop::Handle::Inside => Self::Inside,
        }
    }
}

impl From<CropHandle> for crop::Handle {
    fn from(h: CropHandle) -> Self {
        match h {
            CropHandle::TopLeft => Self::TopLeft,
            CropHandle::Top => Self::Top,
            CropHandle::TopRight => Self::TopRight,
            CropHandle::Right => Self::Right,
            CropHandle::BottomRight => Self::BottomRight,
            CropHandle::Bottom => Self::Bottom,
            CropHandle::BottomLeft => Self::BottomLeft,
            CropHandle::Left => Self::Left,
            CropHandle::Inside => Self::Inside,
        }
    }
}

/// A rectangle on the uncropped picture as shown, in its pixels.
#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct CropArea {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

impl From<Area> for CropArea {
    fn from(a: Area) -> Self {
        Self { left: a.left, top: a.top, right: a.right, bottom: a.bottom }
    }
}

impl From<CropArea> for Area {
    fn from(a: CropArea) -> Self {
        Self { left: a.left, top: a.top, right: a.right, bottom: a.bottom }
    }
}

/// The crop tool's picture: a stored picture under a reframe's turn and
/// flip, uncropped (`deptheos_app::adjust::crop::CropFrame`).
#[derive(uniffi::Object)]
pub struct CropFrame {
    frame: crop::CropFrame,
}

#[uniffi::export]
impl CropFrame {
    /// A `width` × `height` stored picture under `reframe`.
    #[uniffi::constructor]
    pub fn new(width: u32, height: u32, reframe: Reframe) -> Arc<Self> {
        Arc::new(Self { frame: crop::CropFrame::of(Size { width, height }, reframe) })
    }

    /// The crop as drawn: the whole picture when there is none.
    pub fn area(&self) -> CropArea {
        self.frame.area().into()
    }

    /// The handle under `x`, `y` within `reach` shown pixels, if any.
    pub fn handle_at(&self, area: CropArea, x: f64, y: f64, reach: f64) -> Option<CropHandle> {
        self.frame.handle_at(area.into(), (x, y), reach).map(Into::into)
    }

    /// `from` with `handle` moved by `dx`, `dy` shown pixels under `aspect`.
    pub fn drag(
        &self,
        from: CropArea,
        handle: CropHandle,
        dx: f64,
        dy: f64,
        aspect: CropAspect,
    ) -> CropArea {
        self.frame.drag(from.into(), handle.into(), (dx, dy), aspect.into()).into()
    }

    /// The largest of `aspect`'s shape inside `area`, centered.
    pub fn shaped(&self, area: CropArea, aspect: CropAspect) -> CropArea {
        self.frame.shaped(area.into(), aspect.into()).into()
    }

    /// The reframe that keeps `area`.
    pub fn reframed(&self, area: CropArea) -> Reframe {
        self.frame.reframed(area.into())
    }
}

/// The aspects Crop offers, in order.
#[uniffi::export]
pub fn crop_aspects() -> Vec<CropAspect> {
    crop::Aspect::CHOICES.iter().map(|&a| a.into()).collect()
}

/// The same shape turned: 4:5 is 5:4.
#[uniffi::export]
pub fn crop_aspect_turned(aspect: CropAspect) -> CropAspect {
    crop::Aspect::from(aspect).turned().into()
}

/// `reframe` turned a quarter as shown, right (clockwise) or left.
#[uniffi::export]
pub fn reframe_turned(reframe: Reframe, right: bool) -> Reframe {
    if right { reframe.turned_right() } else { reframe.turned_left() }
}

/// `reframe` mirrored as shown, top to bottom or left to right.
#[uniffi::export]
pub fn reframe_flipped(reframe: Reframe, vertical: bool) -> Reframe {
    if vertical { reframe.flipped_vertically() } else { reframe.flipped_horizontally() }
}
