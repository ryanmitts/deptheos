//! The start window: the app doesn't run without its Library. When it won't
//! open (another app has it, a newer version made it, its drive is offline,
//! or it is unreadable) this window says why and offers Try Again, Choose
//! Another Library… and Quit (Get the Latest Version first for a newer
//! Library). Once it opens, the main window takes over.

use std::path::{Path, PathBuf};

use deptheos_app::StoreError;
use deptheos_app::library::history::History;
use deptheos_app::library::stacks::Working;
use gpui::prelude::*;
use gpui::{App, Context, FontWeight, Task, Window, div, px};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, h_flex, v_flex};

use crate::notices::Details;
use crate::prefs::{prefs, update_prefs};
use crate::words::{Wording as _, say};

/// The opened Library for the main window: its History and the working
/// version left on screen.
pub(crate) struct Opened {
    pub history: History,
    pub working: Working,
}

/// Open the Library at `root` and read the working version left; an
/// unreadable `list.json` fails the open so nothing overwrites it. Blocking.
pub(crate) fn open(root: &Path) -> Result<Opened, StoreError> {
    let history = History::open(root)?;
    let working = history.read_working()?;
    Ok(Opened { history, working })
}

/// What opens the main window over the Library once it opens.
pub(crate) type Launch = Box<dyn FnOnce(Opened, &mut App)>;

/// Why the Library didn't open, as the window says it.
#[derive(Clone, Debug)]
pub(crate) struct Refusal {
    /// The Library's folder; none when there is no folder for it.
    folder: Option<PathBuf>,
    /// Why, in sentences.
    why: String,
    /// A newer version made it: Get the Latest Version comes first.
    newer: bool,
}

impl Refusal {
    /// The Library at `folder` didn't open, for `error`.
    pub(crate) fn of(folder: &Path, error: &StoreError, cx: &App) -> Self {
        tracing::error!("the Library at {} didn't open: {error}", folder.display());
        let explained = crate::notices::explain(error, crate::words::of(cx));
        let why = crate::notices::sentences(cx.wording(), None, &explained, Details::Diagnostics);
        let newer = matches!(error, StoreError::NewerLibrary { .. });
        Self { folder: Some(folder.to_path_buf()), why: why.text, newer }
    }

    /// No folder for the Library: the system names none and none is chosen.
    pub(crate) fn no_folder(cx: &App) -> Self {
        tracing::error!("there is no folder for the Library");
        Self { folder: None, why: say!(cx, "start-no-folder"), newer: false }
    }
}

/// The start window, while the Library won't open.
///
/// Lifecycle: `Refused`, `Choosing` another folder or `Trying` again, each
/// holding what was said; changed only by [`LibraryStart::set_start`].
/// `Refused` → `Trying` (Try Again, or a folder chosen) or `Choosing`.
/// `Choosing` → `Refused`: canceled, or a folder that can't be a Library.
/// `Trying` → `Refused` if it still won't open, else the window goes.
enum Start {
    Refused(Refusal),
    Choosing { refusal: Refusal, _pick: Task<()> },
    Trying { refusal: Refusal, _open: Task<()> },
}

impl Start {
    fn refusal(&self) -> &Refusal {
        match self {
            Start::Refused(refusal)
            | Start::Choosing { refusal, .. }
            | Start::Trying { refusal, .. } => refusal,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Start::Refused(_) => "refused",
            Start::Choosing { .. } => "choosing",
            Start::Trying { .. } => "trying",
        }
    }
}

pub(crate) struct LibraryStart {
    start: Start,
    /// Taken once the Library opens.
    launch: Option<Launch>,
}

impl LibraryStart {
    pub(crate) fn new(refusal: Refusal, launch: Launch) -> Self {
        Self { start: Start::Refused(refusal), launch: Some(launch) }
    }

    fn set_start(&mut self, start: Start, cx: &mut Context<Self>) {
        tracing::debug!("library start: {} -> {}", self.start.name(), start.name());
        self.start = start;
        cx.notify();
    }

    /// Open the Library Settings names again, off this thread.
    fn try_again(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let refusal = self.start.refusal().clone();
        let Some(root) = prefs(cx).history_root() else {
            self.set_start(Start::Refused(Refusal::no_folder(cx)), cx);
            return;
        };
        let open = cx.spawn_in(window, async move |this, cx| {
            let tried = root.clone();
            let opened = crate::work::interactive("library-open", move || open(&tried)).await;
            this.update_in(cx, |this, window, cx| this.tried(&root, opened, window, cx)).ok();
        });
        self.set_start(Start::Trying { refusal, _open: open }, cx);
    }

    /// The Library at `root` opened (the main window takes it) or didn't
    /// (why is said).
    fn tried(
        &mut self,
        root: &Path,
        opened: Result<Result<Opened, StoreError>, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(self.start, Start::Trying { .. }) {
            return;
        }
        let refusal = match opened {
            Ok(Ok(opened)) => {
                tracing::info!("the Library at {} opened", root.display());
                // The main window first: the app quits when its last window goes.
                if let Some(launch) = self.launch.take() {
                    launch(opened, cx);
                }
                window.remove_window();
                return;
            }
            Ok(Err(e)) => Refusal::of(root, &e, cx),
            Err(fault) => {
                tracing::error!("opening the Library failed: {fault}");
                let why = crate::notices::fault_sentence(cx.wording()).text;
                Refusal { folder: Some(root.to_path_buf()), why, newer: false }
            }
        };
        self.set_start(Start::Refused(refusal), cx);
    }

    /// Ask for another Library folder; a synced one is refused, others set
    /// in Settings and opened.
    fn choose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let refusal = self.start.refusal().clone();
        let start = refusal.folder.as_deref().and_then(Path::parent).map(Path::to_path_buf);
        let dialog = crate::file_dialog::file_dialog(window, start.as_deref());
        let pick = cx.spawn_in(window, async move |this, cx| {
            let picked = crate::file_dialog::pick_folder(dialog).await;
            this.update_in(cx, |this, window, cx| this.chosen(picked, window, cx)).ok();
        });
        self.set_start(Start::Choosing { refusal, _pick: pick }, cx);
    }

    fn chosen(&mut self, picked: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let Start::Choosing { refusal, .. } = &self.start else { return };
        let refusal = refusal.clone();
        let Some(folder) = picked else {
            self.set_start(Start::Refused(refusal), cx);
            return;
        };
        if let Some(why) = crate::prefs_window::refuse_library(&folder, cx) {
            self.set_start(Start::Refused(Refusal { folder: Some(folder), why, newer: false }), cx);
            return;
        }
        tracing::info!("the Library folder chosen: {}", folder.display());
        update_prefs(cx, |p| p.history_dir = Some(folder));
        self.set_start(Start::Refused(refusal), cx);
        self.try_again(window, cx);
    }
}

impl Render for LibraryStart {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let refusal = self.start.refusal();
        let busy = !matches!(self.start, Start::Refused(_));
        let trying = matches!(self.start, Start::Trying { .. });
        let newer = refusal.newer;
        let try_label = if trying { say!(cx, "start-trying") } else { say!(cx, "start-try-again") };
        let try_again = Button::new("start-try-again")
            .label(try_label)
            .disabled(busy)
            .when(!newer, |b| b.primary())
            .when(newer, |b| b.outline())
            .on_click(cx.listener(|this, _, window, cx| this.try_again(window, cx)));
        let choose = Button::new("start-choose")
            .label(say!(cx, "start-choose"))
            .outline()
            .disabled(busy)
            .on_click(cx.listener(|this, _, window, cx| this.choose(window, cx)));
        let quit = Button::new("start-quit")
            .label(say!(cx, "start-quit"))
            .ghost()
            .on_click(|_, _, cx| crate::app::quit::quit_now(cx));
        let latest = newer.then(|| {
            Button::new("start-get-latest").label(say!(cx, "start-get-latest")).primary().on_click(
                |_, _, cx| cx.open_url(&crate::updates::site_page(crate::updates::DOWNLOAD_PAGE)),
            )
        });
        v_flex()
            .id("library-start")
            .debug_selector(|| "library-start".into())
            .size_full()
            .items_center()
            .justify_center()
            .gap_4()
            .p_8()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                v_flex()
                    .max_w(px(560.0))
                    .gap_3()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(say!(cx, "start-title")),
                    )
                    .when_some(refusal.folder.as_ref(), |el, folder| {
                        let folder = folder.display().to_string();
                        el.child(div().text_sm().text_color(muted).whitespace_normal().child(say!(
                            cx,
                            "start-folder",
                            folder = folder
                        )))
                    })
                    .child(div().whitespace_normal().child(refusal.why.clone()))
                    .child(
                        h_flex()
                            .pt_2()
                            .gap_2()
                            .flex_wrap()
                            .children(latest)
                            .child(try_again)
                            .child(choose)
                            .child(div().flex_1())
                            .child(quit),
                    ),
            )
    }
}

#[cfg(test)]
#[path = "start_tests.rs"]
mod tests;
