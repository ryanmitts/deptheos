//! End to end on real frames: each desktop case in the repository's
//! `e2e.toml` goes through the app as a user does (set added as a folder,
//! method and settings set, Stack clicked, exported with the Export sheet,
//! and when it paints, retouched with a stroke and exported again), and
//! `deptheos_e2e` checks each file: format and bit depth, the camera's
//! details without location, the picture against its reference, and a
//! retouch changed only where the stroke went, to the frame painted from.
//!
//! Ignored by default (it reads your frames and takes a while). `just e2e`
//! runs the desktop's smoke and full cases, `just e2e NAME` one or several
//! (`DEPTHEOS_E2E_CASE`, comma-separated), `just e2e-smoke` and
//! `just e2e-tier TIER` a tier's (`DEPTHEOS_E2E_TIER`).
//!
//! `DEPTHEOS_E2E_REFERENCES` (default `~/DeptheosE2E/references`) holds each
//! case's signed-off pictures; a case without them fails until
//! `DEPTHEOS_E2E_BLESS=1` (`just bless`) signs them off. `DEPTHEOS_E2E_OUT`
//! keeps the exports (default: a temporary folder); `DEPTHEOS_E2E_SETS`
//! overrides the folder of sets.

use std::time::Duration;

use deptheos_app::paint::{Dab, SourceKind};
use deptheos_app::{PyramidConfig, StackerKind, WaveletConfig};
use deptheos_e2e::cases::{Case, Config, Decoder, Method, Platform};
use deptheos_e2e::{Frames, Report, Set, check_paint, check_stack};

use super::*;
use crate::app::run::LastStack;
use crate::prefs::RawDecoder;
use crate::test_support::wait_up_to;
use crate::viewer::PaintInput;

/// The longest a stack of real frames, or its export, may take.
const LONG: Duration = Duration::from_secs(600);

/// One stroke across the middle of the render, this long and this wide.
const STROKE_LENGTH: f32 = 1200.0;
/// The brush's radius, in the render's pixels.
const BRUSH: f32 = 200.0;

#[gpui::test]
#[ignore = "real frames: run with `just e2e`"]
fn end_to_end(cx: &mut TestAppContext) {
    let config = Config::load(&deptheos_e2e::cases::default_path()).unwrap();
    let only = std::env::var("DEPTHEOS_E2E_CASE").ok();
    let tiers = std::env::var("DEPTHEOS_E2E_TIER").ok();
    let cases = config.cases_on(Platform::Desktop, only.as_deref(), tiers.as_deref()).unwrap();
    assert!(!cases.is_empty(), "no desktop cases in e2e.toml");
    let places = places();
    // Unlike other tests, no GPU is a failure: this checks the app works.
    let engine = engine().expect("the end-to-end test needs a GPU");
    let mut reports = Vec::new();
    for case in cases {
        eprintln!("\n== {}", case.name);
        reports.extend(run_case(cx, &engine, &config, case, &places));
    }
    eprintln!("\nexports in {}\n", places.out.display());
    for r in &reports {
        eprintln!("{r}");
    }
    let failed: Vec<&str> =
        reports.iter().filter(|r| !r.passed()).map(|r| r.title.as_str()).collect();
    assert!(failed.is_empty(), "failed: {failed:?}");
}

/// Where the references and exports are, from the environment.
struct Places {
    references: PathBuf,
    bless: bool,
    out: PathBuf,
    _temp: Option<tempfile::TempDir>,
}

fn places() -> Places {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let references = std::env::var_os("DEPTHEOS_E2E_REFERENCES")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join("DeptheosE2E/references"));
    let bless = std::env::var("DEPTHEOS_E2E_BLESS").is_ok_and(|v| v == "1");
    let (out, temp) = match std::env::var_os("DEPTHEOS_E2E_OUT") {
        Some(out) => (PathBuf::from(out), None),
        None => {
            let temp = tempfile::tempdir().unwrap();
            (temp.path().to_path_buf(), Some(temp))
        }
    };
    Places { references, bless, out, _temp: temp }
}

/// The settings `case` gives, its method's defaults for the rest.
fn stacker_config(case: &Case) -> StackerConfig {
    match case.method {
        Method::Pyramid => {
            let d = PyramidConfig::default();
            StackerConfig::Pyramid(PyramidConfig {
                smoothness: case.smoothness.unwrap_or(d.smoothness),
            })
        }
        Method::Depth => {
            let d = DepthConfig::default();
            StackerConfig::Depth(DepthConfig {
                radius: case.radius.unwrap_or(d.radius),
                smoothing: case.smoothing.unwrap_or(d.smoothing),
                blend_range: case.blend_range.unwrap_or(d.blend_range),
            })
        }
        Method::Wavelet => {
            let d = WaveletConfig::default();
            StackerConfig::Wavelet(WaveletConfig {
                denoise: case.denoise.unwrap_or(d.denoise),
                ..d
            })
        }
    }
}

fn run_case(
    cx: &mut TestAppContext,
    engine: &Arc<Engine>,
    config: &Config,
    case: &Case,
    places: &Places,
) -> Vec<Report> {
    if let Err(e) = case.apps_can_run() {
        return vec![Report::failed(case.name.clone(), "case", e)];
    }
    // The frames stacked and those exports are checked against: the same
    // set, or the same frames from another decoder (`check_set`).
    let set = match Set::open_checked(&case.set_dir(config)) {
        Ok(set) => set,
        Err(e) => return vec![Report::failed(case.name.clone(), "set", e)],
    };
    let checked = match &case.check_set {
        None => None,
        Some(_) => match Set::open_checked(&case.check_dir(config)) {
            Ok(checked) if checked.same_frames(&set) => Some(checked),
            Ok(checked) => {
                let detail = format!("{} isn't {}'s frames", checked.dir.display(), set.name);
                return vec![Report::failed(case.name.clone(), "check_set", detail)];
            }
            Err(e) => return vec![Report::failed(case.name.clone(), "check_set", e)],
        },
    };
    let check = checked.as_ref().unwrap_or(&set);
    let frames = match Frames::read(check) {
        Ok(frames) => frames,
        Err(e) => return vec![Report::failed(case.name.clone(), "frames", e)],
    };
    let library = tempfile::tempdir().unwrap();
    let out = places.out.join(&case.name).join("desktop");
    let _ = std::fs::remove_dir_all(&out);
    let reference = |format: ExportFormat, bits: u8| {
        let format = format!("{format:?}").to_lowercase();
        places.references.join(&case.name).join(format!("desktop-{format}-{bits}.png"))
    };
    let mut reports: Vec<Report> = Vec::new();

    let (window, app) = launch(cx, engine, Vec::new(), library.path());
    cx.update(|cx| cx.bind_keys(key_bindings()));

    // The case's decoder, as Settings sets it; else Automatic (the converter
    // where installed).
    let decoder = case.decoder.map(|d| match d {
        Decoder::LibRaw => RawDecoder::LibRaw,
        Decoder::AdobeDng => RawDecoder::AdobeDng,
    });
    cx.update(|cx| crate::prefs::update_prefs(cx, |p| p.raw_backend = decoder.unwrap_or_default()));
    if let Some(decoder) = decoder {
        let wanted = decoder.backend();
        if !wanted.is_available() {
            let detail = format!("{wanted:?} isn't installed here");
            return vec![Report::failed(case.name.clone(), "decoder", detail)];
        }
    }

    // Added as a folder, as a drop or Add Folder… adds one.
    let count = set.frames.len();
    act(cx, window, &app, |app, window, cx| app.add_paths(vec![set.dir.clone()], window, cx));
    wait_until(cx, "the frames listed", |cx| {
        app.read_with(cx, |app, cx| {
            let files = app.files.read(cx);
            files.counts() == (count, count) && files.can_stack()
        })
    });
    // The decoder applies once the list holds camera RAW, as the sidebar
    // offers it then.
    if let Some(decoder) = decoder {
        let chosen =
            app.read_with(cx, |app, cx| app.params.read(cx).session_options(cx).raw_backend);
        assert_eq!(chosen, decoder.backend(), "{}: the decoder the case names", case.name);
    }

    // The method clicked; its sliders set as dragging them would.
    let wanted = stacker_config(case);
    let ix = StackerKind::ALL.iter().position(|k| *k == wanted.kind()).unwrap();
    click(cx, window, &format!("method-{ix}"));
    act(cx, window, &app, |app, window, cx| {
        app.params.update(cx, |p, cx| {
            let options = p.session_options(cx);
            p.restore(&wanted, &options, window, cx)
        })
    });
    let config = app.read_with(cx, |a, cx| a.params.read(cx).stacker_config(cx));
    assert_eq!(format!("{config:?}"), format!("{wanted:?}"), "the settings asked for");
    click(cx, window, "stack");
    wait_up_to(cx, LONG, "the stack and its save", |cx| {
        app.read_with(cx, |app, cx| !app.is_processing(cx) && !app.renders.read(cx).is_saving())
    });
    // A failed stack fails this case, not the run.
    let failed = app.read_with(cx, |app, _| match &app.last_stack {
        LastStack::Stacked(s) => {
            eprintln!("stacked #{} in {:.1} s", s.number, s.seconds);
            None
        }
        LastStack::Failed(f) => Some(format!("the stack failed: {:?}", f.error)),
        LastStack::Nothing => None,
    });
    // A warning ending the stack (frames left out, render not kept) is a
    // notice, and fails the case in its words.
    let failed = failed.or_else(|| {
        let stacked = app.read_with(cx, |app, _| matches!(app.last_stack, LastStack::Stacked(_)));
        (!stacked).then(|| {
            let said =
                cx.update_window(window, |_, window, cx| crate::notices::last_said(window, cx));
            match said.ok().flatten() {
                Some(said) => format!("stacked with a warning: {said}"),
                None => "the stack ended without a render".to_string(),
            }
        })
    });
    if let Some(detail) = failed {
        reports.push(Report::failed(format!("{} · {}", set.name, case.name), "stacked", detail));
        return reports;
    }

    // Exported with the sheet as the case asks, checked at the case's
    // `stack_bits` whatever the sheet offered.
    let stack_dir = out.join("stack");
    let mut plain_tiff = None;
    for export_kind in &case.exports {
        let file =
            export(cx, window, &app, export_kind.format, export_kind.sixteen_bit, &stack_dir);
        let bits = export_kind.bits(case.stack_bits);
        reports.push(check_stack(
            check,
            &frames,
            &file,
            &case.expect(export_kind.format, bits),
            Some(&reference(export_kind.format, bits)),
            places.bless,
        ));
        if export_kind.format == ExportFormat::Tiff && plain_tiff.is_none() {
            plain_tiff = Some((file, export_kind.sixteen_bit));
        }
    }
    if !case.paint {
        return reports;
    }
    // A retouch is checked against the plain TIFF: one made if none was asked for.
    let (tiff, sixteen) = match plain_tiff {
        Some(t) => t,
        None => {
            let file = export(cx, window, &app, ExportFormat::Tiff, false, &stack_dir);
            let bits = 8;
            reports.push(check_stack(
                check,
                &frames,
                &file,
                &case.expect(ExportFormat::Tiff, bits),
                Some(&reference(ExportFormat::Tiff, bits)),
                places.bless,
            ));
            (file, false)
        }
    };

    // Retouch: one stroke across the middle from the named frame, or the one
    // with least detail there, so it visibly changes the stack.
    let (width, height) = image::image_dimensions(&tiff).unwrap();
    let (mid_x, mid_y) = (width as f32 / 2.0, height as f32 / 2.0);
    let source_ix = match &case.paint_from {
        Some(name) => set
            .frames
            .iter()
            .position(|f| f.file_name().is_some_and(|n| n == name.as_str()))
            .unwrap_or_else(|| panic!("{}: paint_from {name} isn't in the set", case.name)),
        None => {
            let at = frames.place_file(&tiff).unwrap();
            let reach = (STROKE_LENGTH / 2.0 + 250.0) as usize;
            let region = (
                at.0 + mid_x as usize - reach,
                at.1 + mid_y as usize - 250,
                at.0 + mid_x as usize + reach,
                at.1 + mid_y as usize + 250,
            );
            frames.softest_in(region)
        }
    };
    let source = set.frames[source_ix].clone();
    eprintln!("painting from {}", source.display());

    click(cx, window, "retouch");
    wait_up_to(cx, LONG, "Retouch open", |cx| {
        let state = paint_state(cx, &app);
        state.open.is_some() && state.source_ready
    });
    // As a click on its row in Paint From's frames does.
    app.update(cx, |app, cx| app.paint.update(cx, |p, cx| p.pick_frame(&source, cx)));
    let picked = SourceKind::Frame { path: source.clone() };
    wait_up_to(cx, LONG, "the frame painted from", |cx| {
        let state = paint_state(cx, &app);
        state.source.as_ref() == Some(&picked) && state.source_ready
    });
    // "]" until the brush is BRUSH px across, as a user sizes it.
    let radius = |cx: &mut TestAppContext| {
        app.read_with(cx, |app, cx| app.paint.read(cx).current_brush(cx).radius)
    };
    for _ in 0..40 {
        if radius(cx) >= BRUSH {
            break;
        }
        cx.simulate_keystrokes(window, "]");
    }
    let radius = radius(cx);
    assert!((BRUSH..BRUSH * 1.3).contains(&radius), "the brush is {radius} px");
    // The dabs the viewer sends for a drag, every 20 px.
    let dabs = (0..=(STROKE_LENGTH / 20.0) as usize).map(|i| Dab {
        x: mid_x - STROKE_LENGTH / 2.0 + 20.0 * i as f32,
        y: mid_y,
        pressure: 1.0,
    });
    app.update(cx, |app, cx| {
        app.paint.update(cx, |p, cx| {
            let mut dabs = dabs.clone();
            p.input(PaintInput::Begin(dabs.next().unwrap()), cx);
            for dab in dabs {
                p.input(PaintInput::Move(dab), cx);
            }
            p.input(PaintInput::End, cx);
        })
    });
    wait_up_to(cx, LONG, "the stroke", |cx| paint_state(cx, &app).undo == 1);
    cx.simulate_keystrokes(window, "escape");
    app.read_with(cx, |app, cx| assert!(!app.paint.read(cx).is_open(), "Retouch closed"));

    let painted = export(cx, window, &app, ExportFormat::Tiff, sixteen, &out.join("retouched"));
    reports.push(check_paint(check, &frames, &tiff, &painted, &check.frames[source_ix]));
    reports
}

/// Export with the sheet as a user does: Export, the format segment, the
/// depth radio, Last Folder (`dir`, Show When Done off), Export. Returns the
/// file once written.
fn export(
    cx: &mut TestAppContext,
    window: gpui::AnyWindowHandle,
    app: &Entity<AppView>,
    format: ExportFormat,
    sixteen_bit: bool,
    dir: &Path,
) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let dir = dir.to_path_buf();
    // As if exported here before, Show When Done off (a test can't open Finder).
    cx.update(|cx| {
        crate::prefs::update_prefs(cx, |p| {
            p.save_dir = Some(dir.clone());
            p.reveal_export = false;
        })
    });
    click(cx, window, "export");
    wait_until(cx, "the Export sheet", |cx| {
        app.read_with(cx, |app, _| export_sheet(app).is_some())
    });
    let segment = match format {
        ExportFormat::Tiff => 0,
        ExportFormat::Jpeg => 1,
        ExportFormat::Png => 2,
        ExportFormat::Dng => 3,
    };
    click(cx, window, &format!("export-format-{segment}"));
    let dialog = app.read_with(cx, |app, _| export_sheet(app)).expect("the sheet is open");
    // The radio, unless the format or the stack fixes the depth.
    if !dialog.read_with(cx, |d, _| d.depth_fixed()) {
        click(cx, window, if sixteen_bit { "export-16" } else { "export-8" });
    }
    click(cx, window, "export-last");
    let choice = dialog.read_with(cx, |d, cx| d.choice(cx)).expect("it can export");
    assert_eq!(choice.format, ExportFormat::ALL[segment], "{format:?} offered");
    assert_eq!(choice.dir, dir, "Last Folder is where it goes");
    let path = choice.path();
    assert!(!path.exists(), "{} is there already", path.display());
    click(cx, window, "export-go-button");
    wait_up_to(cx, LONG, "the export", |cx| {
        path.exists()
            && app.read_with(cx, |app, cx| {
                export_sheet(app).is_none()
                    && app.export_progress_text(cx).is_none()
                    && !app.is_exporting()
            })
    });
    eprintln!("exported {}", path.display());
    path
}
