//! Window layout remembered between runs: the main window's bounds, the
//! sidebar's width, the last folder images were opened from.
//!
//! Not preferences (nothing chosen, nothing worth setting aside on a bad
//! parse), so its own `ui_state.json` beside `preferences.json`; missing or
//! bad means defaults. Changes land in a GPUI global at once and on disk via
//! the store thread once settled ([`deptheos_app::engine::store::Store::keep`]),
//! so a drag writes once; quit waits for the store thread.

use std::path::{Path, PathBuf};

use gpui::{App, Bounds, Global, Pixels, WindowBounds, point, px, size};
use serde::{Deserialize, Serialize};

/// The sidebar's width, and the range a drag keeps it in.
pub const SIDEBAR_WIDTH: f32 = 300.0;
pub const SIDEBAR_RANGE: (f32, f32) = (240.0, 560.0);

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiState {
    /// The main window, as it was last placed.
    pub window: Option<SavedWindow>,
    pub sidebar_width: Option<f32>,
    /// The sidebar or the inspector was hidden.
    pub sidebar_hidden: bool,
    pub inspector_hidden: bool,
    /// Where the open dialog starts: the folder of the images last added.
    pub open_dir: Option<PathBuf>,
    /// Adobe DNG Converter was suggested once; never again.
    pub converter_suggested: bool,
}

/// A window's restore bounds (logical, screen coordinates) and whether it
/// was maximized over them.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedWindow {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
}

impl SavedWindow {
    pub fn from_bounds(bounds: WindowBounds) -> Self {
        let (b, maximized) = match bounds {
            WindowBounds::Windowed(b) => (b, false),
            WindowBounds::Maximized(b) => (b, true),
            // Full screen is not reopened: the restore bounds are kept.
            WindowBounds::Fullscreen(b) => (b, false),
        };
        Self {
            x: b.origin.x.into(),
            y: b.origin.y.into(),
            width: b.size.width.into(),
            height: b.size.height.into(),
            maximized,
        }
    }

    pub fn bounds(&self) -> Bounds<Pixels> {
        Bounds {
            origin: point(px(self.x), px(self.y)),
            size: size(px(self.width), px(self.height)),
        }
    }

    /// The bounds to open with, if enough is on one of `displays` to grab the
    /// title bar (a monitor may be gone); else `None`.
    pub fn restore(&self, displays: &[Bounds<Pixels>]) -> Option<WindowBounds> {
        let bounds = self.bounds();
        if !(self.width.is_finite() && self.height.is_finite()) || self.width < 1.0 {
            return None;
        }
        let visible = displays.iter().any(|d| {
            let i = d.intersect(&bounds);
            i.size.width >= px(100.0) && i.size.height >= px(40.0)
        });
        let restored = if self.maximized {
            WindowBounds::Maximized(bounds)
        } else {
            WindowBounds::Windowed(bounds)
        };
        visible.then_some(restored)
    }
}

impl UiState {
    pub fn sidebar_width(&self) -> f32 {
        self.sidebar_width.unwrap_or(SIDEBAR_WIDTH).clamp(SIDEBAR_RANGE.0, SIDEBAR_RANGE.1)
    }

    /// Read `path`; anything missing or unreadable is the defaults.
    pub fn load(path: &Path) -> Self {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                tracing::warn!("ignoring {}: {e}", path.display());
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    /// Write to `path` through a temporary file.
    pub fn save(&self, path: &Path) -> Result<(), deptheos_app::StoreError> {
        crate::prefs::save_json(path, self)
    }

    fn default_path() -> Option<PathBuf> {
        dirs::config_dir().map(|dir| dir.join("Deptheos").join("ui_state.json"))
    }
}

/// The state in effect and its file (none in tests).
#[derive(Default)]
struct Store {
    values: UiState,
    path: Option<PathBuf>,
}

impl Global for Store {}

/// Load the state from its default file.
pub fn init(cx: &mut App) {
    match UiState::default_path() {
        Some(path) => init_at(path, cx),
        None => cx.set_global(Store::default()),
    }
}

/// Load the state from `path`, and keep it there.
pub fn init_at(path: PathBuf, cx: &mut App) {
    let values = UiState::load(&path);
    cx.set_global(Store { values, path: Some(path) });
}

/// The state in effect (the defaults before [`init`]).
pub fn ui_state(cx: &App) -> UiState {
    cx.try_global::<Store>().map(|s| s.values.clone()).unwrap_or_default()
}

/// Change the state; the file follows shortly.
pub fn update_ui_state(cx: &mut App, f: impl FnOnce(&mut UiState)) {
    if cx.try_global::<Store>().is_none() {
        cx.set_global(Store::default());
    }
    let mut next = cx.global::<Store>().values.clone();
    f(&mut next);
    let store = cx.global_mut::<Store>();
    if next == store.values {
        return;
    }
    // Written on the store thread once settled, never here; quit waits for
    // it (ADR-143).
    if let Some(path) = store.path.clone() {
        let values = next.clone();
        deptheos_app::engine::store::store().keep("ui-state", move || {
            if let Err(e) = values.save(&path) {
                tracing::warn!("could not save {}: {e}", path.display());
            }
        });
    }
    store.values = next;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn round_trips_and_tolerates_bad_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("ui_state.json");
        assert_eq!(UiState::load(&path), UiState::default(), "missing");
        let state = UiState {
            window: Some(SavedWindow {
                x: 10.0,
                y: 20.0,
                width: 1400.0,
                height: 900.0,
                maximized: true,
            }),
            sidebar_width: Some(360.0),
            sidebar_hidden: false,
            inspector_hidden: true,
            open_dir: Some("/photos/bug".into()),
            converter_suggested: true,
        };
        state.save(&path).unwrap();
        assert_eq!(UiState::load(&path), state);
        std::fs::write(&path, b"{\"sidebar_width\": \"wide\"}").unwrap();
        assert_eq!(UiState::load(&path), UiState::default(), "bad field");
        std::fs::write(&path, b"{\"sidebar_width\": 9000, \"future\": 1}").unwrap();
        assert_eq!(UiState::load(&path).sidebar_width(), SIDEBAR_RANGE.1, "clamped");
    }

    /// Changes reach the file once the store thread writes, as at quit: the
    /// last, written once.
    #[gpui::test]
    fn changes_reach_the_file_once_the_store_is_flushed(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ui_state.json");
        cx.update(|cx| {
            init_at(path.clone(), cx);
            update_ui_state(cx, |s| s.sidebar_width = Some(400.0));
            update_ui_state(cx, |s| s.sidebar_hidden = true);
        });
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        assert!(deptheos_app::engine::store::store().flush(crate::test_support::STORE_DRAINS));
        let kept = UiState::load(&path);
        assert_eq!((kept.sidebar_width, kept.sidebar_hidden), (Some(400.0), true));
        assert_eq!(ui_state(&cx.app.borrow()), kept);
    }

    #[test]
    fn restores_only_onto_a_display() {
        let display =
            Bounds { origin: point(px(0.0), px(0.0)), size: size(px(1920.0), px(1080.0)) };
        let at =
            |x: f32, maximized| SavedWindow { x, y: 50.0, width: 1200.0, height: 800.0, maximized };
        assert_eq!(
            at(100.0, false).restore(&[display]),
            Some(WindowBounds::Windowed(at(100.0, false).bounds()))
        );
        assert_eq!(
            at(100.0, true).restore(&[display]),
            Some(WindowBounds::Maximized(at(100.0, true).bounds()))
        );
        // On a monitor to the right that is no longer connected.
        assert_eq!(at(2500.0, false).restore(&[display]), None);
        assert_eq!(at(100.0, false).restore(&[]), None);
    }
}
