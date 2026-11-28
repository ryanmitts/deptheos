//! Adjust through the bindings, as the iOS app makes the calls: a render
//! opened, dragged, committed and undone, pasted onto, its eyedropper,
//! its export and thumbnail under its adjustments, its delete, and the
//! panes' pointer mapping.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use deptheos_app::{CompletedStack, SessionOptions, StackColor};

use super::*;
use crate::tests::now;
use crate::{Engine, RenderInfo, StackResult, ViewControl, ViewEvent};

const W: u32 = 96;
const H: u32 = 64;
const WAIT: Duration = Duration::from_secs(30);

/// What the app heard, as text.
#[derive(Default)]
struct Heard {
    log: Mutex<Vec<String>>,
    changed: Condvar,
}

impl Heard {
    fn push(&self, line: String) {
        crate::lock(&self.log).push(line);
        self.changed.notify_all();
    }

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

impl AdjustObserver for Heard {
    fn state_changed(&self) {
        self.push("state".into());
    }
    fn previewed(&self, preview: Arc<AdjustPreview>, histogram: AdjustHistogram) {
        assert_eq!(preview.byte_count(), u64::from(preview.width() * preview.height() * 4));
        assert_ne!(preview.bgra_address(), 0);
        assert_eq!(preview.space(), ScreenSpace::Srgb);
        self.push(format!("preview {}x{} {}", preview.width(), preview.height(), histogram.pixels));
    }
    fn on_screen(&self, preview: Arc<AdjustPreview>, _: AdjustHistogram) {
        self.push(format!("on screen {}x{}", preview.width(), preview.height()));
    }
    fn settled(&self, _: String, image: DisplayImage, _: Arc<Picture>, h: AdjustHistogram) {
        self.push(format!("settled {}x{} full {}", image.width, image.height, h.counted_full));
    }
    fn counted(&self, _: String, h: AdjustHistogram) {
        self.push(format!("counted {}", h.pixels));
    }
    fn closed(&self, _: String, redraw: bool) {
        self.push(format!("closed {redraw}"));
    }
    fn rewritten(&self, _: String) {
        self.push("rewritten".into());
    }
    fn notice(&self, message: String) {
        self.push(format!("notice {message}"));
    }
}

fn engine(dir: &Path) -> Option<Arc<Engine>> {
    let history = dir.join("history");
    let engine = now(Engine::new(
        Arc::new(crate::paint::tests::ImageCrate),
        PathBuf::from(&history),
        None,
        "en".into(),
    ));
    if let Err(e) = &engine {
        deptheos_app::GpuContext::skip_test(&format!("no GPU engine ({e})"));
    }
    engine.ok()
}

/// A render of a gray with a warm spot, saved as iOS saves one.
fn saved(engine: &Arc<Engine>, dir: &Path, stack_id: &str) -> (Arc<StackResult>, RenderInfo) {
    let frames: Vec<PathBuf> = ["a.png", "b.png"].map(|n| dir.join(n)).to_vec();
    for frame in &frames {
        std::fs::write(frame, b"frame").unwrap();
    }
    let mut pixels = Vec::new();
    for y in 0..H {
        for x in 0..W {
            let near = x.abs_diff(70) <= 4 && y.abs_diff(20) <= 4;
            pixels.extend(if near { [30000, 24000, 18000] } else { [24000; 3] });
        }
    }
    let stack = CompletedStack {
        color: StackColor::Srgb { sixteen_bit: true, profile: None },
        options: SessionOptions::default(),
        ..CompletedStack::srgb(W, H, pixels, frames.clone())
    };
    let presented =
        engine.engine.present_stack(stack, &deptheos_app::Adjustments::NEUTRAL).unwrap();
    let result = StackResult::new(engine.engine.clone(), presented);
    let made = crate::RenderMade::Stacked { base: None };
    let render =
        now(engine.clone().save_render(frames, result.clone(), stack_id.to_string(), made))
            .unwrap();
    (result, render)
}

fn mean_of(path: &Path) -> f64 {
    let image = image::open(path).unwrap().into_rgb8();
    image.pixels().map(|p| f64::from(p[1])).sum::<f64>() / f64::from(W * H)
}

#[test]
fn a_render_adjusted_through_the_bindings() {
    let dir = tempfile::tempdir().unwrap();
    let Some(engine) = engine(dir.path()) else { return };
    let (result, render) = saved(&engine, dir.path(), &crate::new_stack_id());
    let heard = Arc::new(Heard::default());
    let session = engine.adjust_session(heard.clone(), 64);
    session.open(result.clone(), render.id.clone()).unwrap();
    let at = heard.wait_for(0, "state");
    heard.wait_for(at, "state");
    while session.status().is_none_or(|s| s.phase != AdjustPhase::Ready) {
        std::thread::sleep(Duration::from_millis(5));
    }

    // The panel is the engine's, worded: a standard stack's relative white.
    let panel = session.panel().unwrap();
    assert!(!panel.camera_raw);
    let tone = panel.groups.iter().find(|g| g.group == Group::Tone).unwrap();
    let exposure = tone.sliders.iter().find(|s| s.slider == Slider::Exposure).unwrap();
    assert_eq!((exposure.label.as_str(), exposure.min, exposure.max), ("Exposure", -5.0, 5.0));

    // A drag previews, its commit settles with the clipping counted whole.
    for v in [0.3, 0.6, 1.0] {
        session.set_slider(Slider::Exposure, v);
    }
    let at = heard.wait_for(at, "preview");
    session.commit();
    let at = heard.wait_for(at, "settled 64x");
    let value = session.status().unwrap().value;
    assert_eq!(
        (value.get(Slider::Exposure), value.text(Slider::Exposure)),
        (1.0, "+1.00 EV".into())
    );
    let typed = value.parse(Slider::Exposure, "0.333".into()).unwrap();
    assert!((typed - 0.33).abs() < 1e-9, "{typed}");
    assert!(!value.is_default(Slider::Exposure) && value.is_default(Slider::Contrast));

    // Undo, then redo, each a whole value.
    session.undo();
    let at = heard.wait_for(at, "settled");
    assert_eq!(session.status().unwrap().value.get(Slider::Exposure), 0.0);
    session.redo();
    let at = heard.wait_for(at, "settled");

    // The eyedropper sets the warm spot neutral: cooler.
    session.eyedrop(70.5, 20.5);
    let at = heard.wait_for(at, "settled");
    assert!(session.status().unwrap().value.get(Slider::Temperature) < 0.0);

    // A crop, committed.
    let crop = Rect { x: 8, y: 4, width: 64, height: 40 };
    session.set_reframe(Reframe { crop: Some(crop), flip: Flip::None, turn: Turn::Quarter });
    session.commit();
    let at = heard.wait_for(at, "settled");

    session.leave();
    let at = heard.wait_for(at, "closed");
    heard.wait_for(at, "rewritten");

    // Copied, it reads back what was written; its export is drawn under it.
    let copied = now(engine.clone().render_adjustments(render.id.clone())).unwrap();
    assert_eq!(copied.get(Slider::Exposure), 1.0);
    assert_eq!(copied.reframe().crop, Some(crop));
    let loaded = now(engine.clone().load_render(crate::tests::req(), render.id.clone())).unwrap();
    let export = |result: Arc<StackResult>, name: &str| {
        let preferences = crate::default_preferences().export;
        let out = dir.path().to_path_buf();
        let tiff = deptheos_app::library::export::ExportFormat::Tiff;
        now(result.export(crate::tests::req(), out, name.into(), tiff, preferences)).unwrap()
    };
    let adjusted = export(loaded, "adjusted");
    // Cropped and turned a quarter.
    let size = image::image_dimensions(&adjusted).unwrap();
    assert_eq!(size, (40, 64));

    // Pasted onto another render: the sliders, not its crop.
    let (other_result, other) = saved(&engine, dir.path(), &crate::new_stack_id());
    let plain = export(other_result.clone(), "plain");
    now(engine.clone().paste_adjustments(other.id.clone(), copied)).unwrap();
    let pasted = now(engine.clone().render_adjustments(other.id.clone())).unwrap();
    assert_eq!((pasted.get(Slider::Exposure), pasted.reframe().crop), (1.0, None));
    let brighter = export(other_result, "pasted");
    let (b, p) = (mean_of(&brighter), mean_of(&plain));
    assert!(b > p + 10.0, "{b} {p}");
    assert!(now(engine.clone().rewrite_thumbnail(other.id.clone())).unwrap());

    // A delete while Adjust is open leaves it, its write in first.
    session.open(result, render.id.clone()).unwrap();
    while session.adjusting().is_none() {
        std::thread::sleep(Duration::from_millis(5));
    }
    session.set_slider(Slider::Contrast, 30.0);
    now(engine.clone().delete_renders(vec![render.id.clone()], None)).unwrap();
    assert_eq!(session.adjusting(), None);
}

/// The view's Adjust states and each pane's pointer mapping.
#[test]
fn adjust_views_and_panes_through_the_bindings() {
    let view = ViewControl::new();
    view.on(ViewEvent::Shown {
        result: Some(deptheos_app::view::Facts { map: false, lights: false }),
        keep: false,
        supplied: false,
    });
    let answer = view.on(ViewEvent::Adjust { on: true });
    let adjust = answer.adjust.unwrap();
    assert_eq!((adjust.tool, adjust.preview, adjust.before), (Tool::Sliders, false, false));
    assert_eq!(answer.pane, Pane::Result);
    assert!(view.on(ViewEvent::Previewed).adjust.unwrap().preview);
    assert_eq!(view.on(ViewEvent::HoldBefore { held: true }).pane, Pane::Stored);
    view.on(ViewEvent::HoldBefore { held: false });
    assert_eq!(view.on(ViewEvent::Tool { tool: Tool::Crop }).pane, Pane::Uncropped);
    assert!(view.on(ViewEvent::Adjust { on: false }).adjust.is_none());

    let reframe = Reframe {
        crop: Some(Rect { x: 10, y: 5, width: 20, height: 12 }),
        flip: Flip::Horizontal,
        turn: Turn::Quarter,
    };
    let result = PaneFraming::new(40, 30, reframe, Pane::Result);
    let uncropped = PaneFraming::new(40, 30, reframe, Pane::Uncropped);
    assert_eq!(result.size(), PicturePixel { x: 12, y: 20 });
    assert_eq!(uncropped.size(), PicturePixel { x: 30, y: 40 });
    assert!(result.crop_mark().is_none() && uncropped.crop_mark().is_some());
    let p = PicturePoint { x: 3.5, y: 7.5 };
    let back = result.to_shown(result.to_stored(p));
    assert!((back.x - p.x).abs() < 1e-9 && (back.y - p.y).abs() < 1e-9);
    // Switching to the uncropped view keeps the stored point under the
    // pointer.
    let at = PicturePlacement { zoom: 2.0, origin: PicturePoint { x: 100.0, y: 50.0 } };
    let screen = PicturePoint { x: 107.0, y: 65.0 };
    let kept = result.keeping(at, uncropped.clone(), screen);
    let shown = PicturePoint {
        x: (screen.x - kept.origin.x) / kept.zoom,
        y: (screen.y - kept.origin.y) / kept.zoom,
    };
    let (a, b) = (result.to_stored(p), uncropped.to_stored(shown));
    assert!((a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9, "{a:?} {b:?}");
}
