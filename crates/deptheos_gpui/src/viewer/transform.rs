//! View transform controller: zoom, pan, fit-to-view.
//!
//! Positions are pane-local logical pixels unless a doc says otherwise; pan
//! offsets are image pixels. Zoom *percentages* are device pixels: 100 % is
//! one image pixel per screen pixel, half a logical pixel on a 2× display.

/// How the viewport splits for two images. Both panes share one transform
/// sized to one pane, so pan and zoom stay in lockstep.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SplitLayout {
    #[default]
    None,
    /// Two panes left/right: the view transform sees half the width.
    Horizontal,
    /// Two panes top/bottom: the view transform sees half the height.
    Vertical,
}

/// Gap between side-by-side panes, in logical pixels.
pub const PANE_GAP: f32 = 1.0;

/// One pane along a split axis of `len` logical pixels: its length and the
/// second pane's start. Drawing and zoom share this, so fit fills exactly
/// the painted pane.
pub fn split_pane(len: f32) -> (f32, f32) {
    let half = (len / 2.0).floor();
    ((half - PANE_GAP).max(0.0), half + PANE_GAP)
}

const MIN_SCALE: f32 = 0.05;
const MAX_SCALE: f32 = 20.0;
/// Zoom In/Out stops, in percent of device pixels; wheel, pinch and the
/// zoom field go between them.
const ZOOM_STOPS: [f32; 17] = [
    5.0, 10.0, 25.0, 33.33, 50.0, 66.67, 100.0, 150.0, 200.0, 300.0, 400.0, 600.0, 800.0, 1200.0,
    1600.0, 2400.0, 3200.0,
];
/// Zoom per line of mouse-wheel travel; a notch is usually three lines.
const WHEEL_ZOOM_PER_LINE: f32 = 1.08;

#[derive(Clone, Copy, Debug)]
pub struct ViewTransform {
    /// The image point at the pane's top-left, in image pixels; negative when
    /// centered in a larger pane.
    pub x: f32,
    pub y: f32,
    /// Logical pixels per image pixel: 1.0 is 200 % on a 2× display; 100 %
    /// is `1 / pixel_ratio`.
    pub scale: f32,
}

impl Default for ViewTransform {
    fn default() -> Self {
        Self { x: 0.0, y: 0.0, scale: 0.0 }
    }
}

pub struct ViewTransformController {
    pub transform: ViewTransform,
    /// The whole viewport (every pane), in logical pixels.
    pub container_w: f32,
    pub container_h: f32,
    /// The viewed image's full size, in image pixels.
    pub image_w: u32,
    pub image_h: u32,
    /// Fit, zoom and pan work on one pane's size.
    pub split: SplitLayout,
    /// Clear-rect inset at the bottom (logical pixels) reserved for the
    /// floating View Bar at fit scale; zoomed panning still runs full bleed.
    fit_bottom_inset: f32,
    /// Device pixels per logical pixel (the window's scale factor).
    pixel_ratio: f32,
    /// A pan drag, while one is under way.
    drag: Option<Drag>,
    /// An animated zoom, while one is under way.
    anim: Option<ZoomAnim>,
}

/// A pan drag: its start in logical pixels and the pan then (image pixels).
#[derive(Clone, Copy)]
struct Drag {
    from: (f32, f32),
    pan: (f32, f32),
}

/// An animated zoom (exponential smoothing) toward `target` about `anchor`.
#[derive(Clone, Copy)]
struct ZoomAnim {
    target: f32,
    anchor: (f32, f32),
}

impl ViewTransformController {
    pub fn new() -> Self {
        Self {
            transform: ViewTransform::default(),
            container_w: 0.0,
            container_h: 0.0,
            image_w: 0,
            image_h: 0,
            split: SplitLayout::None,
            fit_bottom_inset: 0.0,
            pixel_ratio: 1.0,
            drag: None,
            anim: None,
        }
    }

    /// Set the window's scale factor.
    pub fn set_pixel_ratio(&mut self, ratio: f32) {
        let ratio = if ratio.is_finite() && ratio > 0.0 { ratio } else { 1.0 };
        if ratio == self.pixel_ratio {
            return;
        }
        // On another display the zoom keeps its device percentage; fit stays
        // fit, being in logical pixels.
        let old = self.pixel_ratio;
        self.pixel_ratio = ratio;
        if self.transform.scale > 0.0 && !self.is_fitted() {
            let target = self.target_scale() * old / ratio;
            self.anim = None;
            self.zoom_at(self.effective_w() / 2.0, self.effective_h() / 2.0, target);
        }
    }

    /// Device pixels per image pixel: what picks the display level.
    pub fn device_scale(&self) -> f32 {
        self.transform.scale * self.pixel_ratio
    }

    /// Pane width: one side of a horizontal split.
    fn effective_w(&self) -> f32 {
        match self.split {
            SplitLayout::Horizontal => split_pane(self.container_w).0,
            _ => self.container_w,
        }
    }

    /// Pane height: one side of a vertical split.
    fn effective_h(&self) -> f32 {
        match self.split {
            SplitLayout::Vertical => split_pane(self.container_h).0,
            _ => self.container_h,
        }
    }

    /// Change the layout, keeping the zoom and re-clamping the pan.
    pub fn set_split(&mut self, split: SplitLayout) {
        if self.split == split {
            return;
        }
        let fitted = self.is_fitted();
        self.split = split;
        self.refit_or_clamp(fitted);
    }

    /// A viewport-local point in pane-local coordinates: the second pane's
    /// points shift back to the first, so zoom-at-cursor works in either.
    pub fn pane_local(&self, local_x: f32, local_y: f32) -> (f32, f32) {
        match self.split {
            SplitLayout::Horizontal if local_x >= split_pane(self.container_w).1 => {
                (local_x - split_pane(self.container_w).1, local_y)
            }
            SplitLayout::Vertical if local_y >= split_pane(self.container_h).1 => {
                (local_x, local_y - split_pane(self.container_h).1)
            }
            _ => (local_x, local_y),
        }
    }

    /// Set image dimensions and reset to fit-to-view.
    pub fn set_image_size(&mut self, w: u32, h: u32) {
        self.image_w = w;
        self.image_h = h;
        self.transform = ViewTransform::default();
        self.anim = None;
        self.zoom_to_fit_immediate();
    }

    /// A `w`×`h` image placed with image point `(x, y)` at the pane's top
    /// left at the scale shown, kept within bounds: the same picture
    /// framed another way (ADR-149).
    pub fn place(&mut self, w: u32, h: u32, x: f32, y: f32) {
        self.image_w = w;
        self.image_h = h;
        self.anim = None;
        if self.transform.scale <= 0.0 {
            return self.zoom_to_fit_immediate();
        }
        self.transform.x = x;
        self.transform.y = y;
        let (min, max) = self.scale_bounds();
        self.transform.scale = self.transform.scale.clamp(min, max);
        self.clamp_pan();
    }

    /// The center of one pane, in its logical pixels.
    pub fn pane_center(&self) -> (f32, f32) {
        (self.effective_w() / 2.0, self.effective_h() / 2.0)
    }

    /// Update the viewport size (logical pixels).
    pub fn set_container_size(&mut self, w: f32, h: f32) {
        if (w - self.container_w).abs() < 0.5 && (h - self.container_h).abs() < 0.5 {
            return;
        }
        let fitted = self.transform.scale <= 0.0 || self.is_fitted();
        self.container_w = w;
        self.container_h = h;
        self.refit_or_clamp(fitted);
    }

    /// After a pane resize: a fitted image refits, other zooms keep scale.
    fn refit_or_clamp(&mut self, fitted: bool) {
        if fitted {
            self.anim = None;
            self.zoom_to_fit_immediate();
        } else {
            self.clamp_pan();
        }
    }

    /// Zoom bounds, widened so fit is always reachable (a 45 MP stack in a
    /// narrow split fits below `MIN_SCALE`; a tiny preview above `MAX_SCALE`).
    fn scale_bounds(&self) -> (f32, f32) {
        let fit = self.fit_scale();
        (MIN_SCALE.min(fit), MAX_SCALE.max(fit))
    }

    /// Reserve `inset` logical pixels at the bottom of the viewport for the
    /// floating View Bar at fit scale; refits smoothly if currently fitted.
    pub fn set_fit_bottom_inset(&mut self, inset: f32) {
        let inset = inset.max(0.0);
        if (inset - self.fit_bottom_inset).abs() < 0.5 {
            return;
        }
        let fitted = self.transform.scale > 0.0 && self.is_fitted();
        self.fit_bottom_inset = inset;
        if fitted {
            self.anim = None;
            self.zoom_to_fit_immediate();
        }
    }

    /// Clear height available for fit-to-view (leaving room above the floating
    /// View Bar when it won't crush the pane below 55% of its height).
    fn fit_clear_h(&self) -> f32 {
        let ch = self.effective_h();
        let inset = if self.split == SplitLayout::Vertical {
            self.fit_bottom_inset * 0.5
        } else {
            self.fit_bottom_inset
        };
        if ch > 0.0 && ch - inset >= ch * 0.55 { ch - inset } else { ch }
    }

    /// The scale that fits the image in one pane.
    fn fit_scale(&self) -> f32 {
        let iw = self.image_w as f32;
        let ih = self.image_h as f32;
        let ew = self.effective_w();
        let ch = self.fit_clear_h();
        if iw <= 0.0 || ih <= 0.0 || ew <= 0.0 || ch <= 0.0 {
            return 1.0;
        }
        (ew / iw).min(ch / ih)
    }

    /// Reset to fit-to-view (immediate, no animation).
    fn zoom_to_fit_immediate(&mut self) {
        let s = self.fit_scale();
        let iw = self.image_w as f32;
        let ih = self.image_h as f32;
        let ew = self.effective_w();
        let ch = self.fit_clear_h();
        if ew <= 0.0 || ch <= 0.0 || iw <= 0.0 || ih <= 0.0 {
            return;
        }
        let vis_w = ew / s;
        let vis_h = ch / s;
        self.transform = ViewTransform { x: -(vis_w - iw) / 2.0, y: -(vis_h - ih) / 2.0, scale: s };
    }

    /// Reset to fit-to-view with animation.
    pub fn zoom_to_fit(&mut self) {
        if self.transform.scale <= 0.0 {
            self.zoom_to_fit_immediate();
            return;
        }
        let target = self.fit_scale();
        let cx = self.effective_w() / 2.0;
        let cy = self.effective_h() / 2.0;
        self.animate_to(cx, cy, target);
    }

    /// The point a zoom keeps in place: `at` (the pointer over a pane), else
    /// the pane's center.
    fn anchor(&self, at: Option<(f32, f32)>) -> (f32, f32) {
        at.unwrap_or((self.effective_w() / 2.0, self.effective_h() / 2.0))
    }

    /// Zoom to a device percentage about `at` ([`Self::anchor`]), animated.
    pub fn zoom_to_percent(&mut self, percent: f32, at: Option<(f32, f32)>) {
        let ew = self.effective_w();
        let ch = self.effective_h();
        if ew <= 0.0 || ch <= 0.0 || self.transform.scale <= 0.0 {
            return;
        }
        let (x, y) = self.anchor(at);
        self.animate_to(x, y, percent / 100.0 / self.pixel_ratio);
    }

    /// Zoom to 100 % about `at`, animated.
    pub fn zoom_to_100(&mut self, at: Option<(f32, f32)>) {
        self.zoom_to_percent(100.0, at);
    }

    /// Zoom keeping the image point under a pane-local point in place.
    pub fn zoom_at(&mut self, local_x: f32, local_y: f32, new_scale: f32) {
        let t = self.transform;
        if t.scale <= 0.0 {
            return;
        }
        let (lo, hi) = self.scale_bounds();
        let s = new_scale.clamp(lo, hi);
        let img_x = t.x + local_x / t.scale;
        let img_y = t.y + local_y / t.scale;
        self.transform = ViewTransform { x: img_x - local_x / s, y: img_y - local_y / s, scale: s };
        self.clamp_pan();
    }

    /// The zoom (target) in percent of device pixels, unrounded.
    fn percent(&self) -> f32 {
        self.target_scale() * self.pixel_ratio * 100.0
    }

    /// Zoom in to the next of [`ZOOM_STOPS`] about `at`, animated.
    pub fn zoom_in(&mut self, at: Option<(f32, f32)>) {
        if self.transform.scale <= 0.0 {
            return;
        }
        let now = self.percent();
        let next = ZOOM_STOPS.iter().copied().find(|&stop| stop > now * 1.001);
        let (x, y) = self.anchor(at);
        let (_, hi) = self.scale_bounds();
        let target = next.map_or(hi, |stop| stop / 100.0 / self.pixel_ratio);
        self.animate_to(x, y, target);
    }

    /// Zoom out to the previous of [`ZOOM_STOPS`] about `at`, animated.
    pub fn zoom_out(&mut self, at: Option<(f32, f32)>) {
        if self.transform.scale <= 0.0 {
            return;
        }
        let now = self.percent();
        let next = ZOOM_STOPS.iter().rev().copied().find(|&stop| stop < now / 1.001);
        let (x, y) = self.anchor(at);
        let (lo, _) = self.scale_bounds();
        let target = next.map_or(lo, |stop| stop / 100.0 / self.pixel_ratio);
        self.animate_to(x, y, target);
    }

    /// Animate toward `target_scale`, anchored at a pane-local point.
    fn animate_to(&mut self, anchor_x: f32, anchor_y: f32, target_scale: f32) {
        let (lo, hi) = self.scale_bounds();
        let clamped = target_scale.clamp(lo, hi);
        self.anim = Some(ZoomAnim { target: clamped, anchor: (anchor_x, anchor_y) });
    }

    /// Advance the animation by `dt` seconds; true if the view moved.
    pub fn tick(&mut self, dt: f32) -> bool {
        let Some(ZoomAnim { target, anchor }) = self.anim else {
            return false;
        };
        if self.transform.scale <= 0.0 {
            self.anim = None;
            return false;
        }

        // Rate of the exponential approach, per second.
        const SPEED: f32 = 12.0;
        let t = 1.0 - (-SPEED * dt).exp();

        // Interpolate in log-space (zoom is multiplicative)
        let ln_cur = self.transform.scale.ln();
        let ln_tgt = target.ln();
        let new_ln = ln_cur + (ln_tgt - ln_cur) * t;

        // Snap when close enough
        if (new_ln - ln_tgt).abs() < 0.001 {
            self.zoom_at(anchor.0, anchor.1, target);
            self.anim = None;
            return true;
        }

        self.zoom_at(anchor.0, anchor.1, new_ln.exp());
        true
    }

    /// Pan by a touchpad scroll, in logical pixels; whether the view changed.
    pub fn on_pan_scroll(&mut self, delta_x: f32, delta_y: f32) -> bool {
        let s = self.transform.scale;
        if s <= 0.0 {
            return false;
        }
        let old_x = self.transform.x;
        let old_y = self.transform.y;
        self.transform.x -= delta_x / s;
        self.transform.y -= delta_y / s;
        self.clamp_pan();
        (self.transform.x - old_x).abs() > f32::EPSILON
            || (self.transform.y - old_y).abs() > f32::EPSILON
    }

    /// A mouse wheel as animated zoom at a pane point; positive `lines`
    /// zooms in, notches mid-animation compound. Whether the view changed.
    pub fn on_wheel_zoom(&mut self, local_x: f32, local_y: f32, lines: f32) -> bool {
        if self.transform.scale <= 0.0 || lines == 0.0 {
            return false;
        }
        let base = self.target_scale();
        self.animate_to(local_x, local_y, base * WHEEL_ZOOM_PER_LINE.powf(lines));
        true
    }

    /// A trackpad pinch at a pane point; `magnification` is relative (0.02 is
    /// 2% bigger). True if the view changed.
    pub fn on_magnify(&mut self, local_x: f32, local_y: f32, magnification: f32) -> bool {
        if self.transform.scale <= 0.0 {
            return false;
        }
        self.anim = None;
        let new_scale = self.transform.scale * (1.0 + magnification);
        if (new_scale - self.transform.scale).abs() < f32::EPSILON {
            return false;
        }
        self.zoom_at(local_x, local_y, new_scale);
        true
    }

    /// Toggle between fit-to-view and 100% zoom, 100% about `at`.
    pub fn toggle_fit_100(&mut self, at: Option<(f32, f32)>) {
        let fit = self.fit_scale();
        let current = self.target_scale();
        if current <= 0.0 {
            return;
        }
        if (current / fit - 1.0).abs() < 0.02 {
            self.zoom_to_100(at);
        } else {
            self.zoom_to_fit();
        }
    }

    /// Begin a pan drag (left or right button) at a point in logical pixels.
    pub fn begin_drag(&mut self, local_x: f32, local_y: f32) {
        self.drag =
            Some(Drag { from: (local_x, local_y), pan: (self.transform.x, self.transform.y) });
    }

    /// Continue a drag; whether the view changed.
    pub fn drag(&mut self, local_x: f32, local_y: f32) -> bool {
        let Some(Drag { from: (sx, sy), pan: (tx, ty) }) = self.drag else {
            return false;
        };
        if self.transform.scale <= 0.0 {
            return false;
        }
        let dx = local_x - sx;
        let dy = local_y - sy;
        self.transform.x = tx - dx / self.transform.scale;
        self.transform.y = ty - dy / self.transform.scale;
        self.clamp_pan();
        true
    }

    /// End drag (pointer up).
    pub fn end_drag(&mut self) {
        self.drag = None;
    }

    /// Keep the image covering the pane on an axis where it is larger, and
    /// centered where smaller.
    fn clamp_pan(&mut self) {
        let iw = self.image_w as f32;
        let ih = self.image_h as f32;
        let ew = self.effective_w();
        let ch = self.effective_h();
        let s = self.transform.scale;
        if iw <= 0.0 || ih <= 0.0 || ew <= 0.0 || ch <= 0.0 || s <= 0.0 {
            return;
        }
        let vis_w = ew / s;
        let vis_h = ch / s;

        self.transform.x =
            if vis_w >= iw { -(vis_w - iw) / 2.0 } else { self.transform.x.clamp(0.0, iw - vis_w) };
        self.transform.y =
            if vis_h >= ih { -(vis_h - ih) / 2.0 } else { self.transform.y.clamp(0.0, ih - vis_h) };
    }

    /// The image's `(x, y, w, h)` in one pane, logical pixels from its
    /// top-left: painted exactly here and clipped to the pane.
    pub fn image_rect(&self) -> (f32, f32, f32, f32) {
        let t = self.transform;
        if t.scale <= 0.0 {
            return (0.0, 0.0, 0.0, 0.0);
        }
        (
            -t.x * t.scale,
            -t.y * t.scale,
            self.image_w as f32 * t.scale,
            self.image_h as f32 * t.scale,
        )
    }

    /// The scale shown once any zoom animation ends.
    fn target_scale(&self) -> f32 {
        self.anim.map_or(self.transform.scale, |a| a.target)
    }

    /// Whether the image fits the view (or is animating to it).
    pub fn is_fitted(&self) -> bool {
        let s = self.target_scale();
        s > 0.0 && (s / self.fit_scale() - 1.0).abs() < 0.005
    }

    /// The zoom percentage shown (device pixels); the target while animating.
    pub fn zoom_percent(&self) -> i32 {
        let s = self.target_scale();
        (s * self.pixel_ratio * 100.0).round() as i32
    }

    /// Convert a pane-local point (logical pixels) to image pixels.
    #[cfg(test)]
    pub fn screen_to_image(&self, screen_x: f32, screen_y: f32) -> (f32, f32) {
        let t = &self.transform;
        if t.scale <= 0.0 {
            return (0.0, 0.0);
        }
        (t.x + screen_x / t.scale, t.y + screen_y / t.scale)
    }
}

impl Default for ViewTransformController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn controller(w: f32, h: f32, iw: u32, ih: u32, ratio: f32) -> ViewTransformController {
        let mut c = ViewTransformController::new();
        c.set_pixel_ratio(ratio);
        c.set_container_size(w, h);
        c.set_image_size(iw, ih);
        c
    }

    fn settle(c: &mut ViewTransformController) {
        while c.tick(0.1) {}
    }

    /// On a 2× display 100 % is half a logical pixel per image pixel.
    #[test]
    fn hundred_percent_is_one_device_pixel() {
        let mut c = controller(800.0, 600.0, 8000, 6000, 2.0);
        c.zoom_to_100(None);
        settle(&mut c);
        assert!((c.transform.scale - 0.5).abs() < 1e-4);
        assert!((c.device_scale() - 1.0).abs() < 1e-4);
        assert_eq!(c.zoom_percent(), 100);
    }

    /// Fit in a split fills the painted pane, gap excluded, and the second
    /// pane maps back onto the first.
    #[test]
    fn split_fit_matches_the_drawn_pane() {
        let mut c = controller(1001.0, 500.0, 1000, 10, 1.0);
        c.set_split(SplitLayout::Horizontal);
        c.zoom_to_fit();
        settle(&mut c);
        let (pane, second) = split_pane(1001.0);
        assert_eq!((pane, second), (499.0, 501.0));
        let (_, _, iw, _) = c.image_rect();
        assert!((iw - pane).abs() < 1e-3, "image {iw} wide in a {pane} pane");
        assert_eq!(c.pane_local(second + 10.0, 7.0), (10.0, 7.0));
        assert_eq!(c.pane_local(10.0, 7.0), (10.0, 7.0));
    }

    /// A notch toward the user zooms out, away zooms in, keeping the point
    /// under the cursor.
    #[test]
    fn wheel_zoom_is_anchored_and_reversible() {
        let mut c = controller(800.0, 600.0, 8000, 6000, 1.0);
        c.zoom_to_100(None);
        settle(&mut c);
        let before = c.screen_to_image(200.0, 150.0);
        assert!(c.on_wheel_zoom(200.0, 150.0, -3.0));
        settle(&mut c);
        assert!(c.transform.scale < 1.0);
        assert!(c.on_wheel_zoom(200.0, 150.0, 3.0));
        settle(&mut c);
        assert!((c.transform.scale - 1.0).abs() < 1e-3);
        let after = c.screen_to_image(200.0, 150.0);
        assert!((before.0 - after.0).abs() < 0.5 && (before.1 - after.1).abs() < 0.5);
    }

    /// A fitted image still fits after resize or split; a set zoom keeps scale.
    #[test]
    fn fit_follows_the_pane() {
        let mut c = controller(800.0, 600.0, 8000, 6000, 1.0);
        assert!(c.is_fitted());
        c.set_container_size(1200.0, 900.0);
        assert!(c.is_fitted());
        c.set_split(SplitLayout::Horizontal);
        assert!(c.is_fitted());
        c.set_split(SplitLayout::None);
        assert!(c.is_fitted());

        c.zoom_to_100(None);
        settle(&mut c);
        c.set_container_size(800.0, 600.0);
        assert!(!c.is_fitted());
        assert_eq!(c.zoom_percent(), 100);
    }

    /// Another display scale keeps 100 % at 100 % and fit at fit.
    #[test]
    fn zoom_survives_a_display_change() {
        let mut c = controller(800.0, 600.0, 8000, 6000, 2.0);
        c.zoom_to_100(None);
        settle(&mut c);
        c.set_pixel_ratio(1.0);
        assert_eq!(c.zoom_percent(), 100);
        c.set_pixel_ratio(2.0);
        assert_eq!(c.zoom_percent(), 100);

        c.zoom_to_fit();
        settle(&mut c);
        c.set_pixel_ratio(1.0);
        assert!(c.is_fitted());
    }

    /// Zoom In/Out step through the stops from any zoom, stopping at the bounds.
    #[test]
    fn keyboard_zoom_steps_through_the_stops() {
        let mut c = controller(800.0, 600.0, 8000, 6000, 2.0);
        // Fit is 20 % on a 2× display: the next stops are 25 % and 10 %.
        assert_eq!(c.zoom_percent(), 20);
        c.zoom_in(None);
        settle(&mut c);
        assert_eq!(c.zoom_percent(), 25);
        c.zoom_in(None);
        settle(&mut c);
        assert_eq!(c.zoom_percent(), 33);
        c.zoom_to_100(None);
        settle(&mut c);
        c.zoom_in(None);
        settle(&mut c);
        assert_eq!(c.zoom_percent(), 150);
        c.zoom_out(None);
        settle(&mut c);
        assert_eq!(c.zoom_percent(), 100, "back to the stop it left");
        c.on_magnify(400.0, 300.0, 0.1);
        c.zoom_out(None);
        settle(&mut c);
        assert_eq!(c.zoom_percent(), 100, "from 110 %, out is the stop below");
        for _ in 0..40 {
            c.zoom_in(None);
        }
        settle(&mut c);
        assert_eq!(c.zoom_percent(), 4000, "the most is the zoom's bound, 20 logical px");
        for _ in 0..40 {
            c.zoom_out(None);
        }
        settle(&mut c);
        assert!(c.is_fitted() || c.zoom_percent() <= 10, "the least is the bound below fit");
    }

    /// A zoom keeps the point under the pointer, else the pane's center.
    #[test]
    fn zoom_keeps_the_point_under_the_pointer() {
        let mut c = controller(800.0, 600.0, 8000, 6000, 1.0);
        c.zoom_to_100(Some((200.0, 150.0)));
        settle(&mut c);
        let under = c.screen_to_image(200.0, 150.0);
        c.zoom_in(Some((200.0, 150.0)));
        settle(&mut c);
        let after = c.screen_to_image(200.0, 150.0);
        assert!((under.0 - after.0).abs() < 0.5 && (under.1 - after.1).abs() < 0.5);

        let center = c.screen_to_image(400.0, 300.0);
        c.zoom_out(None);
        settle(&mut c);
        let after = c.screen_to_image(400.0, 300.0);
        assert!((center.0 - after.0).abs() < 0.5 && (center.1 - after.1).abs() < 0.5);
    }

    /// From fit the toggle goes to 100 % at the pointer; else back to fit.
    #[test]
    fn the_toggle_zooms_to_100_at_the_pointer() {
        let mut c = controller(800.0, 600.0, 8000, 6000, 1.0);
        let under = c.screen_to_image(100.0, 500.0);
        c.toggle_fit_100(Some((100.0, 500.0)));
        settle(&mut c);
        assert_eq!(c.zoom_percent(), 100);
        let after = c.screen_to_image(100.0, 500.0);
        assert!((under.0 - after.0).abs() < 0.5 && (under.1 - after.1).abs() < 0.5);
        c.toggle_fit_100(Some((100.0, 500.0)));
        settle(&mut c);
        assert!(c.is_fitted());
    }
}
