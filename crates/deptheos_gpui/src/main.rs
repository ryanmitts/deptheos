//! Deptheos GUI: a GPUI + gpui-kit shell over the stacking engine.
//!
//! The engine's wgpu device is headless; GPUI renders with its own device
//! and results reach the window as CPU pixels (`deptheos_app::Engine`).

// Release builds on Windows open no console window beside the app.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod about;
mod adjust;
mod app;
mod batch_dialog;
mod bench_dialog;
mod bookmarks;
mod brand;
mod choice;
mod color;
mod dates;
mod export_dialog;
mod file_dialog;
mod files;
mod folders;
mod form;
mod grow_dialog;
mod images;
mod instance;
mod look;
mod multi_select;
mod notices;
mod paint;
mod params;
mod prefs;
mod prefs_window;
mod renders;
mod sequences;
mod stacking;
mod start;
#[cfg(test)]
mod test_support;
mod thumbs;
mod r#type;
mod ui_state;
mod updates;
mod viewer;
mod words;
mod work;

use std::borrow::Cow;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use deptheos_app::engine::logs;
use deptheos_app::frames::folder::is_image;
use deptheos_app::{DeviceChoice, DeviceInfo, Engine, GpuContext};
use futures::StreamExt as _;
use gpui::prelude::*;
use gpui::{
    App, AssetSource, Bounds, Context, Pixels, SharedString, Size, TitlebarOptions, Window,
    WindowBounds, WindowOptions, div, point, px, size,
};
use gpui_kit::component::button::Button;
use gpui_kit::component::{ActiveTheme as _, Theme, ThemeSet, v_flex};

use app::AppView;
use words::{DesktopWords, say};

/// Hands Quit to the main window: true to quit now, false when it asked (or
/// waits on writes) and quits itself later.
type AskQuit = Box<dyn Fn(&mut App) -> bool>;
use app::actions::{
    BringAllToFront, ChoosingAMethod, HideApp, HideOthers, MenuState, MinimizeWindow, OpenHelp,
    OpenPreferences, Quit, ReleaseNotes, ReportProblem, ShowAll, ShowLogFolder, ToggleFullScreen,
    ZoomWindow, key_bindings, menus,
};

// Faster than the system allocator on Windows; the workspace manifest has
// the measurements.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// Have mimalloc return its free memory to the system. It keeps a dropped
/// picture's pages: a 45 MP display pyramid left the footprint 360 MB up, a
/// 100 MP one 675 MB, until this brought it within 100 MB
/// (`examples/mimalloc_shed.rs` in deptheos_app). It costs 12 to 16 ms on
/// the main thread, so the viewer calls it only after a drop has settled.
pub(crate) fn give_back_memory() {
    unsafe extern "C" {
        fn mi_collect(force: bool);
    }
    let before = deptheos_app::engine::memory::footprint();
    // SAFETY: mimalloc's own entry point, safe from any thread.
    unsafe { mi_collect(true) };
    if let (Some(before), Some(after)) = (before, deptheos_app::engine::memory::footprint()) {
        tracing::debug!("memory given back: footprint {} MB -> {} MB", before >> 20, after >> 20);
    }
}

/// The Deptheos light and dark themes, generated from `design/tokens.json`
/// by `script/tokens.mjs`.
const BRAND_THEME: &str = include_str!("../themes/deptheos.json");
/// The application id: the Wayland app_id and X11 WM_CLASS matching
/// packaging/linux's com.deptheos.Deptheos.desktop, and the Mac's bundle id.
pub(crate) const APP_ID: &str = "com.deptheos.Deptheos";

/// Keep the window's theme in step with the system's appearance, unless
/// Settings holds it light or dark ([`look::apply_appearance`]).
pub(crate) fn follow_system_appearance(window: &Window) {
    window
        .observe_window_appearance(|window, cx| {
            tracing::debug!("the system appearance is now {:?}", window.appearance());
            look::apply_appearance(Some(window), false, cx);
        })
        .detach();
}

fn brand_theme() -> ThemeSet {
    serde_json::from_str(BRAND_THEME).expect("themes/deptheos.json is valid")
}

/// Makes the brand themes the ones light and dark mode load.
fn install_brand_theme(cx: &mut App) {
    let theme = Theme::global_mut(cx);
    for config in brand_theme().themes {
        if config.mode.is_dark() {
            theme.dark_theme = config.into();
        } else {
            theme.light_theme = config.into();
        }
    }
}

fn main() {
    // Only images and folders: macOS and Xcode inject flags like `-psn_…`.
    let args: Vec<PathBuf> =
        std::env::args().skip(1).map(PathBuf::from).filter(|p| is_image(p) || p.is_dir()).collect();
    // One app per user: a later launch hands its files to the running one
    // and ends here, before touching anything of the app's (macOS does this
    // itself, `on_open_urls`). A launch that can't tell (`Alone`) runs, and
    // the Library's lock refuses it if another app has the Library.
    #[cfg(not(target_os = "macos"))]
    let claim = instance::default_dir().map(|dir| instance::claim(&dir, &args));
    #[cfg(not(target_os = "macos"))]
    if matches!(claim, Some(instance::Claim::HandedOff)) {
        return;
    }
    // Only now, so a launch that hands off makes and prunes no log file:
    // stderr and this launch's file (Help ▸ Show Log Folder), with the DNG
    // bridge's diagnostics.
    let this_app = logs::App {
        name: "Deptheos (desktop)",
        version: env!("CARGO_PKG_VERSION"),
        build: env!("DEPTHEOS_BUILD"),
    };
    logs::install(&this_app, logs::default_dir().as_deref());
    // Windows: a native crash leaves a minidump for Save Diagnostics….
    #[cfg(windows)]
    logs::dumps::install();
    #[cfg(not(target_os = "macos"))]
    let first = match claim {
        Some(instance::Claim::First(first)) => {
            tracing::debug!("instance: first");
            Some(first)
        }
        Some(instance::Claim::Alone(why)) => {
            tracing::warn!("instance: alone: {why}");
            None
        }
        Some(instance::Claim::HandedOff) | None => None,
    };
    // Files handed over while running: by the system (macOS Open With, Dock
    // drops) or a later launch (Windows, Linux; possibly none, just raising
    // the window). They may come before the window opens, so they wait in a
    // channel.
    let (opened_tx, opened_rx) = futures::channel::mpsc::unbounded::<Vec<PathBuf>>();
    // Answered before the engine starts (which can take seconds), since a
    // later launch waits only a moment before starting on its own.
    #[cfg(not(target_os = "macos"))]
    if let Some(first) = first {
        let opened_tx = opened_tx.clone();
        first.listen(move |paths| opened_tx.unbounded_send(paths).is_ok());
    }
    deptheos_app::DngCache::sweep_default_dir();

    // Read before the engine, which runs on the GPU they choose.
    let (preferences, preferences_path, preferences_reset) = prefs::Prefs::load_default();
    // Both crates' sentences in the system's language, before any window.
    let words = DesktopWords::system();
    // A chosen GPU that fails falls back to Automatic; no GPU at all is a
    // message in a window, not a panic. The failure is worded for the user
    // (details to the log); the caller adds the advice.
    let start = |choice: &DeviceChoice| {
        Engine::with_device(choice, words.shared().clone()).map_err(|e| {
            let explained = notices::explain(&e, words.shared());
            match explained.kind {
                deptheos_app::ExplanationKind::Fault => {
                    say!(words, "gpu-fault-reason", reason = explained.reason)
                }
                _ => explained.reason,
            }
        })
    };
    let (engine, gpu_fallback) = start_engine(&preferences.gpu, start);
    let engine =
        engine.map_err(|why| SharedString::from(say!(words, "gpu-start-failed", why = why)));
    let gpu_failed = gpu_fallback.clone().map(|reason| (preferences.gpu.clone(), reason));
    let gpus_found = draw_beside_venus(engine.as_ref().ok().map(|e| e.gpu().device_info()));

    let application = gpui_kit::application().with_assets(AppAssets);
    application.on_open_urls(move |urls| {
        let paths: Vec<PathBuf> = urls
            .iter()
            .filter_map(|url| file_url_path(url))
            .filter(|p| is_image(p) || p.is_dir())
            .collect();
        if !paths.is_empty() {
            opened_tx.unbounded_send(paths).ok();
        }
    });
    application.run(move |cx: &mut App| {
        // Before any window or notification: Windows names notifications by it
        // (its AppUserModelID).
        cx.set_app_identity(APP_ID, "Deptheos");
        app::away::answer_clicks(cx);
        gpui_kit::init(cx);
        words::install(words.clone(), cx);
        install_brand_theme(cx);
        app::place_notices(cx);
        prefs::Prefs::install(preferences.clone(), preferences_path.clone(), cx);
        // Light or dark as chosen, before any window paints.
        look::apply_appearance(None, true, cx);
        look::follow_appearance_choice(cx);
        prefs_window::find_gpus(
            gpus_found.clone(),
            engine.as_ref().ok().map(|e| e.gpu().device_info()),
            gpu_failed.clone(),
            cx,
        );
        ui_state::init(cx);

        cx.bind_keys(key_bindings());
        // Quit arrives here when another window (Preferences) is in front;
        // the main window is still asked first.
        let ask_main: Rc<RefCell<Option<AskQuit>>> = Rc::default();
        let ask = ask_main.clone();
        cx.on_action(move |_: &Quit, cx| {
            if may_quit(&ask, cx) {
                app::quit::quit_now(cx);
            }
        });
        // The Dock's Quit, logging out and shutting down ask it too.
        #[cfg(target_os = "macos")]
        {
            let (ask, app) = (ask_main.clone(), cx.to_async());
            app::quit::guard_system_quit(move || app.update(|cx| may_quit(&ask, cx)));
        }
        cx.on_action(|_: &OpenPreferences, cx| prefs_window::open_preferences(cx));
        // The engine is never dropped: its converted frames are deleted here
        // (else swept next start), and its GPU device left to the OS
        // (`Engine::keep_until_exit`).
        let quitting_engine = engine.as_ref().ok().cloned();
        app::quit::on_quit(cx, move || {
            if let Some(engine) = quitting_engine {
                engine.keep_until_exit();
            }
            // The store thread's last writes, the log, and no static
            // destructor under a running codec (ADR-143).
            deptheos_app::engine::work::exit();
        });
        // A no-op off macOS, where the toolbar's menu has About and Preferences.
        cx.set_menus(menus(&MenuState::default(), &words));
        app_actions(cx);
        // The main window gone, Preferences alone does not keep the app.
        cx.on_window_closed(|cx, _| {
            if cx.windows().into_iter().all(|w| prefs_window::is_preferences_window(w, cx)) {
                app::quit::quit_now(cx);
            }
        })
        .detach();

        // Where the window was last, while that is still on a display.
        let displays: Vec<Bounds<Pixels>> = cx.displays().iter().map(|d| d.bounds()).collect();
        let window_bounds = ui_state::ui_state(cx)
            .window
            .and_then(|saved| saved.restore(&displays))
            .unwrap_or_else(|| {
                let display = cx.primary_display().map(|d| d.visible_bounds().size);
                WindowBounds::Windowed(Bounds::centered(None, first_window_size(display), cx))
            });
        let options = WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some(say!(words, "window-title").into()),
                ..Default::default()
            }),
            window_bounds: Some(window_bounds),
            window_min_size: Some(MIN_WINDOW_SIZE),
            app_id: Some(APP_ID.into()),
            ..Default::default()
        };
        match &engine {
            Ok(engine) => {
                let notices = StartNotices {
                    gpu_fallback: gpu_fallback
                        .clone()
                        .map(|reason| (reason, preferences.gpu.clone())),
                    prefs_reset: preferences_reset.clone(),
                };
                let (engine, args, ask_main) = (engine.clone(), args.clone(), ask_main.clone());
                let launch: start::Launch = Box::new(move |opened, cx| {
                    let window = MainWindow { options, engine, args, notices, opened_rx };
                    open_main_window(cx, window, opened, &ask_main)
                });
                open_library(cx, launch);
            }
            Err(message) => open_startup_error(cx, options, message, preferences_path.clone()),
        }
        cx.activate(true);
    });
}

/// The smallest the main window and the startup error's go.
const MIN_WINDOW_SIZE: Size<Pixels> = size(px(900.0), px(600.0));

/// The first-launch window size on `display`: wide enough for the wide
/// layout ([`app::chrome::NARROW_WIDTH`]), at most 90% of the display, never
/// below [`MIN_WINDOW_SIZE`].
fn first_window_size(display: Option<Size<Pixels>>) -> Size<Pixels> {
    let wanted = size(px(1440.0), px(900.0));
    let Some(display) = display else { return wanted };
    size(
        wanted.width.min(display.width * 0.9).max(MIN_WINDOW_SIZE.width),
        wanted.height.min(display.height * 0.9).max(MIN_WINDOW_SIZE.height),
    )
}

/// What the main window says as it opens.
struct StartNotices {
    /// The chosen GPU could not start: the reason, and the GPU chosen.
    gpu_fallback: Option<(String, DeviceChoice)>,
    /// The settings file couldn't be used.
    prefs_reset: Option<prefs::Reset>,
}

/// Everything the main window opens with but its Library.
struct MainWindow {
    options: WindowOptions,
    engine: Arc<Engine>,
    /// Added to its list.
    args: Vec<PathBuf>,
    notices: StartNotices,
    /// The files the system hands over.
    opened_rx: futures::channel::mpsc::UnboundedReceiver<Vec<PathBuf>>,
}

/// Open the Library Settings names, then `launch` the main window over it;
/// if it won't open, the start window says why and launches once it does
/// (`start.rs`).
fn open_library(cx: &mut App, launch: start::Launch) {
    let root = prefs::prefs(cx).history_root();
    let refusal = match root.as_deref().map(|root| (start::open(root), root)) {
        Some((Ok(opened), _)) => return launch(opened, cx),
        Some((Err(e), root)) => start::Refusal::of(root, &e, cx),
        None => start::Refusal::no_folder(cx),
    };
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some(say!(cx, "window-title").into()),
            ..Default::default()
        }),
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, START_SIZE, cx))),
        window_min_size: Some(START_SIZE),
        app_id: Some(APP_ID.into()),
        ..Default::default()
    };
    gpui_kit::open_window(options, cx, |window, cx| {
        color::tag_window(window, deptheos_app::engine::present::color_managed());
        follow_system_appearance(window);
        cx.new(|_| start::LibraryStart::new(refusal, launch))
    })
    .unwrap_or_else(|e| window_failed(e, cx));
}

/// The start window's size.
const START_SIZE: Size<Pixels> = size(px(640.0), px(360.0));

/// How long after the main window opens a deliberate crash comes, so the
/// window draws and the engine notes its GPU first.
const CRASH_AFTER: std::time::Duration = std::time::Duration::from_secs(2);

/// With `DEPTHEOS_CRASH` naming a kind (`docs/crash-testing.md`), crash so
/// soon after the main window opens, from the UI run loop as a real crash
/// would.
fn crash_on_purpose(cx: &mut App) {
    let Some(kind) = logs::on_purpose::Kind::from_environment() else { return };
    tracing::warn!(
        "{} is set: crashing ({}) in {CRASH_AFTER:?}",
        logs::on_purpose::VARIABLE,
        kind.name()
    );
    cx.spawn(async move |cx| {
        cx.background_executor().timer(CRASH_AFTER).await;
        logs::on_purpose::crash(kind);
    })
    .detach();
}

/// The main window over `main.engine` and the Library `opened`, with
/// `main.args` added, its notices, the quit question in `ask_main` and the
/// files the system hands over.
fn open_main_window(
    cx: &mut App,
    main: MainWindow,
    opened: start::Opened,
    ask_main: &RefCell<Option<AskQuit>>,
) {
    let MainWindow { options, engine, args, notices, mut opened_rx } = main;
    // The app view draws the title bar, with the toolbar in it.
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some(say!(cx, "window-title").into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(14.0), px(15.0))),
        }),
        app_owns_titlebar_drag: true,
        ..options
    };
    let (handle, view) = gpui_kit::open_window(options, cx, |window, cx| {
        color::tag_window(window, deptheos_app::engine::present::color_managed());
        follow_system_appearance(window);
        cx.new(|cx| AppView::new(engine.clone(), args, opened, window, cx))
    })
    .unwrap_or_else(|e| window_failed(e, cx));
    app::quit::set_main_view(&view, cx);
    // Settings ▸ Library asks it about the Library, and deletes it there.
    app::library_delete::register(handle, &view, cx);
    if let Some((reason, choice)) = notices.gpu_fallback {
        let gpu = choice_name(&choice, cx);
        let message = say!(cx, "gpu-fallback-notice", gpu = gpu, reason = reason);
        let settings = notices::Action::new(say!(cx, "open-settings"), |_, cx| {
            prefs_window::open_preferences(cx)
        });
        handle
            .update(cx, |_, window, cx| {
                let (kind, subject) = (notices::Kind::Warning, notices::Subject::Alone);
                notices::post(kind, subject, message, vec![settings], window, cx)
            })
            .ok();
    }
    if let Some(reset) = notices.prefs_reset {
        let message = match &reset.kept {
            Some(kept) => {
                say!(cx, "settings-reset-notice-kept", path = kept.display().to_string())
            }
            None => say!(cx, "settings-reset-notice"),
        };
        handle
            .update(cx, |_, window, cx| {
                let (kind, subject) = (notices::Kind::Warning, notices::Subject::Alone);
                notices::post(kind, subject, message, Vec::new(), window, cx)
            })
            .ok();
    }
    crash_on_purpose(cx);
    // Here, not in AppView::new, so tests never reach GitHub.
    if prefs::prefs(cx).check_for_updates {
        handle
            .update(cx, |_, window, cx| {
                view.update(cx, |view, cx| view.check_for_updates(false, window, cx))
            })
            .ok();
    }
    let view = view.downgrade();
    let asking = view.clone();
    // False once the main window takes the question (it quits itself on
    // yes); true with nothing to ask or no window to ask in.
    *ask_main.borrow_mut() = Some(Box::new(move |cx: &mut App| {
        let quit = handle.update(cx, |_, window, cx| {
            asking.update(cx, |view, cx| view.confirm_quit(window, cx))
        });
        !matches!(quit, Ok(Ok(false)))
    }));
    cx.spawn(async move |cx| {
        while let Some(paths) = opened_rx.next().await {
            // Handed over: the window comes to the front and they are added.
            let loaded = handle.update(cx, |_, window, cx| {
                cx.activate(true);
                window.activate_window();
                view.update(cx, |view, cx| {
                    view.add_paths_from(paths, app::AddedFrom::Outside, window, cx)
                })
            });
            if !matches!(loaded, Ok(Ok(()))) {
                return;
            }
        }
    })
    .detach();
}

/// Quit now: the main window's answer, or yes without one.
fn may_quit(ask: &RefCell<Option<AskQuit>>, cx: &mut App) -> bool {
    ask.borrow().as_ref().is_none_or(|ask| ask(cx))
}

/// The window that says why the engine could not start.
fn open_startup_error(
    cx: &mut App,
    options: WindowOptions,
    message: &SharedString,
    prefs_path: Option<PathBuf>,
) {
    gpui_kit::open_window(options, cx, |window, cx| {
        color::tag_window(window, deptheos_app::engine::present::color_managed());
        follow_system_appearance(window);
        cx.new(|_| StartupError { message: message.clone(), prefs_path })
    })
    .unwrap_or_else(|e| window_failed(e, cx));
}

/// The main window would not open: say so in a system dialog and exit (a
/// Windows release build has no console for a panic's message).
fn window_failed(error: impl std::fmt::Display, cx: &App) -> ! {
    tracing::error!("failed to open the main window: {error}");
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title(say!(cx, "window-title"))
        .set_description(say!(cx, "window-failed", error = error.to_string()))
        .set_buttons(rfd::MessageButtons::Ok)
        .show();
    std::process::exit(1)
}

/// The path a `file://` URL names, percent-decoded; `None` otherwise.
fn file_url_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    // `file://localhost/x` and `file:///x` are the same file.
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let mut bytes = Vec::with_capacity(rest.len());
    let mut iter = rest.bytes();
    while let Some(b) = iter.next() {
        if b == b'%' {
            let hex = [iter.next()?, iter.next()?];
            let hex = std::str::from_utf8(&hex).ok()?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
        } else {
            bytes.push(b);
        }
    }
    let path = String::from_utf8(bytes).ok()?;
    // `/C:/x` on Windows.
    let path = match path.strip_prefix('/') {
        Some(rest) if cfg!(windows) && rest.as_bytes().get(1) == Some(&b':') => rest.to_string(),
        _ => path,
    };
    Some(PathBuf::from(path))
}

// Icons the app uses beyond gpui-kit's default component set.
gpui_kit::assets::icon_assets!(
    ExtraIcons,
    [
        ArrowLeftRight,
        Blend,
        Brush,
        Camera,
        Columns2,
        Ellipsis,
        Eraser,
        Focus,
        Highlighter,
        Image,
        ImagePlus,
        Images,
        LayoutGrid,
        Layers,
        Layers2,
        Map,
        Menu,
        Mountain,
        Pipette,
        RotateCcw,
        Rows2,
        Scan,
        Settings,
        Share,
        SlidersHorizontal,
        Square,
        SquareSplitHorizontal,
        Trash,
        ZoomIn,
        ZoomOut,
    ]
);

/// The app's own glyphs (`design/icons/`, on Lucide's grid), served beside
/// [`ExtraIcons`]: `Icon::new(AppIcon::Render)`.
#[derive(Clone, Copy, Debug)]
pub(crate) enum AppIcon {
    /// A render: Stack's `Layers` glyph with its top layer filled. Beside a
    /// render's number on tiles, chips and menus; never on a sentence that
    /// names the render.
    Render,
}

impl AppIcon {
    const ALL: [AppIcon; 1] = [AppIcon::Render];

    fn path_str(self) -> &'static str {
        match self {
            AppIcon::Render => "icons/deptheos-render.svg",
        }
    }

    fn svg(self) -> &'static [u8] {
        match self {
            AppIcon::Render => include_bytes!("../../../design/icons/render.svg"),
        }
    }
}

impl gpui_kit::assets::IconNamed for AppIcon {
    fn path(self) -> SharedString {
        self.path_str().into()
    }
}

/// gpui-kit's default icons plus [`ExtraIcons`] and [`AppIcon`]s.
struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        if let Some(icon) = AppIcon::ALL.into_iter().find(|i| i.path_str() == path) {
            return Ok(Some(Cow::Borrowed(icon.svg())));
        }
        match ExtraIcons.load(path)? {
            Some(bytes) => Ok(Some(bytes)),
            None => gpui_kit::assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        paths.extend(
            AppIcon::ALL
                .iter()
                .map(|i| i.path_str())
                .filter(|p| p.starts_with(path))
                .map(Into::into),
        );
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

/// Start on `choice`, or on Automatic if it can't, returning why it failed.
/// The choice is kept: an unplugged GPU may return.
fn start_engine<T, E: std::fmt::Display>(
    choice: &DeviceChoice,
    start: impl Fn(&DeviceChoice) -> Result<T, E>,
) -> (Result<T, E>, Option<String>) {
    match start(choice) {
        Err(e) if *choice != DeviceChoice::Automatic => {
            tracing::warn!("the GPU chosen in Settings could not start ({e}); using Automatic");
            (start(&DeviceChoice::Automatic), Some(e.to_string()))
        }
        started => (started, None),
    }
}

/// Under virtio-gpu VMs (UTM), Mesa's Venus driver computes but can't draw
/// a window (Wayland frames never show, X11 aborts). GPUI picks its adapter
/// at the first window, so with the engine on Venus, Venus is hidden from
/// the Vulkan loader first and GPUI draws on lavapipe while the engine keeps
/// the GPU. Returns the GPU list for Preferences made while Venus was still
/// listed, `None` if nothing changed. A user's own `VK_LOADER_DRIVERS_SELECT`
/// is left alone.
fn draw_beside_venus(engine_gpu: Option<DeviceInfo>) -> Option<Vec<DeviceInfo>> {
    const LAVAPIPE_ICDS: [&str; 2] =
        ["/usr/share/vulkan/icd.d/lvp_icd.json", "/etc/vulkan/icd.d/lvp_icd.json"];
    let gpu = engine_gpu?;
    if !cfg!(target_os = "linux")
        || !gpu.name.starts_with("Virtio-GPU Venus")
        || std::env::var_os("VK_LOADER_DRIVERS_SELECT").is_some()
    {
        return None;
    }
    if !LAVAPIPE_ICDS.iter().any(|p| std::path::Path::new(p).is_file()) {
        tracing::warn!(
            "{} cannot draw windows and lavapipe is not installed \
             (mesa-vulkan-drivers): the window may not show",
            gpu.name
        );
        return None;
    }
    let found = GpuContext::devices();
    tracing::info!("{} cannot draw windows: drawing them on the CPU renderer", gpu.name);
    // SAFETY: set before GPUI starts its threads, and the engine's are idle
    // until given work; nothing reads the environment meanwhile.
    unsafe { std::env::set_var("VK_LOADER_DRIVERS_SELECT", "*lvp*") };
    Some(found)
}

/// `choice` as a sentence's subject.
fn choice_name(choice: &DeviceChoice, cx: &App) -> String {
    match choice {
        DeviceChoice::Automatic => say!(cx, "gpu-subject-automatic"),
        DeviceChoice::Software => say!(cx, "gpu-subject-software"),
        DeviceChoice::Named(name) => say!(cx, "gpu-subject-named", name = name.as_str()),
    }
}

/// The window shown when the engine can't start, with its own Settings
/// button (no menu bar off macOS).
struct StartupError {
    message: SharedString,
    prefs_path: Option<PathBuf>,
}

impl Render for StartupError {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_4()
            .p_8()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(div().max_w(px(560.0)).whitespace_normal().child(self.message.clone()))
            .when_some(self.prefs_path.as_ref(), |el, path| {
                el.child(
                    div()
                        .max_w(px(560.0))
                        .whitespace_normal()
                        .text_sm()
                        .text_color(muted)
                        .child(say!(cx, "settings-saved-in", path = path.display().to_string())),
                )
            })
            .child(
                Button::new("open-preferences")
                    .label(say!(cx, "open-settings"))
                    .on_click(|_, _, cx| prefs_window::open_preferences(cx)),
            )
    }
}

/// The website's pages the Help menu opens ([`updates::site_page`]).
const HELP_PAGE: &str = "/docs/";
const METHODS_PAGE: &str = "/docs/methods/";
/// The repository's pages it opens ([`updates::repo_page`]).
const ISSUES_PAGE: &str = "/issues/new";
const RELEASES_PAGE: &str = "/releases";

/// Actions any window can take: hiding the app, the window keys, and Help.
fn app_actions(cx: &mut gpui::App) {
    cx.on_action(|_: &HideApp, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &BringAllToFront, cx| cx.activate(true));
    let in_front = |cx: &mut gpui::App, f: fn(&mut gpui::Window)| {
        if let Some(window) = cx.active_window() {
            window.update(cx, |_, window, _| f(window)).ok();
        }
    };
    cx.on_action(move |_: &MinimizeWindow, cx| in_front(cx, |w| w.minimize_window()));
    cx.on_action(move |_: &ZoomWindow, cx| in_front(cx, |w| w.zoom_window()));
    cx.on_action(move |_: &ToggleFullScreen, cx| in_front(cx, |w| w.toggle_fullscreen()));
    cx.on_action(|_: &OpenHelp, cx| cx.open_url(&updates::site_page(HELP_PAGE)));
    cx.on_action(|_: &ReportProblem, cx| cx.open_url(&updates::repo_page(ISSUES_PAGE)));
    cx.on_action(|_: &ReleaseNotes, cx| cx.open_url(&updates::repo_page(RELEASES_PAGE)));
    cx.on_action(|_: &ShowLogFolder, cx| app::diagnostics::show_log_folder(cx));
    // Without a main window, the website's page on the methods.
    cx.on_action(|_: &ChoosingAMethod, cx| cx.open_url(&updates::site_page(METHODS_PAGE)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// A first launch opens wide on a laptop display or larger, and fits a
    /// small one.
    #[test]
    fn the_first_window_is_wide_where_it_fits() {
        let wide = |w: f32, h: f32| first_window_size(Some(size(px(w), px(h))));
        assert_eq!(first_window_size(None), size(px(1440.0), px(900.0)));
        assert_eq!(wide(2560.0, 1415.0), size(px(1440.0), px(900.0)));
        let laptop = wide(1470.0, 919.0);
        assert!(laptop.width > px(app::chrome::NARROW_WIDTH), "{laptop:?}");
        assert!(laptop.height < px(830.0) && laptop.height > px(820.0), "{laptop:?}");
        assert_eq!(wide(960.0, 640.0), MIN_WINDOW_SIZE);
    }

    /// gpui-kit ignores unknown keys, so a misspelled theme key would fall
    /// back to stock colors silently.
    #[test]
    fn brand_theme_keys_are_all_known() {
        let set = brand_theme();
        assert_eq!(set.themes.len(), 2);
        assert!(set.themes.iter().any(|t| t.mode.is_dark()));
        assert!(set.themes.iter().any(|t| !t.mode.is_dark()));
        let raw: serde_json::Value = serde_json::from_str(BRAND_THEME).unwrap();
        for (config, raw) in set.themes.iter().zip(raw["themes"].as_array().unwrap()) {
            let parsed = serde_json::to_value(&config.colors).unwrap();
            for (key, value) in raw["colors"].as_object().unwrap() {
                assert_eq!(&parsed[key], value, "{}: unknown key {key}", config.name);
            }
        }
    }

    /// A chosen GPU that fails falls back to Automatic once, saying why;
    /// Automatic failing is the startup error.
    #[test]
    fn a_failing_gpu_choice_falls_back_to_automatic() {
        let named = DeviceChoice::Named("eGPU".into());
        let only_automatic = |c: &DeviceChoice| match c {
            DeviceChoice::Automatic => Ok("automatic"),
            _ => Err("adapter not found"),
        };
        let (engine, reason) = start_engine(&named, only_automatic);
        assert_eq!(engine, Ok("automatic"));
        assert_eq!(reason.as_deref(), Some("adapter not found"));

        let (engine, reason) = start_engine(&DeviceChoice::Software, |c| Ok::<_, &str>(c.clone()));
        assert_eq!(engine, Ok(DeviceChoice::Software), "a choice that starts is used");
        assert_eq!(reason, None);

        let tries = std::cell::Cell::new(0);
        let nothing = |_: &DeviceChoice| {
            tries.set(tries.get() + 1);
            Err::<(), _>("no GPU")
        };
        assert_eq!(start_engine(&DeviceChoice::Automatic, nothing), (Err("no GPU"), None));
        assert_eq!(tries.get(), 1, "Automatic is not retried");
        let (engine, reason) = start_engine(&named, nothing);
        assert_eq!(engine, Err("no GPU"));
        assert_eq!(reason.as_deref(), Some("no GPU"));
    }

    #[test]
    fn file_urls_decode_to_paths() {
        let url = "file:///Users/me/My%20Stack/IMG_0001.CR3";
        assert_eq!(file_url_path(url), Some(PathBuf::from("/Users/me/My Stack/IMG_0001.CR3")));
        assert_eq!(file_url_path("file://localhost/a/b%2Cc.tif"), Some("/a/b,c.tif".into()));
        assert_eq!(file_url_path("https://example.com/a.jpg"), None);
        assert_eq!(file_url_path("file:///a%2"), None, "a cut-off escape");
        assert!(is_image(Path::new("/a/IMG_0001.Cr3")));
        assert!(!is_image(Path::new("-psn_0_12345")));
    }
}
