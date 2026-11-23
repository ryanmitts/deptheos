//! The Batch… dialog: everything that makes more than one render.
//!
//! Top to bottom: the folders (picked, a parent's subfolders, or the list
//! as one source), how to split them (per folder, or by capture time),
//! RAW or JPEG of a pair, the RAW decoder when there is a choice, what
//! follows stacking (exports, a turntable sequence), and a summary. The
//! plan ([`deptheos_app::runs::batch::plan`]) is made on a worker, as auto
//! sort reads every capture time; Start hands it to the app view
//! ([`BatchEvent::Start`]).

use std::collections::HashMap;
use std::path::PathBuf;

use deptheos_app::frames::folder::{self, Source};
use deptheos_app::frames::list::List;
use deptheos_app::frames::names::file_label;
use deptheos_app::library::export::ExportFormat;
use deptheos_app::library::history::Batch;
use deptheos_app::runs::batch::sources::Sources;
use deptheos_app::runs::batch::{self, Input, Plan, Split};
use deptheos_app::runs::sequence::{self, Direction, TurntableMode, TurntableOptions};
use deptheos_app::{FrameOrder, RawBackend, SaveOptions};
use gpui::prelude::*;
use gpui::{App, Context, EventEmitter, SharedString, Task, Window, div, px};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::radio::RadioGroup;
use gpui_kit::component::select::Select;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};

use crate::app::ModalDialog;
use crate::choice::{Choice, ChoiceSelect, choice_select, choices_of, chosen};
use crate::file_dialog::{Picker, beside_last_opened, file_dialog, pick_folder, pick_folders};
use crate::form::{footer, heading, note, radio_row};
use crate::grow_dialog::FrameControls;
use crate::prefs::prefs;
use crate::words::{DesktopWords, Wording as _, say};
use crate::work;

// Emitted once a run: the size of Start is no matter.
#[allow(clippy::large_enum_variant)]
pub enum BatchEvent {
    /// Run `plan`; its renders go under `batch`'s header.
    Start {
        plan: Plan,
        batch: Batch,
        /// The decoder chosen for camera RAW, when there was a choice.
        raw_backend: Option<RawBackend>,
        after: After,
    },
    Cancel,
}

/// What a run does with its renders, besides keeping them.
#[derive(Clone, Debug, Default)]
pub struct After {
    pub export: Option<ExportEach>,
    pub turntable: Option<TurntableAfter>,
}

impl After {
    /// Start's 16-bit choice for the next Export sheet or Batch dialog: the
    /// exports', else the turntable frames', each only where it was a choice
    /// ([`depth_format`]). `None` when neither asked.
    pub fn sixteen_bit(&self) -> Option<bool> {
        let chose = |format: ExportFormat| !depth_format(format).is_depth_fixed(true);
        let export = self.export.as_ref().filter(|e| chose(e.format));
        let frames = self.turntable.as_ref().map(|t| &t.options.frames);
        let frames = frames.filter(|f| chose(f.format.into()));
        export.map(|e| e.options.sixteen_bit).or(frames.map(|f| f.save.sixteen_bit))
    }
}

/// The format the 16-bit checkbox stands for: DNG is RAW-only and always
/// 16-bit, so with DNG it is the other stacks' TIFF.
fn depth_format(format: ExportFormat) -> ExportFormat {
    format.for_stack(false)
}

/// Export each render as it is made, as Export… would.
#[derive(Clone, Debug)]
pub struct ExportEach {
    pub folder: PathBuf,
    /// DNG is written for RAW stacks only; others get TIFF.
    pub format: ExportFormat,
    pub options: SaveOptions,
}

/// A turntable sequence of the run's stacks, one per folder.
#[derive(Clone, Debug)]
pub struct TurntableAfter {
    pub folder: PathBuf,
    pub options: TurntableOptions,
}

/// Which folder a Choose… button picks.
#[derive(Clone, Copy)]
enum Output {
    Export,
    Turntable,
}

/// The After stacking controls; dropdowns need a window, so the app view
/// makes them.
pub struct AfterControls {
    /// Pyramid is selected: a turntable sequence needs it.
    pyramid: bool,
    export: bool,
    export_folder: Option<PathBuf>,
    export_format: ChoiceSelect<ExportFormat>,
    export_quality: ChoiceSelect<u8>,
    /// The export settings with 16-bit as ticked; quality is the dropdown's.
    export_save: SaveOptions,
    turntable: bool,
    turntable_folder: Option<PathBuf>,
    mode: TurntableMode,
    direction: Direction,
    frames_n: ChoiceSelect<Option<usize>>,
    frames: FrameControls,
}

/// The grow sequence lengths offered besides the stack depth.
const LENGTHS: [usize; 6] = [50, 100, 150, 200, 300, 400];

impl AfterControls {
    /// The run's turntable mode: grow replays the stack, which needs
    /// Pyramid, so other methods make Full only.
    fn mode(&self) -> TurntableMode {
        if self.pyramid { self.mode } else { TurntableMode::Full }
    }

    /// Starting from the export settings: `format` and `save`.
    pub fn new(
        pyramid: bool,
        format: ExportFormat,
        save: SaveOptions,
        window: &mut Window,
        cx: &mut gpui::App,
    ) -> Self {
        let mut qualities: Vec<u8> = vec![80, 90, 95, 100, save.jpeg_quality];
        qualities.sort_unstable();
        qualities.dedup();
        let qualities = qualities
            .into_iter()
            .map(|q| Choice::new(say!(cx, "batch-export-quality", quality = q.to_string()), q))
            .collect();
        let mut lengths = vec![Choice::new(say!(cx, "batch-length-stack-depth"), None)];
        lengths
            .extend(LENGTHS.iter().map(|&n| Choice::new(say!(cx, "frames-count", n = n), Some(n))));
        let frame_format = crate::app::frame_format(format);
        Self {
            pyramid,
            export: false,
            export_folder: None,
            export_format: choice_select(
                choices_of::<ExportFormat>(cx.wording()),
                &format,
                window,
                cx,
            ),
            export_quality: choice_select(qualities, &save.jpeg_quality, window, cx),
            export_save: save,
            turntable: false,
            turntable_folder: None,
            mode: TurntableMode::Grow,
            direction: Direction::BackToFront,
            frames_n: choice_select(lengths, &None, window, cx),
            frames: FrameControls::new(frame_format, save, window, cx),
        }
    }
}

pub struct BatchDialog {
    /// The folders added, alone or from Add Subfolders Of… (the engine's rules).
    sources: Sources,
    /// Add the Frames List: its included frames as one source, listed first.
    list_input: Option<Input>,
    split: Split,
    /// The current list, for Add the Frames List; `None` when it is empty.
    list: Option<List>,
    /// Which file of a RAW + JPEG pair to stack; starts as the list's choice.
    source: Source,
    /// The order of each render's frames; starts as the list's choice.
    order: FrameOrder,
    /// The settings the run will use, as the summary names them.
    settings: SharedString,
    /// The camera RAW decoder, when the Adobe converter is installed.
    decoder: Option<ChoiceSelect<RawBackend>>,
    /// Changed only by [`Self::set_planning`].
    planning: Planning,
    picker: Picker,
    after: AfterControls,
}

/// Where the plan for what is listed stands.
///
/// - any → `Listing`: a parent is looked in (`list_parents`).
/// - `Listing` → `Listing`: one parent's folders in, or a change meanwhile,
///   planned once all are in.
/// - `Listing` (last parent in), `Idle`, `Working`, `Planned`, `Failed` →
///   `Working`, or `Idle` with nothing listed (`replan`).
/// - `Working` → `Planned`, or `Failed`.
/// - `Listing` → `Idle` when the depth changes, before relisting.
///
/// Each worker's task lives in its state, so leaving it drops a stale
/// result. Start sends a copy of the plan, so a refused start stays planned.
enum Planning {
    /// No plan: nothing is listed.
    Idle,
    /// Parents being looked in, each on a worker: the plan waits for them.
    Listing(HashMap<PathBuf, Task<()>>),
    /// Being worked out on a worker; dropping the task cancels it.
    Working {
        _task: Task<()>,
    },
    Planned(Plan),
    /// Planning failed (a fault: an unreadable folder is part of a plan).
    Failed,
}

impl Planning {
    fn name(&self) -> &'static str {
        match self {
            Planning::Idle => "idle",
            Planning::Listing(_) => "listing",
            Planning::Working { .. } => "working",
            Planning::Planned(_) => "planned",
            Planning::Failed => "failed",
        }
    }

    /// Parents are being looked in.
    fn is_listing(&self) -> bool {
        matches!(self, Planning::Listing(_))
    }
}

impl EventEmitter<BatchEvent> for BatchDialog {}

impl BatchDialog {
    pub fn new(
        list: &List,
        settings: SharedString,
        decoder: Option<ChoiceSelect<RawBackend>>,
        after: AfterControls,
    ) -> Self {
        Self {
            sources: Sources::default(),
            list_input: None,
            split: Split::default(),
            list: (!list.is_empty()).then(|| list.clone()),
            source: list.source,
            order: list.order,
            settings,
            decoder,
            planning: Planning::Idle,
            picker: Picker::default(),
            after,
        }
    }

    /// A turntable sequence is ticked and possible: one stack per folder.
    fn turntable_on(&self) -> bool {
        self.after.turntable && self.split == Split::PerFolder
    }

    fn set_order(&mut self, order: FrameOrder, cx: &mut Context<Self>) {
        if self.order != order {
            self.order = order;
            self.replan(cx);
        }
    }

    fn set_export(&mut self, on: bool, cx: &mut Context<Self>) {
        self.after.export = on;
        cx.notify();
    }

    fn set_turntable(&mut self, on: bool, cx: &mut Context<Self>) {
        self.after.turntable = on;
        // A turntable runs its folders in name order.
        self.replan(cx);
    }

    /// What Start hands on: the ticked options, with their folders.
    fn after(&self, cx: &gpui::App) -> After {
        let a = &self.after;
        let export = a.export_folder.clone().filter(|_| a.export).map(|folder| ExportEach {
            folder,
            format: chosen(&a.export_format, cx).unwrap_or_default(),
            options: SaveOptions {
                jpeg_quality: chosen(&a.export_quality, cx).unwrap_or(a.export_save.jpeg_quality),
                ..a.export_save
            },
        });
        let turntable = a.turntable_folder.clone().filter(|_| self.turntable_on()).map(|folder| {
            TurntableAfter {
                folder,
                options: TurntableOptions {
                    mode: a.mode(),
                    direction: a.direction,
                    frames_n: chosen(&a.frames_n, cx).flatten(),
                    frames: a.frames.options(cx),
                },
            }
        });
        After { export, turntable }
    }

    /// Why the export folder won't do: it is, or is inside, a frames folder,
    /// where exports would be taken for frames next time.
    fn export_refused(&self, cx: &gpui::App) -> Option<String> {
        let folder = self.after.export_folder.as_ref().filter(|_| self.after.export)?;
        let input = self.planned()?.frames_folder_holding(folder)?;
        Some(batch::export_in_frames_text(crate::words::of(cx), &input))
    }

    /// Every ticked option has its folder.
    fn after_ready(&self) -> bool {
        let a = &self.after;
        (!a.export || a.export_folder.is_some())
            && (!self.turntable_on() || a.turntable_folder.is_some())
    }

    fn pick_output(&mut self, output: Output, window: &mut Window, cx: &mut Context<Self>) {
        let title = match output {
            Output::Export => say!(cx, "file-dialog-batch-renders"),
            Output::Turntable => say!(cx, "file-dialog-batch-sequence"),
        };
        // Start in the folder already chosen for this output, else the save folder.
        let chosen = match output {
            Output::Export => self.after.export_folder.clone(),
            Output::Turntable => self.after.turntable_folder.clone(),
        };
        let start = chosen.or_else(|| prefs(cx).save_dir.clone());
        let dialog = file_dialog(window, start.as_deref()).set_title(title);
        self.picker.ask(
            |this: &mut Self| &mut this.picker,
            || pick_folder(dialog),
            move |this, picked, _, cx| {
                if let Some(folder) = picked {
                    match output {
                        Output::Export => this.after.export_folder = Some(folder),
                        Output::Turntable => this.after.turntable_folder = Some(folder),
                    }
                }
                cx.notify();
            },
            window,
            cx,
        );
    }

    /// Add folders, each stacked on its own.
    pub fn add_folders(&mut self, folders: Vec<PathBuf>, cx: &mut Context<Self>) {
        self.sources.add_folders(folders);
        self.replan(cx);
    }

    /// Add the folders inside `parent` (not its own images).
    pub fn add_subfolders_of(&mut self, parent: PathBuf, cx: &mut Context<Self>) {
        self.sources.add_parent(parent.clone());
        self.list_parents(vec![parent], cx);
    }

    /// Add the Frames List: its included frames as one source.
    pub fn use_list(&mut self, cx: &mut Context<Self>) {
        let Some(list) = self.list.clone() else { return };
        if self.list_input.is_none() {
            self.list_input = Some(Input::List { list, label: say!(cx, "batch-frames-list") });
            self.replan(cx);
        }
    }

    pub fn set_deep(&mut self, deep: bool, cx: &mut Context<Self>) {
        // The parents' folders again, at the new depth.
        if !self.sources.set_deep(deep) {
            return;
        }
        // Folders found at the old depth are dropped with their tasks.
        self.set_planning(Planning::Idle);
        self.list_parents(self.sources.parents().to_vec(), cx);
    }

    /// Stack the RAW or the JPEG of each pair.
    pub fn set_source(&mut self, source: Source, cx: &mut Context<Self>) {
        if self.source != source {
            self.source = source;
            self.replan(cx);
        }
    }

    pub fn set_split(&mut self, split: Split, cx: &mut Context<Self>) {
        if self.split != split {
            self.split = split;
            self.replan(cx);
        }
    }

    /// Take row `ix` out: the Frames List, or a folder.
    fn remove(&mut self, ix: usize, cx: &mut Context<Self>) {
        match self.inputs().get(ix) {
            Some(Input::List { .. }) => self.list_input = None,
            Some(Input::Folder(folder)) => self.sources.remove(folder),
            None => return,
        }
        self.replan(cx);
    }

    /// What the run stacks, in order: the Frames List, then the folders.
    fn inputs(&self) -> Vec<Input> {
        let folders = self.sources.entries().iter().map(|e| Input::Folder(e.folder.clone()));
        self.list_input.iter().cloned().chain(folders).collect()
    }

    /// List each parent's folders on a worker (a drive takes a while), then
    /// plan once the last is in.
    fn list_parents(&mut self, parents: Vec<PathBuf>, cx: &mut Context<Self>) {
        if parents.is_empty() {
            self.replan(cx);
            return;
        }
        // The plan shown is for the folders before these: drop it.
        let mut listing = match self.set_planning(Planning::Idle) {
            Planning::Listing(listing) => listing,
            _ => HashMap::new(),
        };
        let deep = self.sources.deep();
        for parent in parents {
            let task = cx.spawn({
                let parent = parent.clone();
                async move |this, cx| {
                    let found = work::background("batch-folders", {
                        let parent = parent.clone();
                        move || folder::folders_inside(&parent, deep)
                    })
                    .await;
                    this.update(cx, |this, cx| this.listed(parent, deep, found, cx)).ok();
                }
            });
            // The same parent again replaces its listing.
            listing.insert(parent, task);
        }
        self.set_planning(Planning::Listing(listing));
        cx.notify();
    }

    /// `parent`'s folders are in: list them, and plan once none is pending.
    fn listed(
        &mut self,
        parent: PathBuf,
        deep: bool,
        found: Result<Vec<PathBuf>, String>,
        cx: &mut Context<Self>,
    ) {
        // Its task lives in `Listing`, so it is still wanted.
        let Planning::Listing(listing) = &mut self.planning else { return };
        // Dropping its own task here, while it runs, is how it ends.
        listing.remove(&parent);
        let left = listing.len();
        tracing::debug!("batch plan: listed a parent, {left} still being looked in");
        match found {
            // Unless removed while it was looked in.
            Ok(folders) => self.sources.listed(&parent, deep, folders),
            Err(e) => tracing::error!("could not list the folders: {e}"),
        }
        if left == 0 {
            self.set_planning(Planning::Idle);
        }
        self.replan(cx);
    }

    /// Move the plan to `planning`; the state it leaves is returned.
    fn set_planning(&mut self, planning: Planning) -> Planning {
        tracing::debug!("batch plan: {} -> {}", self.planning.name(), planning.name());
        std::mem::replace(&mut self.planning, planning)
    }

    /// Work out the plan again, on a worker.
    fn replan(&mut self, cx: &mut Context<Self>) {
        if self.planning.is_listing() {
            // Planned again once the folders are in.
            cx.notify();
            return;
        }
        let mut inputs = self.inputs();
        if inputs.is_empty() {
            self.set_planning(Planning::Idle);
            cx.notify();
            return;
        }
        if self.turntable_on() {
            batch::sort_folders(&mut inputs);
        }
        let (split, source, order) = (self.split, self.source, self.order);
        // No choice offered is LibRaw ([`crate::params::StackParams::batch_backend`]).
        // Every desktop decoder sorts frames into the same kinds, so a new
        // choice needs no new plan.
        let backend = self.decoder.as_ref().and_then(|d| chosen(d, cx)).unwrap_or_default();
        let words = crate::words::of(cx).clone();
        let task = cx.spawn(async move |this, cx| {
            let planned = work::interactive("batch-plan", move || {
                batch::plan(&inputs, split, source, order, backend, batch::capture_info, &words)
            })
            .await;
            // Replacing `Working` drops this task, so a landing plan is current.
            this.update(cx, |this, cx| {
                let next = match planned {
                    Ok(plan) => Planning::Planned(plan),
                    Err(e) => {
                        tracing::error!("could not plan the batch: {e}");
                        Planning::Failed
                    }
                };
                this.set_planning(next);
                cx.notify();
            })
            .ok();
        });
        self.set_planning(Planning::Working { _task: task });
        cx.notify();
    }

    /// The plan, once worked out for what is listed now.
    #[cfg(test)]
    pub fn plan(&self) -> Option<&Plan> {
        self.planned()
    }

    /// The folders added, each to be stacked on its own.
    #[cfg(test)]
    pub fn folders(&self) -> Vec<PathBuf> {
        self.sources.entries().iter().map(|e| e.folder.clone()).collect()
    }

    /// The plan, once worked out for what is listed now.
    fn planned(&self) -> Option<&Plan> {
        match &self.planning {
            Planning::Planned(plan) => Some(plan),
            _ => None,
        }
    }

    fn can_start(&self, cx: &gpui::App) -> bool {
        self.planned().is_some_and(|p| !p.jobs.is_empty())
            && self.after_ready()
            && self.export_refused(cx).is_none()
    }

    pub fn start(&mut self, cx: &mut Context<Self>) {
        if !self.can_start(cx) {
            return;
        }
        let Some(plan) = self.planned().cloned() else { return };
        let batch = Batch::of_plan(&plan);
        let raw_backend = self.decoder.as_ref().and_then(|d| chosen(d, cx));
        let after = self.after(cx);
        cx.emit(BatchEvent::Start { plan, batch, raw_backend, after });
    }

    fn pick_folders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dialog = file_dialog(window, beside_last_opened(cx).as_deref())
            .set_title(say!(cx, "file-dialog-add-folders"));
        self.picker.ask(
            |this: &mut Self| &mut this.picker,
            || pick_folders(dialog),
            |this, folders, _, cx| this.add_folders(folders, cx),
            window,
            cx,
        );
    }

    fn pick_parent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dialog = file_dialog(window, beside_last_opened(cx).as_deref())
            .set_title(say!(cx, "file-dialog-add-subfolders"));
        self.picker.ask(
            |this: &mut Self| &mut this.picker,
            || pick_folder(dialog),
            |this, picked, _, cx| {
                if let Some(parent) = picked {
                    this.add_subfolders_of(parent, cx);
                }
            },
            window,
            cx,
        );
    }

    fn summary(&self, cx: &gpui::App) -> String {
        match &self.planning {
            Planning::Listing(_) => say!(cx, "batch-summary-listing"),
            _ if self.list_input.is_none() && self.sources.entries().is_empty() => {
                say!(cx, "batch-summary-empty")
            }
            Planning::Working { .. } => say!(cx, "batch-summary-working"),
            Planning::Planned(plan) => {
                let mut summary = plan.summary(crate::words::of(cx));
                if self.turntable_on()
                    && let [first, .., last] = plan.jobs.as_slice()
                {
                    let (first, last) = (first.label.as_str(), last.label.as_str());
                    summary = say!(
                        cx,
                        "batch-summary-range",
                        summary = summary,
                        first = first,
                        last = last
                    );
                }
                if self.turntable_on() {
                    let after = &self.after;
                    let grow = chosen(&after.frames_n, cx).flatten().unwrap_or(stack_depth(plan));
                    let full = plan.jobs.len();
                    summary = match after.mode() {
                        TurntableMode::Grow => {
                            say!(cx, "batch-summary-turntable-grow", summary = summary, n = grow)
                        }
                        TurntableMode::Full => {
                            say!(cx, "batch-summary-turntable-full", summary = summary, n = full)
                        }
                        TurntableMode::Both => say!(
                            cx,
                            "batch-summary-turntable-both",
                            summary = summary,
                            grow = grow,
                            full = full
                        ),
                    };
                }
                if let Some(note) = plan.order_notes.first() {
                    let note = note.as_str();
                    summary = match plan.order_notes.len() - 1 {
                        0 => say!(cx, "batch-summary-note", summary = summary, note = note),
                        more => {
                            say!(
                                cx,
                                "batch-summary-notes",
                                summary = summary,
                                note = note,
                                more = more
                            )
                        }
                    };
                }
                summary
            }
            Planning::Idle => say!(cx, "batch-summary-empty"),
            Planning::Failed => {
                say!(cx, "batch-summary-failed", address = crate::app::diagnostics::SUPPORT_ADDRESS)
            }
        }
    }
}

/// The grow sequence's length after the stacks: their usual frame count.
fn stack_depth(plan: &Plan) -> usize {
    let lengths: Vec<usize> = plan.jobs.iter().map(|j| j.frames.len()).collect();
    sequence::frames_per_stack(&lengths)
}

/// "Choose Folder…" and the folder picked, or "No folder chosen".
fn folder_row(
    w: &DesktopWords,
    id: &'static str,
    folder: Option<&PathBuf>,
    muted: gpui::Hsla,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    h_flex()
        .gap_2()
        .items_center()
        .child(
            Button::new(id)
                .label(say!(w, "batch-choose-folder"))
                .small()
                .outline()
                .on_click(on_click),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_xs()
                .when(folder.is_none(), |el| el.text_color(muted))
                .truncate()
                .child(folder.map_or_else(
                    || say!(w, "batch-no-folder-chosen"),
                    |f| f.display().to_string(),
                )),
        )
}

impl BatchDialog {
    /// The After stacking section.
    fn render_after(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let a = &self.after;
        let w = cx.wording().clone();
        let muted = cx.theme().muted_foreground;
        let format = chosen(&a.export_format, cx).unwrap_or_default();
        let jpeg = format == ExportFormat::Jpeg;
        // A batch's stacks aren't known yet: each is taken to have 16 bits.
        let depth_fixed = depth_format(format).is_depth_fixed(true);
        let can_turntable = self.split == Split::PerFolder;
        let mode = a.mode();
        // The stacks' depth, once the folders are read.
        let depth = self.planned().filter(|p| !p.jobs.is_empty()).map(stack_depth);
        let length_note = match (chosen(&a.frames_n, cx).flatten(), depth) {
            (None, Some(n)) => say!(w, "batch-length-per-step", n = n),
            (None, None) => say!(w, "batch-length-per-step-unknown"),
            (Some(_), _) => say!(w, "batch-length-spread"),
        };
        let mut section = v_flex().gap_2().child(
            Checkbox::new("batch-export")
                .label(say!(w, "batch-export-each"))
                .checked(a.export)
                .on_click(cx.listener(|this, checked: &bool, _, cx| this.set_export(*checked, cx))),
        );
        if a.export {
            section = section.child(
                v_flex()
                    .pl_6()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(div().w(px(110.0)).child(Select::new(&a.export_format).small()))
                            .when(jpeg, |el| {
                                el.child(
                                    div()
                                        .w(px(130.0))
                                        .child(Select::new(&a.export_quality).small()),
                                )
                            })
                            .child(
                                Checkbox::new("batch-export-sixteen")
                                    .label(say!(w, "batch-export-sixteen-bit"))
                                    .checked(
                                        depth_format(format).depth(a.export_save.sixteen_bit, true),
                                    )
                                    .disabled(depth_fixed)
                                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                        this.after.export_save.sixteen_bit = *checked;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(folder_row(
                        &w,
                        "batch-export-folder",
                        a.export_folder.as_ref(),
                        muted,
                        cx.listener(|this, _, window, cx| {
                            this.pick_output(Output::Export, window, cx)
                        }),
                    ))
                    .child(note(say!(w, "batch-export-note"), muted))
                    .children(self.export_refused(cx).map(|why| note(why, cx.theme().danger))),
            );
        }
        section = section.child(
            Checkbox::new("batch-turntable")
                .label(say!(w, "batch-turntable"))
                .checked(a.turntable && can_turntable)
                .disabled(!can_turntable)
                .on_click(
                    cx.listener(|this, checked: &bool, _, cx| this.set_turntable(*checked, cx)),
                ),
        );
        if !can_turntable {
            let needs = say!(w, "batch-turntable-needs-per-folder");
            section = section.child(div().pl_6().child(note(needs, muted)));
        } else if a.turntable {
            let frames = a.frames.render("turntable", |this: &mut Self| &mut this.after.frames, cx);
            let words = crate::words::of(cx).clone();
            section = section.child(
                v_flex()
                    .pl_6()
                    .gap_2()
                    .child(radio_row(
                        RadioGroup::horizontal("turntable-mode"),
                        &TurntableMode::ALL,
                        |m| m.label(&words),
                        mode,
                        |m| a.pyramid || m == TurntableMode::Full,
                        cx.listener(|this, mode: &TurntableMode, _, cx| {
                            if this.after.pyramid {
                                this.after.mode = *mode;
                            }
                            cx.notify();
                        }),
                    ))
                    .when(!a.pyramid, |el| {
                        el.child(note(say!(w, "batch-turntable-needs-pyramid"), muted))
                    })
                    .when(mode.grow(), |el| {
                        el.child(h_flex().gap_2().items_center().child(radio_row(
                            RadioGroup::horizontal("turntable-direction"),
                            &Direction::ALL,
                            |d| match d {
                                Direction::BackToFront => say!(w, "batch-direction-back-to-front"),
                                Direction::FrontToBack => say!(w, "batch-direction-front-to-back"),
                            },
                            a.direction,
                            |_| true,
                            cx.listener(|this, direction: &Direction, _, cx| {
                                this.after.direction = *direction;
                                cx.notify();
                            }),
                        )))
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(div().text_sm().child(say!(w, "batch-length")))
                                .child(div().w(px(170.0)).child(Select::new(&a.frames_n).small()))
                                .child(note(length_note.clone(), muted)),
                        )
                    })
                    .child(frames)
                    .child(folder_row(
                        &w,
                        "batch-turntable-folder",
                        a.turntable_folder.as_ref(),
                        muted,
                        cx.listener(|this, _, window, cx| {
                            this.pick_output(Output::Turntable, window, cx)
                        }),
                    ))
                    .child(note(say!(w, "batch-turntable-note"), muted)),
            );
        }
        section.into_any_element()
    }
}

impl Render for BatchDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let w = cx.wording().clone();
        v_flex()
            .w_full()
            .gap_3()
            .child(self.render_folders(cx))
            .child(self.render_grouping(cx))
            .child(heading(say!(w, "batch-after-heading")))
            .child(self.render_after(cx))
            .child(self.render_summary(cx))
            .child(footer(
                "batch-cancel",
                cx.listener(|_, _, _, cx| cx.emit(BatchEvent::Cancel)),
                Button::new("batch-start")
                    .label(say!(w, "batch-start"))
                    .primary()
                    .disabled(!self.can_start(cx))
                    .on_click(cx.listener(|this, _, _, cx| this.start(cx))),
            ))
    }
}

impl BatchDialog {
    /// The folders: add buttons, Include Deeper Folders, and the list with Remove.
    fn render_folders(&self, cx: &mut Context<Self>) -> gpui::Div {
        let w = cx.wording().clone();
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        let inputs = self.inputs();
        let has_entries = !inputs.is_empty();
        let rows = inputs.into_iter().enumerate().map(|(ix, input)| {
            let (name, detail) = match &input {
                Input::Folder(path) => (
                    file_label(path),
                    path.parent().map(|p| p.display().to_string()).unwrap_or_default(),
                ),
                Input::List { list, label } => {
                    let shots = list.shots();
                    let included = shots.iter().filter(|s| list.is_included(s)).count();
                    (label.clone(), say!(w, "frames-count", n = included))
                }
            };
            h_flex()
                .gap_2()
                .px_2()
                .py_1()
                .items_center()
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().text_sm().truncate().child(name))
                        .child(div().text_xs().text_color(muted).truncate().child(detail)),
                )
                .child(
                    Button::new(("batch-remove", ix))
                        .icon(IconName::Close)
                        .xsmall()
                        .ghost()
                        .tooltip(say!(w, "batch-remove"))
                        .on_click(cx.listener(move |this, _, _, cx| this.remove(ix, cx))),
                )
        });
        v_flex()
            .gap_3()
            .child(heading(say!(w, "batch-folders-heading")))
            .child(
                h_flex()
                    .gap_1()
                    .flex_wrap()
                    .child(
                        Button::new("batch-add-folders")
                            .label(say!(w, "batch-add-folders"))
                            .small()
                            .outline()
                            .on_click(
                                cx.listener(|this, _, window, cx| this.pick_folders(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("batch-add-subfolders")
                            .label(say!(w, "batch-add-subfolders"))
                            .small()
                            .outline()
                            .tooltip(say!(w, "batch-add-subfolders-tooltip"))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.pick_parent(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("batch-use-list")
                            .label(say!(w, "batch-use-list"))
                            .small()
                            .outline()
                            .disabled(self.list.is_none())
                            .tooltip(say!(w, "batch-use-list-tooltip"))
                            .on_click(cx.listener(|this, _, _, cx| this.use_list(cx))),
                    ),
            )
            .child(
                Checkbox::new("batch-deep")
                    .label(say!(w, "batch-include-deeper"))
                    .checked(self.sources.deep())
                    .on_click(
                        cx.listener(|this, checked: &bool, _, cx| this.set_deep(*checked, cx)),
                    ),
            )
            .child(
                div()
                    .id("batch-folders")
                    .h(px(180.0))
                    .overflow_y_scroll()
                    .rounded_md()
                    .border_1()
                    .border_color(border)
                    .when(!has_entries, |el| {
                        let empty = say!(w, "batch-no-folders");
                        el.child(div().p_3().text_sm().text_color(muted).child(empty))
                    })
                    .children(rows),
            )
    }

    /// How folders become stacks: split, pair file, frame order, RAW decoder.
    fn render_grouping(&self, cx: &mut Context<Self>) -> gpui::Div {
        let w = cx.wording().clone();
        v_flex()
            .gap_3()
            .child(heading(say!(w, "batch-split-heading")))
            .child(radio_row(
                RadioGroup::vertical("batch-split"),
                &Split::ALL,
                |s| match s {
                    Split::PerFolder => say!(w, "batch-split-per-folder"),
                    Split::AutoSort => say!(w, "batch-split-auto-sort"),
                },
                self.split,
                |_| true,
                cx.listener(|this, split: &Split, _, cx| this.set_split(*split, cx)),
            ))
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(div().text_sm().child(say!(w, "batch-pairs")))
                    .child(radio_row(
                        RadioGroup::horizontal("batch-source"),
                        &Source::ALL,
                        |s| match s {
                            Source::Raw => say!(w, "batch-pairs-raw"),
                            Source::Standard => say!(w, "batch-pairs-jpeg"),
                        },
                        self.source,
                        |_| true,
                        cx.listener(|this, source: &Source, _, cx| this.set_source(*source, cx)),
                    )),
            )
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(div().text_sm().child(say!(w, "batch-order")))
                    .child(radio_row(
                        RadioGroup::horizontal("batch-order"),
                        &FrameOrder::ALL,
                        |o| match o {
                            FrameOrder::Name => say!(w, "batch-order-name"),
                            FrameOrder::CaptureTime => say!(w, "batch-order-capture-time"),
                        },
                        self.order,
                        |_| true,
                        cx.listener(|this, order: &FrameOrder, _, cx| this.set_order(*order, cx)),
                    )),
            )
            .when_some(self.decoder.clone(), |el, decoder| {
                el.child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(div().text_sm().child(say!(w, "batch-raw-decoder")))
                        .child(div().w(px(200.0)).child(Select::new(&decoder).small())),
                )
            })
    }

    /// What the batch will make, and with which settings.
    fn render_summary(&self, cx: &mut Context<Self>) -> gpui::Div {
        let w = cx.wording().clone();
        let muted = cx.theme().muted_foreground;
        v_flex().gap_0p5().child(div().text_sm().child(self.summary(cx))).child(
            div().text_xs().text_color(muted).child({
                let settings = self.settings.to_string();
                match self.source {
                    Source::Raw => say!(w, "batch-settings-pairs-raw", settings = settings),
                    Source::Standard => {
                        say!(w, "batch-settings-pairs-jpeg", settings = settings)
                    }
                }
            }),
        )
    }
}

impl ModalDialog for BatchDialog {
    const WIDTH: f32 = 520.0;

    fn title(cx: &App) -> SharedString {
        say!(cx, "batch-dialog-title").into()
    }

    /// Start, once there is something to stack.
    fn confirm(&mut self, cx: &mut Context<Self>) {
        self.start(cx);
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;
    use crate::test_support::{open, wait_until};

    /// The dialog for `list`, in a window, as the app view opens it.
    fn dialog_for(list: &List, cx: &mut TestAppContext) -> gpui::Entity<BatchDialog> {
        let list = list.clone();
        open(cx, move |window, cx| {
            let save = SaveOptions::default();
            let after = AfterControls::new(true, ExportFormat::Tiff, save, window, cx);
            cx.new(|_| BatchDialog::new(&list, "Pyramid".into(), None, after))
        })
        .1
    }

    /// Start remembers 16-bit where a 16-bit format was chosen: exports'
    /// first, else the turntable frames'.
    #[test]
    fn start_remembers_the_sixteen_bit_choice() {
        let export = |format, sixteen_bit| ExportEach {
            folder: "out".into(),
            format,
            options: SaveOptions { sixteen_bit, ..SaveOptions::default() },
        };
        let turntable = |format, sixteen_bit| TurntableAfter {
            folder: "turn".into(),
            options: TurntableOptions {
                frames: sequence::FrameOptions {
                    format,
                    save: SaveOptions { sixteen_bit, ..SaveOptions::default() },
                    ..Default::default()
                },
                ..Default::default()
            },
        };
        use sequence::FrameFormat as Frames;
        assert_eq!(After::default().sixteen_bit(), None);
        let after = |export, turntable| After { export, turntable };
        assert_eq!(after(Some(export(ExportFormat::Png, false)), None).sixteen_bit(), Some(false));
        assert_eq!(after(Some(export(ExportFormat::Jpeg, false)), None).sixteen_bit(), None);
        let both =
            after(Some(export(ExportFormat::Tiff, true)), Some(turntable(Frames::Png, false)));
        assert_eq!(both.sixteen_bit(), Some(true));
        let frames =
            after(Some(export(ExportFormat::Jpeg, true)), Some(turntable(Frames::Tiff, false)));
        assert_eq!(frames.sixteen_bit(), Some(false));
        assert_eq!(after(None, Some(turntable(Frames::Jpeg, true))).sixteen_bit(), None);
    }

    fn folder(root: &std::path::Path, name: &str, files: &[&str]) -> PathBuf {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        for f in files {
            std::fs::write(dir.join(f), b"").unwrap();
        }
        dir
    }

    #[gpui::test]
    fn folders_parents_and_depth_make_the_plan(cx: &mut TestAppContext) {
        let root = tempfile::tempdir().unwrap();
        let loose = folder(root.path(), "loose", &["l1.jpg", "l2.jpg"]);
        let card = root.path().join("card");
        folder(&card, "a", &["a1.jpg", "a2.jpg"]);
        folder(&card, "b", &["b1.jpg", "b2.jpg", "b3.jpg"]);
        folder(&card, "b/deep", &["d1.jpg", "d2.jpg"]);
        std::fs::write(card.join("own.jpg"), b"").unwrap();
        let dialog = dialog_for(&List::default(), cx);
        let planned = |cx: &mut TestAppContext| {
            wait_until(cx, "the plan", |cx| dialog.read_with(cx, |d, _| d.plan().is_some()));
            dialog.read_with(cx, |d, _| d.plan().unwrap().clone())
        };

        dialog.update(cx, |d, cx| {
            d.add_folders(vec![loose.clone()], cx);
            d.add_subfolders_of(card.clone(), cx);
        });
        let plan = planned(cx);
        let labels: Vec<&str> = plan.jobs.iter().map(|j| j.label.as_str()).collect();
        assert_eq!(labels, ["loose", "a", "b"], "the parent's own image is left out");
        assert_eq!(
            plan.summary(&deptheos_app::Words::english(deptheos_app::Platform::Desktop)),
            "3 folders → 3 renders"
        );

        // Deeper: b/deep too. Removing a found folder keeps it out.
        dialog.update(cx, |d, cx| d.set_deep(true, cx));
        assert_eq!(planned(cx).jobs.len(), 4);
        dialog.update(cx, |d, cx| d.remove(1, cx));
        let labels: Vec<String> = planned(cx).jobs.into_iter().map(|j| j.label).collect();
        assert_eq!(labels, ["loose", "b", "deep"]);
        dialog.update(cx, |d, cx| d.set_deep(false, cx));
        assert_eq!(planned(cx).jobs.len(), 2, "a stays removed");

        // Add the Frames List is off with an empty list.
        dialog.update(cx, |d, cx| d.use_list(cx));
        assert_eq!(planned(cx).folders, 2);
    }

    /// A depth change during a parent's listing drops it: one listing per
    /// parent, at the new depth, and the plan is of what it finds.
    #[gpui::test]
    fn a_depth_change_while_listing_drops_the_listing_before(cx: &mut TestAppContext) {
        let root = tempfile::tempdir().unwrap();
        let card = root.path().join("card");
        folder(&card, "a", &["a1.jpg", "a2.jpg"]);
        folder(&card, "a/deep", &["d1.jpg", "d2.jpg"]);
        let dialog = dialog_for(&List::default(), cx);
        dialog.update(cx, |d, cx| {
            d.add_subfolders_of(card.clone(), cx);
            d.add_subfolders_of(card.clone(), cx);
            d.set_deep(true, cx);
            let Planning::Listing(listing) = &d.planning else { panic!("not listing") };
            assert_eq!(listing.len(), 1, "one listing of the one parent");
        });
        wait_until(cx, "the plan", |cx| dialog.read_with(cx, |d, _| d.plan().is_some()));
        let labels: Vec<String> = dialog
            .read_with(cx, |d, _| d.plan().unwrap().jobs.iter().map(|j| j.label.clone()).collect());
        assert_eq!(labels, ["a", "deep"]);
    }

    /// Start leaves the plan in place, so a refused batch leaves the dialog
    /// as it was.
    #[gpui::test]
    fn start_keeps_the_plan_for_a_refused_start(cx: &mut TestAppContext) {
        let root = tempfile::tempdir().unwrap();
        let bee = folder(root.path(), "bee", &["1.jpg", "2.jpg"]);
        let dialog = dialog_for(&List::default(), cx);
        dialog.update(cx, |d, cx| d.add_folders(vec![bee], cx));
        wait_until(cx, "the plan", |cx| dialog.read_with(cx, |d, _| d.plan().is_some()));
        let started = std::rc::Rc::new(std::cell::Cell::new(0));
        let count = started.clone();
        cx.update(|cx| {
            cx.subscribe(&dialog, move |_, event: &BatchEvent, _| {
                if matches!(event, BatchEvent::Start { .. }) {
                    count.set(count.get() + 1);
                }
            })
            .detach()
        });
        dialog.update(cx, |d, cx| d.start(cx));
        assert_eq!(started.get(), 1);
        dialog.read_with(cx, |d, cx| {
            assert!(d.plan().is_some(), "still planned");
            assert_eq!(d.summary(cx), "1 folder → 1 render");
        });
    }

    /// An export folder that is or is inside a stacked folder is refused
    /// with why; one beside them starts.
    #[gpui::test]
    fn an_export_folder_inside_the_frames_is_refused(cx: &mut TestAppContext) {
        let root = tempfile::tempdir().unwrap();
        let bee = folder(root.path(), "bee", &["1.jpg", "2.jpg"]);
        let dialog = dialog_for(&List::default(), cx);
        dialog.update(cx, |d, cx| d.add_folders(vec![bee.clone()], cx));
        wait_until(cx, "the plan", |cx| dialog.read_with(cx, |d, _| d.plan().is_some()));
        let why = "The exports can't go into \"bee\", which holds frames this batch stacks. \
                   Choose a folder outside the folders being stacked.";
        for (folder, refused) in
            [(bee.clone(), true), (bee.join("out"), true), (root.path().join("out"), false)]
        {
            dialog.update(cx, |d, _| {
                d.after.export = true;
                d.after.export_folder = Some(folder.clone());
            });
            dialog.read_with(cx, |d, cx| {
                let said = d.export_refused(cx);
                assert_eq!(said.as_deref(), refused.then_some(why), "{folder:?}");
                assert_eq!(d.can_start(cx), !refused, "{folder:?}");
            });
        }
    }

    /// A pair stacks its RAW or JPEG as chosen, starting from the list's
    /// choice, for folders and the list alike.
    #[gpui::test]
    fn pairs_stack_the_file_chosen(cx: &mut TestAppContext) {
        let root = tempfile::tempdir().unwrap();
        let shots = folder(root.path(), "shots", &["p1.ORF", "p1.JPG", "p2.ORF", "p2.JPG"]);
        let list =
            List { files: vec![shots.join("p1.ORF"), shots.join("p1.JPG")], ..List::default() };
        let dialog = dialog_for(&list, cx);
        let frames = |cx: &mut TestAppContext| {
            wait_until(cx, "the plan", |cx| dialog.read_with(cx, |d, _| d.plan().is_some()));
            dialog.read_with(cx, |d, _| {
                let plan = d.plan().unwrap();
                plan.jobs
                    .iter()
                    .flat_map(|j| &j.frames)
                    .map(|f| f.extension().unwrap().to_owned())
                    .collect::<Vec<_>>()
            })
        };
        dialog.update(cx, |d, cx| d.add_folders(vec![shots.clone()], cx));
        assert_eq!(frames(cx), ["ORF", "ORF"], "the list's choice: RAW");
        dialog.update(cx, |d, cx| d.set_source(Source::Standard, cx));
        assert_eq!(frames(cx), ["JPG", "JPG"]);
    }
}
