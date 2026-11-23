//! The viewer's view for iOS (ADR-149): `deptheos_app::view`'s reducer
//! behind one call, frames named by path and renders by id.

use std::sync::Mutex;

use deptheos_app::Overlay;
use deptheos_app::view::{self, Back, Compare, Facts, Layout, Light, Look, Mode, Pick, Target};

/// Over the frame on screen: nothing, or what the result took from it, the
/// rest dimmed (`deptheos_app::Overlay`). A state names what is shown: none
/// over a frame that supplied nothing, whatever is chosen
/// ([`ContributionToggle`]).
#[uniffi::remote(Enum)]
pub enum Overlay {
    None,
    Contribution,
}

/// A look as the view menu, deck and keys name it
/// (`deptheos_app::view::Look`).
#[uniffi::remote(Enum)]
pub enum Look {
    Result,
    Split,
    SideBySide,
    TopAndBottom,
    Flicker,
}

/// How a comparison lays out the result and its compare: Flicker on the
/// compare (`source`), or the result (`deptheos_app::view::Layout`; Swift
/// names it `CompareLayout`, as SwiftUI has a `Layout`).
#[uniffi::remote(Enum)]
#[uniffi(name = "CompareLayout")]
pub enum Layout {
    Split,
    SideBySide,
    TopAndBottom,
    Flicker { source: bool },
}

/// What the result is compared with: the selected frame with its overlay,
/// or render `id`.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum CompareWith {
    Frame { overlay: Overlay },
    Render { id: String },
}

/// A comparison: its layout, its sides (`swapped`: the result leading or on
/// top) and what with.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ViewCompare {
    pub layout: Layout,
    pub swapped: bool,
    pub target: CompareWith,
}

/// What turning the depth map off goes back to.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum MapBack {
    Result,
    Frame { overlay: Overlay },
    Compare { compare: ViewCompare },
}

/// The depth map's light: whole, a frame lit (its picture asked with
/// `token`), or a frame that supplied nothing, marked with the map whole.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ViewLight {
    Whole,
    Lit { frame: String, token: u64 },
    Marked { frame: String },
}

/// What the viewer shows (`deptheos_app::view::Mode`).
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ViewState {
    Result,
    /// The selected frame alone (every stack's view before its first
    /// result).
    Frame {
        overlay: Overlay,
    },
    Compare {
        compare: ViewCompare,
    },
    Map {
        back: MapBack,
        light: ViewLight,
    },
}

/// A frame an event names, and whether it supplied part of the result.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct FramePick {
    pub path: String,
    pub supplied: bool,
}

/// What the view knows of the result shown: it has a depth map, and its
/// map names frames, so one can be lit (`deptheos_app::view::Facts`).
#[uniffi::remote(Record)]
pub struct Facts {
    pub map: bool,
    pub lights: bool,
}

/// Contribution's toggle over the frame on screen: chosen (`on`, kept
/// across frames), and whether it can change here: only over a frame that
/// supplied part of the result, where it shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ContributionToggle {
    pub on: bool,
    pub can_change: bool,
}

/// What happened (`deptheos_app::view::Event`).
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ViewEvent {
    Look {
        look: Look,
    },
    ToggleCompare,
    /// A frame tapped, scrubbed or stepped to: lit over a map that lights
    /// frames (with `put_out`, the lit one again puts it out), else the
    /// frame on screen.
    ShowFrame {
        frame: FramePick,
        put_out: bool,
    },
    CompareFrame {
        frame: FramePick,
    },
    CompareRender {
        id: String,
    },
    /// Compared-with render `shown` is now the one shown, and `before` is
    /// compared with: the sides swap.
    Traded {
        shown: String,
        before: String,
    },
    /// Over a frame that supplied part of the result.
    ToggleContribution,
    ToggleMap {
        on_screen: Option<FramePick>,
    },
    Unlight,
    LightFailed {
        token: u64,
    },
    /// A result shown, or none; with `keep` the view stays (a frame picked
    /// while it opened). `supplied`: the frame shown supplied part of it.
    Shown {
        result: Option<Facts>,
        keep: bool,
        supplied: bool,
    },
}

/// What the app does after an event, in order.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ViewAction {
    /// Frame `path` is the frame on screen now: select it.
    Show { path: String },
    /// The overlay on the frame on screen changed: line it up again.
    Reload,
    /// Light the map for `frame`; a failure returns as
    /// [`ViewEvent::LightFailed`] with `token`.
    Light { frame: String, token: u64 },
    /// Let go of the lit picture.
    Unlight,
}

/// The answer to an event: what to do, the view now, and Contribution's
/// toggle, none with no frame on screen.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ViewAnswer {
    pub actions: Vec<ViewAction>,
    pub now: ViewState,
    pub contribution: Option<ContributionToggle>,
}

/// The viewer's view, as on the desktop. No call blocks: each is a reducer
/// step under its own lock.
///
/// Lifecycle: the engine's `View` (ADR-149): the result, a frame alone, a
/// comparison or the depth map, with its overlay and light. Changed only by
/// `on`, whose answer's `now` the app keeps as `viewState`
/// (`ios/DeptheosMobile/Model/AppModel+View.swift`).
#[derive(uniffi::Object)]
pub struct ViewControl {
    view: Mutex<view::View<String, String>>,
}

#[uniffi::export]
impl ViewControl {
    /// A frame alone, no result yet.
    #[uniffi::constructor]
    pub fn new() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self { view: Mutex::new(view::View::new()) })
    }

    /// Take `event`.
    pub fn on(&self, event: ViewEvent) -> ViewAnswer {
        let mut view = crate::lock(&self.view);
        let answer = view.on(event.into());
        let contribution = answer.now.overlaid().map(|o| ContributionToggle {
            on: o.chosen == Overlay::Contribution,
            can_change: o.supplied,
        });
        ViewAnswer {
            actions: answer.actions.into_iter().map(ViewAction::from).collect(),
            now: answer.now.into(),
            contribution,
        }
    }
}

impl From<Compare<String>> for ViewCompare {
    fn from(c: Compare<String>) -> Self {
        let target = match c.target {
            Target::Frame { overlay } => CompareWith::Frame { overlay: overlay.shown() },
            Target::Render(id) => CompareWith::Render { id },
        };
        Self { layout: c.layout, swapped: c.swapped, target }
    }
}

impl From<Mode<String, String>> for ViewState {
    fn from(m: Mode<String, String>) -> Self {
        match m {
            Mode::Result => Self::Result,
            Mode::Frame { overlay } => Self::Frame { overlay: overlay.shown() },
            Mode::Compare(c) => Self::Compare { compare: c.into() },
            Mode::Map { back, light } => Self::Map {
                back: match back {
                    Back::Result => MapBack::Result,
                    Back::Frame { overlay } => MapBack::Frame { overlay: overlay.shown() },
                    Back::Compare(c) => MapBack::Compare { compare: c.into() },
                },
                light: match light {
                    Light::Whole => ViewLight::Whole,
                    Light::Lit { frame, token } => ViewLight::Lit { frame, token },
                    Light::Marked { frame } => ViewLight::Marked { frame },
                },
            },
        }
    }
}

impl From<FramePick> for Pick<String> {
    fn from(p: FramePick) -> Self {
        Self { id: p.path, supplied: p.supplied }
    }
}

impl From<ViewEvent> for view::Event<String, String> {
    fn from(e: ViewEvent) -> Self {
        match e {
            ViewEvent::Look { look } => Self::Look(look),
            ViewEvent::ToggleCompare => Self::ToggleCompare,
            ViewEvent::ShowFrame { frame, put_out } => {
                Self::ShowFrame { frame: frame.into(), put_out }
            }
            ViewEvent::CompareFrame { frame } => Self::CompareFrame(frame.into()),
            ViewEvent::CompareRender { id } => Self::CompareRender(id),
            ViewEvent::Traded { shown, before } => Self::Traded { shown, before },
            ViewEvent::ToggleContribution => Self::ToggleContribution,
            ViewEvent::ToggleMap { on_screen } => {
                Self::ToggleMap { on_screen: on_screen.map(Pick::from) }
            }
            ViewEvent::Unlight => Self::Unlight,
            ViewEvent::LightFailed { token } => Self::LightFailed { token },
            ViewEvent::Shown { result, keep, supplied } => Self::Shown { result, keep, supplied },
        }
    }
}

impl From<view::Action<String>> for ViewAction {
    fn from(a: view::Action<String>) -> Self {
        match a {
            view::Action::Show(path) => Self::Show { path },
            view::Action::Reload => Self::Reload,
            view::Action::Light { frame, token } => Self::Light { frame, token },
            view::Action::Unlight => Self::Unlight,
        }
    }
}
