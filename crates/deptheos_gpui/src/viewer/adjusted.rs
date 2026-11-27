//! The viewer in Adjust: a drag's preview as one flat picture over the
//! settled tiles, the settled picture taking the tiles' place, the
//! picture before its adjustments while held, and what the viewport shows
//! for the session's next draw (ADR-149).

use deptheos_app::PresentedImage;
use deptheos_app::adjust::preview::{Region, Shown};
use deptheos_app::view::{Hold, Picture};
use gpui::{Bounds, Context, Pixels, point, px, size};

use super::{ViewEvent, Viewer, ViewerEvent};
use crate::adjust::Flat;
use crate::images::TiledImage;

impl Viewer {
    /// A preview tick or the view on screen settled: shown over the tiles,
    /// the one it replaces leaving GPUI's atlas.
    pub fn show_flat(&mut self, flat: Flat, cx: &mut Context<Self>) {
        if self.state.adjusting().is_none() {
            return;
        }
        if let Some(old) = self.flat.replace(flat) {
            self.atlas.let_go(old.image);
        }
        self.on(ViewEvent::Previewed, cx);
    }

    /// The flat picture goes (Adjust left).
    pub fn clear_flat(&mut self, cx: &mut Context<Self>) {
        if let Some(old) = self.flat.take() {
            self.atlas.let_go(old.image);
            cx.notify();
        }
    }

    /// The full picture under the value settled: the tiles, in place of
    /// the flat picture.
    pub fn settle(&mut self, picture: PresentedImage, cx: &mut Context<Self>) {
        if self.state.adjusting().is_none() {
            return;
        }
        self.atlas.replace(&mut self.primary, Some(TiledImage::from_presented(picture)));
        if let Some(old) = self.flat.take() {
            self.atlas.let_go(old.image);
        }
        self.on(ViewEvent::Settled, cx);
    }

    /// The result drawn under its saved value as Adjust left, in place of
    /// the one shown.
    pub fn show_drawn(&mut self, picture: PresentedImage, cx: &mut Context<Self>) {
        self.atlas.replace(&mut self.primary, Some(TiledImage::from_presented(picture)));
        cx.notify();
    }

    /// The render without its adjustments, drawn while Before is held;
    /// `None` drops it.
    pub fn show_before(&mut self, before: Option<TiledImage>, cx: &mut Context<Self>) {
        self.atlas.replace(&mut self.before, before);
        cx.notify();
    }

    /// Before held (`true`) or let go.
    pub fn hold_before(&mut self, held: bool, cx: &mut Context<Self>) {
        let holding = self.state.adjusting().is_some_and(|a| a.hold == Hold::Before);
        if self.state.adjusting().is_some() && holding != held {
            self.on(ViewEvent::HoldBefore(held), cx);
        }
    }

    /// Whether Before is held up.
    pub fn holding_before(&self) -> bool {
        self.state.adjusting().is_some_and(|a| a.hold == Hold::Before)
    }

    /// The picture before its adjustments is ready to show.
    #[cfg(test)]
    pub fn has_before(&self) -> bool {
        self.before.is_some()
    }

    /// The flat picture drawn now, if any.
    pub fn flat(&self) -> Option<&Flat> {
        self.flat.as_ref().filter(|_| {
            self.state
                .adjusting()
                .is_some_and(|a| a.picture == Picture::Preview && a.hold == Hold::Released)
        })
    }

    /// Which picture the tiles hold (tests): the same while previews land.
    #[cfg(test)]
    pub fn primary_identity(&self) -> Option<usize> {
        self.primary.as_ref().map(TiledImage::identity)
    }

    /// What the viewport last showed of the stored picture, in stored
    /// pixels.
    pub fn shown_view(&self) -> Option<Shown> {
        self.adjust_view
    }

    /// The flat picture's rect, `rect` being where the whole picture lies;
    /// it covers the turned picture's pixels.
    pub(super) fn flat_rect(&self, flat: &Flat, rect: Bounds<Pixels>) -> Bounds<Pixels> {
        let per = self.per_pixel(rect);
        let Region { x, y, width, height } = flat.covers;
        Bounds {
            origin: point(rect.origin.x + px(x as f32 * per), rect.origin.y + px(y as f32 * per)),
            size: size(px(width as f32 * per), px(height as f32 * per)),
        }
    }

    /// After a layout: what pane `clip` shows of the picture at `rect`,
    /// told to the app when it changed in Adjust.
    pub(super) fn sync_adjust_view(
        &mut self,
        clip: Bounds<Pixels>,
        rect: Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let shown = shown_of(clip, rect, self.framed.shown, self.view.device_scale())
            .map(|s| Shown { visible: self.stored_region(s.visible), ..s });
        if shown.is_none() || shown == self.adjust_view {
            return;
        }
        self.adjust_view = shown;
        if let (Some(shown), Some(_)) = (shown, self.state.adjusting()) {
            cx.emit(ViewerEvent::Shown(shown));
        }
    }
}

/// What `clip` shows of a `image`-sized picture drawn at `rect`, at
/// `device_scale` screen pixels per stored pixel.
pub(super) fn shown_of(
    clip: Bounds<Pixels>,
    rect: Bounds<Pixels>,
    image: (u32, u32),
    device_scale: f32,
) -> Option<Shown> {
    let (w, h) = image;
    if w == 0 || h == 0 || f32::from(rect.size.width) <= 0.0 {
        return None;
    }
    let per = f32::from(rect.size.width) / w as f32;
    let seen = clip.intersect(&rect);
    let at =
        |p: Pixels, origin: Pixels, len: u32| (f32::from(p - origin) / per).clamp(0.0, len as f32);
    let (x0, y0) = (at(seen.origin.x, rect.origin.x, w), at(seen.origin.y, rect.origin.y, h));
    let (x1, y1) = (at(seen.right(), rect.origin.x, w), at(seen.bottom(), rect.origin.y, h));
    let (x, y) = (x0.floor() as u32, y0.floor() as u32);
    let width = (x1.ceil() as u32).saturating_sub(x).max(1).min(w - x.min(w - 1));
    let height = (y1.ceil() as u32).saturating_sub(y).max(1).min(h - y.min(h - 1));
    Some(Shown {
        zoom: device_scale,
        visible: Region { x: x.min(w - 1), y: y.min(h - 1), width, height },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds { origin: point(px(x), px(y)), size: size(px(w), px(h)) }
    }

    #[test]
    fn a_fitted_picture_shows_whole() {
        let shown =
            shown_of(b(0.0, 0.0, 800.0, 600.0), b(100.0, 0.0, 600.0, 600.0), (1200, 1200), 1.0)
                .unwrap();
        assert_eq!(shown.visible, Region { x: 0, y: 0, width: 1200, height: 1200 });
    }

    #[test]
    fn zoomed_in_shows_the_part_on_screen() {
        // At 100%: the picture 2000 wide, its left 400 off screen.
        let shown = shown_of(
            b(0.0, 0.0, 800.0, 600.0),
            b(-400.0, -100.0, 2000.0, 1000.0),
            (2000, 1000),
            2.0,
        )
        .unwrap();
        assert_eq!(shown.visible, Region { x: 400, y: 100, width: 800, height: 600 });
        assert_eq!(shown.zoom, 2.0);
    }
}
