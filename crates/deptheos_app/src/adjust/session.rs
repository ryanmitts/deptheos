//! Adjust for both apps behind one object: the [`Machine`] and what it asks
//! for, as `PaintSession` wraps paint's (ADR-153).
//!
//! Every call and result goes through [`Machine::on`] under one lock, and
//! its updates are queued in order and delivered after the lock is
//! released, one at a time, so an observer that calls back in can't
//! deadlock. The observer is called on the app's, the pool's or the store's
//! thread.
//!
//! The machine's requests run here: the GPU copies are made and drawn on
//! the pool's interactive lane, each write and the open's read on the store
//! thread, in order with the Library's other writes. A result counts only
//! while its request is current: the machine drops a stale one, and a
//! stale upload's copies are freed.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

use deptheos_render::reframe::Size;
use deptheos_render::{Adjustments, Draw, Reframe};

use super::copies::{GpuCopies, RenderMemory};
use super::eyedropper::eyedrop;
use super::histogram::{self, Histogram};
use super::machine::{Action, AdjustClosed, Event, Machine, Phase, Report, RequestId, Status};
use super::preview::{Draft, FlatPicture, FlatSlot, Shown};
use super::sliders::{Panel, Slider};
use crate::engine::orient::Orient;
use crate::engine::store::store;
use crate::engine::work::{self, Key, Lane};
use crate::library::adjustments::rules::{pasted, reset_all};
use crate::library::adjustments::{RenderAdjustments, Unread, Writes};
use crate::library::history::History;
use crate::paint::Flattened;
use crate::words::say;
use crate::{CompletedStack, Engine, Look, PresentedImage, RenderId};

/// Hears the session's updates, in order, each once.
pub trait Observer: Send + Sync {
    fn update(&self, update: Update);
}

/// What the app hears.
pub enum Update {
    /// Where Adjust stands; once per change.
    Status(Status),
    /// A tick of the drag preview: shown over the settled picture until
    /// the next. Its pixels go back with [`AdjustSession::give_back`].
    Preview { picture: FlatPicture, histogram: Histogram },
    /// The settled picture of the view on screen, ahead of the full one:
    /// shown in place of the preview. At fit it is drawn from the reduced
    /// copy without Texture and Sharpening; past 50% it is that window of
    /// the full picture. Its pixels go back with
    /// [`AdjustSession::give_back`].
    OnScreen { picture: FlatPicture, histogram: Histogram },
    /// The full picture of render `key` under its value, with the
    /// histogram's clipping counted over every pixel: in place of the
    /// viewer's display and every preview.
    Settled { key: RenderId, picture: PresentedImage, histogram: Histogram },
    /// Adjust left render `key`, its last write in. `redraw`: the screen
    /// doesn't show its saved value, so the viewer draws it again.
    Closed { key: RenderId, redraw: bool },
    /// Render `key`'s thumbnail was drawn again under its adjustments: the
    /// app reads it again.
    Rewritten(RenderId),
    /// Something to tell the user: a refused eyedropper, a damaged file,
    /// a failed write.
    Notice(String),
}

/// A drawing waiting for the machine's [`Action::Show`].
enum Drawn {
    Preview { picture: FlatPicture, histogram: Histogram },
    Settled { key: RenderId, picture: PresentedImage, histogram: Histogram },
}

/// The GPU copies, of render `key`, made by the upload of `epoch`.
struct Held {
    epoch: u64,
    key: RenderId,
    copies: GpuCopies,
}

/// An open whose adjustments are being read, before the machine hears it.
struct Reading {
    token: u64,
    key: RenderId,
    stack: Arc<CompletedStack>,
}

/// The render open and what the session keeps of it.
#[derive(Clone)]
struct Opened {
    key: RenderId,
    stack: Arc<CompletedStack>,
    panel: Panel,
}

struct Core {
    machine: Machine,
    reading: Option<Reading>,
    reads: u64,
    /// The render whose requests the machine's actions are, the one left
    /// until an open's upload names the next.
    current: Option<Opened>,
    /// An open the machine has heard, until its first upload.
    next: Option<Opened>,
    /// Every render opened since its close, for the close's thumbnail.
    stacks: HashMap<RenderId, Arc<CompletedStack>>,
    /// Renders written since they opened: their thumbnails are drawn again
    /// on close.
    changed: HashSet<RenderId>,
    shown: Option<Shown>,
    /// The latest upload and its copies' epoch, until freed.
    upload: Option<(RequestId, u64)>,
    epoch: u64,
    drawn: HashMap<RequestId, Drawn>,
    writes: HashMap<RenderId, Writes>,
    eyedrops: u64,
}

#[derive(Default)]
struct Outbox {
    queue: VecDeque<Update>,
    delivering: bool,
}

struct Inner {
    engine: Arc<Engine>,
    history: Arc<History>,
    observer: Arc<dyn Observer>,
    core: Mutex<Core>,
    /// Woken after every event, for [`close`].
    changed: Condvar,
    copies: Mutex<Option<Held>>,
    outbox: Mutex<Outbox>,
    slot: FlatSlot,
    me: Weak<Inner>,
}

/// Adjust, one render at a time: create one per app.
pub struct AdjustSession {
    inner: Arc<Inner>,
}

/// How long [`close`] waits for a session to leave and its
/// writes to land.
const CLOSE_WAIT: Duration = Duration::from_secs(10);

impl AdjustSession {
    pub fn new(engine: Arc<Engine>, history: Arc<History>, observer: Arc<dyn Observer>) -> Self {
        let library = history.root().to_path_buf();
        let inner = Arc::new_cyclic(|me: &Weak<Inner>| Inner {
            engine,
            history,
            observer,
            core: Mutex::new(Core {
                machine: Machine::new(),
                reading: None,
                reads: 0,
                current: None,
                next: None,
                stacks: HashMap::new(),
                changed: HashSet::new(),
                shown: None,
                upload: None,
                epoch: 0,
                drawn: HashMap::new(),
                writes: HashMap::new(),
                eyedrops: 0,
            }),
            changed: Condvar::new(),
            copies: Mutex::new(None),
            outbox: Mutex::default(),
            slot: FlatSlot::default(),
            me: me.clone(),
        });
        let mut sessions = crate::lock(&SESSIONS);
        sessions.retain(|(_, s)| s.strong_count() > 0);
        sessions.push((library, Arc::downgrade(&inner)));
        drop(sessions);
        Self { inner }
    }

    /// `None` while Adjust is off; the last [`Update::Status`] or newer.
    pub fn status(&self) -> Option<Status> {
        crate::lock(&self.inner.core).machine.status()
    }

    /// The render adjusted, while Adjust is on.
    pub fn adjusting(&self) -> Option<RenderId> {
        crate::lock(&self.inner.core).machine.adjusting().cloned()
    }

    /// The panel of the render adjusted.
    pub fn panel(&self) -> Option<Panel> {
        crate::lock(&self.inner.core).current.as_ref().map(|o| o.panel)
    }

    /// Adjust render `key` of `stack`, its adjustments read first, after
    /// every write queued before. Another render open is left.
    pub fn open(&self, key: RenderId, stack: Arc<CompletedStack>) {
        let token = {
            let mut core = crate::lock(&self.inner.core);
            core.reads += 1;
            let token = core.reads;
            core.reading = Some(Reading { token, key: key.clone(), stack });
            token
        };
        let (history, me) = (self.inner.history.clone(), self.inner.me.clone());
        let _read = store().write(move || {
            let read = history
                .render(&key)
                .map_err(|e| {
                    tracing::warn!("adjust: render {} not read: {e}", key.as_str());
                    Unread::Unreachable
                })
                .and_then(|render| history.adjustments(&render));
            if let Some(inner) = me.upgrade() {
                inner.opened(token, read);
            }
        });
    }

    /// What the viewer shows, for the next preview.
    pub fn view(&self, shown: Shown) {
        crate::lock(&self.inner.core).shown = Some(shown);
    }

    /// The value shown moves; a change goes on until [`Self::commit`].
    pub fn set(&self, value: RenderAdjustments) {
        self.inner.on(Event::Set(value));
    }

    /// `slider` moves to `value`, clamped to its range.
    pub fn set_slider(&self, slider: Slider, value: f64) {
        self.inner.change(false, |current, panel| RenderAdjustments {
            adjustments: slider.set(&current.adjustments, panel, value),
            ..current.clone()
        });
    }

    /// The reframe moves (a crop handle, a turn, a flip); a change goes on
    /// until [`Self::commit`].
    pub fn set_reframe(&self, reframe: Reframe) {
        self.inner.change(false, |current, _| RenderAdjustments { reframe, ..current.clone() });
    }

    /// The change ends: written and undoable if the value moved.
    pub fn commit(&self) {
        self.inner.on(Event::Commit);
    }

    pub fn undo(&self) {
        self.inner.on(Event::Undo);
    }

    pub fn redo(&self) {
        self.inner.on(Event::Redo);
    }

    /// Every slider to neutral in the latest process, the reframe kept: one
    /// change.
    pub fn reset_all(&self) {
        self.inner.change(true, |current, _| reset_all(current));
    }

    /// `slider` to its default: one change.
    pub fn reset(&self, slider: Slider) {
        self.inner.change(true, |current, panel| RenderAdjustments {
            adjustments: slider.reset(&current.adjustments, panel),
            ..current.clone()
        });
    }

    /// `copied`'s sliders onto the render adjusted (ADR-144): one change.
    pub fn paste(&self, copied: Adjustments) {
        self.inner.change(true, |current, panel| pasted(&copied, current, panel.kind));
    }

    /// Set the white balance from the stored pixels under `at`, a point on
    /// the picture as shown under its reframe: one change, or a notice of
    /// why not.
    pub fn eyedrop(&self, at: (f64, f64)) {
        self.inner.eyedrop(at);
    }

    /// Open the render again after its open or a draw failed.
    pub fn retry(&self) {
        self.inner.on(Event::Retry);
    }

    /// Leave Adjust, a change in progress committed.
    pub fn leave(&self) {
        self.inner.on(Event::Leave);
    }

    /// The app went to the background, or memory ran low: the GPU copies
    /// are freed, the values kept.
    pub fn away(&self) {
        self.inner.on(Event::Away);
    }

    /// The app came back: the GPU copies are made again.
    pub fn back(&self) {
        self.inner.on(Event::Back);
    }

    /// The stored picture with its paint changed (Retouch's generation
    /// moved): its copies are made again.
    pub fn stored_changed(&self) {
        self.inner.on(Event::StoredChanged);
    }

    /// A preview's pixels, replaced on screen, for the next tick.
    pub fn give_back(&self, picture: FlatPicture) {
        self.inner.slot.give_back(picture);
    }

    /// Proof that none of `ids` is adjusted or has a write pending, now.
    pub fn closed(&self, ids: &[RenderId]) -> Option<AdjustClosed> {
        crate::lock(&self.inner.core).machine.closed(ids)
    }
}

/// Every session made, held weakly, by its Library's root, for [`close`].
static SESSIONS: Mutex<Vec<(PathBuf, Weak<Inner>)>> = Mutex::new(Vec::new());

/// Renders `ids` of `library` are about to be deleted: each session leaves
/// any it adjusts, and their writes land, waiting up to [`CLOSE_WAIT`]
/// ([`History::close_paint`]). Never on the store thread, which the writes
/// need: there it is logged and skipped.
pub(crate) fn close(library: &Path, ids: &[RenderId]) -> AdjustClosed {
    let sessions: Vec<Arc<Inner>> = {
        let mut sessions = crate::lock(&SESSIONS);
        sessions.retain(|(_, s)| s.strong_count() > 0);
        sessions.iter().filter(|(l, _)| l == library).filter_map(|(_, s)| s.upgrade()).collect()
    };
    if !sessions.is_empty() && store().is_current() {
        tracing::error!(
            "adjust asked to close {} renders from the store thread: skipped",
            ids.len()
        );
        debug_assert!(false, "adjust closed on the store thread");
    } else {
        for session in sessions {
            session.close(ids);
        }
    }
    AdjustClosed::none_open()
}

impl Inner {
    /// Leave Adjust if it is on one of `ids`, then wait for their writes.
    fn close(&self, ids: &[RenderId]) {
        let adjusting = crate::lock(&self.core).machine.adjusting().cloned();
        if adjusting.is_some_and(|key| ids.contains(&key)) {
            self.on(Event::Leave);
        }
        let until = Instant::now() + CLOSE_WAIT;
        let mut core = crate::lock(&self.core);
        while core.machine.closed(ids).is_none() {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                tracing::error!("adjust: renders still open or writing after {CLOSE_WAIT:?}");
                return;
            }
            core = match self.changed.wait_timeout(core, left) {
                Ok((core, _)) => core,
                Err(e) => e.into_inner().0,
            };
        }
    }

    /// `key`'s pixels with paint, from the Library's paint worker; `None`
    /// when nothing is painted or no worker runs.
    fn painted(&self, key: &RenderId, stack: &Arc<CompletedStack>) -> Option<Receiver<Flattened>> {
        crate::paint::worker::flatten(self.history.root(), key.clone(), stack.clone())
    }

    fn on(&self, event: Event) {
        let mut updates = Vec::new();
        {
            let mut core = crate::lock(&self.core);
            self.feed(&mut core, event, &mut updates);
            crate::lock(&self.outbox).queue.extend(updates);
        }
        self.changed.notify_all();
        self.deliver();
    }

    /// `event` to the machine, its actions carried out, under the lock.
    fn feed(&self, core: &mut Core, event: Event, updates: &mut Vec<Update>) {
        let actions = core.machine.on(event);
        for action in actions {
            self.act(core, action, updates);
        }
    }

    /// A change of the value shown made by `to` from it and the panel,
    /// committed if `commit`.
    fn change(
        &self,
        commit: bool,
        to: impl FnOnce(&RenderAdjustments, &Panel) -> RenderAdjustments,
    ) {
        let mut updates = Vec::new();
        {
            let mut core = crate::lock(&self.core);
            let (Some(status), Some(open)) = (core.machine.status(), core.current.clone()) else {
                return;
            };
            if status.key != open.key {
                return;
            }
            let value = to(&status.value, &open.panel);
            self.feed(&mut core, Event::Set(value), &mut updates);
            if commit {
                self.feed(&mut core, Event::Commit, &mut updates);
            }
            crate::lock(&self.outbox).queue.extend(updates);
        }
        self.changed.notify_all();
        self.deliver();
    }

    fn act(&self, core: &mut Core, action: Action, updates: &mut Vec<Update>) {
        match action {
            Action::UploadReduced(id) => {
                if let Some(next) = core.next.take() {
                    core.current = Some(next);
                }
                let Some(open) = core.current.clone() else { return };
                core.epoch += 1;
                core.upload = Some((id, core.epoch));
                self.upload(id, core.epoch, open);
            }
            Action::FreeGpu => {
                core.upload = None;
                self.free(core.epoch);
            }
            Action::DrawPreview { id, value } => {
                let Some(open) = core.current.clone() else { return };
                let shown = core.shown.unwrap_or_else(|| whole(&open.stack));
                self.draw(id, open, value, shown);
            }
            Action::DrawFull { id, value } => {
                let Some(open) = core.current.clone() else { return };
                let shown = core.shown;
                self.settle(id, open, value, shown);
            }
            Action::Show(id) => match core.drawn.remove(&id) {
                Some(Drawn::Preview { picture, histogram }) => {
                    updates.push(Update::Preview { picture, histogram })
                }
                Some(Drawn::Settled { key, picture, histogram }) => {
                    updates.push(Update::Settled { key, picture, histogram })
                }
                None => tracing::error!("adjust: nothing drawn to show for {id:?}"),
            },
            Action::Write { key, id, value } => {
                core.changed.insert(key.clone());
                let writes = core
                    .writes
                    .entry(key.clone())
                    .or_insert_with(|| self.history.adjustment_writes(&key));
                let write = writes.send(value);
                let (me, words) = (self.me.clone(), self.engine.words().clone());
                let _written = store().write(move || {
                    let event = match write.save() {
                        Ok(_) => Event::Written { key, id },
                        Err(e) => {
                            tracing::warn!("adjust: render {} not written: {e}", key.as_str());
                            Event::WriteFailed { key, id, why: say!(&words, "adjust-write-failed") }
                        }
                    };
                    if let Some(inner) = me.upgrade() {
                        inner.on(event);
                    }
                });
            }
            Action::Report(Report::Status(status)) => updates.push(Update::Status(status)),
            Action::Report(Report::WriteFailed { why, .. }) => updates.push(Update::Notice(why)),
            Action::Report(Report::Closed { key, redraw }) => {
                core.writes.remove(&key);
                let stack = core.stacks.remove(&key);
                if core.current.as_ref().is_some_and(|o| o.key == key) {
                    core.current = None;
                }
                if let (true, Some(stack)) = (core.changed.remove(&key), stack) {
                    self.rewrite(key.clone(), stack);
                }
                updates.push(Update::Closed { key, redraw });
            }
        }
    }

    /// The open's read is in: the machine hears it, if it is still the
    /// open asked for.
    fn opened(&self, token: u64, read: Result<RenderAdjustments, Unread>) {
        let mut updates = Vec::new();
        {
            let mut core = crate::lock(&self.core);
            if core.reading.as_ref().is_none_or(|r| r.token != token) {
                return;
            }
            let Reading { key, stack, .. } = core.reading.take().expect("matched above");
            let words = self.engine.words();
            let value = match read {
                Ok(value) => value,
                Err(Unread::Damaged) => {
                    updates.push(Update::Notice(say!(words, "adjust-unread-damaged")));
                    RenderAdjustments::NONE
                }
                Err(Unread::Unreachable) => {
                    updates.push(Update::Notice(say!(words, "adjust-unread-unreachable")));
                    crate::lock(&self.outbox).queue.extend(updates);
                    drop(core);
                    self.deliver();
                    return;
                }
            };
            let panel = Panel::of(&stack);
            core.stacks.insert(key.clone(), stack.clone());
            core.next = Some(Opened { key: key.clone(), stack, panel });
            core.shown = None;
            self.feed(&mut core, Event::Open { key, value }, &mut updates);
            crate::lock(&self.outbox).queue.extend(updates);
        }
        self.changed.notify_all();
        self.deliver();
    }

    /// Make `open`'s GPU copies on the pool, from its stored pixels with
    /// paint, unless what iOS has left can't hold them.
    fn upload(&self, id: RequestId, epoch: u64, open: Opened) {
        let me = self.me.clone();
        let key = Key::of("adjust-upload", (open.key.as_str().to_string(), epoch));
        work::pool()
            .run(Lane::Interactive, key, work::root(), move || {
                let Some(inner) = me.upgrade() else { return };
                let event = match inner.make(&open) {
                    Ok(copies) => {
                        // A stale upload's copies are dropped here, and an
                        // older one never replaces a newer one's.
                        let current = crate::lock(&inner.core).upload == Some((id, epoch));
                        let mut held = crate::lock(&inner.copies);
                        if current && held.as_ref().is_none_or(|h| h.epoch < epoch) {
                            *held = Some(Held { epoch, key: open.key, copies });
                        }
                        Event::ReducedReady(id)
                    }
                    Err(why) => Event::ReducedFailed { id, why },
                };
                inner.on(event);
            })
            .detach();
    }

    fn make(&self, open: &Opened) -> Result<GpuCopies, String> {
        let (engine, words) = (&self.engine, self.engine.words());
        let stack = &open.stack;
        let texel = if stack.is_linear() { 8 } else { 16 };
        let need = RenderMemory::of(stack.width, stack.height, texel);
        if let Some(limit) = crate::engine::memory::limit()
            && need.gpu() > limit
        {
            tracing::info!("adjust: {need} won't fit in {} MB", limit >> 20);
            return Err(say!(words, "adjust-no-memory"));
        }
        let painted = match self.painted(&open.key, stack).map(|rx| rx.recv()) {
            None | Some(Ok(Ok(None))) => None,
            Some(Ok(Ok(Some(pixels)))) => Some(pixels),
            Some(Ok(Err(e))) => {
                tracing::warn!("adjust: render {}'s paint not read: {e}", open.key.as_str());
                return Err(crate::words::explain::Explain::explain(&e, words).text(words));
            }
            Some(Err(_)) => {
                tracing::warn!("adjust: the paint worker stopped before flattening");
                return Err(say!(words, "adjust-open-failed"));
            }
        };
        let pixels = painted.as_deref().unwrap_or(&stack.pixels);
        let generation = engine.generations().of(&open.key);
        GpuCopies::make(engine, stack, pixels, generation).map_err(|e| {
            tracing::warn!("adjust: render {}'s copies not made: {e}", open.key.as_str());
            say!(words, "adjust-open-failed")
        })
    }

    /// Free the copies of uploads up to `epoch`, on the pool, after any
    /// draw from them.
    fn free(&self, epoch: u64) {
        let me = self.me.clone();
        work::pool()
            .run(Lane::Interactive, Key::of("adjust-free", epoch), work::root(), move || {
                let Some(inner) = me.upgrade() else { return };
                let mut held = crate::lock(&inner.copies);
                if held.as_ref().is_some_and(|h| h.epoch <= epoch) {
                    *held = None;
                    drop(held);
                    deptheos_gpu::trim_pools(0);
                    tracing::debug!("adjust: GPU copies freed");
                }
            })
            .detach();
    }

    /// Draw a preview tick of `value` of `open` at `shown` on the pool.
    fn draw(&self, id: RequestId, open: Opened, value: RenderAdjustments, shown: Shown) {
        let me = self.me.clone();
        work::pool()
            .run(Lane::Interactive, Key::of("adjust-draw", id), work::root(), move || {
                let Some(inner) = me.upgrade() else { return };
                let drawn = inner.drawing_shown(&open, &value, shown, Draft::Tick);
                let mut updates = Vec::new();
                {
                    let mut core = crate::lock(&inner.core);
                    let event = match drawn {
                        Ok((picture, histogram)) => {
                            core.drawn.insert(id, Drawn::Preview { picture, histogram });
                            Event::PreviewDrawn(id)
                        }
                        Err(e) => {
                            tracing::warn!("adjust: a draw of {} failed: {e}", open.key.as_str());
                            let why = say!(inner.engine.words(), "adjust-draw-failed");
                            Event::DrawFailed { id, why }
                        }
                    };
                    inner.feed(&mut core, event, &mut updates);
                    // Not shown: stale.
                    if let Some(Drawn::Preview { picture, .. }) = core.drawn.remove(&id) {
                        inner.slot.give_back(picture);
                    }
                    crate::lock(&inner.outbox).queue.extend(updates);
                }
                inner.changed.notify_all();
                inner.deliver();
            })
            .detach();
    }

    /// Draw full picture `id` on the pool, the view on screen first
    /// ([`Update::OnScreen`], while `id` is still the one settled on), so
    /// the screen settles within its budget though the whole takes longer
    /// (ADR-138).
    fn settle(&self, id: RequestId, open: Opened, value: RenderAdjustments, shown: Option<Shown>) {
        let me = self.me.clone();
        work::pool()
            .run(Lane::Interactive, Key::of("adjust-settle", id), work::root(), move || {
                let Some(inner) = me.upgrade() else { return };
                if let Some(shown) = shown {
                    inner.on_screen(id, &open, &value, shown);
                }
                let drawn = inner.drawing(&open, &value);
                let mut updates = Vec::new();
                {
                    let mut core = crate::lock(&inner.core);
                    let event = match drawn {
                        Ok(drawn) => {
                            core.drawn.insert(id, drawn);
                            Event::FullDrawn(id)
                        }
                        Err(e) => {
                            tracing::warn!("adjust: a draw of {} failed: {e}", open.key.as_str());
                            let why = say!(inner.engine.words(), "adjust-draw-failed");
                            Event::DrawFailed { id, why }
                        }
                    };
                    inner.feed(&mut core, event, &mut updates);
                    core.drawn.remove(&id);
                    crate::lock(&inner.outbox).queue.extend(updates);
                }
                inner.changed.notify_all();
                inner.deliver();
            })
            .detach();
    }

    /// The settled picture of `shown`, handed over at once if full picture
    /// `id` is still the one settled on; a failure waits for the full draw.
    fn on_screen(&self, id: RequestId, open: &Opened, value: &RenderAdjustments, shown: Shown) {
        let drawn = self.drawing_shown(open, value, shown, Draft::Settled);
        let (picture, histogram) = match drawn {
            Ok(drawn) => drawn,
            Err(e) => {
                tracing::warn!("adjust: {}'s view on screen not drawn: {e}", open.key.as_str());
                return;
            }
        };
        let current = crate::lock(&self.core).machine.is_settling(id);
        if current {
            crate::lock(&self.outbox).queue.push_back(Update::OnScreen { picture, histogram });
            self.deliver();
        } else {
            tracing::debug!("adjust: a stale view on screen dropped");
            self.slot.give_back(picture);
        }
    }

    /// The full picture of `value` of `open`, its clipping counted over
    /// every pixel.
    fn drawing(&self, open: &Opened, value: &RenderAdjustments) -> crate::Result<Drawn> {
        let engine = &self.engine;
        let mut held = crate::lock(&self.copies);
        let (copies, constants) = Self::copies_for(&mut held, open, value)?;
        let crop = value.reframe.crop;
        let full = copies.draw_full(engine, &constants);
        let clipping = histogram::clipping(engine, &full, crop)?;
        let picture = Self::orient(open, value).presented(engine.present(&full, None)?);
        drop(full);
        let histogram =
            copies.histogram(engine, &constants, crop, Draft::Settled)?.with_clipping(clipping);
        Ok(Drawn::Settled { key: open.key.clone(), picture, histogram })
    }

    /// `value` of `open` drawn for `shown` as one flat picture, with the
    /// small draw's histogram.
    fn drawing_shown(
        &self,
        open: &Opened,
        value: &RenderAdjustments,
        shown: Shown,
        draft: Draft,
    ) -> crate::Result<(FlatPicture, Histogram)> {
        let engine = &self.engine;
        let mut held = crate::lock(&self.copies);
        let (copies, constants) = Self::copies_for(&mut held, open, value)?;
        let picture = copies.draw_preview(engine, &constants, shown, draft, &self.slot)?;
        let picture = picture.oriented(&Self::orient(open, value), &self.slot);
        let histogram = copies.histogram(engine, &constants, value.reframe.crop, draft)?;
        Ok((picture, histogram))
    }

    /// How `value` turns and flips `open`'s pictures at the hand-off.
    fn orient(open: &Opened, value: &RenderAdjustments) -> Orient {
        let picture = Size { width: open.stack.width, height: open.stack.height };
        Orient::of(value.reframe, picture)
    }

    /// The copies of `open` and the constants of `value` on them.
    fn copies_for<'a>(
        held: &'a mut Option<Held>,
        open: &Opened,
        value: &RenderAdjustments,
    ) -> crate::Result<(&'a mut GpuCopies, Box<deptheos_render::dng_render::Constants>)> {
        let copies = match held.as_mut() {
            Some(held) if held.key == open.key => &mut held.copies,
            _ => return Err(crate::Error::InvalidState("no GPU copies to draw from".into())),
        };
        let look = Look::adjusting(&open.stack, &value.adjustments, value.reframe);
        let Draw::Rendered(constants) = look.resolve() else {
            return Err(crate::Error::InvalidState("Adjust draws on the GPU".into()));
        };
        Ok((copies, constants))
    }

    fn eyedrop(&self, at: (f64, f64)) {
        let (token, open, value) = {
            let mut core = crate::lock(&self.core);
            let (Some(status), Some(open)) = (core.machine.status(), core.current.clone()) else {
                return;
            };
            if status.key != open.key || status.phase != Phase::Ready {
                return;
            }
            core.eyedrops += 1;
            (core.eyedrops, open, status.value)
        };
        let me = self.me.clone();
        work::pool()
            .run(Lane::Interactive, Key::of("adjust-eyedrop", token), work::root(), move || {
                let Some(inner) = me.upgrade() else { return };
                let painted = inner
                    .painted(&open.key, &open.stack)
                    .and_then(|rx| rx.recv().ok())
                    .and_then(|flat| flat.ok().flatten());
                let pixels = painted.as_deref().unwrap_or(&open.stack.pixels);
                let white = eyedrop(&open.stack, pixels, value.reframe, at);
                inner.eyedropped(token, &open.key, white);
            })
            .detach();
    }

    /// The eyedropper's white is in: set and committed, if it is still the
    /// latest asked on the render adjusted.
    fn eyedropped(
        &self,
        token: u64,
        key: &RenderId,
        white: Result<deptheos_render::WhiteBalance, super::eyedropper::Refused>,
    ) {
        let current = {
            let core = crate::lock(&self.core);
            core.eyedrops == token && core.machine.adjusting() == Some(key)
        };
        if !current {
            return;
        }
        match white {
            Ok(white_balance) => self.change(true, |current, _| RenderAdjustments {
                adjustments: Adjustments { white_balance, ..current.adjustments.clone() },
                ..current.clone()
            }),
            Err(refused) => {
                let text = refused.say(self.engine.words());
                crate::lock(&self.outbox).queue.push_back(Update::Notice(text));
                self.deliver();
            }
        }
    }

    /// Draw `key`'s thumbnail again under what it was left with.
    fn rewrite(&self, key: RenderId, stack: Arc<CompletedStack>) {
        let painted = self.painted(&key, &stack);
        let me = self.me.clone();
        let done = key.clone();
        self.history.rewrite_thumbnail_then(
            self.engine.clone(),
            key,
            stack,
            painted,
            move |rewrote| match rewrote {
                Ok(crate::library::thumbnails::rewrite::Rewrote::Written) => {
                    if let Some(inner) = me.upgrade() {
                        crate::lock(&inner.outbox).queue.push_back(Update::Rewritten(done));
                        inner.deliver();
                    }
                }
                Ok(_) => {}
                Err(e) => tracing::warn!("adjust: render {}'s thumbnail: {e}", done.as_str()),
            },
        );
    }

    /// Deliver queued updates outside every lock, unless another thread
    /// already is.
    fn deliver(&self) {
        {
            let mut outbox = crate::lock(&self.outbox);
            if outbox.delivering {
                return;
            }
            outbox.delivering = true;
        }
        let _unwinding = Unwinding(&self.outbox);
        loop {
            let next = {
                let mut outbox = crate::lock(&self.outbox);
                match outbox.queue.pop_front() {
                    Some(update) => update,
                    None => {
                        outbox.delivering = false;
                        return;
                    }
                }
            };
            self.observer.update(next);
        }
    }
}

/// The whole of `stack` at fit, before the viewer says what it shows.
fn whole(stack: &CompletedStack) -> Shown {
    let picture = Size { width: stack.width, height: stack.height };
    let long = picture.width.max(picture.height).max(1);
    Shown {
        zoom: (super::preview::FIT_LONG as f32 / long as f32).min(1.0),
        visible: deptheos_render::dng_render::Region::whole(picture.width, picture.height),
    }
}

/// Frees the outbox if its deliverer unwinds.
struct Unwinding<'a>(&'a Mutex<Outbox>);

impl Drop for Unwinding<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            crate::lock(self.0).delivering = false;
        }
    }
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
