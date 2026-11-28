//! Adjust on the desktop: the window's `deptheos_app::adjust::AdjustSession`
//! (ADR-153), what it last said (status, panel, histogram), the slider
//! panel's controls and the pictures it hands the viewer. Whether Adjust is
//! open is `AppView`'s `Mode` (`app/mode.rs`); the panel is drawn by
//! `app/adjusting.rs`.

use std::sync::Arc;

use deptheos_app::adjust::crop::Aspect;
use deptheos_app::adjust::histogram::Histogram;
use deptheos_app::adjust::machine::{Phase, Status};
use deptheos_app::adjust::preview::{FlatPicture, Region, Shown};
use deptheos_app::adjust::{AdjustSession, Observer, Panel, Slider, Update};
use deptheos_app::engine::orient::Orient;
use deptheos_app::library::history::History;
use deptheos_app::paint::Flattened;
use deptheos_app::view::Pane;
use deptheos_app::view::framing::Framing;
use deptheos_app::{Adjustments, CompletedStack, Engine, Look, PresentedImage, RenderId, reframe};
use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable as _, Global,
    RenderImage, Subscription, Task, Window,
};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::slider::{SliderEvent, SliderState};
use image::{Frame, RgbaImage};

use crate::images::TiledImage;
use crate::viewer::Viewer;

/// A slider's track runs 0 to 1, the engine's track position
/// (`SliderSpec::position`), so the Kelvin track is the engine's.
const TRACK_STEP: f32 = 0.0005;

/// What the app hears from Adjust, besides what goes to the viewer.
pub enum AdjustEvent {
    /// Adjust left render `key`, its last write in; `redraw`: the viewer
    /// doesn't show its saved value.
    Closed {
        key: RenderId,
        redraw: bool,
    },
    /// Render `key` drawn under its saved value as Adjust left it: the
    /// result shown, if it still is `key`'s.
    Drawn {
        key: RenderId,
        /// Taken by the one that shows it.
        picture: std::cell::Cell<Option<PresentedImage>>,
    },
    /// Render `key`'s thumbnail was drawn again: the strip reads it again.
    Rewritten(RenderId),
    Notice(String),
}

/// One flat picture as GPUI draws it, covering `covers` of the stored
/// picture.
#[derive(Clone)]
pub struct Flat {
    pub image: Arc<RenderImage>,
    pub covers: Region,
}

/// The session's updates, a flat picture's pixels copied for GPUI on the
/// session's thread so the UI thread allocates nothing per tick.
enum Heard {
    Update(Update),
    Flat { flat: Flat, picture: FlatPicture, histogram: Histogram },
}

struct ToUi(mpsc::UnboundedSender<Heard>);

impl Observer for ToUi {
    fn update(&self, update: Update) {
        let heard = match update {
            Update::Preview { picture, histogram } | Update::OnScreen { picture, histogram } => {
                flat(picture, histogram)
            }
            update => Heard::Update(update),
        };
        let _ = self.0.unbounded_send(heard);
    }
}

/// `picture` as a GPUI image; its buffer goes back to the session from the
/// UI thread.
fn flat(picture: FlatPicture, histogram: Histogram) -> Heard {
    let buffer = RgbaImage::from_raw(picture.width, picture.height, picture.pixels.clone())
        .unwrap_or_else(|| RgbaImage::new(picture.width.max(1), picture.height.max(1)));
    let image = Arc::new(RenderImage::new(vec![Frame::new(buffer)]));
    let flat = Flat { image, covers: picture.covers };
    Heard::Flat { flat, picture, histogram }
}

/// Which tool a click on the photo is, in Adjust.
///
/// Lifecycle: `Sliders`, `Eyedropper` or `Crop`, held by [`Adjust`] and
/// changed only by [`Adjust::set_tool`]: `Sliders` → `Eyedropper` by W or
/// its button; back by W, the button, Esc, a click on the photo, or Adjust
/// opening or leaving. `Sliders` → `Crop` by C or the Crop button, holding
/// the reframe it opened on; back by C, Done or Return (the crop kept), Esc
/// (the reframe it opened on put back), or Adjust leaving. `Crop` → `Crop`
/// when its aspect changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdjustTool {
    /// Clicks and drags pan.
    Sliders,
    /// A click sets the white balance from the spot.
    Eyedropper,
    /// The picture uncropped, the crop's handles dragged.
    Crop { opened: reframe::Reframe, aspect: Aspect },
}

/// Copy Adjustments' sliders, for Paste Adjustments in any window.
#[derive(Default)]
struct Copied(Option<Adjustments>);

impl Global for Copied {}

/// The sliders of the render open, each a track from 0 to 1.
struct Controls {
    panel: Panel,
    sliders: Vec<(Slider, Entity<SliderState>)>,
    /// The slider dragged: its track isn't moved under the pointer.
    dragging: Option<Slider>,
    _subscriptions: Vec<Subscription>,
}

/// A value being typed into a slider's field.
struct Typing {
    slider: Slider,
    input: Entity<InputState>,
    _events: Subscription,
}

pub struct Adjust {
    session: AdjustSession,
    engine: Arc<Engine>,
    viewer: Entity<Viewer>,
    /// The window's keys, back in charge after a typed value.
    home: FocusHandle,
    status: Option<Status>,
    histogram: Option<Histogram>,
    controls: Option<Controls>,
    /// The slider the arrow keys move: the last one touched.
    selected: Option<Slider>,
    typing: Option<Typing>,
    tool: AdjustTool,
    /// The stored picture with its paint, unadjusted, for Before; made as
    /// Adjust opens, then turned as the value shown is.
    _before: Option<Task<()>>,
    before: Option<Arc<PresentedImage>>,
    /// The turn and flip Before was last handed to the viewer with.
    before_orient: Option<Orient>,
    /// Turns Before when the value's turn or flip changes.
    _turning: Option<Task<()>>,
    _updates: Task<()>,
}

impl EventEmitter<AdjustEvent> for Adjust {}

impl Adjust {
    pub fn new(
        engine: Arc<Engine>,
        history: Arc<History>,
        viewer: Entity<Viewer>,
        home: FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (tx, mut rx) = mpsc::unbounded();
        let session = AdjustSession::new(engine.clone(), history, Arc::new(ToUi(tx)));
        let _updates = cx.spawn_in(window, async move |this, cx| {
            while let Some(heard) = rx.next().await {
                let heard = this.update_in(cx, |this, window, cx| this.on_heard(heard, window, cx));
                if heard.is_err() {
                    return;
                }
            }
        });
        Self {
            session,
            engine,
            viewer,
            home,
            status: None,
            histogram: None,
            controls: None,
            selected: None,
            typing: None,
            tool: AdjustTool::Sliders,
            _before: None,
            before: None,
            before_orient: None,
            _turning: None,
            _updates,
        }
    }

    #[cfg(test)]
    pub fn session(&self) -> &AdjustSession {
        &self.session
    }

    /// Where Adjust stands, as the session last said.
    pub fn status(&self) -> Option<&Status> {
        self.status.as_ref()
    }

    pub fn histogram(&self) -> Option<&Histogram> {
        self.histogram.as_ref()
    }

    pub fn panel(&self) -> Option<Panel> {
        self.controls.as_ref().map(|c| c.panel)
    }

    pub fn tool(&self) -> AdjustTool {
        self.tool
    }

    pub fn selected(&self) -> Option<Slider> {
        self.selected
    }

    /// The track of `slider`, while its panel is shown.
    pub fn track(&self, slider: Slider) -> Option<&Entity<SliderState>> {
        let controls = self.controls.as_ref()?;
        controls.sliders.iter().find(|(s, _)| *s == slider).map(|(_, state)| state)
    }

    /// The field `slider`'s value is being typed into.
    pub fn typing(&self, slider: Slider) -> Option<&Entity<InputState>> {
        self.typing.as_ref().filter(|t| t.slider == slider).map(|t| &t.input)
    }

    /// `slider`'s value in the render's value now.
    pub fn value(&self, slider: Slider) -> Option<f64> {
        let (status, panel) = (self.status.as_ref()?, self.panel()?);
        Some(slider.get(&status.value.adjustments, &panel))
    }

    /// Open Adjust on render `key`, `flattened` its paint for Before.
    pub fn open(
        &mut self,
        key: RenderId,
        stack: Arc<CompletedStack>,
        flattened: std::sync::mpsc::Receiver<Flattened>,
        cx: &mut Context<Self>,
    ) {
        let engine = self.engine.clone();
        self.status = None;
        self.histogram = None;
        self.typing = None;
        self.set_tool(AdjustTool::Sliders, cx);
        self.controls = Some(controls(Panel::of(&stack), cx));
        self.viewer.update(cx, |v, cx| v.show_before(None, cx));
        (self.before, self.before_orient, self._turning) = (None, None, None);
        self.session.open(key, stack.clone());
        self.sync_view(cx);
        self._before = Some(cx.spawn(async move |this, cx| {
            let made = crate::work::interactive("adjust-before", move || {
                let pixels = match flattened.recv() {
                    Ok(Ok(Some(pixels))) => Some(pixels),
                    _ => None,
                };
                let pixels = pixels.as_deref().unwrap_or(&stack.pixels);
                engine.present_pixels(Look::stored(&stack), pixels)
            })
            .await;
            let before = match made {
                Ok(Ok(before)) => Arc::new(before),
                Ok(Err(e)) => return tracing::warn!("adjust: the picture before not drawn: {e}"),
                Err(e) => return tracing::warn!("adjust: the picture before not drawn: {e}"),
            };
            this.update(cx, |this, cx| {
                this.before = Some(before);
                this.turn_before(cx);
            })
            .ok();
        }));
        cx.notify();
    }

    /// Before, turned and flipped as the value shown is, to the viewer
    /// (off the UI thread) when it isn't already.
    fn turn_before(&mut self, cx: &mut Context<Self>) {
        let (Some(before), Some(picture)) = (self.before.clone(), self.viewer.read(cx).picture())
        else {
            return;
        };
        let reframe = self.status.as_ref().map_or(reframe::Reframe::NONE, |s| s.value.reframe);
        let orient = Orient::of(reframe, picture);
        if self.before_orient == Some(orient) {
            return;
        }
        self.before_orient = Some(orient);
        self._turning = Some(cx.spawn(async move |this, cx| {
            let turned = crate::work::interactive("adjust-before-turn", move || {
                orient.presented((*before).clone())
            })
            .await;
            let Ok(turned) = turned else { return };
            let before = TiledImage::from_presented(turned);
            this.update(cx, |this, cx| {
                this.viewer.update(cx, |v, cx| v.show_before(Some(before), cx))
            })
            .ok();
        }));
    }

    /// Leave Adjust, a change in progress committed.
    pub fn leave(&mut self, cx: &mut Context<Self>) {
        self.session.leave();
        self.typing = None;
        self.controls = None;
        self.set_tool(AdjustTool::Sliders, cx);
        (self._before, self.before, self.before_orient, self._turning) = (None, None, None, None);
        self.viewer.update(cx, |v, cx| {
            v.clear_flat(cx);
            v.show_before(None, cx);
        });
        cx.notify();
    }

    /// What the viewer shows changed: the session draws for it next.
    pub fn viewed(&self, shown: Shown) {
        self.session.view(shown);
    }

    /// Tell the session what the viewer shows, for the next preview.
    fn sync_view(&self, cx: &App) {
        if let Some(shown) = self.viewer.read(cx).shown_view() {
            self.session.view(shown);
        }
    }

    /// `slider` moves to `value` (a change goes on until [`Self::commit`]).
    pub fn set(&mut self, slider: Slider, value: f64, cx: &mut Context<Self>) {
        self.selected = Some(slider);
        self.session.set_slider(slider, value);
        cx.notify();
    }

    pub fn commit(&self) {
        self.session.commit();
    }

    /// The reframe moves (a crop drag, a turn); a change until [`Self::commit`].
    pub fn set_reframe(&self, reframe: reframe::Reframe) {
        self.session.set_reframe(reframe);
    }

    /// The selected slider `steps` steps along: one change.
    pub fn nudge(&mut self, steps: f64, cx: &mut Context<Self>) {
        let (Some(slider), Some(panel)) = (self.selected, self.panel()) else { return };
        let Some(now) = self.value(slider) else { return };
        let spec = slider.spec(&panel);
        self.set(slider, spec.snap(now + steps * spec.step), cx);
        self.commit();
    }

    /// The slider before or after the selected one, in panel order.
    pub fn select_next(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(panel) = self.panel() else { return };
        let order: Vec<Slider> = panel.groups().into_iter().flat_map(|(_, s)| s).collect();
        let at = self.selected.and_then(|s| order.iter().position(|o| *o == s));
        let next = match (at, forward) {
            (None, _) => order.first(),
            (Some(i), true) => order.get(i + 1).or(order.last()),
            (Some(i), false) => order.get(i.saturating_sub(1)),
        };
        self.selected = next.copied();
        cx.notify();
    }

    pub fn select(&mut self, slider: Slider, cx: &mut Context<Self>) {
        self.selected = Some(slider);
        cx.notify();
    }

    pub fn reset(&mut self, slider: Slider, cx: &mut Context<Self>) {
        self.selected = Some(slider);
        self.session.reset(slider);
        cx.notify();
    }

    pub fn reset_all(&mut self) {
        self.session.reset_all();
    }

    pub fn undo(&mut self) {
        self.session.undo();
    }

    pub fn redo(&mut self) {
        self.session.redo();
    }

    pub fn retry(&mut self) {
        self.session.retry();
    }

    /// Steps to undo and redo.
    pub fn steps(&self) -> [usize; 2] {
        self.status.as_ref().map_or([0, 0], |s| [s.undo, s.redo])
    }

    /// Copy Adjustments: the sliders of the render adjusted.
    pub fn copy(&self, cx: &mut App) {
        if let Some(status) = &self.status {
            copy(status.value.adjustments.clone(), cx);
        }
    }

    /// Paste Adjustments onto the render adjusted: one change.
    pub fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(copied) = copied(cx) else { return };
        self.session.paste(copied);
    }

    /// The eyedropper on (or off).
    pub fn set_tool(&mut self, tool: AdjustTool, cx: &mut Context<Self>) {
        if self.tool != tool {
            tracing::debug!(from = ?self.tool, to = ?tool, "adjust: tool");
            self.tool = tool;
            cx.notify();
        }
    }

    /// A click with the eyedropper at stored pixel `at`: the white balance
    /// from there, mapped to the picture as shown under its reframe.
    pub fn eyedrop(&mut self, at: (f64, f64), cx: &mut Context<Self>) {
        let Some(status) = &self.status else { return };
        let Some(picture) = self.viewer.read(cx).picture() else { return };
        let framing = Framing::of(picture, status.value.reframe, Pane::Result);
        let shown = framing.to_shown(at);
        self.session.eyedrop(shown);
    }

    /// Start typing `slider`'s value.
    pub fn start_typing(&mut self, slider: Slider, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(panel), Some(value)) = (self.panel(), self.value(slider)) else { return };
        let text = slider.value_text(value, &panel, crate::words::of(cx));
        let input = cx.new(|cx| InputState::new(window, cx));
        input.update(cx, |input, cx| {
            input.set_value(text, window, cx);
            input.select_all(window, cx);
        });
        let _events =
            cx.subscribe_in(&input, window, move |this, _, event, window, cx| match event {
                InputEvent::PressEnter { .. } => this.finish_typing(true, window, cx),
                InputEvent::Blur => this.finish_typing(false, window, cx),
                _ => {}
            });
        window.focus(&input.read(cx).focus_handle(cx), cx);
        self.selected = Some(slider);
        self.typing = Some(Typing { slider, input, _events });
        cx.notify();
    }

    /// Done typing: the value read and set (`keep`), or dropped.
    pub fn finish_typing(&mut self, keep: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Typing { slider, input, .. }) = self.typing.take() else { return };
        cx.notify();
        let text = input.read(cx).value().to_string();
        if keep && let (Some(v), Some(panel)) = (slider.parse(&text), self.panel()) {
            self.set(slider, slider.spec(&panel).snap(v), cx);
            self.commit();
        }
        // Back to the window's keys.
        window.focus(&self.home, cx);
    }

    fn on_heard(&mut self, heard: Heard, window: &mut Window, cx: &mut Context<Self>) {
        match heard {
            Heard::Flat { flat, picture, histogram } => {
                self.session.give_back(picture);
                if self.status.as_ref().is_some_and(|s| s.phase == Phase::Ready) {
                    self.histogram = Some(histogram);
                    self.viewer.update(cx, |v, cx| v.show_flat(flat, cx));
                }
                cx.notify();
            }
            Heard::Update(update) => self.on_update(update, window, cx),
        }
    }

    fn on_update(&mut self, update: Update, window: &mut Window, cx: &mut Context<Self>) {
        match update {
            Update::Status(status) => {
                self.sync_tracks(&status, window, cx);
                let reframe = status.value.reframe;
                self.viewer.update(cx, |v, cx| v.reframe_shown(reframe, cx));
                self.status = Some(status);
                self.turn_before(cx);
                cx.notify();
            }
            Update::Settled { key, picture, histogram } => {
                if self.controls.is_none() {
                    // Drawn as Adjust left: the result under its saved value.
                    cx.emit(AdjustEvent::Drawn { key, picture: Some(picture).into() });
                } else if self.status.as_ref().is_some_and(|s| s.key == key) {
                    self.histogram = Some(histogram);
                    self.viewer.update(cx, |v, cx| v.settle(picture, cx));
                }
                cx.notify();
            }
            Update::Histogram { key, histogram } => {
                if self.status.as_ref().is_some_and(|s| s.key == key) {
                    self.histogram = Some(histogram);
                    cx.notify();
                }
            }
            Update::Closed { key, redraw } => {
                if self.status.as_ref().is_some_and(|s| s.key == key) {
                    self.status = None;
                }
                cx.emit(AdjustEvent::Closed { key, redraw });
            }
            Update::Rewritten(key) => cx.emit(AdjustEvent::Rewritten(key)),
            Update::Notice(text) => cx.emit(AdjustEvent::Notice(text)),
            // Turned into `Heard::Flat` on the session's thread.
            Update::Preview { picture, .. } | Update::OnScreen { picture, .. } => {
                self.session.give_back(picture)
            }
        }
    }

    /// Each track where `status`'s value puts it, but the one dragged.
    fn sync_tracks(&mut self, status: &Status, window: &mut Window, cx: &mut Context<Self>) {
        let Some(controls) = &self.controls else { return };
        let panel = controls.panel;
        for (slider, state) in &controls.sliders {
            if controls.dragging == Some(*slider) {
                continue;
            }
            let value = slider.get(&status.value.adjustments, &panel);
            let at = slider.spec(&panel).position(value) as f32;
            if (state.read(cx).value().start() - at).abs() > TRACK_STEP / 2.0 {
                state.update(cx, |s, cx| s.set_value(at, window, cx));
            }
        }
    }

    fn on_slider(&mut self, slider: Slider, event: &SliderEvent, cx: &mut Context<Self>) {
        let Some(controls) = &mut self.controls else { return };
        let panel = controls.panel;
        let spec = slider.spec(&panel);
        match event {
            SliderEvent::Change(at) => {
                controls.dragging = Some(slider);
                let value = spec.snap(spec.value_at(f64::from(at.start())));
                self.set(slider, value, cx);
            }
            SliderEvent::Release(_) => {
                controls.dragging = None;
                self.commit();
            }
        }
    }
}

/// The tracks of `panel`'s sliders, each heard by the panel.
fn controls(panel: Panel, cx: &mut Context<Adjust>) -> Controls {
    let mut sliders = Vec::new();
    let mut subscriptions = Vec::new();
    for slider in panel.groups().into_iter().flat_map(|(_, s)| s) {
        let at = slider.spec(&panel).position(slider.neutral(&panel)) as f32;
        let state =
            cx.new(|_| SliderState::new().min(0.0).max(1.0).step(TRACK_STEP).default_value(at));
        subscriptions.push(cx.subscribe(&state, move |this, _, event: &SliderEvent, cx| {
            this.on_slider(slider, event, cx)
        }));
        sliders.push((slider, state));
    }
    Controls { panel, sliders, dragging: None, _subscriptions: subscriptions }
}

/// Copy Adjustments: `adjustments`' sliders, for any window's Paste.
pub fn copy(adjustments: Adjustments, cx: &mut App) {
    tracing::debug!("adjust: adjustments copied");
    cx.set_global(Copied(Some(adjustments)));
}

/// What Copy Adjustments copied, if anything.
pub fn copied(cx: &App) -> Option<Adjustments> {
    cx.try_global::<Copied>().and_then(|c| c.0.clone())
}
