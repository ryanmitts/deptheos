//! The Library: renders round-trip, edits keep unknown fields, and the
//! listing skips anything half written.

use std::path::{Path, PathBuf};

use deptheos_app::library::history::{Batch, BatchLabel, History, StackId};
use deptheos_app::runs::batch::Split;
use deptheos_app::{
    AlignMode, AlignSettings, Alignment, CompletedStack, Gray16Image, GridConfig, SessionOptions,
    StackColor, StackGeometry, StackerConfig,
};
use deptheos_core::depth::DepthConfig;
use deptheos_core::{Affine2, FrameAlignment, FramePlane};
use image::RgbImage;
use image::math::Rect;

const W: u32 = 37;
const H: u32 = 23;

/// The frames `names`, made in `dir` (a Library names frames by folder).
fn paths(dir: &Path, names: &[&str]) -> Vec<PathBuf> {
    let paths: Vec<PathBuf> = names.iter().map(|n| dir.join(n)).collect();
    for path in &paths {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, path.as_os_str().as_encoded_bytes()).unwrap();
    }
    paths
}

/// Samples over the full u16 range, so a lossy or 8-bit path would show.
fn pixels(width: u32, height: u32) -> Vec<u16> {
    (0..width * height * 3).map(|i| i.wrapping_mul(2_654_435_761) as u16).collect()
}

fn shift(tx: f32, ty: f32) -> FrameAlignment {
    FrameAlignment::new(
        Affine2::from_matrix_unchecked(nalgebra::Matrix3::new(
            1.000_31, -0.000_7, tx, 0.000_7, 1.000_31, ty, 0.0, 0.0, 1.0,
        )),
        0.98,
    )
}

fn plane_code(index: u32, total: u32) -> u16 {
    (FramePlane::new(index, total).plane * 65535.0).round() as u16
}

/// A finished sRGB stack of `stacked`, made by hand: no GPU needed.
fn srgb_stack(stacked: Vec<PathBuf>) -> CompletedStack {
    let transforms = (0..stacked.len()).map(|i| Some(shift(i as f32 * 1.5, -(i as f32)))).collect();
    CompletedStack {
        width: W,
        height: H,
        pixels: pixels(W, H),
        frame_map: Some(Gray16Image::from_fn(W, H, |x, y| {
            image::Luma([plane_code(300 + (x / 7 + y / 5) % 2, 600)])
        })),
        color: StackColor::Srgb { sixteen_bit: true, profile: None },
        geometry: StackGeometry {
            transforms,
            crop: Some(Rect { x: 2, y: 1, width: W, height: H }),
            reversed: true,
            left_out: Vec::new(),
        },
        config: StackerConfig::Depth(DepthConfig {
            radius: 12.0,
            smoothing: 2.5,
            blend_range: 7.5,
        }),
        options: SessionOptions {
            alignment: Alignment::On(AlignSettings {
                mode: AlignMode::Grid,
                grid: GridConfig::default(),
                crop: false,
            }),
            ..Default::default()
        },
        paths: stacked,
        notes: Vec::new(),
        contributions: Default::default(),
    }
}

fn thumbnail() -> RgbImage {
    RgbImage::from_fn(1000, 600, |x, y| image::Rgb([(x / 4) as u8, (y / 3) as u8, 90]))
}

/// A batch's header is stored as data and worded when shown.
#[test]
fn a_batch_keeps_its_plan_and_words_it_when_shown() {
    use deptheos_app::runs::batch::Plan;
    let dir = tempfile::tempdir().unwrap();
    let history = History::open(dir.path().join("store")).unwrap();
    let listed = paths(dir.path(), &["a.png", "b.png"]);
    let words = deptheos_app::Words::english(deptheos_app::Platform::Desktop);
    let plan = Plan { split: Split::AutoSort, skipped: 2, ..Plan::default() };
    let batch = Batch::of_plan(&plan);
    let stack = srgb_stack(listed.clone());
    let render =
        history.save_render(&listed, &stack, &thumbnail(), &StackId::new(), Some(&batch)).unwrap();
    let file =
        std::fs::read_to_string(history.render_path(&render.id).join("render.json")).unwrap();
    let file: serde_json::Value = serde_json::from_str(&file).unwrap();
    assert_eq!(file["batch"]["split"], "auto_sort");
    assert_eq!(file["batch"]["skipped"], 2);
    let saved = history.list().unwrap().remove(0).batch.unwrap();
    assert_eq!(saved, batch);
    assert_eq!(saved.label, BatchLabel { split: Split::AutoSort, skipped: 2 });
    assert_eq!(saved.label.text(&words), "Auto sort · 2 frames skipped");
}

#[test]
fn a_render_round_trips_with_its_list() {
    let dir = tempfile::tempdir().unwrap();
    let history = History::open(dir.path().join("store")).unwrap();
    let listed = paths(dir.path(), &["a.png", "b.png", "c.png"]);
    // Frame b left out.
    let stack = srgb_stack(vec![listed[0].clone(), listed[2].clone()]);
    let batch = Batch::new(BatchLabel { split: Split::AutoSort, skipped: 0 });
    let render =
        history.save_render(&listed, &stack, &thumbnail(), &StackId::new(), Some(&batch)).unwrap();
    assert_eq!(render.included, [true, false, true]);

    let renders = history.list().unwrap();
    assert_eq!(renders.len(), 1);
    let saved = &renders[0];
    assert_eq!((saved.id.clone(), saved.frames.clone()), (render.id.clone(), listed.clone()));
    assert_eq!(saved.batch, Some(batch));
    assert_eq!((saved.width, saved.height, saved.linear), (W, H, false));

    let loaded = history.load_render(&render.id).unwrap();
    assert_eq!((loaded.width, loaded.height), (W, H));
    assert!(loaded.pixels == stack.pixels, "result samples changed");
    assert!(loaded.frame_map == stack.frame_map, "the frame map is exact");
    assert_eq!(
        loaded.frame_contributions(),
        stack.frame_contributions(),
        "each frame's share, counted again from the frame map read back"
    );
    assert_eq!(loaded.paths, stack.paths);
    assert_eq!(loaded.geometry, stack.geometry);
    assert_eq!(format!("{:?}", loaded.config), format!("{:?}", stack.config));
    assert_eq!(format!("{:?}", loaded.options), format!("{:?}", stack.options));
    assert!(matches!(loaded.color, StackColor::Srgb { sixteen_bit: true, profile: None }));

    let (stacked, geometry) = history.load_geometry(&render.id).unwrap();
    assert_eq!((stacked, geometry), (stack.paths.clone(), stack.geometry.clone()));
    // Its settings alone, as a turntable's grow replays it: no picture.
    let settings = history.load_settings(&render.id).unwrap();
    assert!(settings.pixels.is_empty() && settings.frame_map.is_none());
    assert_eq!((settings.width, settings.height), (W, H));
    assert_eq!((settings.paths.clone(), settings.geometry.clone()), (stack.paths, stack.geometry));
    assert_eq!(format!("{:?}", settings.config), format!("{:?}", loaded.config));
    assert_eq!(format!("{:?}", settings.options), format!("{:?}", loaded.options));
    assert!(matches!(settings.color, StackColor::Srgb { sixteen_bit: true, profile: None }));
    let thumb = history.load_thumbnail(&render.id).unwrap();
    assert_eq!(thumb.dimensions(), (512, 307), "long side capped, aspect kept");

    // Its list: all three, b excluded.
    let list = deptheos_app::frames::list::List::from_render(saved);
    let frames: Vec<(PathBuf, bool)> = list
        .shots()
        .iter()
        .map(|s| (s.file(list.source).to_path_buf(), list.is_included(s)))
        .collect();
    assert_eq!(
        frames,
        [(listed[0].clone(), true), (listed[1].clone(), false), (listed[2].clone(), true)]
    );
}

#[test]
fn a_render_keeps_the_frames_its_stack_left_out() {
    let dir = tempfile::tempdir().unwrap();
    let history = History::open(dir.path().join("store")).unwrap();
    let listed = paths(dir.path(), &["a.png", "b.png", "c.png", "d.png"]);
    // d is excluded by the user; b was given but could not be stacked.
    let mut stack = srgb_stack(listed[..3].to_vec());
    stack.geometry.transforms[1] = None;
    stack.geometry.left_out =
        vec![deptheos_app::LeftOut { index: 1, reason: "alignment failed: too far".into() }];
    let render = history.save_render(&listed, &stack, &thumbnail(), &StackId::new(), None).unwrap();
    let saved = &history.list().unwrap()[0];
    for r in [&render, saved] {
        assert_eq!(r.included, [true, true, true, false], "b keeps its place in the stack");
        assert_eq!(r.left_out.len(), 1);
        assert_eq!(r.left_out[0].path, listed[1]);
        assert_eq!(r.stacked().collect::<Vec<_>>(), [&listed[0], &listed[2]]);
    }
    assert_eq!(
        saved
            .left_out_message(&deptheos_app::Words::english(deptheos_app::Platform::Desktop))
            .unwrap(),
        "Left out b.png (alignment failed: too far)"
    );
    assert_eq!(history.load_render(&render.id).unwrap().geometry, stack.geometry);

    // Its list comes back with b unchecked, to fix and stack again.
    let list = deptheos_app::frames::list::List::from_render(saved);
    let included: Vec<bool> = list.shots().iter().map(|s| list.is_included(s)).collect();
    assert_eq!(included, [true, false, true, false]);
}

/// A render encodes into `.partial`, which a save renames and a drop removes.
#[test]
fn encoded_pictures_wait_in_their_partial_folder() {
    let dir = tempfile::tempdir().unwrap();
    let history = History::open(dir.path().join("store")).unwrap();
    let listed = paths(dir.path(), &["a.png", "b.png", "c.png"]);
    let stack = srgb_stack(listed.clone());
    let renders = history.root().join("Renders");
    let folders = || std::fs::read_dir(&renders).unwrap().count();

    let dropped = history.encode_render(&stack, &thumbnail()).unwrap();
    assert_eq!(folders(), 1, "its partial folder");
    drop(dropped);
    assert_eq!(folders(), 0);

    let pictures = history.encode_render(&stack, &thumbnail()).unwrap();
    let render = history.save_encoded(&listed, &stack, pictures, &StackId::new(), None).unwrap();
    let names: Vec<_> =
        std::fs::read_dir(&renders).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(names, [std::ffi::OsString::from(render.id.as_str())]);
    assert_eq!(history.load_render(&render.id).unwrap().pixels, stack.pixels);
}

/// A save whose future is dropped at once still lands in the Library.
#[test]
fn a_save_dropped_at_once_still_lands() {
    let dir = tempfile::tempdir().unwrap();
    let history = std::sync::Arc::new(History::open(dir.path().join("store")).unwrap());
    let listed = paths(dir.path(), &["a.png", "b.png"]);
    let stack = std::sync::Arc::new(srgb_stack(listed.clone()));
    drop(history.save(listed, stack.clone(), thumbnail(), StackId::new(), None));
    let start = std::time::Instant::now();
    let render = loop {
        if let Some(render) = history.list().unwrap().pop() {
            break render;
        }
        assert!(start.elapsed() < std::time::Duration::from_secs(30), "never saved");
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    assert_eq!(history.load_render(&render.id).unwrap().pixels, stack.pixels);
}

/// Opening a render decodes in the asking work, whose token stops it.
#[test]
fn a_render_opened_within_canceled_work_is_canceled() {
    let dir = tempfile::tempdir().unwrap();
    let history = History::open(dir.path().join("store")).unwrap();
    let listed = paths(dir.path(), &["a.png", "b.png", "c.png"]);
    let render = history
        .save_render(&listed, &srgb_stack(listed.clone()), &thumbnail(), &StackId::new(), None)
        .unwrap();
    let canceled = deptheos_app::Cancel::root();
    canceled.cancel();
    let loaded = deptheos_core::cancel::within(&canceled, 2, || history.load_render(&render.id));
    assert!(
        matches!(loaded, Err(deptheos_app::StoreError::Dng(deptheos_dng::DngError::Canceled))),
        "{:?}",
        loaded.err()
    );
    let live = deptheos_app::Cancel::root();
    let loaded = deptheos_core::cancel::within(&live, 2, || history.load_render(&render.id));
    assert_eq!(loaded.unwrap().pixels, srgb_stack(listed).pixels);
}

#[test]
fn editing_a_saved_render_leaves_no_temporary_file() {
    let dir = tempfile::tempdir().unwrap();
    let history = History::open(dir.path()).unwrap();
    let listed = paths(dir.path(), &["a.png", "b.png"]);
    let render = history
        .save_render(&listed, &srgb_stack(listed.clone()), &thumbnail(), &StackId::new(), None)
        .unwrap();

    history.rename_render(&render.id, Some("Bee")).unwrap();
    history.set_render_picked(&render.id, true).unwrap();
    let listed = history.render(&render.id).unwrap();
    assert_eq!((listed.name.as_deref(), listed.picked), (Some("Bee"), true));
    // No temporary file is left beside it.
    let names: Vec<_> = std::fs::read_dir(history.render_path(&render.id))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".partial"))
        .collect();
    assert!(names.is_empty(), "{names:?}");
}

#[test]
fn the_listing_skips_half_written_and_unreadable_renders() {
    let dir = tempfile::tempdir().unwrap();
    let history = History::open(dir.path()).unwrap();
    let listed = paths(dir.path(), &["a.png", "b.png"]);
    let kept = history
        .save_render(&listed, &srgb_stack(listed.clone()), &thumbnail(), &StackId::new(), None)
        .unwrap();
    let renders = dir.path().join("Renders");
    // A render being written, one with no manifest, one of a newer format.
    let id = |n: u8| format!("{n:032x}");
    std::fs::create_dir_all(renders.join(format!("{}.partial", id(1)))).unwrap();
    std::fs::create_dir_all(renders.join(id(2))).unwrap();
    std::fs::create_dir_all(renders.join(id(3))).unwrap();
    std::fs::write(renders.join(id(3)).join("render.json"), r#"{"schema": 99}"#).unwrap();

    let ids: Vec<_> = history.list().unwrap().into_iter().map(|r| r.id).collect();
    assert_eq!(ids, [kept.id]);
}

#[test]
fn renders_are_numbered_in_their_stack_and_numbers_never_shift() {
    let dir = tempfile::tempdir().unwrap();
    let history = History::open(dir.path()).unwrap();
    let listed = paths(dir.path(), &["a.png", "b.png"]);
    let save = |of: &StackId| {
        history.save_render(&listed, &srgb_stack(listed.clone()), &thumbnail(), of, None).unwrap()
    };
    let (bee, ant) = (StackId::new(), StackId::new());
    let first = save(&bee);
    let second = save(&bee);
    let other = save(&ant);
    assert_eq!((first.number, second.number, other.number), (1, 2, 1));
    assert_eq!(first.stack, bee);

    // Deleting #2 leaves #1, and the next is #3, not #2 again.
    let third = save(&bee);
    history
        .delete_renders(
            std::slice::from_ref(&second.id),
            history.close_paint(std::slice::from_ref(&second.id)),
        )
        .unwrap();
    let fourth = save(&bee);
    assert_eq!((third.number, fourth.number), (3, 4));
    let mut numbers: Vec<u32> =
        history.list().unwrap().iter().filter(|r| r.stack == bee).map(|r| r.number).collect();
    numbers.sort();
    assert_eq!(numbers, [1, 3, 4]);

    // Written down, so the listing reads them back.
    let listed = history.render(&fourth.id).unwrap();
    assert_eq!((listed.stack, listed.number), (bee, 4));
}

/// Delete Library deletes what it lists, leaving a store that opens empty
/// and numbers from #1; unknown folders, unreadable renders and frames stay.
#[test]
fn deleting_the_library_deletes_what_it_lists() {
    use deptheos_app::library::stacks::Settings;
    use deptheos_app::library::stacks::Working;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("store");
    let history = History::open(&root).unwrap();
    let listed = paths(dir.path(), &["a.png", "b.png"]);
    for frame in &listed {
        std::fs::write(frame, b"a frame").unwrap();
    }
    let bee = StackId::new();
    let stack = srgb_stack(listed.clone());
    let render = history.save_render(&listed, &stack, &thumbnail(), &bee, None).unwrap();
    history.rename_stack(&bee, Some("Bee")).unwrap();
    // A stack named and never stacked.
    history.rename_stack(&StackId::new(), Some("Wasp")).unwrap();
    let settings = Settings { config: stack.config, options: stack.options };
    let run = Batch::new(BatchLabel::default());
    history.save_failure("Ant", &listed, &settings, &StackId::new(), &run, "no").unwrap();
    history.write_working(&Working::of_render(&render)).unwrap();
    std::fs::write(root.join("list.unreadable-1.json"), "{").unwrap();
    // What the listing doesn't know.
    let renders = root.join("Renders");
    let unreadable = renders.join(format!("{:032x}", 2));
    std::fs::create_dir_all(&unreadable).unwrap();
    std::fs::write(unreadable.join("render.json"), vec![b'x'; 1000]).unwrap();
    let foreign = root.join("Stacks").join("Mine");
    std::fs::create_dir_all(&foreign).unwrap();
    std::fs::write(foreign.join("notes.txt"), "not the Library's").unwrap();
    std::fs::write(root.join("mine.txt"), "not the Library's").unwrap();

    history.delete_library(history.close_all_paint()).unwrap();

    assert!(history.list().unwrap().is_empty());
    assert!(history.failures().unwrap().is_empty());
    assert!(history.stacks().unwrap().is_empty());
    assert!(!history.render_path(&render.id).exists());
    assert!(!root.join("list.json").exists());
    assert!(!root.join("list.unreadable-1.json").exists());
    assert!(foreign.join("notes.txt").exists(), "a folder of someone else's in Stacks stays");
    assert!(unreadable.join("render.json").exists(), "a render the listing can't read stays");
    assert!(root.join("mine.txt").exists(), "the rest of the root stays");
    assert!(listed.iter().all(|f| f.exists()), "the frames stay");
    // As a first launch: an empty working version, numbering from #1.
    drop(history);
    let reopened = History::open(&root).unwrap();
    assert!(reopened.read_working().unwrap().list.is_empty());
    let again = reopened.save_render(&listed, &stack, &thumbnail(), &bee, None).unwrap();
    assert_eq!(again.number, 1);
    // An empty Library deletes as well.
    let empty = History::open(dir.path().join("empty")).unwrap();
    empty.delete_library(empty.close_all_paint()).unwrap();
}

/// A batch's pending jobs are hidden while it runs, removed as renders save,
/// shown on failure, and the rest shown when it ends or at the next open.
#[test]
fn a_batch_records_its_jobs_as_it_starts() {
    use deptheos_app::library::stacks::Settings;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("store");
    let history = History::open(&root).unwrap();
    let listed = paths(dir.path(), &["a/1.png", "a/2.png", "b/1.png", "b/2.png", "c/1.png"]);
    let stack = srgb_stack(listed[..2].to_vec());
    let settings = Settings { config: stack.config, options: stack.options };
    let batch = Batch::new(BatchLabel::default());
    let of = [StackId::new(), StackId::new(), StackId::new()];
    let jobs =
        [("a", &listed[..2], &of[0]), ("b", &listed[2..4], &of[1]), ("c", &listed[4..], &of[2])];
    let why = "Not stacked yet.";
    history.record_unstacked(&jobs, &settings, &batch, why).unwrap();
    assert!(history.failures().unwrap().is_empty(), "hidden while the run lasts");

    history.save_render(&listed[..2], &stack, &thumbnail(), &of[0], Some(&batch)).unwrap();
    history.save_failure("b", &listed[2..4], &settings, &of[1], &batch, "boom").unwrap();
    let shown = history.failures().unwrap();
    assert_eq!(shown.len(), 1, "a's went as it saved; b's is shown, reworded in place");
    assert_eq!((shown[0].label.as_str(), shown[0].error.as_str()), ("b", "boom"));
    assert_eq!(shown[0].stack, of[1]);

    // A run that never ended: the next open shows what it left.
    drop(history);
    let reopened = History::open(&root).unwrap();
    let mut left: Vec<(String, String)> =
        reopened.failures().unwrap().into_iter().map(|f| (f.label, f.error)).collect();
    left.sort();
    assert_eq!(left, [("b".into(), "boom".into()), ("c".into(), why.into())]);

    // A run that ends shows them at once.
    let again = Batch::new(BatchLabel::default());
    let d = StackId::new();
    reopened.record_unstacked(&[("d", &listed[..2], &d)], &settings, &again, why).unwrap();
    assert_eq!(reopened.failures().unwrap().len(), 2);
    reopened.batch_ended();
    assert_eq!(reopened.failures().unwrap().len(), 3);
}

/// A batch's renders are numbered in save order, not encode order; new
/// stacks start at #1.
#[test]
fn a_batch_numbers_its_renders_in_the_order_they_save() {
    let dir = tempfile::tempdir().unwrap();
    let history = std::sync::Arc::new(History::open(dir.path().join("store")).unwrap());
    let listed = paths(dir.path(), &["a.png", "b.png", "c.png"]);
    let stack = srgb_stack(listed.clone());
    let batch = Batch::new(BatchLabel::default());
    let bee = StackId::new();
    history.save_render(&listed, &stack, &thumbnail(), &bee, None).unwrap();

    // Three encoded side by side, then saved last first.
    let encoded: Vec<_> = std::thread::scope(|scope| {
        let encoding: Vec<_> = (0..3)
            .map(|_| scope.spawn(|| history.encode_render(&stack, &thumbnail()).unwrap()))
            .collect();
        encoding.into_iter().map(|e| e.join().unwrap()).collect()
    });
    let numbers: Vec<u32> = encoded
        .into_iter()
        .rev()
        .map(|pictures| {
            history.save_encoded(&listed, &stack, pictures, &bee, Some(&batch)).unwrap().number
        })
        .collect();
    assert_eq!(numbers, [2, 3, 4]);

    let fresh: Vec<u32> = (0..2)
        .map(|_| {
            let of = StackId::new();
            history.save_render(&listed, &stack, &thumbnail(), &of, Some(&batch)).unwrap().number
        })
        .collect();
    assert_eq!(fresh, [1, 1], "each new stack starts at #1");
    let after = history.save_render(&listed, &stack, &thumbnail(), &bee, None).unwrap();
    assert_eq!(after.number, 5);
}
