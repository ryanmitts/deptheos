//! A stack's run: starting it with the live settings, what a run locks,
//! its number, its progress card and pill, and a failed stack's notice.

use std::path::PathBuf;

use crate::renders::Hold;
use deptheos_app::Explanation;
use deptheos_app::LIVE_PREVIEW_SIDE;
use deptheos_app::library::history::{Batch, RenderId, StackId};
use deptheos_app::runs::batch;
use gpui::prelude::*;
use gpui::{AnyElement, App, ClipboardItem, Context, Entity, FontWeight, Window, div, px};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, h_flex, v_flex};

use super::{AppView, BatchRun, PendingRender, Running, Screen, Settings, Snapshot, away};
use crate::files::FileId;
use crate::look::{Fill, overlay_surface};
use crate::notices::{Details, Kind, Subject};
use crate::stacking::{Phase, Stacking, Whose};
use crate::r#type::TypeStyled as _;
use crate::words::{DesktopWords, Wording as _, say};

/// Whose stack just ended ([`AppView::stack_ended`]).
pub(super) enum StackEnd {
    /// A single stack's, with what its render records.
    Single(Option<PendingRender>),
    /// One of the batch's; its machine holds the list its render records.
    OfBatch,
}

/// What the viewer showed as a single stack started, restored on cancel
/// ([`AppView::show_again`]).
#[derive(Clone, Debug, PartialEq)]
pub(super) enum ShownBefore {
    /// A render's result.
    Result(RenderId),
    /// A frame of the list, previewed.
    Frame(FileId),
    /// Nothing.
    Nothing,
}

/// How the window's last stack ended: a finished one's footer line, or a
/// failed one's card over the settings.
///
/// Lifecycle: `Nothing`, `Stacked` or `Failed`, held by `AppView` and
/// changed only by [`AppView::set_last_stack`]. `Nothing` → `Stacked` (all
/// frames, render kept) or `Failed`; either → `Nothing` when the next stack
/// starts or one ends with a notice instead; `Failed` → `Nothing` when its
/// card is dismissed.
pub(crate) enum LastStack {
    Nothing,
    Stacked(Stacked),
    Failed(StackFailure),
}

impl LastStack {
    fn name(&self) -> &'static str {
        match self {
            Self::Nothing => "nothing",
            Self::Stacked(_) => "stacked",
            Self::Failed(_) => "failed",
        }
    }
}

/// A stack that finished with every frame, its render kept: in the footer
/// while that render is the unchanged base.
pub(crate) struct Stacked {
    pub(crate) stack: StackId,
    /// The number its render was saved as.
    pub(crate) number: u32,
    /// How long it took.
    pub(crate) seconds: f32,
}

/// A failed stack: shown while its stack is on screen, until the next starts
/// or it is dismissed.
pub(crate) struct StackFailure {
    /// Try Again stacks what is on screen, so the card belongs to this stack only.
    pub(crate) stack: StackId,
    /// The number its render would have had.
    pub(crate) number: Option<u32>,
    /// Why, for the card, with the details Copy Details copies.
    pub(crate) error: Explanation,
}

/// "Stack #5 didn't finish.", the lead of a failed stack's message.
pub(crate) fn failure_lead(w: &DesktopWords, number: Option<u32>) -> String {
    match number {
        Some(number) => say!(w, "run-failure-lead-numbered", number = number.to_string()),
        None => say!(w, "run-failure-lead"),
    }
}

/// The pill text's minimum width: "Aligning · 100%", so it holds still as
/// numbers change.
const PILL_TEXT_WIDTH: f32 = 100.0;

/// The pill text's maximum width; longer lines (batches, sequences) are cut.
const PILL_TEXT_MAX_WIDTH: f32 = 280.0;

/// What runs, as its pill shows it ([`AppView::run_status`]).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RunStatus {
    pub(crate) text: String,
    pub(crate) bar: Bar,
}

/// A pill's bar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Bar {
    /// This far, 0 to 1.
    At(f32),
    /// Busy for an unmeasured while: saves or exports between batch stacks.
    Busy,
}

/// A stack's pill: "Blending · 68%".
fn stack_pill(cx: &App, phase: Phase, progress: f32) -> String {
    let percent = format!("{:.0}", progress * 100.0);
    match phase {
        Phase::Reading => say!(cx, "run-pill-reading", percent = percent),
        Phase::Aligning { .. } => say!(cx, "run-pill-aligning", percent = percent),
        Phase::Restarting => say!(cx, "run-pill-restarting", percent = percent),
        Phase::Blending => say!(cx, "run-pill-blending", percent = percent),
        Phase::Finishing => say!(cx, "run-pill-finishing", percent = percent),
        Phase::Canceling => say!(cx, "run-pill-canceling"),
    }
}

/// A batch's stack's step, after the batch's line: "Aligning".
fn stack_step(cx: &App, phase: Phase) -> String {
    match phase {
        Phase::Reading => say!(cx, "run-phase-reading"),
        Phase::Aligning { .. } => say!(cx, "run-phase-aligning"),
        Phase::Restarting => say!(cx, "run-phase-restarting"),
        Phase::Blending => say!(cx, "run-phase-blending"),
        Phase::Finishing => say!(cx, "run-phase-finishing"),
        Phase::Canceling => say!(cx, "run-pill-canceling"),
    }
}

impl AppView {
    /// Stack: the included images, or a selection of two or more, with the
    /// current settings.
    pub(crate) fn run_stacking(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_processing(cx) {
            return;
        }
        if !self.files.read(cx).can_stack() {
            return;
        }
        let (listed, frames) = self.files.read(cx).stack_input();
        let settings = self.settings(cx);
        self.start_stack(listed, frames, settings, None, window, cx);
    }

    /// Stack would start now: nothing runs and there are frames enough.
    pub(crate) fn can_stack(&self, cx: &App) -> bool {
        !self.is_processing(cx) && self.files.read(cx).can_stack()
    }

    /// The settings a stack started now runs with.
    pub(super) fn settings(&self, cx: &App) -> Settings {
        let params = self.params.read(cx);
        Settings { config: params.stacker_config(cx), options: params.session_options(cx) }
    }

    /// Stack `frames` with `settings`; the render records `listed` as its list
    /// and `batch` as its run.
    ///
    /// `false` if it didn't start (something runs, too few frames); nothing is
    /// locked then.
    pub(super) fn start_stack(
        &mut self,
        listed: Vec<PathBuf>,
        frames: Vec<PathBuf>,
        settings: Settings,
        batch: Option<Batch>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        // Only a batch starts stacks while it runs.
        let free =
            matches!((&self.running, &batch), (Running::Idle, None) | (Running::Batch(_), Some(_)));
        if !free || self.entities_running(cx) || !crate::stacking::Stacking::can_start(&frames) {
            return false;
        }
        self.leave_paint(cx);
        self.set_last_stack(LastStack::Nothing, cx);
        // The settings a stack runs with are worth keeping.
        if batch.is_none() {
            self.save_list(cx);
        }
        let Settings { config, mut options } = settings;
        // A batch's stacks show no live map.
        options.live_map = batch.is_none();
        // A batch's stacks are their own, lists kept by its machine; Stack
        // adds to the stack on screen.
        if !matches!(self.running, Running::Batch(_)) {
            let shown_before = self.shown_now(cx);
            let pending = PendingRender {
                listed,
                of: self.stack.clone(),
                from: self.base.clone(),
                shown_before,
            };
            self.set_running(Running::Stack(pending), cx);
        }
        // The live map fills the viewport in device pixels.
        let live_side = (LIVE_PREVIEW_SIDE as f32 * window.scale_factor()).ceil() as u32;
        // A batch makes stacks of its own: the stack on screen stays, locked.
        let whose = if batch.is_some() {
            Whose::Batch
        } else {
            self.renders.update(cx, |r, cx| r.clear_compare(cx));
            self.files.update(cx, |f, cx| f.highlight(None, window, cx));
            self.viewer.update(cx, |v, cx| v.clear(cx));
            // The rail becomes the run's inert key: end a hover.
            self.set_scrub(super::scrubber::Scrub::Idle, cx);
            Whose::Screen
        };
        self.stacking.update(cx, |s, cx| s.start(whose, frames, config, options, live_side, cx));
        true
    }

    /// What the viewer shows now: a render's result, else a frame.
    fn shown_now(&self, cx: &App) -> ShownBefore {
        if let Some(id) = self.renders.read(cx).active(cx) {
            return ShownBefore::Result(id.clone());
        }
        match self.viewer.read(cx).shown_file() {
            Some(id) => ShownBefore::Frame(id),
            None => ShownBefore::Nothing,
        }
    }

    /// Show `before` again in place of a canceled run's live map: the
    /// render's result while it is in the Library, the frame while listed,
    /// else nothing.
    pub(super) fn show_again(
        &mut self,
        before: ShownBefore,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        tracing::debug!(?before, "a canceled stack shows what it replaced");
        self.viewer.update(cx, |v, cx| v.clear(cx));
        match before {
            ShownBefore::Result(id) if self.renders.read(cx).find(&id).is_some() => {
                self.renders.update(cx, |r, cx| r.open(&id, window, cx));
            }
            ShownBefore::Frame(id) if self.files.read(cx).path(id).is_some() => {
                self.show_file(id, window, cx);
            }
            _ => {}
        }
    }

    /// A stack finished: show the result, keep the render, say how it ended.
    pub(super) fn stack_finished(
        &mut self,
        stacking: &Entity<Stacking>,
        seconds: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let left_out = stacking.read(cx).result().and_then(|s| s.notice(crate::words::of(cx)));
        // The render's number: the run's until `stack_ended`.
        let number = self.running_number(cx);
        let made = stacking.update(cx, |s, _| s.take_made());
        let shown = made.is_none();
        let (finished, thumbnail) = match made {
            // A batch's stack: the result on screen stays as it was.
            Some(made) => (Some(made.stack), made.thumbnail),
            None => {
                let finished = stacking.read(cx).result().cloned();
                let images = stacking.update(cx, |s, _| s.take_final_images());
                let mut thumbnail = None;
                if let Some(images) = images {
                    thumbnail = images.thumbnail;
                    self.viewer.update(cx, |v, cx| {
                        v.show_result(images.display, images.map, finished.as_ref(), cx)
                    });
                }
                (finished, thumbnail)
            }
        };
        let pending = match self.stack_ended(cx) {
            // The run saves it, when it says to.
            StackEnd::OfBatch => {
                return self.batch_stack_finished(finished, thumbnail, window, cx);
            }
            StackEnd::Single(pending) if shown => pending,
            // A batch's stack whose batch is over: nothing to keep or say.
            StackEnd::Single(_) => return,
        };
        let of = pending.as_ref().map_or_else(|| self.stack.clone(), |p| p.of.clone());
        let mut not_kept = None;
        if let (Some(stack), Some(pending)) = (finished, pending) {
            match thumbnail {
                // Readback failed: a render is saved with its thumbnail or
                // not at all.
                None => {
                    tracing::warn!("the stack made no thumbnail: its render is not kept");
                    not_kept = Some(NotKept::NoThumbnail);
                }
                Some(thumbnail) => {
                    let PendingRender { listed, of, from, .. } = pending;
                    self.renders.update(cx, |r, cx| {
                        r.autosave(listed, stack, thumbnail, of, from, None, window, cx)
                    });
                }
            }
        }
        let number = number.filter(|_| not_kept.is_none());
        let (message, warn) =
            stacked_message(seconds, number, left_out.as_deref(), not_kept, cx.wording());
        // A render not kept can still be exported.
        let actions = match not_kept {
            Some(NotKept::NoThumbnail) => vec![self.action(
                say!(cx, "app-stacked-export"),
                |this, window, cx| this.save_result(window, cx),
                cx,
            )],
            None => Vec::new(),
        };
        away::tell(away::RunEnd::Stacked, &message, window, cx);
        // A stack that went as asked ends in the inspector's footer, not a
        // notice over the Method picker as the next method is picked. Left-out
        // frames, an unkept render or a hidden footer still get a notice.
        let subject = Subject::Stack(of.clone());
        match number.filter(|_| !warn) {
            Some(number) => {
                let stacked = Stacked { stack: of, number, seconds };
                self.set_last_stack(LastStack::Stacked(stacked), cx);
                if self.footer_in_sight() {
                    crate::notices::dismiss(&subject, window, cx);
                } else {
                    self.post(Kind::Done, subject, message, actions, window, cx);
                }
            }
            None => {
                let kind = if warn { Kind::Warning } else { Kind::Done };
                self.post(kind, subject, message, actions, window, cx);
            }
        }
    }

    /// A stack was canceled: a single one's view goes back, and says so.
    pub(super) fn stack_canceled(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.stack_ended(cx) {
            StackEnd::OfBatch => {
                self.drive_batch(batch::Event::StackStopped, window, cx);
            }
            StackEnd::Single(pending) => {
                // The live map gives way to what was on screen before.
                if let Some(pending) = pending {
                    self.show_again(pending.shown_before, window, cx);
                }
                let subject = Subject::Stack(self.stack.clone());
                let text = say!(cx, "app-stacking-canceled");
                self.post(Kind::Info, subject, text, Vec::new(), window, cx);
            }
        }
    }

    /// A stack failed: a batch's goes on, a single one's is said in the inspector.
    pub(super) fn stack_failed(
        &mut self,
        error: &Explanation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let number = self.running_number(cx);
        // One stack failing doesn't stop the others.
        if let StackEnd::OfBatch = self.stack_ended(cx) {
            let why = crate::notices::sentences(cx.wording(), None, error, Details::Diagnostics);
            self.drive_batch(batch::Event::StackFailed(why.text), window, cx);
            return;
        }
        // Said in the inspector, with Try Again. Focus mode (no inspector)
        // ends; the Library (none) gets a lasting notice with Try Again.
        let stack = self.stack.clone();
        let failed = StackFailure { stack: stack.clone(), number, error: error.clone() };
        self.set_last_stack(LastStack::Failed(failed), cx);
        self.set_chrome(self.chrome.revealing_inspector(), cx);
        let lead = failure_lead(cx.wording(), number);
        let what =
            crate::notices::sentences(cx.wording(), Some(&lead), error, Details::Diagnostics);
        away::tell(away::RunEnd::StackFailed(number), &what.text, window, cx);
        if self.screen == Screen::Library {
            let actions = vec![
                self.action(
                    say!(cx, "run-try-again"),
                    |this, window, cx| this.run_stacking(window, cx),
                    cx,
                ),
                self.action(
                    say!(cx, "app-show-in-stacking"),
                    |this, window, cx| this.set_screen(Screen::Stacking, window, cx),
                    cx,
                ),
            ];
            self.post(Kind::Error, Subject::Stack(stack), what, actions, window, cx);
        }
        cx.notify();
    }

    /// The running stack ended: what it was to record. A batch stays running
    /// and locked until it closes.
    pub(super) fn stack_ended(&mut self, cx: &mut Context<Self>) -> StackEnd {
        match &mut self.running {
            Running::Batch(_) => StackEnd::OfBatch,
            Running::Stack(_) => match self.set_running(Running::Idle, cx) {
                Running::Stack(pending) => StackEnd::Single(Some(pending)),
                _ => StackEnd::Single(None),
            },
            Running::Idle | Running::Sequence { .. } => StackEnd::Single(None),
        }
    }

    /// Lock the strip and Renders while something runs or a stack switch is
    /// written ([`super::list::ListSave::Switching`]), dropping an open in
    /// flight so the leaving stack's render can't land over the new one.
    pub(super) fn lock_renders(&mut self, cx: &mut Context<Self>) {
        let hold = self.hold_in(cx);
        self.renders.update(cx, |r, cx| r.set_hold(hold, cx));
    }

    /// What runs over the strip: a stack into the one on screen shows its
    /// tile, "#5" filling.
    pub(super) fn hold_in(&self, cx: &App) -> Hold {
        let tile = self.running_number(cx).zip(self.stacking.read(cx).progress());
        match tile {
            Some((number, progress)) => Hold::Running { number, progress },
            None if !matches!(self.running, Running::Idle) || self.list_save.is_switching() => {
                Hold::Locked
            }
            None => Hold::Free,
        }
    }

    /// The one place what runs changes; the list, settings and history are
    /// locked unless idle. Returns what ran before.
    pub(super) fn set_running(&mut self, next: Running, cx: &mut Context<Self>) -> Running {
        let before = std::mem::replace(&mut self.running, next);
        tracing::debug!(from = before.name(), to = self.running.name(), "running");
        let locked = !matches!(self.running, Running::Idle);
        if locked != !matches!(before, Running::Idle) {
            self.files.update(cx, |f, cx| f.set_locked(locked, cx));
            self.params.update(cx, |p, cx| p.set_disabled(locked, cx));
            self.lock_renders(cx);
            // Settings' converter or profile changes meanwhile apply at the end.
            match locked {
                true => crate::prefs::hold_environment(cx),
                false => crate::prefs::release_environment(cx),
            }
        }
        cx.notify();
        before
    }

    /// The batch run, while one runs.
    pub(super) fn batch_run(&self) -> Option<&BatchRun> {
        match &self.running {
            Running::Batch(run) => Some(run),
            _ => None,
        }
    }

    pub(super) fn batch_run_mut(&mut self) -> Option<&mut BatchRun> {
        match &mut self.running {
            Running::Batch(run) => Some(run),
            _ => None,
        }
    }

    /// The running stack's render number in the stack on screen
    /// ([`deptheos_app::library::stacks::next_number`]); none for a batch's.
    pub(crate) fn running_number(&self, cx: &App) -> Option<u32> {
        let Running::Stack(pending) = &self.running else { return None };
        let renders = self.renders.read(cx);
        let info = renders.stack_info(&pending.of);
        Some(deptheos_app::library::stacks::next_number(&pending.of, renders.renders(), info))
    }

    /// The running stack's name: "Stacking #5", or the batch's line.
    fn running_title(&self, cx: &App) -> String {
        match (self.running_number(cx), self.batch_progress()) {
            (_, Some(batch)) => batch,
            (Some(number), None) => say!(cx, "run-stacking-numbered", number = number.to_string()),
            (None, None) => say!(cx, "run-stacking"),
        }
    }

    /// The card over the photo while a stack runs: what, time left, steps, bar.
    pub(super) fn render_progress_card(&self, cx: &App) -> Option<AnyElement> {
        let stacking = self.stacking.read(cx);
        let phase = stacking.phase()?;
        let progress = stacking.progress()?;
        let theme = cx.theme();
        let (muted, success) = (theme.muted_foreground, theme.success);
        let step = |done: bool, active: bool, label: String| {
            h_flex()
                .gap_1()
                .items_center()
                .text_xs()
                .when(!done && !active, |el| el.text_color(muted))
                .when(done, |el| el.child(Icon::new(IconName::Check).xsmall().text_color(success)))
                .when(active, |el| el.child(Spinner::new().xsmall()))
                .child(label)
        };
        // The aligning step, then the blending step: (done, active, label).
        let (align, blend) = match phase {
            Phase::Reading => ((false, true, say!(cx, "run-step-reading")), false),
            Phase::Aligning { loaded, total } => (
                (false, true, say!(cx, "run-step-aligning", loaded = loaded, total = total)),
                false,
            ),
            Phase::Restarting => ((false, true, say!(cx, "run-step-restarting")), false),
            Phase::Blending | Phase::Finishing | Phase::Canceling => {
                ((true, false, say!(cx, "run-step-aligned")), true)
            }
        };
        let steps = if phase == Phase::Canceling {
            h_flex().child(step(false, true, say!(cx, "run-step-canceling")))
        } else {
            let finished = phase == Phase::Finishing;
            h_flex().gap_4().child(step(align.0, align.1, align.2)).child(step(
                finished,
                blend && !finished,
                say!(cx, "run-step-blending"),
            ))
        };
        Some(
            overlay_surface(Fill::Controls, cx)
                .flex()
                .flex_col()
                .w(px(420.0))
                .p_3()
                .gap_2()
                .occlude()
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .truncate()
                                .child(self.running_title(cx)),
                        )
                        .when_some(stacking.time_left(), |el, left| {
                            el.child(
                                div()
                                    .text_xs()
                                    .tabular()
                                    .text_color(muted)
                                    .child(time_left(cx, left)),
                            )
                        }),
                )
                .child(steps)
                .child(Progress::new("stack-progress").value(progress * 100.0))
                .into_any_element(),
        )
    }

    /// Why a button is dimmed: what runs, and that it can be waited for or
    /// canceled. `None` while nothing runs.
    pub(crate) fn busy_reason(&self, cx: &App) -> Option<String> {
        match &self.running {
            Running::Idle if self.entities_running(cx) => Some(say!(cx, "run-busy-stack")),
            Running::Idle => None,
            Running::Stack(_) => Some(say!(cx, "run-busy-stack")),
            Running::Sequence { .. } => Some(say!(cx, "run-busy-sequence")),
            Running::Batch(_) => Some(say!(cx, "run-busy-batch")),
        }
    }

    /// The pill's line and progress; `None` while nothing runs.
    pub(crate) fn run_status(&self, cx: &App) -> Option<RunStatus> {
        let stacking = self.stacking.read(cx);
        let stack = stacking.phase().zip(stacking.progress());
        match &self.running {
            Running::Idle => None,
            Running::Stack(_) => {
                let (phase, progress) = stack?;
                Some(RunStatus { text: stack_pill(cx, phase, progress), bar: Bar::At(progress) })
            }
            Running::Sequence { .. } => {
                let (progress, text) = self.sequences.read(cx).progress()?;
                Some(RunStatus { text: text.to_string(), bar: Bar::At(progress) })
            }
            Running::Batch(run) => {
                let status = run.machine.status();
                if let Some((phase, progress)) = stack {
                    let step = stack_step(cx, phase);
                    let text = say!(cx, "run-pill-batch", status = status, step = step);
                    let bar = Bar::At(run.machine.fraction(f64::from(progress)) as f32);
                    return Some(RunStatus { text, bar });
                }
                if let Some((progress, text)) = self.sequences.read(cx).progress() {
                    return Some(RunStatus { text: text.to_string(), bar: Bar::At(progress) });
                }
                let bar = match run.machine.phase() {
                    deptheos_app::runs::batch::Phase::Ready
                    | deptheos_app::runs::batch::Phase::ReadyPaused
                    | deptheos_app::runs::batch::Phase::Paused
                    | deptheos_app::runs::batch::Phase::AwaitingExport { .. } => {
                        Bar::At(run.machine.fraction(0.0) as f32)
                    }
                    // Saving between stacks, or as it closes.
                    _ => Bar::Busy,
                };
                Some(RunStatus { text: status, bar })
            }
        }
    }

    /// The title bar pill while anything runs ("Blending · 68%", "Batch 2 of
    /// 5 · Bee · Aligning"), a small bar and Cancel (⌘.) as `cancel_id`.
    pub(super) fn render_run_status(
        &self,
        cancel_id: &'static str,
        ctx: Option<&'static str>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let RunStatus { text, bar } = self.run_status(cx)?;
        let can_cancel = self.can_cancel(cx);
        let cancel = say!(cx, "button-cancel");
        let cancel_tooltip = match self.running {
            Running::Sequence { .. } => say!(cx, "sidebar-cancel-sequence-tooltip"),
            Running::Batch(_) => say!(cx, "run-cancel-batch-tooltip"),
            Running::Stack(_) | Running::Idle => say!(cx, "run-cancel-stack-tooltip"),
        };
        let theme = cx.theme();
        let bar = match bar {
            Bar::At(progress) => Progress::new("pill-progress").value(progress * 100.0),
            Bar::Busy => Progress::new("pill-progress").loading(true),
        };
        Some(
            h_flex()
                .h(px(super::toolbar::TITLE_BAR_CONTROL_HEIGHT))
                .min_w_0()
                .gap_2()
                .pl_2p5()
                .pr_1()
                .items_center()
                .rounded(theme.radius)
                .border_1()
                .border_color(theme.border)
                .text_xs()
                .child(
                    div()
                        .min_w(px(PILL_TEXT_WIDTH))
                        .max_w(px(PILL_TEXT_MAX_WIDTH))
                        .whitespace_nowrap()
                        .truncate()
                        .tabular()
                        .child(text),
                )
                .child(div().w(px(80.0)).flex_shrink_0().child(bar))
                .child(
                    Button::new(cancel_id)
                        .label(cancel)
                        .xsmall()
                        .ghost()
                        .disabled(!can_cancel)
                        .tooltip_with_action(cancel_tooltip, &super::actions::CancelStack, ctx)
                        .on_click(cx.listener(|this, _, window, cx| this.cancel_run(window, cx))),
                )
                .into_any_element(),
        )
    }

    /// Copy Details' report: version, build and GPU, then `details`.
    pub(crate) fn report_details(&self, details: &str, cx: &App) -> String {
        let engine = self.sequences.read(cx).engine().clone();
        let gpu = &engine.gpu().adapter_info;
        format!(
            "Deptheos {} ({})\nGPU: {} ({:?})\n\n{details}",
            env!("CARGO_PKG_VERSION"),
            env!("DEPTHEOS_BUILD"),
            gpu.name,
            gpu.backend,
        )
    }

    /// The failure to show: the last one, if it is the stack on screen's.
    pub(crate) fn shown_failure(&self) -> Option<&StackFailure> {
        match &self.last_stack {
            LastStack::Failed(f) if f.stack == self.stack => Some(f),
            _ => None,
        }
    }

    /// The one place how the last stack ended changes.
    pub(super) fn set_last_stack(&mut self, next: LastStack, cx: &mut Context<Self>) {
        tracing::debug!(from = self.last_stack.name(), to = next.name(), "last stack");
        self.last_stack = next;
        cx.notify();
    }

    /// "Stacked in 3.9 s · #12" for the footer, while `now`'s working version
    /// is still what the last stack stacked.
    pub(crate) fn stacked_line(&self, now: &Snapshot, cx: &App) -> Option<String> {
        let LastStack::Stacked(stacked) = &self.last_stack else { return None };
        let base = now.base.as_ref()?;
        let same = stacked.stack == self.stack
            && base.number == stacked.number
            && !now.showing().is_edited();
        same.then(|| {
            say!(
                cx,
                "app-stacked-in-numbered",
                seconds = format!("{:.1}", stacked.seconds),
                number = stacked.number.to_string()
            )
        })
    }

    /// "Stack #5 didn't finish", with why, Try Again and Copy Details.
    pub(crate) fn render_failure(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let failure = self.shown_failure()?;
        let w = cx.wording().clone();
        let theme = cx.theme();
        let danger = theme.danger;
        let title = match failure.number {
            Some(number) => say!(w, "run-failure-title-numbered", number = number.to_string()),
            None => say!(w, "run-failure-title"),
        };
        let message = crate::notices::sentences(&w, None, &failure.error, Details::Copy).text;
        let details = self.report_details(&failure.error.details, cx);
        Some(
            v_flex()
                .p_2p5()
                .gap_1p5()
                .rounded(theme.radius)
                .border_1()
                .border_color(danger)
                .bg(danger.opacity(0.08))
                .child(
                    h_flex()
                        .gap_1p5()
                        .items_center()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(danger)
                        .child(Icon::new(IconName::TriangleAlert).small())
                        .child(div().flex_1().child(title))
                        .child(
                            Button::new("dismiss-failure")
                                .icon(IconName::Close)
                                .xsmall()
                                .ghost()
                                .tooltip(say!(w, "run-failure-dismiss"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.set_last_stack(LastStack::Nothing, cx);
                                })),
                        ),
                )
                .child(div().text_xs().child(message))
                .child(
                    h_flex()
                        .gap_1()
                        .child(
                            Button::new("try-again")
                                .label(say!(w, "run-try-again"))
                                .small()
                                .outline()
                                .on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.run_stacking(window, cx)
                                    }),
                                ),
                        )
                        .child(
                            Button::new("copy-failure")
                                .label(say!(w, "run-copy-details"))
                                .small()
                                .ghost()
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        details.clone(),
                                    ))
                                }),
                        ),
                )
                .into_any_element(),
        )
    }
}

/// "About 6 s left", "About 2 min left".
fn time_left(cx: &App, left: std::time::Duration) -> String {
    let seconds = left.as_secs_f32().ceil() as u64;
    match seconds {
        0..60 => say!(cx, "run-seconds-left", seconds = seconds.max(1).to_string()),
        _ => say!(cx, "run-minutes-left", minutes = seconds.div_ceil(60).to_string()),
    }
}

/// Why a finished stack's render was not kept in the Library.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NotKept {
    /// The stack's display could not be read back for its thumbnail.
    NoThumbnail,
}

/// A single stack's end message and whether it warns: time, frames left out,
/// and an unkept render, so it isn't taken for saved.
pub(super) fn stacked_message(
    seconds: f32,
    number: Option<u32>,
    left_out: Option<&str>,
    not_kept: Option<NotKept>,
    w: &DesktopWords,
) -> (String, bool) {
    let seconds = format!("{seconds:.1}");
    let message = match (left_out, not_kept) {
        (None, None) => match number {
            Some(number) => {
                say!(w, "app-stacked-in-numbered", seconds = seconds, number = number.to_string())
            }
            None => say!(w, "app-stacked-in", seconds = seconds),
        },
        (Some(left_out), None) => match number {
            Some(number) => say!(
                w,
                "app-stacked-in-numbered-left-out",
                seconds = seconds,
                number = number.to_string(),
                left_out = left_out
            ),
            None => say!(w, "app-stacked-in-left-out", seconds = seconds, left_out = left_out),
        },
        (None, Some(NotKept::NoThumbnail)) => {
            say!(w, "app-stacked-in-no-thumbnail", seconds = seconds)
        }
        (Some(left_out), Some(NotKept::NoThumbnail)) => {
            say!(w, "app-stacked-in-left-out-no-thumbnail", seconds = seconds, left_out = left_out)
        }
    };
    (message, left_out.is_some() || not_kept.is_some())
}

/// Keeps the Mac from idle sleep through a batch or grow sequence (hours,
/// unattended), held in the run's [`Running`] state. The hold lives in its
/// task until this is dropped.
pub(crate) struct Awake {
    _held: gpui::Task<gpui::Result<gpui::ActivityGuard>>,
}

impl Awake {
    pub(crate) fn new(reason: &str, cx: &App) -> Self {
        tracing::debug!("kept awake: {reason}");
        Self { _held: cx.prevent_idle_sleep(reason) }
    }
}
