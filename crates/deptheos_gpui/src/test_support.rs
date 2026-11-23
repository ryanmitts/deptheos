//! Headless windows for the GUI's tests.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long a test waits for the store thread to drain: all tests share
/// it, so under the whole suite it can take seconds.
pub const STORE_DRAINS: std::time::Duration = std::time::Duration::from_secs(30);

use gpui::{
    AnyWindowHandle, App, Bounds, Context, Entity, Render, TestAppContext, Window, WindowBounds,
    WindowOptions,
};
use gpui::{AppContext as _, point, px, size};

/// The Library at `root`, opened as the app opens it at launch.
pub fn opened(root: &Path) -> crate::start::Opened {
    crate::start::open(root).expect("the Library opens")
}

/// The roots of a Library of the tests' own, for a frame list to ask
/// whether its files are there: one for the whole run.
pub fn frames() -> Arc<deptheos_app::Frames> {
    static FRAMES: std::sync::OnceLock<(tempfile::TempDir, Arc<deptheos_app::Frames>)> =
        std::sync::OnceLock::new();
    FRAMES
        .get_or_init(|| {
            let dir = tempfile::tempdir().unwrap();
            let history = deptheos_app::library::history::History::open(dir.path().join("Library"))
                .expect("a test Library opens");
            let frames = history.frames().clone();
            (dir, frames)
        })
        .1
        .clone()
}

/// Initialize gpui-kit and open a window around `build`'s view, the way
/// `main` does (gpui-kit's root carries dialogs and notifications).
pub fn open<V: Render>(
    cx: &mut TestAppContext,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> (AnyWindowHandle, Entity<V>) {
    // Engine work runs on real threads (`crate::work`), which wake the
    // test scheduler from outside it.
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::words::install(crate::words::DesktopWords::english(), cx);
        crate::app::place_notices(cx);
        let bounds = Bounds { origin: point(px(0.0), px(0.0)), size: size(px(1280.0), px(800.0)) };
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..Default::default()
        };
        let (window, view) = gpui_kit::open_window(options, cx, build).expect("open a test window");
        (window, view)
    })
}

/// Run the scheduler until `done` holds: worker threads finish in real
/// time, so one `run_until_parked` is not enough.
pub fn wait_until(
    cx: &mut TestAppContext,
    what: &str,
    done: impl FnMut(&mut TestAppContext) -> bool,
) {
    wait_up_to(cx, Duration::from_secs(60), what, done);
}

/// [`wait_until`], for as long as `limit`: a stack of real frames.
pub fn wait_up_to(
    cx: &mut TestAppContext,
    limit: Duration,
    what: &str,
    mut done: impl FnMut(&mut TestAppContext) -> bool,
) {
    let deadline = Instant::now() + limit;
    loop {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Wait until the stack switched to is on screen: a switch is written on
/// the store thread first (`AppView::switch_to`).
pub fn switched(cx: &mut TestAppContext, app: &Entity<crate::app::AppView>) {
    wait_until(cx, "the switch of stacks", |cx| app.read_with(cx, |app, _| !app.is_switching()));
}

/// Run `f` on `view` in `window`, as a click or a key there would.
pub fn act<V: 'static, R>(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    view: &Entity<V>,
    f: impl FnOnce(&mut V, &mut Window, &mut Context<V>) -> R,
) -> R {
    cx.update_window(window, |_, window, cx| view.update(cx, |v, cx| f(v, window, cx))).unwrap()
}

/// The Library `app` has open, for a test to read its files back while it
/// runs: a second open would be refused, as the app holds its lock.
pub fn library(
    cx: &mut TestAppContext,
    app: &Entity<crate::app::AppView>,
) -> Arc<deptheos_app::library::history::History> {
    app.read_with(cx, |app, cx| app.renders.read(cx).store().clone())
}

/// Close `app`'s window and let the app go, as a quit does, until its
/// Library's lock is free for the next launch.
pub fn quit(cx: &mut TestAppContext, window: AnyWindowHandle, app: Entity<crate::app::AppView>) {
    let root = library(cx, &app).root().to_path_buf();
    cx.update_window(window, |_, window, _| window.remove_window()).unwrap();
    drop(app);
    // The window's entities are let go of as an update ends.
    wait_until(cx, "the Library let go", |cx| {
        cx.update(|_| {});
        deptheos_app::library::history::History::open(&root).is_ok()
    });
}

pub use deptheos_app::test_support::engine;

/// `count` checkerboard frames of 192 × 128, each slightly brighter than
/// the last.
pub fn write_frames(dir: &Path, count: u8) -> Vec<PathBuf> {
    deptheos_app::test_support::checkerboard_frames(dir, count, (192, 128))
}
