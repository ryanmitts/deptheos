//! Retouching for the iOS app, over the shared paint session
//! (`deptheos_app::paint::PaintSession`) as the desktop drives it.
//!
//! The display crosses as tiles: this mirrors the painted render's display
//! levels and tells the app only which tiles changed, and the app fetches
//! those it shows with [`PaintSession::tile`]. A fast stroke changes a few
//! tiles of every level many times a second; copying all of them would
//! cost more than painting.
//!
//! Of full size it keeps only as many tiles as the app's view wants
//! ([`PaintSession::keep_full_tiles`]), those used last; a missing one is
//! rendered on request and arrives through [`PaintObserver::tiles_changed`].
//!
//! Every call but [`PaintSession::export`], [`PaintSession::save`] and
//! [`PaintSession::tile`] only queues work, so is safe on the main thread.
//! The observer is called in order on whichever thread the session
//! reports from.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use deptheos_app::Engine as AppEngine;
use deptheos_app::engine::present::{CpuImage, TiledLevel};
use deptheos_app::engine::work::Lane;
use deptheos_app::library::export::ExportFormat;
use deptheos_app::library::history::RenderId;
use deptheos_app::paint::{
    Brush, CanvasState, DEFAULT_TOLERANCE, Dab, FLOW_RANGE, Failure, HARDNESS_RANGE, OPACITY_RANGE,
    Observer, OpenRender, RADIUS_RANGE, Report, SourceKind, Status, TOLERANCE_RANGE,
    Told as SourceTold,
};
use deptheos_app::{Adjustments, Look};

use crate::picture::{TileAsk, TileFetched, fetch_tiles};
use crate::request::{Ask, on_pool};
use crate::{
    DisplayImage, Engine, EngineError, ExportPreferences, Picture, Request, Result, StackResult,
    Told, export_target, level_image, shown,
};
use deptheos_app::words::ios::Line;

/// A source's load error as the sentences shown after its lead; details go
/// to the log.
fn source_failure(error: &dyn deptheos_app::Explain, words: &deptheos_app::Words) -> Failure {
    let explained = error.explain(words);
    let fault = explained.kind == deptheos_app::ExplanationKind::Fault;
    Failure { text: shown(&explained, words), fault }
}

/// [`source_failure`], for the session's loaders.
fn teller(engine: &Arc<AppEngine>) -> SourceTold {
    let engine = engine.clone();
    Arc::new(move |error| source_failure(error, engine.words()))
}

/// What a stroke paints from (`deptheos_app::paint::SourceKind`).
#[uniffi::remote(Enum)]
pub enum SourceKind {
    /// The render's own pixels: the eraser.
    Original,
    /// One of the render's frames, at its place in the stack.
    Frame { path: PathBuf },
    /// Another render of the stack, exposure-matched.
    Render { render: String },
}

/// A painted render's display outside paint mode: `image` at most the
/// display side, `picture` the whole at every size, to tile past `image`.
#[derive(uniffi::Record)]
pub struct PaintedDisplay {
    pub image: DisplayImage,
    pub picture: Arc<Picture>,
}

/// The brush, as the desktop's sliders describe it
/// (`deptheos_app::paint::Brush`); a stroke clamps it to their ranges.
#[uniffi::remote(Record)]
pub struct Brush {
    /// In image pixels.
    pub radius: f32,
    pub hardness: f32,
    pub opacity: f32,
    pub flow: f32,
    /// Paint only where the source is within this relative difference of
    /// the original; `None` paints everywhere.
    pub tolerance: Option<f32>,
}

/// A slider's range and where it starts.
#[derive(Clone, Copy, Debug, uniffi::Record)]
pub struct BrushRange {
    pub min: f32,
    pub max: f32,
    pub default_value: f32,
}

/// Every brush setting's range.
#[derive(Clone, Copy, Debug, uniffi::Record)]
pub struct BrushRanges {
    pub radius: BrushRange,
    pub hardness: BrushRange,
    pub opacity: BrushRange,
    pub flow: BrushRange,
    pub tolerance: BrushRange,
}

#[uniffi::export]
pub fn brush_ranges() -> BrushRanges {
    let d = Brush::default();
    let range = |r: std::ops::RangeInclusive<f32>, default_value| BrushRange {
        min: *r.start(),
        max: *r.end(),
        default_value,
    };
    BrushRanges {
        radius: range(RADIUS_RANGE, d.radius),
        hardness: range(HARDNESS_RANGE, d.hardness),
        opacity: range(OPACITY_RANGE, d.opacity),
        flow: range(FLOW_RANGE, d.flow),
        tolerance: range(TOLERANCE_RANGE, DEFAULT_TOLERANCE),
    }
}

/// One pen sample: continuous image coordinates (pixel centers at
/// half-integers) and pressure, 0–1.
#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct PaintSample {
    pub x: f32,
    pub y: f32,
    pub pressure: f32,
}

/// How far off the image a sample may be, in pixels: further ones are
/// pulled in, so a wild sample can't make millions of dabs.
const SAMPLE_REACH: f32 = 1e5;

impl PaintSample {
    /// `None` for a NaN or infinite position.
    fn dab(self) -> Option<Dab> {
        if !(self.x.is_finite() && self.y.is_finite()) {
            return None;
        }
        let pressure = if self.pressure.is_nan() { 1.0 } else { self.pressure.clamp(0.0, 1.0) };
        Some(Dab {
            x: self.x.clamp(-SAMPLE_REACH, SAMPLE_REACH),
            y: self.y.clamp(-SAMPLE_REACH, SAMPLE_REACH),
            pressure,
        })
    }
}

/// One level of the painted display: `1 / 2^index` of full size, in
/// `cols` × `rows` tiles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct PaintLevel {
    pub width: u32,
    pub height: u32,
    pub cols: u32,
    pub rows: u32,
}

/// A display tile that changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct PaintTile {
    pub level: u32,
    pub tx: u32,
    pub ty: u32,
}

/// A tile's side in its level's pixels. It stores one more pixel each side
/// (its neighbors', or its own edge at the border), so it filters as the
/// whole image would.
#[uniffi::export]
pub fn paint_tile_core() -> u32 {
    deptheos_app::engine::present::TILE_CORE
}

/// The canvas of the render open.
#[derive(Clone, Debug, PartialEq, uniffi::Enum)]
pub enum PaintCanvas {
    /// Its tiles not in yet: no strokes.
    Opening,
    /// Strokes paint.
    Ready,
    /// Why it couldn't open: [`PaintSession::retry`].
    Failed { message: String },
}

/// Paint mode on one render, as the app shows it.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct PaintStatus {
    pub render_id: String,
    pub canvas: PaintCanvas,
    /// As picked, shown ahead of the worker, which falls back to what it
    /// had if it can't load it.
    pub selection: SourceKind,
    /// The selection's pixels are in: a stroke starts at once.
    pub ready: bool,
    /// What Frames in the Paint From menu goes back to.
    pub last_frame: Option<PathBuf>,
    /// The frame the depth map is lit for; `None` lights it whole.
    pub lit_frame: Option<PathBuf>,
    pub undo: u32,
    pub redo: u32,
    pub painted: bool,
    /// Why nothing painted can be saved, in full sentences, for a notice
    /// that lasts as long as this.
    pub unsaveable: Option<String>,
    /// The frames and renders some painted pixel came from.
    pub painted_from: Vec<SourceKind>,
}

impl From<&Status> for PaintStatus {
    fn from(s: &Status) -> Self {
        let canvas = match &s.canvas {
            CanvasState::Opening => PaintCanvas::Opening,
            CanvasState::Ready => PaintCanvas::Ready,
            CanvasState::Failed(failure) => PaintCanvas::Failed { message: failure.text.clone() },
        };
        Self {
            render_id: s.key.as_str().to_string(),
            canvas,
            selection: s.selection.clone(),
            ready: s.ready,
            last_frame: s.last_frame.clone(),
            lit_frame: None,
            undo: s.undo as u32,
            redo: s.redo as u32,
            painted: s.painted,
            unsaveable: s.unsaveable.clone(),
            painted_from: s.painted_from.clone(),
        }
    }
}

/// The app's view of the paint, called in order on whichever thread the
/// session reports from. Each names its render.
#[uniffi::export(foreign)]
pub trait PaintObserver: Send + Sync {
    /// `render_id`'s canvas is in: its levels, full size first. Every tile
    /// is new.
    fn opened(&self, render_id: String, levels: Vec<PaintLevel>);
    /// Tiles that changed, or that [`PaintSession::tiles`] lacked and now
    /// has.
    fn tiles_changed(&self, render_id: String, tiles: Vec<PaintTile>);
    /// Paint mode's state changed: [`PaintSession::status`] says how.
    fn state_changed(&self);
    /// Paint mode left `render_id`.
    fn closed(&self, render_id: String);
    /// The answer to [`PaintSession::present`]: the painted display, or
    /// `None` when it shows as stacked (nothing painted, or the paint can't
    /// be shown, which [`Self::failed`] said).
    fn presented(&self, render_id: String, painted: Option<PaintedDisplay>);
    /// The source picked, lined up with `render_id`: what a hold shows, as
    /// `image` and at every size `picture`.
    fn source_shown(
        &self,
        render_id: String,
        source: SourceKind,
        image: DisplayImage,
        picture: Arc<Picture>,
    );
    /// With the render paint mode was on, if any. An open's failure is the
    /// status's canvas, never this.
    fn failed(&self, render_id: Option<String>, message: String);
    /// Something to say briefly over the photo.
    fn notice(&self, render_id: String, message: String);
}

/// The open render's display, as the app last heard of it.
///
/// Lifecycle: none, or the mirror of one render, an `Option` beside the
/// `PaintSession` (`Forward::mirror`): set by each `Report::Canvas`, its
/// tiles patched by `Report::Patches` of its key, `None` at
/// `Report::Closed` of its key.
struct Mirror {
    key: RenderId,
    /// Full size first. Level 0 has no tiles: they are in `full`.
    levels: Vec<TiledLevel>,
    full: FullTiles,
}

/// Some full-size tiles, by `[tx, ty]`.
#[derive(Default)]
struct FullTiles {
    /// Each with when it was last used.
    tiles: HashMap<[u32; 2], (CpuImage, u64)>,
    clock: u64,
    /// Asked of the worker and not in yet.
    requested: HashSet<[u32; 2]>,
}

impl FullTiles {
    fn get(&mut self, at: [u32; 2]) -> Option<&CpuImage> {
        self.clock += 1;
        let (tile, used) = self.tiles.get_mut(&at)?;
        *used = self.clock;
        Some(tile)
    }

    /// Keep `tile` if kept or asked for, and at most `keep`, the least
    /// recently used going.
    fn update(&mut self, at: [u32; 2], tile: CpuImage, keep: usize) {
        if !self.requested.remove(&at) && !self.tiles.contains_key(&at) {
            return;
        }
        self.clock += 1;
        self.tiles.insert(at, (tile, self.clock));
        self.trim(keep);
    }

    /// At most `keep` tiles, those used last.
    fn trim(&mut self, keep: usize) {
        while self.tiles.len() > keep
            && let Some(&oldest) = self.tiles.iter().min_by_key(|(_, (_, used))| *used).map(|e| e.0)
        {
            self.tiles.remove(&oldest);
        }
    }
}

/// The session's reports to the mirror and the observer. The mirror is of
/// the last canvas reported until the session reports leaving it; it
/// reports nothing of a render no longer open.
struct Forward {
    mirror: Arc<Mutex<Option<Mirror>>>,
    observer: Arc<dyn PaintObserver>,
    /// The longest side of the displays the observer is sent.
    side: u32,
    /// As many as the app's view wants at once.
    full_tiles: AtomicUsize,
}

impl Observer for Forward {
    fn report(&self, report: Report) {
        match report {
            Report::Status(_) => self.observer.state_changed(),
            Report::Closed { key } => {
                let mut mirror = crate::lock(&self.mirror);
                if mirror.as_ref().is_some_and(|m| m.key == key) {
                    *mirror = None;
                }
                drop(mirror);
                self.observer.closed(key.as_str().to_string());
            }
            Report::Canvas { key, display } => {
                let levels = display
                    .levels
                    .iter()
                    .map(|l| PaintLevel {
                        width: l.width,
                        height: l.height,
                        cols: l.cols,
                        rows: l.rows,
                    })
                    .collect();
                let mut tiled = display.levels;
                if let Some(full) = tiled.first_mut() {
                    full.tiles = Vec::new();
                }
                let mirror = Mirror { key: key.clone(), levels: tiled, full: FullTiles::default() };
                *crate::lock(&self.mirror) = Some(mirror);
                self.observer.opened(key.as_str().to_string(), levels);
            }
            Report::Patches { key, patches } => {
                let mut changed = Vec::with_capacity(patches.len());
                {
                    let mut mirror = crate::lock(&self.mirror);
                    // A render left: its tiles aren't the mirror's.
                    let Some(mirror) = mirror.as_mut().filter(|m| m.key == key) else { return };
                    for patch in patches {
                        let Some(level) = mirror.levels.get_mut(patch.level) else { continue };
                        if patch.tx >= level.cols || patch.ty >= level.rows {
                            continue;
                        }
                        let tile =
                            PaintTile { level: patch.level as u32, tx: patch.tx, ty: patch.ty };
                        if patch.level == 0 {
                            // Reported even when not kept: the app may show
                            // one it fetched before.
                            let keep = self.full_tiles.load(Ordering::Relaxed);
                            mirror.full.update([patch.tx, patch.ty], patch.tile, keep);
                            changed.push(tile);
                            continue;
                        }
                        let i = (patch.ty * level.cols + patch.tx) as usize;
                        if let Some(kept) = level.tiles.get_mut(i) {
                            *kept = patch.tile;
                            changed.push(tile);
                        }
                    }
                }
                if !changed.is_empty() {
                    self.observer.tiles_changed(key.as_str().to_string(), changed);
                }
            }
            Report::Presented { key, display } => {
                let painted = display.and_then(|display| {
                    let image = level_image(&display, self.side)?;
                    Some(PaintedDisplay { image, picture: Picture::of(display) })
                });
                self.observer.presented(key.as_str().to_string(), painted);
            }
            Report::Source { key, kind, display } => {
                if let Some(image) = level_image(&display, self.side) {
                    let (id, source) = (key.as_str().to_string(), kind);
                    self.observer.source_shown(id, source, image, Picture::shared(display));
                }
            }
            Report::Failed { key, message } => {
                // The failure may be a full-size tile asked for: asked again
                // when next wanted, not waited on forever.
                if let Some(mirror) = crate::lock(&self.mirror).as_mut() {
                    mirror.full.requested.clear();
                }
                self.observer.failed(key.map(|k| k.as_str().to_string()), message.text)
            }
            Report::Notice { key, text } => self.observer.notice(key.as_str().to_string(), text),
        }
    }
}

/// Retouching, one render at a time. One per app, from
/// [`Engine::paint_session`]; dropping it saves every render's paint.
#[derive(uniffi::Object)]
pub struct PaintSession {
    pub(crate) engine: Arc<AppEngine>,
    session: deptheos_app::paint::PaintSession,
    mirror: Arc<Mutex<Option<Mirror>>>,
    forward: Arc<Forward>,
    /// The longest side of the displays the observer is sent.
    display_side: u32,
}

#[uniffi::export]
impl Engine {
    /// Start the paint worker; `display_side` is the longest side
    /// [`PaintObserver::presented`] sends. A worker that can't start leaves
    /// Retouch off ([`PaintSession::blocked`]).
    pub fn paint_session(
        &self,
        observer: Arc<dyn PaintObserver>,
        display_side: u32,
    ) -> Arc<PaintSession> {
        let mirror = Arc::new(Mutex::new(None));
        let forward = Arc::new(Forward {
            mirror: mirror.clone(),
            observer,
            side: display_side,
            full_tiles: AtomicUsize::new(0),
        });
        let session = deptheos_app::paint::PaintSession::new(
            self.engine.clone(),
            self.history.clone(),
            forward.clone(),
            teller(&self.engine),
        );
        Arc::new(PaintSession {
            engine: self.engine.clone(),
            session,
            mirror,
            forward,
            display_side,
        })
    }
}

impl PaintSession {
    /// While it is of the render open.
    fn mirror<T>(&self, f: impl FnOnce(&mut Mirror) -> Option<T>) -> Option<T> {
        let (key, _) = self.session.painting()?;
        let mut mirror = crate::lock(&self.mirror);
        mirror.as_mut().filter(|m| m.key == key).and_then(f)
    }

    fn id(&self, render_id: &str) -> Result<RenderId> {
        crate::render_id(render_id).told(self.engine.words())
    }
}

#[uniffi::export]
impl PaintSession {
    /// `None` while off. What [`PaintObserver::state_changed`] said last,
    /// or newer.
    pub fn status(&self) -> Option<PaintStatus> {
        let status = self.session.status()?;
        let mut said = PaintStatus::from(&status);
        // The lit frame is one of the open render's frames, by place.
        let (key, stack) = self.session.painting()?;
        if key == status.key {
            said.lit_frame = status.lit.and_then(|i| stack.paths.get(i).cloned());
        }
        Some(said)
    }

    /// Paint `result`, saved as render `render_id`, from its stacked frame
    /// `first_source` (else its first, else its original). The status
    /// follows with the canvas opening, then [`PaintObserver::opened`], or
    /// failed.
    pub fn open(
        &self,
        result: Arc<StackResult>,
        render_id: String,
        first_source: Option<PathBuf>,
    ) -> Result<()> {
        let key = self.id(&render_id)?;
        let stack = result.stack.clone();
        let frames = stack.paths.clone();
        self.session.open(OpenRender { key, stack, frames, first: first_source });
        Ok(())
    }

    /// The open failed: open the render again.
    pub fn retry(&self) {
        self.session.retry();
    }

    /// The canvas as it shows now, at most the displays' side, from the
    /// smaller levels kept (no GPU work, no wait): shown on leaving paint
    /// mode until [`PaintObserver::presented`] brings the whole. `None`
    /// when nothing is open or only full size exists (its tiles aren't all
    /// kept).
    pub fn canvas_image(&self) -> Option<DisplayImage> {
        // Level 0's tiles are only some, in `full`.
        self.mirror(|m| crate::levels_image(m.levels.get(1..)?, self.display_side))
    }

    /// The render's paint is saved, its sources let go.
    pub fn close(&self) {
        self.session.close();
    }

    /// Paint the original back: the eraser.
    pub fn pick_original(&self) {
        self.session.pick_original();
    }

    /// Paint from the stacked frame at `path`.
    pub fn pick_frame(&self, path: PathBuf) {
        self.session.pick_frame(path);
    }

    /// Shown at once; its pixels fetched once scrubbing rests.
    pub fn scrub_to(&self, path: PathBuf) {
        self.session.scrub_to(path);
    }

    /// The frame `by` places from the one painted from; from the original
    /// or a render, the first or the last.
    pub fn step(&self, by: i32) {
        self.session.step(by as isize);
    }

    /// Paint from the frame sharpest at `x`, `y` (as [`PaintSample`]); off
    /// the image picks nothing, and a render that can't tell says so
    /// through [`PaintObserver::notice`].
    pub fn pick_at(&self, x: f32, y: f32) {
        self.session.pick_at(x, y);
    }

    /// Folders were relinked ([`crate::Engine::locate`] and kin): the
    /// worker's sources follow (paint names frames by root, so it is
    /// unchanged). The app then opens the render again.
    pub fn relink(&self, moved: Vec<crate::FolderMove>) {
        self.session.relink(&crate::moves(moved));
    }

    /// Paint from render `render_id`, lined up with the open render through
    /// a frame both stacked, exposure-matched. One that can't be is said
    /// through [`PaintObserver::failed`], and the selection goes back.
    pub fn pick_render(&self, render_id: String) -> Result<()> {
        let id = self.id(&render_id)?;
        self.session.pick_render(&id);
        Ok(())
    }

    /// After the eraser: what was painted from before, or the first frame.
    pub fn use_brush(&self) {
        self.session.use_brush();
    }

    /// From a render or the original back to the frame painted from last,
    /// or the first stacked.
    pub fn use_frames(&self) {
        self.session.use_frames();
    }

    /// Only once the canvas is in.
    pub fn begin_stroke(&self, brush: Brush, sample: PaintSample) {
        self.session.begin_stroke(brush.clamped(), sample.dab());
    }

    /// In order; samples at a NaN or infinite position are skipped.
    pub fn add_samples(&self, samples: Vec<PaintSample>) {
        self.session.dabs(samples.into_iter().filter_map(PaintSample::dab).collect());
    }

    pub fn end_stroke(&self) {
        self.session.end_stroke();
    }

    pub fn undo(&self) {
        self.session.undo();
    }

    pub fn redo(&self) {
        self.session.redo();
    }

    /// Every painted pixel back to the original and the history cleared,
    /// on disk too: not undoable, so ask first.
    pub fn reset(&self) {
        self.session.reset();
    }

    /// Show `render_id`'s paint over `result` outside paint mode:
    /// [`PaintObserver::presented`] answers, with no display if none.
    pub fn present(&self, result: Arc<StackResult>, render_id: String) -> Result<()> {
        let key = self.id(&render_id)?;
        self.session.present(key, result.stack.clone());
        Ok(())
    }
}

impl PaintSession {
    /// Write `result` with `render_id`'s paint to `directory` as
    /// `<name>.<extension>` with `save`, and return the path. Blocks until
    /// the paint is flattened and written: call it off the main thread.
    pub(crate) fn export_saved(
        &self,
        result: Arc<StackResult>,
        render_id: &str,
        directory: &Path,
        name: &str,
        format: ExportFormat,
        save: deptheos_app::SaveOptions,
    ) -> Result<PathBuf> {
        let path =
            export_target(&result.stack, directory, name, format).told(self.engine.words())?;
        let key = self.id(render_id)?;
        let stack = result.stack.clone();
        let flattened = self.session.flatten(key, stack.clone());
        // Every failure leads with what didn't happen: paint unreadable, the
        // worker gone, or the write.
        self.engine
            .save_flattened(
                Look::exported(&stack, Some(&Adjustments::NEUTRAL)),
                Some(flattened),
                &path,
                save,
            )
            .map_err(|e| EngineError::led(Line::RetouchedExportFailed, &e, self.engine.words()))?;
        Ok(path)
    }
}

#[uniffi::export]
impl PaintSession {
    /// Write `result` with `render_id`'s paint, as [`StackResult::export`]
    /// does, once flattened.
    #[allow(clippy::too_many_arguments)]
    pub async fn export(
        self: Arc<Self>,
        request: Arc<Request>,
        result: Arc<StackResult>,
        render_id: String,
        directory: PathBuf,
        name: String,
        format: ExportFormat,
        preferences: ExportPreferences,
    ) -> Result<PathBuf> {
        let save = preferences.save_options();
        let this = self.clone();
        let write = move || this.export_saved(result, &render_id, &directory, &name, format, save);
        let words = self.engine.words();
        on_pool(words, &request, Lane::Background, Ask::Own("export-retouched"), write).await
    }
}

#[uniffi::export]
impl PaintSession {
    /// Write every render's unsaved paint and close its files, waiting up
    /// to `timeout_ms` on the pool, as the app goes to the background
    /// (ADR-148). Whether every change is on disk.
    pub async fn save(self: Arc<Self>, timeout_ms: u32) -> Result<bool> {
        let this = self.clone();
        let save = move || Ok(this.session.save(Duration::from_millis(timeout_ms.into())));
        let words = self.engine.words();
        on_pool(words, &crate::Request::lasting(), Lane::Interactive, Ask::Own("paint-save"), save)
            .await
    }

    /// As many full-size tiles as the app's view wants: those shown and
    /// around them, none at a smaller level. Said before asking
    /// ([`Self::tiles`]) as zoom and size change; extra tiles go at once.
    pub fn keep_full_tiles(&self, count: u32) {
        let keep = count as usize;
        if self.forward.full_tiles.swap(keep, Ordering::Relaxed) != keep {
            tracing::debug!("paint: {keep} full-size tiles kept");
        }
        if let Some(mirror) = crate::lock(&self.mirror).as_mut() {
            mirror.full.trim(keep);
        }
    }

    /// The tiles `asked` of the open display, each with its one-pixel
    /// gutter ([`paint_tile_core`]), copied on the pool for `request`.
    /// Nothing when nothing is shown or no such tile. A full-size tile not
    /// kept is nothing too; those of one ask are rendered together and
    /// follow through [`PaintObserver::tiles_changed`].
    pub async fn tiles(
        self: Arc<Self>,
        request: Arc<crate::Request>,
        asked: Vec<TileAsk>,
    ) -> Result<Vec<TileFetched>> {
        let this = self.clone();
        let fetch = move || Ok(this.fetch(&asked));
        let words = self.engine.words();
        on_pool(words, &request, Lane::Interactive, Ask::Own("paint-tiles"), fetch).await
    }
}

impl PaintSession {
    /// [`Self::tiles`]' work: full-size tiles not kept go to the worker in
    /// one request, rendered in one pass and reported together.
    fn fetch(&self, asked: &[TileAsk]) -> Vec<TileFetched> {
        let levels = self.mirror(|m| Some(m.levels.len() as u32)).unwrap_or(0);
        let wanted = std::cell::RefCell::new(Vec::new());
        let fetched = fetch_tiles(asked, levels, |l, x, y| self.tile(l, x, y, &wanted));
        let wanted = wanted.into_inner();
        if !wanted.is_empty() {
            self.session.render_tiles(wanted);
        }
        fetched
    }

    /// `None` when missing; a full-size one not yet asked for goes into
    /// `wanted`.
    fn tile(
        &self,
        level: u32,
        tx: u32,
        ty: u32,
        wanted: &std::cell::RefCell<Vec<[u32; 2]>>,
    ) -> Option<DisplayImage> {
        self.mirror(|mirror| {
            let tiled = mirror.levels.get(level as usize)?;
            if tx >= tiled.cols || ty >= tiled.rows {
                return None;
            }
            let tile = if level == 0 {
                let Some(tile) = mirror.full.get([tx, ty]) else {
                    if mirror.full.requested.insert([tx, ty]) {
                        wanted.borrow_mut().push([tx, ty]);
                    }
                    return None;
                };
                tile
            } else {
                tiled.tiles.get((ty * tiled.cols + tx) as usize)?
            };
            Some(DisplayImage { width: tile.width, height: tile.height, bgra: tile.pixels.clone() })
        })
    }

    /// One tile, as [`Self::tiles`] fetches it without a stand-in.
    #[cfg(test)]
    fn one_tile(&self, level: u32, tx: u32, ty: u32) -> Option<DisplayImage> {
        let ask = TileAsk { level, tx, ty, stand_in: false };
        match self.fetch(&[ask]).pop() {
            Some(TileFetched::Tile { image }) => Some(image),
            _ => None,
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::mpsc::{Receiver, Sender, channel};

    use deptheos_app::{CompletedStack, SessionOptions, StackColor};

    use super::*;
    use crate::tests::{now, req};
    use crate::{FrameColor, FrameDecoder, FrameHeader, FrameLayout, OpenedFrame, SampleDepth};

    /// Frames as the `image` crate reads them, for the app's decoder. The
    /// first engine's decoder stays for the process, so every test uses
    /// this one.
    pub(crate) struct ImageCrate;

    impl FrameDecoder for ImageCrate {
        fn open_frame(
            &self,
            path: PathBuf,
        ) -> std::result::Result<Arc<dyn OpenedFrame>, EngineError> {
            let image = image::open(&path)
                .map_err(|e| EngineError::Failed { message: e.to_string() })?
                .into_rgba8();
            Ok(Arc::new(Rgba8(image)))
        }
    }

    /// As RGBA at 8 bits.
    struct Rgba8(image::RgbaImage);

    impl OpenedFrame for Rgba8 {
        fn header(&self) -> FrameHeader {
            FrameHeader {
                width: self.0.width(),
                height: self.0.height(),
                depth: SampleDepth::Eight,
                layout: FrameLayout::Rgbx,
                color: FrameColor::Untagged,
                orientation: 1,
            }
        }

        fn decode_into(
            &self,
            destination: u64,
            length: u64,
        ) -> std::result::Result<(), EngineError> {
            let bytes = self.0.as_raw();
            assert_eq!(bytes.len() as u64, length);
            // SAFETY: the engine passes a live buffer of `length` bytes.
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), destination as *mut u8, bytes.len())
            };
            Ok(())
        }
    }

    #[derive(Debug)]
    enum Heard {
        Opened(String, Vec<PaintLevel>),
        Tiles(String, Vec<PaintTile>),
        /// The session's status says how.
        State,
        Closed(String),
        /// The render, and if painted its image's size, its picture's full
        /// size, and whether full size has a first tile.
        Presented(String, Option<(u32, u32, (u32, u32), bool)>),
        Source(String, SourceKind),
        Failed(Option<String>, String),
        Notice(String, String),
    }

    struct Recorder(Mutex<Sender<Heard>>);

    impl Recorder {
        fn hear(&self, heard: Heard) {
            let _ = self.0.lock().unwrap().send(heard);
        }
    }

    impl PaintObserver for Recorder {
        fn opened(&self, render_id: String, levels: Vec<PaintLevel>) {
            self.hear(Heard::Opened(render_id, levels));
        }
        fn tiles_changed(&self, render_id: String, tiles: Vec<PaintTile>) {
            self.hear(Heard::Tiles(render_id, tiles));
        }
        fn state_changed(&self) {
            self.hear(Heard::State);
        }
        fn closed(&self, render_id: String) {
            self.hear(Heard::Closed(render_id));
        }
        fn presented(&self, render_id: String, painted: Option<PaintedDisplay>) {
            let painted = painted.map(|PaintedDisplay { image, picture }| {
                let size = picture.size();
                let tiled = picture.tile(0, 0, 0).is_some();
                (image.width, image.height, (size.width, size.height), tiled)
            });
            self.hear(Heard::Presented(render_id, painted));
        }
        fn source_shown(
            &self,
            render_id: String,
            source: SourceKind,
            _: DisplayImage,
            _: Arc<Picture>,
        ) {
            self.hear(Heard::Source(render_id, source));
        }
        fn failed(&self, render_id: Option<String>, message: String) {
            self.hear(Heard::Failed(render_id, message));
        }
        fn notice(&self, render_id: String, message: String) {
            self.hear(Heard::Notice(render_id, message));
        }
    }

    /// Until `pick` takes, failing on an error.
    fn until<T>(
        heard: &Receiver<Heard>,
        what: &str,
        mut pick: impl FnMut(Heard) -> Option<T>,
    ) -> T {
        loop {
            let next = heard
                .recv_timeout(Duration::from_secs(30))
                .unwrap_or_else(|_| panic!("never heard {what}"));
            if let Heard::Failed(_, message) = &next {
                panic!("paint failed: {message}");
            }
            if let Some(t) = pick(next) {
                return t;
            }
        }
    }

    /// Once a change makes it `f`'s, failing on a failed open.
    fn state(
        session: &PaintSession,
        heard: &Receiver<Heard>,
        what: &str,
        f: impl Fn(&PaintStatus) -> bool,
    ) -> PaintStatus {
        until(heard, what, |h| match h {
            Heard::State => {
                let status = session.status()?;
                if let PaintCanvas::Failed { message } = &status.canvas {
                    panic!("the open failed: {message}");
                }
                f(&status).then_some(status)
            }
            _ => None,
        })
    }

    /// Rendered for the asking if not kept.
    fn full_tile(
        session: &PaintSession,
        heard: &Receiver<Heard>,
        tx: u32,
        ty: u32,
    ) -> DisplayImage {
        if let Some(tile) = session.one_tile(0, tx, ty) {
            return tile;
        }
        let wanted = PaintTile { level: 0, tx, ty };
        until(heard, "the full-size tile", |h| match h {
            Heard::Tiles(_, tiles) if tiles.contains(&wanted) => Some(()),
            _ => None,
        });
        session.one_tile(0, tx, ty).expect("kept once rendered")
    }

    fn center_pixel(path: &Path, width: u32, height: u32) -> [u8; 3] {
        let image = image::open(path).unwrap().into_rgb8();
        image.get_pixel(width / 2, height / 2).0
    }

    fn engine(dir: &std::path::Path) -> Option<Arc<Engine>> {
        let history = dir.join("history");
        let engine =
            now(Engine::new(Arc::new(ImageCrate), PathBuf::from(&history), None, "en".into()));
        if let Err(e) = &engine {
            deptheos_app::GpuContext::skip_test(&format!("no GPU engine ({e})"));
        }
        engine.ok()
    }

    /// A render opened, painted from a frame, undone and redone, exported
    /// with its paint, and shown painted once closed.
    #[test]
    fn a_render_painted_through_the_bindings() {
        let dir = tempfile::tempdir().unwrap();
        let Some(engine) = engine(dir.path()) else { return };
        let (width, height) = (600, 400);
        // Two flat frames, white and black, and a mid-gray result.
        let frames: Vec<PathBuf> = [255u8, 0]
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                let path = dir.path().join(format!("f{i}.png"));
                image::RgbImage::from_pixel(width, height, image::Rgb([v, v, v]))
                    .save(&path)
                    .unwrap();
                path
            })
            .collect();
        let pixels = vec![128 * 257; (3 * width * height) as usize];
        // The left half from the first frame, the right from the second.
        let frame_map = deptheos_app::Gray16Image::from_fn(width, height, |x, _| {
            image::Luma([if x < width / 2 { 0 } else { 65535 }])
        });
        let stack = CompletedStack {
            color: StackColor::Srgb { sixteen_bit: false, profile: None },
            options: SessionOptions::default(),
            frame_map: Some(frame_map),
            ..CompletedStack::srgb(width, height, pixels, frames.clone())
        };
        let presented =
            engine.engine.present_stack(stack, &deptheos_app::Adjustments::NEUTRAL).unwrap();
        let result = StackResult::new(engine.engine.clone(), presented);
        let saving = engine.clone().save_render(
            frames.clone(),
            result.clone(),
            crate::new_stack_id(),
            crate::RenderMade::Stacked { base: None },
        );
        let render = now(saving).unwrap();
        // Saved and shown, the result lets go of its display; painting and
        // exporting read the stack's pixels.
        assert!(result.held(false).is_some());
        result.release_displays();
        assert!(result.held(false).is_none() && result.held(true).is_none());

        let (tx, heard) = channel();
        let session = engine.paint_session(Arc::new(Recorder(Mutex::new(tx))), 1024);
        // As the app's view says before it asks for full-size tiles.
        session.keep_full_tiles(4);
        assert_eq!(session.status(), None, "nothing open");
        // Not painted yet, it shows as stacked: answered with no display.
        session.present(result.clone(), render.id.clone()).unwrap();
        let shown = until(&heard, "the answer", |h| match h {
            Heard::Presented(id, painted) => Some((id, painted)),
            _ => None,
        });
        assert_eq!(shown, (render.id.clone(), None));
        // Opened on the first frame: the selection at once, the canvas
        // opening, no stroke until it is in.
        let (left, right) = (PathBuf::from(&frames[0]), PathBuf::from(&frames[1]));
        session.open(result.clone(), render.id.clone(), Some(left.clone())).unwrap();
        let opening = session.status().expect("open");
        assert_eq!(opening.canvas, PaintCanvas::Opening);
        assert_eq!(opening.selection, SourceKind::Frame { path: left.clone() });
        assert_eq!(opening.lit_frame, Some(left.clone()), "the map lit for it");
        let levels = until(&heard, "the canvas", |h| match h {
            Heard::Opened(id, levels) => Some((id, levels)),
            _ => None,
        });
        assert_eq!(levels.0, render.id);
        assert_eq!(levels.1[0], PaintLevel { width, height, cols: 2, rows: 1 });
        // Full size is rendered when first asked; smaller levels are kept
        // whole.
        if levels.1.len() > 1 {
            assert!(session.one_tile(1, 0, 0).is_some());
        }
        assert!(session.one_tile(0, 0, 0).is_none());
        let opened = state(&session, &heard, "the first frame in", |s| {
            s.canvas == PaintCanvas::Ready && s.ready
        });
        assert_eq!(opened.selection, SourceKind::Frame { path: left.clone() });
        let before = full_tile(&session, &heard, 0, 0);
        assert_eq!((before.width, before.height), (paint_tile_core() + 2, height + 2));

        // The sharpest frame at a point is the one the result took; off the
        // image, nothing.
        session.pick_at(width as f32 - 0.5, 0.5);
        let picked = SourceKind::Frame { path: right.clone() };
        let shown = until(&heard, "the right frame shown", |h| match h {
            Heard::Source(id, source) if source == picked => Some(id),
            _ => None,
        });
        assert_eq!(shown, render.id);
        state(&session, &heard, "the right frame", |s| s.selection == picked && s.ready);
        session.pick_at(width as f32, 0.5);
        session.pick_at(f32::NAN, 0.5);
        session.pick_at(10.5, 10.5);
        let back = state(&session, &heard, "the left frame", |s| {
            s.selection == SourceKind::Frame { path: left.clone() } && s.ready
        });
        assert_eq!(back.last_frame, Some(left.clone()));

        // Painted from the white frame.
        let brush = Brush { radius: 40.0, hardness: 1.0, opacity: 1.0, flow: 1.0, tolerance: None };
        let (cx, cy) = (width as f32 / 2.0, height as f32 / 2.0);
        // From left of the center across the first tile's right edge.
        session.begin_stroke(brush, PaintSample { x: cx - 20.0, y: cy, pressure: 1.0 });
        session.add_samples(vec![
            PaintSample { x: cx + 110.0, y: cy, pressure: 1.0 },
            PaintSample { x: cx + 240.0, y: cy, pressure: 1.0 },
        ]);
        session.end_stroke();
        // The tiles it changed come before the state that counts it.
        let mut changed = Vec::new();
        let painted = until(&heard, "the stroke", |h| match h {
            Heard::Tiles(id, tiles) => {
                assert_eq!(id, render.id);
                changed.extend(tiles);
                None
            }
            Heard::State => session.status().filter(|s| s.painted && s.undo == 1),
            _ => None,
        });
        assert_eq!(
            painted.painted_from,
            [SourceKind::Frame { path: left.clone() }],
            "painted from in this retouch"
        );
        assert!(changed.contains(&PaintTile { level: 0, tx: 0, ty: 0 }));
        assert!(
            changed.contains(&PaintTile { level: 0, tx: 1, ty: 0 }),
            "the stroke crosses x 510"
        );
        // A kept tile is updated by the stroke; one never asked for is
        // rendered, painted, when it is.
        let kept = session.one_tile(0, 0, 0).expect("kept since it was asked for");
        assert!(kept.bgra != before.bgra, "the stroke shows");
        let at = |tile: &DisplayImage, x: u32, y: u32| {
            let i = (((y + 1) * tile.width + x + 1) * 4) as usize;
            tile.bgra[i..i + 3].to_vec()
        };
        assert_eq!(at(&kept, width / 2, height / 2), [255; 3]);
        assert!(session.one_tile(0, 1, 0).is_none());
        let right_tile = full_tile(&session, &heard, 1, 0);
        let core = paint_tile_core();
        assert_eq!(at(&right_tile, (width / 2 + 230) - core, height / 2), [255; 3]);
        assert_eq!(at(&right_tile, width - 1 - core, 0), [128; 3], "unpainted stays");

        // The eraser is the original; swapping goes back to the frame.
        session.pick_original();
        state(&session, &heard, "the eraser", |s| s.selection == SourceKind::Original && s.ready);
        session.session.swap();
        state(&session, &heard, "the frame again", |s| {
            s.selection == SourceKind::Frame { path: left.clone() } && s.ready
        });

        session.undo();
        state(&session, &heard, "the undo", |s| s.undo == 0 && s.redo == 1);
        session.redo();
        state(&session, &heard, "the redo", |s| s.undo == 1 && s.redo == 0);

        let out = PathBuf::from(dir.path());
        let export = crate::default_preferences().export;
        let plain = now(result.clone().export(
            req(),
            out.clone(),
            "plain".into(),
            ExportFormat::Png,
            export.clone(),
        ))
        .unwrap();
        let retouched = now(session.clone().export(
            req(),
            result.clone(),
            render.id.clone(),
            out,
            "painted".into(),
            ExportFormat::Png,
            export,
        ))
        .unwrap();
        assert_eq!(center_pixel(&plain, width, height), [128; 3]);
        assert_eq!(center_pixel(&retouched, width, height), [255; 3], "the export has the paint");

        // Leaving, the canvas from its largest smaller level, full size's
        // tiles being only some.
        if levels.1.len() > 1 {
            let canvas = session.canvas_image().expect("open, with smaller levels");
            let PaintLevel { width: w, height: h, .. } = levels.1[1];
            assert_eq!((canvas.width, canvas.height), (w, h));
            let i = ((h / 2 * w + w / 2) * 4) as usize;
            assert_eq!(canvas.bgra[i..i + 3], [255; 3], "the paint shows");
        } else {
            assert!(session.canvas_image().is_none(), "no level whose tiles are all kept");
        }
        // Closed, the render shows painted.
        session.close();
        assert!(session.canvas_image().is_none(), "nothing open");
        assert_eq!(session.status(), None);
        let closed = until(&heard, "the close", |h| match h {
            Heard::Closed(id) => Some(id),
            _ => None,
        });
        assert_eq!(closed, render.id);
        session.present(result.clone(), render.id.clone()).unwrap();
        let shown = until(&heard, "the painted display", |h| match h {
            Heard::Presented(id, painted) => Some((id, painted)),
            _ => None,
        });
        // The picture is the painted render's whole, to tile at full size.
        assert_eq!(shown, (render.id.clone(), Some((width, height, (width, height), true))));
        assert!(now(session.clone().save(10_000)).unwrap());

        // Closed before its canvas came, a render leaves nothing to show.
        session.open(result.clone(), render.id.clone(), None).unwrap();
        session.close();
        assert!(now(session.clone().save(10_000)).unwrap());
        assert!(session.status().is_none() && session.canvas_image().is_none());
    }

    /// A render with smaller levels gives its canvas at once at the
    /// displays' side while open; closed, nothing.
    #[test]
    fn the_canvas_image_is_its_smaller_levels_at_the_display_side() {
        let dir = tempfile::tempdir().unwrap();
        let Some(engine) = engine(dir.path()) else { return };
        let (width, height) = (2400, 1600);
        let frame = dir.path().join("f.png");
        image::RgbImage::from_pixel(width, height, image::Rgb([0, 0, 0])).save(&frame).unwrap();
        let pixels = vec![128 * 257; (3 * width * height) as usize];
        let stack = CompletedStack::srgb(width, height, pixels, vec![frame.clone()]);
        let presented =
            engine.engine.present_stack(stack, &deptheos_app::Adjustments::NEUTRAL).unwrap();
        let result = StackResult::new(engine.engine.clone(), presented);
        let saving = engine.clone().save_render(
            vec![frame],
            result.clone(),
            crate::new_stack_id(),
            crate::RenderMade::Stacked { base: None },
        );
        let render = now(saving).unwrap();
        let (tx, heard) = channel();
        let session = engine.paint_session(Arc::new(Recorder(Mutex::new(tx))), 1024);
        session.open(result, render.id, None).unwrap();
        until(&heard, "the canvas", |h| matches!(h, Heard::Opened(..)).then_some(()));
        // 1200 px, the smallest level at least 1024, scaled to 1024.
        let canvas = session.canvas_image().expect("open, with smaller levels");
        assert_eq!((canvas.width, canvas.height), (1024, 683));
        let i = ((341 * 1024 + 512) * 4) as usize;
        assert_eq!(canvas.bgra[i..i + 3], [128; 3]);
        // Full-size tiles not kept, asked together, are rendered together.
        session.keep_full_tiles(3);
        let asked = (0..3).map(|tx| TileAsk { level: 0, tx, ty: 0, stand_in: false }).collect();
        let fetched = now(session.clone().tiles(req(), asked)).unwrap();
        assert!(fetched.iter().all(|f| matches!(f, TileFetched::Nothing)), "none kept yet");
        let rendered = until(&heard, "the tiles", |h| match h {
            Heard::Tiles(_, tiles) => Some(tiles),
            _ => None,
        });
        let want: Vec<_> = (0..3).map(|tx| PaintTile { level: 0, tx, ty: 0 }).collect();
        assert_eq!(rendered, want, "in one report");
        session.close();
        assert!(session.canvas_image().is_none(), "nothing open");
        assert!(now(session.clone().save(10_000)).unwrap());
    }

    /// A render with no record of where its detail came from says so as a
    /// notice when a point is picked; the selection stays.
    #[test]
    fn a_pick_at_a_point_without_a_record_says_why() {
        let dir = tempfile::tempdir().unwrap();
        let Some(engine) = engine(dir.path()) else { return };
        let frame = dir.path().join("f.png");
        image::RgbImage::from_pixel(64, 64, image::Rgb([0, 0, 0])).save(&frame).unwrap();
        let stack = CompletedStack::srgb(64, 64, vec![0; 3 * 64 * 64], vec![frame.clone()]);
        let presented =
            engine.engine.present_stack(stack, &deptheos_app::Adjustments::NEUTRAL).unwrap();
        let result = StackResult::new(engine.engine.clone(), presented);
        let render = now(engine.clone().save_render(
            vec![frame.clone()],
            result.clone(),
            crate::new_stack_id(),
            crate::RenderMade::Stacked { base: None },
        ))
        .unwrap();
        let (tx, heard) = channel();
        let session = engine.paint_session(Arc::new(Recorder(Mutex::new(tx))), 1024);
        session.open(result, render.id.clone(), None).unwrap();
        let first = SourceKind::Frame { path: PathBuf::from(&frame) };
        state(&session, &heard, "the frame in", |s| s.ready && s.canvas == PaintCanvas::Ready);
        session.pick_at(10.5, 10.5);
        let (id, text) = until(&heard, "the notice", |h| match h {
            Heard::Notice(id, text) => Some((id, text)),
            _ => None,
        });
        assert_eq!(id, render.id);
        assert!(text.contains("doesn't record"), "{text}");
        assert_eq!(session.status().unwrap().selection, first);
        session.close();
        assert!(now(session.clone().save(10_000)).unwrap());
    }

    /// A failure lets the full-size tiles asked for be asked again.
    #[test]
    fn a_failure_lets_tiles_be_asked_again() {
        let key = crate::render_id(&format!("{:032x}", 7)).unwrap();
        let mut full = FullTiles::default();
        full.requested.insert([0, 0]);
        let mirror =
            Arc::new(Mutex::new(Some(Mirror { key: key.clone(), levels: Vec::new(), full })));
        let (tx, heard) = channel();
        let forward = Forward {
            mirror: mirror.clone(),
            observer: Arc::new(Recorder(Mutex::new(tx))),
            side: 1024,
            full_tiles: AtomicUsize::new(0),
        };
        let message = Failure::user(String::from("no"));
        forward.report(Report::Failed { key: Some(key), message });
        let requested = mirror.lock().unwrap().as_ref().map(|m| m.full.requested.len());
        assert_eq!(requested, Some(0));
        assert!(matches!(heard.try_recv(), Ok(Heard::Failed(Some(_), _))));
    }

    /// As many full-size tiles are kept as the app says, those used last;
    /// fewer said, the rest go.
    #[test]
    fn the_full_size_tiles_kept_follow_what_the_app_wants() {
        let key = crate::render_id(&format!("{:032x}", 7)).unwrap();
        let level = TiledLevel { width: 4, height: 1, cols: 4, rows: 1, tiles: Vec::new() };
        let mut full = FullTiles::default();
        full.requested.extend((0..4).map(|x| [x, 0]));
        let mirror = Mirror { key: key.clone(), levels: vec![level], full };
        let (tx, _heard) = channel();
        let forward = Forward {
            mirror: Arc::new(Mutex::new(Some(mirror))),
            observer: Arc::new(Recorder(Mutex::new(tx))),
            side: 1024,
            full_tiles: AtomicUsize::new(3),
        };
        let tile = || CpuImage { width: 1, height: 1, pixels: vec![0; 4] };
        let patches = (0..4)
            .map(|tx| deptheos_app::paint::DisplayPatch { level: 0, tx, ty: 0, tile: tile() })
            .collect();
        forward.report(Report::Patches { key, patches });
        let mut guard = forward.mirror.lock().unwrap();
        let full = &mut guard.as_mut().unwrap().full;
        let mut kept: Vec<_> = full.tiles.keys().copied().collect();
        kept.sort();
        assert_eq!(kept, [[1, 0], [2, 0], [3, 0]], "the three used last");
        full.get([1, 0]);
        full.trim(1);
        assert_eq!(full.tiles.keys().collect::<Vec<_>>(), [&[1, 0]]);
        full.trim(0);
        assert!(full.tiles.is_empty(), "a smaller level shown: none");
    }

    /// Tiles of a render other than the mirror's change nothing and aren't
    /// told: a closed render's never reach the next.
    #[test]
    fn another_renders_tiles_are_not_the_mirrors() {
        let key = crate::render_id(&format!("{:032x}", 7)).unwrap();
        let other = crate::render_id(&format!("{:032x}", 8)).unwrap();
        let level = TiledLevel { width: 1, height: 1, cols: 1, rows: 1, tiles: Vec::new() };
        let mirror = Mirror { key, levels: vec![level], full: FullTiles::default() };
        let (tx, heard) = channel();
        let forward = Forward {
            mirror: Arc::new(Mutex::new(Some(mirror))),
            observer: Arc::new(Recorder(Mutex::new(tx))),
            side: 1024,
            full_tiles: AtomicUsize::new(0),
        };
        let tile = CpuImage { width: 1, height: 1, pixels: vec![0; 4] };
        let patch = deptheos_app::paint::DisplayPatch { level: 0, tx: 0, ty: 0, tile };
        forward.report(Report::Patches { key: other, patches: vec![patch] });
        assert!(heard.try_recv().is_err(), "nothing told");
    }
}
