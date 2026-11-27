//! The session on a real Library and the GPU: an open, a drag and its
//! commit written, undo, the eyedropper, leaving with its thumbnail drawn
//! again, a damaged file, and the delete's proof.

use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use image::RgbImage;

use deptheos_render::WhiteBalance;

use super::*;
use crate::library::history::Made;
use crate::library::ids::StackId;

const W: u32 = 96;
const H: u32 = 64;
const WAIT: Duration = Duration::from_secs(30);

/// What the app heard, as text, and the settled pictures' sizes.
#[derive(Default)]
struct Heard {
    log: Mutex<Vec<String>>,
    changed: Condvar,
}

impl Observer for Heard {
    fn update(&self, update: Update) {
        let line = match update {
            Update::Status(status) => format!("status {:?} undo {}", status.phase, status.undo),
            Update::Preview { picture, .. } => {
                format!("preview {}x{}", picture.width, picture.height)
            }
            Update::Settled { picture, histogram, .. } => {
                format!("settled {}x{} {}", picture.width(), picture.height(), histogram.pixels)
            }
            Update::Closed { redraw, .. } => format!("closed {redraw}"),
            Update::Rewritten(_) => "rewritten".into(),
            Update::Notice(text) => format!("notice {text}"),
        };
        crate::lock(&self.log).push(line);
        self.changed.notify_all();
    }
}

impl Heard {
    /// Wait until a line heard after the first `from` starts with `start`;
    /// the count heard then.
    fn wait_for(&self, from: usize, start: &str) -> usize {
        let mut log = crate::lock(&self.log);
        loop {
            if let Some(i) = log.iter().skip(from).position(|l| l.starts_with(start)) {
                return from + i + 1;
            }
            let (next, timeout) = self.changed.wait_timeout(log, WAIT).unwrap();
            log = next;
            assert!(!timeout.timed_out(), "no {start:?} after {from}: {:?}", *log);
        }
    }
}

struct Library {
    _dir: tempfile::TempDir,
    history: Arc<History>,
    key: RenderId,
    stack: Arc<CompletedStack>,
}

/// A Library with one render of a gray `spot` on a mid gray.
fn made(spot: [u16; 3]) -> Library {
    let dir = tempfile::tempdir().unwrap();
    let history = Arc::new(History::open(dir.path()).unwrap());
    let frames: Vec<PathBuf> = ["a.png", "b.png"].map(|n| dir.path().join(n)).to_vec();
    for frame in &frames {
        std::fs::write(frame, b"frame").unwrap();
    }
    let mut pixels = Vec::new();
    for y in 0..H {
        for x in 0..W {
            let near = x.abs_diff(70) <= 4 && y.abs_diff(20) <= 4;
            pixels.extend(if near { spot } else { [24000; 3] });
        }
    }
    let stack = CompletedStack::srgb(W, H, pixels, frames.clone());
    let render = history
        .save_render(
            &frames,
            &stack,
            &RgbImage::new(W, H),
            &StackId::new(),
            &Made::Stacked { base: None },
        )
        .unwrap();
    let stack = Arc::new(history.load_render(&render.id).unwrap());
    Library { _dir: dir, history, key: render.id, stack }
}

fn start(engine: Arc<Engine>, library: &Library) -> (AdjustSession, Arc<Heard>) {
    let heard = Arc::new(Heard::default());
    (AdjustSession::new(engine, library.history.clone(), heard.clone()), heard)
}

fn saved(library: &Library) -> RenderAdjustments {
    let render = library.history.render(&library.key).unwrap();
    library.history.adjustments(&render).unwrap()
}

#[test]
fn a_drag_previews_then_its_commit_settles_writes_and_undoes() {
    let Some(engine) = crate::test_support::engine() else { return };
    let library = made([30000, 24000, 18000]);
    let (session, heard) = start(engine, &library);
    session.open(library.key.clone(), library.stack.clone());
    let at = heard.wait_for(0, "status Ready");

    for v in [0.3, 0.6, 1.0] {
        session.set_slider(Slider::Exposure, v);
    }
    let at = heard.wait_for(at, "preview");
    session.commit();
    let at = heard.wait_for(at, "settled 96x64");
    // Written once, after the change, not per tick.
    wait_until(|| saved(&library).adjustments.exposure == 1.0);

    session.undo();
    let at = heard.wait_for(at, "settled");
    wait_until(|| saved(&library).adjustments.exposure == 0.0);
    assert_eq!(session.status().map(|s| (s.undo, s.redo)), Some((0, 1)));

    session.leave();
    let at = heard.wait_for(at, "closed");
    heard.wait_for(at, "rewritten");
    assert!(session.closed(std::slice::from_ref(&library.key)).is_some());
}

#[test]
fn the_eyedropper_sets_the_spot_gray_and_refuses_a_clipped_one() {
    let Some(engine) = crate::test_support::engine() else { return };
    let library = made([30000, 24000, 18000]);
    let (session, heard) = start(engine.clone(), &library);
    session.open(library.key.clone(), library.stack.clone());
    let at = heard.wait_for(0, "status Ready");
    session.eyedrop((70.5, 20.5));
    heard.wait_for(at, "settled");
    let set = session.status().unwrap().value.adjustments.white_balance;
    let WhiteBalance::Relative { temperature, .. } = set else { panic!("{set:?}") };
    // A warm spot is corrected cooler.
    assert!(temperature < 0.0, "{set:?}");
    wait_until(|| saved(&library).adjustments.white_balance == set);

    let clipped = made([65535, 30000, 30000]);
    let (session, heard) = start(engine, &clipped);
    session.open(clipped.key.clone(), clipped.stack.clone());
    let at = heard.wait_for(0, "status Ready");
    session.eyedrop((70.5, 20.5));
    heard.wait_for(at, "notice That spot is too bright");
    assert!(session.status().unwrap().value.adjustments.white_balance.is_as_shot());
}

#[test]
fn a_damaged_file_opens_neutral_and_says_so() {
    let Some(engine) = crate::test_support::engine() else { return };
    let library = made([24000; 3]);
    let path = library
        .history
        .render_path(&library.key)
        .join(crate::library::adjustments::ADJUSTMENTS_FILE);
    std::fs::write(path, b"{ not json").unwrap();
    let (session, heard) = start(engine, &library);
    session.open(library.key.clone(), library.stack.clone());
    let at = heard.wait_for(0, "notice This render's adjustments were damaged");
    heard.wait_for(at, "status Ready");
    assert_eq!(session.status().unwrap().value, RenderAdjustments::NONE);
}

/// A delete's close leaves the render and waits for its last write.
#[test]
fn closing_for_a_delete_leaves_and_waits_for_the_write() {
    let Some(engine) = crate::test_support::engine() else { return };
    let library = made([24000; 3]);
    let (session, heard) = start(engine, &library);
    session.open(library.key.clone(), library.stack.clone());
    heard.wait_for(0, "status Ready");
    session.set_slider(Slider::Contrast, 40.0);
    let ids = std::slice::from_ref(&library.key);
    assert!(session.closed(ids).is_none());
    let _closed = library.history.close_paint(ids);
    assert_eq!(session.adjusting(), None);
    assert!(session.closed(ids).is_some());
    assert_eq!(saved(&library).adjustments.contrast, 40.0);
    library.history.delete_renders(ids, _closed).unwrap();
}

fn wait_until(done: impl Fn() -> bool) {
    let until = std::time::Instant::now() + WAIT;
    while !done() {
        assert!(std::time::Instant::now() < until, "not in time");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// An export reads the render's adjustments after its pending writes, and
/// a damaged file fails it.
#[test]
fn an_export_reads_the_written_value_and_fails_on_a_damaged_file() {
    let Some(engine) = crate::test_support::engine() else { return };
    let library = made([24000; 3]);
    let (session, heard) = start(engine, &library);
    session.open(library.key.clone(), library.stack.clone());
    heard.wait_for(0, "status Ready");
    session.set_slider(Slider::Exposure, 0.7);
    session.commit();
    let read = library.history.adjustments_to_export(&library.key).unwrap();
    assert_eq!(read.adjustments.exposure, 0.7);

    session.leave();
    let _closed = library.history.close_paint(std::slice::from_ref(&library.key));
    let path = library
        .history
        .render_path(&library.key)
        .join(crate::library::adjustments::ADJUSTMENTS_FILE);
    std::fs::write(path, b"{ not json").unwrap();
    let failed = library.history.adjustments_to_export(&library.key).unwrap_err();
    assert!(matches!(failed, crate::Error::AdjustmentsUnread(Unread::Damaged)), "{failed:?}");
}
