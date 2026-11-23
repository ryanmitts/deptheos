//! The paint worker on a real engine: strokes, patched tiles, and history
//! across render switches.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use deptheos_app::library::history::History;
use deptheos_app::paint::{
    Brush, Command, Dab, Output, PaintState, PaintWorker, SourceKind, SourceLoader,
};
use deptheos_app::test_support::engine;
use deptheos_app::{CompletedStack, Engine, PresentedImage, RenderId, SessionOptions};

/// A frame the render didn't stack is lined up through another render that
/// did, listed once per open; a frame no render stacked says why it can't be
/// painted from.
#[test]
fn a_frame_outside_the_render_lines_up_through_a_render_listed_once() {
    use deptheos_app::library::history::StackId;
    use deptheos_app::paint::{Failure, Siblings, Told, frame_loader};
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let history = Arc::new(History::open(dir.path().join("Library")).unwrap());
    let (width, height) = (64, 48);
    let frame = |name: &str| {
        let path = dir.path().join(name);
        image::RgbImage::from_pixel(width, height, image::Rgb([200, 100, 50])).save(&path).unwrap();
        path
    };
    let [a, b, c] = [frame("a.png"), frame("b.png"), frame("c.png")];
    let thumbnail = image::RgbImage::from_pixel(1000, 750, image::Rgb([9, 9, 9]));
    let save = |paths: Vec<PathBuf>| {
        let stack = CompletedStack::srgb(width, height, vec![30000; 3 * 64 * 48], paths.clone());
        let render = history.save_render(&paths, &stack, &thumbnail, &StackId::new(), None);
        (render.unwrap().id, Arc::new(stack))
    };
    let (painted, stack) = save(vec![a.clone()]);
    save(vec![a.clone(), b.clone()]);
    let siblings = Siblings::listed_once(history.clone(), painted);
    let told: Told = Arc::new(|e| Failure::user(e.to_string()));
    let load = |path: &PathBuf| {
        let (paths, path) = (stack.paths.clone(), path.clone());
        let loader = frame_loader(
            engine.clone(),
            stack.clone(),
            paths,
            path,
            siblings.clone(),
            told.clone(),
        );
        loader(false).map(|_| ())
    };
    load(&a).expect("its own frame");
    load(&b).expect("lined up through the other render");
    // Listed already: a render saved since isn't looked at.
    save(vec![a.clone(), c.clone()]);
    let refused = load(&c).expect_err("no render listed stacked it");
    assert!(refused.text.contains("isn't in this render"), "{}", refused.text);
}

/// A scrubbed frame shows at half size without pixels; picked, it comes whole.
#[test]
fn a_scrubbed_frame_is_shown_from_half_size_and_a_picked_one_whole() {
    use deptheos_app::library::history::StackId;
    use deptheos_app::paint::{Failure, Siblings, Told, frame_loader};
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let history = Arc::new(History::open(dir.path().join("Library")).unwrap());
    // Over 1024 px wide: the display has a half-size level.
    let (width, height) = (1300, 900);
    let path = dir.path().join("a.png");
    image::RgbImage::from_pixel(width, height, image::Rgb([200, 100, 50])).save(&path).unwrap();
    let pixels = vec![30000; (3 * width * height) as usize];
    let stack = Arc::new(CompletedStack::srgb(width, height, pixels, vec![path.clone()]));
    let thumbnail = image::RgbImage::from_pixel(1000, 750, image::Rgb([9, 9, 9]));
    let paths = vec![path.clone()];
    let render = history.save_render(&paths, &stack, &thumbnail, &StackId::new(), None).unwrap();
    let siblings = Siblings::listed_once(history.clone(), render.id);
    let told: Told = Arc::new(|e| Failure::user(e.to_string()));
    let load = frame_loader(engine.clone(), stack.clone(), paths, path, siblings, told);

    let (pixels, scrubbed) = load(false).unwrap();
    assert!(pixels.is_none());
    assert_eq!((scrubbed.first_level, scrubbed.width(), scrubbed.height()), (1, width, height));
    assert_eq!(scrubbed.levels[0].width, width.div_ceil(2));
    let (pixels, picked) = load(true).unwrap();
    assert_eq!(pixels.map(|p| p.len()), Some((3 * width * height) as usize));
    assert_eq!((picked.first_level, picked.levels[0].width), (0, width));
    assert!(scrubbed.bytes() * 3 < picked.bytes(), "about a quarter");
}

/// A frame to paint from in a shared folder; `gone.png` is never made.
fn frame_file(name: &str) -> PathBuf {
    static FOLDER: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    let path = FOLDER.get_or_init(|| tempfile::tempdir().unwrap()).path().join(name);
    if name != "gone.png" {
        std::fs::write(&path, name).unwrap();
    }
    path
}

/// An sRGB stack with some texture, `width` × `height`.
fn stack(width: u32, height: u32, seed: u32) -> Arc<CompletedStack> {
    let pixels = (0..3 * width * height).map(|i| ((i * 37 + seed) % 50000) as u16 + 5000).collect();
    let paths = vec![PathBuf::from("a.png"), PathBuf::from("b.png")];
    Arc::new(CompletedStack {
        options: SessionOptions::default(),
        ..CompletedStack::srgb(width, height, pixels, paths)
    })
}

/// A source of one flat value, shown through `engine`.
fn flat_source(engine: &Arc<Engine>, stack: &Arc<CompletedStack>, value: u16) -> SourceLoader {
    let (engine, stack) = (engine.clone(), stack.clone());
    Arc::new(move |with_pixels| {
        let pixels = vec![value; stack.pixels.len()];
        let display = engine.present_pixels(&stack, &pixels).map_err(|e| e.to_string())?;
        Ok((with_pixels.then_some(pixels), display))
    })
}

fn key(n: u64) -> RenderId {
    RenderId::parse(&format!("{n:032x}")).unwrap()
}

/// A history, where the worker keeps each render's paint.
struct Renders {
    _root: tempfile::TempDir,
    history: Arc<History>,
}

impl Renders {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let history = Arc::new(History::open(root.path()).unwrap());
        Self { _root: root, history }
    }

    /// Render `n`'s directory with a listed manifest, so a delete takes it.
    fn dir(&self, n: u64) -> PathBuf {
        let dir = self.history.render_path(&key(n));
        std::fs::create_dir_all(&dir).unwrap();
        let frame = self.history.root().join("Frames/1.png");
        std::fs::create_dir_all(frame.parent().unwrap()).unwrap();
        std::fs::write(&frame, b"1").unwrap();
        let frames = self.history.frames().refs(&[frame]).unwrap();
        let manifest = serde_json::json!({
            "schema": 1, "created_unix_ms": 5, "frames": frames, "included": [true],
            "stacker": {"algorithm": "pyramid", "smoothness": 0.5},
            "options": {
                "align": true, "align_mode": "grid", "crop": true, "reverse_check": true,
                "raw_backend": "libraw", "camera_profile": "adobe_standard", "order": "name",
                "grid": {
                    "horizontal_pct": 5.0, "vertical_pct": 5.0, "rotation_deg": 1.0,
                    "magnification_pct": 5.0, "apply_brightness": false,
                },
            },
            "alignment": [null], "crop": null, "reversed": false,
            "result": {"width": 4, "height": 3},
            "color": {"kind": "srgb", "sixteen_bit": false, "profile": false},
            "frame_map": false, "stack": key(n).as_str(), "number": 1,
        });
        std::fs::write(dir.join("render.json"), manifest.to_string()).unwrap();
        assert!(self.history.render(&key(n)).is_ok(), "listed");
        dir
    }
}

struct Harness {
    worker: PaintWorker,
    outputs: Receiver<Output>,
}

impl Harness {
    fn new(engine: &Arc<Engine>, renders: &Renders) -> Self {
        let (tx, outputs) = channel();
        let worker = PaintWorker::spawn(engine.clone(), renders.history.clone(), move |o| {
            let _ = tx.send(o);
        });
        Self { worker, outputs }
    }

    /// Outputs until one `pick` takes, failing on a worker error.
    fn until<T>(&self, what: &str, mut pick: impl FnMut(Output) -> Option<T>) -> T {
        loop {
            let output = self
                .outputs
                .recv_timeout(Duration::from_secs(30))
                .unwrap_or_else(|_| panic!("no {what}"));
            if let Output::Failed(e)
            | Output::OpenFailed { message: e, .. }
            | Output::SourceFailed { message: e, .. } = &output
            {
                panic!("worker failed: {e}");
            }
            if let Some(t) = pick(output) {
                return t;
            }
        }
    }

    fn canvas(&self) -> PresentedImage {
        self.until("canvas", |o| match o {
            Output::Canvas { display, .. } => Some(display),
            _ => None,
        })
    }

    /// Apply patches to `display` until the state says `done`.
    fn settle(&self, display: &mut PresentedImage, done: impl Fn(&PaintState) -> bool) {
        self.until("state", |o| match o {
            Output::Patches { patches, .. } => {
                for p in patches {
                    let level = &mut display.levels[p.level];
                    level.tiles[(p.ty * level.cols + p.tx) as usize] = p.tile;
                }
                None
            }
            Output::State(Some(state)) if done(&state) => Some(()),
            _ => None,
        })
    }

    fn stroke(&self, path: &[[f32; 2]], radius: f32) {
        let brush = Brush { radius, hardness: 0.6, flow: 0.7, ..Brush::default() };
        self.worker.send(Command::BeginStroke { brush });
        for p in path {
            self.worker.send(Command::Dabs(vec![Dab { x: p[0], y: p[1], pressure: 1.0 }]));
        }
        self.worker.send(Command::EndStroke);
    }
}

fn open(h: &Harness, n: u64, stack: &Arc<CompletedStack>) {
    h.worker.send(Command::Open { key: key(n), stack: stack.clone() });
}

fn same(got: &PresentedImage, want: &PresentedImage, what: &str) {
    assert_eq!(got.levels.len(), want.levels.len());
    for (k, (g, w)) in got.levels.iter().zip(&want.levels).enumerate() {
        assert!(g.assemble().pixels == w.assemble().pixels, "{what}: level {k} differs");
    }
}

/// Two renders painted in turn: patched displays match fresh ones, strokes
/// survive the switch, and undoing all restores the original.
#[test]
fn strokes_patch_the_display_and_survive_a_render_switch() {
    let Some(engine) = engine() else { return };
    let renders = Renders::new();
    let h = Harness::new(&engine, &renders);
    // Over 1020 px wide, so strokes cross display tile edges at two levels.
    let (a, b) = (stack(1300, 700, 1), stack(600, 520, 2));
    renders.dir(1);
    renders.dir(2);

    open(&h, 1, &a);
    let mut shown = h.canvas();
    same(&shown, &engine.present_pixels(&a, &a.pixels).unwrap(), "fresh canvas");
    let source = SourceKind::Frame { path: frame_file("a.png") };
    h.worker.send(Command::SetSource {
        kind: source.clone(),
        load: flat_source(&engine, &a, 60000),
        scrub: true,
        pick: 0,
    });
    // A scrubbed source gets its pixels when the stroke starts.
    h.stroke(&[[480.0, 300.0], [530.0, 320.0], [1100.0, 520.0], [1299.0, 699.0]], 40.0);
    h.stroke(&[[0.0, 0.0], [700.0, 10.0]], 25.0);
    // The state names the frame painted from.
    h.settle(&mut shown, |s| s.undo == 2 && s.source_ready() && s.painted_from == [source.clone()]);

    // Paint B, then back to A, whose fresh display must match the patched one.
    open(&h, 2, &b);
    let mut shown_b = h.canvas();
    h.worker.send(Command::SetSource {
        kind: source.clone(),
        load: flat_source(&engine, &b, 9000),
        scrub: false,
        pick: 0,
    });
    h.stroke(&[[10.0, 500.0], [590.0, 30.0]], 60.0);
    h.settle(&mut shown_b, |s| s.undo == 1);

    open(&h, 1, &a);
    let reopened = h.canvas();
    same(&shown, &reopened, "patched A vs its reopened display");
    let mut shown = reopened;
    h.worker.send(Command::Undo);
    h.worker.send(Command::Undo);
    h.settle(&mut shown, |s| s.undo == 0 && s.redo == 2 && !s.painted && s.painted_from.is_empty());
    same(&shown, &engine.present_pixels(&a, &a.pixels).unwrap(), "A undone");

    // Redo, then Reset: the original, with no history left.
    h.worker.send(Command::Redo);
    h.worker.send(Command::Reset);
    h.settle(&mut shown, |s| s.undo == 0 && s.redo == 0 && !s.painted);
    same(&shown, &engine.present_pixels(&a, &a.pixels).unwrap(), "A reset");

    // B kept its stroke throughout.
    open(&h, 2, &b);
    same(&shown_b, &h.canvas(), "patched B vs its reopened display");
}

/// Undoing a diagonal stroke updates only the tiles along it.
#[test]
fn undoing_a_diagonal_stroke_updates_only_the_tiles_along_it() {
    let Some(engine) = engine() else { return };
    let renders = Renders::new();
    let h = Harness::new(&engine, &renders);
    // Four by three full-size display tiles.
    let a = stack(2040, 1530, 6);
    renders.dir(1);
    open(&h, 1, &a);
    let mut shown = h.canvas();
    paint_source(&h, &engine, &a, 60000);
    h.stroke(&[[0.0, 0.0], [2039.0, 1529.0]], 20.0);
    h.settle(&mut shown, |s| s.undo == 1);

    h.worker.send(Command::Undo);
    let mut full = BTreeSet::new();
    h.until("the undo", |o| match o {
        Output::Patches { patches, .. } => {
            for p in patches {
                if p.level == 0 {
                    full.insert([p.tx, p.ty]);
                }
                let level = &mut shown.levels[p.level];
                level.tiles[(p.ty * level.cols + p.tx) as usize] = p.tile;
            }
            None
        }
        Output::State(Some(s)) if s.undo == 0 => Some(()),
        _ => None,
    });
    assert!(full.contains(&[0, 0]) && full.contains(&[3, 2]), "the stroke's ends: {full:?}");
    assert!(!full.contains(&[3, 0]) && !full.contains(&[0, 2]), "the other corners: {full:?}");
    assert!(full.len() <= 8, "the tiles along the stroke, of 12: {full:?}");
    same(&shown, &engine.present_pixels(&a, &a.pixels).unwrap(), "undone");
}

/// A source's display names its render, so a shell that moved on drops it.
#[test]
fn a_shown_source_names_its_render() {
    let Some(engine) = engine() else { return };
    let renders = Renders::new();
    let h = Harness::new(&engine, &renders);
    let (a, b) = (stack(200, 120, 1), stack(200, 120, 2));
    renders.dir(1);
    renders.dir(2);
    let shown_for = |h: &Harness| {
        h.until("source", |o| match o {
            Output::Source { key, .. } => Some(key),
            _ => None,
        })
    };
    for (n, stack) in [(1, &a), (2, &b)] {
        open(&h, n, stack);
        h.worker.send(Command::SetSource {
            kind: SourceKind::Frame { path: frame_file("a.png") },
            load: flat_source(&engine, stack, 30000),
            scrub: true,
            pick: 0,
        });
        assert_eq!(shown_for(&h), key(n));
    }
}

fn paint_source(h: &Harness, engine: &Arc<Engine>, stack: &Arc<CompletedStack>, value: u16) {
    h.worker.send(Command::SetSource {
        kind: SourceKind::Frame { path: frame_file("a.png") },
        load: flat_source(engine, stack, value),
        scrub: false,
        pick: 0,
    });
}

/// After quitting the worker, a new one restores the paint with a fresh
/// history, the result includes it, the spill is gone and the WAL emptied.
#[test]
fn paint_survives_a_restart_and_history_does_not() {
    let Some(engine) = engine() else { return };
    let (a, renders) = (stack(700, 400, 3), Renders::new());
    let render = renders.dir(1);
    let undo_file = render.join("paint-undo.sqlite");
    let painted = {
        let h = Harness::new(&engine, &renders);
        open(&h, 1, &a);
        let mut shown = h.canvas();
        paint_source(&h, &engine, &a, 60000);
        h.stroke(&[[10.0, 10.0], [690.0, 390.0]], 50.0);
        h.stroke(&[[350.0, 0.0], [350.0, 399.0]], 30.0);
        h.settle(&mut shown, |s| s.undo == 2);
        // A save answers once every earlier job, the spill too, is done.
        assert!(h.worker.save(Duration::from_secs(30)));
        assert!(undo_file.exists(), "the strokes' undo snapshots are spilled");
        h.worker.send(Command::Undo);
        h.settle(&mut shown, |s| s.undo == 1 && s.redo == 1);
        h.worker.send(Command::Redo);
        h.settle(&mut shown, |s| s.undo == 2 && s.redo == 0);
        // Leaving paint mode saves; so does the worker going away.
        h.worker.send(Command::Close);
        shown
    };
    assert!(render.join("paint.sqlite").exists());
    assert!(!undo_file.exists(), "the history went with the worker");
    let wal = std::fs::metadata(render.join("paint.sqlite-wal"));
    assert!(wal.map_or(true, |wal| wal.len() == 0), "no WAL left behind");

    let h = Harness::new(&engine, &renders);
    // Outside paint mode, from disk.
    let (reply, flat) = channel();
    h.worker.send(Command::Flatten { key: key(1), stack: a.clone(), reply });
    let flat = flat.recv().unwrap().unwrap().expect("painted");
    same(&engine.present_pixels(&a, &flat).unwrap(), &painted, "flattened from disk");

    open(&h, 1, &a);
    let mut shown = h.canvas();
    same(&shown, &painted, "reopened after a restart");
    h.settle(&mut shown, |s| s.undo == 0 && s.redo == 0 && s.painted);

    // Reset is saved at once: the original after the next restart.
    h.worker.send(Command::Reset);
    h.settle(&mut shown, |s| s.undo == 0 && s.redo == 0 && !s.painted);
    drop(h);
    let h = Harness::new(&engine, &renders);
    open(&h, 1, &a);
    let mut shown = h.canvas();
    same(&shown, &engine.present_pixels(&a, &a.pixels).unwrap(), "reset after a restart");
    h.settle(&mut shown, |s| s.undo == 0 && s.redo == 0 && !s.painted);
}

/// Unreadable paint isn't taken for none: export fails, and the view says so.
#[test]
fn paint_that_cannot_be_read_is_not_left_out_quietly() {
    let Some(engine) = engine() else { return };
    let (a, renders) = (stack(300, 200, 5), Renders::new());
    let file = renders.dir(1).join("paint.sqlite");
    {
        let h = Harness::new(&engine, &renders);
        open(&h, 1, &a);
        let mut shown = h.canvas();
        paint_source(&h, &engine, &a, 60000);
        h.stroke(&[[10.0, 10.0], [290.0, 190.0]], 30.0);
        h.settle(&mut shown, |s| s.painted);
        // Saved in memory nowhere else.
        assert!(h.worker.save(Duration::from_secs(30)));
        h.worker.send(Command::Close);
    }
    let saved = std::fs::read(&file).unwrap();

    std::fs::write(&file, vec![0x5a; saved.len()]).unwrap();
    let h = Harness::new(&engine, &renders);
    let (reply, flat) = channel();
    h.worker.send(Command::Flatten { key: key(1), stack: a.clone(), reply });
    let unread = deptheos_app::paint::store::Unread::Damaged;
    match flat.recv().unwrap() {
        Err(deptheos_app::Error::PaintUnread(u)) => assert_eq!(u, unread),
        other => panic!("flattened {:?}", other.map(|p| p.map(|p| p.len()))),
    }
    h.worker.send(Command::Present { key: key(1), stack: a.clone() });
    let failure = next_failure(&h);
    assert!(failure.contains("without its retouching"), "{failure}");
}

/// The worker's next failure, with the render the open was of.
fn next_failure(h: &Harness) -> String {
    loop {
        match h.outputs.recv_timeout(Duration::from_secs(30)).expect("a failure") {
            Output::OpenFailed { key: k, message } => {
                assert_eq!(k, key(1));
                return message.text;
            }
            Output::Failed(message) => return message.text,
            Output::State(_) => {}
            _ => panic!("expected a failure"),
        }
    }
}

/// A source whose pixels fail to load refuses the stroke and says so.
#[test]
fn a_source_that_fails_to_load_refuses_the_stroke() {
    let Some(engine) = engine() else { return };
    let renders = Renders::new();
    let h = Harness::new(&engine, &renders);
    let a = stack(300, 200, 4);
    renders.dir(1);
    open(&h, 1, &a);
    let mut shown = h.canvas();
    // Shown while scrubbing, but its pixels can't be had.
    let (engine2, a2) = (engine.clone(), a.clone());
    let load: SourceLoader = Arc::new(move |with_pixels| {
        if with_pixels {
            return Err("the frame is gone".into());
        }
        let display = engine2.present_pixels(&a2, &a2.pixels).map_err(|e| e.to_string())?;
        Ok((None, display))
    });
    let kind = SourceKind::Frame { path: frame_file("gone.png") };
    h.worker.send(Command::SetSource { kind: kind.clone(), load, scrub: true, pick: 0 });
    h.stroke(&[[10.0, 10.0], [290.0, 190.0]], 40.0);
    // A source of the wrong size is refused too.
    let small = stack(100, 100, 5);
    paint_source(&h, &engine, &small, 9000);
    h.stroke(&[[10.0, 10.0], [290.0, 190.0]], 40.0);

    let mut failures = Vec::new();
    loop {
        match h.outputs.recv_timeout(Duration::from_secs(30)).expect("the failures") {
            Output::SourceFailed { kind: k, .. } => failures.push(Some(k)),
            Output::Failed(_) => failures.push(None),
            Output::Patches { patches, .. } => {
                for p in patches {
                    let level = &mut shown.levels[p.level];
                    level.tiles[(p.ty * level.cols + p.tx) as usize] = p.tile;
                }
            }
            Output::State(s) => assert!(s.is_none_or(|s| s.undo == 0), "nothing was painted"),
            _ => {}
        }
        if failures.len() == 2 {
            break;
        }
    }
    assert_eq!(failures, [Some(kind), None]);
    same(&shown, &engine.present_pixels(&a, &a.pixels).unwrap(), "unpainted");
}

/// Undo mid-stroke is ignored; the state names the undo and redo steps.
#[test]
fn undo_during_a_stroke_is_ignored_and_the_state_names_its_step() {
    let Some(engine) = engine() else { return };
    let renders = Renders::new();
    let h = Harness::new(&engine, &renders);
    let a = stack(400, 300, 3);
    renders.dir(1);
    open(&h, 1, &a);
    let mut shown = h.canvas();
    paint_source(&h, &engine, &a, 60000);
    h.stroke(&[[10.0, 10.0], [390.0, 290.0]], 30.0);
    h.settle(&mut shown, |s| s.undo == 1 && s.undo_label.as_deref() == Some("Paint"));

    let brush = Brush { radius: 20.0, ..Brush::default() };
    let dab = |x, y| Command::Dabs(vec![Dab { x, y, pressure: 1.0 }]);
    h.worker.send(Command::BeginStroke { brush });
    h.worker.send(dab(10.0, 290.0));
    h.worker.send(Command::Undo);
    h.worker.send(dab(390.0, 10.0));
    h.worker.send(Command::EndStroke);
    h.settle(&mut shown, |s| s.undo == 2 && s.redo == 0);

    h.worker.send(Command::Undo);
    h.settle(&mut shown, |s| {
        s.undo == 1 && s.redo == 1 && s.redo_label.as_deref() == Some("Paint")
    });
}

/// Deleting a render being painted drops its doc unsaved first; nothing
/// brings the folder back.
#[test]
fn deleting_a_render_being_painted_closes_its_paint_first() {
    let Some(engine) = engine() else { return };
    let (a, renders) = (stack(300, 200, 6), Renders::new());
    let render = renders.dir(1);
    let h = Harness::new(&engine, &renders);
    open(&h, 1, &a);
    let mut shown = h.canvas();
    paint_source(&h, &engine, &a, 60000);
    h.stroke(&[[10.0, 10.0], [290.0, 190.0]], 30.0);
    h.stroke(&[[10.0, 190.0], [290.0, 10.0]], 30.0);
    h.settle(&mut shown, |s| s.undo == 2);
    assert!(h.worker.save(Duration::from_secs(30)));
    assert!(render.join("paint-undo.sqlite").exists());
    h.stroke(&[[150.0, 0.0], [150.0, 199.0]], 30.0);

    renders.history.delete_renders(&[key(1)], renders.history.close_paint(&[key(1)])).unwrap();
    assert!(!render.exists(), "deleted");
    // The doc went with it: nothing left to save.
    assert!(h.worker.save(Duration::from_secs(30)));
    drop(h);
    assert!(!render.exists(), "not made again");
}

/// Closing paint for a delete reaches only that Library's worker; with no
/// worker the close returns at once.
#[test]
fn closing_paint_reaches_only_its_own_library() {
    let Some(engine) = engine() else { return };
    let a = stack(300, 200, 6);
    let (mine, theirs) = (Renders::new(), Renders::new());
    mine.dir(1);
    theirs.dir(1);
    let other = Harness::new(&engine, &theirs);
    open(&other, 1, &a);
    let mut shown = other.canvas();
    paint_source(&other, &engine, &a, 60000);
    other.stroke(&[[10.0, 10.0], [290.0, 190.0]], 30.0);
    other.settle(&mut shown, |s| s.undo == 1);

    let h = Harness::new(&engine, &mine);
    open(&h, 1, &a);
    h.canvas();
    mine.history.delete_renders(&[key(1)], mine.history.close_paint(&[key(1)])).unwrap();
    other.stroke(&[[10.0, 190.0], [290.0, 10.0]], 30.0);
    other.settle(&mut shown, |s| s.undo == 2);
    assert!(other.worker.save(Duration::from_secs(30)), "its paint saved");
    assert!(theirs.history.render_path(&key(1)).join("paint.sqlite").exists());

    drop(h);
    let _ = mine.history.close_paint(&[key(2)]);
    let alone = Renders::new();
    let _ = alone.history.close_paint(&[key(1)]);
}

/// A store-thread delete (as on iOS) of a render open in Retouch closes its
/// paint first, so the store thread and worker never wait on each other.
#[test]
fn a_delete_on_the_store_thread_ends_with_a_render_being_painted() {
    let Some(engine) = engine() else { return };
    let (a, renders) = (stack(300, 200, 6), Renders::new());
    let render = renders.dir(1);
    let h = Harness::new(&engine, &renders);
    open(&h, 1, &a);
    let mut shown = h.canvas();
    paint_source(&h, &engine, &a, 60000);
    h.stroke(&[[10.0, 10.0], [290.0, 190.0]], 30.0);
    h.settle(&mut shown, |s| s.undo == 1);
    assert!(h.worker.save(Duration::from_secs(30)));
    h.stroke(&[[150.0, 0.0], [150.0, 199.0]], 30.0);

    let history = renders.history.clone();
    let closed = history.close_paint(&[key(1)]);
    let (tx, deleted) = std::sync::mpsc::channel();
    let _sent = deptheos_app::engine::store::store().write(move || {
        let _ = tx.send(history.delete_renders(&[key(1)], closed));
    });
    let deleted = deleted.recv_timeout(Duration::from_secs(30));
    deleted.expect("the delete ended").unwrap();
    assert!(!render.exists(), "deleted");
    assert!(h.worker.save(Duration::from_secs(30)));
}
