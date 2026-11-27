//! Writing grow and turntable sequences (`deptheos_app::runs::sequence`),
//! one at a time on a worker, with progress and Cancel (which keeps the
//! frames written). Draws nothing; the app view reacts to [`SequenceEvent`]s.

use deptheos_app::Cancel;
use std::path::PathBuf;
use std::sync::Arc;

use deptheos_app::library::history::History;
use deptheos_app::runs::sequence::{
    self, GrowOptions, ManifestGap, SequenceProgress, SequenceReport, StackStore, TurntableOptions,
    TurntableStack,
};
use deptheos_app::{CompletedStack, Engine, RenderId};
use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui::{Context, EventEmitter, SharedString, Task};

use crate::words::{Wording as _, say};
use crate::work;

#[derive(Clone, Debug, PartialEq)]
pub enum SequenceEvent {
    /// Every frame was written: what was made, and the folder to show.
    Finished {
        message: SharedString,
        folder: PathBuf,
    },
    /// Canceled: the frames so far are kept, in `folder` if any were.
    Canceled {
        message: SharedString,
    },
    Failed(crate::notices::Worded),
}

/// Where a grow sequence's render comes from.
pub enum GrowFrom {
    /// Already loaded: the result on screen.
    Loaded(Arc<CompletedStack>),
    /// Read from the history on the worker.
    History,
}

/// A grow sequence to write, of a render of `store`, under its saved
/// adjustments.
pub struct GrowJob {
    pub store: Arc<History>,
    pub from: GrowFrom,
    pub render: RenderId,
    pub name: String,
    pub parent: PathBuf,
    pub options: GrowOptions,
}

/// A turntable run's sequence to write, from its renders in the history.
pub struct TurntableJob {
    pub store: Arc<History>,
    pub renders: Vec<RenderId>,
    pub stacks: Vec<TurntableStack>,
    pub label: String,
    pub parent: PathBuf,
    pub options: TurntableOptions,
    pub skipped: Vec<ManifestGap>,
    pub notes: Vec<String>,
}

enum Message {
    Progress(SequenceProgress),
    Done(Result<Vec<SequenceReport>, deptheos_app::Error>),
}

/// The sequence being written.
struct Run {
    cancel: Cancel,
    /// The bar's fill (0–1).
    progress: f32,
    /// The bar's line.
    text: SharedString,
    _drain: Task<()>,
}

pub struct Sequences {
    engine: Arc<Engine>,
    run: Option<Run>,
}

impl EventEmitter<SequenceEvent> for Sequences {}

impl Sequences {
    pub fn new(engine: Arc<Engine>) -> Self {
        Self { engine, run: None }
    }

    pub fn engine(&self) -> &Arc<Engine> {
        &self.engine
    }

    pub fn is_running(&self) -> bool {
        self.run.is_some()
    }

    /// While a sequence is written, the bar's fill (0–1) and its line:
    /// "Grow sequence · frame 30 of 66 written".
    pub fn progress(&self) -> Option<(f32, SharedString)> {
        self.run.as_ref().map(|run| (run.progress, run.text.clone()))
    }

    pub fn cancel(&self) {
        if let Some(run) = &self.run {
            run.cancel.cancel();
        }
    }

    /// Cancel was asked of the sequence running.
    pub fn cancel_requested(&self) -> bool {
        self.run.as_ref().is_some_and(|run| run.cancel.is_canceled())
    }

    pub fn start_grow(&mut self, job: GrowJob, cx: &mut Context<Self>) {
        let engine = self.engine.clone();
        let starting = say!(cx, "sequence-grow-starting");
        self.start(starting, cx, move |cancel, progress| {
            let stack = match job.from {
                GrowFrom::Loaded(stack) => stack,
                GrowFrom::History => Arc::new(job.store.load_render(&job.render)?),
            };
            let saved = job.store.adjustments_to_export(&job.render)?;
            sequence::export_grow(
                &engine,
                &stack,
                &saved.adjustments,
                Some(job.render.as_str()),
                &job.name,
                &job.parent,
                &job.options,
                progress,
                cancel,
            )
            .map(|report| vec![report])
        });
    }

    pub fn start_turntable(&mut self, job: TurntableJob, cx: &mut Context<Self>) {
        let engine = self.engine.clone();
        let starting = say!(cx, "sequence-turntable-starting");
        self.start(starting, cx, move |cancel, progress| {
            let mut store = HistoryStore {
                store: job.store.clone(),
                renders: job.renders,
                loaded: None,
                settings: None,
            };
            sequence::export_turntable(
                &engine,
                &job.stacks,
                &mut store,
                &job.label,
                &job.parent,
                &job.options,
                &job.skipped,
                &job.notes,
                progress,
                cancel,
            )
        });
    }

    fn start(
        &mut self,
        starting: String,
        cx: &mut Context<Self>,
        work: impl FnOnce(
            &Cancel,
            &mut dyn FnMut(&SequenceProgress),
        ) -> Result<Vec<SequenceReport>, deptheos_app::Error>
        + Send
        + 'static,
    ) {
        if self.run.is_some() {
            return;
        }
        let cancel = deptheos_app::engine::work::root().child();
        let (tx, mut rx) = mpsc::unbounded();
        let worker_cancel = cancel.clone();
        let done_tx = tx.clone();
        // The pool itself, not `work::run`: a sequence canceled while it
        // is still queued ends as canceled.
        let job = deptheos_app::engine::work::pool().run(
            work::Lane::Background,
            work::Key::kind("sequence"),
            &cancel,
            move || {
                let mut progress = |p: &SequenceProgress| {
                    tx.unbounded_send(Message::Progress(p.clone())).ok();
                };
                done_tx.unbounded_send(Message::Done(work(&worker_cancel, &mut progress))).ok();
            },
        );
        let drain = cx.spawn(async move |this, cx| {
            // The worker runs on the pool; this is its result.
            let finished = job;
            while let Some(message) = rx.next().await {
                let done = matches!(message, Message::Done(..));
                this.update(cx, |this, cx| this.receive(message, cx)).ok();
                if done {
                    break;
                }
            }
            // A worker that panicked, or was canceled before it started,
            // never sent Done.
            if let Err(ended) = finished.await {
                let failed = Message::Done(Err(match ended {
                    work::Ended::Canceled => deptheos_app::Error::Canceled,
                    work::Ended::Panicked(message) => deptheos_app::Error::InvalidState(message),
                }));
                this.update(cx, |this, cx| this.receive(failed, cx)).ok();
            }
        });
        let text = starting.into();
        self.run = Some(Run { cancel, progress: 0.0, text, _drain: drain });
        cx.notify();
    }

    fn receive(&mut self, message: Message, cx: &mut Context<Self>) {
        match message {
            Message::Progress(p) => {
                if let Some(run) = self.run.as_mut() {
                    run.progress =
                        if p.total > 0 { p.written as f32 / p.total as f32 } else { 0.0 };
                    run.text = p.message(crate::words::of(cx)).into();
                }
            }
            Message::Done(result) => {
                if self.run.take().is_none() {
                    return;
                }
                let event = match result {
                    Ok(reports) => {
                        let parts: Vec<String> =
                            reports.iter().map(|r| r.line(crate::words::of(cx))).collect();
                        let folder = reports.first().map(|r| r.folder.clone()).unwrap_or_default();
                        SequenceEvent::Finished {
                            message: say!(
                                cx,
                                "sequence-exported",
                                made = parts.join(deptheos_app::words::SEPARATOR)
                            )
                            .into(),
                            folder,
                        }
                    }
                    Err(deptheos_app::Error::Canceled) => {
                        SequenceEvent::Canceled { message: say!(cx, "sequence-canceled").into() }
                    }
                    Err(e) => SequenceEvent::Failed(crate::notices::notice(
                        cx.wording(),
                        &say!(cx, "sequence-didnt-finish"),
                        &e,
                    )),
                };
                cx.emit(event);
            }
        }
        cx.notify();
    }
}

/// A turntable run's renders, read back from the history one at a time.
struct HistoryStore {
    store: Arc<History>,
    renders: Vec<RenderId>,
    /// The render the full sequence read last, whole.
    loaded: Option<(usize, CompletedStack)>,
    /// The render the grow read last, without its pictures.
    settings: Option<(usize, CompletedStack)>,
}

impl HistoryStore {
    fn id(&self, index: usize) -> Result<&RenderId, deptheos_app::Error> {
        self.renders
            .get(index)
            .ok_or_else(|| deptheos_app::Error::InvalidState("no such stack".into()))
    }
}

impl StackStore for HistoryStore {
    fn load(&mut self, index: usize) -> Result<&CompletedStack, deptheos_app::Error> {
        if self.loaded.as_ref().is_none_or(|(i, _)| *i != index) {
            // Let go of the last one before reading the next.
            self.loaded = None;
            let stack = self.store.load_render(self.id(index)?)?;
            self.loaded = Some((index, stack));
        }
        Ok(&self.loaded.as_ref().expect("loaded above").1)
    }

    fn load_settings(&mut self, index: usize) -> Result<&CompletedStack, deptheos_app::Error> {
        if self.settings.as_ref().is_none_or(|(i, _)| *i != index) {
            let stack = self.store.load_settings(self.id(index)?)?;
            self.settings = Some((index, stack));
        }
        Ok(&self.settings.as_ref().expect("read above").1)
    }
}
