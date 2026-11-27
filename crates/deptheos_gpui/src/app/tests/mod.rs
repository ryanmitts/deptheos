//! The app view's tests, one file per area, on the helpers here.

use std::path::Path;

use deptheos_app::frames::folder;
use deptheos_app::library::export::ExportFormat;
use gpui::TestAppContext;
use gpui_kit::component::WindowExt as _;
use image::{ImageBuffer, Rgb};

use deptheos_app::{DepthConfig, SessionOptions, StackerConfig};

use super::run::{NotKept, stacked_message};
use super::working::TitleNote;
use super::*;
use crate::files::FileId;
use crate::renders::LibraryRead;
use crate::test_support::{act, engine, open, wait_until, write_frames};
use crate::viewer::{Look, ViewEvent, ViewMode};
use deptheos_app::view::{Back, Compare, Layout, Light, Target};

mod adjust;
mod batch;
mod compare;
mod e2e;
mod export;
mod frames;
mod library;
mod menus;
mod notices;
mod rail;
mod retouch;
mod samples;
mod saving;
mod stacking;
mod window;

/// Two checkerboard frames, the first blurred on its left quarter, the
/// second on its right, sharp between for the aligner to match.
fn write_quarter_blurred_frames(dir: &Path) -> Vec<PathBuf> {
    const W: u32 = 192;
    const H: u32 = 128;
    let sharp = |x: u32, y: u32| {
        if ((x / 8) + (y / 8)).is_multiple_of(2) { [220u8, 180, 60] } else { [30, 60, 120] }
    };
    (0..2)
        .map(|i| {
            let path = dir.join(format!("frame_{i}.png"));
            ImageBuffer::from_fn(W, H, |x, y| {
                let blurred = if i == 0 { x < W / 4 } else { x >= W * 3 / 4 };
                if !blurred {
                    return Rgb(sharp(x, y));
                }
                let r = 5i64;
                let mut sum = [0u64; 3];
                for dy in -r..=r {
                    for dx in -r..=r {
                        let sx = (x as i64 + dx).clamp(0, W as i64 - 1) as u32;
                        let sy = (y as i64 + dy).clamp(0, H as i64 - 1) as u32;
                        for (s, c) in sum.iter_mut().zip(sharp(sx, sy)) {
                            *s += u64::from(c);
                        }
                    }
                }
                let n = ((2 * r + 1) * (2 * r + 1)) as u64;
                Rgb(sum.map(|s| (s / n) as u8))
            })
            .save(&path)
            .unwrap();
            path
        })
        .collect()
}

fn stack(cx: &mut TestAppContext, window: gpui::AnyWindowHandle, app: &Entity<AppView>) {
    // Images added at launch are listed off the UI thread.
    wait_until(cx, "the images", |cx| app.read_with(cx, |app, cx| app.files.read(cx).can_stack()));
    act(cx, window, app, |app, window, cx| app.run_stacking(window, cx));
    wait_until(cx, "the stack and its save", |cx| {
        app.read_with(cx, |app, cx| !app.is_processing(cx) && !app.renders.read(cx).is_saving())
    });
}

fn launch(
    cx: &mut TestAppContext,
    engine: &Arc<Engine>,
    paths: Vec<PathBuf>,
    root: &Path,
) -> (gpui::AnyWindowHandle, Entity<AppView>) {
    let (engine, root) = (engine.clone(), crate::test_support::opened(root));
    let listing = !paths.is_empty();
    let opened = open(cx, |window, cx| cx.new(|cx| AppView::new(engine, paths, root, window, cx)));
    let app = opened.1.clone();
    // The Library listed, and the frames listed and sorted on the pool:
    // Stack waits for them.
    wait_until(cx, "the history and the list", |cx| {
        app.read_with(cx, |app, cx| {
            let files = app.files.read(cx);
            app.renders.read(cx).is_listed()
                && (!listing || files.counts().1 > 0)
                && !files.sorting()
        })
    });
    opened
}

/// A launch on a real engine: `frames` frames written to `dir`
/// ([`write_frames`]) and added, the Library in `dir/history`. Keep `dir`
/// bound: frames and Library go with it.
struct Fixture {
    dir: tempfile::TempDir,
    /// For a second launch on the same Library.
    engine: Arc<Engine>,
    window: gpui::AnyWindowHandle,
    app: Entity<AppView>,
}

impl Fixture {
    /// None without a GPU, and the test skips.
    fn launch(cx: &mut TestAppContext, frames: u8) -> Option<Self> {
        let engine = engine()?;
        let dir = tempfile::tempdir().unwrap();
        let paths = write_frames(dir.path(), frames);
        let (window, app) = launch(cx, &engine, paths, &dir.path().join("history"));
        Some(Self { dir, engine, window, app })
    }
}

fn exclude_first(cx: &mut TestAppContext, app: &Entity<AppView>) {
    let ids: Vec<FileId> = app
        .read_with(cx, |app, cx| app.files.read(cx).included())
        .into_iter()
        .map(|r| r.0)
        .collect();
    app.update(cx, |app, cx| app.files.update(cx, |f, cx| f.set_included(ids[0], false, cx)));
}

fn active(cx: &mut TestAppContext, app: &Entity<AppView>) -> Option<RenderId> {
    app.read_with(cx, |app, cx| app.renders.read(cx).active(cx).cloned())
}

fn open_render(
    cx: &mut TestAppContext,
    window: gpui::AnyWindowHandle,
    app: &Entity<AppView>,
    render: &RenderId,
) {
    act(cx, window, app, |app, window, cx| {
        app.renders.update(cx, |r, cx| r.open(render, window, cx))
    });
    wait_until(cx, "the render", |cx| {
        app.read_with(cx, |app, cx| {
            let renders = app.renders.read(cx);
            !renders.is_pending() && renders.active(cx) == Some(render)
        })
    });
}

/// Whether a dialog (Revert's confirmation) is open.
fn asking(cx: &mut TestAppContext, window: gpui::AnyWindowHandle) -> bool {
    use gpui_kit::component::WindowExt as _;
    cx.update_window(window, |_, window, cx| window.has_active_dialog(cx)).unwrap()
}

/// The Export sheet over the window, if that is the sheet up.
fn export_sheet(app: &AppView) -> Option<Entity<crate::export_dialog::ExportDialog>> {
    match &app.sheet {
        super::Sheet::Export(s) => Some(s.modal.dialog.clone()),
        _ => None,
    }
}

/// The Batch sheet over the window, if that is the sheet up.
fn batch_sheet(app: &AppView) -> Option<Entity<crate::batch_dialog::BatchDialog>> {
    match &app.sheet {
        super::Sheet::Batch(m) => Some(m.dialog.clone()),
        _ => None,
    }
}

/// Revert to the render shown, which asks first (it would lose unstacked
/// changes), and confirm it as its Revert button does.
fn revert_and_confirm(
    cx: &mut TestAppContext,
    window: gpui::AnyWindowHandle,
    app: &Entity<AppView>,
) {
    use gpui_kit::component::WindowExt as _;
    act(cx, window, app, |app, window, cx| app.revert_shown(window, cx));
    assert!(asking(cx, window), "Revert asks before losing the edits");
    act(cx, window, app, |app, window, cx| {
        window.close_dialog(cx);
        let to = app.snapshot(cx).showing().shown().map(|r| r.id.clone()).unwrap();
        app.revert_confirmed(&to, window, cx);
    });
    cx.run_until_parked();
}

/// Paint mode's state as these tests read it: flat, with nothing open
/// (`open: None`) as all zeros.
#[derive(Default)]
struct PaintSeen {
    open: Option<RenderId>,
    undo: usize,
    redo: usize,
    painted: bool,
    source: Option<deptheos_app::paint::SourceKind>,
    source_ready: bool,
    painted_from: Vec<deptheos_app::paint::SourceKind>,
}

fn paint_state(cx: &mut TestAppContext, app: &Entity<AppView>) -> PaintSeen {
    app.read_with(cx, |app, cx| {
        let Some(s) = app.paint.read(cx).state() else { return PaintSeen::default() };
        PaintSeen {
            open: Some(s.key.clone()),
            undo: s.undo,
            redo: s.redo,
            painted: s.painted,
            source: Some(s.selection.clone()),
            source_ready: s.ready,
            painted_from: s.painted_from.clone(),
        }
    })
}

/// A stroke across the result, as the viewer reports one.
fn stroke(cx: &mut TestAppContext, app: &Entity<AppView>, y: f32) {
    use crate::viewer::PaintInput;
    let dab = |x| deptheos_app::paint::Dab { x, y, pressure: 1.0 };
    app.update(cx, |app, cx| {
        app.paint.update(cx, |p, cx| {
            p.input(PaintInput::Begin(dab(20.0)), cx);
            p.input(PaintInput::Move(dab(90.0)), cx);
            p.input(PaintInput::Move(dab(170.0)), cx);
            p.input(PaintInput::End, cx);
        })
    });
}

/// `names` folders under `parent` of `frames` frames each, as a batch's
/// inputs, and their paths.
fn batch_folders(parent: &Path, names: &[&str], frames: u8) -> Vec<PathBuf> {
    names
        .iter()
        .map(|n| {
            let folder = parent.join(n);
            std::fs::create_dir_all(&folder).unwrap();
            write_frames(&folder, frames);
            folder
        })
        .collect()
}

/// One render per folder, frames in name order.
fn per_folder_plan(folders: &[PathBuf]) -> deptheos_app::runs::batch::Plan {
    use deptheos_app::runs::batch::{Input, Split, plan};
    let inputs: Vec<Input> = folders.iter().cloned().map(Input::Folder).collect();
    plan(
        &inputs,
        Split::PerFolder,
        folder::Source::Raw,
        deptheos_app::FrameOrder::Name,
        deptheos_app::RawBackend::LibRaw,
        |_| deptheos_app::frames::capture::CaptureInfo::default(),
        &deptheos_app::Words::english(deptheos_app::Platform::Desktop),
    )
}

fn batch_over(cx: &mut TestAppContext, app: &Entity<AppView>) -> bool {
    app.read_with(cx, |app, cx| {
        !app.batch_run().is_some() && !app.is_processing(cx) && !app.renders.read(cx).is_saving()
    })
}

/// The files in `dir`, sorted.
fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{dir:?}: {e}"))
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Where the view bar's parts are, drawn afresh.
fn bar_part(
    cx: &mut TestAppContext,
    window: gpui::AnyWindowHandle,
    selector: &'static str,
) -> Option<gpui::Bounds<Pixels>> {
    cx.update_window(window, |_, window, _| window.refresh()).unwrap();
    cx.run_until_parked();
    gpui::VisualTestContext::from_window(window, cx).debug_bounds(selector)
}

/// The middle of tick `i` of `count` on the rail at `rail`.
fn tick(rail: gpui::Bounds<Pixels>, i: usize, count: usize) -> gpui::Point<Pixels> {
    let column = rail.size.width / count as f32;
    gpui::point(rail.origin.x + column * (i as f32 + 0.5), rail.center().y)
}

/// Click `name` (its `debug_selector`) at its middle, a moment after the last
/// click as a user would, so a sliding notice comes and goes meanwhile (the
/// test clock moves only when told).
fn click(cx: &mut TestAppContext, window: gpui::AnyWindowHandle, name: &str) {
    cx.executor().advance_clock(std::time::Duration::from_secs(10));
    // GPUI looks a name up as 'static; a test's few are leaked.
    let name: &'static str = Box::leak(name.to_owned().into_boxed_str());
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    vcx.run_until_parked();
    let bounds = vcx.debug_bounds(name).unwrap_or_else(|| panic!("{name} isn't on screen"));
    vcx.simulate_click(bounds.center(), gpui::Modifiers::none());
    vcx.run_until_parked();
}
