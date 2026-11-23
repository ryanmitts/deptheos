//! The session over a real worker, with timers it fires by hand: an open,
//! a scrub settling and fetching ahead, a stroke, leaving; an observer that
//! calls back in or panics.

use std::sync::OnceLock;
use std::sync::mpsc::Receiver;

use super::*;
use crate::paint::machine::{CanvasState, PREFETCH_AFTER, SCRUB_SETTLE};
use crate::test_support::engine;
use crate::{Gray16Image, Platform, Words};

/// Timers fired by the test.
#[derive(Default)]
struct Manual(Mutex<Vec<(Duration, Fire)>>);

impl Clock for Manual {
    fn after(&self, delay: Duration, fire: Fire) {
        self.0.lock().unwrap().push((delay, fire));
    }
}

impl Manual {
    /// Fire every timer started for `delay`; whether there was one.
    fn fire(&self, delay: Duration) -> bool {
        let due: Vec<Fire> = {
            let mut timers = self.0.lock().unwrap();
            let (due, wait) =
                std::mem::take(&mut *timers).into_iter().partition(|(d, _)| *d == delay);
            *timers = wait;
            due.into_iter().map(|(_, fire)| fire).collect()
        };
        let any = !due.is_empty();
        for fire in due {
            fire();
        }
        any
    }
}

/// Hears every report, and as the canvas lands calls back into the
/// session from inside the report.
struct Recorder {
    heard: Mutex<Sender<String>>,
    session: OnceLock<Weak<PaintSession>>,
}

impl Observer for Recorder {
    fn report(&self, report: Report) {
        if let Report::Canvas { .. } = &report
            && let Some(session) = self.session.get().and_then(Weak::upgrade)
        {
            // A call back in: no deadlock, and the machine's state is there.
            assert_eq!(session.status().unwrap().canvas, CanvasState::Ready);
            session.undo();
        }
        let _ = self.heard.lock().unwrap().send(format!("{report:?}"));
    }
}

fn until(heard: &Receiver<String>, what: &str, pick: impl Fn(&str) -> bool) {
    loop {
        let next = heard
            .recv_timeout(Duration::from_secs(30))
            .unwrap_or_else(|_| panic!("never heard {what}"));
        assert!(!next.starts_with("Failed"), "{next}");
        if pick(&next) {
            return;
        }
    }
}

fn wait_for(session: &PaintSession, what: &str, done: impl Fn(&Status) -> bool) {
    let start = Instant::now();
    while !session.status().as_ref().is_some_and(&done) {
        assert!(start.elapsed() < Duration::from_secs(30), "never {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn told() -> Told {
    Arc::new(|e: &dyn crate::Explain| {
        crate::paint::Failure::user(e.explain(&Words::english(Platform::Desktop)).reason)
    })
}

#[test]
fn a_render_opened_scrubbed_painted_and_left() {
    let Some(engine) = engine() else { return };
    let library = tempfile::tempdir().unwrap();
    let history = Arc::new(History::open(library.path().join("Library")).unwrap());
    let key = RenderId::parse(&format!("{:032x}", 1)).unwrap();
    let render = history.render_path(&key);
    std::fs::create_dir_all(&render).unwrap();
    std::fs::write(render.join("render.json"), "{}").unwrap();
    let (width, height) = (64, 32);
    // Three flat frames, the left of the result from the first.
    let frames: Vec<PathBuf> = [255u8, 128, 0]
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            let path = library.path().join(format!("f{i}.png"));
            image::RgbImage::from_pixel(width, height, image::Rgb([v, v, v])).save(&path).unwrap();
            path
        })
        .collect();
    let mut stack =
        CompletedStack::srgb(width, height, vec![128 * 257; 3 * 64 * 32], frames.clone());
    stack.frame_map = Some(Gray16Image::from_fn(width, height, |x, _| {
        image::Luma([if x < width / 2 { 0 } else { 65535 }])
    }));
    let stack = Arc::new(stack);

    let (tx, heard) = channel();
    let recorder = Arc::new(Recorder { heard: Mutex::new(tx), session: OnceLock::new() });
    let clock = Arc::new(Manual::default());
    let session = Arc::new(PaintSession::with_clock(
        engine,
        history,
        recorder.clone(),
        told(),
        clock.clone(),
    ));
    let _ = recorder.session.set(Arc::downgrade(&session));

    session.open(OpenRender {
        key: key.clone(),
        stack: stack.clone(),
        frames: frames.clone(),
        first: Some(frames[0].clone()),
    });
    let status = session.status().unwrap();
    assert_eq!((status.canvas, status.lit), (CanvasState::Opening, Some(0)));
    until(&heard, "the canvas", |h| h.starts_with("Canvas"));
    wait_for(&session, "the first frame", |s| s.ready);

    // The rail: the frame is shown, its pixels fetched once it settles,
    // then the next one along fetched ahead.
    session.scrub_to(frames[1].clone());
    assert_eq!(session.status().unwrap().selection, SourceKind::Frame { path: frames[1].clone() });
    let start = Instant::now();
    while !clock.fire(SCRUB_SETTLE) {
        assert!(start.elapsed() < Duration::from_secs(30), "never settled");
        std::thread::sleep(Duration::from_millis(5));
    }
    wait_for(&session, "the frame scrubbed to, in full", |s| s.ready);
    assert!(clock.fire(PREFETCH_AFTER), "the next frame fetched ahead");

    // The sharpest frame at a point, then a stroke from it.
    session.pick_at(1.5, 1.5);
    wait_for(&session, "the frame at the point", |s| {
        s.ready && s.selection == SourceKind::Frame { path: frames[0].clone() }
    });
    let dab = |x| Dab { x, y: 16.0, pressure: 1.0 };
    session.begin_stroke(Brush { radius: 8.0, ..Brush::default() }, Some(dab(10.0)));
    session.dabs(vec![dab(50.0)]);
    session.end_stroke();
    wait_for(&session, "the stroke", |s| s.undo == 1 && s.painted);

    // Left: its last tiles may still come, then nothing open.
    session.close();
    assert!(session.status().is_none());
    until(&heard, "the close", |h| h.starts_with("Closed"));
    assert!(session.save(Duration::from_secs(10)));
    assert!(super::super::store::has_paint(&render) == Ok(true));

    // Paint of another schema is set aside; the render opens unpainted.
    let file = super::super::store::paint_file(&render);
    rusqlite::Connection::open(&file).unwrap().execute_batch("PRAGMA user_version = 99").unwrap();
    session.open(OpenRender {
        key: key.clone(),
        stack: stack.clone(),
        frames: frames.clone(),
        first: None,
    });
    wait_for(&session, "the open over damaged paint", |s| s.ready && !s.painted);
    session.close();
    assert!(session.save(Duration::from_secs(10)));
}

/// Panics at its first report, then hears the rest.
struct Panicky {
    panicked: std::sync::atomic::AtomicBool,
    heard: Mutex<Sender<String>>,
}

impl Observer for Panicky {
    fn report(&self, report: Report) {
        if !self.panicked.swap(true, std::sync::atomic::Ordering::Relaxed) {
            panic!("the observer's first report");
        }
        let _ = self.heard.lock().unwrap().send(format!("{report:?}"));
    }
}

/// After an observer panics, the next call's reports are heard.
#[test]
fn an_observer_that_panics_hears_the_next_reports() {
    let Some(engine) = engine() else { return };
    let (tx, heard) = channel();
    let observer = Arc::new(Panicky { panicked: Default::default(), heard: Mutex::new(tx) });
    let library = tempfile::tempdir().unwrap();
    let history = Arc::new(History::open(library.path().join("Library")).unwrap());
    let session = PaintSession::new(engine, history, observer, told());
    let key = RenderId::parse(&format!("{:032x}", 3)).unwrap();
    let stack = Arc::new(CompletedStack::srgb(2, 2, vec![0; 12], Vec::new()));
    let open =
        || OpenRender { key: key.clone(), stack: stack.clone(), frames: Vec::new(), first: None };
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| session.open(open())));
    assert!(caught.is_err(), "the observer panicked");
    session.open(open());
    // The first open's canvas, or the second's status: either is heard.
    heard.recv_timeout(Duration::from_secs(5)).expect("heard after the panic");
}

/// A stroke from a white frame over a gray render, undone, redone, and
/// saved as last left.
#[test]
fn undo_and_redo_take_a_stroke_back_and_forth() {
    let Some(engine) = engine() else { return };
    let library = tempfile::tempdir().unwrap();
    let history = Arc::new(History::open(library.path().join("Library")).unwrap());
    let key = RenderId::parse(&format!("{:032x}", 2)).unwrap();
    let render = history.render_path(&key);
    std::fs::create_dir_all(&render).unwrap();
    std::fs::write(render.join("render.json"), "{}").unwrap();
    let (width, height) = (64u32, 32u32);
    let frames: Vec<PathBuf> = [255u8, 0]
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            let path = library.path().join(format!("f{i}.png"));
            image::RgbImage::from_pixel(width, height, image::Rgb([v, v, v])).save(&path).unwrap();
            path
        })
        .collect();
    let gray = 128 * 257;
    let stack = Arc::new(CompletedStack::srgb(
        width,
        height,
        vec![gray; (3 * width * height) as usize],
        frames.clone(),
    ));
    let (tx, _heard) = channel();
    let recorder = Arc::new(Recorder { heard: Mutex::new(tx), session: OnceLock::new() });
    let session = Arc::new(PaintSession::with_clock(
        engine,
        history,
        recorder,
        told(),
        Arc::new(Manual::default()),
    ));
    session.open(OpenRender {
        key: key.clone(),
        stack: stack.clone(),
        frames: frames.clone(),
        first: Some(frames[0].clone()),
    });
    wait_for(&session, "the white frame", |s| s.ready && s.canvas == CanvasState::Ready);
    // The stroke's middle, and a pixel far from it.
    let (inside, outside) = ((30u32, 16u32), (60u32, 2u32));
    let flattened = |session: &PaintSession| -> Vec<u16> {
        session
            .flatten(key.clone(), stack.clone())
            .recv_timeout(Duration::from_secs(30))
            .expect("an answer")
            .expect("its paint read")
            .unwrap_or_else(|| stack.pixels.clone())
    };
    let at = |pixels: &[u16], (x, y): (u32, u32)| pixels[((y * width + x) * 3) as usize];

    let dab = |x| Dab { x, y: 16.0, pressure: 1.0 };
    session.begin_stroke(Brush { radius: 6.0, ..Brush::default() }, Some(dab(20.0)));
    session.dabs(vec![dab(40.0)]);
    session.end_stroke();
    wait_for(&session, "the stroke", |s| (s.undo, s.redo) == (1, 0) && s.painted);
    let painted = flattened(&session);
    // The brush is soft: its middle is nearly the frame.
    assert!(at(&painted, inside) > 65000, "painted white: {}", at(&painted, inside));
    assert_eq!(at(&painted, outside), gray, "untouched away from the stroke");

    session.undo();
    wait_for(&session, "the undo", |s| (s.undo, s.redo) == (0, 1));
    assert!(!session.status().unwrap().painted);
    let undone = flattened(&session);
    assert_eq!((at(&undone, inside), at(&undone, outside)), (gray, gray), "the render again");

    session.redo();
    wait_for(&session, "the redo", |s| (s.undo, s.redo) == (1, 0) && s.painted);
    assert_eq!(flattened(&session), painted, "the stroke again, as it was");

    // Undone again and left: what is saved is the render as stacked.
    session.undo();
    wait_for(&session, "the second undo", |s| (s.undo, s.redo) == (0, 1));
    session.close();
    assert!(session.save(Duration::from_secs(10)));
    assert_eq!(at(&flattened(&session), inside), gray, "saved as last left");
}
