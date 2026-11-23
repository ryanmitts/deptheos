//! Grow sequences: a render replayed with snapshots into numbered frames,
//! the last the render itself; and what a grow or turntable refuses up front.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use deptheos_app::runs::sequence::{
    self, Direction, FrameFormat, FrameOptions, GrowOptions, MANIFEST_FILE, StackStore,
    TurntableMode, TurntableOptions, TurntableStack, replay_order,
};
use deptheos_app::{
    Alignment, CompletedStack, Engine, LeftOut, SaveOptions, SessionOptions, StackGeometry,
    StackerConfig, StackingSession,
};
use deptheos_core::pyramid::PyramidConfig;
use deptheos_core::{Affine2, CommonPipelines, FrameAlignment};
use deptheos_gpu::GpuContext;
use image::math::Rect;
use image::{ImageBuffer, Rgb};

const SIZE: u32 = 192;
const FRAMES: u32 = 5;

/// `FRAMES` PNGs of one checkerboard, frame f sharp in vertical band f.
fn write_frames(dir: &Path) -> Vec<PathBuf> {
    let sharp: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_fn(SIZE, SIZE, |x, y| {
        if ((x / 6) + (y / 6)) % 2 == 0 { Rgb([220, 180, 60]) } else { Rgb([30, 60, 120]) }
    });
    let blurred = image::imageops::blur(&sharp, 3.0);
    let band = SIZE / FRAMES;
    (0..FRAMES)
        .map(|f| {
            let frame = ImageBuffer::from_fn(SIZE, SIZE, |x, y| {
                if x / band == f { *sharp.get_pixel(x, y) } else { *blurred.get_pixel(x, y) }
            });
            let path = dir.join(format!("f{}.png", f + 1));
            frame.save(&path).unwrap();
            path
        })
        .collect()
}

fn geometry(reversed: bool, left_out: Option<usize>) -> StackGeometry {
    let transforms = (0..FRAMES as usize)
        .map(|i| {
            if left_out == Some(i) {
                return None;
            }
            let f = i as f32;
            #[rustfmt::skip]
            let m = nalgebra::Matrix3::new(
                1.0 - 0.003 * f, 0.0, 0.6 * f,
                0.0, 1.0 - 0.003 * f, -0.3 * f,
                0.0, 0.0, 1.0,
            );
            Some(FrameAlignment::new(Affine2::from_matrix_unchecked(m), 1.0))
        })
        .collect();
    StackGeometry {
        transforms,
        crop: Some(Rect { x: 3, y: 5, width: SIZE - 9, height: SIZE - 11 }),
        reversed,
        left_out: left_out
            .map(|index| vec![LeftOut { index, reason: "unreadable".into() }])
            .unwrap_or_default(),
    }
}

fn options() -> SessionOptions {
    SessionOptions {
        alignment: Alignment::Off { resumes: Default::default() },
        ..Default::default()
    }
}

fn stack_of(
    ctx: &Arc<GpuContext>,
    common: &Arc<CommonPipelines>,
    paths: Vec<PathBuf>,
    geometry: StackGeometry,
) -> CompletedStack {
    let config = StackerConfig::Pyramid(PyramidConfig::default());
    StackingSession::new(
        ctx.clone(),
        common.clone(),
        paths,
        config,
        options(),
        deptheos_app::Words::english(deptheos_app::Platform::Desktop),
    )
    .unwrap()
    .with_alignment(geometry)
    .unwrap()
    .run(&mut |_, _| {}, &deptheos_app::Cancel::root())
    .unwrap()
    .stack
}

fn read(path: &Path) -> image::RgbImage {
    image::open(path).unwrap().to_rgb8()
}

fn to_rgb8(stack: &CompletedStack) -> image::RgbImage {
    let bytes = deptheos_core::io::dither_rgb_to_u8(&stack.pixels, stack.width);
    image::RgbImage::from_raw(stack.width, stack.height, bytes).unwrap()
}

fn max_diff(a: &image::RgbImage, b: &image::RgbImage) -> u8 {
    assert_eq!(a.dimensions(), b.dimensions());
    a.as_raw().iter().zip(b.as_raw()).map(|(x, y)| x.abs_diff(*y)).max().unwrap()
}

/// Grow `render` into `out` and check every frame: the last is the render
/// exactly, frame k the separate stack of the first k frames replayed.
fn check_grow(reversed: bool, left_out: Option<usize>, direction: Direction) {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let common = Arc::new(CommonPipelines::new(&ctx));
    let dir = tempfile::tempdir().unwrap();
    let paths = write_frames(dir.path());
    let geometry = geometry(reversed, left_out);
    let render = stack_of(&ctx, &common, paths.clone(), geometry.clone());

    let out = dir.path().join("out");
    let options = GrowOptions {
        direction,
        every: 1,
        frames: FrameOptions {
            format: FrameFormat::Png,
            save: SaveOptions::default(),
            long_edge: None,
        },
    };
    let engine = Engine::with_gpu(
        ctx.clone(),
        deptheos_app::Words::english(deptheos_app::Platform::Desktop),
    );
    let mut lines = Vec::new();
    let report = sequence::export_grow(
        &engine,
        &render,
        Some("0123"),
        "bee",
        &out,
        &options,
        &mut |p| {
            lines.push(p.message(&deptheos_app::Words::english(deptheos_app::Platform::Desktop)))
        },
        &deptheos_app::Cancel::root(),
    )
    .unwrap();
    let n = FRAMES as usize - left_out.is_some() as usize;
    assert_eq!(report.frames, n);
    assert_eq!(report.folder, out.join("bee grow"));
    assert_eq!(lines.last().unwrap(), &format!("Grow sequence · frame {n} of {n} written"));

    let mut names: Vec<String> = std::fs::read_dir(&report.folder)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    let mut want: Vec<String> = (1..=n).map(|i| format!("bee grow_{i:04}.png")).collect();
    want.push(MANIFEST_FILE.into());
    assert_eq!(names, want);
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(report.folder.join(MANIFEST_FILE)).unwrap()).unwrap();
    assert_eq!(manifest["complete"], true);
    assert_eq!(manifest["frames"].as_array().unwrap().len(), n);
    assert_eq!(manifest["frames"][n - 1]["kind"], "full");
    assert_eq!(manifest["left_out"].as_array().map_or(0, Vec::len), left_out.is_some() as usize);

    // The last frame is the render.
    let last = read(&report.folder.join(format!("bee grow_{n:04}.png")));
    assert_eq!(max_diff(&last, &to_rgb8(&render)), 0, "the last frame is the render");

    // Frame k is the stack of the replay's first k frames.
    let replay = replay_order(&paths, &geometry, direction);
    for k in 1..n {
        let part = replay.first(k);
        let separate = stack_of(&ctx, &common, part.paths, part.geometry);
        let frame = read(&report.folder.join(format!("bee grow_{k:04}.png")));
        let diff = max_diff(&frame, &to_rgb8(&separate));
        assert!(diff <= 1, "k {k}: max diff {diff}");
    }
}

#[test]
fn a_grow_back_to_front_ends_on_the_render() {
    check_grow(false, None, Direction::BackToFront);
}

#[test]
fn a_grow_of_a_reversed_render_front_to_back() {
    check_grow(true, None, Direction::FrontToBack);
}

/// A grow whose replay can't start fails with a manifest saying incomplete,
/// never an empty folder.
#[test]
fn a_grow_that_cannot_start_leaves_a_manifest() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let common = Arc::new(CommonPipelines::new(&ctx));
    let dir = tempfile::tempdir().unwrap();
    let paths = write_frames(dir.path());
    let render = stack_of(&ctx, &common, paths.clone(), geometry(false, None));
    for path in &paths {
        std::fs::write(path, b"not an image any more").unwrap();
    }
    let out = dir.path().join("out");
    let options = GrowOptions {
        direction: Direction::BackToFront,
        every: 1,
        frames: FrameOptions { format: FrameFormat::Png, ..FrameOptions::default() },
    };
    let words = deptheos_app::Words::english(deptheos_app::Platform::Desktop);
    let engine = Engine::with_gpu(ctx.clone(), words);
    let cancel = deptheos_app::Cancel::root();
    let ended =
        sequence::export_grow(&engine, &render, None, "bee", &out, &options, &mut |_| {}, &cancel);
    assert!(ended.is_err());
    let folder = out.join("bee grow");
    let names: Vec<String> = std::fs::read_dir(&folder)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, [MANIFEST_FILE]);
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(folder.join(MANIFEST_FILE)).unwrap()).unwrap();
    assert_eq!(manifest["complete"], false);
}

#[test]
fn a_grow_skips_left_out_frames() {
    check_grow(false, Some(2), Direction::BackToFront);
}

/// Snapshots are numbered by the frame they follow, so a left-out frame keeps
/// its place.
#[test]
fn a_grow_numbers_its_frames_by_frame_when_the_replay_leaves_one_out() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let common = Arc::new(CommonPipelines::new(&ctx));
    let dir = tempfile::tempdir().unwrap();
    let paths = write_frames(dir.path());
    let render = stack_of(&ctx, &common, paths.clone(), geometry(false, None));
    // f4, the replay's second frame back to front: its header reads, its
    // pixels don't.
    let bytes = std::fs::read(&paths[3]).unwrap();
    std::fs::write(&paths[3], &bytes[..200]).unwrap();
    let out = dir.path().join("out");
    let options = GrowOptions {
        direction: Direction::BackToFront,
        every: 1,
        frames: FrameOptions { format: FrameFormat::Png, ..FrameOptions::default() },
    };
    let words = deptheos_app::Words::english(deptheos_app::Platform::Desktop);
    let engine = Engine::with_gpu(ctx.clone(), words);
    let cancel = deptheos_app::Cancel::root();
    let report =
        sequence::export_grow(&engine, &render, None, "bee", &out, &options, &mut |_| {}, &cancel)
            .unwrap();
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(report.folder.join(MANIFEST_FILE)).unwrap()).unwrap();
    let frames = manifest["frames"].as_array().unwrap();
    let ks: Vec<u64> = frames.iter().map(|f| f["k"].as_u64().unwrap()).collect();
    assert_eq!(ks, [1, 2, 3, 4, 5]);
    let to: Vec<&str> = frames.iter().map(|f| f["to"].as_str().unwrap()).collect();
    assert_eq!(to, ["f5.png", "f4.png", "f3.png", "f2.png", "f1.png"]);
    let left_out = manifest["left_out"].as_array().unwrap();
    assert_eq!(left_out.len(), 1, "{left_out:?}");
    assert_eq!(left_out[0]["what"], "f4.png");
}

/// A grow that won't fit on its volume is refused before writing.
#[test]
fn a_grow_that_wont_fit_is_refused_before_it_writes() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let common = Arc::new(CommonPipelines::new(&ctx));
    let dir = tempfile::tempdir().unwrap();
    let paths = write_frames(dir.path());
    let mut render = stack_of(&ctx, &common, paths.clone(), geometry(false, None));
    // Frames of a million pixels square: terabytes each.
    (render.width, render.height) = (1 << 20, 1 << 20);
    let out = dir.path().join("out");
    let options = GrowOptions {
        direction: Direction::BackToFront,
        every: 1,
        frames: FrameOptions { format: FrameFormat::Png, ..FrameOptions::default() },
    };
    let words = deptheos_app::Words::english(deptheos_app::Platform::Desktop);
    let engine = Engine::with_gpu(ctx.clone(), words);
    let cancel = deptheos_app::Cancel::root();
    let refused =
        sequence::export_grow(&engine, &render, None, "bee", &out, &options, &mut |_| {}, &cancel);
    assert!(matches!(refused, Err(deptheos_app::Error::NoSpace { .. })), "{refused:?}");
    assert!(!out.exists(), "nothing written");
}

/// A turntable grow numbers snapshots the same way; the deepest is the full
/// stack.
#[test]
fn a_turntable_grow_numbers_its_frames_by_frame_when_the_replay_leaves_one_out() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let common = Arc::new(CommonPipelines::new(&ctx));
    let dir = tempfile::tempdir().unwrap();
    let paths = write_frames(dir.path());
    let render = stack_of(&ctx, &common, paths.clone(), geometry(false, None));
    // f4, the replay's second frame back to front: its header reads, its
    // pixels don't.
    let bytes = std::fs::read(&paths[3]).unwrap();
    std::fs::write(&paths[3], &bytes[..200]).unwrap();
    struct Store(CompletedStack);
    impl StackStore for Store {
        fn load(&mut self, _: usize) -> deptheos_app::Result<&CompletedStack> {
            Ok(&self.0)
        }
        fn load_settings(&mut self, _: usize) -> deptheos_app::Result<&CompletedStack> {
            Ok(&self.0)
        }
    }
    let stacks = [TurntableStack {
        label: "s001".into(),
        render: None,
        paths: paths.clone(),
        geometry: geometry(false, None),
    }];
    let options = TurntableOptions {
        mode: TurntableMode::Grow,
        direction: Direction::BackToFront,
        frames: FrameOptions { format: FrameFormat::Png, ..FrameOptions::default() },
        ..Default::default()
    };
    let words = deptheos_app::Words::english(deptheos_app::Platform::Desktop);
    let engine = Engine::with_gpu(ctx.clone(), words);
    let out = dir.path().join("out");
    let reports = sequence::export_turntable(
        &engine,
        &stacks,
        &mut Store(render),
        "bee",
        &out,
        &options,
        &[],
        &[],
        &mut |_| {},
        &deptheos_app::Cancel::root(),
    )
    .unwrap();
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(reports[0].folder.join(MANIFEST_FILE)).unwrap())
            .unwrap();
    let frames = manifest["frames"].as_array().unwrap();
    let ks: Vec<u64> = frames.iter().map(|f| f["k"].as_u64().unwrap()).collect();
    assert_eq!(ks, [1, 2, 3, 4, 5]);
    assert!(frames.iter().all(|f| f["of"] == 5), "{frames:?}");
    let kinds: Vec<&str> = frames.iter().map(|f| f["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["partial", "partial", "partial", "partial", "full"]);
    let to: Vec<&str> = frames.iter().map(|f| f["to"].as_str().unwrap()).collect();
    assert_eq!(to, ["f5.png", "f4.png", "f3.png", "f2.png", "f1.png"]);
    let left_out = manifest["left_out"].as_array().unwrap();
    assert_eq!(left_out.len(), 1, "{left_out:?}");
    assert_eq!(left_out[0]["what"], "s001/f4.png");
}

/// A grow or turntable of mixed frame sizes is refused up front, naming the
/// frame.
#[test]
fn frames_of_another_size_are_refused_up_front() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let common = Arc::new(CommonPipelines::new(&ctx));
    let dir = tempfile::tempdir().unwrap();
    let paths = write_frames(dir.path());
    let mut render = stack_of(&ctx, &common, paths.clone(), geometry(false, None));
    let small = dir.path().join("small.png");
    ImageBuffer::from_pixel(SIZE / 2, SIZE / 2, Rgb([90u8, 90, 90])).save(&small).unwrap();
    let engine = Engine::with_gpu(
        ctx.clone(),
        deptheos_app::Words::english(deptheos_app::Platform::Desktop),
    );
    let frames = FrameOptions { format: FrameFormat::Png, ..FrameOptions::default() };
    let out = dir.path().join("out");
    let refused = |e: deptheos_app::Error| {
        let deptheos_app::Error::FrameSize { file, size, first } = e else { panic!("{e}") };
        assert_eq!((file.as_str(), size, first), ("small.png", (96, 96), (192, 192)));
        assert!(!out.exists(), "nothing written");
    };
    let cancel = deptheos_app::Cancel::root();

    // A grow whose frame was replaced since the render.
    let frame = std::mem::replace(&mut render.paths[3], small.clone());
    let options = GrowOptions { direction: Direction::FrontToBack, every: 1, frames };
    refused(
        sequence::export_grow(&engine, &render, None, "bee", &out, &options, &mut |_| {}, &cancel)
            .unwrap_err(),
    );
    render.paths[3] = frame;

    // A turntable whose second stack is of smaller frames.
    struct Store(CompletedStack);
    impl StackStore for Store {
        fn load(&mut self, _: usize) -> deptheos_app::Result<&CompletedStack> {
            Ok(&self.0)
        }
        fn load_settings(&mut self, _: usize) -> deptheos_app::Result<&CompletedStack> {
            Ok(&self.0)
        }
    }
    let stack = |label: &str, paths: Vec<PathBuf>| TurntableStack {
        label: label.into(),
        render: None,
        paths,
        geometry: geometry(false, None),
    };
    let stacks = [stack("s001", paths.clone()), stack("s002", vec![small.clone(); paths.len()])];
    let options = TurntableOptions { mode: TurntableMode::Both, frames, ..Default::default() };
    let ended = sequence::export_turntable(
        &engine,
        &stacks,
        &mut Store(render),
        "bee",
        &out,
        &options,
        &[],
        &[],
        &mut |_| {},
        &cancel,
    );
    refused(ended.unwrap_err());
}
