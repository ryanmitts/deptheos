//! Files on a user's disk load, and unreadable ones are handled as
//! `fsutil.rs`'s module doc says. Each folder under `tests/fixtures/` was
//! written once by [`write_fixtures`] or [`write_library_fixture`] and is
//! never rewritten; bumping a schema adds a folder beside the old one, whose
//! test then checks the migration.
//!
//! A fixture is a Library holding its frames (`Photos/Bee`), named relative
//! to it, so a copy anywhere finds them. Tests load a temporary copy, since
//! loading may set files aside.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use deptheos_app::frames::list::List;
use deptheos_app::library::history::{Batch, BatchLabel, History, StackId};
use deptheos_app::library::stacks::Settings;
use deptheos_app::library::stacks::Working;
use deptheos_app::paint::store;
use deptheos_app::paint::{Layer, SourceKind, TILE, TILE_PIXELS, TileData, TileId};
use deptheos_app::runs::batch::Split;
use deptheos_app::{
    AlignMode, AlignSettings, Alignment, CompletedStack, DcpPreference, FrameOrder, Gray16Image,
    GridConfig, RawBackend, SessionOptions, StackerConfig,
};
use deptheos_core::depth::DepthConfig;
use deptheos_core::pyramid::PyramidConfig;
use image::RgbImage;

const W: u32 = 37;
const H: u32 = 23;
/// The paint fixture's render size: 2×2 canvas tiles.
const PAINT_W: u32 = 200;
const PAINT_H: u32 = 150;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// The frames `names`, in the Library at `library`.
fn frames(library: &Path, names: &[&str]) -> Vec<PathBuf> {
    names.iter().map(|n| library.join("Photos/Bee").join(n)).collect()
}

/// The frames a.png, b.png and c.png, made in the Library at `library`.
fn make_frames(library: &Path) -> Vec<PathBuf> {
    let listed = frames(library, &["a.png", "b.png", "c.png"]);
    std::fs::create_dir_all(listed[0].parent().unwrap()).unwrap();
    for frame in &listed {
        std::fs::write(frame, frame.file_name().unwrap().as_encoded_bytes()).unwrap();
    }
    listed
}

fn pixels() -> Vec<u16> {
    (0..W * H * 3).map(|i| i.wrapping_mul(2_654_435_761) as u16).collect()
}

/// Not the defaults, so a file that dropped one would show.
fn options() -> SessionOptions {
    SessionOptions {
        alignment: Alignment::On(AlignSettings {
            mode: AlignMode::Grid,
            grid: GridConfig::default(),
            crop: false,
        }),
        raw_backend: RawBackend::AdobeDng,
        camera_profile: DcpPreference::CameraStandard,
        ..Default::default()
    }
}

fn depth() -> StackerConfig {
    StackerConfig::Depth(DepthConfig { radius: 12.0, smoothing: 2.5, blend_range: 7.5 })
}

/// Frames a.png and c.png stacked; b.png listed but excluded.
fn render_stack(library: &Path) -> CompletedStack {
    let mut stack = CompletedStack {
        frame_map: Some(Gray16Image::from_fn(W, H, |x, _| image::Luma([(x * 1000) as u16]))),
        config: depth(),
        options: options(),
        ..CompletedStack::srgb(W, H, pixels(), frames(library, &["a.png", "c.png"]))
    };
    stack.geometry.reversed = true;
    stack
}

/// A painted tile: a gradient, half covered, from source 1.
fn painted_tile() -> TileData {
    TileData {
        rgb: (0..3 * TILE_PIXELS).map(|i| (i * 7) as u16).collect(),
        coverage: (0..TILE_PIXELS).map(|i| (i % 256) as u8).collect(),
        source: vec![1; TILE_PIXELS].into_boxed_slice(),
    }
}

/// The batch the fixtures were made by.
fn auto_sort() -> BatchLabel {
    BatchLabel { split: Split::AutoSort, skipped: 0 }
}

/// The schema of the JSON file at `path`.
fn schema(path: &Path) -> u64 {
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    json["schema"].as_u64().unwrap()
}

/// Move `from` to `fixtures/<name>-v<schema>` unless that version exists.
fn keep(from: &Path, name: &str, schema: u64) {
    let to = fixtures().join(format!("{name}-v{schema}"));
    if to.exists() {
        println!("{} is already there", to.display());
        return;
    }
    copy_dir(from, &to);
    println!("wrote {}", to.display());
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// A temporary copy of fixture `name`.
fn fixture(name: &str) -> tempfile::TempDir {
    let from = fixtures().join(name);
    assert!(from.is_dir(), "{} is missing", from.display());
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&from, dir.path());
    dir
}

/// Write any missing fixtures for the current schemas:
/// `cargo test -p deptheos_app --test integration formats -- --ignored`
#[test]
#[ignore = "writes tests/fixtures; run by hand after a schema bump"]
fn write_fixtures() {
    // A render.
    let dir = tempfile::tempdir().unwrap();
    let history = History::open(dir.path()).unwrap();
    let thumb = RgbImage::from_fn(W, H, |x, y| image::Rgb([x as u8 * 6, y as u8 * 10, 90]));
    let listed = make_frames(dir.path());
    let batch = Batch::new(auto_sort());
    let render = history
        .save_render(&listed, &render_stack(dir.path()), &thumb, &StackId::new(), Some(&batch))
        .unwrap();
    history.rename_render(&render.id, Some("Bee")).unwrap();
    let manifest = history.render_path(&render.id).join("render.json");
    keep(dir.path(), "render", schema(&manifest));

    // The list and live settings.
    let dir = tempfile::tempdir().unwrap();
    let history = History::open(dir.path()).unwrap();
    let listed = make_frames(dir.path());
    let mut list = List::of_frames(&listed, FrameOrder::default());
    let b = list.shots()[1].clone();
    list.set_included(&b, false);
    let settings =
        Settings { config: StackerConfig::Pyramid(PyramidConfig::default()), options: options() };
    let working = Working { list, settings: Some(settings), ..Working::new(None) };
    history.write_working(&working).unwrap();
    keep(dir.path(), "list", schema(&dir.path().join("list.json")));

    // A failed batch stack.
    let dir = tempfile::tempdir().unwrap();
    let history = History::open(dir.path()).unwrap();
    let listed = make_frames(dir.path());
    let failure = history
        .save_failure(
            "Bee · 2",
            &listed,
            &Settings { config: depth(), options: options() },
            &StackId::new(),
            &batch,
            "alignment failed",
        )
        .unwrap();
    let file = dir.path().join("Failed").join(format!("{}.json", failure.id.as_str()));
    keep(dir.path(), "failure", schema(&file));

    // Paint, in a render folder of its own (the store needs only its
    // manifest to exist), named for its `user_version`.
    let dir = tempfile::tempdir().unwrap();
    let history = History::open(dir.path()).unwrap();
    make_frames(dir.path());
    let render = dir.path().join("render");
    std::fs::create_dir_all(&render).unwrap();
    std::fs::write(render.join("render.json"), "{}").unwrap();
    {
        let mut doc = store::open_doc(&render, PAINT_W, PAINT_H);
        doc.open(paint_base(dir.path()));
        let b = frames(dir.path(), &["b.png"])[0].clone();
        doc.sources.id(&SourceKind::Frame { path: b }, history.frames()).unwrap();
        let canvas = doc.canvas_mut().unwrap();
        canvas.replace(TileId { x: 1, y: 1 }, Some(Arc::new(painted_tile())));
        assert!(store::save(&doc).unwrap());
    }
    keep(dir.path(), "paint", store::USER_VERSION as u64);
}

/// The version of the app whose Library `library-<version>` is.
const LIBRARY_VERSION: &str = "0.2.0";

/// Write `fixtures/library-<version>`, a whole Library, unless it exists.
/// Run once per release:
/// `cargo test -p deptheos_app --test integration library_fixture -- --ignored`
#[test]
#[ignore = "writes tests/fixtures; run by hand for a new release"]
fn write_library_fixture() {
    let dir = tempfile::tempdir().unwrap();
    let library = dir.path();
    let history = History::open(library).unwrap();
    let thumb = RgbImage::from_fn(W, H, |x, y| image::Rgb([x as u8 * 6, y as u8 * 10, 90]));
    // Bees: two batch renders, the first named and starred, the stack
    // renamed, its working version kept while Ants is on screen.
    let bees = make_frames(library);
    let bee = StackId::new();
    let batch = Batch::new(auto_sort());
    let first =
        history.save_render(&bees, &render_stack(library), &thumb, &bee, Some(&batch)).unwrap();
    history.rename_render(&first.id, Some("Bee")).unwrap();
    history.set_render_picked(&first.id, true).unwrap();
    let pyramid = CompletedStack {
        config: StackerConfig::Pyramid(PyramidConfig::default()),
        ..render_stack(library)
    };
    history.save_render(&bees, &pyramid, &thumb, &bee, None).unwrap();
    history.rename_stack(&bee, Some("Bees")).unwrap();
    // A failed stack of the batch.
    let settings = Settings { config: depth(), options: options() };
    history
        .save_failure("Bee · 2", &bees, &settings, &StackId::new(), &batch, "alignment failed")
        .unwrap();
    // Ants: a render with paint, on screen with one frame left out.
    let ants: Vec<PathBuf> =
        ["a.png", "b.png"].iter().map(|n| library.join("Photos/Ant").join(n)).collect();
    std::fs::create_dir_all(ants[0].parent().unwrap()).unwrap();
    for frame in &ants {
        std::fs::write(frame, frame.file_name().unwrap().as_encoded_bytes()).unwrap();
    }
    let ant = StackId::new();
    let base = paint_base_of(&ants);
    let ant_thumb = RgbImage::from_pixel(PAINT_W, PAINT_H, image::Rgb([90, 90, 90]));
    let painted = history.save_render(&ants, &base, &ant_thumb, &ant, None).unwrap();
    {
        let render = history.render_path(&painted.id);
        let mut doc = store::open_doc(&render, PAINT_W, PAINT_H);
        doc.open(Arc::new(paint_base_of(&ants)));
        doc.sources.id(&SourceKind::Frame { path: ants[1].clone() }, history.frames()).unwrap();
        let canvas = doc.canvas_mut().unwrap();
        canvas.replace(TileId { x: 1, y: 1 }, Some(Arc::new(painted_tile())));
        assert!(store::save(&doc).unwrap());
        doc.close();
    }
    let renders = history.list().unwrap();
    let mut edited = Working::of_render(&renders[1]);
    let a = edited.list.shots()[0].clone();
    edited.list.set_included(&a, false);
    history.write_working(&Working { stack: bee.clone(), ..edited }).unwrap();
    let on_screen = history.read_working().unwrap();
    history.switch_stack(&on_screen, &ant, &renders, None).unwrap().expect("Ants opens");
    drop(history);
    let to = fixtures().join(format!("library-{LIBRARY_VERSION}"));
    if to.exists() {
        println!("{} is already there", to.display());
        return;
    }
    copy_dir(library, &to);
    // Not the Library's: the lock, and SQLite's files beside an open one.
    walk(&to, &mut |p| {
        let name = p.file_name().unwrap().to_string_lossy();
        if ["library.lock", "library-owner.json"].contains(&&*name)
            || name.ends_with("-wal")
            || name.ends_with("-shm")
        {
            std::fs::remove_file(p).unwrap();
        }
    });
    println!("wrote {}", to.display());
}

/// What `library-<version>` holds, as [`write_library_fixture`] wrote it.
fn check_library_fixture(dir: &Path) {
    let history = History::open(dir).unwrap();
    let renders = history.list().unwrap();
    assert_eq!(renders.len(), 3, "every render is listed");
    let (first, second, painted) = (&renders[0], &renders[1], &renders[2]);
    assert_eq!(first.stack, second.stack);
    assert_eq!((first.number, second.number, painted.number), (1, 2, 1));
    assert_eq!(first.name.as_deref(), Some("Bee"));
    assert!(first.picked && !second.picked);
    assert_eq!(first.batch.as_ref().map(|b| b.label), Some(auto_sort()));
    assert_eq!(first.frames, frames(dir, &["a.png", "b.png", "c.png"]));
    assert_eq!(first.included, [true, false, true]);
    for render in &renders {
        history.load_render(&render.id).expect("its pixels load");
        history.load_thumbnail(&render.id).expect("its thumbnail loads");
    }
    let stacks = history.stacks().unwrap();
    let bees = &stacks[&first.stack];
    assert_eq!(bees.name.as_deref(), Some("Bees"));
    assert_eq!(bees.last_number, 2);
    let failures = history.failures().unwrap();
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].error, "alignment failed");
    // Ants on screen, as left; Bees' working version kept, one frame out.
    let on_screen = history.read_working().unwrap();
    assert_eq!(on_screen.stack, painted.stack);
    assert_eq!(on_screen.base.as_ref(), Some(&painted.id));
    let back = history.switch_stack(&on_screen, &first.stack, &renders, None).unwrap();
    let back = back.expect("Bees opens");
    assert_eq!(back.base.as_ref(), Some(&second.id));
    assert_eq!(back.list.excluded.len(), 2, "its working version, as kept");
    // The paint.
    let render = history.render_path(&painted.id);
    let mut doc = store::load_doc(&render, PAINT_W, PAINT_H).unwrap().expect("the paint loads");
    let layer: Layer = doc.layer().expect("read, not released");
    let cols = PAINT_W.div_ceil(TILE) as usize;
    assert!(layer.tiles[cols + 1].as_deref() == Some(&painted_tile()));
    doc.close();
    // Nothing set aside.
    let mut aside = Vec::new();
    walk(dir, &mut |p| {
        if p.to_string_lossy().contains("unreadable") {
            aside.push(p.to_path_buf());
        }
    });
    assert!(aside.is_empty(), "{aside:?}");
}

fn walk(dir: &Path, each: &mut dyn FnMut(&Path)) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        each(&entry.path());
        if entry.file_type().unwrap().is_dir() {
            walk(&entry.path(), each);
        }
    }
}

/// Every later 0.x build opens a 0.2.0 Library whole, setting nothing aside.
#[test]
fn a_library_made_by_0_2_0_opens() {
    let dir = fixture(&format!("library-{LIBRARY_VERSION}"));
    check_library_fixture(dir.path());
}

/// A render of the paint fixture's size, of `frames`.
fn paint_base_of(frames: &[PathBuf]) -> CompletedStack {
    let pixels = vec![20000; (3 * PAINT_W * PAINT_H) as usize];
    CompletedStack::srgb(PAINT_W, PAINT_H, pixels, frames.to_vec())
}

fn paint_base(library: &Path) -> Arc<CompletedStack> {
    let pixels = vec![20000; (3 * PAINT_W * PAINT_H) as usize];
    let frames = frames(library, &["a.png", "b.png"]);
    Arc::new(CompletedStack::srgb(PAINT_W, PAINT_H, pixels, frames))
}

/// The one file of `dir`'s folder `in_dir` (a render's folder, a failure).
fn only_file(dir: &Path, in_dir: &str) -> PathBuf {
    let mut entries = std::fs::read_dir(dir.join(in_dir)).unwrap();
    entries.next().unwrap().unwrap().path()
}

/// Set the `schema` of the JSON file at `path` to `schema`.
fn set_schema(path: &Path, schema: u64) {
    let mut json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    json["schema"] = schema.into();
    std::fs::write(path, serde_json::to_vec_pretty(&json).unwrap()).unwrap();
}

#[test]
fn a_v1_render_loads() {
    let dir = fixture("render-v1");
    let history = History::open(dir.path()).unwrap();
    let renders = history.list().unwrap();
    assert_eq!(renders.len(), 1, "the render is listed");
    let render = &renders[0];
    assert_eq!(render.frames, frames(dir.path(), &["a.png", "b.png", "c.png"]));
    assert_eq!(render.included, [true, false, true]);
    assert_eq!(render.name.as_deref(), Some("Bee"));
    assert_eq!(render.number, 1);
    assert_eq!(render.batch.as_ref().map(|b| b.label), Some(auto_sort()));
    assert_eq!((render.width, render.height, render.linear), (W, H, false));

    let stack = history.load_render(&render.id).unwrap();
    let saved = render_stack(dir.path());
    assert!(stack.pixels == saved.pixels, "result samples changed");
    assert!(stack.frame_map == saved.frame_map, "the frame map is exact");
    assert_eq!(stack.paths, saved.paths);
    assert_eq!(stack.geometry, saved.geometry);
    assert_eq!(format!("{:?}", stack.config), format!("{:?}", saved.config));
    assert_eq!(format!("{:?}", stack.options), format!("{:?}", saved.options));
    assert_eq!(history.load_thumbnail(&render.id).unwrap().dimensions(), (W, H));

    // Its stack's file keeps the number it gave.
    let stacks = history.stacks().unwrap();
    assert_eq!(stacks[&render.stack].last_number, 1);
}

#[test]
fn a_v1_list_loads() {
    let dir = fixture("list-v1");
    let Working { list, settings, .. } = History::open(dir.path()).unwrap().read_working().unwrap();
    let frames_in: Vec<(PathBuf, bool)> = list
        .shots()
        .iter()
        .map(|s| (s.file(list.source).to_path_buf(), list.is_included(s)))
        .collect();
    let [a, b, c] = frames(dir.path(), &["a.png", "b.png", "c.png"]).try_into().unwrap();
    assert_eq!(frames_in, [(a, true), (b, false), (c, true)]);
    let Settings { config, options } = settings.expect("the live settings were saved");
    let pyramid = StackerConfig::Pyramid(PyramidConfig::default());
    assert_eq!(format!("{config:?}"), format!("{pyramid:?}"));
    assert_eq!(format!("{options:?}"), format!("{:?}", self::options()));
}

#[test]
fn a_v1_failure_loads() {
    let dir = fixture("failure-v1");
    let failures = History::open(dir.path()).unwrap().failures().unwrap();
    assert_eq!(failures.len(), 1, "the failure is listed");
    let failure = &failures[0];
    assert_eq!(failure.label, "Bee · 2");
    assert_eq!(failure.frames, frames(dir.path(), &["a.png", "b.png", "c.png"]));
    assert_eq!(failure.batch.label, auto_sort());
    assert_eq!(failure.error, "alignment failed");
    assert_eq!(format!("{:?}", failure.config), format!("{:?}", depth()));
    assert_eq!(format!("{:?}", failure.options), format!("{:?}", options()));
}

#[test]
fn v1_paint_loads() {
    let dir = fixture("paint-v1");
    let history = History::open(dir.path()).unwrap();
    let render = dir.path().join("render");
    let loaded = store::load_doc(&render, PAINT_W, PAINT_H).unwrap();
    let mut doc = loaded.expect("the paint loads");
    let source = SourceKind::Frame { path: frames(dir.path(), &["b.png"])[0].clone() };
    assert_eq!(doc.sources.kind(1, history.frames()), Some(source));
    let layer: Layer = doc.layer().expect("read, not released");
    let cols = PAINT_W.div_ceil(TILE) as usize;
    for (i, tile) in layer.tiles.iter().enumerate() {
        match i {
            i if i == cols + 1 => assert!(tile.as_deref() == Some(&painted_tile())),
            _ => assert!(tile.is_none(), "only tile (1, 1) is painted"),
        }
    }
    doc.close();
}

/// A render of another schema is not listed, left and not edited.
#[test]
fn a_render_of_another_schema_is_skipped_and_left() {
    for schema in [2, 0] {
        let dir = fixture("render-v1");
        let file = only_file(dir.path(), "Renders").join("render.json");
        set_schema(&file, schema);
        let before = std::fs::read(&file).unwrap();
        let history = History::open(dir.path()).unwrap();
        assert!(history.list().unwrap().is_empty(), "schema {schema}: not listed");
        let id =
            only_file(dir.path(), "Renders").file_name().unwrap().to_string_lossy().into_owned();
        let id = deptheos_app::library::history::RenderId::parse(&id).unwrap();
        assert!(history.rename_render(&id, Some("Ant")).is_err(), "schema {schema}: not edited");
        assert_eq!(std::fs::read(&file).unwrap(), before, "schema {schema}: left as it was");
    }
}

/// A list of another schema starts the working version empty; the next
/// save replaces it.
#[test]
fn a_list_of_another_schema_is_replaced() {
    for version in [2, 0] {
        let dir = fixture("list-v1");
        let file = dir.path().join("list.json");
        set_schema(&file, version);
        let history = History::open(dir.path()).unwrap();
        let working = history.read_working().unwrap();
        assert!(working.list.is_empty(), "schema {version}");
        let names = || -> Vec<String> {
            std::fs::read_dir(dir.path())
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        };
        assert!(!names().iter().any(|n| n.contains("unreadable")), "{:?}", names());
        history.write_working(&working).unwrap();
        assert_eq!(schema(&file), 1, "schema {version}: replaced");
    }
}

/// A failure of another schema is skipped and left as it is.
#[test]
fn a_failure_of_another_schema_is_skipped() {
    for schema in [2, 0] {
        let dir = fixture("failure-v1");
        let file = only_file(dir.path(), "Failed");
        set_schema(&file, schema);
        let failures = History::open(dir.path()).unwrap().failures().unwrap();
        assert!(failures.is_empty(), "schema {schema}");
        assert!(file.exists(), "schema {schema}");
    }
}

/// A stack file of another schema: the stack opens from its renders, and a
/// rename replaces the file.
#[test]
fn a_damaged_stack_file_opens_its_stack_from_its_renders() {
    let dir = fixture("render-v1");
    let file = only_file(dir.path(), "Stacks");
    set_schema(&file, 2);
    let history = History::open(dir.path()).unwrap();
    let renders = history.list().unwrap();
    let stack = renders[0].stack.clone();
    assert!(!history.stacks().unwrap().contains_key(&stack), "nothing read from it");
    let on_screen = history.read_working().unwrap();
    let opened = history.switch_stack(&on_screen, &stack, &renders, None).unwrap();
    assert_eq!(opened.map(|w| w.list.files), Some(renders[0].frames.clone()));
    history.rename_stack(&stack, Some("Ant")).unwrap();
    assert_eq!(schema(&file), 1, "replaced");
}

/// Hand-made from `library-0.2.0`: `library-newer` (`sources.json` at schema
/// 1000, by 0.9.0), `library-damaged` (a render's file cut short, a stack's
/// file not JSON) and `library-unknown-value` (a render's method, the list's
/// order).
const NEWER_VERSION: &str = "0.9.0";

/// A newer Library is refused whole, naming its last version, unchanged.
#[test]
fn a_newer_library_is_refused_untouched() {
    let dir = fixture("library-newer");
    let before = tree(dir.path());

    let Err(refused) = History::open(dir.path()) else { panic!("opened") };
    let deptheos_app::StoreError::NewerLibrary { written_by, .. } = &refused else {
        panic!("{refused:?}")
    };
    assert_eq!(written_by.as_deref(), Some(NEWER_VERSION));
    let words = deptheos_app::Words::english(deptheos_app::Platform::Desktop);
    let said = deptheos_app::Explain::explain(&refused, &words);
    assert!(said.reason.contains(&format!("last used by Deptheos {NEWER_VERSION}")), "{said:?}");
    assert!(said.advice.contains("latest version"), "{said:?}");
    assert_eq!(tree(dir.path()), before, "untouched");
}

/// A newer Library is refused before anything but the lock is written, even
/// without its `Renders` folder.
#[test]
fn a_newer_library_is_refused_before_anything_is_written() {
    let listing = |dir: &Path| {
        let mut names = Vec::new();
        walk(dir, &mut |p| names.push(p.strip_prefix(dir).unwrap().to_path_buf()));
        names.sort();
        names
    };
    for renders in [true, false] {
        let dir = fixture("library-newer");
        if !renders {
            std::fs::remove_dir_all(dir.path().join("Renders")).unwrap();
        }
        let mut expected = listing(dir.path());
        expected.push(PathBuf::from("library.lock"));
        expected.sort();

        let refused = History::open(dir.path()).err();
        assert!(
            matches!(refused, Some(deptheos_app::StoreError::NewerLibrary { .. })),
            "{refused:?}"
        );
        assert_eq!(listing(dir.path()), expected, "with Renders: {renders}");
    }
}

/// Damage doesn't stop a Library opening: a cut-short render is skipped and
/// left; a non-JSON stack file reads as none and a rename sets it aside and
/// rewrites it. Nothing else is set aside.
#[test]
fn a_damaged_file_in_a_library_is_damage_not_a_refusal() {
    let dir = fixture("library-damaged");
    let cut = dir.path().join("Renders/01a1f70e9c5f712b87ea7667ca143d1a/render.json");
    let kept = std::fs::read(&cut).unwrap();
    let stack_files: Vec<Vec<u8>> = std::fs::read_dir(dir.path().join("Stacks"))
        .unwrap()
        .map(|e| std::fs::read(e.unwrap().path()).unwrap())
        .collect();
    let history = History::open(dir.path()).unwrap();
    let renders = history.list().unwrap();
    assert_eq!(renders.len(), 2, "the one cut short skipped");
    assert_eq!(std::fs::read(&cut).unwrap(), kept, "left as it was");
    let bees = renders.iter().find(|r| r.number == 2).unwrap();
    let stacks = history.stacks().unwrap();
    assert!(!stacks.contains_key(&bees.stack), "its damaged file read as none");
    let on_screen = history.read_working().unwrap();
    let opened = history.switch_stack(&on_screen, &bees.stack, &renders, None).unwrap();
    assert_eq!(opened.map(|w| w.base), Some(Some(bees.id.clone())), "from its newest render");
    history.rename_stack(&bees.stack, Some("Bees")).unwrap();
    assert_eq!(history.stacks().unwrap()[&bees.stack].name.as_deref(), Some("Bees"));
    let mut aside = Vec::new();
    walk(dir.path(), &mut |p| {
        if p.to_string_lossy().contains("unreadable") {
            aside.push(p.to_path_buf());
        }
    });
    assert_eq!(aside.len(), 1, "{aside:?}");
    let name = aside[0].file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.starts_with(&format!("{}.unreadable-", bees.stack.as_str())), "{name}");
    assert_eq!(aside[0].parent().unwrap(), dir.path().join("Stacks"));
    assert!(stack_files.contains(&std::fs::read(&aside[0]).unwrap()), "kept byte for byte");
}

/// An unknown value is damage: such a render is skipped and left, such a
/// list starts the working version empty. The rest opens.
#[test]
fn an_unknown_value_is_damage() {
    let dir = fixture("library-unknown-value");
    let unknown = dir.path().join("Renders/01a1f70e9ca0721090f61a4df5fb2917/render.json");
    let kept = std::fs::read(&unknown).unwrap();
    let history = History::open(dir.path()).unwrap();
    let renders = history.list().unwrap();
    let numbers: Vec<u32> = renders.iter().map(|r| r.number).collect();
    assert_eq!(numbers, [1, 1], "the unknown method's render skipped");
    assert_eq!(std::fs::read(&unknown).unwrap(), kept, "left as it was");
    let working = history.read_working().unwrap();
    assert!(working.list.is_empty() && working.base.is_none(), "an unknown order");
    history.write_working(&working).unwrap();
    assert_eq!(schema(&dir.path().join("list.json")), 1, "replaced");
}

/// Every file under `dir` but the lock's, with its bytes.
fn tree(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    walk(dir, &mut |p| {
        let name = p.file_name().unwrap().to_string_lossy();
        if p.is_file() && name != "library.lock" && name != "library-owner.json" {
            files.push((p.to_path_buf(), std::fs::read(p).unwrap()));
        }
    });
    files.sort();
    files
}

/// Opening a Library records this version in `sources.json` for older builds.
#[test]
fn an_open_says_which_version_last_used_the_library() {
    let dir = fixture(&format!("library-{LIBRARY_VERSION}"));
    let sources = dir.path().join("sources.json");
    let read = |p: &Path| -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap()
    };
    assert!(read(&sources).get("written_by").is_none(), "0.2.0 wrote none");
    drop(History::open(dir.path()).unwrap());
    assert_eq!(read(&sources)["written_by"], env!("CARGO_PKG_VERSION"));
    assert_eq!(read(&sources)["schema"], 2, "moved to the format of Recently Deleted");
    // A new Library has one from its first open.
    let new = tempfile::tempdir().unwrap();
    drop(History::open(new.path()).unwrap());
    assert_eq!(read(&new.path().join("sources.json"))["written_by"], env!("CARGO_PKG_VERSION"));
}
