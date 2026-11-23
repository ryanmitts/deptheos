//! A batch run as one state machine, shared by every front end.
//!
//! The front end sends each [`Event`] to [`Machine::on`] and each render to
//! [`Machine::stacked`], then carries out the [`Action`]s returned, in order,
//! saving a render first when [`Stacked::save`] says so. The machine does no
//! I/O and knows each stack only by its [`StackNo`] (ADR-144).
//!
//! ```text
//!   Ready ◀──Pause/Resume──▶ ReadyPaused ──Start──▶ Paused
//!     │
//!     │      Start
//!   Ready ─────────▶ Stacking ◀───────────────┐ next job, or a restack
//!                    │ │ │   ▲ Resume          │ (turntable check)
//!          returned  │ │ │   └──────┐          │
//!          while     │ │ └─▶ Paused ┘          │
//!          paused    │ │                       │
//!                    │ └─▶ AwaitingExport ─────┤ its export landed
//!                    ▼ all jobs ended          │
//!               AwaitingSaves ─────────────────┘ (turntable: saves in)
//!                    │
//!                    ▼ (turntable)            exports or saves
//!                 Sequence ─────▶ Closing ───────────────▶ Done
//!                                   ▲ in flight               ▲
//!                                   └──── nothing turntable ──┘
//! ```
//!
//! Rules, the same in every front end:
//!
//! - Cancel stops what runs; the run is done once saves and exports under
//!   way land.
//! - A render that fails to save is recorded as a failed stack.
//! - With a history, Start records every job as an unstacked failed stack
//!   ([`Action::RecordUnstacked`]), hidden while the run lasts: the job's
//!   saved render removes it, a failure rewords it
//!   ([`Action::RecordFailure`]), and whatever ends the run leaves the rest
//!   to try again.
//! - The first failed save or export stops the run ([`End::Stopped`]).
//! - A stack returning while paused (iOS in the background) runs again
//!   unless it made a render, however it failed.
//! - A pause holds the next step in every phase; the sequence and closing
//!   start nothing more, so they go on.
//! - One stack at a time, and one export at a time: the next stack waits
//!   for the export, so each export takes a free file name and the file
//!   system settles labels that differ only in case.

use std::path::PathBuf;

use super::Job;
use crate::frames::names::file_label;
use crate::runs::sequence::{Fixed, ManifestGap, StackFix, StackShape, fixing_text};
use crate::words::explain::quoted;
use crate::words::{SEPARATOR, Words, say};

/// A stack the run started (a job's or a turntable restack); the front
/// end keys what it keeps by it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StackNo(pub u32);

/// A job's index in the run, and in [`Action::RecordUnstacked`]'s jobs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct JobNo(pub usize);

/// What a run does besides stacking.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// Keep each render in the history ([`Event::Saved`] or
    /// [`Event::SaveFailed`] follows); without it a render counts as kept.
    pub save: bool,
    /// Each render is exported as it is made.
    pub export: bool,
    /// A turntable sequence is written from the renders.
    pub turntable: Option<Turntable>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Turntable {
    /// Restack stacks whose first frame throws them off ([`StackFix`])
    /// before writing the sequence.
    pub fix: bool,
}

/// What a front end saw.
#[derive(Clone, Debug)]
pub enum Event {
    /// Start the run.
    Start,
    /// The stack failed: sentences of reason and advice
    /// (`deptheos_app::words::explain`), shown after the run's lead.
    StackFailed(String),
    /// The stack ended without a render: canceled or stopped.
    StackStopped,
    Saved(StackNo),
    /// The render couldn't be kept; `error` as for [`Event::StackFailed`].
    SaveFailed {
        stack: StackNo,
        error: String,
    },
    /// The export ended; an `Err` as for [`Event::StackFailed`].
    Exported {
        stack: StackNo,
        result: Result<(), String>,
    },
    /// The turntable sequence ended.
    SequenceEnded(SequenceEnd),
    /// Cancel: stop what runs; what is made is kept.
    Cancel,
    /// Start nothing more until [`Event::Resume`]; a stack returning
    /// without a render runs again.
    Pause,
    Resume,
}

/// How the turntable sequence ended, with the summary's words: a short
/// part ("48 frames written"), or sentences saying why it failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SequenceEnd {
    Finished(String),
    Canceled(String),
    Failed(String),
}

/// What the front end is to do, in the order given.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Stack `frames`, of the `listed` files, as `stack`; one at a time.
    Stack {
        stack: StackNo,
        label: String,
        listed: Vec<PathBuf>,
        frames: Vec<PathBuf>,
    },
    CancelStack,
    /// Export `stack`'s render as `label`, over `replaces`'s file if given.
    Export {
        stack: StackNo,
        label: String,
        replaces: Option<StackNo>,
    },
    /// At Start, with a history: record `jobs[n]` as `JobNo(n)`, a failed
    /// stack saying `why`, in the stack its render will be saved into,
    /// hidden from the Library while the run lasts.
    RecordUnstacked {
        jobs: Vec<Job>,
        why: String,
    },
    /// Job `no` failed: reword and show its record from
    /// [`Action::RecordUnstacked`], to try again.
    RecordFailure {
        no: JobNo,
        job: Job,
        error: String,
    },
    /// Write the turntable sequence of `stacks`, in order.
    WriteSequence {
        stacks: Vec<SequenceStack>,
        skipped: Vec<ManifestGap>,
        notes: Vec<String>,
    },
    CancelSequence,
    /// Delete these renders from the history: restacks replaced them.
    Delete(Vec<StackNo>),
    /// The run is over.
    Finish(Summary),
}

/// What to do with the render just made ([`Machine::stacked`]).
#[derive(Clone, Debug, PartialEq)]
#[must_use]
pub struct Stacked {
    /// Save it before anything in `then`, then report [`Event::Saved`] or
    /// [`Event::SaveFailed`]. `None` without a history or when the render
    /// isn't kept (a restack still off, a canceled run's).
    pub save: Option<Save>,
    pub then: Vec<Action>,
}

/// Save the render just made as `stack`, recording `listed` (the job's
/// frames, or for a restack the replaced render's list).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Save {
    pub stack: StackNo,
    /// The job whose record ([`Action::RecordUnstacked`]) the save
    /// replaces; `None` for a restack, saved as its own stack.
    pub job: Option<JobNo>,
    pub listed: Vec<PathBuf>,
}

/// A stack of the turntable sequence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SequenceStack {
    pub stack: StackNo,
    pub label: String,
    pub paths: Vec<PathBuf>,
}

/// Why a stack running is to stop early.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    Cancel,
    /// Paused while it ran: unless it made a render it runs again.
    Pause(Again),
}

/// When a stack stopped by a pause runs again. The last of
/// [`Event::Pause`] and [`Event::Resume`] before it returns decides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Again {
    /// Still paused: once resumed.
    OnResume,
    /// Resumed before the stack returned: as soon as it does.
    AtOnce,
}

/// Which stack of a job runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StackKind {
    /// The job's own stack, of its frames.
    Job,
    /// The turntable check's restack without the first frame.
    Restack,
}

/// Where the run is.
///
/// Lifecycle: ready, paused before Start, stacking, paused, awaiting an
/// export, awaiting saves, the sequence, closing, done; changed only by
/// its [`Machine`], as the module's diagram shows. Pause takes Ready to
/// ReadyPaused and AwaitingExport or AwaitingSaves to Paused; Resume takes
/// ReadyPaused to Ready, and Paused on.
#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    /// Not started.
    Ready,
    /// Not started, paused: Start records the jobs and goes to
    /// [`Phase::Paused`].
    ReadyPaused,
    /// A stack runs: job `slot`'s, or its restack in the turntable check.
    Stacking { stack: StackNo, slot: usize, kind: StackKind, stop: Option<Stop> },
    /// Nothing runs; the next step starts on [`Event::Resume`], once what
    /// it waits for has landed.
    Paused,
    /// The next stack waits for job `slot`'s export to land.
    AwaitingExport { slot: usize },
    /// A turntable run waits for its renders' saves.
    AwaitingSaves,
    /// The turntable sequence is being written.
    Sequence,
    /// Waiting for saves and exports in flight.
    Closing,
    /// Ended, with how it went.
    Done(Summary),
}

/// What became of a run's turntable sequence.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SequenceOutcome {
    /// The run makes no turntable.
    #[default]
    NotAsked,
    /// The run makes one, but ended before writing it.
    NotWritten,
    /// How writing it ended.
    Ended(SequenceEnd),
}

/// How a run ends: [`End::Full`] until Cancel (`Canceled`) or the first
/// failed save or export (`Stopped`). The first word stands:
///
/// ```text
/// Full ──Cancel──▶ Canceled
/// Full ──a save or an export failed──▶ Stopped(why)
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum End {
    /// Every job is run.
    #[default]
    Full,
    Canceled,
    /// A save or export failed, and why, in sentences.
    Stopped(String),
}

/// How a run went, for its closing notice.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub end: End,
    pub total: usize,
    pub made: usize,
    pub failed: usize,
    /// Why the last failed stack failed, in sentences.
    pub last_failure: Option<String>,
    /// How its exports went, when the run exports.
    pub exports: Option<Exports>,
    pub sequence: SequenceOutcome,
    pub notes: Vec<String>,
    pub skipped: Vec<String>,
}

/// How a run's exports went, one per job with a render, by its last export.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Exports {
    pub written: usize,
    /// The labels of those that weren't written, in the run's order.
    pub failed: Vec<String>,
    /// Why the last of those wasn't, in sentences.
    pub last_failure: Option<String>,
}

impl Summary {
    /// The counts, then why the last stack failed: "Batch: 12 of 14 made ·
    /// 2 failed. …".
    pub fn line(&self, words: &Words) -> String {
        let counts = self.counts(words);
        match &self.last_failure {
            Some(why) => say!(words, "batch-reasons", short = counts, why = why),
            None => counts,
        }
    }

    /// "Batch: 12 of 14 made · 2 failed".
    fn counts(&self, w: &Words) -> String {
        let (made, total) = (self.made, self.total);
        let mut line = match self.end {
            End::Full => say!(w, "batch-made", made = made, total = total),
            End::Canceled => say!(w, "batch-canceled-made", made = made, total = total),
            End::Stopped(_) => say!(w, "batch-stopped-made", made = made, total = total),
        };
        if self.failed > 0 {
            line.push_str(SEPARATOR);
            line.push_str(&say!(w, "batch-failed", count = self.failed));
        }
        line
    }

    /// The closing notice's short parts ("12 exported to \"Out\""),
    /// naming `export_folder`.
    pub fn short_parts(&self, w: &Words, export_folder: Option<&std::path::Path>) -> Vec<String> {
        let mut parts = vec![self.counts(w)];
        if let Some(Exports { written: exported, failed, .. }) = &self.exports {
            let exported = *exported;
            match export_folder {
                Some(folder) => {
                    let folder = quoted(&file_label(folder), w);
                    parts.push(say!(w, "batch-exported-to", count = exported, folder = folder))
                }
                None => parts.push(say!(w, "batch-exported", count = exported)),
            }
            if !failed.is_empty() {
                parts.push(say!(w, "batch-not-exported", count = failed.len()));
            }
        }
        match &self.sequence {
            SequenceOutcome::Ended(SequenceEnd::Finished(line) | SequenceEnd::Canceled(line)) => {
                parts.push(line.clone())
            }
            SequenceOutcome::NotWritten if self.end != End::Canceled => {
                parts.push(say!(w, "batch-sequence-not-written"))
            }
            SequenceOutcome::Ended(SequenceEnd::Failed(_))
            | SequenceOutcome::NotWritten
            | SequenceOutcome::NotAsked => {}
        }
        parts.extend(self.notes.iter().cloned());
        parts.extend(
            self.skipped
                .iter()
                .map(|what| say!(w, "batch-left-out-of-sequence", stack = what.as_str())),
        );
        parts
    }

    /// Why things went wrong, each once: the last failed stack, what
    /// stopped the run, the last export, the sequence.
    pub fn reasons(&self) -> Vec<String> {
        let mut reasons: Vec<String> = self.last_failure.iter().cloned().collect();
        for why in self.reasons_besides_stacks() {
            if !reasons.contains(&why) {
                reasons.push(why);
            }
        }
        reasons
    }

    /// [`Summary::reasons`] without the stacks', for a front end that says
    /// why each stack failed on its own.
    pub fn reasons_besides_stacks(&self) -> Vec<String> {
        let stopped = match &self.end {
            End::Stopped(why) => Some(why.clone()),
            End::Full | End::Canceled => None,
        };
        let export = self.exports.iter().filter_map(|e| e.last_failure.clone());
        let sequence = match &self.sequence {
            SequenceOutcome::Ended(SequenceEnd::Failed(why)) => Some(why.clone()),
            _ => None,
        };
        let mut reasons: Vec<String> = Vec::new();
        for why in stopped.into_iter().chain(export).chain(sequence) {
            if !reasons.contains(&why) {
                reasons.push(why);
            }
        }
        reasons
    }

    /// The closing notice: the short parts joined by a middle dot, then the
    /// reasons as sentences.
    pub fn text(&self, words: &Words, export_folder: Option<&std::path::Path>) -> String {
        let short = self.short_parts(words, export_folder).join(SEPARATOR);
        let reasons = self.reasons();
        match reasons.is_empty() {
            true => short,
            false => {
                let why = crate::words::sentences(words, &reasons);
                say!(words, "batch-reasons", short = short, why = why)
            }
        }
    }

    /// Something went wrong: a stack or an export failed, or the sequence.
    pub fn problem(&self) -> bool {
        matches!(self.end, End::Stopped(_))
            || self.failed > 0
            || self.exports.as_ref().is_some_and(|e| !e.failed.is_empty())
            || matches!(self.sequence, SequenceOutcome::Ended(SequenceEnd::Failed(_)))
    }
}

/// "\"bee\" didn't finish.", then `error`'s sentences.
pub fn stack_failure(words: &Words, label: &str, error: &str) -> String {
    let lead = say!(words, "batch-stack-failed", stack = quoted(label, words));
    crate::words::sentences(words, [lead.as_str(), error])
}

/// "\"bee\" couldn't be exported.", then `error`'s sentences.
pub fn export_failure(words: &Words, label: &str, error: &str) -> String {
    let lead = say!(words, "batch-export-failed", stack = quoted(label, words));
    crate::words::sentences(words, [lead.as_str(), error])
}

/// Whether a render has been kept in the history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SaveState {
    Pending,
    Saved,
    Failed,
}

/// A render the run made.
#[derive(Clone, Debug)]
struct Made {
    stack: StackNo,
    paths: Vec<PathBuf>,
    shape: StackShape,
    save: SaveState,
}

/// A job the run started, and what came of it.
#[derive(Clone, Debug)]
struct Slot {
    job: Job,
    outcome: Outcome,
    /// Its render's export, when the run exports.
    export: SlotExport,
}

/// Where a job's export is. A restack's export writes over the replaced
/// render's file, so a job counts once, by its last export; a restack
/// starts only once every export has landed.
///
/// ```text
/// None ──render──▶ Writing ──landed──▶ Written | Failed
/// Written | Failed ──restack──▶ Writing (the restack's)
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
enum SlotExport {
    /// Not exported: the run doesn't export, or nothing was made.
    None,
    /// `stack`'s export is being written.
    Writing {
        stack: StackNo,
    },
    Written,
    /// The last export failed, and why.
    Failed(String),
}

/// What came of a job: `Nothing` → `Made` (or still `Nothing` if it
/// failed); `Made` → `Dropped` when the turntable leaves it out. A
/// restack keeps the variant.
#[derive(Clone, Debug)]
enum Outcome {
    Nothing,
    /// Its render, the restack's once one replaces it.
    Made(Made),
    /// Its render, left out of the sequence.
    Dropped(Made),
}

impl Outcome {
    /// Its render, in the sequence or not.
    fn made(&self) -> Option<&Made> {
        match self {
            Outcome::Nothing => None,
            Outcome::Made(made) | Outcome::Dropped(made) => Some(made),
        }
    }

    fn made_mut(&mut self) -> Option<&mut Made> {
        match self {
            Outcome::Nothing => None,
            Outcome::Made(made) | Outcome::Dropped(made) => Some(made),
        }
    }

    /// Its render, unless the sequence leaves it out.
    fn in_sequence(&self) -> Option<&Made> {
        match self {
            Outcome::Made(made) => Some(made),
            Outcome::Nothing | Outcome::Dropped(_) => None,
        }
    }

    /// Put `new` in place of the render, returning the one it replaces.
    fn replace(&mut self, new: Made) -> Option<Made> {
        match self {
            Outcome::Nothing => {
                *self = Outcome::Made(new);
                None
            }
            Outcome::Made(made) | Outcome::Dropped(made) => Some(std::mem::replace(made, new)),
        }
    }

    /// Leave the render out of the sequence.
    fn drop_out(&mut self) {
        *self = match std::mem::replace(self, Outcome::Nothing) {
            Outcome::Made(made) => Outcome::Dropped(made),
            other => other,
        };
    }
}

/// How far a turntable run is past its stacks.
#[derive(Clone, Debug)]
enum Check {
    /// Its stacks are still being made.
    Pending,
    /// Restacking the flagged ones, numbered among `slots` (those kept
    /// when the check started).
    Fixing { fix: StackFix, slots: Vec<usize> },
    /// Checked; the sequence is next.
    Checked,
    /// The sequence was written, or couldn't be.
    Written(SequenceEnd),
}

/// A batch run (see the module).
///
/// Lifecycle: its [`Phase`] (ready, paused before Start, stacking,
/// paused, awaiting an export, awaiting saves, the sequence, closing,
/// done), changed only by [`Machine::on`] and [`Machine::stacked`]
/// (ADR-144); a stack stopped by a pause is `Stop::Pause(Again)` in its
/// `Stacking`. Driven by `deptheos_gpui/src/app/batch_run.rs`,
/// `deptheos_cli/src/drive.rs` and `deptheos_ffi/src/batch.rs`.
#[derive(Clone, Debug)]
pub struct Machine {
    options: Options,
    phase: Phase,
    jobs: Vec<Job>,
    /// The jobs started, in order: `jobs[..slots.len()]`, less any put back.
    slots: Vec<Slot>,
    /// The first job not yet started.
    next_job: usize,
    next_stack: u32,
    made: usize,
    failed: usize,
    last_failure: Option<String>,
    end: End,
    /// None for a run without a turntable.
    check: Option<Check>,
    /// (replaced, restack): deleted once the restack is saved.
    replaced: Vec<(StackNo, StackNo)>,
    skipped: Vec<ManifestGap>,
    notes: Vec<String>,
    /// The language of its sentences.
    words: Words,
}

impl Machine {
    /// A run of `jobs`, its sentences worded in `words`' language.
    pub fn new(jobs: Vec<Job>, options: Options, words: Words) -> Self {
        let check = options.turntable.map(|_| Check::Pending);
        Self {
            options,
            phase: Phase::Ready,
            jobs,
            slots: Vec::new(),
            next_job: 0,
            next_stack: 0,
            made: 0,
            failed: 0,
            last_failure: None,
            end: End::Full,
            check,
            replaced: Vec::new(),
            skipped: Vec::new(),
            notes: Vec::new(),
            words,
        }
    }

    pub fn phase(&self) -> &Phase {
        &self.phase
    }

    /// The language its sentences are worded in.
    pub fn words(&self) -> &Words {
        &self.words
    }

    /// Done: nothing runs and nothing will.
    pub(crate) fn is_done(&self) -> bool {
        matches!(self.phase, Phase::Done(_))
    }

    /// Exports are still being written.
    pub fn is_exporting(&self) -> bool {
        self.exporting().is_some()
    }

    /// The job whose export is being written: one at most.
    fn exporting(&self) -> Option<usize> {
        self.slots.iter().position(|s| matches!(s.export, SlotExport::Writing { .. }))
    }

    /// Canceled, or stopped by a failed save or export: it starts
    /// nothing more.
    pub fn is_ending(&self) -> bool {
        self.end != End::Full
    }

    /// Jobs made or failed.
    pub fn ended(&self) -> usize {
        self.made + self.failed
    }

    /// The job running, or whose restack runs.
    pub(crate) fn current(&self) -> Option<&Job> {
        match &self.phase {
            Phase::Stacking { slot, .. } => Some(&self.slots[*slot].job),
            _ => None,
        }
    }

    /// The stack running and which it is.
    pub fn running(&self) -> Option<(StackNo, StackKind)> {
        match &self.phase {
            Phase::Stacking { stack, kind, .. } => Some((*stack, *kind)),
            _ => None,
        }
    }

    /// The files the running stack's render records (a restack: the
    /// replaced render's); empty when none runs.
    fn listed(&self) -> &[PathBuf] {
        match &self.phase {
            Phase::Stacking { slot, kind: StackKind::Job, .. } => &self.slots[*slot].job.frames,
            Phase::Stacking { slot, kind: StackKind::Restack, .. } => {
                self.slots[*slot].outcome.made().map_or(&[], |made| &made.paths)
            }
            _ => &[],
        }
    }

    /// The whole run's progress, given `current` (0 to 1) of the running
    /// stack.
    pub fn fraction(&self, current: f64) -> f64 {
        let total = self.jobs.len();
        if total == 0 {
            return 0.0;
        }
        let current = match &self.phase {
            Phase::Stacking { kind: StackKind::Job, .. } => current.clamp(0.0, 1.0),
            Phase::Stacking { kind: StackKind::Restack, .. }
            | Phase::Ready
            | Phase::ReadyPaused
            | Phase::Paused
            | Phase::AwaitingExport { .. } => 0.0,
            _ => return 1.0,
        };
        ((self.ended() as f64 + current) / total as f64).min(1.0)
    }

    /// "Batch 2 of 14 · P1010217–219".
    fn progress(&self) -> String {
        let (n, total) = (self.next_job, self.jobs.len());
        let line = say!(self.words, "batch-progress", n = n, total = total);
        match self.current() {
            Some(job) => format!("{line}{SEPARATOR}{}", job.label),
            None => line,
        }
    }

    /// What the run is doing, for its progress line.
    pub fn status(&self) -> String {
        match &self.phase {
            Phase::Ready => self.progress(),
            Phase::Stacking { slot, kind: StackKind::Restack, .. } => {
                fixing_text(&self.words, &self.slots[*slot].job.label)
            }
            Phase::Stacking { .. } => self.progress(),
            Phase::ReadyPaused | Phase::Paused => {
                format!("{}{SEPARATOR}{}", self.progress(), say!(self.words, "batch-paused"))
            }
            Phase::AwaitingExport { slot } => {
                let stack = quoted(&self.slots[*slot].job.label, &self.words);
                let exporting = say!(self.words, "batch-exporting-stack", stack = stack);
                format!("{}{SEPARATOR}{exporting}", self.progress())
            }
            Phase::AwaitingSaves => say!(self.words, "batch-saving"),
            Phase::Sequence => say!(self.words, "batch-writing-sequence"),
            Phase::Closing if !self.is_exporting() => say!(self.words, "batch-saving"),
            Phase::Closing => {
                let landed =
                    |e: &SlotExport| matches!(e, SlotExport::Written | SlotExport::Failed(_));
                let written = self.slots.iter().filter(|s| landed(&s.export)).count();
                let total = self.slots.iter().filter(|s| s.export != SlotExport::None).count();
                say!(self.words, "batch-exporting", written = written, total = total)
            }
            Phase::Done(summary) => summary.line(&self.words),
        }
    }

    /// The running stack made its render, of `paths`, with `shape` for the
    /// turntable check. The only way to report a render, so a save is
    /// always of the stack the front end holds.
    pub fn stacked(&mut self, paths: Vec<PathBuf>, shape: StackShape) -> Stacked {
        let before = std::mem::discriminant(&self.phase);
        tracing::debug!(phase = ?PhaseName(&self.phase), frames = paths.len(), "batch stacked");
        // Read while it still runs.
        let listed = self.listed().to_vec();
        let job = match self.phase {
            Phase::Stacking { slot, kind: StackKind::Job, .. } => Some(JobNo(slot)),
            _ => None,
        };
        let mut then = Vec::new();
        let save = self.made(paths, shape, &mut then).map(|stack| Save { stack, job, listed });
        self.log_phase(before);
        Stacked { save, then }
    }

    fn log_phase(&self, before: std::mem::Discriminant<Phase>) {
        if std::mem::discriminant(&self.phase) != before {
            tracing::debug!(phase = ?PhaseName(&self.phase), "batch phase");
        }
    }

    /// Take in `event`; what to do about it.
    pub fn on(&mut self, event: Event) -> Vec<Action> {
        let mut out = Vec::new();
        let before = std::mem::discriminant(&self.phase);
        tracing::debug!(phase = ?PhaseName(&self.phase), ?event, "batch event");
        match event {
            Event::Start => {
                let paused = self.phase == Phase::ReadyPaused;
                if paused || self.phase == Phase::Ready {
                    if self.options.save {
                        let why = say!(self.words, "batch-not-stacked-yet");
                        out.push(Action::RecordUnstacked { jobs: self.jobs.clone(), why });
                    }
                    match paused {
                        true => self.phase = Phase::Paused,
                        false => self.advance(&mut out),
                    }
                }
            }
            Event::StackFailed(error) => self.stack_ended(Some(error), &mut out),
            Event::StackStopped => self.stack_ended(None, &mut out),
            Event::Saved(stack) => self.saved(stack, Ok(()), &mut out),
            Event::SaveFailed { stack, error } => self.saved(stack, Err(error), &mut out),
            Event::Exported { stack, result } => self.exported(stack, result, &mut out),
            Event::SequenceEnded(end) => {
                if self.phase == Phase::Sequence {
                    self.check = Some(Check::Written(end));
                    self.advance(&mut out);
                }
            }
            Event::Cancel => self.halt(End::Canceled, &mut out),
            Event::Pause => self.pause(),
            Event::Resume => match &mut self.phase {
                Phase::Stacking { stop: Some(Stop::Pause(again)), .. } => *again = Again::AtOnce,
                Phase::ReadyPaused => self.phase = Phase::Ready,
                Phase::Paused => self.advance(&mut out),
                Phase::Ready
                | Phase::Stacking { .. }
                | Phase::AwaitingExport { .. }
                | Phase::AwaitingSaves
                | Phase::Sequence
                | Phase::Closing
                | Phase::Done(_) => {}
            },
        }
        self.log_phase(before);
        out
    }

    /// Hold the run before its next step, whatever the phase.
    fn pause(&mut self) {
        match &mut self.phase {
            // Re-paused before the stack returned: it waits again.
            Phase::Stacking { stop: stop @ (None | Some(Stop::Pause(_))), .. } => {
                *stop = Some(Stop::Pause(Again::OnResume))
            }
            Phase::Ready => self.phase = Phase::ReadyPaused,
            // What is awaited lands while paused; the next step waits.
            Phase::AwaitingExport { .. } | Phase::AwaitingSaves => self.phase = Phase::Paused,
            // Stopping, already held, or nothing starts after it.
            Phase::Stacking { stop: Some(Stop::Cancel), .. }
            | Phase::ReadyPaused
            | Phase::Paused
            | Phase::Sequence
            | Phase::Closing
            | Phase::Done(_) => {}
        }
    }

    /// Stop what runs and start nothing more; the first `end` stands.
    fn halt(&mut self, end: End, out: &mut Vec<Action>) {
        if self.is_done() || self.is_ending() {
            return;
        }
        tracing::debug!(?end, "batch ends early");
        self.end = end;
        match &mut self.phase {
            Phase::Stacking { stop, .. } => {
                *stop = Some(Stop::Cancel);
                out.push(Action::CancelStack);
            }
            Phase::Sequence => out.push(Action::CancelSequence),
            // Nothing runs: on to the saves and exports in flight.
            Phase::Ready
            | Phase::ReadyPaused
            | Phase::Paused
            | Phase::AwaitingExport { .. }
            | Phase::AwaitingSaves => self.advance(out),
            Phase::Closing | Phase::Done(_) => {}
        }
    }

    fn new_stack(&mut self) -> StackNo {
        self.next_stack += 1;
        StackNo(self.next_stack)
    }

    /// Nothing runs: once the export has landed, start what is next or
    /// close the run.
    fn advance(&mut self, out: &mut Vec<Action>) {
        if !self.is_ending() {
            if let Some(slot) = self.exporting() {
                self.phase = Phase::AwaitingExport { slot };
                return;
            }
            if self.next_job < self.jobs.len() {
                let job = self.jobs[self.next_job].clone();
                self.next_job += 1;
                let stack = self.new_stack();
                let slot = self.slots.len();
                self.slots.push(Slot {
                    job: job.clone(),
                    outcome: Outcome::Nothing,
                    export: SlotExport::None,
                });
                self.phase = Phase::Stacking { stack, slot, kind: StackKind::Job, stop: None };
                let listed = job.frames.clone();
                out.push(Action::Stack { stack, label: job.label, listed, frames: job.frames });
                return;
            }
            if self.turntable_next(out) {
                return;
            }
        }
        if self.saves_pending() || self.is_exporting() {
            self.phase = Phase::Closing;
            return;
        }
        self.finish(out);
    }

    /// The turntable's next step once every job has ended; `true` when it
    /// started something or waits.
    fn turntable_next(&mut self, out: &mut Vec<Action>) -> bool {
        loop {
            let pending = self.saves_pending();
            let Some(check) = self.check.as_mut() else { return false };
            match check {
                Check::Pending | Check::Checked if pending => {
                    self.phase = Phase::AwaitingSaves;
                    return true;
                }
                Check::Pending => {
                    let fix = self.options.turntable.is_some_and(|t| t.fix);
                    if !fix {
                        *check = Check::Checked;
                        continue;
                    }
                    // Only the stacks that made it into the history count.
                    let slots: Vec<usize> = (0..self.slots.len())
                        .filter(|&i| {
                            self.slots[i].outcome.made().is_some_and(|m| m.save == SaveState::Saved)
                        })
                        .collect();
                    let fix = StackFix::new(slots.iter().map(|&i| {
                        let made = self.slots[i].outcome.made().expect("kept");
                        (made.shape, made.paths.len())
                    }));
                    self.check = Some(Check::Fixing { fix, slots });
                }
                Check::Fixing { fix, slots } => {
                    let Some(i) = fix.next_to_fix() else {
                        *check = Check::Checked;
                        continue;
                    };
                    let slot = slots[i];
                    let paths = self.slots[slot].outcome.made().expect("kept").paths.clone();
                    let stack = self.new_stack();
                    self.phase =
                        Phase::Stacking { stack, slot, kind: StackKind::Restack, stop: None };
                    out.push(Action::Stack {
                        stack,
                        label: self.slots[slot].job.label.clone(),
                        frames: paths[1..].to_vec(),
                        listed: paths,
                    });
                    return true;
                }
                Check::Checked => {
                    let stacks: Vec<SequenceStack> = self
                        .slots
                        .iter()
                        .filter_map(|s| {
                            let made =
                                s.outcome.in_sequence().filter(|m| m.save == SaveState::Saved)?;
                            Some(SequenceStack {
                                stack: made.stack,
                                label: s.job.label.clone(),
                                paths: made.paths.clone(),
                            })
                        })
                        .collect();
                    if stacks.is_empty() {
                        let why = say!(self.words, "batch-no-stack-for-sequence");
                        *check = Check::Written(SequenceEnd::Failed(why));
                        return false;
                    }
                    self.phase = Phase::Sequence;
                    out.push(Action::WriteSequence {
                        stacks,
                        skipped: self.skipped.clone(),
                        notes: self.notes.clone(),
                    });
                    return true;
                }
                Check::Written(_) => return false,
            }
        }
    }

    /// The turntable check under way, and where `slot` is among its stacks.
    fn fixing(&mut self, slot: usize) -> Option<(&mut StackFix, usize)> {
        let Some(Check::Fixing { fix, slots }) = self.check.as_mut() else { return None };
        let i = slots.iter().position(|&s| s == slot)?;
        Some((fix, i))
    }

    fn saves_pending(&self) -> bool {
        self.slots.iter().any(|s| s.outcome.made().is_some_and(|m| m.save == SaveState::Pending))
    }

    /// The running stack made a render: the stack to save it as, if any.
    fn made(
        &mut self,
        paths: Vec<PathBuf>,
        shape: StackShape,
        out: &mut Vec<Action>,
    ) -> Option<StackNo> {
        let &Phase::Stacking { stack, slot, kind, stop } = &self.phase else { return None };
        let save = if self.options.save { SaveState::Pending } else { SaveState::Saved };
        let label = self.slots[slot].job.label.clone();
        if kind == StackKind::Restack {
            // A canceled run writes no sequence: the restack isn't needed.
            if stop == Some(Stop::Cancel) {
                self.after_stack(stop, out);
                return None;
            }
            let words = self.words.clone();
            let (fix, i) = self.fixing(slot)?;
            let mut saved = None;
            match fix.judge(&words, i, &label, Ok(shape)) {
                Fixed::Kept { note } => {
                    let old = self.slots[slot].outcome.replace(Made { stack, paths, shape, save });
                    let old = old.expect("a restack replaces a render").stack;
                    self.replaced.push((old, stack));
                    self.notes.push(note);
                    saved = self.keep(slot, stack, Some(old), out);
                }
                // Never saved: the first render stays, out of the sequence.
                Fixed::Dropped(gap) => {
                    self.slots[slot].outcome.drop_out();
                    self.skipped.push(gap);
                }
            }
            self.after_stack(stop, out);
            saved
        } else {
            self.made += 1;
            self.slots[slot].outcome = Outcome::Made(Made { stack, paths, shape, save });
            let saved = self.keep(slot, stack, None, out);
            self.after_stack(stop, out);
            saved
        }
    }

    /// Export job `slot`'s new render `stack` (over `replaces` for a
    /// restack); the stack to save it as, with a history.
    fn keep(
        &mut self,
        slot: usize,
        stack: StackNo,
        replaces: Option<StackNo>,
        out: &mut Vec<Action>,
    ) -> Option<StackNo> {
        let saved = self.options.save.then_some(stack);
        if !self.options.export {
            return saved;
        }
        let Slot { job, export, .. } = &mut self.slots[slot];
        // A stack starts only once the last export has landed.
        debug_assert!(!matches!(export, SlotExport::Writing { .. }), "two exports at once");
        *export = SlotExport::Writing { stack };
        out.push(Action::Export { stack, label: job.label.clone(), replaces });
        saved
    }

    /// The stack running ended without a render: failed (`Some(error)`)
    /// or stopped (`None`).
    fn stack_ended(&mut self, error: Option<String>, out: &mut Vec<Action>) {
        let &Phase::Stacking { slot, kind, stop, .. } = &self.phase else { return };
        if let Some(Stop::Pause(_)) = stop {
            // Stopped by the pause, whatever it says: it runs again.
            if kind == StackKind::Restack {
                if let Some((fix, i)) = self.fixing(slot) {
                    fix.again(i);
                }
            } else {
                self.slots.pop();
                self.next_job -= 1;
            }
            return self.after_stack(stop, out);
        }
        let label = self.slots[slot].job.label.clone();
        match (kind, error) {
            (StackKind::Job, Some(error)) => {
                self.failed += 1;
                self.last_failure = Some(stack_failure(&self.words, &label, &error));
                if self.check.is_some() {
                    self.skipped.push(ManifestGap { what: label, reason: error.clone() });
                }
                let job = self.slots[slot].job.clone();
                out.push(Action::RecordFailure { no: JobNo(slot), job, error });
            }
            (StackKind::Restack, Some(error)) if stop.is_none() => {
                let words = self.words.clone();
                if let Some((fix, i)) = self.fixing(slot)
                    && let Fixed::Dropped(gap) = fix.judge(&words, i, &label, Err(&error))
                {
                    self.skipped.push(gap);
                }
                self.slots[slot].outcome.drop_out();
            }
            // Canceled: the run ends, as canceled unless a failure stopped
            // it first.
            _ => {
                if !self.is_ending() {
                    self.end = End::Canceled;
                }
            }
        }
        self.after_stack(stop, out);
    }

    /// A stack returned: go on, or wait to be resumed.
    fn after_stack(&mut self, stop: Option<Stop>, out: &mut Vec<Action>) {
        if stop == Some(Stop::Pause(Again::OnResume)) && !self.is_ending() {
            self.phase = Phase::Paused;
            return;
        }
        self.advance(out);
    }

    fn made_mut(&mut self, stack: StackNo) -> Option<(usize, &mut Made)> {
        self.slots
            .iter_mut()
            .enumerate()
            .find_map(|(i, s)| s.outcome.made_mut().filter(|m| m.stack == stack).map(|m| (i, m)))
    }

    fn saved(&mut self, stack: StackNo, result: Result<(), String>, out: &mut Vec<Action>) {
        let restack = self.replaced.iter().any(|&(_, new)| new == stack);
        let Some((slot, made)) = self.made_mut(stack) else { return };
        if made.save != SaveState::Pending {
            return;
        }
        match result {
            Ok(()) => made.save = SaveState::Saved,
            Err(error) => {
                made.save = SaveState::Failed;
                let label = self.slots[slot].job.label.clone();
                let render =
                    say!(self.words, "batch-render-unsaved", stack = quoted(&label, &self.words));
                let failure = crate::words::sentences(&self.words, [render, error.to_string()]);
                if restack {
                    // Left out; the render it was to replace stays.
                    self.slots[slot].outcome.drop_out();
                    self.replaced.retain(|&(_, new)| new != stack);
                    let lead = say!(self.words, "gap-restack-unsaved");
                    let reason = crate::words::sentences(&self.words, [lead, error.to_string()]);
                    self.skipped.push(ManifestGap { what: label, reason });
                } else {
                    self.made -= 1;
                    self.failed += 1;
                    self.last_failure = Some(failure.clone());
                    // Recorded as failed, for Show Failed and Try Again.
                    let lead = say!(self.words, "batch-failure-render-unsaved");
                    let why = crate::words::sentences(&self.words, [lead, error.to_string()]);
                    out.push(Action::RecordFailure {
                        no: JobNo(slot),
                        job: self.slots[slot].job.clone(),
                        error: why,
                    });
                    if self.check.is_some() {
                        let lead = say!(self.words, "gap-render-unsaved");
                        let reason =
                            crate::words::sentences(&self.words, [lead, error.to_string()]);
                        self.skipped.push(ManifestGap { what: label, reason });
                    }
                }
                // The first that fails ends the run.
                self.halt(End::Stopped(failure), out);
            }
        }
        self.waited(out);
    }

    fn exported(&mut self, stack: StackNo, result: Result<(), String>, out: &mut Vec<Action>) {
        let Some(Slot { job, export, .. }) =
            self.slots.iter_mut().find(|s| s.export == SlotExport::Writing { stack })
        else {
            return;
        };
        let mut failed = None;
        *export = match result {
            Ok(()) => SlotExport::Written,
            Err(error) => {
                failed = Some(export_failure(&self.words, &job.label, &error));
                SlotExport::Failed(error)
            }
        };
        // The first that fails ends the run.
        if let Some(why) = failed {
            self.halt(End::Stopped(why), out);
        }
        self.waited(out);
    }

    /// Something waited for landed: go on if nothing else is awaited.
    fn waited(&mut self, out: &mut Vec<Action>) {
        if matches!(
            self.phase,
            Phase::AwaitingExport { .. } | Phase::AwaitingSaves | Phase::Closing
        ) {
            self.advance(out);
        }
    }

    /// The jobs whose last export failed, in order: label and why.
    fn export_failures(&self) -> impl Iterator<Item = (&str, &str)> {
        self.slots.iter().filter_map(|s| match &s.export {
            SlotExport::Failed(error) => Some((s.job.label.as_str(), error.as_str())),
            _ => None,
        })
    }

    fn finish(&mut self, out: &mut Vec<Action>) {
        // Each replaced render goes once its restack is kept.
        let delete: Vec<StackNo> = self
            .replaced
            .iter()
            .filter(|&&(_, new)| {
                self.slots.iter().any(|s| {
                    s.outcome.made().is_some_and(|m| m.stack == new && m.save == SaveState::Saved)
                })
            })
            .map(|&(old, _)| old)
            .collect();
        if !delete.is_empty() {
            out.push(Action::Delete(delete));
        }
        let summary = Summary {
            end: self.end.clone(),
            total: self.jobs.len(),
            made: self.made,
            failed: self.failed,
            last_failure: self.last_failure.clone(),
            exports: self.options.export.then(|| Exports {
                written: self.slots.iter().filter(|s| s.export == SlotExport::Written).count(),
                failed: self.export_failures().map(|(label, _)| label.to_string()).collect(),
                last_failure: self
                    .export_failures()
                    .last()
                    .map(|(label, error)| export_failure(&self.words, label, error)),
            }),
            sequence: match &self.check {
                None => SequenceOutcome::NotAsked,
                Some(Check::Written(end)) => SequenceOutcome::Ended(end.clone()),
                Some(_) => SequenceOutcome::NotWritten,
            },
            notes: self.notes.clone(),
            skipped: self.skipped.iter().map(|g| g.what.clone()).collect(),
        };
        self.phase = Phase::Done(summary.clone());
        out.push(Action::Finish(summary));
    }
}

/// The phase's name alone, for the log.
struct PhaseName<'a>(&'a Phase);

impl std::fmt::Debug for PhaseName<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self.0 {
            Phase::Ready => "Ready",
            Phase::ReadyPaused => "ReadyPaused",
            Phase::Stacking { kind: StackKind::Job, .. } => "Stacking",
            Phase::Stacking { kind: StackKind::Restack, .. } => "Restacking",
            Phase::Paused => "Paused",
            Phase::AwaitingExport { .. } => "AwaitingExport",
            Phase::AwaitingSaves => "AwaitingSaves",
            Phase::Sequence => "Sequence",
            Phase::Closing => "Closing",
            Phase::Done(_) => "Done",
        };
        f.write_str(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::words::explain::Platform;

    fn words() -> Words {
        Words::english(Platform::Desktop)
    }

    fn jobs(labels: &[&str]) -> Vec<Job> {
        labels
            .iter()
            .map(|l| Job {
                label: l.to_string(),
                frames: (1..=3).map(|i| PathBuf::from(format!("/{l}/{i}.png"))).collect(),
            })
            .collect()
    }

    fn shape(reversed: bool) -> StackShape {
        StackShape { reversed, first_step: None }
    }

    /// The running stack made a render of all its frames: the stack it is
    /// saved as, if any, and what follows.
    fn stacked(m: &mut Machine) -> (Option<StackNo>, Vec<Action>) {
        let paths = m.current().unwrap().frames.clone();
        let made = m.stacked(paths, shape(false));
        (made.save.map(|s| s.stack), made.then)
    }

    /// The labels of the stacks started, in `actions`.
    fn started(actions: &[Action]) -> Vec<&str> {
        actions
            .iter()
            .filter_map(|a| match a {
                Action::Stack { label, .. } => Some(label.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Every part of the closing notice, the short parts and the reasons.
    fn parts(summary: &Summary) -> Vec<String> {
        let mut parts = summary.short_parts(&words(), None);
        parts.extend(summary.reasons());
        parts
    }

    fn summary(actions: &[Action]) -> &Summary {
        actions
            .iter()
            .find_map(|a| match a {
                Action::Finish(s) => Some(s),
                _ => None,
            })
            .expect("the run finished")
    }

    const SAVE: Options = Options { save: true, export: false, turntable: None };

    #[test]
    fn stacks_one_at_a_time_and_counts() {
        let mut m = Machine::new(jobs(&["a", "b", "c"]), SAVE, words());
        assert_eq!(m.status(), "Batch 0 of 3");
        let out = m.on(Event::Start);
        // Every job is recorded first, hidden until it fails or the run ends.
        assert_eq!(
            out[0],
            Action::RecordUnstacked {
                jobs: jobs(&["a", "b", "c"]),
                why: "Not stacked yet: the batch ended before it got to this stack.".into()
            }
        );
        assert_eq!(started(&out), ["a"]);
        assert_eq!(m.status(), "Batch 1 of 3 · a");
        assert_eq!(m.fraction(0.5), 0.5 / 3.0);
        // Starting again starts nothing more.
        assert!(m.on(Event::Start).is_empty());
        let a = m.running().unwrap().0;
        let made = m.stacked(m.current().unwrap().frames.clone(), shape(false));
        let save = made.save.unwrap();
        assert_eq!((save.stack, save.job), (a, Some(JobNo(0))), "into a's record's stack");
        assert_eq!(started(&made.then), ["b"]);
        let out = m.on(Event::StackFailed("The GPU ran out of memory.".into()));
        assert!(matches!(
            &out[0],
            Action::RecordFailure { no: JobNo(1), job, .. } if job.label == "b"
        ));
        assert_eq!(started(&out), ["c"]);
        m.on(Event::Saved(a));
        let c = m.running().unwrap().0;
        assert_eq!(stacked(&mut m), (Some(c), vec![]));
        // The last save is still out.
        assert_eq!(m.phase(), &Phase::Closing);
        assert_eq!(m.fraction(0.0), 1.0);
        let out = m.on(Event::Saved(c));
        assert_eq!(
            summary(&out).line(&words()),
            "Batch: 2 of 3 made · 1 failed. \"b\" didn't finish. The GPU ran out of memory."
        );
        assert!(m.is_done());
        // Late events change nothing.
        assert!(m.on(Event::StackFailed("x".into())).is_empty());
        assert!(m.on(Event::Cancel).is_empty());
    }

    #[test]
    fn cancel_stops_the_stack_and_keeps_what_is_made() {
        let mut m = Machine::new(jobs(&["a", "b"]), SAVE, words());
        m.on(Event::Start);
        assert_eq!(m.on(Event::Cancel), [Action::CancelStack]);
        // It finished before it saw the cancel: kept, counted, saved.
        let a = m.running().unwrap().0;
        assert_eq!(stacked(&mut m), (Some(a), vec![]), "nothing more starts");
        assert_eq!(m.phase(), &Phase::Closing, "the save is waited for");
        let out = m.on(Event::Saved(a));
        assert_eq!(summary(&out).line(&words()), "Batch canceled: 1 of 2 made");
        assert!(!summary(&out).problem());

        let mut m = Machine::new(jobs(&["a", "b"]), SAVE, words());
        m.on(Event::Start);
        m.on(Event::Cancel);
        let out = m.on(Event::StackStopped);
        assert_eq!(summary(&out).line(&words()), "Batch canceled: 0 of 2 made");
    }

    /// A failed save is recorded and ends the run, stopping the next stack.
    #[test]
    fn a_failed_save_counts_as_failed_and_ends_the_run() {
        let mut m = Machine::new(jobs(&["a", "b", "c"]), SAVE, words());
        m.on(Event::Start);
        let a = m.running().unwrap().0;
        let (_, out) = stacked(&mut m);
        assert_eq!(started(&out), ["b"], "saves are waited for alongside the next stack");
        let out = m.on(Event::SaveFailed { stack: a, error: "The disk is full.".into() });
        assert_eq!(
            out,
            [
                // Recorded, to find with Show Failed and try again.
                Action::RecordFailure {
                    no: JobNo(0),
                    job: jobs(&["a"]).remove(0),
                    error: "Its render couldn't be saved. The disk is full.".into()
                },
                Action::CancelStack
            ]
        );
        assert!(m.is_ending());
        assert!(m.on(Event::Cancel).is_empty(), "Cancel changes nothing more");
        let out = m.on(Event::StackStopped);
        assert!(started(&out).is_empty(), "c never starts");
        let summary = summary(&out);
        let why = "The render of \"a\" couldn't be saved. The disk is full.";
        assert_eq!(summary.end, End::Stopped(why.into()));
        assert_eq!(summary.line(&words()), format!("Batch stopped: 0 of 3 made · 1 failed. {why}"));
        assert_eq!(summary.text(&words(), None), summary.line(&words()), "one sentence, said once");
        assert_eq!(summary.reasons_besides_stacks(), [why], "said even where stacks are not");
        assert!(summary.problem());
    }

    const EXPORT: Options = Options { save: true, export: true, turntable: None };

    /// The next stack waits for the last export, not for its save.
    #[test]
    fn the_next_stack_waits_for_the_last_export() {
        let mut m = Machine::new(jobs(&["a", "b"]), EXPORT, words());
        m.on(Event::Start);
        let a = m.running().unwrap().0;
        let (saved, out) = stacked(&mut m);
        assert_eq!(saved, Some(a));
        assert_eq!(out, [Action::Export { stack: a, label: "a".into(), replaces: None }]);
        assert_eq!(m.phase(), &Phase::AwaitingExport { slot: 0 });
        assert!(m.running().is_none());
        assert_eq!(m.status(), "Batch 1 of 2 · Exporting \"a\"");
        assert_eq!(m.fraction(0.7), 0.5);
        assert!(m.on(Event::Saved(a)).is_empty(), "the save doesn't start the next");
        let out = m.on(Event::Exported { stack: a, result: Ok(()) });
        assert_eq!(started(&out), ["b"]);
        // Its save still out: the next starts all the same.
        let b = m.running().unwrap().0;
        let _ = stacked(&mut m);
        let out = m.on(Event::Exported { stack: b, result: Ok(()) });
        assert!(out.is_empty());
        assert_eq!(m.phase(), &Phase::Closing);
        let out = m.on(Event::Saved(b));
        assert_eq!(summary(&out).exports, Some(Exports { written: 2, ..Exports::default() }));
    }

    /// A failed export ends the run and says why; the save out is awaited.
    #[test]
    fn a_failed_export_ends_the_run() {
        let mut m = Machine::new(jobs(&["a", "b", "c"]), EXPORT, words());
        m.on(Event::Start);
        let a = m.running().unwrap().0;
        let _ = stacked(&mut m);
        m.on(Event::Saved(a));
        m.on(Event::Exported { stack: a, result: Ok(()) });
        let b = m.running().unwrap().0;
        let (saved, out) = stacked(&mut m);
        assert_eq!(saved, Some(b));
        assert_eq!(out, [Action::Export { stack: b, label: "b".into(), replaces: None }]);
        let out = m.on(Event::Exported { stack: b, result: Err("The disk is full.".into()) });
        assert!(out.is_empty(), "nothing runs to stop, and c never starts");
        assert_eq!(m.phase(), &Phase::Closing);
        let out = m.on(Event::Saved(b));
        let summary = summary(&out);
        let why = "\"b\" couldn't be exported. The disk is full.";
        assert_eq!(summary.end, End::Stopped(why.into()));
        assert_eq!(
            summary.text(&words(), None),
            format!("Batch stopped: 2 of 3 made · 1 exported · 1 not exported. {why}")
        );
    }

    #[test]
    fn exports_in_flight_hold_the_run_open_even_canceled() {
        let mut m = Machine::new(jobs(&["a", "b"]), EXPORT, words());
        m.on(Event::Start);
        let a = m.running().unwrap().0;
        let (saved, out) = stacked(&mut m);
        assert_eq!(saved, Some(a));
        assert_eq!(out, [Action::Export { stack: a, label: "a".into(), replaces: None }]);
        m.on(Event::Saved(a));
        assert!(m.on(Event::Cancel).is_empty(), "no stack runs to stop");
        assert_eq!(m.phase(), &Phase::Closing);
        assert!(m.is_exporting());
        assert_eq!(m.status(), "Exporting · 0 of 1");
        assert!(m.on(Event::Cancel).is_empty(), "a second cancel doesn't close it");
        let out = m.on(Event::Exported { stack: a, result: Ok(()) });
        let summary = summary(&out);
        assert_eq!(summary.exports, Some(Exports { written: 1, ..Exports::default() }));
        assert_eq!(
            summary.short_parts(&words(), Some(std::path::Path::new("/x/Out"))),
            ["Batch canceled: 1 of 2 made", "1 exported to \"Out\""]
        );
        assert!(summary.reasons().is_empty());
    }

    #[test]
    fn a_pause_decides_when_it_happens() {
        let mut m = Machine::new(jobs(&["a", "b"]), SAVE, words());
        m.on(Event::Start);
        let first = m.running().unwrap().0;
        // Background: a GPU error. It runs again once resumed, uncounted.
        m.on(Event::Pause);
        assert!(m.on(Event::StackFailed("device lost".into())).is_empty());
        assert_eq!(m.phase(), &Phase::Paused);
        assert_eq!(m.status(), "Batch 0 of 2 · paused");
        let out = m.on(Event::Resume);
        assert_eq!(started(&out), ["a"]);
        assert_ne!(m.running().unwrap().0, first, "a new stack");
        // Resumed before the stack returned: it runs again at once.
        m.on(Event::Pause);
        m.on(Event::Resume);
        let out = m.on(Event::StackFailed("device lost".into()));
        assert_eq!(started(&out), ["a"]);
        // Re-paused before the stack returned: nothing starts.
        m.on(Event::Pause);
        m.on(Event::Resume);
        m.on(Event::Pause);
        assert!(m.on(Event::StackFailed("device lost".into())).is_empty());
        assert_eq!(m.phase(), &Phase::Paused);
        assert_eq!(started(&m.on(Event::Resume)), ["a"]);
        // A stack that made its render while paused is kept.
        m.on(Event::Pause);
        let a = m.running().unwrap().0;
        assert_eq!(stacked(&mut m), (Some(a), vec![]));
        assert_eq!(m.phase(), &Phase::Paused);
        // Cancel while paused closes the run.
        m.on(Event::Saved(a));
        let out = m.on(Event::Cancel);
        assert_eq!(summary(&out).line(&words()), "Batch canceled: 1 of 2 made");
        // A failure with no pause is a failure.
        let mut m = Machine::new(jobs(&["a"]), SAVE, words());
        m.on(Event::Start);
        let out = m.on(Event::StackFailed("out of memory".into()));
        assert!(matches!(out[0], Action::RecordFailure { .. }));
        assert_eq!(summary(&out).failed, 1);
    }

    /// Each action in short: "stack a", "export a", "record", ….
    fn steps(actions: &[Action]) -> Vec<String> {
        actions
            .iter()
            .map(|a| match a {
                Action::Stack { label, .. } => format!("stack {label}"),
                Action::Export { label, .. } => format!("export {label}"),
                Action::RecordUnstacked { .. } => "record".into(),
                Action::RecordFailure { .. } => "failure".into(),
                Action::WriteSequence { .. } => "sequence".into(),
                Action::Finish(_) => "finish".into(),
                other => format!("{other:?}"),
            })
            .collect()
    }

    fn phase_name(m: &Machine) -> String {
        format!("{:?}", PhaseName(m.phase()))
    }

    /// A run brought to a phase, and the events that would take it on.
    type Setup = fn() -> (Machine, Vec<Event>);

    /// The phase, where Pause leaves it, what the events do and where they
    /// leave it, and what Resume starts.
    type Row = (
        &'static str,
        Setup,
        &'static str,
        &'static [&'static str],
        &'static str,
        &'static [&'static str],
    );

    /// A pause holds the next step in every phase; the sequence and
    /// closing go on.
    #[test]
    fn a_pause_holds_in_every_phase() {
        let ready: Setup = || {
            let m = Machine::new(jobs(&["a", "b"]), SAVE, words());
            (m, vec![Event::Start])
        };
        let stacking: Setup = || {
            let mut m = Machine::new(jobs(&["a", "b"]), SAVE, words());
            m.on(Event::Start);
            (m, vec![Event::StackFailed("device lost".into())])
        };
        let paused: Setup = || {
            let mut m = Machine::new(jobs(&["a", "b"]), SAVE, words());
            m.on(Event::Start);
            m.on(Event::Pause);
            m.on(Event::StackStopped);
            (m, vec![])
        };
        let awaiting_export: Setup = || {
            let mut m = Machine::new(jobs(&["a", "b"]), EXPORT, words());
            m.on(Event::Start);
            let a = stacked(&mut m).0.unwrap();
            m.on(Event::Saved(a));
            (m, vec![Event::Exported { stack: a, result: Ok(()) }])
        };
        let awaiting_saves: Setup = || {
            let mut m = Machine::new(jobs(&["s1", "s2", "s3"]), TURNTABLE, words());
            m.on(Event::Start);
            let mut saves = Vec::new();
            for i in 0..3 {
                let stack = m.running().unwrap().0;
                let _ = m.stacked(m.current().unwrap().frames.clone(), shape(i == 2));
                m.on(Event::Exported { stack, result: Ok(()) });
                saves.push(Event::Saved(stack));
            }
            (m, saves)
        };
        let sequence: Setup = || {
            let options =
                Options { save: false, export: false, turntable: Some(Turntable { fix: false }) };
            let mut m = Machine::new(jobs(&["s1"]), options, words());
            m.on(Event::Start);
            let _ = stacked(&mut m);
            (m, vec![Event::SequenceEnded(SequenceEnd::Finished("4 frames".into()))])
        };
        let closing: Setup = || {
            let mut m = Machine::new(jobs(&["a"]), SAVE, words());
            m.on(Event::Start);
            let a = stacked(&mut m).0.unwrap();
            (m, vec![Event::Saved(a)])
        };
        let done: Setup = || {
            let mut m = Machine::new(jobs(&["a"]), SAVE, words());
            m.on(Event::Start);
            m.on(Event::StackFailed("out of memory".into()));
            (m, vec![])
        };
        #[rustfmt::skip]
        let rows: [Row; 8] = [
            ("Ready",          ready,           "ReadyPaused", &["record"], "Paused", &["stack a"]),
            ("Stacking",       stacking,        "Stacking",    &[],         "Paused", &["stack a"]),
            ("Paused",         paused,          "Paused",      &[],         "Paused", &["stack a"]),
            ("AwaitingExport", awaiting_export, "Paused",      &[],         "Paused", &["stack b"]),
            ("AwaitingSaves",  awaiting_saves,  "Paused",      &[],         "Paused", &["stack s3"]),
            ("Sequence",       sequence,        "Sequence",    &["finish"], "Done",   &[]),
            ("Closing",        closing,         "Closing",     &["finish"], "Done",   &[]),
            ("Done",           done,            "Done",        &[],         "Done",   &[]),
        ];
        for (phase, setup, held, landed, then, resumed) in rows {
            let (mut m, events) = setup();
            assert_eq!(phase_name(&m), phase, "set up");
            assert!(m.on(Event::Pause).is_empty(), "{phase}: Pause does nothing itself");
            assert_eq!(phase_name(&m), held, "{phase}: paused");
            let mut out = Vec::new();
            for event in events {
                out.extend(m.on(event));
            }
            assert_eq!(steps(&out), landed, "{phase}: while paused");
            assert_eq!(phase_name(&m), then, "{phase}: after");
            if then == "Paused" {
                assert!(m.status().ends_with(" · paused"), "{phase}: {}", m.status());
            }
            assert_eq!(steps(&m.on(Event::Resume)), resumed, "{phase}: resumed");
        }
    }

    /// Paused before Start: Resume undoes it, and Cancel ends the run.
    #[test]
    fn a_pause_before_start() {
        let mut m = Machine::new(jobs(&["a"]), SAVE, words());
        m.on(Event::Pause);
        assert_eq!(m.status(), "Batch 0 of 1 · paused");
        assert!(m.on(Event::Resume).is_empty());
        assert_eq!(m.phase(), &Phase::Ready);
        assert_eq!(steps(&m.on(Event::Start)), ["record", "stack a"]);
        let mut m = Machine::new(jobs(&["a"]), SAVE, words());
        m.on(Event::Pause);
        let out = m.on(Event::Cancel);
        assert_eq!(summary(&out).line(&words()), "Batch canceled: 0 of 1 made");
    }

    const TURNTABLE: Options =
        Options { save: true, export: true, turntable: Some(Turntable { fix: true }) };

    /// Stack and save every job, the last reversed; the actions after the
    /// last save.
    fn stack_all(m: &mut Machine, labels: usize) -> (Vec<StackNo>, Vec<Action>) {
        let mut stacks = Vec::new();
        m.on(Event::Start);
        for i in 0..labels {
            let (stack, _) = m.running().unwrap();
            stacks.push(stack);
            let paths = m.current().unwrap().frames.clone();
            let made = m.stacked(paths.clone(), shape(i == labels - 1));
            let save = Save { stack, job: Some(JobNo(i)), listed: paths };
            assert_eq!(made.save, Some(save), "a job's stack records its frames");
            if m.options.export {
                m.on(Event::Exported { stack, result: Ok(()) });
            }
        }
        assert_eq!(m.phase(), &Phase::AwaitingSaves);
        assert!(m.listed().is_empty(), "nothing runs");
        assert_eq!(m.status(), "Saving the renders");
        let mut out = Vec::new();
        for &stack in &stacks {
            out = m.on(Event::Saved(stack));
        }
        (stacks, out)
    }

    #[test]
    fn a_turntable_restacks_the_stack_that_is_off() {
        let mut m = Machine::new(jobs(&["s1", "s2", "s3"]), TURNTABLE, words());
        let (stacks, out) = stack_all(&mut m, 3);
        let Action::Stack { stack: restack, label, listed, frames } = &out[0] else {
            panic!("{out:?}")
        };
        assert_eq!(label, "s3");
        assert_eq!(listed.len(), 3);
        assert_eq!(frames[..], listed[1..], "without its first frame");
        assert_eq!(m.status(), "Checking stacks · restacking s3 without its first frame");
        let restack = *restack;
        // It lines up: saved with the render's list, exported over its file.
        let made = m.stacked(frames.clone(), shape(false));
        assert_eq!(made.save, Some(Save { stack: restack, job: None, listed: listed.clone() }));
        assert_eq!(
            made.then,
            [Action::Export { stack: restack, label: "s3".into(), replaces: Some(stacks[2]) }]
        );
        assert_eq!(m.phase(), &Phase::AwaitingExport { slot: 2 });
        assert!(m.on(Event::Saved(restack)).is_empty(), "its export is still out");
        let out = m.on(Event::Exported { stack: restack, result: Ok(()) });
        let Action::WriteSequence { stacks: sequence, notes, .. } = &out[0] else {
            panic!("{out:?}")
        };
        let order: Vec<StackNo> = sequence.iter().map(|s| s.stack).collect();
        assert_eq!(order, [stacks[0], stacks[1], restack]);
        assert_eq!(notes, &["s3 restacked without its first frame"]);
        assert_eq!(m.phase(), &Phase::Sequence);
        assert_eq!(m.on(Event::Cancel), [Action::CancelSequence]);
        let out = m.on(Event::SequenceEnded(SequenceEnd::Canceled("12 frames kept".into())));
        assert_eq!(out[0], Action::Delete(vec![stacks[2]]), "the replaced render goes");
        let summary = summary(&out);
        assert_eq!(
            summary.exports.as_ref().map(|e| (e.written, &e.failed[..])),
            Some((3, &[][..])),
            "the restack wrote over s3's file"
        );
        assert_eq!(
            parts(summary),
            [
                "Batch canceled: 3 of 3 made",
                "3 exported",
                "12 frames kept",
                "s3 restacked without its first frame"
            ]
        );
    }

    /// A restack's export writes over the replaced file; each stack counts
    /// once, by its last export, and the sequence follows.
    #[test]
    fn a_restack_export_counts_the_stack_once() {
        for second in [Ok(()), Err("The disk is full.".to_string())] {
            let case = format!("restack {second:?}");
            let mut m = Machine::new(jobs(&["s1", "s2", "s3"]), TURNTABLE, words());
            let (stacks, out) = stack_all(&mut m, 3);
            let Action::Stack { stack: restack, frames, .. } = &out[0] else { panic!("{out:?}") };
            let restack = *restack;
            let made = m.stacked(frames.clone(), shape(false));
            assert_eq!(made.save.map(|s| s.stack), Some(restack), "{case}");
            assert_eq!(
                made.then,
                [Action::Export { stack: restack, label: "s3".into(), replaces: Some(stacks[2]) }],
                "{case}"
            );
            m.on(Event::Saved(restack));
            let ok = second.is_ok();
            let out = m.on(Event::Exported { stack: restack, result: second });
            let summary = if ok {
                assert!(matches!(out[0], Action::WriteSequence { .. }), "{case}");
                let out = m.on(Event::SequenceEnded(SequenceEnd::Finished("12 frames".into())));
                summary(&out).clone()
            } else {
                summary(&out).clone()
            };
            if ok {
                assert_eq!(
                    summary.exports.as_ref().map(|e| (e.written, &e.failed[..])),
                    Some((3, &[][..])),
                    "{case}"
                );
            } else {
                assert_eq!(
                    summary.exports.as_ref().map(|e| (e.written, &e.failed[..])),
                    Some((2, &["s3".to_string()][..])),
                    "{case}"
                );
                assert_eq!(summary.sequence, SequenceOutcome::NotWritten, "{case}");
            }
        }
    }

    /// The last export fails: the run ends before the turntable check.
    #[test]
    fn a_failed_export_ends_the_run_before_the_check() {
        let mut m = Machine::new(jobs(&["s1", "s2", "s3"]), TURNTABLE, words());
        m.on(Event::Start);
        let mut stacks = Vec::new();
        for i in 0..3 {
            let stack = m.running().unwrap().0;
            stacks.push(stack);
            let paths = m.current().unwrap().frames.clone();
            let _ = m.stacked(paths, shape(i == 2));
            let result = if i < 2 { Ok(()) } else { Err("The disk is full.".into()) };
            let out = m.on(Event::Exported { stack, result });
            assert_eq!(started(&out).len(), usize::from(i < 2));
        }
        m.on(Event::Saved(stacks[0]));
        m.on(Event::Saved(stacks[1]));
        let out = m.on(Event::Saved(stacks[2]));
        assert!(started(&out).is_empty(), "no restack");
        assert_eq!(
            summary(&out).text(&words(), None),
            "Batch stopped: 3 of 3 made · 2 exported · 1 not exported · the turntable sequence \
             wasn't written. \"s3\" couldn't be exported. The disk is full."
        );
    }

    /// A restack landing after Cancel isn't saved: no sequence is written.
    #[test]
    fn a_restack_made_after_cancel_is_never_saved() {
        let mut m = Machine::new(jobs(&["s1", "s2", "s3"]), TURNTABLE, words());
        let (_, out) = stack_all(&mut m, 3);
        let Action::Stack { frames, .. } = &out[0] else { panic!("{out:?}") };
        assert_eq!(m.on(Event::Cancel), [Action::CancelStack]);
        let made = m.stacked(frames.clone(), shape(false));
        assert_eq!(made.save, None);
        assert!(m.running().is_none());
    }

    #[test]
    fn a_restack_still_off_is_dropped_and_never_saved() {
        let options = Options { export: false, ..TURNTABLE };
        let mut m = Machine::new(jobs(&["s1", "s2", "s3"]), options, words());
        let (stacks, out) = stack_all(&mut m, 3);
        let Action::Stack { frames, .. } = &out[0] else { panic!() };
        let made = m.stacked(frames.clone(), shape(true));
        assert_eq!(made.save, None);
        let out = made.then;
        let [Action::WriteSequence { stacks: sequence, skipped, .. }] = &out[..] else {
            panic!("{out:?}")
        };
        assert_eq!(sequence.iter().map(|s| s.stack).collect::<Vec<_>>(), stacks[..2]);
        assert_eq!(skipped[0].what, "s3");
        let out = m.on(Event::SequenceEnded(SequenceEnd::Finished("8 frames".into())));
        assert_eq!(
            parts(summary(&out)),
            ["Batch: 3 of 3 made", "8 frames", "s3 left out of the sequence"]
        );
    }

    /// The first failed save ends a turntable run before its sequence,
    /// after the saves out land.
    #[test]
    fn a_turntable_stops_at_the_first_failed_save() {
        let mut m = Machine::new(jobs(&["s1", "s2", "s3"]), TURNTABLE, words());
        m.on(Event::Start);
        let mut stacks = Vec::new();
        for _ in 0..2 {
            stacks.push(m.running().unwrap().0);
            let _ = stacked(&mut m);
            if stacks.len() == 1 {
                m.on(Event::Exported { stack: stacks[0], result: Ok(()) });
            }
        }
        // s2's export is being written.
        let read_only = "The disk can't be written to.";
        let out = m.on(Event::SaveFailed { stack: stacks[0], error: read_only.into() });
        assert!(matches!(out[..], [Action::RecordFailure { .. }]), "{out:?}");
        assert!(m.is_ending());
        assert!(m.on(Event::Exported { stack: stacks[1], result: Ok(()) }).is_empty());
        assert_eq!(m.phase(), &Phase::Closing, "s2's save is still out");
        let out = m.on(Event::Saved(stacks[1]));
        let summary = summary(&out);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.sequence, SequenceOutcome::NotWritten);
        assert!(summary.problem());
        assert_eq!(summary.skipped, ["s1"]);
        assert_eq!(
            summary.text(&words(), None),
            "Batch stopped: 1 of 3 made · 1 failed · 2 exported · the turntable sequence wasn't \
             written · s1 left out of the sequence. The render of \"s1\" couldn't be saved. The \
             disk can't be written to."
        );
    }

    #[test]
    fn a_turntable_without_the_check_writes_straight_away() {
        let options =
            Options { save: false, export: false, turntable: Some(Turntable { fix: false }) };
        let mut m = Machine::new(jobs(&["s1", "s2", "s3"]), options, words());
        m.on(Event::Start);
        let mut out = Vec::new();
        for i in 0..3 {
            let paths = m.current().unwrap().frames.clone();
            let made = m.stacked(paths, shape(i == 2));
            assert_eq!(made.save, None, "no history");
            out = made.then;
        }
        // No history: no saves to wait for, nothing restacked.
        let [Action::WriteSequence { stacks, .. }] = &out[..] else { panic!("{out:?}") };
        assert_eq!(stacks.len(), 3);
        // A canceled run writes none.
        let mut m = Machine::new(jobs(&["s1", "s2"]), options, words());
        m.on(Event::Start);
        m.on(Event::Cancel);
        let out = m.on(Event::StackStopped);
        assert_eq!(parts(summary(&out)), ["Batch canceled: 0 of 2 made"]);
    }
}
