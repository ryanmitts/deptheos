//! The viewer's view, on both apps (ADR-149): one pure reducer on the UI
//! thread. [`View::on`] takes an event and answers with actions and the new
//! view. It holds ids, never pixels; the loads stay each app's, and nothing
//! in it waits.
//!
//! The frame on screen is the app's selection: the view says only what goes
//! over it ([`Action::Show`]). The frame lit on the depth map is the view's
//! own, since it differs from the selection until the map is left.

use crate::Overlay;

/// The overlay chosen, kept from frame to frame, and whether this frame
/// supplied part of the result. Over a frame that didn't, Contribution stays
/// chosen but hidden and can't be toggled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Overlaid {
    pub chosen: Overlay,
    pub supplied: bool,
}

impl Overlaid {
    /// What goes over the frame.
    pub fn shown(self) -> Overlay {
        if self.supplied { self.chosen } else { Overlay::None }
    }

    fn plain() -> Self {
        Self::default()
    }
}

/// A view as the bar, keys and menus name it, without its state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    Result,
    Split,
    SideBySide,
    TopAndBottom,
    Flicker,
}

/// How a comparison lays the result and what it is compared with out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// Split by a divider to drag.
    Split,
    /// Side by side, one shared pan and zoom.
    SideBySide,
    /// One above the other.
    TopAndBottom,
    /// One of the two in place, flipped with each press; `source` is the
    /// side compared with.
    Flicker { source: bool },
}

impl Layout {
    /// The bar's and menu's name for it.
    pub fn look(self) -> Look {
        match self {
            Self::Split => Look::Split,
            Self::SideBySide => Look::SideBySide,
            Self::TopAndBottom => Look::TopAndBottom,
            Self::Flicker { .. } => Look::Flicker,
        }
    }

    /// `look` as first shown: Flicker on the side compared with.
    fn first(look: Look) -> Option<Self> {
        match look {
            Look::Result => None,
            Look::Split => Some(Self::Split),
            Look::SideBySide => Some(Self::SideBySide),
            Look::TopAndBottom => Some(Self::TopAndBottom),
            Look::Flicker => Some(Self::Flicker { source: true }),
        }
    }
}

/// What the result is compared with: the frame on screen or another render
/// (`R`, its id).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target<R> {
    Frame { overlay: Overlaid },
    Render(R),
}

/// A comparison; `swapped` puts the result on the leading or top side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Compare<R> {
    pub layout: Layout,
    pub swapped: bool,
    pub target: Target<R>,
}

/// What turning the depth map off goes back to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Back<R> {
    Result,
    Frame { overlay: Overlaid },
    Compare(Compare<R>),
}

impl<R> Back<R> {
    /// The view it goes back to.
    pub fn mode<F>(self) -> Mode<F, R> {
        match self {
            Self::Result => Mode::Result,
            Self::Frame { overlay } => Mode::Frame { overlay },
            Self::Compare(c) => Mode::Compare(c),
        }
    }
}

/// The depth map's light, inside [`Mode::Map`].
///
/// - `Whole` → `Lit` or `Marked`: a frame picked (or, in Retouch, painted
///   from); `Lit` if it supplied part of a result whose map lights frames.
/// - `Lit` or `Marked` → another of either: another frame picked.
/// - → `Whole`: the light put out, the lit picture failed, or a new result
///   under the map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Light<F> {
    /// The whole map, each frame's part in its color.
    Whole,
    /// `frame`'s part lit, the rest dimmed; its picture is asked for with
    /// `token` ([`Action::Light`]).
    Lit { frame: F, token: u64 },
    /// A frame that supplied nothing: the map shows whole, its tick marked.
    Marked { frame: F },
}

/// What the viewer shows, owned by [`View`] and changed only by
/// [`View::on`], each change logged at debug level.
///
/// - → `Result`: Result chosen, Compare off, the comparison stopped,
///   Retouch left, or a new result over a frame alone.
/// - → `Frame`: a frame picked from the result or a frame alone,
///   Contribution toggled, the map turned off from one (or lit for a frame,
///   outside Retouch), or no result to show.
/// - → `Compare`: a layout chosen (Flicker again flips), Compare on (the
///   last comparison), a frame or render compared with, Swap Sides, a render
///   traded, Contribution over its frame, or the map turned off from one. A
///   layout, Compare or frame compared with over the map leaves it first.
/// - → `Map`: Depth Map on with a map to show, lit for the frame on screen
///   when it has Contribution on; another [`Light`] while shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode<F, R> {
    /// The result alone.
    Result,
    /// The frame on screen alone (before a stack's first result).
    Frame { overlay: Overlaid },
    /// The result and what it is compared with, laid out together.
    Compare(Compare<R>),
    /// The result's depth map.
    Map { back: Back<R>, light: Light<F> },
}

impl<F, R> Mode<F, R> {
    /// What the depth map would go back to from here; `None` on the map.
    fn back(self) -> Option<Back<R>> {
        match self {
            Self::Result => Some(Back::Result),
            Self::Frame { overlay } => Some(Back::Frame { overlay }),
            Self::Compare(c) => Some(Back::Compare(c)),
            Self::Map { .. } => None,
        }
    }

    /// The comparison on screen, if one is.
    pub fn comparing(&self) -> Option<&Compare<R>> {
        match self {
            Self::Compare(c) => Some(c),
            Self::Result | Self::Frame { .. } | Self::Map { .. } => None,
        }
    }

    /// The frame on screen shows, alone or compared with (a render too).
    pub fn shows_source(&self) -> bool {
        matches!(self, Self::Frame { .. } | Self::Compare(_))
    }

    pub fn is_map(&self) -> bool {
        matches!(self, Self::Map { .. })
    }

    /// What goes over the frame on screen; none where no frame shows or it
    /// supplied nothing.
    pub fn overlay(&self) -> Overlay {
        self.overlaid().map_or(Overlay::None, Overlaid::shown)
    }

    /// The frame on screen's overlay, for Contribution's button; none where
    /// no frame shows.
    pub fn overlaid(&self) -> Option<Overlaid> {
        match self {
            Self::Frame { overlay } => Some(*overlay),
            Self::Compare(Compare { target: Target::Frame { overlay }, .. }) => Some(*overlay),
            Self::Compare(Compare { target: Target::Render(_), .. })
            | Self::Result
            | Self::Map { .. } => None,
        }
    }

    /// The overlay to change, where a frame is on screen.
    fn overlay_mut(&mut self) -> Option<&mut Overlaid> {
        match self {
            Self::Frame { overlay } => Some(overlay),
            Self::Compare(Compare { target: Target::Frame { overlay }, .. }) => Some(overlay),
            Self::Compare(Compare { target: Target::Render(_), .. })
            | Self::Result
            | Self::Map { .. } => None,
        }
    }

    /// The frame's overlay it holds, on screen or under the map.
    fn overlays_mut(&mut self) -> Option<&mut Overlaid> {
        match self {
            Self::Map { back: Back::Frame { overlay }, .. }
            | Self::Map {
                back: Back::Compare(Compare { target: Target::Frame { overlay }, .. }),
                ..
            } => Some(overlay),
            Self::Map { .. } => None,
            on_screen => on_screen.overlay_mut(),
        }
    }

    /// The frame lit or marked on the map, while one is.
    pub fn lit(&self) -> Option<&F> {
        match self {
            Self::Map { light: Light::Lit { frame, .. } | Light::Marked { frame }, .. } => {
                Some(frame)
            }
            Self::Map { light: Light::Whole, .. }
            | Self::Result
            | Self::Frame { .. }
            | Self::Compare(_) => None,
        }
    }

    /// The lit picture's token, while a frame is lit.
    pub fn lit_token(&self) -> Option<u64> {
        match self {
            Self::Map { light: Light::Lit { token, .. }, .. } => Some(*token),
            _ => None,
        }
    }

    /// The bar's and menu's name for it; `None` while a frame alone or
    /// the map is shown.
    pub fn look(&self) -> Option<Look> {
        match self {
            Self::Result => Some(Look::Result),
            Self::Compare(c) => Some(c.layout.look()),
            Self::Frame { .. } | Self::Map { .. } => None,
        }
    }

    /// For the log: the shape, without the frames and renders it names.
    fn name(&self) -> String {
        let back = |b: &Back<R>| match b {
            Back::Result => "result".to_string(),
            Back::Frame { overlay } => format!("frame ({overlay:?})"),
            Back::Compare(c) => compare(c),
        };
        fn compare<R>(c: &Compare<R>) -> String {
            let target = match &c.target {
                Target::Frame { overlay } => format!("a frame ({overlay:?})"),
                Target::Render(_) => "a render".to_string(),
            };
            let sides = if c.swapped { ", swapped" } else { "" };
            format!("compare ({:?}{sides}, with {target})", c.layout)
        }
        match self {
            Self::Result => "result".to_string(),
            Self::Frame { overlay } => format!("frame ({overlay:?})"),
            Self::Compare(c) => compare(c),
            Self::Map { back: b, light } => {
                let light = match light {
                    Light::Whole => "whole".to_string(),
                    Light::Lit { token, .. } => format!("lit, picture {token}"),
                    Light::Marked { .. } => "a frame left out".to_string(),
                };
                format!("map ({light}, back to {})", back(b))
            }
        }
    }
}

/// A frame an event names, and whether it supplied part of the result
/// (only such a frame shows Contribution or lights the map).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pick<F> {
    pub id: F,
    pub supplied: bool,
}

/// What the view needs to know of the result shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Facts {
    /// It has a depth map to show.
    pub map: bool,
    /// Its map names the frames, so a frame picked over it is lit there.
    pub lights: bool,
}

/// What happened, from the user or the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event<F, R> {
    /// A look chosen. Needs a result; no Flicker in Retouch.
    Look(Look),
    /// The bar's Compare: on, or off to the result alone.
    ToggleCompare,
    /// The sides exchanged (S), now and for the next comparison.
    SwapSides,
    /// The comparison stopped (Esc, or its render unpinned); the sides reset.
    StopCompare,
    /// A frame picked or stepped to: lit over a map that lights frames
    /// (`put_out`: picking the lit one again puts the light out), else shown.
    ShowFrame { frame: Pick<F>, put_out: bool },
    /// A frame to compare the result with.
    CompareFrame(Pick<F>),
    /// A render to compare the result with.
    CompareRender(R),
    /// The render compared with (`shown`) is shown and `before` compared
    /// with; the sides swap so neither picture moves.
    Traded { shown: R, before: R },
    /// Contribution toggled; nothing over a frame that supplied nothing.
    ToggleContribution,
    /// Depth Map toggled; `on_screen` is lit as the map opens when it has
    /// Contribution on.
    ToggleMap { on_screen: Option<Pick<F>> },
    /// Light a frame on the map shown: Retouch's frame painted from.
    Light(Pick<F>),
    /// The map's light put out: the map whole.
    Unlight,
    /// The lit picture asked for with `token` couldn't be made.
    LightFailed { token: u64 },
    /// A result is shown, or none. `keep` leaves the view as it is (iOS: a
    /// frame was picked while it opened); `supplied`: the selected frame
    /// supplied part of it.
    Shown { result: Option<Facts>, keep: bool, supplied: bool },
    /// Retouch opened with what it was left showing, or closed (`None`).
    Retouch(Option<Retouched<R>>),
}

/// What Retouch was left showing and its last layout, kept by the desktop's
/// Retouch between opens ([`View::retouched`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Retouched<R> {
    pub back: Back<R>,
    pub layout: Layout,
}

impl<R> Default for Retouched<R> {
    /// Retouch as it first opens: the canvas alone, Split next.
    fn default() -> Self {
        Self { back: Back::Result, layout: Layout::Split }
    }
}

/// What the app does, in order, after an event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action<F> {
    /// Select frame `F` and show it with the view's overlay.
    Show(F),
    /// The frame's overlay changed: show it again.
    Reload,
    /// Make the map lit for `frame`, replacing any earlier request (the
    /// shown picture stays until it lands); failure comes back as
    /// [`Event::LightFailed`] with `token`.
    Light { frame: F, token: u64 },
    /// Drop the lit picture and any being made.
    Unlight,
}

/// [`View::on`]'s answer: what to do, and the view now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer<F, R> {
    pub actions: Vec<Action<F>>,
    pub now: Mode<F, R>,
}

/// The viewer, or the desktop's Retouch, which shows its canvas and source
/// through the same view.
///
/// `Viewer` → `Retouch`: opened; `Retouch` → `Viewer`: closed, to the result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Place {
    Viewer,
    /// Its layout used last: the one Compare turns on there.
    Retouch {
        layout: Layout,
    },
}

/// The viewer's view and its memories.
///
/// Lifecycle: its [`Mode`] (result, frame, comparison, depth map) with its
/// [`Back`], [`Light`] and [`Overlaid`]; changed only by [`View::on`], whose
/// table tests are the behavior (ADR-149).
#[derive(Clone, Debug)]
pub struct View<F, R> {
    now: Mode<F, R>,
    place: Place,
    /// The last comparison outside Retouch (layout, swapped): Compare
    /// turns it on again.
    last: (Layout, bool),
    /// The result shown, if one is.
    result: Option<Facts>,
    /// The selected frame supplied part of the result, per the last event
    /// naming it; copied into every [`Overlaid`] the mode holds.
    supplied: bool,
    /// The last lit picture's token.
    token: u64,
}

impl<F: Clone + PartialEq + std::fmt::Debug, R: Clone + PartialEq + std::fmt::Debug> Default
    for View<F, R>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<F: Clone + PartialEq + std::fmt::Debug, R: Clone + PartialEq + std::fmt::Debug> View<F, R> {
    /// A frame alone, no result yet.
    pub fn new() -> Self {
        Self {
            now: Mode::Frame { overlay: Overlaid::plain() },
            place: Place::Viewer,
            last: (Layout::Split, false),
            result: None,
            supplied: false,
            token: 0,
        }
    }

    pub fn now(&self) -> &Mode<F, R> {
        &self.now
    }

    /// The layout Compare turns on next.
    pub fn next_layout(&self) -> Layout {
        let layout = match self.place {
            Place::Viewer => self.last.0,
            Place::Retouch { layout } => layout,
        };
        Layout::first(layout.look()).unwrap_or(layout)
    }

    /// In Retouch, what it would be left showing were it closed now.
    pub fn retouched(&self) -> Option<Retouched<R>> {
        match self.place {
            Place::Retouch { layout } => {
                Some(Retouched { back: self.leave_map().back().unwrap_or(Back::Result), layout })
            }
            Place::Viewer => None,
        }
    }

    /// Take `event`: the view changes, and the answer says what to do.
    pub fn on(&mut self, event: Event<F, R>) -> Answer<F, R> {
        let before = self.now.clone();
        let mut actions = Vec::new();
        self.step(event, &mut actions);
        if let Some(overlay) = self.now.overlays_mut() {
            overlay.supplied = self.supplied;
        }
        // Compare turns on the comparison last shown.
        if let Mode::Compare(c) = &self.now {
            match &mut self.place {
                Place::Viewer => self.last = (c.layout, c.swapped),
                Place::Retouch { layout } => *layout = c.layout,
            }
        }
        // The lit picture follows the light: asked for as a frame is lit,
        // dropped as it goes out.
        match (before.lit_token(), &self.now) {
            (before, Mode::Map { light: Light::Lit { frame, token }, .. })
                if before != Some(*token) =>
            {
                actions.push(Action::Light { frame: frame.clone(), token: *token });
            }
            (Some(_), now) if now.lit_token().is_none() => actions.push(Action::Unlight),
            _ => {}
        }
        if self.now != before {
            tracing::debug!("view: {} -> {}", before.name(), self.now.name());
        }
        Answer { actions, now: self.now.clone() }
    }

    fn step(&mut self, event: Event<F, R>, actions: &mut Vec<Action<F>>) {
        let retouch = matches!(self.place, Place::Retouch { .. });
        match event {
            Event::Look(look) => {
                if self.result.is_none() || (retouch && look == Look::Flicker) {
                    return;
                }
                let flipped = match &self.now {
                    Mode::Compare(Compare { layout: Layout::Flicker { source }, .. })
                        if look == Look::Flicker =>
                    {
                        Some(Layout::Flicker { source: !source })
                    }
                    _ => None,
                };
                let from = self.leave_map();
                self.now = match flipped.or_else(|| Layout::first(look)) {
                    None => Mode::Result,
                    Some(layout) => self.compare_from(from, layout),
                };
            }
            Event::ToggleCompare => {
                if self.result.is_none() {
                    return;
                }
                let on = self.now.comparing().is_some();
                self.now = match self.leave_map() {
                    _ if on => Mode::Result,
                    // Off over the map, so it turns on: the map's comparison, or the last.
                    Mode::Compare(c) => Mode::Compare(c),
                    from => self.compare_from(from, self.next_layout()),
                };
            }
            Event::SwapSides => {
                if retouch {
                    return;
                }
                self.last.1 = !self.last.1;
                if let Mode::Compare(c) | Mode::Map { back: Back::Compare(c), .. } = &mut self.now {
                    c.swapped = !c.swapped;
                }
            }
            Event::StopCompare => {
                if retouch {
                    return;
                }
                self.now = Mode::Result;
                self.last.1 = false;
            }
            Event::ShowFrame { frame, put_out } => {
                if retouch {
                    return;
                }
                self.supplied = frame.supplied;
                if self.now.is_map() && self.result.is_some_and(|r| r.lights) {
                    return self.light(frame, put_out);
                }
                self.now = match self.leave_map() {
                    Mode::Result | Mode::Map { .. } => Mode::Frame { overlay: Overlaid::plain() },
                    Mode::Frame { overlay } => Mode::Frame { overlay },
                    Mode::Compare(c) => {
                        Mode::Compare(Compare { target: Self::frame_target(&c.target), ..c })
                    }
                };
                actions.push(Action::Show(frame.id));
            }
            Event::CompareFrame(frame) => {
                if retouch || self.result.is_none() {
                    return;
                }
                self.supplied = frame.supplied;
                self.now = match self.leave_map() {
                    Mode::Compare(c) => {
                        Mode::Compare(Compare { target: Self::frame_target(&c.target), ..c })
                    }
                    from => self.compare_from(from, self.next_layout()),
                };
                actions.push(Action::Show(frame.id));
            }
            Event::CompareRender(render) => {
                if retouch || self.result.is_none() {
                    return;
                }
                let target = Target::Render(render);
                self.now = match self.leave_map() {
                    Mode::Compare(c) => Mode::Compare(Compare { target, ..c }),
                    from => {
                        Mode::Compare(Compare { target, ..self.compare(from, self.next_layout()) })
                    }
                };
            }
            Event::Traded { shown, before } => {
                // On the map too, over the comparison it goes back to.
                if let Mode::Compare(c) | Mode::Map { back: Back::Compare(c), .. } = &mut self.now
                    && c.target == Target::Render(shown)
                {
                    c.target = Target::Render(before);
                    c.swapped = !c.swapped;
                }
            }
            Event::ToggleContribution => {
                if retouch || !self.supplied {
                    return;
                }
                let Some(overlay) = self.now.overlay_mut() else { return };
                overlay.chosen = match overlay.chosen {
                    Overlay::Contribution => Overlay::None,
                    Overlay::None => Overlay::Contribution,
                };
                actions.push(Action::Reload);
            }
            Event::ToggleMap { on_screen } => {
                if !self.result.is_some_and(|r| r.map) {
                    return;
                }
                let Some(back) = self.now.clone().back() else {
                    return self.leave_map_lit(actions);
                };
                let overlay = self.now.overlay();
                self.now = Mode::Map { back, light: Light::Whole };
                // Contribution and the lit map are one light: it carries.
                if let Some(frame) = on_screen
                    && !retouch
                    && overlay == Overlay::Contribution
                {
                    self.light(frame, false);
                }
            }
            Event::Light(frame) => {
                if self.now.is_map() {
                    self.light(frame, false);
                }
            }
            Event::Unlight => {
                if let Mode::Map { light, .. } = &mut self.now {
                    *light = Light::Whole;
                }
            }
            Event::LightFailed { token } => {
                if let Mode::Map { light, .. } = &mut self.now
                    && matches!(light, Light::Lit { token: t, .. } if *t == token)
                {
                    tracing::warn!("the depth map couldn't be lit (picture {token}): shown whole");
                    *light = Light::Whole;
                }
            }
            Event::Shown { result, keep, supplied } => {
                self.result = result;
                // Without a frame map no frame supplied anything it shows.
                self.supplied = supplied && result.is_some_and(|r| r.lights);
                let Some(facts) = result else {
                    self.now = Mode::Frame { overlay: Overlaid::plain() };
                    return;
                };
                self.now = match (self.now.clone(), keep || retouch) {
                    (Mode::Map { back, .. }, true) if !facts.map => back.mode(),
                    // A light was the old result's: whole for this one.
                    (Mode::Map { back, .. }, true) => Mode::Map { back, light: Light::Whole },
                    (now, true) => now,
                    (Mode::Frame { .. } | Mode::Map { back: Back::Frame { .. }, .. }, false) => {
                        Mode::Result
                    }
                    (Mode::Map { back, .. }, false) => back.mode(),
                    (now @ (Mode::Result | Mode::Compare(_)), false) => now,
                };
            }
            Event::Retouch(open) => match (self.place, open) {
                (Place::Viewer, Some(Retouched { back, layout })) => {
                    self.place = Place::Retouch { layout };
                    self.now = back.mode();
                }
                (Place::Retouch { .. }, None) => {
                    self.place = Place::Viewer;
                    self.now = Mode::Result;
                    self.last.1 = false;
                }
                (Place::Viewer, None) | (Place::Retouch { .. }, Some(_)) => {}
            },
        }
    }

    /// The view under the map, or itself.
    fn leave_map(&self) -> Mode<F, R> {
        match self.now.clone() {
            Mode::Map { back, .. } => back.mode(),
            now => now,
        }
    }

    /// Depth Map turned off, the light carried outside Retouch: a lit frame
    /// comes up with Contribution on, a marked one plain with its choice kept.
    fn leave_map_lit(&mut self, actions: &mut Vec<Action<F>>) {
        let Mode::Map { back, light } = self.now.clone() else { return };
        let chosen = back.clone().mode::<F>().overlaid().unwrap_or_default().chosen;
        let (frame, overlay) = match light {
            Light::Lit { frame, .. } if self.place == Place::Viewer => {
                (frame, Overlaid { chosen: Overlay::Contribution, supplied: true })
            }
            Light::Marked { frame } if self.place == Place::Viewer => {
                (frame, Overlaid { chosen, supplied: false })
            }
            Light::Whole | Light::Lit { .. } | Light::Marked { .. } => {
                self.now = back.mode();
                return;
            }
        };
        self.supplied = overlay.supplied;
        self.now = match back {
            Back::Compare(c) => Mode::Compare(Compare { target: Target::Frame { overlay }, ..c }),
            Back::Result | Back::Frame { .. } => Mode::Frame { overlay },
        };
        actions.push(Action::Show(frame));
    }

    /// Light `frame` on the map; `put_out` puts the light out if it is lit.
    fn light(&mut self, frame: Pick<F>, put_out: bool) {
        let lights = self.result.is_some_and(|r| r.lights);
        let Mode::Map { light, .. } = &mut self.now else { return };
        let now = match light {
            Light::Lit { frame: f, .. } | Light::Marked { frame: f } => Some(&*f),
            Light::Whole => None,
        };
        if now == Some(&frame.id) {
            if put_out {
                *light = Light::Whole;
            }
            return;
        }
        *light = match frame.supplied && lights {
            true => {
                self.token += 1;
                Light::Lit { frame: frame.id, token: self.token }
            }
            false => Light::Marked { frame: frame.id },
        };
    }

    /// A frame in place of `target`, keeping a frame's overlay.
    fn frame_target(target: &Target<R>) -> Target<R> {
        match target {
            Target::Frame { overlay } => Target::Frame { overlay: *overlay },
            Target::Render(_) => Target::Frame { overlay: Overlaid::plain() },
        }
    }

    /// A comparison in `layout` from `from`, the sides as last left.
    fn compare(&self, from: Mode<F, R>, layout: Layout) -> Compare<R> {
        let swapped = match self.place {
            Place::Viewer => self.last.1,
            Place::Retouch { .. } => false,
        };
        match from {
            Mode::Compare(c) => Compare { layout, ..c },
            from => Compare {
                layout,
                swapped,
                target: Target::Frame { overlay: from.overlaid().unwrap_or_default() },
            },
        }
    }

    fn compare_from(&self, from: Mode<F, R>, layout: Layout) -> Mode<F, R> {
        Mode::Compare(self.compare(from, layout))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type V = View<&'static str, &'static str>;
    type M = Mode<&'static str, &'static str>;
    type E = Event<&'static str, &'static str>;
    type A = Action<&'static str>;

    /// Contribution or nothing, over a frame that supplied part of the result.
    const C: Overlaid = Overlaid { chosen: Overlay::Contribution, supplied: true };
    const N: Overlaid = Overlaid { chosen: Overlay::None, supplied: true };
    /// Nothing, over a frame that supplied nothing or none named yet.
    const PLAIN: Overlaid = Overlaid { chosen: Overlay::None, supplied: false };
    const LIGHTS: Facts = Facts { map: true, lights: true };

    fn shown(facts: Facts) -> E {
        Event::Shown { result: Some(facts), keep: false, supplied: true }
    }

    fn pick(id: &'static str) -> Pick<&'static str> {
        Pick { id, supplied: true }
    }

    fn left_out(id: &'static str) -> Pick<&'static str> {
        Pick { id, supplied: false }
    }

    fn show(id: &'static str) -> E {
        Event::ShowFrame { frame: pick(id), put_out: false }
    }

    fn press(id: &'static str) -> E {
        Event::ShowFrame { frame: pick(id), put_out: true }
    }

    fn map_on(on_screen: Option<Pick<&'static str>>) -> E {
        Event::ToggleMap { on_screen }
    }

    fn compare(layout: Layout, swapped: bool, overlay: Overlaid) -> M {
        Mode::Compare(Compare { layout, swapped, target: Target::Frame { overlay } })
    }

    fn with_render(layout: Layout, swapped: bool, id: &'static str) -> M {
        Mode::Compare(Compare { layout, swapped, target: Target::Render(id) })
    }

    fn map(back: Back<&'static str>, light: Light<&'static str>) -> M {
        Mode::Map { back, light }
    }

    fn lit(frame: &'static str, token: u64) -> Light<&'static str> {
        Light::Lit { frame, token }
    }

    const SPLIT: Layout = Layout::Split;
    const FLICKER_SOURCE: Layout = Layout::Flicker { source: true };
    const FLICKER_RESULT: Layout = Layout::Flicker { source: false };

    /// A view with a result whose map lights frames, after `events`.
    fn after(events: &[E]) -> V {
        let mut view = V::new();
        view.on(shown(LIGHTS));
        for event in events {
            view.on(event.clone());
        }
        view
    }

    /// Name, setup events (after `after`'s result), event, view, actions.
    type Row = (&'static str, Vec<E>, E, M, Vec<A>);

    fn check(rows: Vec<Row>) {
        for (name, setup, event, mode, actions) in rows {
            let mut view = after(&setup);
            let answer = view.on(event);
            assert_eq!(answer.now, mode, "{name}: the view");
            assert_eq!(answer.actions, actions, "{name}: the actions");
        }
    }

    #[test]
    fn looks() {
        let side = Layout::SideBySide;
        check(vec![
            (
                "Result from a frame",
                vec![show("a")],
                Event::Look(Look::Result),
                Mode::Result,
                vec![],
            ),
            (
                "a layout from the result",
                vec![],
                Event::Look(Look::Split),
                compare(SPLIT, false, N),
                vec![],
            ),
            (
                "a layout from a frame with Contribution keeps it",
                vec![show("a"), Event::ToggleContribution],
                Event::Look(Look::Split),
                compare(SPLIT, false, C),
                vec![],
            ),
            (
                "another layout keeps the sides and target",
                vec![Event::CompareRender("r2"), Event::SwapSides],
                Event::Look(Look::SideBySide),
                with_render(side, true, "r2"),
                vec![],
            ),
            // Flicker's first press shows the side compared with.
            (
                "Flicker's first press",
                vec![],
                Event::Look(Look::Flicker),
                compare(FLICKER_SOURCE, false, N),
                vec![],
            ),
            (
                "Flicker again flips",
                vec![Event::Look(Look::Flicker)],
                Event::Look(Look::Flicker),
                compare(FLICKER_RESULT, false, N),
                vec![],
            ),
            (
                "and back",
                vec![Event::Look(Look::Flicker), Event::Look(Look::Flicker)],
                Event::Look(Look::Flicker),
                compare(FLICKER_SOURCE, false, N),
                vec![],
            ),
            (
                "Flicker from another layout is a first press",
                vec![Event::Look(Look::Split)],
                Event::Look(Look::Flicker),
                compare(FLICKER_SOURCE, false, N),
                vec![],
            ),
            (
                "Flicker over the map from Flicker is a first press",
                vec![Event::Look(Look::Flicker), Event::Look(Look::Flicker), map_on(None)],
                Event::Look(Look::Flicker),
                compare(FLICKER_SOURCE, false, N),
                vec![],
            ),
            (
                "a layout over the map leaves it for its comparison",
                vec![Event::Look(Look::Split), map_on(None), show("a")],
                Event::Look(Look::TopAndBottom),
                compare(Layout::TopAndBottom, false, N),
                vec![A::Unlight],
            ),
            (
                "a layout over a lit map doesn't carry the light",
                vec![show("a"), map_on(None), show("b")],
                Event::Look(Look::Split),
                compare(SPLIT, false, N),
                vec![A::Unlight],
            ),
        ]);
        // Without a result there is nothing to compare.
        let mut view = V::new();
        assert_eq!(view.on(Event::Look(Look::Split)).now, Mode::Frame { overlay: PLAIN });
        assert_eq!(view.on(E::ToggleCompare).now, Mode::Frame { overlay: PLAIN });
    }

    #[test]
    fn compare_on_and_off() {
        check(vec![
            ("on from the result", vec![], E::ToggleCompare, compare(SPLIT, false, N), vec![]),
            ("off", vec![E::ToggleCompare], E::ToggleCompare, Mode::Result, vec![]),
            (
                "on from a frame, its overlay kept",
                vec![show("a"), Event::ToggleContribution],
                E::ToggleCompare,
                compare(SPLIT, false, C),
                vec![],
            ),
            // Compare over a map reached from a comparison turns it on.
            (
                "over a map reached from a compare",
                vec![Event::Look(Look::SideBySide), map_on(None)],
                E::ToggleCompare,
                compare(Layout::SideBySide, false, N),
                vec![],
            ),
            (
                "over a lit map reached from a compare, no light carried",
                vec![Event::Look(Look::SideBySide), map_on(None), show("b")],
                E::ToggleCompare,
                compare(Layout::SideBySide, false, N),
                vec![A::Unlight],
            ),
            (
                "over a map reached from the result: the last comparison",
                vec![Event::Look(Look::TopAndBottom), Event::Look(Look::Result), map_on(None)],
                E::ToggleCompare,
                compare(Layout::TopAndBottom, false, N),
                vec![],
            ),
        ]);
    }

    /// Compare remembers layout and sides; Flicker comes back as first
    /// pressed; stopping resets the sides.
    #[test]
    fn what_compare_remembers() {
        check(vec![
            (
                "the layout",
                vec![Event::Look(Look::TopAndBottom), E::ToggleCompare],
                E::ToggleCompare,
                compare(Layout::TopAndBottom, false, N),
                vec![],
            ),
            (
                "the sides",
                vec![E::ToggleCompare, E::SwapSides, E::ToggleCompare],
                E::ToggleCompare,
                compare(SPLIT, true, N),
                vec![],
            ),
            (
                "the sides swapped with no comparison on",
                vec![E::SwapSides],
                E::ToggleCompare,
                compare(SPLIT, true, N),
                vec![],
            ),
            (
                "Flicker, as first pressed",
                vec![Event::Look(Look::Flicker), Event::Look(Look::Flicker), E::ToggleCompare],
                E::ToggleCompare,
                compare(FLICKER_SOURCE, false, N),
                vec![],
            ),
            (
                "stopping resets the sides",
                vec![E::ToggleCompare, E::SwapSides, E::StopCompare],
                E::ToggleCompare,
                compare(SPLIT, false, N),
                vec![],
            ),
            (
                "a frame compared with, in the layout last used",
                vec![Event::Look(Look::SideBySide), Event::Look(Look::Result)],
                Event::CompareFrame(pick("a")),
                compare(Layout::SideBySide, false, N),
                vec![A::Show("a")],
            ),
            (
                "a render compared with, in the layout last used",
                vec![Event::Look(Look::SideBySide), Event::Look(Look::Result)],
                Event::CompareRender("r2"),
                with_render(Layout::SideBySide, false, "r2"),
                vec![],
            ),
        ]);
        let view = after(&[Event::Look(Look::TopAndBottom)]);
        assert_eq!(view.next_layout(), Layout::TopAndBottom);
    }

    #[test]
    fn frames() {
        check(vec![
            (
                "from the result, plain",
                vec![],
                show("a"),
                Mode::Frame { overlay: N },
                vec![A::Show("a")],
            ),
            (
                "from a frame, its overlay kept",
                vec![show("a"), Event::ToggleContribution],
                show("b"),
                Mode::Frame { overlay: C },
                vec![A::Show("b")],
            ),
            (
                "in a comparison, compared with",
                vec![E::ToggleCompare],
                show("b"),
                compare(SPLIT, false, N),
                vec![A::Show("b")],
            ),
            (
                "in a comparison with a render, a frame",
                vec![Event::CompareRender("r2")],
                show("b"),
                compare(SPLIT, false, N),
                vec![A::Show("b")],
            ),
            (
                "compared with from a frame, its overlay kept",
                vec![show("a"), Event::ToggleContribution],
                Event::CompareFrame(pick("b")),
                compare(SPLIT, false, C),
                vec![A::Show("b")],
            ),
            (
                "a render traded for the one compared: the sides swap",
                vec![Event::CompareRender("r2")],
                Event::Traded { shown: "r2", before: "r1" },
                with_render(SPLIT, true, "r1"),
                vec![],
            ),
            (
                "a render traded under the depth map: the comparison it goes back to swaps",
                vec![Event::CompareRender("r2"), map_on(None)],
                Event::Traded { shown: "r2", before: "r1" },
                map(
                    Back::Compare(Compare {
                        layout: SPLIT,
                        swapped: true,
                        target: Target::Render("r1"),
                    }),
                    Light::Whole,
                ),
                vec![],
            ),
            (
                "a trade of another render: nothing",
                vec![Event::CompareRender("r2")],
                Event::Traded { shown: "r3", before: "r1" },
                with_render(SPLIT, false, "r2"),
                vec![],
            ),
        ]);
    }

    #[test]
    fn contribution() {
        let on = Event::ToggleContribution;
        let left_out_x = Event::ShowFrame { frame: left_out("x"), put_out: false };
        let kept = Overlaid { chosen: Overlay::Contribution, supplied: false };
        check(vec![
            (
                "on over a frame",
                vec![show("a")],
                on.clone(),
                Mode::Frame { overlay: C },
                vec![A::Reload],
            ),
            (
                "off",
                vec![show("a"), on.clone()],
                on.clone(),
                Mode::Frame { overlay: N },
                vec![A::Reload],
            ),
            (
                "kept over a frame that supplied nothing, shown plain",
                vec![show("a"), on.clone()],
                left_out_x.clone(),
                Mode::Frame { overlay: kept },
                vec![A::Show("x")],
            ),
            (
                "not turned off there",
                vec![show("a"), on.clone(), left_out_x.clone()],
                on.clone(),
                Mode::Frame { overlay: kept },
                vec![],
            ),
            (
                "back on the next frame that supplied part",
                vec![show("a"), on.clone(), left_out_x.clone()],
                show("b"),
                Mode::Frame { overlay: C },
                vec![A::Show("b")],
            ),
            (
                "not turned on over a frame left out",
                vec![left_out_x.clone()],
                on.clone(),
                Mode::Frame { overlay: PLAIN },
                vec![],
            ),
            (
                "kept over a frame left out compared with",
                vec![show("a"), on.clone(), E::ToggleCompare],
                Event::CompareFrame(left_out("x")),
                compare(SPLIT, false, kept),
                vec![A::Show("x")],
            ),
            (
                "over a frame compared with",
                vec![E::ToggleCompare],
                on.clone(),
                compare(SPLIT, false, C),
                vec![A::Reload],
            ),
            (
                "not over a render",
                vec![Event::CompareRender("r2")],
                on.clone(),
                with_render(SPLIT, false, "r2"),
                vec![],
            ),
            ("not over the result", vec![], on.clone(), Mode::Result, vec![]),
            ("not over the map", vec![map_on(None)], on, map(Back::Result, Light::Whole), vec![]),
        ]);
    }

    #[test]
    fn the_map() {
        let on = Event::ToggleContribution;
        check(vec![
            ("on from the result", vec![], map_on(None), map(Back::Result, Light::Whole), vec![]),
            ("off again", vec![map_on(None)], map_on(None), Mode::Result, vec![]),
            (
                "on from a comparison, back to it",
                vec![E::ToggleCompare, map_on(None)],
                map_on(None),
                compare(SPLIT, false, N),
                vec![],
            ),
            (
                "on from a frame with Contribution: lit for it",
                vec![show("a"), on.clone()],
                map_on(Some(pick("a"))),
                map(Back::Frame { overlay: C }, lit("a", 1)),
                vec![A::Light { frame: "a", token: 1 }],
            ),
            (
                "on from a frame without: whole",
                vec![show("a")],
                map_on(Some(pick("a"))),
                map(Back::Frame { overlay: N }, Light::Whole),
                vec![],
            ),
            (
                "a frame picked over it is lit",
                vec![map_on(None)],
                show("b"),
                map(Back::Result, lit("b", 1)),
                vec![A::Light { frame: "b", token: 1 }],
            ),
            (
                "another frame: the light moves",
                vec![map_on(None), show("b")],
                show("c"),
                map(Back::Result, lit("c", 2)),
                vec![A::Light { frame: "c", token: 2 }],
            ),
            (
                "the list's pick of the frame lit: nothing",
                vec![map_on(None), show("b")],
                show("b"),
                map(Back::Result, lit("b", 1)),
                vec![],
            ),
            (
                "the lit tick pressed again: whole",
                vec![map_on(None), show("b")],
                press("b"),
                map(Back::Result, Light::Whole),
                vec![A::Unlight],
            ),
            (
                "a frame left out is marked",
                vec![map_on(None)],
                Event::ShowFrame { frame: left_out("x"), put_out: false },
                map(Back::Result, Light::Marked { frame: "x" }),
                vec![],
            ),
            (
                "put out",
                vec![map_on(None), show("b")],
                E::Unlight,
                map(Back::Result, Light::Whole),
                vec![A::Unlight],
            ),
            (
                "off, lit: the frame with Contribution on",
                vec![map_on(None), show("b")],
                map_on(None),
                Mode::Frame { overlay: C },
                vec![A::Show("b"), A::Unlight],
            ),
            (
                "off, lit, from a comparison: compared with, Contribution on",
                vec![E::ToggleCompare, map_on(None), show("b")],
                map_on(None),
                compare(SPLIT, false, C),
                vec![A::Show("b"), A::Unlight],
            ),
            (
                "off, a frame left out: that frame plain, Contribution kept for the next",
                vec![
                    show("a"),
                    on.clone(),
                    map_on(Some(pick("a"))),
                    Event::ShowFrame { frame: left_out("x"), put_out: false },
                ],
                map_on(None),
                Mode::Frame {
                    overlay: Overlaid { chosen: Overlay::Contribution, supplied: false },
                },
                vec![A::Show("x")],
            ),
            (
                "off, a frame left out, from a comparison: compared with, plain",
                vec![
                    E::ToggleCompare,
                    map_on(None),
                    Event::ShowFrame { frame: left_out("x"), put_out: false },
                ],
                map_on(None),
                compare(SPLIT, false, PLAIN),
                vec![A::Show("x")],
            ),
            (
                "a frame compared with over the map leaves it",
                vec![E::ToggleCompare, map_on(None), show("b")],
                Event::CompareFrame(pick("c")),
                compare(SPLIT, false, N),
                vec![A::Show("c"), A::Unlight],
            ),
        ]);
    }

    /// Contribution chosen before the map stays chosen over a frame left out.
    #[test]
    fn contribution_after_the_map_over_a_frame_left_out() {
        let mut view = after(&[show("a"), Event::ToggleContribution]);
        view.on(map_on(Some(pick("a"))));
        view.on(Event::ShowFrame { frame: left_out("x"), put_out: false });
        let answer = view.on(map_on(None));
        assert_eq!(
            answer.now.overlay(),
            Overlay::None,
            "a frame that supplied nothing shows no Contribution"
        );
        let kept = Overlaid { chosen: Overlay::Contribution, supplied: false };
        assert_eq!(answer.now.overlaid(), Some(kept), "the choice kept, for the next frame");
        assert_eq!(answer.actions, vec![A::Show("x")]);
    }

    #[test]
    fn a_map_that_does_not_light() {
        let mut view = V::new();
        view.on(shown(Facts { map: true, lights: false }));
        view.on(E::ToggleCompare);
        view.on(map_on(None));
        // A frame picked goes into what the map came from: compared with.
        let answer = view.on(show("a"));
        assert_eq!(answer.now, compare(SPLIT, false, N));
        assert_eq!(answer.actions, vec![A::Show("a")]);
        // Without a map, no map.
        let mut view = V::new();
        view.on(shown(Facts { map: false, lights: false }));
        assert_eq!(view.on(map_on(None)).now, Mode::Result);
    }

    /// A lit picture that fails puts the light out; a stale failure is ignored.
    #[test]
    fn a_lit_picture_that_fails() {
        check(vec![
            (
                "the picture asked for",
                vec![map_on(None), show("b")],
                E::LightFailed { token: 1 },
                map(Back::Result, Light::Whole),
                vec![A::Unlight],
            ),
            (
                "one overtaken",
                vec![map_on(None), show("b"), show("c")],
                E::LightFailed { token: 1 },
                map(Back::Result, lit("c", 2)),
                vec![],
            ),
            (
                "the map left",
                vec![map_on(None), show("b"), map_on(None)],
                E::LightFailed { token: 1 },
                Mode::Frame { overlay: C },
                vec![],
            ),
        ]);
    }

    #[test]
    fn a_result_shown() {
        let none = Event::Shown { result: None, keep: false, supplied: false };
        let keep = Event::Shown { result: Some(LIGHTS), keep: true, supplied: true };
        let no_map = Event::Shown {
            result: Some(Facts { map: false, lights: false }),
            keep: true,
            supplied: false,
        };
        check(vec![
            ("over a frame: the result", vec![show("a")], shown(LIGHTS), Mode::Result, vec![]),
            (
                "a comparison stays",
                vec![E::ToggleCompare],
                shown(LIGHTS),
                compare(SPLIT, false, N),
                vec![],
            ),
            (
                "over the map: what it goes back to",
                vec![E::ToggleCompare, map_on(None), show("b")],
                shown(LIGHTS),
                compare(SPLIT, false, N),
                vec![A::Unlight],
            ),
            (
                "over the map from a frame: the result",
                vec![show("a"), map_on(None)],
                shown(LIGHTS),
                Mode::Result,
                vec![],
            ),
            (
                "kept, over a frame",
                vec![show("a")],
                keep.clone(),
                Mode::Frame { overlay: N },
                vec![],
            ),
            (
                "kept, over the map lit: whole",
                vec![map_on(None), show("b")],
                keep,
                map(Back::Result, Light::Whole),
                vec![A::Unlight],
            ),
            (
                "kept, without a map: what it goes back to",
                vec![E::ToggleCompare, map_on(None)],
                no_map,
                compare(SPLIT, false, PLAIN),
                vec![],
            ),
            (
                "none: a frame alone",
                vec![E::ToggleCompare],
                none,
                Mode::Frame { overlay: PLAIN },
                vec![],
            ),
        ]);
    }

    /// What Retouch is left showing; none outside it.
    #[test]
    fn retouch_is_left_as_it_shows() {
        let mut view = after(&[]);
        assert_eq!(view.retouched(), None);
        view.on(Event::Retouch(Some(Retouched::default())));
        assert_eq!(view.retouched(), Some(Retouched::default()));
        view.on(Event::Look(Look::TopAndBottom));
        view.on(map_on(None));
        let left = view.retouched().unwrap();
        assert_eq!(left.layout, Layout::TopAndBottom);
        assert_eq!(left.back.clone().mode(), compare(Layout::TopAndBottom, false, N));
        view.on(Event::Retouch(None));
        assert_eq!(view.retouched(), None);
        assert_eq!(
            view.on(Event::Retouch(Some(left))).now,
            compare(Layout::TopAndBottom, false, N)
        );
    }

    #[test]
    fn retouch() {
        let open = Event::Retouch(Some(Retouched::default()));
        let close = Event::Retouch(None);
        check(vec![
            ("opens on the result", vec![show("a")], open.clone(), Mode::Result, vec![]),
            ("no Flicker", vec![open.clone()], Event::Look(Look::Flicker), Mode::Result, vec![]),
            (
                "its layouts",
                vec![open.clone()],
                Event::Look(Look::SideBySide),
                compare(Layout::SideBySide, false, N),
                vec![],
            ),
            (
                "Compare in its own layout",
                vec![
                    Event::Look(Look::TopAndBottom),
                    open.clone(),
                    Event::Look(Look::SideBySide),
                    Event::Look(Look::Result),
                ],
                E::ToggleCompare,
                compare(Layout::SideBySide, false, N),
                vec![],
            ),
            (
                "the viewer's layout isn't Retouch's",
                vec![
                    open.clone(),
                    Event::Look(Look::SideBySide),
                    close.clone(),
                    Event::Look(Look::Result),
                ],
                E::ToggleCompare,
                compare(SPLIT, false, N),
                vec![],
            ),
            (
                "opens on what it was left showing",
                vec![close.clone()],
                Event::Retouch(Some(Retouched {
                    back: Back::Compare(Compare {
                        layout: Layout::TopAndBottom,
                        swapped: false,
                        target: Target::Frame { overlay: N },
                    }),
                    layout: Layout::TopAndBottom,
                })),
                compare(Layout::TopAndBottom, false, N),
                vec![],
            ),
            (
                "opened again while open: the view stays",
                vec![open.clone(), Event::Look(Look::SideBySide), map_on(None)],
                open.clone(),
                map(split_side(), Light::Whole),
                vec![],
            ),
            (
                "closes on the result",
                vec![open.clone(), Event::Look(Look::SideBySide)],
                close,
                Mode::Result,
                vec![],
            ),
            (
                "its map lit for the frame painted from",
                vec![open.clone(), map_on(Some(pick("a")))],
                Event::Light(pick("b")),
                map(Back::Result, lit("b", 1)),
                vec![A::Light { frame: "b", token: 1 }],
            ),
            (
                "off, lit: back, nothing carried",
                vec![open.clone(), map_on(None), Event::Light(pick("b"))],
                map_on(None),
                Mode::Result,
                vec![A::Unlight],
            ),
            (
                "a new result keeps the view, its light out",
                vec![open.clone(), map_on(None), Event::Light(pick("b"))],
                shown(LIGHTS),
                map(Back::Result, Light::Whole),
                vec![A::Unlight],
            ),
            ("no frames picked", vec![open.clone()], show("a"), Mode::Result, vec![]),
            (
                "no Contribution",
                vec![open.clone(), Event::Look(Look::Split)],
                Event::ToggleContribution,
                compare(SPLIT, false, N),
                vec![],
            ),
            (
                "no swap",
                vec![open, Event::Look(Look::Split)],
                E::SwapSides,
                compare(SPLIT, false, N),
                vec![],
            ),
        ]);
    }

    fn split_side() -> Back<&'static str> {
        Back::Compare(Compare {
            layout: Layout::SideBySide,
            swapped: false,
            target: Target::Frame { overlay: N },
        })
    }

    #[test]
    fn swap_sides_over_the_map() {
        let mut view = after(&[E::ToggleCompare, map_on(None)]);
        view.on(E::SwapSides);
        assert_eq!(view.on(map_on(None)).now, compare(SPLIT, true, N));
    }
}
