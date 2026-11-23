//! Batch runs: the dialog, then the run, driven by
//! [`deptheos_app::runs::batch::Machine`]: this feeds it events and carries
//! out its actions (exports, sequence and closing notice in `batch_after.rs`).

use std::collections::VecDeque;
use std::sync::Arc;

use deptheos_app::library::history::{Batch, RenderId, StackId};
use deptheos_app::runs::batch::machine::{Options, Turntable};
use deptheos_app::runs::batch::{Action, Event, Machine, Plan, SequenceEnd};
use deptheos_app::runs::sequence::StackShape;
use deptheos_app::{AlignSettings, Alignment, CompletedStack, RawBackend};
use gpui::prelude::*;
use gpui::{App, Context, Window};
use image::RgbImage;

use super::{AppView, BatchRun, Running, Settings, batch_after};
use crate::batch_dialog::{BatchDialog, BatchEvent};
use crate::choice::choice_select;
use crate::notices::{Kind, Subject};
use crate::params::installed_backends;
use crate::prefs::prefs;
use crate::sequences::SequenceEvent;
use crate::words::{Wording as _, say};

impl AppView {
    /// A batch's settings: the inspector's, camera RAW decoded by
    /// `raw_backend` (the dialog's), else
    /// [`crate::params::StackParams::batch_backend`], never the unused list's.
    fn batch_settings(&self, raw_backend: Option<RawBackend>, cx: &App) -> Settings {
        let mut settings = self.settings(cx);
        let params = self.params.read(cx);
        settings.options.raw_backend = raw_backend.unwrap_or_else(|| params.batch_backend(cx));
        settings
    }

    /// New Batch…, in the Library, where its renders show.
    pub(crate) fn open_batch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_processing(cx) {
            return;
        }
        self.set_screen(super::Screen::Library, window, cx);
        let settings = self.settings(cx);
        let summary = settings.config.summary(crate::words::of(cx));
        let alignment = settings.options.alignment;
        let grid = alignment.mode() == Some(deptheos_app::AlignMode::Grid);
        let label = match (grid, alignment.crops()) {
            (false, true) => say!(cx, "batch-settings", summary = summary),
            (true, true) => say!(cx, "batch-settings-grid", summary = summary),
            (false, false) => say!(cx, "batch-settings-uncropped", summary = summary),
            (true, false) => say!(cx, "batch-settings-grid-uncropped", summary = summary),
        };
        let list = self.files.read(cx).list().clone();
        // Which decoder camera RAW goes through, when there is a choice.
        let params = self.params.read(cx);
        let decoder = params.has_backend_choice().then(|| params.batch_backend(cx));
        let decoder = decoder.map(|b| choice_select(installed_backends(), &b, window, cx));
        let pyramid = matches!(settings.config, deptheos_app::StackerConfig::Pyramid(_));
        let format = prefs(cx).save_format;
        let after = crate::batch_dialog::AfterControls::new(
            pyramid,
            format,
            prefs(cx).save_options(),
            window,
            cx,
        );
        let dialog = cx.new(|_| BatchDialog::new(&list, label.into(), decoder, after));
        self.open_modal(
            dialog,
            super::Sheet::Batch,
            |this, event: &BatchEvent, window, cx| {
                let BatchEvent::Start { plan, batch, raw_backend, after } = event else {
                    return true;
                };
                let (plan, batch, after) = (plan.clone(), batch.clone(), after.clone());
                if let Some(sixteen_bit) = after.sixteen_bit() {
                    crate::prefs::update_prefs(cx, |p| p.export.sixteen_bit = sixteen_bit);
                }
                this.start_batch_with(plan, batch, *raw_backend, after, window, cx)
            },
            window,
            cx,
        );
    }

    /// Run `plan`'s stacks in turn with the current settings (`raw_backend`
    /// if given), renders under `batch`'s header.
    #[cfg(test)]
    pub(crate) fn start_batch(
        &mut self,
        plan: Plan,
        batch: Batch,
        raw_backend: Option<RawBackend>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let after = crate::batch_dialog::After::default();
        self.start_batch_with(plan, batch, raw_backend, after, window, cx);
    }

    /// `start_batch` (the tests' entry point) with `after`: exports and the
    /// turntable sequence. A turntable run aligns and crops regardless.
    /// `false`, with a notice why, when it can't start.
    pub(crate) fn start_batch_with(
        &mut self,
        plan: Plan,
        batch: Batch,
        raw_backend: Option<RawBackend>,
        after: crate::batch_dialog::After,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let busy = self.is_processing(cx);
        if busy || plan.jobs.is_empty() {
            let why = match self.running {
                _ if !busy => say!(cx, "batch-nothing-to-stack"),
                Running::Batch(_) => say!(cx, "batch-busy-batch"),
                Running::Sequence { .. } => say!(cx, "batch-busy-sequence"),
                Running::Stack(_) | Running::Idle => say!(cx, "batch-busy-stack"),
            };
            self.post(Kind::Error, Subject::Batch, why, Vec::new(), window, cx);
            return false;
        }
        // The working version's settings, as Stack's.
        let mut settings = self.batch_settings(raw_backend, cx);
        settings.options.order = plan.order;
        let mut after = after;
        if let Some(turntable) = after.turntable.as_mut() {
            let alignment = settings.options.alignment.settings();
            settings.options.alignment = Alignment::On(AlignSettings { crop: true, ..alignment });
            // Grow replays each stack's blend: Pyramid only.
            if !matches!(settings.config, deptheos_app::StackerConfig::Pyramid(_)) {
                turntable.options.mode = deptheos_app::runs::sequence::TurntableMode::Full;
            }
        }
        if let Err(why) = Self::check_turntable_space(cx.wording(), &after, &plan.jobs) {
            let w = cx.wording().clone();
            let message = why.reworded(|why| say!(w, "batch-cant-start", why = why));
            self.post(Kind::Error, Subject::Batch, message, Vec::new(), window, cx);
            return false;
        }
        let options = Options {
            save: true,
            export: after.export.is_some(),
            turntable: after.turntable.as_ref().map(|_| Turntable { fix: true }),
        };
        // Locked from start to closing notice, between stacks too.
        let stacks = plan.jobs.iter().map(|_| StackId::new()).collect();
        let run = BatchRun {
            batch,
            settings,
            stacks,
            machine: Machine::new(plan.jobs, options, crate::words::of(cx).clone()),
            after: batch_after::AfterRun::new(after),
            _awake: super::run::Awake::new("Deptheos is running a batch", cx),
        };
        self.set_running(Running::Batch(Box::new(run)), cx);
        self.drive_batch(Event::Start, window, cx);
        true
    }

    /// Tell the run `event`, and do what it says.
    pub(super) fn drive_batch(
        &mut self,
        event: Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(run) = self.batch_run_mut() else { return };
        let actions = run.machine.on(event);
        self.carry_out(actions, None, window, cx);
    }

    /// Do `actions` in turn, reporting each outcome and doing what the run
    /// answers, until it says nothing more. `made` is the stack just made.
    fn carry_out(
        &mut self,
        actions: Vec<Action>,
        made: Option<&JustStacked>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut actions = VecDeque::from(actions);
        while let Some(action) = actions.pop_front() {
            if let Some(next) = self.batch_act(action, made, window, cx) {
                let Some(run) = self.batch_run_mut() else { break };
                actions.extend(run.machine.on(next));
            }
        }
        cx.notify();
    }

    /// Carry out `action`; what it came to at once, if anything.
    fn batch_act(
        &mut self,
        action: Action,
        made: Option<&JustStacked>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Event> {
        let run = self.batch_run_mut()?;
        let batch = run.batch.clone();
        match action {
            Action::Stack { listed, frames, .. } => {
                let settings = run.settings;
                if !self.start_stack(listed, frames, settings, Some(batch), window, cx) {
                    // The run would wait for a stack that never started.
                    return Some(Event::StackFailed(say!(cx, "batch-stack-didnt-start")));
                }
            }
            Action::CancelStack => self.stacking.update(cx, |s, cx| s.cancel(cx)),
            Action::Export { stack, label, replaces } => {
                // Only ever the stack just made: one export at a time.
                let Some(completed) = made.filter(|m| m.no == stack).map(|m| m.stack.clone())
                else {
                    // The run would wait for an export that never started.
                    tracing::error!("the batch exports {label} with no stack to export");
                    let error = run.after.reason(crate::notices::fault_sentence(cx.wording()));
                    return Some(Event::Exported { stack, result: Err(error) });
                };
                return self.export_render(stack, &completed, &label, replaces, window, cx);
            }
            // Every job, unstacked and hidden until it fails or the run ends,
            // each in the stack its render goes into.
            Action::RecordUnstacked { jobs, why } => {
                let (settings, of) = (run.settings, run.stacks.clone());
                self.renders.update(cx, |r, cx| {
                    r.record_unstacked(jobs, of, settings, batch, why, window, cx)
                });
            }
            // Kept under the run's divider to retry, counted by the closing
            // notice; no notice of its own.
            Action::RecordFailure { no, job, error } => {
                let (settings, of) = (run.settings, run.stacks[no.0].clone());
                self.renders.update(cx, |r, cx| {
                    r.record_failure(job, of, settings, batch, error, window, cx)
                });
            }
            Action::WriteSequence { stacks, skipped, notes } => {
                return self.start_turntable(stacks, skipped, notes, cx);
            }
            Action::CancelSequence => self.sequences.read(cx).cancel(),
            Action::Delete(stacks) => run.after.delete(&stacks),
            Action::Finish(summary) => self.finish_batch(summary, window, cx),
        }
        None
    }

    /// A batch stack ended with `stack` (none if nothing made) and `thumbnail`.
    pub(super) fn batch_stack_finished(
        &mut self,
        stack: Option<Arc<CompletedStack>>,
        thumbnail: Option<RgbImage>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(run) = self.batch_run_mut() else { return };
        let (Some(stack), Some((no, _))) = (stack, run.machine.running()) else {
            let event = Event::StackFailed(say!(cx, "batch-stack-no-render"));
            return self.drive_batch(event, window, cx);
        };
        run.after.made(no, &stack);
        let made = run.machine.stacked(stack.paths.clone(), StackShape::of(&stack.geometry));
        let batch = run.batch.clone();
        // A job's render goes into its record's stack (the save removes the
        // record); a restack's is a stack of its own.
        let into = match made.save.as_ref().and_then(|s| s.job) {
            Some(job) => run.stacks[job.0].clone(),
            None => StackId::new(),
        };
        // Saved first: the run says so only of the stack just made.
        let mut unsaved = None;
        match (made.save, thumbnail) {
            (Some(save), Some(thumbnail)) => {
                let completed = stack.clone();
                self.renders.update(cx, |r, cx| {
                    let listed = save.listed;
                    r.autosave(listed, completed, thumbnail, into, None, Some(batch), window, cx)
                });
            }
            (Some(save), None) => {
                let error = say!(cx, "batch-stack-no-thumbnail");
                unsaved = Some(Event::SaveFailed { stack: save.stack, error });
            }
            (None, _) => {}
        }
        let just = JustStacked { no, stack: stack.clone() };
        self.carry_out(made.then, Some(&just), window, cx);
        if let Some(unsaved) = unsaved {
            self.drive_batch(unsaved, window, cx);
        }
    }

    /// A render was saved, or not: if it is the run's, tell the run.
    pub(super) fn batch_render_saved(
        &mut self,
        stack: &Arc<CompletedStack>,
        saved: Result<&RenderId, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(run) = self.batch_run_mut() else { return };
        let Some(no) = run.after.saved(stack, saved.as_ref().ok().copied()) else { return };
        let event = match saved {
            Ok(_) => Event::Saved(no),
            Err(error) => Event::SaveFailed { stack: no, error },
        };
        self.drive_batch(event, window, cx);
    }

    /// The turntable sequence ended. `true` when it was the run's.
    pub(super) fn batch_sequence_ended(
        &mut self,
        event: &SequenceEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(run) = self.batch_run_mut() else { return false };
        if !matches!(run.machine.phase(), deptheos_app::runs::batch::Phase::Sequence) {
            return false;
        }
        run.after.sequence = Some(event.clone());
        let end = match event {
            SequenceEvent::Finished { message, .. } => SequenceEnd::Finished(message.to_string()),
            SequenceEvent::Canceled { message } => SequenceEnd::Canceled(message.to_string()),
            SequenceEvent::Failed(message) => {
                SequenceEnd::Failed(run.after.reason(message.clone()))
            }
        };
        self.drive_batch(Event::SequenceEnded(end), window, cx);
        true
    }

    /// What the batch is on: "Batch 2 of 5 · Bee".
    pub(crate) fn batch_progress(&self) -> Option<String> {
        Some(self.batch_run()?.machine.status())
    }

    /// ⌘. and Cancel have something to cancel: a batch not yet canceled
    /// (even between stacks), a grow sequence, or a stack not asked to stop.
    pub(super) fn can_cancel(&self, cx: &gpui::App) -> bool {
        match &self.running {
            Running::Batch(run) => !run.machine.is_ending(),
            Running::Sequence { .. } => {
                let sequences = self.sequences.read(cx);
                sequences.is_running() && !sequences.cancel_requested()
            }
            Running::Idle | Running::Stack(_) => {
                let stacking = self.stacking.read(cx);
                stacking.is_running() && !stacking.is_canceling()
            }
        }
    }

    /// Every Cancel: the batch, else the running stack or sequence; what is
    /// made is kept.
    pub(crate) fn cancel_run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.batch_run().is_some() {
            self.drive_batch(Event::Cancel, window, cx);
        } else {
            self.stacking.update(cx, |s, cx| s.cancel(cx));
            self.sequences.read(cx).cancel();
        }
    }
}

/// The stack a batch just made, for its export.
pub(super) struct JustStacked {
    no: deptheos_app::runs::batch::StackNo,
    stack: Arc<CompletedStack>,
}
