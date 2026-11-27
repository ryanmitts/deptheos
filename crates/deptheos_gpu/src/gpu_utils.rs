//! Shared GPU utilities: bind groups, dispatch helpers, clears, downloads.

use crate::context::GpuContext;
use crate::error::{GpuError, Result};
use wgpu::{ComputePipeline, Texture};

/// Workgroup size of the compute shaders (16x16 threads) unless they say
/// otherwise; tiled kernels size theirs to their workgroup-memory tile.
pub const WORKGROUP_SIZE: u32 = 16;

/// Shorthand for `wgpu::BindingResource::TextureView(view)`.
#[inline]
pub fn tex<'a>(view: &'a wgpu::TextureView) -> wgpu::BindingResource<'a> {
    wgpu::BindingResource::TextureView(view)
}

/// Create a bind group with bindings numbered 0, 1, 2, ... in order.
///
/// Only the order links an entry to its `@binding(n)`, and wgpu doesn't
/// catch two of one type swapped, so in a group of more than five entries
/// each line names its WGSL variable in a trailing comment.
pub fn create_bind_group(
    device: &wgpu::Device,
    pipeline: &ComputePipeline,
    group: u32,
    label: &str,
    resources: &[wgpu::BindingResource],
) -> wgpu::BindGroup {
    let entries: Vec<wgpu::BindGroupEntry> = resources
        .iter()
        .enumerate()
        .map(|(i, resource)| wgpu::BindGroupEntry { binding: i as u32, resource: resource.clone() })
        .collect();

    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout: &pipeline.get_bind_group_layout(group),
        entries: &entries,
    })
}

/// Create a compute pipeline from a shader module entry point (auto-layout).
pub fn create_compute_pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    entry_point: &str,
) -> ComputePipeline {
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(entry_point),
        layout: None,
        module,
        entry_point: Some(entry_point),
        compilation_options: Default::default(),
        cache: None,
    })
}

/// [`create_compute_pipeline`] without WebGPU's zero-fill of
/// `var<workgroup>` memory, which made a 12.5 KB tile kernel 10× slower on
/// Metal. Only for kernels that write every workgroup entry they read,
/// before a `workgroupBarrier()`.
pub fn create_tiled_compute_pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    entry_point: &str,
) -> ComputePipeline {
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(entry_point),
        layout: None,
        module,
        entry_point: Some(entry_point),
        compilation_options: wgpu::PipelineCompilationOptions {
            zero_initialize_workgroup_memory: false,
            ..Default::default()
        },
        cache: None,
    })
}

/// Begin a compute pass, set pipeline + bind group, dispatch over width×height.
pub fn dispatch_compute(
    encoder: &mut wgpu::CommandEncoder,
    pipeline: &ComputePipeline,
    bind_group: &wgpu::BindGroup,
    label: &str,
    width: u32,
    height: u32,
) {
    dispatch_compute_with(encoder, pipeline, &[bind_group], label, width, height, WORKGROUP_SIZE);
}

/// [`dispatch_compute`] for a pipeline with several bind groups (set at
/// group indices 0, 1, ... in order) or a shader whose `@workgroup_size` is
/// `group`×`group` rather than [`WORKGROUP_SIZE`].
pub fn dispatch_compute_with(
    encoder: &mut wgpu::CommandEncoder,
    pipeline: &ComputePipeline,
    bind_groups: &[&wgpu::BindGroup],
    label: &str,
    width: u32,
    height: u32,
    group: u32,
) {
    let timer = crate::profile::begin_pass(encoder, label);
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some(label),
            timestamp_writes: timer.compute(),
        });
        pass.set_pipeline(pipeline);
        for (index, bind_group) in bind_groups.iter().enumerate() {
            pass.set_bind_group(index as u32, *bind_group, &[]);
        }
        pass.dispatch_workgroups(width.div_ceil(group), height.div_ceil(group), 1);
    }
    timer.end(encoder);
}

/// Create a bind group from textures (+ optional extra bindings), then dispatch.
#[allow(clippy::too_many_arguments)]
pub fn dispatch_compute_textures(
    encoder: &mut wgpu::CommandEncoder,
    device: &wgpu::Device,
    pipeline: &ComputePipeline,
    label: &str,
    textures: &[&Texture],
    extra_bindings: &[wgpu::BindingResource],
    width: u32,
    height: u32,
) {
    let views: Vec<wgpu::TextureView> =
        textures.iter().map(|t| t.create_view(&Default::default())).collect();

    let mut resources: Vec<wgpu::BindingResource> = views.iter().map(|v| tex(v)).collect();
    resources.extend_from_slice(extra_bindings);

    let bg = create_bind_group(device, pipeline, 0, label, &resources);
    dispatch_compute(encoder, pipeline, &bg, label, width, height);
}

/// Hardware clear via render-pass load op. The texture must have
/// RENDER_ATTACHMENT usage.
pub fn encode_clear(encoder: &mut wgpu::CommandEncoder, texture: &Texture, value: wgpu::Color) {
    let view = texture.create_view(&Default::default());
    let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("clear_pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &view,
            resolve_target: None,
            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(value), store: wgpu::StoreOp::Store },
            depth_slice: None,
        })],
        depth_stencil_attachment: None,
        ..Default::default()
    });
}

/// Map a buffer for reading and block until the mapping is ready.
///
/// The poll can return before the callback runs (another thread's poll
/// collected it), so the callback is waited for too.
fn map_read_blocking(
    device: &wgpu::Device,
    buffer_slice: &wgpu::BufferSlice<'_>,
    submission_index: Option<wgpu::SubmissionIndex>,
) -> Result<()> {
    let (sender, receiver) = std::sync::mpsc::channel();
    buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });

    let timeout = Some(crate::WAIT_LIMIT);
    device.poll(wgpu::PollType::Wait { submission_index, timeout }).map_err(|e| match e {
        wgpu::PollError::Timeout => GpuError::TimedOut(crate::WAIT_LIMIT),
        e => GpuError::DeviceLost(e.to_string()),
    })?;

    use std::sync::mpsc::RecvTimeoutError;
    receiver
        .recv_timeout(crate::WAIT_LIMIT)
        .map_err(|e| match e {
            RecvTimeoutError::Timeout => GpuError::TimedOut(crate::WAIT_LIMIT),
            RecvTimeoutError::Disconnected => {
                GpuError::BufferMappingFailed("map callback was dropped".into())
            }
        })?
        .map_err(|e| GpuError::BufferMappingFailed(format!("{e:?}")))
}

/// The first `size` bytes of `buffer` (made with `COPY_SRC`), read back
/// once the queue reaches them.
pub fn download_buffer(ctx: &GpuContext, buffer: &wgpu::Buffer, size: u64) -> Result<Vec<u8>> {
    let staging = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("download_buffer"),
        size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = ctx.encoder("download_buffer");
    encoder.copy_buffer_to_buffer(buffer, 0, &staging, 0, size);
    let sub = ctx.submit(encoder);
    let slice = staging.slice(..);
    let mapped = map_read_blocking(&ctx.device, &slice, Some(sub));
    ctx.check()?;
    mapped?;
    let bytes = slice.get_mapped_range().to_vec();
    staging.unmap();
    Ok(bytes)
}

/// Bytes per texel of an uncompressed single-plane format; an error for
/// block-compressed and depth/stencil formats.
pub(crate) fn texel_bytes(format: wgpu::TextureFormat) -> Result<u32> {
    match format.block_copy_size(None) {
        Some(bpp) if format.block_dimensions() == (1, 1) => Ok(bpp),
        _ => Err(GpuError::Unsupported(format!("download format {format:?}"))),
    }
}

/// Read back the `size` region of `texture` at `origin`, in its own format,
/// handing each packed row, top to bottom, to `row` straight from the
/// mapped staging buffer: no full-size copy. Rows are read once, in order,
/// since on discrete GPUs the mapping is uncached.
pub fn download_texture_with(
    ctx: &GpuContext,
    texture: &Texture,
    origin: [u32; 2],
    size: [u32; 2],
    mut row: impl FnMut(&[u8]),
) -> Result<()> {
    download_texture_rows(ctx, texture, origin, size, |_, mapped, stride, row_bytes| {
        for padded in mapped.chunks_exact(stride) {
            row(&padded[..row_bytes]);
        }
    })
}

/// Bytes in each of a readback's two staging buffers: the region is read
/// in bands this size or less, one copied while the other is read, rather
/// than one buffer the region's size (800 MB for 100 MP Rgba16Unorm).
const READBACK_BAND: u64 = 32 << 20;

/// [`download_texture_with`] a band at a time, for converting in parallel:
/// `rows` gets the band's first row (from the region's top), its mapped
/// bytes, the row `stride` and `row_bytes`. Bands come top to bottom.
pub fn download_texture_rows(
    ctx: &GpuContext,
    texture: &Texture,
    origin: [u32; 2],
    size: [u32; 2],
    mut rows: impl FnMut(u32, &[u8], usize, usize),
) -> Result<()> {
    let format = texture.format();
    let bpp = texel_bytes(format)?;
    let [x, y] = origin;
    let [width, height] = size;
    let fits = |o: u32, n: u32, limit: u32| o.checked_add(n).is_some_and(|end| end <= limit);
    if width == 0
        || height == 0
        || !fits(x, width, texture.width())
        || !fits(y, height, texture.height())
    {
        return Err(GpuError::Unsupported(format!(
            "download region {width}x{height}+{x}+{y} outside a {}x{} texture",
            texture.width(),
            texture.height()
        )));
    }
    // u64 throughout: 16384² rgba32f is 4 GiB, past u32.
    let row_bytes = u64::from(width) * u64::from(bpp);
    let padded_row_bytes =
        u32::try_from(row_bytes.next_multiple_of(u64::from(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)))
            .map_err(|_| GpuError::Unsupported(format!("{width} px row of {format:?}")))?;
    let band_rows =
        (READBACK_BAND / u64::from(padded_row_bytes)).clamp(1, u64::from(height)) as u32;
    let bands: Vec<(u32, u32)> = (0..height)
        .step_by(band_rows as usize)
        .map(|top| (top, band_rows.min(height - top)))
        .collect();
    let staging: Vec<wgpu::Buffer> = (0..bands.len().min(2))
        .map(|_| {
            ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("download_staging"),
                size: u64::from(padded_row_bytes) * u64::from(band_rows),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        })
        .collect();
    // Copy band `i` into its staging buffer.
    let copy = |i: usize| {
        let (top, count) = bands[i];
        let mut encoder = ctx.encoder("download_encoder");
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y: y + top, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &staging[i % 2],
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row_bytes),
                    rows_per_image: Some(count),
                },
            },
            wgpu::Extent3d { width, height: count, depth_or_array_layers: 1 },
        );
        ctx.submit(encoder)
    };

    let mut next = Some(copy(0));
    let mut arrived = 0;
    for (i, &(top, count)) in bands.iter().enumerate() {
        let sub = next.take();
        // The next band copies while this one is read.
        if i + 1 < bands.len() {
            next = Some(copy(i + 1));
        }
        let slice = staging[i % 2].slice(..u64::from(padded_row_bytes) * u64::from(count));
        let mapped = map_read_blocking(&ctx.device, &slice, sub);
        // Checked before the mapping's result, which on a lost device or a
        // failed allocation says only that the mapping failed.
        ctx.check()?;
        mapped?;
        let view = slice.get_mapped_range();
        let whole = view.len() / padded_row_bytes as usize;
        if whole != count as usize {
            let arrived = arrived + whole as u32;
            return Err(GpuError::ShortReadback { arrived, rows: height });
        }
        rows(top, &view, padded_row_bytes as usize, row_bytes as usize);
        arrived += count;
        drop(view);
        staging[i % 2].unmap();
    }
    if arrived != height {
        return Err(GpuError::ShortReadback { arrived, rows: height });
    }
    Ok(())
}
