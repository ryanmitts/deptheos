use deptheos_gpu::GpuContext;
use deptheos_gpu::gpu_utils::download_texture_with;

/// A whole texture's bytes in its own format, row padding stripped.
fn download_texture(ctx: &GpuContext, texture: &wgpu::Texture) -> deptheos_gpu::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let size = [texture.width(), texture.height()];
    download_texture_with(ctx, texture, [0, 0], size, |row| bytes.extend_from_slice(row))?;
    Ok(bytes)
}

#[test]
fn context_initializes() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };

    // A texture allocation in the negotiated formats must succeed.
    let tex = ctx.create_texture(
        256,
        256,
        ctx.source_format(),
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        "smoke",
    );
    assert_eq!(tex.width(), 256);
}

/// A readback returns exactly what was uploaded, for the formats the
/// stackers read back. 37 px makes every row pitch unaligned.
#[test]
fn download_round_trips_unaligned_rows() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let (w, h) = (37, 5);
    for format in [
        wgpu::TextureFormat::R8Unorm,
        wgpu::TextureFormat::R32Float,
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Rgba16Unorm,
        wgpu::TextureFormat::Rgba32Float,
    ] {
        if format == wgpu::TextureFormat::Rgba16Unorm
            && !ctx.device.features().contains(wgpu::Features::TEXTURE_FORMAT_16BIT_NORM)
        {
            continue;
        }
        let bpp = format.block_copy_size(None).unwrap();
        let data: Vec<u8> = (0..w * h * bpp).map(|i| (i * 7 + 3) as u8).collect();
        let tex = ctx.create_texture(
            w,
            h,
            format,
            wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
            "download_round_trip",
        );
        ctx.queue.write_texture(
            tex.as_image_copy(),
            &data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * bpp),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        let bytes = download_texture(&ctx, &tex).unwrap();
        assert!(bytes == data, "{format:?} round trip differs");
    }
}

/// A region taller than one staging band reads back whole, bands in order:
/// 32 MiB bands of a 2049-wide rgba32f texture are 1023 rows, so 2500 rows
/// take three.
#[test]
fn a_tall_download_reads_back_in_bands() {
    use deptheos_gpu::gpu_utils::download_texture_rows;
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let (w, h, bpp) = (2049u32, 2504u32, 16u32);
    let data: Vec<u8> = (0..w * h * bpp).map(|i| (i % 251) as u8 ^ (i / 4099) as u8).collect();
    let format = wgpu::TextureFormat::Rgba32Float;
    let tex = ctx.create_texture(
        w,
        h,
        format,
        wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
        "tall_download",
    );
    ctx.queue.write_texture(
        tex.as_image_copy(),
        &data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(w * bpp),
            rows_per_image: Some(h),
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    let (mut bytes, mut tops) = (Vec::new(), Vec::new());
    download_texture_rows(&ctx, &tex, [0, 3], [w, 2500], |top, mapped, stride, row_bytes| {
        assert_eq!(top as usize, bytes.len() / row_bytes, "bands in order");
        tops.push(top);
        for row in mapped.chunks_exact(stride) {
            bytes.extend_from_slice(&row[..row_bytes]);
        }
    })
    .unwrap();
    assert_eq!(tops.len(), 3, "{tops:?}");
    let row = (w * bpp) as usize;
    assert!(bytes == data[3 * row..2503 * row], "the region, row for row");
}

/// The `MAP_R8` shader def and `map_format()` agree: a kernel storing under
/// `#ifdef MAP_R8` binds, and the gray reads back from each pixel's byte 0.
#[test]
fn map_format_matches_map_r8_def() {
    use deptheos_gpu::gpu_utils::{create_compute_pipeline, dispatch_compute_textures};

    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let source = "
#ifdef MAP_R8
@group(0) @binding(0) var dst: texture_storage_2d<r8unorm, write>;
#else
@group(0) @binding(0) var dst: texture_storage_2d<rgba8unorm, write>;
#endif

@compute @workgroup_size(16, 16)
fn fill(@builtin(global_invocation_id) gid: vec3u) {
    let dims = textureDimensions(dst);
    if (gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }
    let g = f32(gid.x) / 255.0;
    textureStore(dst, gid.xy, vec4f(g, g, g, 1.0));
}
";
    let module = ctx.create_shader_module(source, "map_r8_probe");
    let pipeline = create_compute_pipeline(&ctx.device, &module, "fill");
    let (w, h) = (37, 3);
    let format = ctx.map_format();
    let tex = ctx.create_texture(
        w,
        h,
        format,
        wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
        "map_r8_probe",
    );
    let mut encoder = ctx.encoder("test");
    dispatch_compute_textures(&mut encoder, &ctx.device, &pipeline, "fill", &[&tex], &[], w, h);
    ctx.submit(encoder);

    let bytes = download_texture(&ctx, &tex).unwrap();
    let bpp = format.block_copy_size(None).unwrap() as usize;
    for y in 0..h as usize {
        for x in 0..w as usize {
            assert_eq!(bytes[(y * w as usize + x) * bpp], x as u8, "{format:?} at ({x}, {y})");
        }
    }
}

/// A texture wgpu rejects (zero width).
fn invalid_texture(ctx: &GpuContext) -> wgpu::Texture {
    ctx.create_texture(0, 16, wgpu::TextureFormat::R32Float, wgpu::TextureUsages::COPY_SRC, "bad")
}

/// A GPU error is returned by the next check, once, instead of wgpu's
/// default handler panicking.
#[test]
fn a_gpu_error_is_returned_once_not_a_panic() {
    use deptheos_gpu::GpuError;

    let Some(ctx) = GpuContext::new_or_skip() else { return };
    ctx.check().expect("a fresh context has no error");
    drop(invalid_texture(&ctx));
    assert!(matches!(ctx.check(), Err(GpuError::Internal(_))));
    ctx.check().expect("each error is returned once");
    ctx.check_lost().expect("not lost");
}

/// A readback after a failed call is an error, not pixels from work that
/// failed silently.
#[test]
fn a_readback_after_a_gpu_error_fails() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let good = ctx.create_texture(
        8,
        8,
        wgpu::TextureFormat::R32Float,
        wgpu::TextureUsages::COPY_SRC,
        "good",
    );
    drop(invalid_texture(&ctx));
    assert!(download_texture(&ctx, &good).is_err());
    assert_eq!(download_texture(&ctx, &good).expect("the error was reported").len(), 8 * 8 * 4);
}

/// An error belongs to the thread that caused it: work on another thread
/// (a stack beside a paint session) does not see it.
#[test]
fn a_gpu_error_stays_on_its_thread() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    std::thread::scope(|s| {
        s.spawn(|| drop(invalid_texture(&ctx))).join().unwrap();
    });
    ctx.check().expect("the other thread's error is not this one's");
}

/// Clearing a thread's errors, as a shared worker does between jobs,
/// forgets every context's error unchecked there.
#[test]
fn clearing_a_thread_forgets_its_unchecked_errors() {
    let (Some(one), Some(two)) = (GpuContext::new_or_skip(), GpuContext::new_or_skip()) else {
        return;
    };
    drop(invalid_texture(&one));
    drop(invalid_texture(&two));
    deptheos_gpu::clear_thread_faults();
    one.check().expect("cleared");
    two.check().expect("cleared");
}

/// Destroying the device, which wgpu reports as a loss too, fails no
/// check. (A real loss cannot be caused from a test.)
#[test]
fn destroying_the_device_is_not_a_loss() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    ctx.device.destroy();
    ctx.check().expect("not lost");
    ctx.check_lost().expect("not lost");
}

/// Work past the budget (Metal's) is refused with what it needs and what
/// is usable.
#[test]
fn work_past_the_memory_budget_is_refused() {
    use deptheos_gpu::GpuError;

    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let Some(budget) = ctx.memory_budget() else { return };
    let usable = budget.usable;
    budget.check(usable / 2, 100, 100).expect("half the budget fits");
    match budget.check(usable * 2, 8256, 5504) {
        Err(GpuError::OverBudget { width: 8256, height: 5504, needed, budget: b }) => {
            assert_eq!((needed, b), (usable * 2, usable));
        }
        other => panic!("expected OverBudget, got {other:?}"),
    }
}

/// A budget set on the context is the one planned on and checked against,
/// whatever the device has.
#[test]
fn a_set_budget_is_the_one_planned_on() {
    use deptheos_gpu::{GpuError, MemoryBudget, MemoryPool};

    let ctx = match GpuContext::new() {
        Ok(ctx) => ctx,
        Err(GpuError::NoAdapter) => return GpuContext::skip_test("no GPU adapter available"),
        Err(e) => panic!("GPU init failed: {e}"),
    };
    let set = MemoryBudget { usable: 300 << 20, pool: MemoryPool::Shared };
    let ctx = ctx.with_memory_budget(set);
    let budget = ctx.memory_budget().expect("a set budget");
    assert_eq!(budget, set);
    budget.check(300 << 20, 100, 100).expect("all of it fits");
    match budget.check((300 << 20) + 1, 100, 100) {
        Err(GpuError::OverBudget { needed, budget, .. }) => {
            assert_eq!((needed, budget), ((300 << 20) + 1, 300 << 20));
        }
        other => panic!("expected OverBudget, got {other:?}"),
    }
}

/// A buffer that fails to allocate is an error, not a panic: a mapped one
/// is refused before mapping, and writing to one fails the next check.
#[test]
fn a_buffer_that_cannot_be_made_is_an_error() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    ctx.check().expect("clean start");
    let Some(size) = ctx.device.limits().max_buffer_size.checked_add(wgpu::COPY_BUFFER_ALIGNMENT)
    else {
        return;
    };
    let mapped = ctx.create_mapped_buffer(&wgpu::BufferDescriptor {
        label: Some("too_big_mapped"),
        size,
        usage: wgpu::BufferUsages::MAP_WRITE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: true,
    });
    assert!(mapped.is_err(), "a buffer past the limit was made");
    // The scope caught it: nothing is left for the next check.
    ctx.check().expect("the mapped buffer's error was returned, not recorded");

    // What `create_buffer_with_data` does, on a buffer that failed.
    let invalid = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("too_big"),
        size,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    ctx.queue.write_buffer(&invalid, 0, &[0; 16]);
    assert!(ctx.check().is_err(), "the failed buffer went unreported");
}

/// Every GPU [`GpuContext::devices`] lists can be chosen by name, and
/// Software is the CPU renderer.
#[test]
fn each_listed_device_can_be_chosen() {
    use deptheos_gpu::DeviceChoice;

    if std::env::var_os("DEPTHEOS_SOFTWARE_GPU").is_some() {
        return; // It overrides every choice.
    }
    for device in GpuContext::devices() {
        let ctx = GpuContext::with_device(&DeviceChoice::Named(device.name.clone()))
            .unwrap_or_else(|e| panic!("{}: {e}", device.name));
        assert_eq!(ctx.device_info(), device);
        eprintln!("chose {} ({:?})", ctx.device_info().name, ctx.device_info().kind());
        // A 179 MB stack-sized texture fits, even on an integrated GPU.
        let _texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("stack_sized"),
            size: wgpu::Extent3d { width: 8192, height: 5464, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        });
        ctx.check().unwrap_or_else(|e| panic!("{}: {e}", device.name));
    }
    match GpuContext::with_device(&DeviceChoice::Software) {
        Ok(ctx) => assert!(ctx.device_info().is_software(), "{:?}", ctx.device_info()),
        // No CPU renderer (some Linux and macOS machines).
        Err(deptheos_gpu::GpuError::NoAdapter) => {}
        Err(e) => panic!("software: {e}"),
    }
    // A GPU this machine does not have is the fastest instead.
    let missing = GpuContext::with_device(&DeviceChoice::Named("No Such GPU 9000".into()));
    if let (Ok(missing), Some(fastest)) = (missing, GpuContext::devices().first()) {
        assert_eq!(&missing.device_info().name, &fastest.name);
    }
}

/// An error on one context fails only that context's next check, even
/// when another context's work runs on the same thread.
#[test]
fn an_error_fails_only_its_own_context() {
    let Some(a) = GpuContext::new_or_skip() else { return };
    let Some(b) = GpuContext::new_or_skip() else { return };
    let Some(size) = a.device.limits().max_buffer_size.checked_add(wgpu::COPY_BUFFER_ALIGNMENT)
    else {
        return;
    };
    let invalid = a.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("too_big"),
        size,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    a.queue.write_buffer(&invalid, 0, &[0; 16]);
    b.check().expect("b did nothing wrong");
    b.clear_fault();
    assert!(a.check().is_err(), "a's error is a's");
    a.check().expect("returned once");
}

/// `programs` builds each type once per context and hands every caller
/// the same one; another context builds its own.
#[test]
fn programs_are_made_once_per_context() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    struct Kernel(u32);
    let mut built = 0;
    let first = ctx.programs(|_| {
        built += 1;
        Kernel(7)
    });
    let again = ctx.programs(|_| {
        built += 1;
        Kernel(8)
    });
    assert_eq!(built, 1);
    assert!(std::sync::Arc::ptr_eq(&first, &again));
    assert_eq!(again.0, 7);
    // Another type is its own entry.
    assert_eq!(*ctx.programs(|_| 3u8), 3);
    assert_eq!(ctx.programs(|_| Kernel(9)).0, 7);
}
