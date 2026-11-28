//! The focus rail in the view bar: one tick per frame in stack order, as
//! tall as its share of the result (`CompletedStack::frame_contributions`;
//! even without a frame map). The frame on screen is marked, left-out
//! frames are amber, and in Retouch painted-from frames carry a dot. Drawn
//! in device pixels (`columns.rs`): a tick per frame while there is room,
//! else a column per pixel.
//!
//! Dragging scrubs; with Option held a drag steps a frame per
//! [`FINE_STEP`], reaching every frame however dense; the wheel and ← →
//! step too. A frame scrubbed to is shown, compared or painted from as the
//! view says.
//!
//! Over the depth map the rail is its key, ticks in each frame's legend
//! color (`CompletedStack::frame_color`): a scrubbed frame is lit on the map
//! (pressing the lit one again outside Retouch puts it out), and the frame
//! under the pointer on the map is marked and clickable.
//!
//! While the stack on screen runs, the rail is the live picture's read-only
//! key ([`RailOf::Run`]): ticks take their color once taken in, all one
//! height, none marked.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use deptheos_app::frames::names::file_label;
use deptheos_app::paint::SourceKind;

use gpui::prelude::*;
use gpui::{
    App, Bounds, Context, Div, Hsla, MouseButton, Pixels, ScrollDelta, ScrollWheelEvent, Window,
    canvas, div, fill, point, px, size,
};
use gpui_kit::component::ActiveTheme as _;

use super::AppView;
use crate::look::{Fill, overlay_surface};
use crate::r#type::TypeStyled as _;
use crate::viewer::ViewEvent;
use crate::words::{Wording, say};

mod columns;
use columns::{Columns, Density};

/// The rail's height, and its ticks' shortest and tallest.
const RAIL_HEIGHT: f32 = 24.0;
const TICK_MIN: f32 = 4.0;
const TICK_MAX: f32 = 16.0;
/// The tallest the frame on screen's marker stands.
const MARKER: f32 = TICK_MAX + 1.0;
/// The lane under the ticks for frames left out, and the gap over it.
const LANE: f32 = 2.0;
const LANE_GAP: f32 = 1.0;
/// The painted-from dots, and how far below the rail's top their lane is.
const DOT: f32 = 3.0;
const DOT_TOP: f32 = 1.0;
/// The rail's minimum width; it takes whatever room the bar has beyond it.
pub(super) const RAIL_MIN: f32 = 96.0;
/// With Option (Alt) held, a drag this far steps one frame.
pub(super) const FINE_STEP: f32 = 8.0;
/// How far the hover label starts before its tick, away from the nearer end.
const LABEL_REACH: f32 = 90.0;
/// A trackpad's scroll this far steps one frame.
const SCROLL_STEP: f32 = 24.0;

/// The pointer on the rail, or on the depth map it is the key to.
///
/// Lifecycle: `Idle`, `Hovering`, `Dragging`, a `Fine` drag or `OverMap`,
/// owned by [`Rail`] and changed only by [`AppView::set_scrub`]. What a
/// frame picked does is [`AppView::scrub_to`]'s.
///
/// - `Idle`/`OverMap` → `Hovering` over a tick; → another; → `Idle` on leaving.
/// - `Idle`/`Hovering` → `Dragging` on a press (scrubs to it); crossing ticks
///   (or beyond the ends) stays `Dragging`; release → `Hovering` or `Idle`.
/// - `Idle`/`Hovering` → `Fine` on an Option-press; `Dragging` → `Fine` when
///   Option goes down, from the frame and place then; each [`FINE_STEP`]
///   moves a frame; Option up → `Dragging`; release → `Hovering` or `Idle`.
/// - `Idle`/`OverMap` → `OverMap` over a part of the map a frame supplied;
///   → `Idle` off it or when the map is off. Never while the rail has the
///   pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Scrub {
    Idle,
    Hovering(usize),
    Dragging(usize),
    /// An Option drag on frame `at`: a frame per [`FINE_STEP`] from window x
    /// `x`, where it started on frame `from`.
    Fine {
        at: usize,
        from: usize,
        x: Pixels,
    },
    /// The pointer over a part of the map frame `0` supplied.
    OverMap(usize),
}

impl Scrub {
    /// The tick the label is over.
    fn at(self) -> Option<usize> {
        match self {
            Scrub::Idle => None,
            Scrub::Hovering(i) | Scrub::Dragging(i) | Scrub::OverMap(i) => Some(i),
            Scrub::Fine { at, .. } => Some(at),
        }
    }
}

/// The rail's pointer, and what it needs between frames.
pub(crate) struct Rail {
    gesture: Scrub,
    /// A trackpad's scroll not yet a whole step: a memory, not a mode.
    wheel: f32,
    /// Where the rail was last laid out, in window coordinates.
    bounds: Rc<Cell<Bounds<Pixels>>>,
}

impl Default for Rail {
    fn default() -> Self {
        Self { gesture: Scrub::Idle, wheel: 0.0, bounds: Rc::default() }
    }
}

/// The value a drag along the rail carries.
struct RailDrag;

/// What a drag along the rail draws under the pointer: nothing.
struct NoGhost;

impl Render for NoGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

/// What the rail is the key to, derived each time ([`AppView::rail_model`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RailOf {
    /// The stack on screen running, `taken` frames in: the live key, inert.
    Run { taken: usize },
    /// The result on screen: scrubbed along, and the depth map's key.
    Result,
}

/// What the rail draws: from the result, view and Retouch, or the run.
pub(crate) struct RailModel {
    pub(crate) of: RailOf,
    pub(crate) paths: Vec<PathBuf>,
    /// Each frame's share of the result; `None` without a frame map.
    pub(crate) shares: Option<Arc<[f32]>>,
    pub(crate) left_out: Vec<bool>,
    /// Retouch: the frames painted from.
    pub(crate) painted: Vec<bool>,
    /// The frame shown, compared with, painted from or lit on the map.
    pub(crate) current: Option<usize>,
    /// Over the depth map, each frame's legend color (sRGB), `None` for one
    /// left out; `None` off the map.
    pub(crate) legend: Option<Vec<Option<[u8; 3]>>>,
}

impl RailModel {
    pub(crate) fn count(&self) -> usize {
        self.paths.len()
    }

    /// Tick `i`'s height against the largest share, or mid-range without shares.
    pub(crate) fn height(&self, i: usize) -> f32 {
        let Some(shares) = &self.shares else { return (TICK_MIN + TICK_MAX) / 2.0 };
        let top = shares.iter().copied().fold(0.0f32, f32::max);
        let share = shares.get(i).copied().unwrap_or(0.0);
        if top <= 0.0 {
            return TICK_MIN;
        }
        TICK_MIN + (TICK_MAX - TICK_MIN) * (share / top).clamp(0.0, 1.0)
    }

    /// "Frame 18 · IMG_0418.DNG · 14%", the label over tick `i`.
    fn label(&self, i: usize, cx: &impl Wording) -> String {
        let number = i + 1;
        let name = self.paths.get(i).map(|p| file_label(p)).unwrap_or_default();
        if self.left_out.get(i).copied().unwrap_or(false) {
            return say!(cx, "view-bar-rail-frame-left-out", number = number, name = name);
        }
        match self.shares.as_ref().and_then(|s| s.get(i)) {
            Some(&share) => {
                let percent = share * 100.0;
                let value = if percent > 0.0 && percent < 1.0 {
                    say!(cx, "view-bar-rail-under-one")
                } else {
                    format!("{percent:.0}")
                };
                let share = say!(cx, "unit-percent", value = value);
                say!(cx, "view-bar-rail-frame-share", number = number, name = name, share = share)
            }
            None => say!(cx, "view-bar-rail-frame", number = number, name = name),
        }
    }
}

/// The tick under window x `x`, clamped to the ends.
fn tick_at(bounds: Bounds<Pixels>, x: Pixels, count: usize) -> usize {
    let width = f32::from(bounds.size.width).max(1.0);
    let at = (f32::from(x - bounds.origin.x) / width * count as f32).floor();
    (at.max(0.0) as usize).min(count.saturating_sub(1))
}

impl AppView {
    /// The only way the rail's pointer changes.
    pub(crate) fn set_scrub(&mut self, gesture: Scrub, cx: &mut Context<Self>) {
        if gesture != self.rail.gesture {
            tracing::debug!("rail: {:?} -> {gesture:?}", self.rail.gesture);
            self.rail.gesture = gesture;
            cx.notify();
        }
    }

    #[cfg(test)]
    pub(crate) fn scrub_gesture(&self) -> Scrub {
        self.rail.gesture
    }

    /// The rail's model: the run's while the stack on screen runs, else the
    /// result's.
    pub(crate) fn rail_model(&self, cx: &App) -> Option<RailModel> {
        let stacking = self.stacking.read(cx);
        if let Some(taken) = stacking.taken() {
            // Shares and left-out frames are known only once it is done.
            let paths = stacking.stacked().to_vec();
            let count = paths.len();
            return Some(RailModel {
                of: RailOf::Run { taken: taken.count },
                paths,
                shares: None,
                left_out: vec![false; count],
                painted: vec![false; count],
                current: None,
                legend: Some(taken.legend()),
            });
        }
        let stack = stacking.result()?.clone();
        let paths = stack.paths.clone();
        let left_out = (0..paths.len()).map(|i| stack.geometry.is_left_out(i)).collect();
        let paint = self.paint.read(cx);
        let (painted, current) = if self.is_retouching() {
            let from: Vec<&PathBuf> = paint
                .painted_from()
                .iter()
                .filter_map(|k| match k {
                    SourceKind::Frame { path } => Some(path),
                    _ => None,
                })
                .collect();
            let painted = paths.iter().map(|p| from.contains(&p)).collect();
            let current = match paint.source() {
                Some(SourceKind::Frame { path }) => paths.iter().position(|p| p == path),
                _ => None,
            };
            (painted, current)
        } else {
            let viewer = self.viewer.read(cx);
            let mode = viewer.mode();
            let current = match mode.is_map() {
                true => mode.lit().and_then(|lit| paths.iter().position(|p| p == lit)),
                false => viewer.shown_frame_index().filter(|_| mode.shows_source()),
            };
            (vec![false; paths.len()], current)
        };
        let legend = self
            .viewer
            .read(cx)
            .mode()
            .is_map()
            .then(|| (0..paths.len()).map(|i| stack.frame_color(i)).collect());
        let shares = stack.frame_contributions();
        Some(RailModel { of: RailOf::Result, shares, paths, left_out, painted, current, legend })
    }

    /// The map is shown and names frames, so a picked frame is lit on it
    /// rather than shown (in Retouch, painted from and lit).
    pub(super) fn lights_map(&self, cx: &App) -> bool {
        self.viewer.read(cx).mode().is_map()
            && self.stacking.read(cx).result().is_some_and(|s| s.frame_map.is_some())
    }

    /// A press on tick `index` (or its part of the map): scrubbed to, or on
    /// the map outside Retouch, the light put out when it is the one lit.
    /// Retouch always paints from a source, so its light stays.
    pub(crate) fn press_tick(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.scrub(index, true, window, cx);
    }

    /// The pointer over the map at result pixel `at`: mark the frame that
    /// supplied it, unless the rail has the pointer.
    pub(crate) fn point_at_map(&mut self, at: Option<(u32, u32)>, cx: &mut Context<Self>) {
        if matches!(self.rail.gesture, Scrub::Hovering(_) | Scrub::Dragging(_) | Scrub::Fine { .. })
        {
            return;
        }
        let stack = self.stacking.read(cx).result().cloned();
        let frame = at.zip(stack).and_then(|((x, y), stack)| stack.frame_at(x, y));
        match frame.filter(|_| self.viewer.read(cx).mode().is_map()) {
            Some(i) => self.set_scrub(Scrub::OverMap(i), cx),
            None => self.leave_map_pointer(cx),
        }
    }

    /// The pointer is no longer over the depth map's frames.
    pub(crate) fn leave_map_pointer(&mut self, cx: &mut Context<Self>) {
        if let Scrub::OverMap(_) = self.rail.gesture {
            self.set_scrub(Scrub::Idle, cx);
        }
    }

    /// A click on the map at result pixel `x`, `y`: as a press on its frame's
    /// tick; nothing where no frame supplied it.
    pub(crate) fn click_map(
        &mut self,
        x: u32,
        y: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.lights_map(cx) {
            return;
        }
        let frame = self.stacking.read(cx).result().and_then(|s| s.frame_at(x, y));
        if let Some(i) = frame {
            self.press_tick(i, window, cx);
        }
    }

    /// Scrub to frame `index`: shown, compared, lit on the map, or in
    /// Retouch painted from ([`crate::paint::Paint::light_map`]). The viewer
    /// and paint worker keep only the newest load. A map that can't be lit
    /// (a depth render without frame map) is left for the frame.
    pub(crate) fn scrub_to(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.scrub(index, false, window, cx);
    }

    /// [`Self::scrub_to`], or with `put_out` [`Self::press_tick`].
    fn scrub(&mut self, index: usize, put_out: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_processing(cx) {
            return;
        }
        let Some(stack) = self.stacking.read(cx).result().cloned() else { return };
        let Some(frame) = crate::viewer::pick(&stack, index) else { return };
        let file = self.files.read(cx).row_of(&frame.id);
        self.highlight_row(file, window, cx);
        if self.is_retouching() {
            self.paint.update(cx, |p, cx| p.scrub_to(&frame.id, cx));
            return;
        }
        self.view(ViewEvent::ShowFrame { frame, put_out }, window, cx);
    }

    /// `delta` frames on from the one marked (from an end when none is).
    pub(crate) fn scrub_by(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(model) = self.rail_model(cx) else { return };
        let count = model.count();
        if model.of != RailOf::Result || count == 0 || delta == 0 {
            return;
        }
        let next = match model.current {
            Some(at) => at.saturating_add_signed(delta).min(count - 1),
            None if delta > 0 => 0,
            None => count - 1,
        };
        if Some(next) != model.current {
            self.scrub_to(next, window, cx);
        }
    }

    pub(super) fn highlight_row(
        &mut self,
        file: Option<crate::files::FileId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.files.read(cx).selected(cx) != file {
            self.files.update(cx, |f, cx| f.highlight(file, window, cx));
        }
    }

    /// A wheel notch or [`SCROLL_STEP`] of trackpad is a frame; down or left
    /// goes on.
    fn scroll_rail(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let steps = match event.delta {
            ScrollDelta::Lines(lines) => {
                let d = if lines.x.abs() > lines.y.abs() { lines.x } else { lines.y };
                -d.signum() as isize * (d.abs().round().max(1.0) as isize)
            }
            ScrollDelta::Pixels(pixels) => {
                let (x, y) = (f32::from(pixels.x), f32::from(pixels.y));
                self.rail.wheel += if x.abs() > y.abs() { x } else { y };
                let whole = (self.rail.wheel / SCROLL_STEP).trunc();
                self.rail.wheel -= whole * SCROLL_STEP;
                -(whole as isize)
            }
        };
        if steps != 0 {
            self.scrub_by(steps, window, cx);
        }
    }

    /// The rail (at least [`RAIL_MIN`] wide), painted in device pixels
    /// ([`paint_rail`]), with its hover label.
    pub(super) fn render_rail(&self, model: RailModel, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let colors = RailColors {
            muted: theme.muted_foreground,
            warning: theme.warning,
            ring: theme.ring,
            foreground: theme.foreground,
            edited: crate::params::edited_color(cx),
        };
        let count = model.count();
        let bounds = self.rail.bounds.clone();
        let laid_out = f32::from(bounds.get().size.width);
        let column = if laid_out > 0.0 { laid_out / count.max(1) as f32 } else { 4.0 };
        let of = model.of;
        // The map's pointer counts only while it is shown, not over the run's key.
        let gesture = match (of, self.rail.gesture) {
            (RailOf::Run { .. }, _) => Scrub::Idle,
            (RailOf::Result, Scrub::OverMap(_)) if model.legend.is_none() => Scrub::Idle,
            (RailOf::Result, gesture) => gesture,
        };
        let label = gesture.at().filter(|&i| i < count).map(|i| {
            let text = model.label(i, cx.wording());
            // Within the rail: reaching right from the left half, left from
            // the right half; a name longer than the rail is cut.
            let width = f32::from(bounds.get().size.width);
            let x = (i as f32 + 0.5) * column;
            let row = div().absolute().bottom(px(RAIL_HEIGHT + 8.0)).left_0().right_0().flex();
            let row = if x <= width / 2.0 {
                row.justify_start().pl(px((x - LABEL_REACH).max(0.0)))
            } else {
                row.justify_end().pr(px((width - x - LABEL_REACH).max(0.0)))
            };
            row.child(
                overlay_surface(Fill::Hud, cx)
                    .debug_selector(|| "rail-label".into())
                    .min_w_0()
                    .px_2()
                    .py_0p5()
                    .text_xs()
                    .tabular()
                    .child(div().truncate().child(text)),
            )
        });
        let pointed = match gesture {
            Scrub::OverMap(i) => Some(i),
            _ => None,
        };
        let drawing = RailDrawing {
            heights: (0..count).map(|i| model.height(i)).collect(),
            left_out: model.left_out,
            painted: model.painted,
            current: model.current,
            pointed,
            legend: model.legend,
            colors,
        };
        let store = bounds.clone();
        let rail = div()
            .id("focus-rail")
            .debug_selector(|| "focus-rail".into())
            .relative()
            .h(px(RAIL_HEIGHT))
            .w_full()
            .child(
                canvas(
                    move |b, _, _| store.set(b),
                    move |b, _, window, _| paint_rail(b, &drawing, window),
                )
                .absolute()
                .inset_0()
                .size_full(),
            );
        let rail = match of {
            // The run's key takes no pointer.
            RailOf::Run { .. } => rail,
            RailOf::Result => self.rail_input(rail, count, cx),
        };
        // As wide as the bar has room for, down to its least.
        div().relative().flex_1().min_w(px(RAIL_MIN)).child(rail).children(label)
    }

    /// The pointer on the result's rail: press, drag, hover and wheel.
    fn rail_input(
        &self,
        rail: gpui::Stateful<Div>,
        count: usize,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        rail.cursor_col_resize()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    let i = tick_at(this.rail.bounds.get(), event.position.x, count);
                    // With Option (Alt) held, the drag steps from here.
                    let gesture = match event.modifiers.alt {
                        true => Scrub::Fine { at: i, from: i, x: event.position.x },
                        false => Scrub::Dragging(i),
                    };
                    this.set_scrub(gesture, cx);
                    this.press_tick(i, window, cx);
                }),
            )
            // Over the rail; the drag beyond its ends is `on_drag_move`'s.
            .on_mouse_move(cx.listener(move |this, event: &gpui::MouseMoveEvent, window, cx| {
                match this.rail.gesture {
                    Scrub::Dragging(_) | Scrub::Fine { .. } if event.dragging() => {
                        let bounds = this.rail.bounds.get();
                        let (x, alt) = (event.position.x, event.modifiers.alt);
                        this.drag_rail(x, alt, bounds, count, window, cx);
                    }
                    _ => {
                        let i = tick_at(this.rail.bounds.get(), event.position.x, count);
                        this.set_scrub(Scrub::Hovering(i), cx)
                    }
                }
            }))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !hovered && matches!(this.rail.gesture, Scrub::Hovering(_)) {
                    this.set_scrub(Scrub::Idle, cx);
                }
            }))
            .on_drag(RailDrag, |_, _, _, cx| cx.new(|_| NoGhost))
            .on_drag_move(cx.listener(
                move |this, event: &gpui::DragMoveEvent<RailDrag>, window, cx| {
                    let (x, alt) = (event.event.position.x, event.event.modifiers.alt);
                    this.drag_rail(x, alt, event.bounds, count, window, cx);
                },
            ))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseUpEvent, _, cx| {
                    let i = tick_at(this.rail.bounds.get(), event.position.x, count);
                    this.set_scrub(Scrub::Hovering(i), cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.set_scrub(Scrub::Idle, cx)),
            )
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, window, cx| {
                this.scroll_rail(event, window, cx);
                cx.stop_propagation();
            }))
    }

    /// A drag at window x `x`: without Option, the frame under the pointer;
    /// with it, a frame per [`FINE_STEP`] since Option took effect.
    fn drag_rail(
        &mut self,
        x: Pixels,
        alt: bool,
        bounds: Bounds<Pixels>,
        count: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let next = match self.rail.gesture {
            Scrub::Fine { from, x: anchor, .. } if alt => {
                Scrub::Fine { at: fine_frame(from, anchor, x, count), from, x: anchor }
            }
            Scrub::Dragging(at) if alt => Scrub::Fine { at, from: at, x },
            Scrub::Dragging(_) | Scrub::Fine { .. } => Scrub::Dragging(tick_at(bounds, x, count)),
            Scrub::Idle | Scrub::Hovering(_) | Scrub::OverMap(_) => return,
        };
        let moved = next.at() != self.rail.gesture.at();
        self.set_scrub(next, cx);
        if let Some(i) = next.at().filter(|_| moved) {
            self.scrub_to(i, window, cx);
        }
    }

    /// Option pressed or released anywhere: a rail drag starts or stops
    /// stepping where the pointer is.
    pub(super) fn rail_modifiers(
        &mut self,
        alt: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.rail_dragging() {
            return;
        }
        let model = self.rail_model(cx).filter(|m| m.of == RailOf::Result);
        let Some(count) = model.map(|m| m.count()).filter(|&n| n > 0) else {
            return;
        };
        let bounds = self.rail.bounds.get();
        self.drag_rail(window.mouse_position().x, alt, bounds, count, window, cx);
    }

    /// A drag on the rail is under way, stepping or not.
    pub(super) fn rail_dragging(&self) -> bool {
        matches!(self.rail.gesture, Scrub::Dragging(_) | Scrub::Fine { .. })
    }
}

/// A stepping drag's frame: `from` plus a frame per [`FINE_STEP`] from
/// `anchor` to `x` (back leftward), within `count`.
fn fine_frame(from: usize, anchor: Pixels, x: Pixels, count: usize) -> usize {
    let steps = (f32::from(x - anchor) / FINE_STEP).trunc() as isize;
    from.saturating_add_signed(steps).min(count.saturating_sub(1))
}

/// The theme's colors the rail draws in.
struct RailColors {
    muted: Hsla,
    warning: Hsla,
    ring: Hsla,
    foreground: Hsla,
    edited: Hsla,
}

/// What the rail's painted element draws, read from the [`RailModel`].
struct RailDrawing {
    /// Each frame's height in points.
    heights: Vec<f32>,
    left_out: Vec<bool>,
    painted: Vec<bool>,
    current: Option<usize>,
    /// The frame under the pointer on the depth map.
    pointed: Option<usize>,
    legend: Option<Vec<Option<[u8; 3]>>>,
    colors: RailColors,
}

impl RailDrawing {
    /// Frame `i`'s legend color on the depth map.
    fn legend(&self, i: usize) -> Option<Hsla> {
        let [r, g, b] = self.legend.as_ref()?.get(i).copied().flatten()?;
        Some(gpui::rgb(crate::color::srgb(u32::from_be_bytes([0, r, g, b]))).into())
    }
}

/// Paint the rail in whole device pixels, laid out by [`Columns`]: from the
/// bottom, the left-out lane, the ticks on a baseline, the painted-from
/// dots, and the current frame's marker over all.
fn paint_rail(bounds: Bounds<Pixels>, d: &RailDrawing, window: &mut Window) {
    let s = window.scale_factor();
    let dev = |v: Pixels| (f32::from(v) * s).round();
    let left = dev(bounds.origin.x);
    let (top, bottom) = (dev(bounds.top()), dev(bounds.bottom()));
    let width = (dev(bounds.right()) - left).max(0.0) as u32;
    let layout = Columns::new(width, d.heights.len(), s);
    if width == 0 || layout.count == 0 {
        return;
    }
    // A rectangle in device pixels, from the rail's left edge.
    let rect = |x: u32, y: f32, w: u32, h: f32| {
        Bounds::new(point(px((left + x as f32) / s), px(y / s)), size(px(w as f32 / s), px(h / s)))
    };
    let lane = (LANE * s).round().max(1.0);
    let base = bottom - lane - (LANE_GAP * s).round().max(1.0);
    let columns = layout.density == Density::Columns;
    let spaced = matches!(layout.density, Density::Spaced { .. });
    for bar in layout.bars(&d.heights) {
        let h = (bar.height * s).round().max(1.0);
        let legend = d.legend(bar.frame);
        let color = legend.unwrap_or(d.colors.muted);
        // Columns read a little lighter, so the change of mode shows.
        let color = if columns { lighter(color) } else { color };
        let at = rect(bar.x, base - h, bar.width, h);
        // A hairline around a spaced map tick so pale colors show on a light bar.
        if spaced && legend.is_some() {
            let hairline = Bounds::new(
                at.origin - point(px(1.0 / s), px(1.0 / s)),
                at.size + size(px(2.0 / s), px(2.0 / s)),
            );
            window.paint_quad(fill(hairline, d.colors.foreground.opacity(0.2)));
        }
        let radius = if spaced { px((bar.width as f32 / 2.0 / s).min(1.0)) } else { px(0.0) };
        window.paint_quad(fill(at, color).corner_radii(radius));
    }
    // Left-out frames in their own lane: coloring a column would hide a
    // sharp neighbor.
    for (x0, x1) in layout.lane(&d.left_out) {
        window.paint_quad(fill(rect(x0, bottom - lane, x1 - x0, lane), d.colors.warning));
    }
    // Dots over painted-from frames, merged where they overlap, and the
    // frame under the pointer on the map.
    let dot = (DOT * s).round().max(1.0) as u32;
    let dot_top = top + (DOT_TOP * s).round();
    let radius = px(dot as f32 / 2.0 / s);
    for (x0, x1) in layout.dots(&d.painted, dot) {
        let at = rect(x0, dot_top, x1 - x0, dot as f32);
        window.paint_quad(fill(at, d.colors.edited).corner_radii(radius));
    }
    if let Some(i) = d.pointed.filter(|&i| i < layout.count) {
        let (x0, x1) = layout.around(i, dot);
        let at = rect(x0, dot_top, x1 - x0, dot as f32);
        window.paint_quad(fill(at, d.colors.foreground).corner_radii(radius));
    }
    // The current frame at its exact center, over all; on the map in its
    // legend color inside a ring of text color.
    if let Some(i) = d.current.filter(|&i| i < layout.count) {
        let h = (MARKER * s).round();
        let legend = d.legend(i);
        let ring = (1.0 * s).round().max(1.0);
        let width = layout.marker_width() + if legend.is_some() { 2 * ring as u32 } else { 0 };
        let (x0, x1) = layout.around(i, width);
        let quad = fill(rect(x0, base - h, x1 - x0, h), legend.unwrap_or(d.colors.ring))
            .corner_radii(px(1.0));
        let quad = match legend {
            Some(_) => quad.border_widths(px(ring / s)).border_color(d.colors.foreground),
            None => quad,
        };
        window.paint_quad(quad);
    }
}

/// `color` a tenth of the way to white.
fn lighter(color: Hsla) -> Hsla {
    Hsla { l: color.l + (1.0 - color.l) * 0.1, ..color }
}

#[cfg(test)]
mod tests {
    use gpui::{point, size};

    use super::*;

    fn model(shares: Option<Vec<f32>>) -> RailModel {
        RailModel {
            of: RailOf::Result,
            paths: (0..4).map(|i| PathBuf::from(format!("f{i}.jpg"))).collect(),
            shares: shares.map(Into::into),
            left_out: vec![false, false, false, true],
            painted: vec![false; 4],
            current: None,
            legend: None,
        }
    }

    #[test]
    fn ticks_rise_with_the_share_and_are_even_without_one() {
        let m = model(Some(vec![0.1, 0.6, 0.3, 0.0]));
        assert_eq!(m.height(1), TICK_MAX, "the largest share is the tallest");
        assert_eq!(m.height(3), TICK_MIN);
        assert!(m.height(0) < m.height(2) && m.height(2) < m.height(1));
        let even = model(None);
        assert!((0..4).all(|i| even.height(i) == even.height(0)));
    }

    #[test]
    fn the_label_names_the_frame_and_its_share() {
        let w = crate::words::DesktopWords::english();
        let m = model(Some(vec![0.14, 0.004, 0.0, 0.0]));
        assert_eq!(m.label(0, &w), "Frame 1 · f0.jpg · 14%");
        assert_eq!(m.label(1, &w), "Frame 2 · f1.jpg · <1%");
        assert_eq!(m.label(2, &w), "Frame 3 · f2.jpg · 0%");
        assert_eq!(m.label(3, &w), "Frame 4 · f3.jpg · left out");
        assert_eq!(model(None).label(0, &w), "Frame 1 · f0.jpg");
    }

    #[test]
    fn a_position_on_the_rail_is_a_tick() {
        let b = Bounds { origin: point(px(100.0), px(0.0)), size: size(px(40.0), px(20.0)) };
        assert_eq!(tick_at(b, px(100.0), 4), 0);
        assert_eq!(tick_at(b, px(119.0), 4), 1);
        assert_eq!(tick_at(b, px(139.9), 4), 3);
        assert_eq!(tick_at(b, px(60.0), 4), 0, "before the start");
        assert_eq!(tick_at(b, px(400.0), 4), 3, "past the end");
    }
}
