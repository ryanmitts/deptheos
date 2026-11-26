//! What a batch does besides stacking: export each render as made, write the
//! turntable sequence, and close with a notice of what was made, fixed and
//! skipped. The order is [`deptheos_app::runs::batch::Machine`]'s; this keeps
//! what the desktop needs per stack number.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

use deptheos_app::library::export;
use deptheos_app::library::history::RenderId;
use deptheos_app::runs::batch::{End, Event, SequenceStack, StackNo, Summary};
use deptheos_app::runs::sequence::{self, ManifestGap, TurntableStack};
use deptheos_app::{CompletedStack, Explain, StackGeometry};
use gpui::{Context, Window};

use super::{AppView, Running};
use crate::batch_dialog::After;
use crate::notices::{Details, Kind, Subject, Worded, show_folder};
use crate::sequences::{SequenceEvent, TurntableJob};
use crate::words::{DesktopWords, Wording as _, say};
use crate::work;

/// Why `error` happened, worded for the closing notice (no Copy Details).
fn why(w: &DesktopWords, error: &dyn Explain) -> Worded {
    let explained = crate::notices::explain(error, w.shared());
    crate::notices::sentences(w, None, &explained, Details::Diagnostics)
}

/// What the desktop keeps of a stack the run made.
struct Kept {
    /// To match the render saved from it without keeping its pixels.
    stack: Weak<CompletedStack>,
    render: Option<RenderId>,
    geometry: StackGeometry,
    /// Where its export went, so a restack replacing it writes over it.
    export: Option<PathBuf>,
}

/// The desktop's side of a batch run.
#[derive(Default)]
pub(super) struct AfterRun {
    /// What the Batch dialog asked to be done besides stacking.
    pub(super) asked: After,
    /// By stack number, every stack the run made.
    kept: HashMap<StackNo, Kept>,
    /// How the sequence ended, for its folder.
    pub(super) sequence: Option<SequenceEvent>,
    /// Renders restacks replaced: deleted as the run closes.
    delete: Vec<RenderId>,
    /// Reasons given to the machine that name a fault, so the closing notice
    /// offers the details.
    faults: HashSet<String>,
}

impl AfterRun {
    pub(super) fn new(after: After) -> Self {
        Self { asked: after, ..Self::default() }
    }

    /// Stack `no` made `stack`.
    pub(super) fn made(&mut self, no: StackNo, stack: &Arc<CompletedStack>) {
        let made = Kept {
            stack: Arc::downgrade(stack),
            render: None,
            geometry: stack.geometry.clone(),
            export: None,
        };
        self.kept.insert(no, made);
    }

    /// `stack` was saved as `render` (or failed): its number, if the run's.
    pub(super) fn saved(
        &mut self,
        stack: &Arc<CompletedStack>,
        render: Option<&RenderId>,
    ) -> Option<StackNo> {
        let (no, made) = self
            .kept
            .iter_mut()
            .find(|(_, m)| std::ptr::eq(m.stack.as_ptr(), Arc::as_ptr(stack)))?;
        made.render = render.cloned();
        Some(*no)
    }

    /// `why`'s text, for the run's machine, its fault remembered.
    pub(super) fn reason(&mut self, why: Worded) -> String {
        if why.fault {
            self.faults.insert(why.text.clone());
        }
        why.text
    }

    pub(super) fn delete(&mut self, stacks: &[StackNo]) {
        let renders = stacks.iter().filter_map(|no| self.kept.get(no)?.render.clone());
        self.delete.extend(renders);
    }
}

impl AppView {
    /// Disk room for the run's turntable sequence
    /// ([`sequence::estimate_turntable`]); the renders it reads are the
    /// Library's, costing nothing there. `Err` says why not. Unchecked
    /// without a sequence or a readable first frame; exports aren't
    /// estimated, since the first to fail ends the run.
    pub(super) fn check_turntable_space(
        words: &DesktopWords,
        after: &After,
        jobs: &[deptheos_app::runs::batch::Job],
    ) -> Result<(), Worded> {
        let Some(turntable) = &after.turntable else { return Ok(()) };
        let Some(bytes) = sequence::estimate_turntable(jobs, &turntable.options) else {
            return Ok(());
        };
        deptheos_app::check_space(&turntable.folder, bytes.sequence).map_err(|e| why(words, &e))
    }

    /// Export `completed` (stack `no`) to the run's folder as `label`, or
    /// over `replaces`' file. The name is picked on the worker as it writes:
    /// one export at a time, so the folder then says which names are taken,
    /// by the file system's case rules. Returns the outcome if it couldn't
    /// start.
    pub(super) fn export_render(
        &mut self,
        no: StackNo,
        completed: &Arc<CompletedStack>,
        label: &str,
        replaces: Option<StackNo>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Event> {
        let run = self.batch_run_mut()?;
        let Some(export) = run.after.asked.export.clone() else {
            tracing::error!("the batch exports with no export folder");
            let error = run.after.reason(crate::notices::fault_sentence(cx.wording()));
            return Some(Event::Exported { stack: no, result: Err(error) });
        };
        // DNG only for a RAW stack.
        let extension = export.format.for_stack(completed.is_linear()).extension();
        let slot = replaces.and_then(|r| run.after.kept.get(&r)?.export.clone());
        let batch = run.batch.id.clone();
        let engine = self.sequences.read(cx).engine().clone();
        let (completed, label) = (completed.clone(), label.to_string());
        let (folder, options) = (export.folder, export.options);
        let stem = label.clone();
        let words = cx.wording().clone();
        #[cfg(test)]
        let probe = self.export_probe.clone();
        cx.spawn_in(window, async move |this, cx| {
            let save = move || {
                #[cfg(test)]
                probe.wait();
                let target =
                    export::batch_target(&folder, &stem, extension, slot.as_deref(), Path::exists);
                let saved = engine.save(
                    deptheos_app::Look::exported(
                        &completed,
                        Some(&deptheos_app::Adjustments::NEUTRAL),
                    ),
                    &target,
                    options,
                );
                #[cfg(test)]
                probe.wrote();
                saved.map(|()| target)
            };
            let saved = work::write("batch-export", save).await;
            let saved = match saved {
                Ok(Ok(target)) => Ok(target),
                Ok(Err(e)) => Err(why(&words, &e)),
                Err(e) => {
                    tracing::error!("exporting {label} failed: {e}");
                    Err(crate::notices::fault_sentence(&words))
                }
            };
            this.update_in(cx, |this, window, cx| {
                // The run it was for, not one started since.
                if let Some(run) = this.batch_run_mut().filter(|run| run.batch.id == batch) {
                    // Where it went, so a restack replacing it writes over it.
                    if let (Ok(target), Some(made)) = (&saved, run.after.kept.get_mut(&no)) {
                        made.export = Some(target.clone());
                    }
                    let saved = saved.map(|_| ()).map_err(|why| run.after.reason(why));
                    let event = Event::Exported { stack: no, result: saved };
                    this.drive_batch(event, window, cx);
                }
            })
            .ok();
        })
        .detach();
        None
    }

    /// Write the turntable sequence of `stacks`, from their renders.
    pub(super) fn start_turntable(
        &mut self,
        stacks: Vec<SequenceStack>,
        skipped: Vec<ManifestGap>,
        notes: Vec<String>,
        cx: &mut Context<Self>,
    ) -> Option<Event> {
        let failed = |why: String| {
            Some(Event::SequenceEnded(deptheos_app::runs::batch::SequenceEnd::Failed(why)))
        };
        let store = self.renders.read(cx).store().clone();
        let w = cx.wording().clone();
        let run = self.batch_run_mut()?;
        let Some(turntable) = run.after.asked.turntable.clone() else {
            tracing::error!("the batch wrote a turntable sequence it wasn't asked for");
            let fault = crate::notices::fault_sentence(&w)
                .reworded(|fault| say!(w, "batch-turntable-fault", fault = fault));
            return failed(run.after.reason(fault));
        };
        let mut renders = Vec::new();
        let mut sequence = Vec::new();
        for stack in stacks {
            let Some(made) = run.after.kept.get(&stack.stack) else { continue };
            let Some(render) = made.render.clone() else { continue };
            sequence.push(TurntableStack {
                label: stack.label,
                render: Some(render.as_str().to_string()),
                paths: stack.paths,
                geometry: made.geometry.clone(),
            });
            renders.push(render);
        }
        // The folders' parent names the run ("scan1"), as the CLI does.
        let label = sequence
            .first()
            .and_then(|s| s.paths.first())
            .and_then(|p| p.parent()?.parent()?.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| run.batch.label.text(crate::words::of(cx)));
        let job = TurntableJob {
            store,
            renders,
            stacks: sequence,
            label,
            parent: turntable.folder,
            options: turntable.options,
            skipped,
            notes,
        };
        // The batch holds the lock already; paint closes for the sequence.
        self.leave_paint(cx);
        cx.notify();
        self.sequences.update(cx, |s, cx| s.start_turntable(job, cx));
        None
    }

    /// The viewer holds a result no longer the stack on screen's (another
    /// stack's render, where Retouch and Export would misname it, or one
    /// deleted): show the stack's base result, else a frame.
    pub(super) fn show_own_result(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.forget_result(window, cx);
        let own = self.base.clone();
        match own.filter(|id| self.renders.read(cx).find(id).is_some()) {
            Some(id) => self.renders.update(cx, |r, cx| r.open(&id, window, cx)),
            None => self.show_first_included(window, cx),
        }
    }

    /// The Library with only its failed stacks shown.
    fn show_failed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_screen(super::Screen::Library, window, cx);
        self.library.update(cx, |l, cx| l.show_only(crate::renders::Filter::Failed, cx));
    }

    /// Close the run: unlock, delete renders restacks replaced, and say
    /// what was made, fixed and skipped. The stack on screen was never taken over.
    pub(super) fn finish_batch(
        &mut self,
        summary: Summary,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.batch_run().is_none() {
            return;
        }
        // The run is over: unlocked from here.
        let Running::Batch(run) = self.set_running(Running::Idle, cx) else { return };
        let after = run.after;
        if !after.delete.is_empty() {
            self.renders.update(cx, |r, cx| r.delete_replaced(after.delete, window, cx));
        }
        // Unmade stacks show from here, to retry: after the delete, which a
        // save in flight would refuse.
        self.renders.update(cx, |r, cx| r.batch_ended(window, cx));
        let export_folder = after.asked.export.as_ref().map(|e| e.folder.clone());
        let message = closing_text(&summary, export_folder.as_deref(), &after.faults, cx.wording());
        let kind = if summary.problem() {
            Kind::Warning
        } else if summary.end == End::Canceled {
            Kind::Info
        } else {
            Kind::Done
        };
        let folder = match after.sequence {
            Some(SequenceEvent::Finished { folder, .. }) => Some(folder),
            _ => export_folder,
        };
        let mut actions = Vec::new();
        // The failed stacks' tiles say why each failed, with Try Again.
        if summary.failed > 0 {
            actions.push(self.action(
                say!(cx, "batch-show-failed"),
                |this, window, cx| this.show_failed(window, cx),
                cx,
            ));
        }
        actions.extend(folder.map(|folder| show_folder(cx.wording(), folder)));
        if summary.end != End::Canceled {
            super::away::tell(super::away::RunEnd::Batch, &message.text, window, cx);
        }
        self.post(kind, Subject::Batch, message, actions, window, cx);
        cx.notify();
    }
}

/// The closing notice: counts, exports and sequence ("Batch: 12 of 14 made ·
/// 2 failed · 12 exported to \"Out\""), then why an export or the sequence
/// failed. Stack failures are on their tiles, not here ([`Summary::text`],
/// for iOS and the CLI, says the last).
fn closing_text(
    summary: &Summary,
    export_folder: Option<&std::path::Path>,
    faults: &HashSet<String>,
    w: &DesktopWords,
) -> Worded {
    let short = summary.short_parts(w.shared(), export_folder).join(deptheos_app::words::SEPARATOR);
    let reasons = summary.reasons_besides_stacks();
    match reasons.is_empty() {
        true => Worded::plain(short),
        false => {
            let fault = reasons.iter().any(|r| faults.contains(r));
            let why = deptheos_app::words::sentences(w.shared(), &reasons);
            let text = say!(w, "batch-closing-reasons", short = short, why = why);
            Worded { text, fault }
        }
    }
}

/// For tests: holds exports on the worker while `hold` is set, counting
/// those written.
#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct ExportProbe {
    pub(crate) hold: Arc<std::sync::atomic::AtomicBool>,
    pub(crate) written: Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(test)]
impl ExportProbe {
    /// On the worker, before an export: wait while held.
    fn wait(&self) {
        while self.hold.load(std::sync::atomic::Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    /// On the worker, after an export.
    fn wrote(&self) {
        self.written.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use deptheos_app::runs::batch::{SequenceEnd, SequenceOutcome, Summary};

    use super::closing_text;
    use crate::words::DesktopWords;

    /// The closing notice counts failed stacks without the last's reason
    /// (its tile has it), but says why an export or the sequence failed.
    #[test]
    fn the_closing_notice_counts_and_leaves_stack_reasons_to_their_tiles() {
        let w = DesktopWords::english();
        let failed = Summary {
            total: 3,
            made: 2,
            failed: 1,
            last_failure: Some("\"Gone\" didn't finish. The frames are missing.".into()),
            ..Summary::default()
        };
        let none = Default::default();
        assert_eq!(closing_text(&failed, None, &none, &w).text, "Batch: 2 of 3 made · 1 failed");
        assert!(
            failed.text(w.shared(), None).contains("didn't finish"),
            "the shared text, for iOS and the command line, still says why"
        );
        let sequence = Summary {
            total: 2,
            made: 2,
            sequence: SequenceOutcome::Ended(SequenceEnd::Failed(
                "The sequence couldn't be written.".into(),
            )),
            ..Summary::default()
        };
        let said = closing_text(&sequence, None, &none, &w);
        assert_eq!(said.text, "Batch: 2 of 2 made. The sequence couldn't be written.");
        assert!(!said.fault);
        let faults = ["The sequence couldn't be written.".to_string()].into();
        assert!(closing_text(&sequence, None, &faults, &w).fault, "a fault's reason says so");
    }
}
