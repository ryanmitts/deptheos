//! The paint worker against a model, in random sequences of strokes,
//! undo, redo, reset, close and reopen, save, relink, present, crash and
//! full disk. The model is the pixels, undo and redo stacks of whole
//! copies, and the pixels last saved. After every step the worker's
//! pixels and counts match, nothing is unsaveable or set aside, and a
//! present is answered once. A full disk is SQLite's (`store::tests::full`);
//! a crash sends nothing more to the store thread (a kill mid-commit is
//! `store_kill.rs`'s).
//!
//! The model follows the worker where it can't see: whether a stroke that
//! changed no pixel was a step, and the oldest steps shed
//! (`UndoStack::shed`), allowed only once the disk was full.
//!
//! Each run prints its seed; `PROPTEST_RNG_SEED=<seed>` replays it and
//! `PROPTEST_CASES` sets the count.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex, OnceLock};

use proptest::prelude::*;
use proptest::test_runner::{Config, RngSeed};
use proptest_state_machine::{ReferenceStateMachine, StateMachineTest};

use super::{Command, Output, PaintStore, Report, SourceLoader, Worker};
use crate::frames::sources::FrameRef;
use crate::library::history::History;
use crate::paint::brush::{Brush, Dab};
use crate::paint::source::SourceKind;
use crate::paint::store::tests::full;
use crate::paint::tile::TileData;
use crate::test_support::engine;
use crate::{CompletedStack, Engine, RenderId};

/// 3 × 2 tiles, the last column and row partly outside.
const WIDTH: u32 = 300;
const HEIGHT: u32 = 200;
/// The frames painted from, each one flat value.
const FRAMES: [u16; 3] = [9000, 40000, 61000];

fn key() -> RenderId {
    RenderId::parse(&format!("{:032x}", 7)).unwrap()
}

/// What a stroke paints from.
#[derive(Clone, Copy, Debug)]
enum From {
    Original,
    Frame(usize),
}

#[derive(Clone, Debug)]
enum Step {
    /// One dab; `scrub` picks the source as the rail does.
    Stroke {
        from: From,
        x: u16,
        y: u16,
        radius: u8,
        hardness: u8,
        opacity: u8,
        scrub: bool,
    },
    Undo,
    Redo,
    Reset,
    Close,
    Open,
    Save,
    /// The frames' folder moved and was relinked.
    Relink,
    Present,
    Crash,
    Full(bool),
}

/// Generates the steps; the model is in [`Rig`].
#[derive(Clone, Debug)]
struct Steps;

impl ReferenceStateMachine for Steps {
    type State = Self;
    type Transition = Step;

    fn init_state() -> BoxedStrategy<Self> {
        Just(Steps).boxed()
    }

    fn transitions(_: &Self) -> BoxedStrategy<Step> {
        let from =
            prop_oneof![1 => Just(From::Original), 3 => (0..FRAMES.len()).prop_map(From::Frame)];
        let stroke =
            (
                from,
                0..WIDTH as u16 + 20,
                0..HEIGHT as u16 + 20,
                2..90u8,
                1..=10u8,
                1..=10u8,
                any::<bool>(),
            )
                .prop_map(|(from, x, y, radius, hardness, opacity, scrub)| {
                    Step::Stroke { from, x, y, radius, hardness, opacity, scrub }
                });
        prop_oneof![
            8 => stroke,
            3 => Just(Step::Undo),
            2 => Just(Step::Redo),
            1 => Just(Step::Reset),
            1 => Just(Step::Close),
            2 => Just(Step::Open),
            1 => Just(Step::Save),
            1 => Just(Step::Relink),
            1 => Just(Step::Present),
            1 => Just(Step::Crash),
            1 => any::<bool>().prop_map(Step::Full),
        ]
        .boxed()
    }

    fn apply(state: Self, _: &Step) -> Self {
        state
    }
}

/// The model.
struct Model {
    original: Vec<u16>,
    image: Vec<u16>,
    undo: Vec<Vec<u16>>,
    redo: Vec<Vec<u16>>,
    /// What reopening after a crash shows.
    saved: Vec<u16>,
    open: bool,
    full: bool,
    /// The disk was full since the history began, so it may shed steps.
    may_shed: bool,
    /// The pixels this step's commit writes.
    due: Option<Vec<u16>>,
}

impl Model {
    /// [`Rig::settle`] says if it held.
    fn commit(&mut self) {
        self.due = Some(self.image.clone());
    }

    /// One dab from `source` (`None`: the original), as a stroke
    /// composites it, rounded once.
    fn dab(&mut self, brush: &Brush, dab: Dab, source: Option<u16>) {
        let r = brush.radius;
        let clamp = |v: f32, max: u32| (v.max(0.0) as u32).min(max);
        let (x0, x1) = (clamp((dab.x - r).floor(), WIDTH), clamp((dab.x + r).ceil(), WIDTH));
        let (y0, y1) = (clamp((dab.y - r).floor(), HEIGHT), clamp((dab.y + r).ceil(), HEIGHT));
        for gy in y0..y1 {
            let dy = gy as f32 + 0.5 - dab.y;
            for gx in x0..x1 {
                let dx = gx as f32 + 0.5 - dab.x;
                let f = brush.falloff(dx.hypot(dy));
                if f == 0.0 {
                    continue;
                }
                let c = 0f32.max(f * dab.pressure);
                let alpha = brush.opacity * c * 1.0;
                let at = 3 * (gy * WIDTH + gx) as usize;
                for ch in 0..3 {
                    let s = f32::from(source.unwrap_or(self.original[at + ch]));
                    let b = f32::from(self.image[at + ch]);
                    self.image[at + ch] = ((b + (s - b) * alpha).clamp(0.0, 65535.0) + 0.5) as u16;
                }
            }
        }
    }
}

/// A running worker, driven on the test's thread.
struct Running {
    worker: Worker,
    store: PaintStore,
}

/// The worker on a Library of its own, and the model.
struct Rig {
    engine: Arc<Engine>,
    library: tempfile::TempDir,
    stack: Arc<CompletedStack>,
    /// The render's folder, whose disk fills.
    render: PathBuf,
    running: Option<Running>,
    /// `a` or `b` beside the Library.
    frames_in: PathBuf,
    frames: Vec<FrameRef>,
    /// Whether each `Command::Present` answer had a display.
    presented: Arc<Mutex<Vec<bool>>>,
    failed: Arc<Mutex<Vec<String>>>,
    shown: Arc<Mutex<Vec<SourceKind>>>,
    model: Model,
}

impl Rig {
    fn new() -> Self {
        let engine = engine().expect("checked first");
        let library = tempfile::tempdir().unwrap();
        let history = Arc::new(History::open(library.path().join("Library")).unwrap());
        let render = history.render_path(&key());
        std::fs::create_dir_all(&render).unwrap();
        std::fs::write(render.join("render.json"), "{}").unwrap();
        let frames_in = library.path().join("a");
        std::fs::create_dir_all(&frames_in).unwrap();
        let paths: Vec<PathBuf> =
            (0..FRAMES.len()).map(|i| frames_in.join(format!("{i}.png"))).collect();
        for (i, path) in paths.iter().enumerate() {
            std::fs::write(path, [i as u8]).unwrap();
        }
        let frames = history.frames().refs(&paths).unwrap();
        let pixels: Vec<u16> =
            (0..3 * WIDTH * HEIGHT).map(|i| ((i * 37) % 50000) as u16 + 5000).collect();
        let stack = Arc::new(CompletedStack::srgb(WIDTH, HEIGHT, pixels.clone(), Vec::new()));
        let model = Model {
            original: pixels.clone(),
            image: pixels.clone(),
            undo: Vec::new(),
            redo: Vec::new(),
            saved: pixels,
            open: false,
            full: false,
            may_shed: false,
            due: None,
        };
        // Released so the worker can open the Library afresh.
        drop(history);
        let mut rig = Self {
            engine,
            library,
            stack,
            render,
            running: None,
            frames_in,
            frames,
            presented: Arc::default(),
            failed: Arc::default(),
            shown: Arc::default(),
            model,
        };
        rig.start();
        rig
    }

    /// Start a worker as the app does.
    fn start(&mut self) {
        let history = Arc::new(History::open(self.library.path().join("Library")).unwrap());
        let (presented, failed) = (self.presented.clone(), self.failed.clone());
        let shown = self.shown.clone();
        let output: Report = Arc::new(move |output| match output {
            Output::Presented { display, .. } => presented.lock().unwrap().push(display.is_some()),
            Output::Failed(failure) => failed.lock().unwrap().push(failure.text),
            Output::Source { kind, .. } => shown.lock().unwrap().push(kind),
            _ => {}
        });
        let store = PaintStore::new(self.engine.clone(), output.clone());
        let mut worker = Worker::new(self.engine.clone(), history, output, store.clone());
        // Small, so undo snapshots spill.
        worker.budget = crate::engine::memory::Budget::Fixed(3 * TileData::BYTES);
        self.running = Some(Running { worker, store });
    }

    fn worker(&mut self) -> &mut Worker {
        &mut self.running.as_mut().expect("running").worker
    }

    /// Run `command` as the worker's loop does, then wait for the store
    /// thread, so commits land before the disk fills or empties.
    fn send(&mut self, command: Command) {
        let worker = self.worker();
        worker.handle(command);
        worker.drain_store();
        worker.flush();
    }

    /// Frame `i` where the Library says it is now.
    fn frame(&mut self, i: usize) -> PathBuf {
        let frames = self.worker().history.frames().clone();
        frames.paths(&self.frames[i..=i]).unwrap().remove(0)
    }

    fn flat(&self, value: u16) -> SourceLoader {
        let (engine, stack) = (self.engine.clone(), self.stack.clone());
        Arc::new(move |pixels| {
            let flat = vec![value; stack.pixels.len()];
            let display = engine
                .present_pixels(crate::Look::stored(&stack), &flat)
                .map_err(|e| e.to_string())?;
            Ok((pixels.then_some(flat), display))
        })
    }

    fn undo_counts(&mut self) -> [usize; 2] {
        let key = key();
        self.worker().docs.get(&key).map_or([0, 0], |d| d.history.counts())
    }

    fn revision(&mut self) -> Option<u64> {
        let key = key();
        self.worker().docs.get(&key).map(|d| d.revision)
    }

    fn stroke(
        &mut self,
        from: From,
        at: [u16; 2],
        [radius, hardness, opacity]: [u8; 3],
        scrub: bool,
    ) {
        let brush = Brush {
            radius: f32::from(radius),
            hardness: f32::from(hardness) / 10.0,
            opacity: f32::from(opacity) / 10.0,
            flow: 1.0,
            tolerance: None,
        };
        let dab = Dab { x: f32::from(at[0]) + 0.25, y: f32::from(at[1]) + 0.75, pressure: 1.0 };
        let (kind, load, value) = match from {
            From::Original => (SourceKind::Original, self.flat(0), None),
            From::Frame(i) => {
                (SourceKind::Frame { path: self.frame(i) }, self.flat(FRAMES[i]), Some(FRAMES[i]))
            }
        };
        let before = self.revision();
        self.send(Command::SetSource { kind, load, scrub, pick: 0 });
        self.send(Command::BeginStroke { brush });
        self.send(Command::Dabs(vec![dab]));
        self.send(Command::EndStroke);
        if !self.model.open {
            return;
        }
        let was = self.model.image.clone();
        self.model.dab(&brush, dab, value);
        // A changed pixel is a step; moved coverage alone may be too.
        let stepped = self.revision() != before;
        assert!(stepped || self.model.image == was, "a stroke that changed pixels is a step");
        if stepped {
            self.model.undo.push(was);
            self.model.redo.clear();
            self.model.commit();
        }
        if self.model.full {
            self.model.may_shed = true;
        }
    }

    fn step(&mut self, step: Step) {
        self.run(step);
        self.settle();
    }

    /// The step's commit holds unless it failed, which only a full disk
    /// may cause.
    fn settle(&mut self) {
        let failed = std::mem::take(&mut *self.failed.lock().unwrap());
        let model = &mut self.model;
        assert!(failed.is_empty() || model.full, "{failed:?}");
        if let Some(image) = model.due.take() {
            if failed.is_empty() {
                model.saved = image;
            }
        } else {
            assert!(failed.is_empty(), "a failure with no commit: {failed:?}");
        }
    }

    /// [`Self::run_step`]; every change to the pixels is a new generation
    /// of the render.
    fn run(&mut self, step: Step) {
        let before = (self.revision(), self.engine.generations().of(&key()));
        self.run_step(step);
        let after = (self.revision(), self.engine.generations().of(&key()));
        // Tests share the engine and the render's id, so only this way holds.
        if let (Some(was), Some(now)) = (before.0, after.0)
            && was != now
        {
            assert_ne!(before.1, after.1, "a change is a new generation");
        }
    }

    fn run_step(&mut self, step: Step) {
        let model = &mut self.model;
        match step {
            Step::Stroke { from, x, y, radius, hardness, opacity, scrub } => {
                self.stroke(from, [x, y], [radius, hardness, opacity], scrub)
            }
            Step::Undo => {
                if model.open
                    && let Some(image) = model.undo.pop()
                {
                    model.redo.push(std::mem::replace(&mut model.image, image));
                    model.commit();
                }
                self.send(Command::Undo);
            }
            Step::Redo => {
                if model.open
                    && let Some(image) = model.redo.pop()
                {
                    model.undo.push(std::mem::replace(&mut model.image, image));
                    model.commit();
                }
                self.send(Command::Redo);
            }
            Step::Reset => {
                let untouched =
                    model.image == model.original && model.undo.is_empty() && model.redo.is_empty();
                if model.open && !untouched {
                    model.image = model.original.clone();
                    model.undo.clear();
                    model.redo.clear();
                    model.may_shed = model.full;
                    model.commit();
                }
                self.send(Command::Reset);
            }
            Step::Close => {
                if model.open {
                    model.commit();
                    model.open = false;
                }
                self.send(Command::Close);
            }
            Step::Open => {
                // Opening commits what an earlier commit failed to write.
                model.commit();
                model.open = true;
                let stack = self.stack.clone();
                self.send(Command::Open { key: key(), stack });
                // Opening writes nothing, so a full disk can't make it
                // unsaveable.
                let unsaveable = self.worker().state().and_then(|s| s.unsaveable);
                assert_eq!(unsaveable, None, "nothing makes the paint unsaveable");
            }
            Step::Save => {
                model.commit();
                let (reply, done) = channel();
                self.send(Command::Save(reply));
                let saved = done.recv().unwrap();
                let failed = !self.failed.lock().unwrap().is_empty();
                assert_eq!(saved, !failed, "the save says whether it saved");
            }
            Step::Present => {
                // The doc held, else the last save.
                let key = key();
                let held = self.worker().docs.get(&key).is_some();
                let shows = if held { &self.model.image } else { &self.model.saved };
                let plain = *shows == self.model.original;
                let stack = self.stack.clone();
                self.send(Command::Present { key, stack });
                let answers = std::mem::take(&mut *self.presented.lock().unwrap());
                // Paint back at the original may still have a display.
                assert_eq!(answers.len(), 1, "a present is answered once");
                assert!(answers[0] || plain, "a painted render presented without its paint");
            }
            Step::Relink => {
                let other = if self.frames_in.ends_with("a") { "b" } else { "a" };
                let to = self.library.path().join(other);
                std::fs::rename(&self.frames_in, &to).unwrap();
                let frames = self.worker().history.frames().clone();
                let moves = frames.relink(&self.frames_in, &to, None).unwrap();
                self.frames_in = to;
                self.send(Command::Relink(moves));
            }
            Step::Crash => {
                let Running { worker, store } = self.running.take().expect("running");
                drop(worker);
                store.leave_all();
                let model = &mut self.model;
                model.image = model.saved.clone();
                model.undo.clear();
                model.redo.clear();
                model.open = false;
                model.may_shed = model.full;
                self.start();
            }
            Step::Full(full) => {
                full::set(&self.render, full);
                model.full = full;
                model.may_shed |= full;
            }
        }
    }

    fn check(&mut self) {
        let (reply, flat) = channel();
        let stack = self.stack.clone();
        self.send(Command::Flatten { key: key(), stack, reply });
        let pixels = match flat.recv().unwrap() {
            Ok(Some(pixels)) => pixels,
            Ok(None) => self.model.original.clone(),
            Err(e) => panic!("the paint couldn't be read: {e}"),
        };
        assert!(pixels == self.model.image, "the worker's pixels are the model's");
        if self.model.open {
            let [undo, redo] = self.undo_counts();
            let model = &mut self.model;
            if undo < model.undo.len() && model.may_shed {
                model.undo.drain(..model.undo.len() - undo);
            }
            assert_eq!([undo, redo], [model.undo.len(), model.redo.len()], "undo and redo");
        }
        let render = self.worker().history.render_path(&key());
        let aside = std::fs::read_dir(&render)
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with("paint.unreadable-"));
        assert!(!aside, "nothing is set aside: nothing was damaged");
        // Painted-from follows the relink.
        let frames: Vec<PathBuf> = (0..FRAMES.len()).map(|i| self.frame(i)).collect();
        if let Some(state) = self.worker().state() {
            for kind in state.painted_from {
                if let SourceKind::Frame { path } = kind {
                    assert!(frames.contains(&path), "{} followed the relink", path.display());
                }
            }
        }
    }
}

struct WorkerTest;

impl StateMachineTest for WorkerTest {
    type SystemUnderTest = Rig;
    type Reference = Steps;

    fn init_test(_: &Steps) -> Rig {
        let mut rig = Rig::new();
        rig.check();
        rig
    }

    /// Checks drive the worker, so they run here, not in
    /// `check_invariants`.
    fn apply(mut rig: Rig, _: &Steps, step: Step) -> Rig {
        rig.step(step);
        rig.check();
        rig
    }
}

/// 64 cases or `PROPTEST_CASES`; `PROPTEST_RNG_SEED` replays a seed.
fn config() -> Config {
    static SEED: OnceLock<u64> = OnceLock::new();
    let seed = *SEED.get_or_init(|| {
        let seed = std::env::var("PROPTEST_RNG_SEED")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(rand_seed);
        eprintln!("paint model: seed {seed} (PROPTEST_RNG_SEED={seed} runs it again)");
        seed
    });
    let cases = std::env::var("PROPTEST_CASES").ok().and_then(|s| s.parse().ok()).unwrap_or(64);
    Config { cases, rng_seed: RngSeed::Fixed(seed), failure_persistence: None, ..Config::default() }
}

fn rand_seed() -> u64 {
    use std::hash::{BuildHasher, RandomState};
    RandomState::new().hash_one(std::time::SystemTime::now())
}

proptest! {
    #![proptest_config(config())]
    #[test]
    fn the_worker_follows_its_model(
        (initial, steps, seen) in Steps::sequential_strategy(1..40)
    ) {
        if engine().is_none() {
            return Ok(());
        }
        WorkerTest::test_sequential(config(), initial, steps, seen);
    }
}

/// After a relink, a moved frame's loader never runs: a stroke from it,
/// only scrubbed to, is refused.
#[test]
fn a_relink_drops_the_loaders_of_frames_that_moved() {
    if engine().is_none() {
        return;
    }
    let mut rig = Rig::new();
    rig.step(Step::Open);
    let ran = Arc::new(AtomicBool::new(false));
    let load: SourceLoader = {
        let (flat, ran) = (rig.flat(FRAMES[0]), ran.clone());
        Arc::new(move |pixels| {
            ran.store(pixels, Ordering::Relaxed);
            flat(pixels)
        })
    };
    let kind = SourceKind::Frame { path: rig.frame(0) };
    rig.send(Command::SetSource { kind, load, scrub: true, pick: 0 });
    assert!(!ran.load(Ordering::Relaxed), "scrubbed to: its display alone");
    rig.step(Step::Relink);
    let brush = Brush { radius: 40.0, ..Brush::default() };
    rig.send(Command::BeginStroke { brush });
    rig.send(Command::Dabs(vec![Dab { x: 50.0, y: 50.0, pressure: 1.0 }]));
    rig.send(Command::EndStroke);
    assert!(!ran.load(Ordering::Relaxed), "a loader made for the frame's old place ran");
    let key = key();
    let painted = rig.worker().docs.get(&key).is_some_and(|d| d.is_painted());
    assert!(!painted, "the stroke was refused");
}

/// A stroke from a frame scrubbed to shows its full-size display.
#[test]
fn a_stroke_from_a_frame_scrubbed_to_shows_it_at_full_size() {
    if engine().is_none() {
        return;
    }
    let mut rig = Rig::new();
    rig.step(Step::Open);
    let kind = SourceKind::Frame { path: rig.frame(0) };
    let load = rig.flat(FRAMES[0]);
    rig.send(Command::SetSource { kind: kind.clone(), load, scrub: true, pick: 0 });
    assert_eq!(rig.shown.lock().unwrap().len(), 1, "the scrub shows it");
    let brush = Brush { radius: 40.0, ..Brush::default() };
    rig.send(Command::BeginStroke { brush });
    rig.send(Command::EndStroke);
    assert_eq!(*rig.shown.lock().unwrap(), [kind.clone(), kind], "shown again in full");
}

/// A stroke, and an undo, each survive a crash right after.
#[test]
fn a_change_survives_a_crash_right_after_it() {
    if engine().is_none() {
        return;
    }
    let mut rig = Rig::new();
    rig.step(Step::Open);
    let stroke = |x| Step::Stroke {
        from: From::Frame(1),
        x,
        y: 60,
        radius: 30,
        hardness: 5,
        opacity: 10,
        scrub: false,
    };
    rig.step(stroke(60));
    let painted = rig.model.image.clone();
    assert_ne!(painted, rig.model.original, "the stroke painted");
    rig.step(Step::Crash);
    rig.check();
    rig.step(Step::Open);
    rig.step(stroke(200));
    rig.step(Step::Undo);
    rig.step(Step::Crash);
    rig.check();
    assert!(rig.model.image == painted, "the first stroke is there, the undone one isn't");
}

/// A memory warning evicts all spilled history, and undo still works.
#[test]
fn a_memory_warning_trims_the_undo_history() {
    if engine().is_none() {
        return;
    }
    let mut rig = Rig::new();
    rig.worker().budget = crate::engine::memory::Budget::Fixed(1 << 30);
    rig.step(Step::Open);
    for x in [40, 150, 260] {
        rig.step(Step::Stroke {
            from: From::Frame(2),
            x,
            y: 100,
            radius: 40,
            hardness: 5,
            opacity: 10,
            scrub: false,
        });
    }
    assert!(rig.worker().docs.charged() > 0, "the history is in RAM, under its budget");
    rig.send(Command::Shed(crate::engine::memory::Shed::Moderate));
    assert_eq!(rig.worker().docs.charged(), 0, "written to the spill, let go of");
    for _ in 0..3 {
        rig.step(Step::Undo);
        rig.check();
    }
    assert!(rig.model.image == rig.model.original, "every stroke undone");
}

/// Save reports failure while the disk is full, success after.
#[test]
fn a_save_says_whether_it_saved() {
    if engine().is_none() {
        return;
    }
    let mut rig = Rig::new();
    rig.step(Step::Open);
    rig.step(Step::Full(true));
    let stroke = Step::Stroke {
        from: From::Frame(0),
        x: 100,
        y: 100,
        radius: 60,
        hardness: 5,
        opacity: 10,
        scrub: false,
    };
    rig.step(stroke);
    let save = |rig: &mut Rig| {
        let (reply, done) = channel();
        rig.send(Command::Save(reply));
        done.recv().unwrap()
    };
    assert!(!save(&mut rig), "the disk is full: not saved");
    rig.failed.lock().unwrap().clear();
    rig.step(Step::Full(false));
    assert!(save(&mut rig), "saved once there is room");
}
