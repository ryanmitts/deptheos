//! The render's reframe in the viewer (ADR-149): every picture comes
//! turned and flipped from the engine, the result's crop is a window over
//! the uncropped picture, the crop is marked where the pane marks it, and
//! pointers map back to stored pixels through `Framing`.

use deptheos_app::adjust::preview::Region;
use deptheos_app::engine::orient::Orient;
use deptheos_app::engine::present::PixelRect;
use deptheos_app::reframe::{Rect, Reframe, Size};
use deptheos_app::view::Pane;
use deptheos_app::view::framing::{Framing, Placement};
use gpui::{Bounds, Context, Pixels, point, px, size};

use super::{MapPicture, Shown, Viewer};

/// How the pictures held are framed, as last laid out.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Framed {
    /// The size of the pictures held: the stored picture turned.
    pub(super) shown: (u32, u32),
    /// The pane's framing, while the pictures held are of the render's
    /// stored picture turned as its reframe says.
    pub(super) framing: Option<Framing>,
    /// The result's crop, in the held pictures' pixels, drawn as a window.
    pub(super) window: Option<Rect>,
}

impl Viewer {
    /// The held pictures' pixels to stored ones.
    fn uncropped(&self) -> Option<Framing> {
        let f = self.framed.framing?;
        Some(Framing::of(f.picture(), self.reframe, Pane::Stored))
    }

    /// The render shown is `picture` stored pixels under `reframe`; the
    /// pictures handed after are turned and flipped as it says.
    pub fn set_reframe(&mut self, picture: Option<Size>, reframe: Reframe, cx: &mut Context<Self>) {
        let reframe = picture.map_or(Reframe::NONE, |_| reframe);
        if (self.picture, self.reframe) != (picture, reframe) {
            tracing::debug!("viewer reframe: {reframe:?} of {picture:?}");
            let size = picture.unwrap_or(Size { width: 1, height: 1 });
            let turned = Orient::of(self.reframe, size) != Orient::of(reframe, size);
            (self.picture, self.reframe) = (picture, reframe);
            if turned {
                self.turned(cx);
            }
            cx.notify();
        }
    }

    /// The render shown, now under `reframe` (Adjust's value).
    pub fn reframe_shown(&mut self, reframe: Reframe, cx: &mut Context<Self>) {
        self.set_reframe(self.picture, reframe, cx);
    }

    /// The stored size of the render shown.
    pub fn picture(&self) -> Option<Size> {
        self.picture
    }

    /// How loads turn and flip what they make, as the render shown is.
    pub(super) fn orient(&self) -> Orient {
        match self.picture {
            Some(picture) => Orient::of(self.reframe, picture),
            None => Orient::of(Reframe::NONE, Size { width: 1, height: 1 }),
        }
    }

    /// The pane's framing, the window and the view's image size for
    /// pictures `shown` large: the view keeps the stored point at its
    /// center when only the framing changed.
    pub(super) fn frame(&mut self, shown: (u32, u32)) -> (u32, u32) {
        let pane = self.state.pane();
        let framing = self.picture.map(|p| Framing::of(p, self.reframe, pane)).filter(|f| {
            let s = Framing::of(f.picture(), self.reframe, Pane::Stored).size();
            (s.width, s.height) == shown
        });
        let window = framing
            .filter(|f| f.pane() == Pane::Result)
            .and_then(|f| Framing::of(f.picture(), self.reframe, Pane::Uncropped).crop_mark());
        let to = window.map_or(shown, |w| (w.width, w.height));
        let from = self.framed;
        self.framed = Framed { shown, framing, window };
        let view = &mut self.view;
        if from.framing == framing && (view.image_w, view.image_h) == to {
            return to;
        }
        match (from.framing, framing) {
            (Some(a), Some(b)) if a.picture() == b.picture() && !view.is_fitted() => {
                let s = view.transform.scale as f64;
                let c = view.pane_center();
                let at = Placement {
                    zoom: s,
                    origin: (-view.transform.x as f64 * s, -view.transform.y as f64 * s),
                };
                let kept = at.keeping(&a, &b, (c.0 as f64, c.1 as f64));
                let (x, y) = (-kept.origin.0 / s, -kept.origin.1 / s);
                view.place(to.0, to.1, x as f32, y as f32);
            }
            _ if (view.image_w, view.image_h) != to || from.framing != framing => {
                view.set_image_size(to.0, to.1);
            }
            _ => {}
        }
        to
    }

    /// The crop marked over a pane whose whole picture lies at `rect`.
    pub(super) fn crop_mark(&self, rect: Bounds<Pixels>) -> Option<Bounds<Pixels>> {
        let mark = self.framed.framing?.crop_mark()?;
        Some(self.shown_rect(rect, mark))
    }

    /// Shown rect `r` on screen, the whole picture lying at `rect`.
    pub(super) fn shown_rect(&self, rect: Bounds<Pixels>, r: Rect) -> Bounds<Pixels> {
        let per = self.per_pixel(rect);
        Bounds {
            origin: point(
                rect.origin.x + px(r.x as f32 * per),
                rect.origin.y + px(r.y as f32 * per),
            ),
            size: size(px(r.width as f32 * per), px(r.height as f32 * per)),
        }
    }

    /// Logical pixels per shown pixel of a whole picture at `rect`.
    pub(super) fn per_pixel(&self, rect: Bounds<Pixels>) -> f32 {
        f32::from(rect.size.width) / self.framed.shown.0.max(1) as f32
    }

    /// The stored point under shown point `p`.
    pub(super) fn to_stored(&self, p: (f32, f32)) -> (f32, f32) {
        match self.uncropped() {
            Some(f) => {
                let (x, y) = f.to_stored((f64::from(p.0), f64::from(p.1)));
                (x as f32, y as f32)
            }
            None => p,
        }
    }

    /// The stored pixel under shown point `p`, if on the picture.
    pub(super) fn stored_pixel(&self, p: (f32, f32)) -> Option<(u32, u32)> {
        let p64 = (f64::from(p.0), f64::from(p.1));
        match self.uncropped() {
            Some(f) => f.stored_pixel(p64),
            None => {
                let (w, h) = self.framed.shown;
                ((0.0..w as f32).contains(&p.0) && (0.0..h as f32).contains(&p.1))
                    .then_some((p.0 as u32, p.1 as u32))
            }
        }
    }

    /// The turn or flip changed: what was turned the old way goes, made
    /// again the new way (the result comes from the app).
    fn turned(&mut self, cx: &mut Context<Self>) {
        self.atlas.replace(&mut self.lit_map, None);
        if let MapPicture::Held(_, Some(stack)) = &self.map {
            let stack = stack.clone();
            self.set_map(MapPicture::Off(stack));
        }
        self.make_map(cx);
        match self.shown.clone() {
            Some(Shown { file, name, source, labels }) => {
                self.load(file, &name, source, labels, cx)
            }
            None => self.drop_compare(),
        }
    }

    /// Region `r` of the turned picture, in stored pixels.
    pub(super) fn stored_region(&self, r: Region) -> Region {
        let r = PixelRect { x: r.x, y: r.y, width: r.width, height: r.height };
        let PixelRect { x, y, width, height } = self.orient().to_stored(r);
        Region { x, y, width, height }
    }

    /// The reframe of the render shown.
    #[cfg(test)]
    pub fn reframe_of_shown(&self) -> Reframe {
        self.reframe
    }

    /// The size of the pictures held, as last laid out.
    #[cfg(test)]
    pub fn image_shown(&self) -> (u32, u32) {
        self.framed.shown
    }

    /// Whether the crop is marked, as last laid out.
    #[cfg(test)]
    pub fn crop_marked(&self) -> bool {
        self.framed.framing.and_then(|f| f.crop_mark()).is_some()
    }

    /// The result's crop drawn as a window, as last laid out.
    #[cfg(test)]
    pub fn crop_window(&self) -> Option<Rect> {
        self.framed.window
    }
}
