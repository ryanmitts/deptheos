//! The history's store queue: saves, picks, failures and relinks one at a
//! time on the store thread after the listing; Recently Deleted's delete,
//! restore and Empty; rereading and forgetting renders.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use deptheos_app::frames::sources::Matched;
use deptheos_app::library::history::{
    self, Batch, DeletedId, Deletion, Failure, History, Made, Render as Saved, RenderId, StackId,
    StoreError, Unsaved,
};
use deptheos_app::library::stacks::Settings;
use deptheos_app::runs::batch::Job;
use deptheos_app::{CompletedStack, CpuImage};
use gpui::{App, Context, Window, px};
use image::RgbImage;

use super::{Renders, RendersEvent, THUMB_DECODE_HEIGHT, Writer};
use crate::bookmarks::RESOLVE_WITHIN;
use crate::images::render_image;
use crate::notices::{Details, Subject, Worded};
use crate::words::{DesktopWords, Wording as _, say};
use crate::work;

impl Renders {
    /// Save a finished stack as the next render in stack `of`. `frames` is
    /// every file listed at its start; `from` its base then; `batch` its run.
    #[allow(clippy::too_many_arguments)]
    pub fn autosave(
        &mut self,
        frames: Vec<PathBuf>,
        stack: Arc<CompletedStack>,
        thumbnail: RgbImage,
        of: StackId,
        from: Option<RenderId>,
        batch: Option<Batch>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.saves.push_back(SaveRequest::Render { frames, stack, thumbnail, of, from, batch });
        self.pump(window, cx);
        cx.notify();
    }

    /// Keep a failed batch stack under its run, reworded, in stack `of`.
    #[allow(clippy::too_many_arguments)]
    pub fn record_failure(
        &mut self,
        job: Job,
        of: StackId,
        settings: Settings,
        batch: Batch,
        error: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Job { label, frames } = job;
        self.saves.push_back(SaveRequest::Failure { label, frames, of, settings, batch, error });
        self.pump(window, cx);
        cx.notify();
    }

    /// Keep each of a batch's `jobs` as it starts as an unstacked failure
    /// in its stack, saying `why`, hidden until it fails or the run ends
    /// ([`History::record_unstacked`]).
    #[allow(clippy::too_many_arguments)]
    pub fn record_unstacked(
        &mut self,
        jobs: Vec<Job>,
        of: Vec<StackId>,
        settings: Settings,
        batch: Batch,
        why: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.saves.push_back(SaveRequest::Unstacked { jobs, of, settings, batch, why });
        self.pump(window, cx);
    }

    /// The batch ended: its unstacked stacks show from here, reread after
    /// every save it sent.
    pub fn batch_ended(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.saves.push_back(SaveRequest::BatchEnded);
        self.pump(window, cx);
    }

    /// Load a failed batch stack's frames and settings, to try again.
    pub fn retry(&mut self, id: &RenderId, cx: &mut Context<Self>) {
        if self.is_locked() {
            return;
        }
        if let Some(failure) = self.failures.iter().find(|f| f.id == *id) {
            cx.emit(RendersEvent::Retry(Box::new(failure.clone())));
        }
    }

    /// Star the render on screen, or take its star away.
    pub fn toggle_active_pick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.active(cx).cloned() {
            self.toggle_pick(&id, window, cx);
        }
    }

    pub(super) fn toggle_pick(
        &mut self,
        id: &RenderId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(render) = self.renders.iter_mut().find(|r| r.id == *id) else { return };
        render.picked = !render.picked;
        let picked = render.picked;
        cx.notify();
        self.saves.push_back(SaveRequest::Pick { render: id.clone(), picked });
        self.pump(window, cx);
    }

    /// The only way [`Self::writer`] changes.
    pub(super) fn set_writer(&mut self, writer: Writer) {
        tracing::debug!(from = self.writer.name(), to = writer.name(), "library writer");
        self.writer = writer;
    }

    /// Start the queue's next save, restore or Empty, unless a write runs or
    /// the listing isn't in.
    pub(super) fn pump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.listing.is_listing() || !matches!(self.writer, Writer::Idle) {
            return;
        }
        let Some(request) = self.saves.pop_front() else { return };
        let request = match request {
            SaveRequest::Restore(ids) => return self.start_restore(ids, window, cx),
            SaveRequest::EmptyDeleted => return self.start_empty(window, cx),
            request => request,
        };
        let store = self.store.clone();
        // Which stack, the notice's subject and words, should the save fail.
        // A stack's own render fails under its stack, replacing any
        // "Stacked in" notice; a batch's stack is said by its run.
        let (saving, about) = match &request {
            SaveRequest::Render { stack, of, batch, .. } => (
                Some(stack.clone()),
                if batch.is_none() { Subject::Stack(of.clone()) } else { Subject::Alone },
            ),
            _ => (None, Subject::Alone),
        };
        let failed = request.failed(cx.wording());
        let starred = match &request {
            SaveRequest::Pick { render, picked } => Some((render.clone(), *picked)),
            _ => None,
        };
        self.set_writer(Writer::saving(cx.spawn_in(window, async move |this, cx| {
            let saved = match request {
                // The one render save, both apps' (History::save), with the
                // strip's thumbnail made beside it.
                SaveRequest::Render { frames, stack, thumbnail, of, from, batch } => {
                    let strip = thumbnail.clone();
                    let strip =
                        work::background("strip-thumbnail", move || strip_thumbnail(&strip));
                    let made = match batch {
                        Some(batch) => Made::Batch(batch),
                        None => Made::Stacked { base: from.clone() },
                    };
                    match store.save(frames, stack.clone(), thumbnail, of, made).await {
                        Ok(render) => strip
                            .await
                            .map(|strip| Ok(Done::Saved(Box::new((render, strip, stack, from))))),
                        Err(Unsaved::Failed(e)) => Ok(Err(e)),
                        Err(Unsaved::Panicked(e)) => Err(e),
                    }
                }
                request => {
                    work::store(move || match request {
                        SaveRequest::Render { .. } => {
                            unreachable!("a render is saved by History::save")
                        }
                        SaveRequest::Pick { render, picked } => {
                            store.set_render_picked(&render, picked)?;
                            Ok::<_, StoreError>(Done::Nothing)
                        }
                        SaveRequest::Failure { label, frames, of, settings, batch, error } => {
                            let failure = store
                                .save_failure(&label, &frames, &settings, &of, &batch, &error)?;
                            Ok(Done::Failed(Box::new(failure)))
                        }
                        SaveRequest::Unstacked { jobs, of, settings, batch, why } => {
                            let jobs: Vec<(&str, &[PathBuf], &StackId)> = jobs
                                .iter()
                                .zip(&of)
                                .map(|(job, of)| (job.label.as_str(), job.frames.as_slice(), of))
                                .collect();
                            store.record_unstacked(&jobs, &settings, &batch, &why)?;
                            Ok(Done::Nothing)
                        }
                        SaveRequest::BatchEnded => {
                            store.batch_ended();
                            Ok(Done::Failures(store.failures()?))
                        }
                        SaveRequest::FindAgain => {
                            let frames = store.frames();
                            let led = crate::bookmarks::resolve(&frames.sources(), RESOLVE_WITHIN);
                            let moves = frames.find_again(&led)?;
                            if moves.is_empty() {
                                return Ok(Done::Nothing);
                            }
                            let (listed, failures) = (store.list()?, store.failures()?);
                            Ok(Done::Relinked { moves, said: None, listed, failures })
                        }
                        SaveRequest::Relink { old, new } => {
                            let located = store.locate_folder(&old, &new)?;
                            let (listed, failures) = (store.list()?, store.failures()?);
                            let said = Some((new, located.matched));
                            let moves = located.moves;
                            Ok(Done::Relinked { moves, said, listed, failures })
                        }
                        SaveRequest::Restore(_) | SaveRequest::EmptyDeleted => {
                            unreachable!("a restore or Empty runs as a delete")
                        }
                    })
                    .await
                }
            };
            this.update_in(cx, |this, window, cx| {
                this.set_writer(Writer::Idle);
                match saved {
                    Ok(Ok(Done::Saved(saved))) => {
                        let (render, thumbnail, stack, from) = *saved;
                        let id = render.id.clone();
                        this.thumbnails.saved(&id, render_image(thumbnail), cx);
                        // The store drops its stack's failure as it saves.
                        this.failures.retain(|f| f.stack != render.stack);
                        // A restore's listing may have taken it already.
                        this.renders.retain(|r| r.id != render.id);
                        this.renders.push(render);
                        // The app marks it shown if still on screen; the new
                        // render leads the strip.
                        this.scroll.set_offset(gpui::point(px(0.0), px(0.0)));
                        cx.emit(RendersEvent::Saved { stack, render: id, from });
                    }
                    Ok(Ok(Done::Failed(failure))) => {
                        this.failures.retain(|f| f.id != failure.id);
                        this.failures.push(*failure);
                        cx.notify();
                    }
                    Ok(Ok(Done::Failures(failures))) => {
                        this.failures = failures;
                        cx.notify();
                    }
                    Ok(Ok(Done::Relinked { moves, said, listed, failures })) => {
                        // Only the frames change: what was shown stays.
                        for render in &mut this.renders {
                            if let Some(now) = listed.iter().find(|r| r.id == render.id) {
                                render.frames = now.frames.clone();
                                render.left_out = now.left_out.clone();
                            }
                        }
                        for failure in &mut this.failures {
                            if let Some(now) = failures.iter().find(|f| f.id == failure.id) {
                                failure.frames = now.frames.clone();
                            }
                        }
                        if !moves.is_empty() {
                            cx.emit(RendersEvent::Relinked(moves));
                        }
                        if let Some((folder, matched)) = said {
                            cx.emit(RendersEvent::Located { folder, matched });
                        }
                        cx.notify();
                    }
                    Ok(Ok(Done::Nothing)) => {}
                    Ok(Err(e)) => {
                        this.undo_star(starred);
                        let explained = crate::notices::explain(&e, crate::words::of(cx));
                        let why = crate::notices::sentences(
                            cx.wording(),
                            None,
                            &explained,
                            Details::Diagnostics,
                        );
                        this.save_failed(saving, about, &failed, why, cx)
                    }
                    Err(e) => {
                        this.undo_star(starred);
                        tracing::error!("{failed} {e}");
                        let why = crate::notices::fault_sentence(cx.wording());
                        this.save_failed(saving, about, &failed, why, cx)
                    }
                }
                this.pump(window, cx);
                cx.notify();
            })
            .ok();
        })));
    }

    /// Add the `listed` renders not here yet: all at launch, the restored
    /// after a restore.
    pub(super) fn add_listed(&mut self, listed: Vec<Saved>) {
        let known: HashSet<RenderId> = self.renders.iter().map(|r| r.id.clone()).collect();
        let mut renders: Vec<Saved> =
            listed.into_iter().filter(|r| !known.contains(&r.id)).collect();
        renders.append(&mut self.renders);
        renders.sort_by(|a, b| a.id.cmp(&b.id));
        self.renders = renders;
    }

    /// `old` is at `new` now: the roots point there, every render's frames
    /// are reread, and what is open follows ([`RendersEvent::Relinked`]).
    pub fn relink(
        &mut self,
        old: PathBuf,
        new: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.saves.push_back(SaveRequest::Relink { old, new });
        self.pump(window, cx);
    }

    /// A star toggle not written: show what is on disk, unless toggled again.
    fn undo_star(&mut self, starred: Option<(RenderId, bool)>) {
        let Some((id, picked)) = starred else { return };
        if let Some(render) = self.renders.iter_mut().find(|r| r.id == id && r.picked == picked) {
            render.picked = !picked;
        }
    }

    /// A save failed: `lead` says what, `why` why, in a notice about `about`.
    /// An unkept stack render is told to the app with why alone.
    fn save_failed(
        &mut self,
        stack: Option<Arc<CompletedStack>>,
        about: Subject,
        lead: &str,
        why: Worded,
        cx: &mut Context<Self>,
    ) {
        if let Some(stack) = stack {
            cx.emit(RendersEvent::NotSaved { stack, message: why.text.clone().into() });
        }
        let w = cx.wording().clone();
        let message = why.reworded(|why| say!(w, "error-with-lead", lead = lead, why = why));
        self.fail_about(message, about, cx);
    }

    pub(super) fn fail(&mut self, message: impl Into<Worded>, cx: &mut Context<Self>) {
        self.fail_about(message.into(), Subject::Alone, cx);
    }

    /// Say `message` in an error notice about `about`.
    fn fail_about(&mut self, message: Worded, about: Subject, cx: &mut Context<Self>) {
        tracing::error!("{message}");
        cx.emit(RendersEvent::Failed { message, about });
    }

    /// Nothing can be deleted while a delete, save or stack runs.
    pub(super) fn can_delete(&self) -> bool {
        !self.is_locked() && matches!(self.writer, Writer::Idle) && self.saves.is_empty()
    }

    /// Restore and Empty work: no delete, restore or Empty runs or waits.
    pub(super) fn can_restore(&self) -> bool {
        !self.is_deleting()
    }

    /// Why nothing can be deleted now ([`Self::can_delete`]).
    fn why_not_deleted(&self, cx: &App) -> String {
        if self.is_deleting() {
            say!(cx, "renders-delete-deleting")
        } else if self.is_saving() {
            say!(cx, "renders-delete-saving")
        } else {
            say!(cx, "renders-delete-busy")
        }
    }

    /// Delete Other Renders: `id`'s stack less `id` and the starred, into
    /// Recently Deleted.
    pub(super) fn delete_others(
        &mut self,
        id: &RenderId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete(self.others_of(id), window, cx);
    }

    /// Delete `stacks` whole into Recently Deleted without asking (ADR-140):
    /// renders, retouching, failures, kept working version and names, one
    /// record each, with an Undo notice. Source images are never touched.
    pub(crate) fn delete_stacks(
        &mut self,
        stacks: Vec<StackId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if stacks.is_empty() {
            return;
        }
        let store = self.store.clone();
        if !self.can_delete() {
            self.fail(self.why_not_deleted(cx), cx);
            return;
        }
        // Each stack with what the history lists for it.
        let doomed: Vec<(StackId, Vec<RenderId>, Vec<RenderId>)> = stacks
            .iter()
            .map(|stack| {
                let of = |s: &StackId| s == stack;
                (
                    stack.clone(),
                    self.renders.iter().filter(|r| of(&r.stack)).map(|r| r.id.clone()).collect(),
                    self.failures.iter().filter(|f| of(&f.stack)).map(|f| f.id.clone()).collect(),
                )
            })
            .collect();
        let ids: Vec<RenderId> =
            doomed.iter().flat_map(|(_, r, f)| r.iter().chain(f)).cloned().collect();
        let closing = (store.clone(), doomed.clone());
        self.run_delete(
            Doomed::Stacks,
            move || {
                let (store, doomed) = closing;
                doomed.iter().map(|(_, renders, _)| store.close_paint(renders)).collect::<Vec<_>>()
            },
            move |closed| {
                let mut deletion = Deletion::default();
                for ((stack, renders, failures), closed) in doomed.iter().zip(closed) {
                    deletion.0.extend(store.delete_stack(stack, renders, failures, closed)?);
                }
                Ok(deletion)
            },
            move |this, deletion, window, cx| {
                this.forget(&ids, window, cx);
                for stack in &stacks {
                    this.stacks.remove(stack);
                }
                cx.emit(RendersEvent::StacksDeleted(stacks.clone()));
                if !deletion.is_empty() {
                    cx.emit(RendersEvent::MovedToDeleted(deletion));
                }
            },
            window,
            cx,
        );
    }

    /// Delete renders and failed stacks `ids` into Recently Deleted without
    /// asking (ADR-140), with an Undo notice. Source images are never touched.
    pub(crate) fn delete(
        &mut self,
        ids: Vec<RenderId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete_renders(ids, true, window, cx);
    }

    /// [`Self::delete`] without notice: a batch's restacks replaced these.
    pub(crate) fn delete_replaced(
        &mut self,
        ids: Vec<RenderId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete_renders(ids, false, window, cx);
    }

    fn delete_renders(
        &mut self,
        ids: Vec<RenderId>,
        said: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ids.is_empty() {
            return;
        }
        let store = self.store.clone();
        // The items are disabled then, but a key or batch end may still ask.
        if !self.can_delete() {
            self.fail(self.why_not_deleted(cx), cx);
            return;
        }
        let (failures, renders): (Vec<RenderId>, Vec<RenderId>) =
            ids.iter().cloned().partition(|id| self.failures.iter().any(|f| f.id == *id));
        // A kept working version on a deleted base moves to its stack's
        // newest left, or none: `run_delete` rereads the stacks.
        let closing = (store.clone(), renders.clone());
        self.run_delete(
            Doomed::Renders,
            move || closing.0.close_paint(&closing.1),
            move |closed| {
                let doomed: Vec<RenderId> = renders.iter().chain(&failures).cloned().collect();
                store.delete_renders(&doomed, closed)
            },
            move |this, deletion, window, cx| {
                this.forget(&ids, window, cx);
                if said && !deletion.is_empty() {
                    cx.emit(RendersEvent::MovedToDeleted(deletion));
                }
            },
            window,
            cx,
        );
    }

    /// Put `ids` back from Recently Deleted, after the writes queued before
    /// ([`Self::start_restore`]).
    pub(crate) fn restore(
        &mut self,
        ids: Vec<DeletedId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ids.is_empty() {
            return;
        }
        self.saves.push_back(SaveRequest::Restore(ids));
        self.pump(window, cx);
        cx.notify();
    }

    /// Restore `ids` on the store thread as the writer, then reread the
    /// renders, failures, stacks and sizes. Stopping part way says why and
    /// shows what came back.
    fn start_restore(&mut self, ids: Vec<DeletedId>, window: &mut Window, cx: &mut Context<Self>) {
        let store = self.store.clone();
        tracing::debug!("delete: restoring {} records", ids.len());
        self.set_writer(Writer::deleting(cx.spawn_in(window, async move |this, cx| {
            let done = work::store(move || {
                let restored = store.restore(&ids);
                let listed = store.list().and_then(|renders| Ok((renders, store.failures()?)));
                (restored, listed)
            })
            .await;
            this.update_in(cx, |this, window, cx| {
                this.set_writer(Writer::Idle);
                let lead = history::restore_failed(crate::words::of(cx));
                match done {
                    Ok((restored, listed)) => {
                        if let Err(e) = restored {
                            this.fail(crate::notices::notice(cx.wording(), &lead, &e), cx);
                        }
                        match listed {
                            Ok((renders, failures)) => {
                                tracing::debug!("delete: restored, the Library read again");
                                this.take_listing(renders, failures, window, cx)
                            }
                            Err(e) => tracing::warn!("couldn't read the Library again: {e}"),
                        }
                    }
                    Err(e) => this.fail(crate::notices::fault(cx.wording(), &lead, &e), cx),
                }
                this.trash_changed(cx);
                this.pump(window, cx);
            })
            .ok();
        })));
        cx.notify();
    }

    /// Empty Recently Deleted (it asks first), after the writes queued before.
    pub(crate) fn empty_deleted(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.saves.push_back(SaveRequest::EmptyDeleted);
        self.pump(window, cx);
        cx.notify();
    }

    /// Remove every Recently Deleted record for good, as the writer; stopping
    /// part way says why.
    fn start_empty(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let store = self.store.clone();
        tracing::debug!("delete: emptying Recently Deleted");
        self.set_writer(Writer::deleting(cx.spawn_in(window, async move |this, cx| {
            let emptied = work::store(move || store.empty_deleted()).await;
            this.update_in(cx, |this, window, cx| {
                this.set_writer(Writer::Idle);
                let lead = history::empty_deleted_failed(crate::words::of(cx));
                match emptied {
                    Ok(Ok(())) => tracing::debug!("delete: Recently Deleted emptied"),
                    Ok(Err(e)) => this.fail(crate::notices::notice(cx.wording(), &lead, &e), cx),
                    Err(e) => this.fail(crate::notices::fault(cx.wording(), &lead, &e), cx),
                }
                this.trash_changed(cx);
                this.pump(window, cx);
            })
            .ok();
        })));
        cx.notify();
    }

    /// Recently Deleted may have changed: the Library rereads it, and the
    /// stacks, sizes and trash are reread.
    fn trash_changed(&mut self, cx: &mut Context<Self>) {
        self.trash_changes += 1;
        self.refresh_stacks(cx);
        cx.notify();
    }

    /// Take a fresh listing: forget what is gone, add what is new (restored).
    fn take_listing(
        &mut self,
        renders: Vec<Saved>,
        failures: Vec<Failure>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let gone: Vec<RenderId> = self
            .renders
            .iter()
            .filter(|r| !renders.iter().any(|n| n.id == r.id))
            .map(|r| r.id.clone())
            .chain(
                self.failures
                    .iter()
                    .filter(|f| !failures.iter().any(|n| n.id == f.id))
                    .map(|f| f.id.clone()),
            )
            .collect();
        self.forget(&gone, window, cx);
        self.add_listed(renders);
        self.failures = failures;
    }

    /// A delete (renders, stacks or Library) runs, or a restore or Empty runs
    /// or waits.
    pub fn is_deleting(&self) -> bool {
        matches!(self.writer, Writer::Deleting { .. })
            || self.saves.iter().any(SaveRequest::deletes)
    }

    /// Delete Library (it asks first): everything the Library keeps, on the
    /// store thread ([`History::delete_library`]), then [`RendersEvent::Deleted`]
    /// and [`RendersEvent::LibraryDeleted`]. Stopping part way says so and
    /// lists what is left. The frames are never touched.
    pub(crate) fn delete_library(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let store = self.store.clone();
        if !self.can_delete() {
            self.fail(self.why_not_deleted(cx), cx);
            return;
        }
        tracing::info!(
            "deleting the Library: {} renders, {} failed stacks",
            self.renders.len(),
            self.failures.len()
        );
        let ids: Vec<RenderId> = self
            .renders
            .iter()
            .map(|r| r.id.clone())
            .chain(self.failures.iter().map(|f| f.id.clone()))
            .collect();
        let closing = store.clone();
        self.run_delete(
            Doomed::Library,
            move || closing.close_all_paint(),
            move |closed| store.delete_library(closed).map(|()| Deletion::default()),
            move |this, _, window, cx| {
                // A star toggled while it ran is of a render gone.
                this.saves.retain(|s| !matches!(s, SaveRequest::Pick { .. }));
                this.forget(&ids, window, cx);
                this.stacks.clear();
                this.sizes.clear();
                cx.emit(RendersEvent::LibraryDeleted);
            },
            window,
            cx,
        );
    }

    /// Run a delete as the writer: `close` closes the paint of what goes on a
    /// worker (it waits for the store thread, so never on it), then `delete`
    /// runs on the store thread in order with earlier writes. Done, `done`
    /// drops what went; stopped part way (error or panic), it says so and
    /// rereads the Library.
    pub(super) fn run_delete<C: Send + 'static>(
        &mut self,
        doomed: Doomed,
        close: impl FnOnce() -> C + Send + 'static,
        delete: impl FnOnce(C) -> Result<Deletion, StoreError> + Send + 'static,
        done: impl FnOnce(&mut Self, Deletion, &mut Window, &mut Context<Self>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        tracing::debug!("delete: deleting {doomed:?}");
        self.set_writer(Writer::deleting(cx.spawn_in(window, async move |this, cx| {
            let removed = match work::write(doomed.worker(), close).await {
                Ok(closed) => work::store(move || delete(closed)).await,
                Err(e) => Err(e),
            };
            this.update_in(cx, |this, window, cx| {
                this.set_writer(Writer::Idle);
                let stopped = match removed {
                    Ok(Ok(deletion)) => Ok(deletion),
                    Ok(Err(e)) => {
                        let lead = doomed.failed(cx.wording());
                        Err(crate::notices::notice(cx.wording(), &lead, &e))
                    }
                    Err(e) => {
                        let lead = doomed.unsure(cx.wording());
                        Err(crate::notices::fault(cx.wording(), &lead, &e))
                    }
                };
                match stopped {
                    Ok(deletion) => {
                        tracing::debug!(
                            "delete: {doomed:?} deleted, {} records in Recently Deleted",
                            deletion.0.len()
                        );
                        done(this, deletion, window, cx);
                        this.trash_changed(cx);
                        this.pump(window, cx);
                    }
                    Err(why) => {
                        tracing::debug!("delete: {doomed:?} stopped, reading the Library again");
                        this.fail(why, cx);
                        this.trash_changed(cx);
                        this.reread(window, cx);
                    }
                }
            })
            .ok();
        })));
        cx.notify();
    }

    /// Reread the history after a partial delete, as the writer until
    /// reconciled so no queued save lands in between.
    fn reread(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let store = self.store.clone();
        self.set_writer(Writer::deleting(cx.spawn_in(window, async move |this, cx| {
            let listed = work::try_run(
                work::Lane::Interactive,
                work::Key::kind("library-reread"),
                work::root(),
                move || Ok::<_, StoreError>((store.list()?, store.failures()?)),
            )
            .await;
            this.update_in(cx, |this, window, cx| {
                this.set_writer(Writer::Idle);
                if let Ok((renders, failures)) = listed {
                    this.take_listing(renders, failures, window, cx);
                }
                this.refresh_stacks(cx);
                this.pump(window, cx);
                cx.notify();
            })
            .ok();
        })));
    }

    /// Drop deleted renders from the strip; the app takes one off screen
    /// ([`RendersEvent::Deleted`]).
    fn forget(&mut self, ids: &[RenderId], window: &mut Window, cx: &mut Context<Self>) {
        if ids.is_empty() {
            return;
        }
        for thumbnail in self.thumbnails.forget(ids) {
            cx.drop_image(thumbnail, Some(window));
        }
        self.renders.retain(|r| !ids.contains(&r.id));
        self.failures.retain(|f| !ids.contains(&f.id));
        let gone = |slot: &Option<RenderId>| slot.as_ref().is_some_and(|r| ids.contains(r));
        if self.pending_id().is_some_and(|r| ids.contains(r)) {
            self.cancel_open();
        }
        let compared = gone(&self.compare);
        if compared {
            self.compare = None;
        }
        cx.emit(RendersEvent::Deleted { ids: ids.to_vec(), compared });
    }
}

/// A delete's worker name and what it says when it stops part way.
#[derive(Clone, Copy, Debug)]
pub(super) enum Doomed {
    Renders,
    Stacks,
    Library,
}

impl Doomed {
    fn worker(self) -> &'static str {
        match self {
            Doomed::Renders => "history-delete",
            Doomed::Stacks => "stack-delete",
            Doomed::Library => "library-delete",
        }
    }

    /// It failed part way.
    fn failed(self, w: &DesktopWords) -> String {
        match self {
            Doomed::Renders => say!(w, "renders-delete-failed"),
            Doomed::Stacks => say!(w, "renders-delete-stacks-failed"),
            Doomed::Library => history::library_delete_failed(w.shared()),
        }
    }

    /// It ended unexpectedly, perhaps part way.
    fn unsure(self, w: &DesktopWords) -> String {
        match self {
            Doomed::Renders => say!(w, "renders-delete-unsure"),
            Doomed::Stacks => say!(w, "renders-delete-stacks-unsure"),
            Doomed::Library => say!(w, "library-delete-unsure"),
        }
    }
}

/// A write waiting its turn.
pub(super) enum SaveRequest {
    Render {
        /// Every file listed when it ran, in order.
        frames: Vec<PathBuf>,
        stack: Arc<CompletedStack>,
        thumbnail: RgbImage,
        /// The stack it is part of.
        of: StackId,
        /// The base of the working version it was stacked from.
        from: Option<RenderId>,
        batch: Option<Batch>,
    },
    Pick {
        render: RenderId,
        picked: bool,
    },
    /// A batch stack that failed: its record in stack `of` reworded.
    Failure {
        label: String,
        frames: Vec<PathBuf>,
        of: StackId,
        settings: Settings,
        batch: Batch,
        error: String,
    },
    /// A batch's jobs as it starts, each an unstacked failure in its stack.
    Unstacked {
        jobs: Vec<Job>,
        of: Vec<StackId>,
        settings: Settings,
        batch: Batch,
        why: String,
    },
    /// The batch running ended: its stacks not stacked yet are shown.
    BatchEnded,
    /// At launch, once read: look for moved roots
    /// ([`deptheos_app::Frames::find_again`]), resolving bookmarks first
    /// ([`crate::bookmarks::resolve`]). First in the queue.
    FindAgain,
    /// Locate…: `old` moved to `new`. Queued, so a render saved before it is
    /// reread from the new place.
    Relink {
        old: PathBuf,
        new: PathBuf,
    },
    /// Put these records back; runs as a delete does ([`Renders::start_restore`]).
    Restore(Vec<DeletedId>),
    /// Empty Recently Deleted ([`Renders::start_empty`]).
    EmptyDeleted,
}

impl SaveRequest {
    /// Whether it runs as a delete: a restore or Empty.
    pub(super) fn deletes(&self) -> bool {
        matches!(self, Self::Restore(_) | Self::EmptyDeleted)
    }

    /// What its failure says, before why.
    pub(super) fn failed(&self, w: &DesktopWords) -> String {
        match self {
            SaveRequest::Render { .. } => say!(w, "render-save-failed"),
            SaveRequest::Pick { picked: true, .. } => say!(w, "render-star-failed"),
            SaveRequest::Pick { picked: false, .. } => say!(w, "render-unstar-failed"),
            SaveRequest::Failure { .. } => say!(w, "render-failure-save-failed"),
            SaveRequest::Unstacked { .. } => say!(w, "render-unstacked-save-failed"),
            SaveRequest::BatchEnded => say!(w, "render-failures-read-failed"),
            SaveRequest::Relink { .. } => say!(w, "render-relink-failed"),
            SaveRequest::FindAgain => say!(w, "render-find-again-failed"),
            SaveRequest::Restore(_) => history::restore_failed(w.shared()),
            SaveRequest::EmptyDeleted => history::empty_deleted_failed(w.shared()),
        }
    }
}

/// What a finished save hands back.
enum Done {
    Saved(Box<(Saved, deptheos_app::CpuImage, Arc<CompletedStack>, Option<RenderId>)>),
    Failed(Box<Failure>),
    /// Every failure, read again.
    Failures(Vec<Failure>),
    /// What a relink moved, what Locate… found, and every render and failure
    /// after it.
    Relinked {
        moves: deptheos_app::Moves,
        said: Option<(PathBuf, Matched)>,
        listed: Vec<Saved>,
        failures: Vec<Failure>,
    },
    Nothing,
}

/// Thumbnails of `ids` at the strip's size. Runs on a worker.
pub(super) fn thumbnails(store: &History, ids: &[RenderId]) -> Vec<(RenderId, CpuImage)> {
    ids.iter()
        .filter_map(|id| match store.load_thumbnail(id) {
            Ok(image) => Some((id.clone(), strip_thumbnail(&image))),
            Err(e) => {
                tracing::warn!("no thumbnail for render {}: {e}", id.as_str());
                None
            }
        })
        .collect()
}

/// A stored thumbnail at the strip's size, in the display's BGRA.
pub(super) fn strip_thumbnail(image: &RgbImage) -> CpuImage {
    let small;
    let image = if image.height() > THUMB_DECODE_HEIGHT {
        let width = (image.width() as f32 * THUMB_DECODE_HEIGHT as f32 / image.height() as f32)
            .round()
            .max(1.0) as u32;
        small = image::imageops::thumbnail(image, width, THUMB_DECODE_HEIGHT);
        &small
    } else {
        image
    };
    CpuImage::from_rgb(image)
}
