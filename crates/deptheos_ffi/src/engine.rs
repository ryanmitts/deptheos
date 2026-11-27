//! The engine: the GPU, the history and the renders' records it keeps.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use deptheos_app::Adjustments;
use deptheos_app::engine::work::Lane;
use deptheos_app::library::history::{
    Batch, History, Made, PaintClosed, Render, RenderId, StackId, StoreError, Unsaved,
};
use deptheos_app::library::stacks::{self, Compared, StackInfo, StackReads, Working};
use deptheos_app::preferences::AlignmentPreferences;
use deptheos_app::runs::session::Progress;
use deptheos_app::words::ios::Line;
use deptheos_app::{
    CompletedStack, Engine as AppEngine, Explain, Platform, RawBackend, SessionOptions,
    Words as AppWords,
};

use crate::request::{Ask, on_pool, on_store, on_thread};
use crate::{
    DeletedInfo, DisplayImage, EngineError, FailureInfo, FrameDecoder, HostAdapter, Records,
    RenderInfo, RendersDeleted, Request, Result, SetupSectionInfo, ShowingInfo, ShowingKind,
    ShowingsInfo, StackObserver, StackProgress, StackResult, StackSettings, Told, UnstackedInfo,
    Words, WorkingInfo, deleted_ids, level_image, logging, render_id, render_ids, rgb_to_display,
    stack_id, stack_options,
};

/// The engine: one headless GPU context (Metal through wgpu) and the
/// history. One per app.
#[derive(uniffi::Object)]
pub struct Engine {
    pub(crate) engine: Arc<AppEngine>,
    pub(crate) history: Arc<History>,
    /// The renders' records as last read, by id, for [`Self::showing`] and
    /// [`Self::render_setup`] to compare without rereading the history. A
    /// saved render's frames and settings never change but by a relink,
    /// after which all are reread.
    records: Mutex<Records>,
    /// Settings ▸ Folders ([`Self::read_folders`]) rereads only those
    /// changed since.
    stack_reads: Mutex<StackReads>,
}

#[uniffi::export]
impl Engine {
    /// Start the engine: install `decoder` for every non-DNG frame, open the
    /// GPU, and keep the history under `history_directory`. A frame under
    /// `app_directory` (the app's container) is kept relative to it, so it
    /// is found after the container moves (a reinstall, a restore). Fails
    /// with no GPU; every Metal device stacks, the Simulator's too
    /// (ADR-110).
    ///
    /// Sentences are worded in `language`, the one iOS chose for the app
    /// (`Bundle.main.preferredLocalizations.first`, such as "en" or
    /// "pt-BR"), or English when the engine lacks it. Opened on the pool:
    /// the GPU and history take a moment.
    #[uniffi::constructor]
    pub async fn new(
        decoder: Arc<dyn FrameDecoder>,
        history_directory: PathBuf,
        app_directory: Option<PathBuf>,
        language: String,
    ) -> Result<Arc<Self>> {
        // The app installs the log at launch; without that, stderr only.
        logging::install_fallback();
        let words = AppWords::new(&[language], Platform::Ios);
        let open = move || {
            // One decoder for the process: a later engine's replaces this one.
            deptheos_core::io::set_host_decoder(Arc::new(HostAdapter(decoder)));
            // Led by what didn't start: the GPU or the Library (in use, from a
            // newer version, damaged, unreadable). The start checks the GPU's
            // shared buffers, so a device out of memory fails here, not at
            // every stack.
            let engine = AppEngine::new(words.clone())
                .map_err(|e| EngineError::led(Line::GpuDidntStart, &e, &words))?;
            let history = History::open_in(history_directory, app_directory)
                .map_err(|e| EngineError::led(Line::LibraryDidntOpen, &e, &words))?;
            let history = Arc::new(history);
            Ok(Arc::new(Self {
                engine,
                history,
                records: Mutex::default(),
                stack_reads: Mutex::default(),
            }))
        };
        let english = AppWords::english(Platform::Ios);
        on_pool(&english, &Request::lasting(), Lane::Interactive, Ask::Own("engine-start"), open)
            .await
    }

    /// The language this engine words its sentences in.
    pub fn words(&self) -> Arc<Words> {
        Arc::new(Words { words: self.engine.words().clone() })
    }

    /// Stack `paths` (in focus order) on a stack thread of its own until
    /// `request` is canceled; all DNG or all other formats, never mixed.
    /// Grid search's limits and exposure compensation come from the
    /// preferences and are recorded with the render, as on the desktop;
    /// they never make a working version edited.
    #[allow(clippy::too_many_arguments)]
    pub async fn stack(
        &self,
        paths: Vec<PathBuf>,
        settings: StackSettings,
        alignment: AlignmentPreferences,
        live_side: u32,
        request: Arc<Request>,
        observer: Arc<dyn StackObserver>,
    ) -> Result<Arc<StackResult>> {
        // The viewer shows the live map for every stack, a batch's too.
        let options = SessionOptions { live_map: true, ..stack_options(&settings, alignment) };
        let config = settings.algorithm.config();
        let mut progress = Progress::new(paths.len() as u32, config.needs_blend());
        let engine = self.engine.clone();
        on_thread(self.engine.words(), "stack", &request, move |cancel| {
            let presented = engine
                .stack(
                    paths,
                    config,
                    options,
                    &Adjustments::NEUTRAL,
                    live_side.max(64),
                    cancel,
                    |event| {
                        progress.on(event);
                        observer.progress(StackProgress::from(&progress));
                    },
                    |map| {
                        if let Some(image) = level_image(&map, live_side) {
                            observer.live_map(image);
                        }
                    },
                )
                .told(engine.words())?;
            Ok(StackResult::new(engine, presented))
        })
        .await
    }

    // ---- the history

    /// Every render, oldest first.
    pub async fn list_renders(self: Arc<Self>, request: Arc<Request>) -> Result<Vec<RenderInfo>> {
        self.pool(&request, Lane::Interactive, Ask::Own("library-list"), |e| {
            let renders = e.told(e.history.list())?;
            *e.records() = Records::listed(&renders);
            Ok(e.infos(&renders, &renders))
        })
        .await
    }

    /// Keep `result` as a new render, the next number in `stack` (the
    /// working version's, or a batch stack's [`new_stack_id`]). `frames` is
    /// every file listed when it ran, in list order; `made` what it was
    /// stacked from, which says what adjustments it starts with. The thumbnail comes from the display; then the result lets
    /// go of its map's levels, so the encode holds only the display (the map
    /// picture is remade when asked). Saved as the desktop does
    /// ([`History::save`]): encoded on the pool, written on the store
    /// thread, never canceled.
    pub async fn save_render(
        self: Arc<Self>,
        frames: Vec<PathBuf>,
        result: Arc<StackResult>,
        stack: String,
        made: crate::RenderMade,
    ) -> Result<RenderInfo> {
        let of = self.stack_id(&stack)?;
        let made = match made {
            crate::RenderMade::Stacked { base } => {
                Made::Stacked { base: base.as_deref().map(|id| self.render_id(id)).transpose()? }
            }
            crate::RenderMade::Batch { batch } => Made::Batch(batch),
        };
        let words = self.engine.words();
        let thumbnail = result.thumbnail().ok_or_else(|| {
            EngineError::fault(
                words,
                Some(Line::SaveFailed),
                "the result has no display to thumbnail",
            )
        })?;
        // Not shown until asked for (later, or never), so not held through
        // the encode.
        result.release_map();
        let engine = self.engine.clone();
        let saving = self.history.save(frames, result.stack.clone(), thumbnail, of, made, engine);
        let render = match saving.await {
            Ok(render) => render,
            Err(Unsaved::Failed(e)) => return self.told(Err(e)),
            Err(Unsaved::Panicked(e)) => {
                return Err(EngineError::fault(words, Some(Line::SaveFailed), e));
            }
        };
        // After the save, in order with the writes after it.
        self.store(move |e| {
            // Named with its stack: by its given name, read from the stack's
            // file so a rename counts, else by its first render's folder,
            // among the records kept since the last listing (relisting for
            // each of a batch's renders would read every file each time).
            let stack: Vec<Render> = {
                let mut records = e.records();
                records.keep(&render);
                records.of_stack(&render.stack)
            };
            let info = e.infos(std::slice::from_ref(&render), &stack).pop();
            Ok(info.expect("one render in, one out"))
        })
        .await
    }

    /// Every batch stack that failed, oldest first.
    pub async fn list_failures(self: Arc<Self>, request: Arc<Request>) -> Result<Vec<FailureInfo>> {
        self.pool(&request, Lane::Interactive, Ask::Own("library-failures"), |e| {
            let failures = e.told(e.history.failures())?;
            Ok(failures.iter().map(FailureInfo::from).collect())
        })
        .await
    }

    /// Keep a failed batch stack: the `frames` and `settings` it had, its
    /// `batch`, and why, in `stack`, rewording its unstacked record
    /// ([`Self::record_unstacked`]).
    pub async fn save_failure(
        self: Arc<Self>,
        label: String,
        frames: Vec<PathBuf>,
        settings: StackSettings,
        stack: String,
        batch: Batch,
        error: String,
    ) -> Result<FailureInfo> {
        let settings =
            stacks::Settings { config: settings.algorithm.config(), options: settings.options() };
        self.store(move |e| {
            let stack = e.stack_id(&stack)?;
            let saved = e.history.save_failure(&label, &frames, &settings, &stack, &batch, &error);
            let failure = e.told(saved)?;
            Ok(FailureInfo::from(&failure))
        })
        .await
    }

    /// Keep each of a batch's `jobs` as it starts as a failed stack saying
    /// `why`, in its stack from `stacks`: hidden from
    /// [`Self::list_failures`] until it fails or [`Self::batch_ended`],
    /// removed as its render saves, and shown next launch if the app ends
    /// first.
    pub async fn record_unstacked(
        self: Arc<Self>,
        jobs: Vec<crate::BatchJob>,
        stacks: Vec<String>,
        settings: StackSettings,
        batch: Batch,
        why: String,
    ) -> Result<()> {
        let settings =
            stacks::Settings { config: settings.algorithm.config(), options: settings.options() };
        self.store(move |e| {
            let stacks: Vec<StackId> =
                stacks.iter().map(|s| e.stack_id(s)).collect::<Result<_>>()?;
            let jobs: Vec<(&str, &[PathBuf], &StackId)> = jobs
                .iter()
                .zip(&stacks)
                .map(|(job, of)| (job.label.as_str(), job.frames.as_slice(), of))
                .collect();
            e.told(e.history.record_unstacked(&jobs, &settings, &batch, &why))
        })
        .await
    }

    /// The running batch ended: its unstacked stacks are listed from here.
    /// Every failure, read after the run's writes, so in the background too.
    pub async fn batch_ended(self: Arc<Self>) -> Result<Vec<FailureInfo>> {
        self.store(move |e| {
            e.history.batch_ended();
            let failures = e.told(e.history.failures())?;
            Ok(failures.iter().map(FailureInfo::from).collect())
        })
        .await
    }

    /// Load a render to show, with its display and map until
    /// [`StackResult::pictures`]. The latest wins: a newer one cancels it.
    pub async fn load_render(
        self: Arc<Self>,
        request: Arc<Request>,
        id: String,
    ) -> Result<Arc<StackResult>> {
        let open = Ask::Latest("render-open", &self.engine);
        self.pool(&request, Lane::Interactive, open, move |e| {
            let stack = e.read_render(&id)?;
            let presented = e.told(e.engine.present_stack(stack, &Adjustments::NEUTRAL))?;
            Ok(StackResult::new(e.engine.clone(), presented))
        })
        .await
    }

    /// Load a render's pixels only, to export, compare or paint from: no
    /// display or map (a picture asked of it is made from the stack), which
    /// for a linear stack skips a GPU render.
    pub async fn load_render_pixels(
        self: Arc<Self>,
        request: Arc<Request>,
        id: String,
    ) -> Result<Arc<StackResult>> {
        self.pool(&request, Lane::Interactive, Ask::Own("render-pixels"), move |e| {
            Ok(StackResult::pixels_only(e.engine.clone(), e.read_render(&id)?))
        })
        .await
    }

    /// Render `id`'s list (left-out frames excluded) and settings as a
    /// working version: Revert to it.
    pub async fn working_of_render(
        self: Arc<Self>,
        request: Arc<Request>,
        id: String,
    ) -> Result<WorkingInfo> {
        self.pool(&request, Lane::Interactive, Ask::Own("render-working"), move |e| {
            let render = e.told(e.history.render(&e.render_id(&id)?))?;
            Ok(WorkingInfo::from(&Working::of_render(&render)))
        })
        .await
    }

    /// As saved with it.
    pub async fn render_thumbnail(
        self: Arc<Self>,
        request: Arc<Request>,
        id: String,
    ) -> Result<DisplayImage> {
        self.pool(&request, Lane::Background, Ask::Own("render-thumbnail"), move |e| {
            let rgb = e.told(e.history.load_thumbnail(&e.render_id(&id)?))?;
            Ok(rgb_to_display(&rgb))
        })
        .await
    }

    /// Move renders and failed batch stacks into Recently Deleted
    /// (`History::delete_renders`), one record per stack. Returns the
    /// records, for the notice's Undo ([`Self::restore_deleted`]), and the
    /// working version's `base` now: if deleted, the stack's newest render
    /// left (`stacks::newest_of`), so Edited and Revert still have one. The
    /// stacks stay, with their names.
    pub async fn delete_renders(
        self: Arc<Self>,
        ids: Vec<String>,
        base: Option<String>,
    ) -> Result<RendersDeleted> {
        let ids = self.render_ids(&ids)?;
        let base = base.map(|b| self.render_id(&b)).transpose()?;
        self.records().forget(&ids);
        let closed = self.close_paint(Some(ids.clone())).await?;
        self.store(move |e| {
            let gone = base.as_ref().filter(|b| ids.contains(b));
            let base_stack = gone.and_then(|b| e.history.render(b).ok()).map(|r| r.stack);
            let deletion = e.told(e.history.delete_renders(&ids, closed))?;
            let base = match base_stack {
                Some(stack) => {
                    let left = e.told(e.history.list())?;
                    stacks::newest_of(&stack, &left).map(|r| r.id.clone())
                }
                None => base,
            };
            Ok(RendersDeleted {
                records: deletion.0.iter().map(DeletedInfo::from).collect(),
                base: base.map(|b| b.as_str().to_string()),
            })
        })
        .await
    }

    /// Move a stack into Recently Deleted as one record: its `renders` and
    /// `failures`, kept working version and name (none when nothing was
    /// there). The frames aren't touched; a working version on screen of
    /// this stack is the app's to replace.
    pub async fn delete_stack(
        self: Arc<Self>,
        stack: String,
        renders: Vec<String>,
        failures: Vec<String>,
    ) -> Result<Vec<DeletedInfo>> {
        let (stack, renders, failures) =
            (self.stack_id(&stack)?, self.render_ids(&renders)?, self.render_ids(&failures)?);
        self.records().forget(&renders);
        let closed = self.close_paint(Some(renders.clone())).await?;
        self.store(move |e| {
            let deleted = e.told(e.history.delete_stack(&stack, &renders, &failures, closed))?;
            Ok(deleted.iter().map(DeletedInfo::from).collect())
        })
        .await
    }

    /// Recently Deleted, newest first.
    pub async fn deleted(self: Arc<Self>, request: Arc<Request>) -> Result<Vec<DeletedInfo>> {
        self.pool(&request, Lane::Interactive, Ask::Own("deleted"), move |e| {
            Ok(e.told(e.history.deleted())?.iter().map(DeletedInfo::from).collect())
        })
        .await
    }

    /// Put Recently Deleted records back whole (Undo, Restore): renders,
    /// failed runs, and for a stack its name and working version. List the
    /// history again to see them.
    pub async fn restore_deleted(self: Arc<Self>, ids: Vec<String>) -> Result<()> {
        let ids = self.told(deleted_ids(&ids))?;
        self.store(move |e| e.told(e.history.restore(&ids))).await
    }

    /// For good (Settings ▸ Storage, which asks first).
    pub async fn empty_deleted(self: Arc<Self>) -> Result<()> {
        self.store(|e| e.told(e.history.empty_deleted())).await
    }

    /// Its newest render's.
    pub async fn deleted_thumbnail(
        self: Arc<Self>,
        request: Arc<Request>,
        id: String,
    ) -> Result<DisplayImage> {
        let id = self.told(deleted_ids(&[id]))?.remove(0);
        self.pool(&request, Lane::Background, Ask::Own("deleted-thumbnail"), move |e| {
            let rgb = e.told(e.history.deleted_thumbnail(&id))?;
            Ok(rgb_to_display(&rgb))
        })
        .await
    }

    /// Delete the whole Library as listed (`History::delete_library`): every
    /// stack with its renders and retouching, failures, name and working
    /// version, and the saved working version. What the listing doesn't
    /// know stays; frames, Imports copies and exports aren't touched. The
    /// working version on screen is the app's to replace, the paint
    /// worker's renders its to forget. One stopping part way says why once
    /// the rest is gone: relist to see what is left.
    pub async fn delete_library(self: Arc<Self>) -> Result<()> {
        let closed = self.close_paint(None).await?;
        self.store(|e| {
            let removed = e.history.delete_library(closed);
            e.records().forget_all();
            e.told(removed)
        })
        .await
    }

    /// With none (or a blank name), back to its folder's.
    pub async fn rename_render(self: Arc<Self>, id: String, name: Option<String>) -> Result<()> {
        let id = self.render_id(&id)?;
        let name = name.filter(|n| !n.trim().is_empty());
        self.store(move |e| e.told(e.history.rename_render(&id, name.as_deref()))).await
    }

    /// Or with `picked` false take its star away.
    pub async fn set_render_picked(self: Arc<Self>, id: String, picked: bool) -> Result<()> {
        let id = self.render_id(&id)?;
        self.store(move |e| e.told(e.history.set_render_picked(&id, picked))).await
    }

    // ---- stacks and their working versions

    /// The working version on screen as left: an empty one in a new stack
    /// when there is none.
    pub async fn read_working(self: Arc<Self>, request: Arc<Request>) -> Result<WorkingInfo> {
        let read = |e: &Engine| Ok(WorkingInfo::from(&e.told(e.history.read_working())?));
        self.pool(&request, Lane::Interactive, Ask::Own("working-read"), read).await
    }

    /// Keep `working` as the one on screen for the next launch: written on
    /// the store thread once it stands, only the newest of a run of changes,
    /// never after a stack switch (`History::keep_working`).
    /// [`Self::written`] writes it at once.
    pub fn keep_working(&self, working: WorkingInfo) -> Result<()> {
        let working = self.told(Working::try_from(working))?;
        self.history.keep_working(working);
        Ok(())
    }

    /// Waits up to `timeout_ms` for every Library write sent to be on disk,
    /// as the app goes to the background; whether nothing was left.
    pub async fn written(self: Arc<Self>, timeout_ms: u32) -> Result<bool> {
        let timeout = std::time::Duration::from_millis(timeout_ms.into());
        let flush = move |_: &Engine| Ok(deptheos_app::engine::store::store().flush(timeout));
        let request = Request::lasting();
        self.pool(&request, Lane::Interactive, Ask::Own("store-flush"), flush).await
    }

    /// Put stack `to` on screen in place of `outgoing`, kept for its own
    /// stack, and return `to`'s working version: its kept one, else its
    /// newest render's, else `fallback` ([`failure_working`]). `None`, with
    /// nothing changed, when none. Within one stack `outgoing` stays.
    pub async fn switch_stack(
        self: Arc<Self>,
        outgoing: WorkingInfo,
        to: String,
        fallback: Option<WorkingInfo>,
    ) -> Result<Option<WorkingInfo>> {
        let outgoing = self.told(Working::try_from(outgoing))?;
        let fallback = self.told(fallback.map(Working::try_from).transpose())?;
        let to = self.stack_id(&to)?;
        self.store(move |e| {
            let renders = e.told(e.history.list())?;
            let incoming = e.told(e.history.switch_stack(&outgoing, &to, &renders, fallback))?;
            Ok(incoming.as_ref().map(WorkingInfo::from))
        })
        .await
    }

    /// In bytes, by render id.
    pub async fn render_sizes(
        self: Arc<Self>,
        request: Arc<Request>,
    ) -> Result<std::collections::HashMap<String, u64>> {
        self.pool(&request, Lane::Background, Ask::Own("render-sizes"), |e| {
            let sizes = e.history.render_sizes();
            Ok(sizes.into_iter().map(|(id, bytes)| (id.as_str().to_string(), bytes)).collect())
        })
        .await
    }

    /// Stacks with frames kept and no render yet, by name, but the one on
    /// screen.
    pub async fn list_unstacked(
        self: Arc<Self>,
        request: Arc<Request>,
    ) -> Result<Vec<UnstackedInfo>> {
        self.pool(&request, Lane::Background, Ask::Own("library-unstacked"), |e| {
            let (stacks, renders) = (e.stacks()?, e.told(e.history.list())?);
            Ok(deptheos_app::library::stacks::unstacked(&stacks, &renders)
                .into_iter()
                .map(|(stack, info)| UnstackedInfo {
                    stack: stack.as_str().to_string(),
                    name: info.unstacked_name(),
                    frames: info.working.as_ref().map_or(0, |w| w.list.shots().len() as u32),
                })
                .collect())
        })
        .await
    }

    /// The frame folders the Library still uses: every folder a Library
    /// file names, Recently Deleted's included, and every one named this
    /// session (`History::folders_in_use`). `None` when a file couldn't be
    /// read: then no copy may be called unused.
    pub async fn folders_in_use(
        self: Arc<Self>,
        request: Arc<Request>,
    ) -> Result<Option<Vec<PathBuf>>> {
        self.pool(&request, Lane::Background, Ask::Own("folders-in-use"), |e| {
            Ok(e.history.folders_in_use())
        })
        .await
    }

    /// With none (or a blank name), back to its folder's.
    pub async fn rename_stack(self: Arc<Self>, stack: String, name: Option<String>) -> Result<()> {
        let stack = self.stack_id(&stack)?;
        self.store(move |e| e.told(e.history.rename_stack(&stack, name.as_deref()))).await
    }

    /// How `working` stands against `shown` (by id; `None` for none) and
    /// against each of `renders` as if shown, in one ask: base or other,
    /// edited or not, the Revert line and what Revert would do, in the
    /// engine's words, the list sent once. Compared as iOS edits settings
    /// ([`Compared::ios`]). A render or base gone from the history counts
    /// as none. On the pool: it can read records and the list's DNGs.
    pub async fn showings(
        self: Arc<Self>,
        request: Arc<Request>,
        working: WorkingInfo,
        shown: Option<String>,
        renders: Vec<String>,
    ) -> Result<ShowingsInfo> {
        self.pool(&request, Lane::Interactive, Ask::Own("showings"), move |e| {
            Ok(e.showings_of(working, shown, renders))
        })
        .await
    }

    /// Render `id`'s record for Render Info by section, with the working
    /// version's value under each row that differs (as iOS compares).
    pub async fn render_setup(
        self: Arc<Self>,
        request: Arc<Request>,
        id: String,
        working: WorkingInfo,
    ) -> Result<Vec<SetupSectionInfo>> {
        self.pool(&request, Lane::Interactive, Ask::Own("render-setup"), move |e| {
            let render = e.told(e.record(&e.render_id(&id)?))?;
            let working = e.told(Working::try_from(working))?;
            let sections =
                stacks::render_setup(&render, &working, compared(&working), e.engine.words());
            Ok(sections.into_iter().map(SetupSectionInfo::from).collect())
        })
        .await
    }
}

/// The decoder is compared only while the list has a frame it decides
/// (ADR-131).
fn compared(working: &Working) -> Compared {
    Compared::ios(working.list.files.iter().any(|f| RawBackend::chooses(f)))
}

impl Engine {
    /// `result`, its error told in the engine's words.
    pub(crate) fn told<T>(&self, result: std::result::Result<T, impl Explain>) -> Result<T> {
        result.told(self.engine.words())
    }

    /// As the engine gave it to the app.
    fn render_id(&self, id: &str) -> Result<RenderId> {
        self.told(render_id(id))
    }

    fn render_ids(&self, ids: &[String]) -> Result<Vec<RenderId>> {
        self.told(render_ids(ids))
    }

    /// As the engine gave it to the app.
    fn stack_id(&self, id: &str) -> Result<StackId> {
        self.told(stack_id(id))
    }

    /// `f` with the engine on the pool in `lane`, for `request`.
    pub(crate) async fn pool<T: Send + 'static>(
        self: &Arc<Self>,
        request: &Arc<Request>,
        lane: Lane,
        ask: Ask<'_>,
        f: impl FnOnce(&Engine) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let engine = self.clone();
        on_pool(self.engine.words(), request, lane, ask, move || f(&engine)).await
    }

    /// Have the paint worker close the files of `ids` (or all), and Adjust
    /// leave them and land their writes, before a delete goes to the store
    /// thread. On the pool, never the store thread: both wait on that
    /// thread, so asking from it would deadlock.
    async fn close_paint(self: &Arc<Self>, ids: Option<Vec<RenderId>>) -> Result<PaintClosed> {
        let close = move |e: &Engine| {
            Ok(match &ids {
                Some(ids) => e.history.close_paint(ids),
                None => e.history.close_all_paint(),
            })
        };
        // A request of its own, never canceled: it is part of a write.
        self.pool(&Request::lasting(), Lane::Interactive, Ask::Own("close-paint"), close).await
    }

    /// `f` with the engine on the store thread, after the writes before it.
    pub(crate) async fn store<T: Send + 'static>(
        self: &Arc<Self>,
        f: impl FnOnce(&Engine) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let engine = self.clone();
        on_store(self.engine.words(), move || f(&engine)).await
    }

    /// Rereading only those changed since ([`Self::stack_reads`]).
    pub(crate) fn stacks(&self) -> Result<std::collections::HashMap<StackId, StackInfo>> {
        let mut reads = self.stack_reads.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        self.told(self.history.stacks_reusing(&mut reads))
    }

    pub(crate) fn records(&self) -> std::sync::MutexGuard<'_, Records> {
        self.records.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The one kept, else read and kept.
    fn record(&self, id: &RenderId) -> std::result::Result<Render, StoreError> {
        if let Some(render) = self.records().get(id) {
            return Ok(render.clone());
        }
        let render = self.history.render(id)?;
        self.records().keep(&render);
        Ok(render)
    }

    /// For what the app asks as it draws: kept, else read once; `None` when
    /// unreadable, remembered until the next listing.
    fn known(&self, id: &RenderId) -> Option<Render> {
        if self.records().is_missing(id) {
            return None;
        }
        match self.record(id) {
            Ok(render) => Some(render),
            Err(e) => {
                tracing::debug!("no record of render {}: {e}", id.as_str());
                self.records().forget(std::slice::from_ref(id));
                None
            }
        }
    }

    /// [`Engine::showings`], on the pool. Records are kept from the last
    /// listing, and an unreadable one isn't retried until the next.
    fn showings_of(
        &self,
        working: WorkingInfo,
        shown: Option<String>,
        renders: Vec<String>,
    ) -> ShowingsInfo {
        let Some(working) = Self::working_shown(working) else {
            let shown = ShowingInfo::nothing(self.engine.words());
            return ShowingsInfo { shown, renders: Default::default() };
        };
        let base = working.base.as_ref().and_then(|id| self.known(id));
        // Once for every render: it can read the list's DNGs.
        let compared = compared(&working);
        let each = renders
            .into_iter()
            .filter_map(|id| {
                let showing = self.showing_of(&working, Some(&id), base.as_ref(), compared);
                (showing.kind != ShowingKind::Nothing).then_some((id, showing))
            })
            .collect();
        ShowingsInfo {
            shown: self.showing_of(&working, shown.as_deref(), base.as_ref(), compared),
            renders: each,
        }
    }

    /// As the engine compares it; `None`, logged, when its ids don't parse.
    fn working_shown(working: WorkingInfo) -> Option<Working> {
        Working::try_from(working).map_err(|e| tracing::warn!("showing: {e}")).ok()
    }

    /// How `working`, based on `base` (its record, if any), stands against
    /// `shown`, `compared` as [`compared`] worked out by the caller.
    fn showing_of(
        &self,
        working: &Working,
        shown: Option<&str>,
        base: Option<&Render>,
        compared: Compared,
    ) -> ShowingInfo {
        let shown = shown.and_then(|id| {
            render_id(id).map_err(|e| tracing::debug!("no record of render {id}: {e}")).ok()
        });
        let shown = shown.and_then(|id| self.known(&id));
        let showing = stacks::Showing::new(working, shown.as_ref(), base, compared);
        ShowingInfo::of(&showing, self.engine.words())
    }

    /// Read from the history.
    fn read_render(&self, id: &str) -> Result<CompletedStack> {
        let id = self.render_id(id)?;
        self.told(self.history.load_render(&id))
    }

    /// As the app sees them, each with its stack's name from `all`.
    fn infos(&self, renders: &[Render], all: &[Render]) -> Vec<RenderInfo> {
        let stacks = self.history.stacks().unwrap_or_else(|e| {
            tracing::warn!("couldn't read the stacks' names: {e}");
            Default::default()
        });
        let mut names: std::collections::HashMap<&StackId, Option<String>> = Default::default();
        renders
            .iter()
            .map(|render| {
                let name = names
                    .entry(&render.stack)
                    .or_insert_with(|| {
                        stacks::stack_name(&render.stack, stacks.get(&render.stack), all)
                    })
                    .clone();
                RenderInfo::of(render, name)
            })
            .collect()
    }
}
