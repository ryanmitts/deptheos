//! Batch runs: planning them and the queue over the batch machine.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use deptheos_app::engine::work::{self, Lane};
use deptheos_app::frames::folder::{self, Source};
use deptheos_app::runs::batch::sources::Sources;
use deptheos_app::runs::batch::{self, Input, Split};
use deptheos_app::{Platform, RawBackend, Words as AppWords};

use crate::request::{Ask, on_pool};
use crate::{ListInfo, Request, Result, Told, Words};

/// Where a batch run's images come from.
#[derive(Clone, Debug, uniffi::Enum)]
pub enum BatchInput {
    /// The images directly inside a folder.
    Folder { path: PathBuf },
    /// The list's included images, as one source.
    List { list: ListInfo, label: String },
}

impl From<BatchInput> for Input {
    fn from(input: BatchInput) -> Self {
        match input {
            BatchInput::Folder { path } => Self::Folder(path),
            BatchInput::List { list, label } => Self::List { list: list.into(), label },
        }
    }
}

/// One render per folder, or each folder split by capture time
/// (`deptheos_app::runs::batch::Split`).
#[uniffi::remote(Enum)]
pub enum Split {
    PerFolder,
    AutoSort,
}

/// One render a run makes.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct BatchJob {
    pub label: String,
    /// In order.
    pub frames: Vec<PathBuf>,
}

/// What a run will do.
#[derive(Clone, Debug, uniffi::Record)]
pub struct BatchPlan {
    pub jobs: Vec<BatchJob>,
    /// Images left out: strays auto sort can't place, folders too small.
    pub skipped: u32,
    /// "1 folder, 312 images → 14 stacks, 9 skipped".
    pub summary: String,
    /// The run's header in the history: "Auto sort · 9 skipped".
    pub label: String,
    /// Which the header is worded from.
    pub split: Split,
}

/// What a run of `inputs` split by `split` would make, pairs by `source`,
/// RAW by `raw_decoder`. Auto sort reads every capture time inside the
/// folders' scope, until `request` is canceled.
#[uniffi::export]
pub async fn plan_batch(
    request: Arc<Request>,
    inputs: Vec<BatchInput>,
    split: Split,
    source: Source,
    raw_decoder: RawBackend,
    words: Arc<Words>,
) -> Result<BatchPlan> {
    let inputs: Vec<Input> = inputs.into_iter().map(Input::from).collect();
    let told = words.words.clone();
    on_pool(&words.words, &request, Lane::Interactive, Ask::Own("batch-plan"), move || {
        let cancel = work::current();
        let plan = batch::plan(
            &inputs,
            split,
            source,
            deptheos_app::FrameOrder::Name,
            raw_decoder,
            // Only auto sort reads the files (iOS stacks in name order with
            // no note); once canceled, nothing more is read.
            |path: &std::path::Path| match split {
                Split::AutoSort if !cancel.is_canceled() => batch::capture_info(path),
                _ => deptheos_app::frames::capture::CaptureInfo::default(),
            },
            &told,
        );
        cancel.check().told(&told)?;
        Ok(BatchPlan {
            summary: plan.summary(&told),
            skipped: plan.skipped as u32,
            jobs: plan.jobs.iter().map(BatchJob::from).collect(),
            label: plan.label(&told),
            split: plan.split,
        })
    })
    .await
}

impl From<BatchJob> for batch::Job {
    fn from(job: BatchJob) -> Self {
        Self { label: job.label, frames: job.frames }
    }
}

impl From<&batch::Job> for BatchJob {
    fn from(job: &batch::Job) -> Self {
        Self { label: job.label.clone(), frames: job.frames.clone() }
    }
}

/// A run under way, as [`batch::Machine`] keeps it on every platform. The
/// app reports what happened and does what it answers, in order; it starts
/// a stack only when none runs, so one task drives the run and a UI call
/// (Cancel, backgrounding) never waits on it. A paused run resuming with
/// nothing running answers with the next stack.
///
/// Lifecycle: the machine's `Phase` (ADR-144): ready, paused before Start,
/// stacking, paused, closing or done. Held by iOS's run
/// (`ios/DeptheosMobile/Model/AppModel+Batch.swift`), changed only through
/// these methods, each a machine step under its lock.
#[derive(uniffi::Object)]
pub struct BatchQueue {
    run: Mutex<batch::Machine>,
}

/// How a run is going, as of the last call that changed it.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct BatchStatus {
    /// Canceled, or stopped by an unsaveable render: a stack it said to run
    /// is to be reported `stopped`, not run.
    pub canceled: bool,
    /// "Batch 2 of 14 · P1010217–219", "Batch 2 of 14 · paused".
    pub progress: String,
}

impl From<&batch::Machine> for BatchStatus {
    fn from(run: &batch::Machine) -> Self {
        Self { canceled: run.is_ending(), progress: run.status() }
    }
}

/// What the app is to do for its run.
#[derive(Clone, Debug, PartialEq, uniffi::Enum)]
pub enum BatchAction {
    /// Stack `frames` as stack `stack`, then report `stacked`, `failed` or
    /// `stopped`.
    Stack { stack: u32, frames: Vec<PathBuf> },
    /// Stop the stack running.
    CancelStack,
    /// First, at `start`: keep every job (`jobs[n]` as job `n`) as a failed
    /// stack saying `why`, in the stack its render saves into
    /// (`Engine::record_unstacked`), hidden until it fails or the run ends.
    RecordUnstacked { jobs: Vec<BatchJob>, why: String },
    /// Keep failed job `no` in the history to retry, rewording its record
    /// (`Engine::save_failure`).
    RecordFailure { no: u32, job: BatchJob, error: String },
    /// The run is over: `summary` says how it went.
    Finish { summary: String },
}

/// What the run says of the render just made (`BatchQueue::stacked`).
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct BatchStacked {
    /// Keep it as stack `save`, recording `listed`, before anything in
    /// `then`; then report `saved` or `save_failed`. Given for every render
    /// a started stack made, after Cancel too; `None` only when none ran.
    pub save: Option<BatchSave>,
    pub then: Vec<BatchAction>,
}

/// Keep the render as stack `stack`, recording `listed`, into job `job`'s
/// record's stack, which the save removes.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct BatchSave {
    pub stack: u32,
    pub job: u32,
    pub listed: Vec<PathBuf>,
}

impl BatchQueue {
    fn on(&self, event: batch::Event) -> Vec<BatchAction> {
        let mut run = crate::lock(&self.run);
        let actions = run.on(event);
        BatchAction::all(actions, run.words())
    }
}

impl BatchAction {
    fn all(actions: Vec<batch::Action>, words: &AppWords) -> Vec<Self> {
        actions.into_iter().filter_map(|a| Self::from_action(a, words)).collect()
    }

    /// The app's run saves and nothing else: no exports, no sequence.
    fn from_action(action: batch::Action, words: &AppWords) -> Option<Self> {
        Some(match action {
            batch::Action::Stack { stack, frames, .. } => Self::Stack { stack: stack.0, frames },
            batch::Action::CancelStack => Self::CancelStack,
            batch::Action::RecordUnstacked { jobs, why } => {
                Self::RecordUnstacked { jobs: jobs.iter().map(BatchJob::from).collect(), why }
            }
            batch::Action::RecordFailure { no, job, error } => {
                Self::RecordFailure { no: no.0 as u32, job: BatchJob::from(&job), error }
            }
            // Without exports there is no folder to name.
            batch::Action::Finish(summary) => Self::Finish { summary: summary.text(words, None) },
            other => {
                debug_assert!(false, "a run without exports or a sequence asked for {other:?}");
                return None;
            }
        })
    }
}

#[uniffi::export]
impl BatchQueue {
    /// A run of `jobs`, its progress and summary worded in `words`.
    #[uniffi::constructor]
    pub fn new(jobs: Vec<BatchJob>, words: Arc<Words>) -> Arc<Self> {
        let jobs = jobs.into_iter().map(batch::Job::from).collect();
        let options = batch::machine::Options { save: true, ..Default::default() };
        let run = batch::Machine::new(jobs, options, words.words.clone());
        Arc::new(Self { run: Mutex::new(run) })
    }

    pub fn status(&self) -> BatchStatus {
        BatchStatus::from(&*crate::lock(&self.run))
    }

    pub fn start(&self) -> Vec<BatchAction> {
        self.on(batch::Event::Start)
    }

    /// The running stack made its render: whether and as what to save it,
    /// then what else to do.
    pub fn stacked(&self) -> BatchStacked {
        // Without a turntable the run reads neither.
        let shape = deptheos_app::runs::sequence::StackShape { reversed: false, first_step: None };
        let mut run = crate::lock(&self.run);
        let made = run.stacked(Vec::new(), shape);
        // Without a turntable every render is a job's own.
        let save = made.save.and_then(|s| {
            let job = s.job?.0 as u32;
            Some(BatchSave { stack: s.stack.0, job, listed: s.listed })
        });
        BatchStacked { save, then: BatchAction::all(made.then, run.words()) }
    }

    /// The stack running failed with `error`.
    pub fn failed(&self, error: String) -> Vec<BatchAction> {
        self.on(batch::Event::StackFailed(error))
    }

    /// The stack running was canceled or stopped before it finished.
    pub fn stopped(&self) -> Vec<BatchAction> {
        self.on(batch::Event::StackStopped)
    }

    pub fn saved(&self, stack: u32) -> Vec<BatchAction> {
        self.on(batch::Event::Saved(batch::StackNo(stack)))
    }

    pub fn save_failed(&self, stack: u32, error: String) -> Vec<BatchAction> {
        self.on(batch::Event::SaveFailed { stack: batch::StackNo(stack), error })
    }

    /// Stop what runs; what is made is kept.
    pub fn cancel(&self) -> Vec<BatchAction> {
        self.on(batch::Event::Cancel)
    }

    /// The app left the foreground: start nothing more, and rerun the stack
    /// running unless it makes its render.
    pub fn pause(&self) -> Vec<BatchAction> {
        self.on(batch::Event::Pause)
    }

    /// The app is back.
    pub fn resume(&self) -> Vec<BatchAction> {
        self.on(batch::Event::Resume)
    }

    /// The whole run's progress, with `current` (0 to 1) of the running job.
    pub fn fraction(&self, current: f64) -> f64 {
        crate::lock(&self.run).fraction(current)
    }
}

/// The folders inside `parent` holding images, directly or with `deep` at
/// any depth, read on the pool inside its scope.
#[uniffi::export]
pub async fn folders_inside(
    request: Arc<Request>,
    parent: PathBuf,
    deep: bool,
) -> Result<Vec<PathBuf>> {
    let read = move || Ok(folder::folders_inside(&parent, deep));
    let words = AppWords::english(Platform::Ios);
    on_pool(&words, &request, Lane::Interactive, Ask::Own("batch-folders"), read).await
}

/// A folder the batch stacks, and the parent it was found in, if any.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct BatchSource {
    pub folder: PathBuf,
    pub parent: Option<PathBuf>,
}

/// The batch sheet's folders: those to stack in order, the parents to list,
/// and whether deeper folders count.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct BatchSourcesNow {
    pub entries: Vec<BatchSource>,
    pub parents: Vec<PathBuf>,
    pub deep: bool,
}

/// The batch sheet's folders, the engine's `runs::batch::Sources` as on the
/// desktop. Each call is one step under its own lock, answered with where
/// they stand.
#[derive(uniffi::Object)]
pub struct BatchSources {
    sources: Mutex<Sources>,
}

#[uniffi::export]
impl BatchSources {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self { sources: Mutex::new(Sources::default()) })
    }

    pub fn now(&self) -> BatchSourcesNow {
        now(&crate::lock(&self.sources))
    }

    /// Folders, each stacked on its own.
    pub fn add_folders(&self, folders: Vec<PathBuf>) -> BatchSourcesNow {
        self.step(|s| s.add_folders(folders))
    }

    /// A parent, whose folders to list ([`folders_inside`]) and hand to
    /// [`Self::listed`].
    pub fn add_parent(&self, parent: PathBuf) -> BatchSourcesNow {
        self.step(|s| s.add_parent(parent))
    }

    /// `parent`'s folders, found at depth `deep`.
    pub fn listed(&self, parent: PathBuf, deep: bool, found: Vec<PathBuf>) -> BatchSourcesNow {
        self.step(|s| s.listed(&parent, deep, found))
    }

    pub fn remove(&self, folder: PathBuf) -> BatchSourcesNow {
        self.step(|s| s.remove(&folder))
    }

    /// Deeper folders count or not; the parents are listed again.
    pub fn set_deep(&self, deep: bool) -> BatchSourcesNow {
        self.step(|s| {
            s.set_deep(deep);
        })
    }
}

impl BatchSources {
    fn step(&self, f: impl FnOnce(&mut Sources)) -> BatchSourcesNow {
        let mut sources = crate::lock(&self.sources);
        f(&mut sources);
        now(&sources)
    }
}

fn now(s: &Sources) -> BatchSourcesNow {
    let entries = s
        .entries()
        .iter()
        .map(|e| BatchSource { folder: e.folder.clone(), parent: e.parent.clone() });
    BatchSourcesNow { entries: entries.collect(), parents: s.parents().to_vec(), deep: s.deep() }
}
