//! The pointer: what a held button does, and the presses, moves, releases,
//! scrolls and pinches that change it (window coordinates).

use deptheos_app::paint::Dab;
use gpui::{Context, MouseButton, Pixels, Point, ScrollDelta};

use deptheos_app::adjust::crop::{Area, CropFrame, Handle};

use super::{PaintInput, Tool, Viewer, ViewerEvent};

/// Points a map press may move and still be a click rather than a pan.
const CLICK_SLOP: f32 = 3.0;

/// How near a crop edge or corner a press takes it, in points.
const CROP_REACH: f32 = 10.0;

/// What a held mouse button is doing: one at a time; the other button's
/// press is ignored and only the holder's release ends it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Gesture {
    Idle,
    /// Dragging the view.
    Pan(MouseButton),
    /// A left press on the map at `at`, not yet past [`CLICK_SLOP`]: released
    /// there it is a click lighting the frame (in Retouch, picking it);
    /// moved, a pan. No stroke starts on the map.
    MapPress {
        button: MouseButton,
        at: Point<Pixels>,
    },
    /// Dragging the compare divider.
    Divider(MouseButton),
    /// A paint stroke, mapped wholly through its starting pane so crossing
    /// into the other doesn't jump the brush.
    Stroke {
        button: MouseButton,
        pane: usize,
    },
    /// Held after its stroke ended (Paint tool left): inert until released.
    Spent(MouseButton),
    /// The crop tool's `handle` dragged from shown point `at`, the crop
    /// then `from`, now `now`.
    Crop {
        button: MouseButton,
        handle: Handle,
        at: (f32, f32),
        from: Area,
        now: Area,
    },
}

impl Gesture {
    /// The button holding it.
    fn button(self) -> Option<MouseButton> {
        match self {
            Gesture::Idle => None,
            Gesture::Pan(b) | Gesture::Divider(b) | Gesture::Spent(b) => Some(b),
            Gesture::Stroke { button, .. }
            | Gesture::MapPress { button, .. }
            | Gesture::Crop { button, .. } => Some(button),
        }
    }
}

impl Viewer {
    /// The shown picture's point under window point `p`, through the pane
    /// it is in: the stored picture turned, uncropped (`framed.rs`).
    pub(super) fn image_point(&self, p: Point<Pixels>) -> Option<(f32, f32)> {
        let pane = self.pane_at(p)?;
        self.image_point_in(pane, p)
    }

    fn pane_at(&self, p: Point<Pixels>) -> Option<usize> {
        self.panes.iter().position(|(clip, _)| clip.contains(&p))
    }

    /// The image pixel under window point `p` through pane `pane`, inside it
    /// or not (clamped later).
    fn image_point_in(&self, pane: usize, p: Point<Pixels>) -> Option<(f32, f32)> {
        let (_, rect) = self.panes.get(pane)?;
        let scale = self.per_pixel(*rect);
        Some((f32::from(p.x - rect.origin.x) / scale, f32::from(p.y - rect.origin.y) / scale))
    }

    fn local(&self, position: Point<Pixels>) -> (f32, f32) {
        (f32::from(position.x - self.bounds.origin.x), f32::from(position.y - self.bounds.origin.y))
    }

    /// A right drag pans; a left one drags the divider, paints or pans. A
    /// press while the other button drags is ignored; the same button again
    /// means its release was missed outside the window.
    pub(super) fn mouse_down(
        &mut self,
        position: Point<Pixels>,
        button: MouseButton,
        on_divider: bool,
        cx: &mut Context<Self>,
    ) {
        match self.gesture.button() {
            Some(held) if held != button => return,
            Some(_) => {
                self.mouse_up(button, cx);
            }
            None => {}
        }
        let (x, y) = self.local(position);
        let gesture = if button == MouseButton::Right {
            self.view.begin_drag(x, y);
            Gesture::Pan(button)
        } else if on_divider {
            Gesture::Divider(button)
        } else if self.tool == Tool::Eyedropper && !self.held_keys.space {
            // The pixel's middle; beside the picture picks nothing.
            if let Some((x, y)) = self.pixel_at(position) {
                cx.emit(ViewerEvent::Eyedrop { x: f64::from(x) + 0.5, y: f64::from(y) + 0.5 });
            }
            Gesture::Spent(button)
        } else if let Some(gesture) = self.crop_press(position, button) {
            gesture
        } else if self.tool == Tool::Paint && !self.held_keys.space && !self.state.now().is_map() {
            let Some(pane) = self.pane_at(position) else { return };
            let Some(dab) = self.dab(pane, position) else { return };
            cx.emit(ViewerEvent::Paint(PaintInput::Begin(dab)));
            Gesture::Stroke { button, pane }
        } else if self.state.now().is_map() {
            self.view.begin_drag(x, y);
            Gesture::MapPress { button, at: position }
        } else {
            self.view.begin_drag(x, y);
            Gesture::Pan(button)
        };
        self.set_gesture(gesture);
    }

    /// The crop tool's picture: the stored picture uncropped, turned and
    /// flipped as the render is.
    pub(super) fn crop_frame(&self) -> Option<CropFrame> {
        let frame = CropFrame::of(self.picture?, self.reframe);
        let shown = frame.shown();
        ((shown.width, shown.height) == self.framed.shown).then_some(frame)
    }

    /// The crop as the tool draws it: dragged, else the reframe's.
    pub(super) fn crop_area(&self) -> Option<Area> {
        match self.gesture {
            Gesture::Crop { now, .. } => Some(now),
            _ => Some(self.crop_frame()?.area()),
        }
    }

    /// A press with the crop tool on one of the crop's handles, or inside
    /// it: the drag it starts; elsewhere it pans.
    fn crop_press(&self, position: Point<Pixels>, button: MouseButton) -> Option<Gesture> {
        if !matches!(self.tool, Tool::Crop(_)) || self.held_keys.space {
            return None;
        }
        let (frame, area) = (self.crop_frame()?, self.crop_area()?);
        let at = self.image_point(position)?;
        let rect = self.panes.last()?.1;
        let reach = f64::from(CROP_REACH / self.per_pixel(rect).max(f32::MIN_POSITIVE));
        let handle = frame.handle_at(area, (f64::from(at.0), f64::from(at.1)), reach)?;
        Some(Gesture::Crop { button, handle, at, from: area, now: area })
    }

    /// ⌥-click: the pixel under `position`, for the sharpest frame there. No
    /// stroke; beside the image picks nothing.
    pub(super) fn pick_at(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if let Some((x, y)) = self.pixel_at(position) {
            cx.emit(ViewerEvent::PickAt { x, y });
        }
    }

    /// The stored pixel under `position`, if on the picture.
    fn pixel_at(&self, position: Point<Pixels>) -> Option<(u32, u32)> {
        self.stored_pixel(self.image_point(position)?)
    }

    /// Over the map: the result's pixel under the pointer, or `None` off it.
    pub(super) fn point_at_map(&mut self, position: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        if self.state.now().is_map() {
            cx.emit(ViewerEvent::MapPointer(position.and_then(|p| self.pixel_at(p))));
        }
    }

    pub(super) fn set_gesture(&mut self, gesture: Gesture) {
        self.gesture = gesture;
        self.held.set(gesture != Gesture::Idle);
    }

    /// Every move matters, not just drags: the Paint ring follows it and the
    /// map reports what is under it.
    pub(super) fn sync_tracking(&self) {
        self.tracking.set(self.tool == Tool::Paint || self.state.now().is_map());
    }

    fn dab(&self, pane: usize, position: Point<Pixels>) -> Option<Dab> {
        let (x, y) = self.to_stored(self.image_point_in(pane, position)?);
        Some(Dab { x, y, pressure: 1.0 })
    }

    /// Whether the view changed (or the ring moved).
    pub(super) fn mouse_move(
        &mut self,
        position: Point<Pixels>,
        over: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let (x, y) = self.local(position);
        let ring_moved = if self.tool == Tool::Paint {
            let hover = over.then_some(position);
            std::mem::replace(&mut self.hover, hover) != hover
        } else {
            false
        };
        if self.state.now().is_map() {
            self.point_at_map(over.then_some(position), cx);
        }
        match self.gesture {
            Gesture::MapPress { button, at } => {
                let moved = position - at;
                if f32::from(moved.x).hypot(f32::from(moved.y)) > CLICK_SLOP {
                    self.set_gesture(Gesture::Pan(button));
                }
                self.view.drag(x, y) || ring_moved
            }
            Gesture::Divider(_) => {
                let w = f32::from(self.bounds.size.width).max(1.0);
                self.split = (x / w).clamp(0.05, 0.95);
                true
            }
            Gesture::Stroke { pane, .. } => {
                if let Some(dab) = self.dab(pane, position) {
                    cx.emit(ViewerEvent::Paint(PaintInput::Move(dab)));
                }
                ring_moved
            }
            Gesture::Crop { button, handle, at, from, .. } => {
                let (Some(frame), Some(p)) = (self.crop_frame(), self.image_point(position)) else {
                    return false;
                };
                let Tool::Crop(aspect) = self.tool else { return false };
                let by = (f64::from(p.0 - at.0), f64::from(p.1 - at.1));
                let now = frame.drag(from, handle, by, aspect);
                self.set_gesture(Gesture::Crop { button, handle, at, from, now });
                cx.emit(ViewerEvent::Crop(frame.reframed(now)));
                true
            }
            Gesture::Pan(_) => self.view.drag(x, y) || ring_moved,
            Gesture::Idle | Gesture::Spent(_) => ring_moved,
        }
    }

    pub(super) fn mouse_up(&mut self, button: MouseButton, cx: &mut Context<Self>) -> bool {
        if self.gesture.button() != Some(button) {
            return false;
        }
        let gesture = self.gesture;
        match gesture {
            Gesture::Stroke { .. } => cx.emit(ViewerEvent::Paint(PaintInput::End)),
            Gesture::Crop { .. } => cx.emit(ViewerEvent::CropEnd),
            Gesture::MapPress { at, .. } if self.state.now().is_map() => {
                if let Some((x, y)) = self.pixel_at(at) {
                    cx.emit(ViewerEvent::MapClick { x, y });
                }
            }
            _ => {}
        }
        self.view.end_drag();
        self.set_gesture(Gesture::Idle);
        !matches!(gesture, Gesture::Spent(_))
    }

    /// A mouse wheel zooms at the cursor; a touchpad scroll pans unless
    /// `zoom` (⌘, or Ctrl off macOS), when any scroll zooms.
    pub(super) fn scroll(
        &mut self,
        position: Point<Pixels>,
        delta: ScrollDelta,
        line_height: Pixels,
        zoom: bool,
    ) -> bool {
        let (x, y) = self.local(position);
        let (px_, py_) = self.view.pane_local(x, y);
        match delta {
            ScrollDelta::Lines(lines) => self.view.on_wheel_zoom(px_, py_, lines.y),
            ScrollDelta::Pixels(pixels) if zoom => {
                let lines = f32::from(pixels.y) / f32::from(line_height).max(1.0);
                self.view.on_wheel_zoom(px_, py_, lines)
            }
            ScrollDelta::Pixels(_) => {
                let delta = delta.pixel_delta(line_height);
                self.view.on_pan_scroll(f32::from(delta.x), f32::from(delta.y))
            }
        }
    }

    /// The pane-local point under the pointer: where zoom keys anchor.
    pub(super) fn zoom_anchor(&self) -> Option<(f32, f32)> {
        let p = self.pointer.get()?;
        self.pane_at(p)?;
        let (x, y) = self.local(p);
        Some(self.view.pane_local(x, y))
    }

    /// Double-click: Zoom to Fit, or Actual Size at the pointer.
    pub(super) fn double_click(&mut self, position: Point<Pixels>) {
        let (x, y) = self.local(position);
        let at = self.view.pane_local(x, y);
        tracing::debug!("double-click: fit or 100% at {at:?}");
        self.view.toggle_fit_100(Some(at));
    }

    /// Pinch zoom at the cursor, remapped into its pane side by side.
    pub(super) fn pinch(&mut self, position: Point<Pixels>, delta: f32) -> bool {
        let (x, y) = self.local(position);
        let (px_, py_) = self.view.pane_local(x, y);
        self.view.on_magnify(px_, py_, delta)
    }
}
