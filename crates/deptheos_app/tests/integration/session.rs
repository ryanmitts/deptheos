//! Session runs: events, the completed stack and its textures, cancel,
//! saving, and aligned runs (crop, reverse check, replay).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use deptheos_app::library::history::Made;
use deptheos_app::{
    Adjustments, AlignMode, AlignSettings, Alignment, FrameKind, GridConfig, Look, SaveOptions,
    SessionEvent, SessionOptions, SessionOutput, StackerConfig, StackingSession,
};
use deptheos_core::CommonPipelines;
use deptheos_core::depth::DepthConfig;
use deptheos_core::pyramid::PyramidConfig;
use deptheos_core::wavelet::WaveletConfig;
use deptheos_gpu::GpuContext;
use image::{ImageBuffer, Rgb};

const SIZE: u32 = 256;

/// Two half-blurred checkerboard frames written as PNGs (the sRGB pipeline).
fn write_frames(dir: &std::path::Path) -> Vec<PathBuf> {
    let sharp: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_fn(SIZE, SIZE, |x, y| {
        if ((x / 8) + (y / 8)) % 2 == 0 { Rgb([220, 180, 60]) } else { Rgb([30, 60, 120]) }
    });
    let blur_half = |left: bool| -> ImageBuffer<Rgb<u8>, Vec<u8>> {
        let radius: i64 = 5;
        ImageBuffer::from_fn(SIZE, SIZE, |x, y| {
            let in_blur = if left { x < SIZE / 2 } else { x >= SIZE / 2 };
            if !in_blur {
                return *sharp.get_pixel(x, y);
            }
            let mut sum = [0u64; 3];
            let mut count = 0u64;
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    let sx = (x as i64 + dx).clamp(0, SIZE as i64 - 1) as u32;
                    let sy = (y as i64 + dy).clamp(0, SIZE as i64 - 1) as u32;
                    let p = sharp.get_pixel(sx, sy);
                    for c in 0..3 {
                        sum[c] += p[c] as u64;
                    }
                    count += 1;
                }
            }
            Rgb([(sum[0] / count) as u8, (sum[1] / count) as u8, (sum[2] / count) as u8])
        })
    };
    let a = dir.join("frame_a.png");
    let b = dir.join("frame_b.png");
    blur_half(true).save(&a).unwrap();
    blur_half(false).save(&b).unwrap();
    vec![a, b]
}

fn common(ctx: &GpuContext) -> Arc<CommonPipelines> {
    Arc::new(CommonPipelines::new(ctx))
}

fn run_session(
    ctx: Arc<GpuContext>,
    paths: Vec<PathBuf>,
    config: StackerConfig,
    cancel_after: Option<u32>,
) -> (Vec<SessionEvent>, Result<SessionOutput, deptheos_app::Error>) {
    // No alignment: ECC finds false basins in periodic checkerboards.
    let options = SessionOptions {
        alignment: Alignment::Off { resumes: Default::default() },
        ..Default::default()
    };
    let session = StackingSession::new(
        ctx.clone(),
        common(&ctx),
        paths,
        config,
        options,
        deptheos_app::Words::english(deptheos_app::Platform::Desktop),
    )
    .unwrap();

    let mut events = Vec::new();
    let cancel = deptheos_app::Cancel::root();
    let result = session.run(
        &mut |e, _| {
            if let (Some(after), SessionEvent::Accumulated { current, .. }) = (cancel_after, e)
                && current >= after
            {
                cancel.cancel();
            }
            events.push(e);
        },
        &cancel,
    );
    (events, result)
}

#[test]
fn pyramid_event_sequence_and_save() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let dir = tempfile::tempdir().unwrap();
    let paths = write_frames(dir.path());

    let (events, result) = run_session(
        ctx.clone(),
        paths.clone(),
        StackerConfig::Pyramid(PyramidConfig::default()),
        None,
    );
    let stack = result.unwrap().stack;

    assert_eq!(
        events,
        vec![
            SessionEvent::Started { total: 2 },
            SessionEvent::Accumulated { current: 1, total: 2 },
            SessionEvent::Accumulated { current: 2, total: 2 },
            SessionEvent::Finalized,
            SessionEvent::Completed,
        ]
    );

    assert_eq!((stack.width, stack.height), (SIZE, SIZE));
    assert_eq!(stack.pixels.len(), (SIZE * SIZE * 3) as usize);
    assert!(!stack.is_linear());
    assert_eq!(stack.paths, paths);

    let out = dir.path().join("result.png");
    deptheos_app::test_support::engine()
        .unwrap()
        .save(Look::exported(&stack, Some(&Adjustments::NEUTRAL)), &out, SaveOptions::default())
        .unwrap();
    let saved = image::open(&out).unwrap();
    assert_eq!((saved.width(), saved.height()), (SIZE, SIZE));

    // DNG save must refuse: sources were PNG, no metadata to carry.
    let dng = dir.path().join("result.dng");
    assert!(
        deptheos_app::test_support::engine()
            .unwrap()
            .save(Look::exported(&stack, Some(&Adjustments::NEUTRAL)), &dng, SaveOptions::default())
            .is_err()
    );
}

#[test]
fn depth_emits_blend_events() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let dir = tempfile::tempdir().unwrap();
    let paths = write_frames(dir.path());

    let (events, result) =
        run_session(ctx, paths, StackerConfig::Depth(DepthConfig::default()), None);
    result.unwrap();

    assert_eq!(
        events,
        vec![
            SessionEvent::Started { total: 2 },
            SessionEvent::Accumulated { current: 1, total: 2 },
            SessionEvent::Accumulated { current: 2, total: 2 },
            SessionEvent::Finalized,
            SessionEvent::Blended { current: 1, total: 2 },
            SessionEvent::Blended { current: 2, total: 2 },
            SessionEvent::Completed,
        ]
    );
}

#[test]
fn cancel_stops_between_frames() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let dir = tempfile::tempdir().unwrap();
    let paths = write_frames(dir.path());

    let (events, result) =
        run_session(ctx, paths, StackerConfig::Pyramid(PyramidConfig::default()), Some(1));
    assert!(matches!(result, Err(deptheos_app::Error::Canceled)));
    // Ran the first frame, then stopped before the second.
    assert_eq!(
        events,
        vec![
            SessionEvent::Started { total: 2 },
            SessionEvent::Accumulated { current: 1, total: 2 },
        ]
    );
}

/// A stack short of memory reads fewer frames ahead with the same pixels, or
/// is refused with what it needs; the budget counts CPU decodes too (iOS).
#[test]
fn a_stack_that_barely_fits_reads_fewer_frames_ahead() {
    use deptheos_core::CoreError;
    use deptheos_gpu::{GpuError, MemoryBudget, MemoryPool};

    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let budgeted = |usable: u64| match GpuContext::new() {
        Ok(ctx) => {
            Arc::new(ctx.with_memory_budget(MemoryBudget { usable, pool: MemoryPool::Shared }))
        }
        Err(e) => panic!("GPU init failed: {e}"),
    };
    let dir = tempfile::tempdir().unwrap();
    let paths = write_frames(dir.path());
    for config in [
        StackerConfig::Pyramid(PyramidConfig::default()),
        StackerConfig::Depth(DepthConfig::default()),
    ] {
        let roomy = run_session(ctx.clone(), paths.clone(), config, None).1.unwrap().stack;
        // A budget of nothing is refused with the least it needs (one ahead).
        let least = match run_session(budgeted(1), paths.clone(), config, None).1 {
            Err(deptheos_app::Error::Core(CoreError::Gpu(GpuError::OverBudget {
                needed,
                budget,
                ..
            }))) => {
                assert_eq!(budget, 1);
                needed
            }
            other => panic!("expected OverBudget, got {:?}", other.err()),
        };
        // Exactly that fits, reading fewer ahead, with the same pixels.
        let (events, tight) = run_session(budgeted(least), paths.clone(), config, None);
        assert_eq!(tight.unwrap().stack.pixels, roomy.pixels);
        assert_eq!(events.last(), Some(&SessionEvent::Completed));
        // A byte less doesn't, and says what it needs and has.
        match run_session(budgeted(least - 1), paths.clone(), config, None).1 {
            Err(deptheos_app::Error::Core(CoreError::Gpu(GpuError::OverBudget {
                needed,
                budget,
                ..
            }))) => assert_eq!((needed, budget), (least, least - 1)),
            other => panic!("expected OverBudget, got {:?}", other.err()),
        }
    }
}

/// A stack with no header to size it by reads the fewest frames ahead and is
/// checked at its reference frame.
#[test]
fn an_unsized_stack_that_barely_fits_reads_one_frame_ahead() {
    use deptheos_core::CoreError;
    use deptheos_gpu::{GpuError, MemoryBudget, MemoryPool};

    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let budgeted = |usable: u64| match GpuContext::new() {
        Ok(ctx) => {
            Arc::new(ctx.with_memory_budget(MemoryBudget { usable, pool: MemoryPool::Shared }))
        }
        Err(e) => panic!("GPU init failed: {e}"),
    };
    let refused = |result: Result<SessionOutput, deptheos_app::Error>| match result {
        Err(deptheos_app::Error::Core(CoreError::Gpu(GpuError::OverBudget { needed, .. }))) => {
            needed
        }
        other => panic!("expected OverBudget, got {:?}", other.err()),
    };
    let dir = tempfile::tempdir().unwrap();
    let paths = ten_frames(dir.path(), &[(0, true)]);
    let config = StackerConfig::Pyramid(PyramidConfig::default());
    let roomy = run_session(ctx, paths.clone(), config, None).1.unwrap().stack;
    // A frame with no header counts as 16-bit: sized as 16-bit frames, one ahead.
    let deep = tempfile::tempdir().unwrap();
    let sixteen: Vec<PathBuf> = paths[1..]
        .iter()
        .map(|p| {
            let path = deep.path().join(p.file_name().unwrap());
            image::open(p).unwrap().into_rgb16().save(&path).unwrap();
            path
        })
        .collect();
    let least = refused(run_session(budgeted(1), sixteen, config, None).1);
    let (events, tight) = run_session(budgeted(least), paths.clone(), config, None);
    let tight = tight.unwrap().stack;
    assert_eq!(tight.pixels, roomy.pixels);
    assert_eq!(tight.geometry.left_out.len(), 1);
    assert_eq!(events.last(), Some(&SessionEvent::Completed));
    // A byte less is refused at the reference, with what it needs then.
    assert_eq!(refused(run_session(budgeted(least - 1), paths, config, None).1), least);
}

/// A cancel kills a pending conversion rather than wait for it.
#[cfg(unix)]
#[test]
fn cancel_kills_the_conversion_a_session_waits_on() {
    use std::os::unix::fs::PermissionsExt as _;

    use deptheos_app::{DngCache, RawBackend, StackConversions};

    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let dir = tempfile::tempdir().unwrap();
    // A converter that never finishes.
    let exe = dir.path().join("converter.sh");
    std::fs::write(&exe, "#!/bin/sh\nexec sleep 60\n").unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    let paths: Vec<PathBuf> = ["a.cr3", "b.cr3"].iter().map(|n| dir.path().join(n)).collect();
    for path in &paths {
        std::fs::write(path, "raw").unwrap();
    }
    let cache = Arc::new(DngCache::new(dir.path().join("cache"), Some(exe)).unwrap());
    let conversions = StackConversions::start(cache, paths.clone(), RawBackend::AdobeDng, 1);
    let options = SessionOptions { raw_backend: RawBackend::AdobeDng, ..Default::default() };
    let common = common(&ctx);
    let session = StackingSession::new(
        ctx,
        common,
        paths,
        StackerConfig::Pyramid(PyramidConfig::default()),
        options,
        deptheos_app::Words::english(deptheos_app::Platform::Desktop),
    )
    .unwrap()
    .with_frame_files(Arc::new(conversions));

    let cancel = deptheos_app::Cancel::root();
    let start = std::time::Instant::now();
    let result = std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(200));
            cancel.cancel();
        });
        session.run(&mut |_, _| {}, &cancel)
    });
    assert!(matches!(result, Err(deptheos_app::Error::Canceled)), "{:?}", result.err());
    assert!(start.elapsed() < std::time::Duration::from_secs(5), "took {:?}", start.elapsed());
}

#[test]
fn output_carries_display_and_frame_map() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let dir = tempfile::tempdir().unwrap();
    let paths = write_frames(dir.path());

    let session = StackingSession::new(
        ctx.clone(),
        common(&ctx),
        paths,
        StackerConfig::Pyramid(PyramidConfig::default()),
        SessionOptions {
            alignment: Alignment::Off { resumes: Default::default() },
            ..Default::default()
        },
        deptheos_app::Words::english(deptheos_app::Platform::Desktop),
    )
    .unwrap();
    let output = session.run(&mut |_, _| {}, &deptheos_app::Cancel::root()).unwrap();

    assert!(output.display.is_none(), "an sRGB stack displays its own pixels");
    let frame_map = output.stack.frame_map.expect("frame map kept on the stack");
    assert_eq!(frame_map.dimensions(), (SIZE, SIZE));
}

/// Without Rgba16Unorm storage every method stacks in Rgba32Float within a
/// code of 65535 of the 16-bit path (the context fakes the format support).
#[test]
fn the_float_path_stacks_as_the_16_bit_path_does() {
    use deptheos_dng::test_dng::{Layout, TestDng};

    let Some(sixteen) = GpuContext::new_or_skip() else { return };
    if !sixteen.format_support.rgba16unorm_storage {
        return eprintln!("skipping: this GPU has only the float path");
    }
    let mut float = GpuContext::new().unwrap();
    float.format_support.rgba16unorm_storage = false;
    let float = Arc::new(float);
    assert_eq!(float.source_format(), wgpu::TextureFormat::Rgba32Float);
    assert!(common(&float).packer.is_none(), "no packer on the float path");

    let dir = tempfile::tempdir().unwrap();
    let srgb = write_frames(dir.path());
    let (w, h) = (96, 64);
    let dng = TestDng {
        width: w,
        height: h,
        cfa: false,
        big_endian: false,
        layout: Layout::Strips { rows: 16 },
        black: [0; 3],
        white: [65535; 3],
        active: [0, 0, h, w],
        crop: [0, 0, w, h],
    };
    let linear: Vec<PathBuf> = (0..3u16)
        .map(|i| {
            let path = dir.path().join(format!("{i}.dng"));
            let pixels: Vec<u16> =
                dng.pixels().iter().map(|&v| v.wrapping_mul(i + 1) ^ (i * 977)).collect();
            dng.write(&path, &pixels);
            path
        })
        .collect();

    let configs = [
        StackerConfig::Pyramid(PyramidConfig::default()),
        StackerConfig::Depth(DepthConfig::default()),
        StackerConfig::Wavelet(WaveletConfig::default()),
    ];
    for config in configs {
        for paths in [&srgb, &linear] {
            let stack = |ctx: &Arc<GpuContext>| {
                run_session(ctx.clone(), paths.clone(), config, None).1.unwrap().stack
            };
            let (a, b) = (stack(&sixteen), stack(&float));
            let label = format!("{:?}, {} frames", config.kind(), paths.len());
            assert_eq!((a.width, a.height, a.is_linear()), (b.width, b.height, b.is_linear()));
            assert_eq!(a.pixels.len(), b.pixels.len(), "{label}");
            let max = a.pixels.iter().zip(&b.pixels).map(|(x, y)| x.abs_diff(*y)).max();
            assert!(max <= Some(1), "{label}: samples {max:?} apart");
        }
    }
}

/// A linear stack's display is rendered only when asked for (the CLI never is).
#[test]
fn a_linear_display_renders_on_request() {
    use deptheos_dng::test_dng::{Layout, TestDng};

    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let (w, h) = (64, 48);
    let dng = TestDng {
        width: w,
        height: h,
        cfa: false,
        big_endian: false,
        layout: Layout::Strips { rows: 16 },
        black: [0; 3],
        white: [65535; 3],
        active: [0, 0, h, w],
        crop: [0, 0, w, h],
    };
    let dir = tempfile::tempdir().unwrap();
    let paths: Vec<PathBuf> = (0..2).map(|i| dir.path().join(format!("{i}.dng"))).collect();
    for path in &paths {
        dng.write(path, &dng.pixels());
    }
    let session = StackingSession::new(
        ctx.clone(),
        common(&ctx),
        paths,
        StackerConfig::Pyramid(PyramidConfig::default()),
        SessionOptions {
            alignment: Alignment::Off { resumes: Default::default() },
            ..Default::default()
        },
        deptheos_app::Words::english(deptheos_app::Platform::Desktop),
    )
    .unwrap();
    let output = session.run(&mut |_, _| {}, &deptheos_app::Cancel::root()).unwrap();
    assert!(output.stack.is_linear());
    let deptheos_render::Draw::Rendered(constants) = deptheos_render::resolve(
        deptheos_render::Color::Linear(output.stack.render_params().unwrap()),
        None,
        deptheos_render::OutputSpace::Srgb,
    ) else {
        panic!("a linear stack renders on the GPU");
    };
    let color = deptheos_render::dng_render::DngRenderPipeline::new(&ctx);
    let display =
        output.display.expect("a linear stack has a display to render").render(&color, &constants);
    assert_eq!((display.width(), display.height()), (w, h));

    // It exports into a wide space with that space's profile; ProPhoto
    // at 16 bits only.
    let engine = deptheos_app::test_support::engine().unwrap();
    let export = |space, name: &str| {
        let path = dir.path().join(name);
        let (neutral, none) = (Adjustments::NEUTRAL, deptheos_app::reframe::Reframe::NONE);
        let look = Look::redrawn(&output.stack, &neutral, none, space);
        engine.save(look, &path, SaveOptions::default()).map(|()| path)
    };
    let icc = |path: &std::path::Path| {
        use image::ImageDecoder;
        let reader = image::ImageReader::open(path).unwrap().with_guessed_format().unwrap();
        reader.into_decoder().unwrap().icc_profile().unwrap()
    };
    let p3 = deptheos_render::OutputSpace::DisplayP3;
    let png = export(p3, "p3.png").unwrap();
    assert_eq!(icc(&png).as_deref(), Some(p3.wide_icc().unwrap().bytes()));
    let prophoto = deptheos_render::OutputSpace::ProPhoto;
    assert!(export(prophoto, "prophoto.tif").is_ok());
    assert!(export(prophoto, "prophoto.jpg").is_err());

    // A reframed DNG keeps every pixel, its crop and turn as tags.
    use deptheos_app::reframe::{Flip, Rect, Reframe, Turn};
    let crop = Rect { x: 6, y: 4, width: 50, height: 30 };
    let reframe = Reframe { crop: Some(crop), flip: Flip::Horizontal, turn: Turn::Quarter };
    let path = dir.path().join("framed.dng");
    let look = Look::redrawn(&output.stack, &Adjustments::NEUTRAL, reframe, p3);
    engine.save(look, &path, SaveOptions::default()).unwrap();
    let file = std::io::BufReader::new(std::fs::File::open(&path).unwrap());
    let exif = exif::Reader::new().read_from_container(&mut std::io::BufReader::new(file)).unwrap();
    let orientation = exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY).unwrap();
    assert_eq!(orientation.value.get_uint(0), Some(u32::from(reframe.orientation())));
    // The user crop is in the raw image's IFD, read by the SDK.
    let frame = deptheos_dng::sdk_frame_of(&path).unwrap();
    assert_eq!(frame.orientation, u32::from(reframe.orientation()));
    let (wf, hf) = (f64::from(w), f64::from(h));
    let fractions = [4.0 / hf, 6.0 / wf, 34.0 / hf, 56.0 / wf];
    assert_eq!(frame.user_crop, fractions, "top, left, bottom, right");
    let read = deptheos_dng::DngImage::read_metadata(&path).unwrap();
    assert_eq!((read.width(), read.height()), (h, w), "every pixel, turned");
}

/// With the live map on, every method shows the stack so far after each
/// frame and pass 1, at the size asked for; without, none.
#[test]
fn every_method_has_a_live_picture() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let dir = tempfile::tempdir().unwrap();
    let paths = write_frames(dir.path());
    let configs = [
        StackerConfig::Pyramid(PyramidConfig::default()),
        StackerConfig::Depth(DepthConfig::default()),
        StackerConfig::Wavelet(WaveletConfig::default()),
    ];
    for (config, live_map) in configs.into_iter().flat_map(|c| [(c, true), (c, false)]) {
        let kind = config.kind();
        let session = StackingSession::new(
            ctx.clone(),
            common(&ctx),
            paths.clone(),
            config,
            SessionOptions {
                alignment: Alignment::Off { resumes: Default::default() },
                live_map,
                ..Default::default()
            },
            deptheos_app::Words::english(deptheos_app::Platform::Desktop),
        )
        .unwrap();
        let mut pictures = 0;
        session
            .run(
                &mut |event, live| {
                    let changed =
                        matches!(event, SessionEvent::Accumulated { .. } | SessionEvent::Finalized);
                    if changed && live_map {
                        // Never larger than asked, and capped.
                        let picture = live.render(SIZE).expect("a picture after a frame");
                        assert!(picture.width() <= SIZE && picture.height() <= SIZE, "{kind:?}");
                        let small = live.render(SIZE / 4).expect("a reduced picture");
                        assert_eq!((small.width(), small.height()), (SIZE / 4, SIZE / 4));
                        pictures += 1;
                    } else {
                        assert!(live.render(SIZE).is_none(), "{kind:?}: {event:?}");
                    }
                },
                &deptheos_app::Cancel::root(),
            )
            .unwrap();
        let expected = if live_map { 3 } else { 0 };
        assert_eq!(pictures, expected, "{kind:?}: two frames and the end of pass 1");
    }
}

#[test]
fn compare_source_lines_up_with_result() {
    use deptheos_app::Engine;

    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let dir = tempfile::tempdir().unwrap();
    let paths = write_frames(dir.path());
    let (_, result) = run_session(
        ctx.clone(),
        paths.clone(),
        StackerConfig::Pyramid(PyramidConfig::default()),
        None,
    );
    let stack = result.unwrap().stack;

    // Unaligned sRGB session: no transforms, no crop, no color science.
    assert_eq!(stack.geometry.transforms, vec![None, None]);
    assert!(stack.geometry.crop.is_none());
    assert!(!stack.geometry.reversed);
    assert!(stack.render_params().is_none());

    let engine = Engine::with_gpu(
        ctx.clone(),
        deptheos_app::Words::english(deptheos_app::Platform::Desktop),
    );
    // BGRA8 with half-size levels; 256 px is under the 1024 px floor: one level.
    let presented = engine
        .load_compare(&paths[1], Some(1), Look::stored(&stack), deptheos_app::Overlay::None)
        .unwrap();
    assert_eq!((presented.width(), presented.height()), (stack.width, stack.height));
    assert_eq!(presented.levels.len(), 1);
    assert_eq!(
        presented.levels[0].assemble().pixels.len(),
        (stack.width * stack.height * 4) as usize
    );

    let preview = engine.load_preview(&paths[0], deptheos_app::RawBackend::LibRaw).unwrap();
    assert_eq!((preview.width(), preview.height()), (SIZE, SIZE));

    let _ = ctx.device.poll(wgpu::PollType::wait_indefinitely());
}

/// The contribution overlay keeps the frame's share and dims the rest, for
/// every method's frame map.
#[test]
fn contribution_lights_the_frames_share_of_the_result() {
    use deptheos_app::{Engine, Overlay};

    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let dir = tempfile::tempdir().unwrap();
    let paths = write_frames(dir.path());
    let engine = Engine::with_gpu(
        ctx.clone(),
        deptheos_app::Words::english(deptheos_app::Platform::Desktop),
    );

    for config in [
        StackerConfig::Pyramid(PyramidConfig::default()),
        StackerConfig::Depth(DepthConfig::default()),
        StackerConfig::Wavelet(WaveletConfig::default()),
    ] {
        let (_, result) = run_session(ctx.clone(), paths.clone(), config, None);
        let stack = result.unwrap().stack;
        let name = config.kind().key();
        let map = stack.frame_map.as_ref().unwrap_or_else(|| panic!("{name}: a frame map"));
        assert_eq!(map.dimensions(), (SIZE, SIZE));
        assert_eq!(stack.frame_plane(0).plane, 0.0);
        assert_eq!(stack.frame_plane(1).plane, 1.0);

        // Each frame is sharp on half the picture, so took about half.
        let shares = stack.frame_contributions().unwrap_or_else(|| panic!("{name}: shares"));
        assert_eq!(shares.len(), 2, "{name}");
        assert!(shares.iter().all(|&s| (0.3..0.7).contains(&s)), "{name}: {shares:?}");
        assert!((shares.iter().sum::<f32>() - 1.0).abs() < 1e-3, "{name}: {shares:?}");

        // Frame 0 is sharp on the right, frame 1 on the left.
        for (index, sharp_right) in [(0, true), (1, false)] {
            let plain = engine.load_compare(
                &paths[index],
                Some(index),
                Look::stored(&stack),
                Overlay::None,
            );
            let tinted = engine.load_compare(
                &paths[index],
                Some(index),
                Look::stored(&stack),
                Overlay::Contribution,
            );
            let (plain, tinted) =
                (plain.unwrap().levels[0].assemble(), tinted.unwrap().levels[0].assemble());
            let changed = |x_from: u32, x_to: u32| {
                (0..SIZE)
                    .flat_map(|y| (x_from..x_to).map(move |x| ((y * SIZE + x) * 4) as usize))
                    .filter(|&i| plain.pixels[i..i + 4] != tinted.pixels[i..i + 4])
                    .count()
            };
            // Skip the seam (blur, smoothing) and the border (the pyramid's
            // Laplacian clamps); elsewhere the pyramid may pick a few percent
            // oddly where the checkerboard is flat.
            let (margin, border) = (16, 8);
            let (own, other) = if sharp_right {
                (changed(SIZE / 2 + margin, SIZE), changed(border, SIZE / 2 - margin))
            } else {
                (changed(0, SIZE / 2 - margin), changed(SIZE / 2 + margin, SIZE - border))
            };
            let half = ((SIZE / 2 - margin) * SIZE) as usize;
            assert!(own < half / 10, "{name} frame {index}: its sharp half is as it was ({own})");
            assert!(other > half * 9 / 10, "{name} frame {index}: the other half is dim ({other})");
        }
        // A frame the stack did not include has no share to show.
        let outside = engine
            .load_compare(&paths[0], None, Look::stored(&stack), Overlay::Contribution)
            .unwrap();
        let plain =
            engine.load_compare(&paths[0], None, Look::stored(&stack), Overlay::None).unwrap();
        assert!(outside.levels[0].assemble().pixels == plain.levels[0].assemble().pixels);
    }
    let _ = ctx.device.poll(wgpu::PollType::wait_indefinitely());
}

/// A RAW file can't be compared with a standard-image stack: refused by
/// extension, before decoding.
#[test]
fn compare_refuses_the_other_source_kind() {
    use deptheos_app::Engine;

    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let dir = tempfile::tempdir().unwrap();
    let paths = write_frames(dir.path());
    let (_, result) =
        run_session(ctx.clone(), paths, StackerConfig::Pyramid(PyramidConfig::default()), None);
    let stack = result.unwrap().stack;

    let engine =
        Engine::with_gpu(ctx, deptheos_app::Words::english(deptheos_app::Platform::Desktop));
    let err = engine
        .load_compare(
            &dir.path().join("frame.cr3"),
            None,
            Look::stored(&stack),
            deptheos_app::Overlay::None,
        )
        .err()
        .unwrap();
    assert!(
        matches!(&err, deptheos_app::Error::MixedKinds { file, kind: FrameKind::Raw, .. } if file == "frame.cr3"),
        "{err}"
    );
}

/// A mix of RAW/DNG and standard images is refused up front, either order.
#[test]
fn mixed_raw_and_standard_stack_is_rejected() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let options = SessionOptions::default;
    let config = || StackerConfig::Pyramid(PyramidConfig::default());
    for paths in [
        vec![PathBuf::from("a.jpg"), PathBuf::from("b.CR3")],
        vec![PathBuf::from("a.dng"), PathBuf::from("b.png"), PathBuf::from("c.dng")],
    ] {
        let err = StackingSession::new(
            ctx.clone(),
            common(&ctx),
            paths,
            config(),
            options(),
            deptheos_app::Words::english(deptheos_app::Platform::Desktop),
        )
        .err()
        .expect("a mixed stack must be refused");
        assert!(matches!(err, deptheos_app::Error::MixedKinds { .. }), "{err}");
    }
    // The engine's session refuses it too, before converting the RAW frame.
    let engine = deptheos_app::Engine::with_gpu(
        ctx.clone(),
        deptheos_app::Words::english(deptheos_app::Platform::Desktop),
    );
    let converting =
        SessionOptions { raw_backend: deptheos_app::RawBackend::AdobeDng, ..options() };
    let mixed = vec![PathBuf::from("a.jpg"), PathBuf::from("b.CR3")];
    let err = engine.session(mixed, config(), converting).err().expect("refused");
    assert!(matches!(err, deptheos_app::Error::MixedKinds { .. }), "{err}");
    // One kind is fine (the files are not read until the session runs).
    let raw = vec![PathBuf::from("a.cr3"), PathBuf::from("b.dng")];
    assert!(
        StackingSession::new(
            ctx.clone(),
            common(&ctx),
            raw,
            config(),
            options(),
            deptheos_app::Words::english(deptheos_app::Platform::Desktop)
        )
        .is_ok()
    );
}

/// The GUI's path: events in order, live maps delivered before `stack`
/// returns, final images read back.
#[test]
fn engine_stack_delivers_live_maps_and_final_images() {
    use deptheos_app::Engine;

    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let dir = tempfile::tempdir().unwrap();
    let paths = write_frames(dir.path());
    let engine =
        Engine::with_gpu(ctx, deptheos_app::Words::english(deptheos_app::Platform::Desktop));

    let live_maps = std::sync::atomic::AtomicU32::new(0);
    let mut events = Vec::new();
    let done = engine
        .stack(
            paths,
            StackerConfig::Depth(DepthConfig::default()),
            SessionOptions {
                alignment: Alignment::Off { resumes: Default::default() },
                live_map: true,
                ..Default::default()
            },
            &Adjustments::NEUTRAL,
            deptheos_app::LIVE_PREVIEW_SIDE,
            &deptheos_app::Cancel::root(),
            |e| events.push(e),
            |live| {
                assert!(live.width().max(live.height()) <= deptheos_app::LIVE_PREVIEW_SIDE);
                live_maps.fetch_add(1, Ordering::Relaxed);
            },
        )
        .unwrap();
    assert_eq!(events.first(), Some(&SessionEvent::Started { total: 2 }));
    assert_eq!(events.last(), Some(&SessionEvent::Completed));
    // Three maps offered (two frames + finalize); the mailbox may coalesce.
    let shown = live_maps.load(Ordering::Relaxed);
    assert!((1..=3).contains(&shown), "{shown} live maps");
    let display = done.display.expect("display read back");
    assert_eq!((display.width(), display.height()), (SIZE, SIZE));
    // The shown map keeps the result's geometry but starts at half size.
    let map = done.map.expect("map presented");
    assert_eq!((map.width(), map.height()), (SIZE, SIZE));
    assert_eq!(map.first_level, 1);
    assert_eq!((map.levels[0].width, map.levels[0].height), (SIZE / 2, SIZE / 2));
    assert_eq!((done.stack.width, done.stack.height), (SIZE, SIZE));
}

/// Output frame size of the aligned fixtures.
const ALIGN_W: u32 = 640;
const ALIGN_H: u32 = 400;

/// An `800/zoom`×`500/zoom` window at `(cx, cy)` of the aligner's 1200×750
/// natural-image fixture, resized to 640×400.
fn write_fixture_frames(dir: &std::path::Path, frames: &[(f32, f32, f32)]) -> Vec<PathBuf> {
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/../deptheos_aligner/fixtures/spider.jpg");
    let base = image::open(fixture).unwrap().resize_exact(
        1200,
        750,
        image::imageops::FilterType::Triangle,
    );
    frames
        .iter()
        .enumerate()
        .map(|(i, &(cx, cy, zoom))| {
            let (w, h) = (800.0 / zoom, 500.0 / zoom);
            let window =
                base.crop_imm((cx - w / 2.0) as u32, (cy - h / 2.0) as u32, w as u32, h as u32);
            let frame =
                window.resize_exact(ALIGN_W, ALIGN_H, image::imageops::FilterType::Lanczos3);
            let path = dir.join(format!("frame_{i}.png"));
            frame.to_rgb8().save(&path).unwrap();
            path
        })
        .collect()
}

fn aligned_run(
    ctx: &Arc<GpuContext>,
    paths: &[PathBuf],
    mode: AlignMode,
    recorded: Option<deptheos_app::StackGeometry>,
) -> (Vec<SessionEvent>, deptheos_app::CompletedStack) {
    let options = SessionOptions {
        alignment: Alignment::On(AlignSettings { mode, grid: GridConfig::default(), crop: true }),
        ..Default::default()
    };
    let mut session = StackingSession::new(
        ctx.clone(),
        common(ctx),
        paths.to_vec(),
        StackerConfig::Pyramid(PyramidConfig::default()),
        options,
        deptheos_app::Words::english(deptheos_app::Platform::Desktop),
    )
    .unwrap();
    if let Some(geometry) = recorded {
        session = session.with_alignment(geometry).unwrap();
    }
    let mut events = Vec::new();
    let output = session.run(&mut |e, _| events.push(e), &deptheos_app::Cancel::root()).unwrap();
    (events, output.stack)
}

/// Both aligners find shifts; the result is cropped to the common area, in
/// the caller's order.
#[test]
fn aligned_session_crops_to_the_shared_area() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let dir = tempfile::tempdir().unwrap();
    // The window moves right and down, so the transforms translate by the
    // negated shift.
    let paths = write_fixture_frames(
        dir.path(),
        &[(600.0, 375.0, 1.0), (620.0, 385.0, 1.0), (640.0, 395.0, 1.0)],
    );

    for mode in AlignMode::ALL {
        let (events, stack) = aligned_run(&ctx, &paths, mode, None);
        assert!(!events.contains(&SessionEvent::Reversed), "{mode:?}: no magnification change");
        let geometry = &stack.geometry;
        assert!(!geometry.reversed);
        assert_eq!(geometry.transforms.len(), 3);
        let shift = |i: usize| {
            let t = geometry.transforms[i].expect("aligned").transform;
            (t[(0, 2)], t[(1, 2)])
        };
        for (i, (want_x, want_y)) in
            [(0.0, 0.0), (-16.0, -8.0), (-32.0, -16.0)].into_iter().enumerate()
        {
            let (tx, ty) = shift(i);
            assert!(
                (tx - want_x).abs() < 1.0 && (ty - want_y).abs() < 1.0,
                "{mode:?} frame {i}: shift ({tx}, {ty}), want ({want_x}, {want_y})"
            );
        }
        let crop = geometry.crop.expect("shifted frames trim the result");
        assert!(crop.x + crop.width <= ALIGN_W && crop.y + crop.height <= ALIGN_H);
        assert!(
            (ALIGN_W - 34..=ALIGN_W - 31).contains(&crop.width)
                && (ALIGN_H - 18..=ALIGN_H - 15).contains(&crop.height),
            "{mode:?}: crop {crop:?}"
        );
        assert_eq!((stack.width, stack.height), (crop.width, crop.height));
        assert_eq!(stack.frame_map.as_ref().unwrap().dimensions(), (crop.width, crop.height));
    }
}

/// A common area under a quarter of the frame leaves the result uncropped.
#[test]
fn frames_moved_too_far_leave_the_result_uncropped() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let dir = tempfile::tempdir().unwrap();
    // Steps of 25 × 15 px (under ECC's 5%) leave 265 × 175 of 640 × 400.
    let frames: Vec<_> =
        (0..16).map(|i| (400.0 + 25.0 * i as f32, 250.0 + 15.0 * i as f32, 1.25)).collect();
    let paths = write_fixture_frames(dir.path(), &frames);

    for mode in AlignMode::ALL {
        let (_, stack) = aligned_run(&ctx, &paths, mode, None);
        assert!(stack.geometry.left_out.is_empty(), "{mode:?}: {:?}", stack.geometry.left_out);
        assert_eq!(stack.geometry.crop, None, "{mode:?}");
        assert_eq!((stack.width, stack.height), (ALIGN_W, ALIGN_H), "{mode:?}");
        assert!(
            matches!(&stack.notes[..], [note] if note.contains("moved too far to crop")),
            "{mode:?}: {:?}",
            stack.notes
        );
    }
}

/// Aligned frames ([`deptheos_app::runs::aligned`]) come out cropped and line
/// up; a left-out frame has none.
#[test]
fn aligned_frames_are_written_in_the_stacks_geometry() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let engine = deptheos_app::Engine::with_gpu(
        ctx,
        deptheos_app::Words::english(deptheos_app::Platform::CommandLine),
    );
    let dir = tempfile::tempdir().unwrap();
    let mut paths = write_fixture_frames(
        dir.path(),
        &[(600.0, 375.0, 1.0), (620.0, 385.0, 1.0), (640.0, 395.0, 1.0)],
    );
    let unreadable = dir.path().join("frame_3.png");
    std::fs::write(&unreadable, b"not a png").unwrap();
    paths.push(unreadable);

    for mode in AlignMode::ALL {
        let options = SessionOptions {
            alignment: Alignment::On(AlignSettings {
                mode,
                grid: GridConfig::default(),
                crop: true,
            }),
            ..Default::default()
        };
        let aligned = engine
            .align(paths.clone(), options, &mut |_, _| {}, &deptheos_app::Cancel::root())
            .unwrap();
        let crop = aligned.geometry().crop.expect("shifted frames trim the result");
        assert_eq!(aligned.size(), (crop.width, crop.height));
        assert_eq!(aligned.aligned().collect::<Vec<_>>(), [0, 1, 2], "{mode:?}");
        assert!(aligned.left_out_message(engine.words()).is_some());
        // The frames land where a stack of them puts them.
        let (_, stack) = aligned_run(engine.gpu(), &paths, mode, None);
        assert_eq!(aligned.geometry(), &stack.geometry, "{mode:?}");
        assert_eq!(aligned.size(), (stack.width, stack.height), "{mode:?}");

        let out = dir.path().join(format!("{mode:?}"));
        std::fs::create_dir(&out).unwrap();
        let written: Vec<image::RgbImage> = aligned
            .aligned()
            .map(|i| {
                let path = out.join(format!("{i}.tif"));
                engine.save_aligned(&aligned, i, &path, SaveOptions::default()).unwrap();
                image::open(&path).unwrap().to_rgb8()
            })
            .collect();
        assert!(
            engine.save_aligned(&aligned, 3, &out.join("3.tif"), SaveOptions::default()).is_err()
        );
        for image in &written {
            assert_eq!(image.dimensions(), aligned.size(), "{mode:?}");
        }

        // Mean absolute difference over the middle of two images of one size.
        let differ = |a: &image::RgbImage, b: &image::RgbImage| {
            let (w, h) = a.dimensions();
            let (mut sum, mut n) = (0u64, 0u64);
            for y in h / 4..3 * h / 4 {
                for x in w / 4..3 * w / 4 {
                    for c in 0..3 {
                        sum += u64::from(a.get_pixel(x, y)[c].abs_diff(b.get_pixel(x, y)[c]));
                        n += 1;
                    }
                }
            }
            sum as f64 / n as f64
        };
        let shot: Vec<image::RgbImage> =
            paths[..3].iter().map(|p| image::open(p).unwrap().to_rgb8()).collect();
        let (before, after) = (differ(&shot[0], &shot[2]), differ(&written[0], &written[2]));
        assert!(after < before / 3.0, "{mode:?}: {after:.1} apart aligned, {before:.1} as shot");
    }
}

/// Growing magnification trips the reverse check; the geometry maps back
/// to the caller's order, and replaying it reproduces the stack exactly.
#[test]
fn reversed_alignment_replays_exactly() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let dir = tempfile::tempdir().unwrap();
    let paths = write_fixture_frames(
        dir.path(),
        &[(600.0, 375.0, 1.0), (600.0, 375.0, 1.02), (600.0, 375.0, 1.04)],
    );

    for mode in AlignMode::ALL {
        let (events, stack) = aligned_run(&ctx, &paths, mode, None);
        assert!(events.contains(&SessionEvent::Reversed), "{mode:?}: reverse check should trip");
        let geometry = stack.geometry.clone();
        assert!(geometry.reversed);
        assert_eq!(stack.paths, paths, "paths stay in the caller's order");
        // The last path (most magnified) was the reference.
        let reference = geometry.transforms[2].expect("aligned").transform;
        assert!((reference[(0, 0)] - 1.0).abs() < 1e-6 && reference[(0, 2)].abs() < 1e-6);
        let first = geometry.transforms[0].expect("aligned");
        assert!(first.magnification() < 1.0, "{mode:?}: frame 0 is the widest view");

        let (replay_events, replay) = aligned_run(&ctx, &paths, mode, Some(geometry.clone()));
        assert!(!replay_events.contains(&SessionEvent::Reversed));
        assert_eq!(replay.geometry, geometry);
        assert_eq!((replay.width, replay.height), (stack.width, stack.height));
        assert!(
            replay.pixels == stack.pixels,
            "{mode:?}: replayed alignment must reproduce the stack"
        );
    }
}

/// A display texture reads back the same from Rgba16Unorm and Rgba32Float,
/// with levels down to the 1024 px floor; other formats are refused.
#[test]
fn display_readback_matches_in_both_result_formats() {
    use deptheos_app::engine::present::{ToDisplay, present_texture};
    use deptheos_core::io::quantize_u16;

    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let (w, h) = (2049u32, 65u32);
    let value = |i: u32| (i % 997) as f32 / 996.0;
    let upload = |format: wgpu::TextureFormat, bytes: &[u8], texel: u32| {
        let texture = ctx.create_texture(
            w,
            h,
            format,
            wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            "display_src",
        );
        ctx.queue.write_texture(
            texture.as_image_copy(),
            bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * texel),
                rows_per_image: None,
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        texture
    };
    let floats: Vec<f32> = (0..w * h * 4).map(value).collect();
    let unorm: Vec<u16> = floats.iter().map(|&v| quantize_u16(v)).collect();
    let f32_texture = upload(wgpu::TextureFormat::Rgba32Float, bytemuck::cast_slice(&floats), 16);
    let u16_texture = upload(wgpu::TextureFormat::Rgba16Unorm, bytemuck::cast_slice(&unorm), 8);

    let from_f32 = present_texture(&ctx, &f32_texture, &ToDisplay::SRGB).unwrap();
    let from_u16 = present_texture(&ctx, &u16_texture, &ToDisplay::SRGB).unwrap();
    let sizes: Vec<_> = from_u16.levels.iter().map(|l| (l.width, l.height)).collect();
    assert_eq!(
        sizes,
        [(2049, 65), (1025, 33), (513, 17)],
        "halving rounds up: 1025 > 1024 halves once more"
    );
    for (a, b) in from_f32.levels.iter().zip(&from_u16.levels) {
        assert!(a.assemble().pixels == b.assemble().pixels, "both formats display the same");
    }
    // Pixel 1 is (R, G, B) = values 4, 5, 6 of 996, in BGRA order.
    let px = |i: u32| deptheos_core::io::u16_to_u8(quantize_u16(value(i)));
    assert_eq!(&from_u16.levels[0].assemble().pixels[4..8], &[px(6), px(5), px(4), 255]);

    let rgba8 = ctx.create_texture(
        w,
        h,
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureUsages::COPY_SRC,
        "rgba8",
    );
    assert!(present_texture(&ctx, &rgba8, &ToDisplay::SRGB).is_err());
    let _ = ctx.device.poll(wgpu::PollType::wait_indefinitely());
}

/// Ten frames from [`write_frames`]; those at `bad` replaced by a non-image
/// (`Some(true)`) or one of another size (`Some(false)`).
fn ten_frames(dir: &std::path::Path, bad: &[(usize, bool)]) -> Vec<PathBuf> {
    let pair = write_frames(dir);
    (0..10)
        .map(|i| {
            let path = dir.join(format!("{i:02}.png"));
            match bad.iter().find(|(b, _)| *b == i) {
                Some((_, true)) => std::fs::write(&path, b"not a png").unwrap(),
                Some((_, false)) => ImageBuffer::from_pixel(SIZE / 2, SIZE, Rgb([90u8, 90, 90]))
                    .save(&path)
                    .unwrap(),
                None => {
                    std::fs::copy(&pair[i % 2], &path).unwrap();
                }
            }
            path
        })
        .collect()
}

/// No pixel of `stack` is near black (brightest channel at least 40).
fn assert_no_holes(stack: &deptheos_app::CompletedStack) {
    let darkest = stack.pixels.chunks(3).map(|p| p.iter().max().copied().unwrap()).min().unwrap();
    assert!(darkest > 40 * 257, "a pixel no frame reached: {darkest}");
}

#[test]
fn a_frame_that_cannot_be_read_is_left_out() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let dir = tempfile::tempdir().unwrap();
    let paths = ten_frames(dir.path(), &[(3, true)]);

    let (events, result) =
        run_session(ctx, paths.clone(), StackerConfig::Pyramid(PyramidConfig::default()), None);
    let stack = result.unwrap().stack;
    assert_eq!(stack.paths, paths, "the stack keeps every frame's place");
    let left_out = &stack.geometry.left_out;
    assert_eq!(left_out.len(), 1);
    assert_eq!(left_out[0].index, 3);
    // Kept in the user's words: a clause about the frame.
    assert_eq!(left_out[0].reason, "couldn't be read as an image file");
    assert_eq!(stack.geometry.transforms.len(), 10);
    let accumulated =
        events.iter().filter(|e| matches!(e, SessionEvent::Accumulated { .. })).count();
    assert_eq!(accumulated, 10, "progress counts the left-out frame");
    // It wins nowhere: the frame map never shows its plane.
    let plane = (3.0 / 9.0 * 65535.0f32).round() as u16;
    let frame_map = stack.frame_map.as_ref().unwrap();
    assert!(frame_map.pixels().all(|p| p[0].abs_diff(plane) > 100));
    assert_no_holes(&stack);
}

#[test]
fn a_frame_of_another_size_stops_the_stack() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let dir = tempfile::tempdir().unwrap();
    let paths = ten_frames(dir.path(), &[(5, false)]);

    let (_, result) =
        run_session(ctx, paths, StackerConfig::Pyramid(PyramidConfig::default()), None);
    let error = result.err().expect("a frame of another size stops the stack").to_string();
    assert!(error.contains("05.png"), "the error names the frame: {error}");
}

#[test]
fn depth_blends_across_a_left_out_frame() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let dir = tempfile::tempdir().unwrap();
    let paths = ten_frames(dir.path(), &[(4, true)]);

    let (events, result) =
        run_session(ctx, paths, StackerConfig::Depth(DepthConfig::default()), None);
    let stack = result.unwrap().stack;
    assert_eq!(stack.geometry.left_out.iter().map(|l| l.index).collect::<Vec<_>>(), [4]);
    let blended = events.iter().filter(|e| matches!(e, SessionEvent::Blended { .. })).count();
    assert_eq!(blended, 9, "the left-out frame is not blended");
    assert_no_holes(&stack);
}

#[test]
fn too_many_bad_frames_stop_the_stack() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let dir = tempfile::tempdir().unwrap();
    // Ten frames leave out at most one.
    let paths = ten_frames(dir.path(), &[(2, true), (6, true)]);

    let (_, result) =
        run_session(ctx, paths, StackerConfig::Pyramid(PyramidConfig::default()), None);
    match result {
        Err(deptheos_app::Error::TooManyLeftOut { count: 2, total: 10, first, reason }) => {
            assert!(first.starts_with("02.png"), "{first}");
            assert_eq!(reason, "couldn't be read as an image file");
        }
        Err(e) => panic!("unexpected error: {e}"),
        Ok(_) => panic!("stacked with two of ten frames bad"),
    }
}

/// Rewrite the PNG `path` with the ICC profile `icc`.
fn retag(path: &std::path::Path, icc: Option<&[u8]>) {
    use image::ImageEncoder;
    let image = image::open(path).unwrap().into_rgb8();
    let file = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    let mut encoder = image::codecs::png::PngEncoder::new(file);
    if let Some(icc) = icc {
        encoder.set_icc_profile(icc.to_vec()).unwrap();
    }
    let (w, h) = image.dimensions();
    encoder.write_image(image.as_raw(), w, h, image::ExtendedColorType::Rgb8).unwrap();
}

/// Adobe RGB frames stack in Adobe RGB, embedded in exports and the Library.
#[test]
fn a_wide_gamut_stack_keeps_its_profile() {
    use deptheos_app::StackColor;
    use deptheos_app::library::history::{History, StackId};
    use deptheos_core::io::IccProfile;
    use image::ImageDecoder;
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let dir = tempfile::tempdir().unwrap();
    let paths = write_frames(dir.path());
    let adobe = IccProfile::adobe_rgb_compatible();
    for path in &paths {
        retag(path, Some(adobe.bytes()));
    }
    let config = StackerConfig::Pyramid(PyramidConfig::default());
    let stack = run_session(ctx.clone(), paths.clone(), config, None).1.unwrap().stack;
    assert!(matches!(&stack.color, StackColor::Srgb { profile: Some(p), .. } if *p == adobe));

    for ext in ["tif", "jpg", "png"] {
        let out = dir.path().join(format!("result.{ext}"));
        deptheos_app::test_support::engine()
            .unwrap()
            .save(Look::exported(&stack, Some(&Adjustments::NEUTRAL)), &out, SaveOptions::default())
            .unwrap();
        let reader = image::ImageReader::open(&out).unwrap().with_guessed_format().unwrap();
        let icc = reader.into_decoder().unwrap().icc_profile().unwrap();
        assert_eq!(icc.as_deref(), Some(adobe.bytes()), ".{ext}");
    }

    let history = History::open(dir.path().join("store")).unwrap();
    let render = history
        .save_render(
            &paths,
            &stack,
            &image::RgbImage::new(8, 8),
            &StackId::new(),
            &Made::Stacked { base: None },
        )
        .unwrap();
    let loaded = history.load_render(&render.id).unwrap();
    assert!(matches!(&loaded.color, StackColor::Srgb { profile: Some(p), .. } if *p == adobe));
}

/// Frames convert into the first frame's color space (ADR-132); a frame
/// out of its gamut is noted.
#[test]
fn frames_in_other_color_spaces_are_converted_into_the_first() {
    use deptheos_app::StackColor;
    use deptheos_core::io::IccProfile;
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let dir = tempfile::tempdir().unwrap();
    let paths = write_frames(dir.path());
    let p3 = moxcms::ColorProfile::new_display_p3().encode().unwrap();
    retag(&paths[0], None);
    retag(&paths[1], Some(&p3));
    let config = StackerConfig::Pyramid(PyramidConfig::default());
    let stack = run_session(ctx.clone(), paths.clone(), config, None).1.unwrap().stack;
    assert!(matches!(&stack.color, StackColor::Srgb { profile: None, .. }), "the first's: sRGB");
    assert!(
        stack.notes.iter().any(|n| n.contains("Display P3") && n.contains("clipped")),
        "{:?}",
        stack.notes
    );
    // Wide first, the stack keeps the gamut.
    retag(&paths[0], Some(IccProfile::adobe_rgb_compatible().bytes()));
    let stack = run_session(ctx, paths, config, None).1.unwrap().stack;
    let adobe = IccProfile::adobe_rgb_compatible();
    assert!(matches!(&stack.color, StackColor::Srgb { profile: Some(p), .. } if *p == adobe));
}

/// A CMYK TIFF among sRGB PNGs stops the stack by name before decoding.
#[test]
fn a_cmyk_frame_stops_the_stack_by_name() {
    use deptheos_core::CoreError;
    use deptheos_core::io::Refused;
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let dir = tempfile::tempdir().unwrap();
    let mut paths = write_frames(dir.path());
    let cmyk = dir.path().join("cmyk.tif");
    {
        let file = std::io::BufWriter::new(std::fs::File::create(&cmyk).unwrap());
        let mut tiff = tiff::encoder::TiffEncoder::new(file).unwrap();
        let samples = vec![0u8; (SIZE * SIZE * 4) as usize];
        tiff.write_image::<tiff::encoder::colortype::CMYK8>(SIZE, SIZE, &samples).unwrap();
    }
    paths.push(cmyk);
    let config = StackerConfig::Pyramid(PyramidConfig::default());
    let (events, result) = run_session(ctx, paths, config, None);
    let Err(deptheos_app::Error::Core(CoreError::Refused { path, what: Refused::Cmyk })) = result
    else {
        panic!("a CMYK frame stacked: {:?}", result.map(|_| ()))
    };
    assert!(path.ends_with("cmyk.tif"));
    assert!(!events.iter().any(|e| matches!(e, SessionEvent::Accumulated { .. })), "nothing read");
}

/// A replay never reads a frame its alignment left out (here a pipe a read
/// would block on).
#[cfg(unix)]
#[test]
fn a_replay_never_reads_the_frames_it_left_out() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let dir = tempfile::tempdir().unwrap();
    let paths = ten_frames(dir.path(), &[(3, true)]);
    let config = StackerConfig::Pyramid(PyramidConfig::default());
    let (_, first) = run_session(ctx.clone(), paths.clone(), config, None);
    let geometry = first.unwrap().stack.geometry;
    assert_eq!(geometry.left_out.iter().map(|l| l.index).collect::<Vec<_>>(), [3]);

    std::fs::remove_file(&paths[3]).unwrap();
    let fifo = std::ffi::CString::new(paths[3].to_str().unwrap()).unwrap();
    // SAFETY: a valid, NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);

    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let options = SessionOptions {
            alignment: Alignment::Off { resumes: Default::default() },
            ..Default::default()
        };
        let session = StackingSession::new(
            ctx.clone(),
            common(&ctx),
            paths,
            config,
            options,
            deptheos_app::Words::english(deptheos_app::Platform::Desktop),
        )
        .unwrap()
        .with_alignment(geometry)
        .unwrap();
        let _ = tx.send(session.run(&mut |_, _| {}, &deptheos_app::Cancel::root()));
    });
    let replayed = rx
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("the replay waited on the frame it left out")
        .unwrap();
    let left_out = &replayed.stack.geometry.left_out;
    assert_eq!(left_out.iter().map(|l| l.index).collect::<Vec<_>>(), [3]);
    assert_eq!(left_out[0].reason, "couldn't be read as an image file", "as recorded");
}
