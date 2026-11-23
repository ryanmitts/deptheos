//! CPU→GPU image transfer: raw bytes via storage buffer + compute unpack.
//!
//! Two phases, one frame at a time:
//!   1. [`ImageUploader::prepare`] (or `prepare_frame`) writes the frame's
//!      bytes into the uploader's buffer and returns a [`PreparedUpload`]
//!      that borrows the uploader, so no other frame is prepared over it.
//!   2. [`PreparedUpload::dispatch_to_rgba`] records the unpack on the
//!      caller's encoder, using the upload up.
//!
//! The CPU copy of phase 1 (~25 ms at 45 MP RGB8) can run on the decode
//! thread instead: [`StagedUpload`] holds it, and preparing it records a
//! GPU buffer copy, or nothing ([`StagedFrames::InPlace`]).
//!
//! A stacker unpacks into a [`Source`] whose format follows its frames'
//! [`FrameDepth`].

use deptheos_gpu::gpu_utils::{
    WORKGROUP_SIZE, create_bind_group, create_compute_pipeline, dispatch_compute_with, tex,
};
use deptheos_gpu::{GpuContext, GpuError, StagedFrames};
use image::DynamicImage;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use wgpu::{ComputePipeline, Texture};

use crate::error::{CoreError, Result};

// A module so the allow covers the dead layout-check fns encase generates.
mod params {
    #![allow(dead_code)]

    #[derive(Clone, Copy, encase::ShaderType)]
    pub(super) struct UploadParams {
        pub(super) width: u32,
        pub(super) channels: u32,
        pub(super) bytes_per_channel: u32,
    }
}
use params::UploadParams;

/// How many bits a frame's channels carry, which a stacker's source must
/// hold exactly (ADR-110). Ordered by depth.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FrameDepth {
    Eight,
    Sixteen,
}

impl FrameDepth {
    /// Bytes of an RGB frame of `pixels` at this depth, decoded or staged.
    pub const fn rgb_bytes(self, pixels: u64) -> u64 {
        match self {
            Self::Eight => pixels * 3,
            Self::Sixteen => pixels * 6,
        }
    }

    /// A stacker's source format for this depth, holding values exactly:
    /// Rgba8Unorm (which every adapter stores), else the context's
    /// (Rgba16Unorm, or Rgba32Float where that can't be stored).
    pub fn source_format(self, ctx: &GpuContext) -> wgpu::TextureFormat {
        match self {
            Self::Eight => wgpu::TextureFormat::Rgba8Unorm,
            Self::Sixteen => ctx.source_format(),
        }
    }

    /// GPU bytes of a source of `pixels` at this depth, with Metal's 1/16
    /// padding, for memory estimates.
    pub fn source_bytes(self, ctx: &GpuContext, pixels: u64) -> u64 {
        let texel = u64::from(self.source_format(ctx).block_copy_size(None).unwrap_or(16));
        let bytes = pixels * texel;
        bytes + bytes / 16
    }
}

/// How an upload's interleaved pixels are laid out: 3 or 4 channels of 1 or
/// 2 bytes (little-endian, as the unpack shader reads them).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PixelLayout {
    width: u32,
    height: u32,
    channels: u32,
    bytes_per_channel: u32,
}

impl PixelLayout {
    fn of(image: &DynamicImage) -> Result<(&[u8], Self)> {
        let (width, height) = (image.width(), image.height());
        let (bytes, channels, bytes_per_channel): (&[u8], u32, u32) = match image {
            DynamicImage::ImageRgb8(buf) => (buf.as_raw(), 3, 1),
            DynamicImage::ImageRgba8(buf) => (buf.as_raw(), 4, 1),
            DynamicImage::ImageRgb16(buf) => (bytemuck::cast_slice(buf.as_raw()), 3, 2),
            DynamicImage::ImageRgba16(buf) => (bytemuck::cast_slice(buf.as_raw()), 4, 2),
            other => {
                return Err(CoreError::PixelLayout(format!("{:?}", other.color())));
            }
        };
        Ok((bytes, Self { width, height, channels, bytes_per_channel }))
    }

    /// Validate an upload of `len` bytes and return it padded to the
    /// multiple of 4 buffer writes need (the tail zeroed).
    fn check(&self, ctx: &GpuContext, len: usize) -> Result<wgpu::BufferSize> {
        let Self { width, height, channels, bytes_per_channel } = *self;
        if !matches!(channels, 3 | 4) || !matches!(bytes_per_channel, 1 | 2) {
            return Err(CoreError::InvalidState(format!(
                "upload of {channels} channels × {bytes_per_channel} bytes"
            )));
        }
        let expected = width as u64 * height as u64 * channels as u64 * bytes_per_channel as u64;
        if len as u64 != expected {
            return Err(CoreError::InvalidState(format!(
                "upload buffer has {len} bytes, expected {expected} for {width}×{height}"
            )));
        }
        ctx.check_frame_fits(width, height, len as u64)?;
        wgpu::BufferSize::new((len as u64 + 3) & !3).ok_or(CoreError::EmptyImage)
    }

    fn depth(&self) -> FrameDepth {
        if self.bytes_per_channel == 2 { FrameDepth::Sixteen } else { FrameDepth::Eight }
    }

    fn params(&self) -> UploadParams {
        UploadParams {
            width: self.width,
            channels: self.channels,
            bytes_per_channel: self.bytes_per_channel,
        }
    }
}

/// A prepared frame, to unpack with [`Self::dispatch_to_rgba`]. It borrows
/// the uploader, so no other frame is prepared over it.
pub struct PreparedUpload<'a> {
    uploader: &'a ImageUploader,
    bytes: PreparedBytes<'a>,
    depth: FrameDepth,
    pub width: u32,
    pub height: u32,
}

/// Where a prepared frame's bytes are for the unpack to read.
enum PreparedBytes<'a> {
    /// Written into the uploader's buffer.
    Written,
    /// In a staging buffer, copied into the uploader's on the caller's
    /// encoder first ([`StagedFrames::Copied`]).
    Copy { staged: &'a wgpu::Buffer, size: u64 },
    /// In a staging buffer the unpack reads itself
    /// ([`StagedFrames::InPlace`]).
    InPlace(&'a wgpu::Buffer),
}

impl PreparedUpload<'_> {
    /// Unpack the prepared frame into a storage texture
    /// ([`ImageUploader::create_target`], or a stacker's [`Source`]).
    pub fn dispatch_to_rgba(
        self,
        ctx: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        target: &Texture,
    ) {
        let uploader = self.uploader;
        let pipeline = match target.format() {
            wgpu::TextureFormat::Rgba8Unorm => {
                // A 16-bit frame there would lose its low bits silently.
                assert_eq!(self.depth, FrameDepth::Eight, "a deeper frame than its target holds");
                &uploader.unpack.rgba8
            }
            _ => &uploader.unpack.rgba,
        };
        let written = || uploader.buffer.as_ref().expect("a prepared frame is in the buffer");
        let buffer = match self.bytes {
            PreparedBytes::Written => written(),
            PreparedBytes::Copy { staged, size } => {
                let buffer = written();
                encoder.copy_buffer_to_buffer(staged, 0, buffer, 0, size);
                buffer
            }
            PreparedBytes::InPlace(staged) => staged,
        };
        // Per frame: a buffer read in place differs each time.
        let params = create_bind_group(
            &ctx.device,
            pipeline,
            0,
            "upload_params_bg",
            &[uploader.params_buffer.as_entire_binding(), buffer.as_entire_binding()],
        );
        let target_view = target.create_view(&Default::default());
        let target =
            create_bind_group(&ctx.device, pipeline, 1, "upload_target_bg", &[tex(&target_view)]);
        dispatch_compute_with(
            encoder,
            pipeline,
            &[&params, &target],
            "upload_unpack",
            self.width,
            self.height,
            WORKGROUP_SIZE,
        );
    }
}

pub struct ImageUploader {
    /// The buffer frames are written or copied into, kept between uploads;
    /// unused where staged frames are read in place.
    buffer: Option<wgpu::Buffer>,
    /// The unpack, built once per device ([`GpuContext::programs`]).
    unpack: Arc<UnpackKernel>,
    /// The unpack's uniform, rewritten per frame.
    params_buffer: wgpu::Buffer,
}

/// The unpack into `GpuContext::source_format()`, and into Rgba8Unorm for
/// a [`Source`] of 8-bit frames.
struct UnpackKernel {
    rgba: ComputePipeline,
    rgba8: ComputePipeline,
}

impl UnpackKernel {
    fn new(ctx: &GpuContext) -> Self {
        let source = include_str!("upload.wgsl");
        let module = ctx.create_shader_module(source, "upload");
        let rgba8_module = ctx.create_shader_module_with(source, "upload_rgba8", &["TARGET_RGBA8"]);
        Self {
            rgba: create_compute_pipeline(&ctx.device, &module, "unpack_to_rgba"),
            rgba8: create_compute_pipeline(&ctx.device, &rgba8_module, "unpack_to_rgba"),
        }
    }
}

impl ImageUploader {
    pub fn new(ctx: &GpuContext) -> Self {
        let params_buffer = ctx.create_uniform_buffer(
            &UploadParams { width: 0, channels: 0, bytes_per_channel: 0 },
            "upload_params",
        );
        Self { buffer: None, unpack: ctx.programs(UnpackKernel::new), params_buffer }
    }

    /// The buffer, at least `size` bytes (a multiple of 4), reused while
    /// under twice `size` so one large upload doesn't pin it. A failed
    /// allocation isn't kept, so the next upload tries again.
    fn buffer(&mut self, ctx: &GpuContext, size: u64) -> Result<&wgpu::Buffer> {
        if self.buffer.as_ref().is_none_or(|b| b.size() < size || b.size() / 2 > size) {
            // Drop the old buffer first, so its memory is free for the new one.
            self.buffer = None;
            let buffer = ctx.create_storage_buffer(
                "image_upload_buffer",
                size,
                wgpu::BufferUsages::COPY_DST,
            );
            ctx.check()?;
            self.buffer = Some(buffer);
        }
        Ok(self.buffer.as_ref().expect("allocated above"))
    }

    /// Upload raw image bytes to the GPU staging buffer.
    pub fn prepare(
        &mut self,
        ctx: &GpuContext,
        image: &DynamicImage,
    ) -> Result<PreparedUpload<'_>> {
        let (bytes, layout) = PixelLayout::of(image)?;
        self.prepare_bytes(ctx, bytes, layout)
    }

    /// [`Self::prepare`] for an interleaved RGB u16 slice.
    pub fn prepare_rgb16(
        &mut self,
        ctx: &GpuContext,
        pixels: &[u16],
        width: u32,
        height: u32,
    ) -> Result<PreparedUpload<'_>> {
        let layout = PixelLayout { width, height, channels: 3, bytes_per_channel: 2 };
        self.prepare_bytes(ctx, bytemuck::cast_slice(pixels), layout)
    }

    fn prepare_bytes(
        &mut self,
        ctx: &GpuContext,
        bytes: &[u8],
        layout: PixelLayout,
    ) -> Result<PreparedUpload<'_>> {
        let aligned_len = layout.check(ctx, bytes.len())?;
        let buffer = self.buffer(ctx, aligned_len.get())?;
        // Refused when the buffer or device has failed since.
        let Some(mut view) = ctx.queue.write_buffer_with(buffer, 0, aligned_len) else {
            self.buffer = None;
            ctx.check()?;
            return Err(GpuError::Internal("the GPU refused the frame's upload".into()).into());
        };
        view.slice(..bytes.len()).copy_from_slice(bytes);
        view.slice(bytes.len()..).fill(0);
        drop(view);
        ctx.write_uniform_buffer(&self.params_buffer, &layout.params());
        Ok(PreparedUpload {
            uploader: self,
            bytes: PreparedBytes::Written,
            depth: layout.depth(),
            width: layout.width,
            height: layout.height,
        })
    }

    /// [`Self::prepare`] for a frame already in GPU-visible memory: read in
    /// place, or copied by [`PreparedUpload::dispatch_to_rgba`].
    fn prepare_staged<'a>(
        &'a mut self,
        ctx: &GpuContext,
        staged: &'a StagedUpload,
    ) -> Result<PreparedUpload<'a>> {
        let bytes = match ctx.staged_frames() {
            StagedFrames::InPlace => PreparedBytes::InPlace(&staged.buffer),
            StagedFrames::Copied => {
                let size = staged.buffer.size();
                self.buffer(ctx, size)?;
                PreparedBytes::Copy { staged: &staged.buffer, size }
            }
        };
        let layout = &staged.layout;
        ctx.write_uniform_buffer(&self.params_buffer, &layout.params());
        Ok(PreparedUpload {
            uploader: self,
            bytes,
            depth: layout.depth(),
            width: layout.width,
            height: layout.height,
        })
    }

    /// [`Self::prepare`] or the staged copy, whichever `frame` is.
    pub fn prepare_frame<'a>(
        &'a mut self,
        ctx: &GpuContext,
        frame: FrameInput<'a>,
    ) -> Result<PreparedUpload<'a>> {
        match frame {
            FrameInput::Image(image) => self.prepare(ctx, image),
            FrameInput::Staged(staged) => self.prepare_staged(ctx, staged),
        }
    }

    /// Allocate a target for [`PreparedUpload::dispatch_to_rgba`] in the format the
    /// unpack shader was compiled for (`GpuContext::source_format()`).
    pub fn create_target(ctx: &GpuContext, width: u32, height: u32, label: &str) -> Texture {
        ctx.create_texture(
            width,
            height,
            ctx.source_format(),
            wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            label,
        )
    }

    pub fn release_resources(&mut self) {
        self.buffer = None;
    }
}

/// A stacker's source texture, each frame unpacked into it and warped from
/// there, at the stack's size and depth ([`FrameDepth::source_format`]),
/// decided from the headers. A deeper frame is refused ([`Self::check`]),
/// never narrowed.
pub(crate) struct Source {
    texture: Texture,
    depth: FrameDepth,
}

impl Source {
    pub(crate) fn new(
        ctx: &GpuContext,
        width: u32,
        height: u32,
        depth: FrameDepth,
        label: &'static str,
    ) -> Self {
        let texture = ctx.create_texture(
            width,
            height,
            depth.source_format(ctx),
            wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            label,
        );
        Self { texture, depth }
    }

    pub(crate) fn texture(&self) -> &Texture {
        &self.texture
    }

    /// Whether `frame` fits: an error for another size
    /// ([`CoreError::FrameSize`]) or a deeper frame (a file changed after
    /// its header was read).
    pub(crate) fn check(&self, frame: &FrameInput) -> Result<()> {
        let size = (self.texture.width(), self.texture.height());
        crate::error::check_frame_size((frame.width(), frame.height()), size)?;
        let depth = frame.depth();
        if depth > self.depth {
            return Err(CoreError::InvalidState(format!(
                "a {depth:?}-bit frame in a stack sized for {:?}-bit frames",
                self.depth
            )));
        }
        Ok(())
    }
}

/// A frame's bytes in a GPU-visible staging buffer, for
/// [`ImageUploader::prepare_frame`]. Made on the decode thread, so the copy
/// is off the thread feeding the GPU.
pub struct StagedUpload {
    buffer: wgpu::Buffer,
    layout: PixelLayout,
    /// Where the buffer goes back to once the GPU has copied out of it.
    pool: Option<StagingPool>,
}

impl StagedUpload {
    /// Stage `image` in a buffer from `pool` (which may wait), returned on
    /// drop, or in its own buffer without one.
    pub fn with_pool(
        ctx: &GpuContext,
        image: &DynamicImage,
        pool: Option<&StagingPool>,
    ) -> Result<Self> {
        let (bytes, layout) = PixelLayout::of(image)?;
        let aligned_len = layout.check(ctx, bytes.len())?;
        let buffer = match pool {
            Some(pool) => pool.take(ctx, aligned_len.get())?,
            None => StagingPool::create(ctx, aligned_len.get())?,
        };
        {
            let mut view = buffer.slice(..).get_mapped_range_mut();
            view.slice(..bytes.len()).copy_from_slice(bytes);
            view.slice(bytes.len()..).fill(0);
        }
        buffer.unmap();
        Ok(Self { buffer, layout, pool: pool.cloned() })
    }

    pub fn width(&self) -> u32 {
        self.layout.width
    }

    pub fn height(&self) -> u32 {
        self.layout.height
    }

    /// How many bits the frame's channels carry.
    pub fn depth(&self) -> FrameDepth {
        self.layout.depth()
    }

    /// Bytes of the staging buffer this frame holds.
    pub fn byte_len(&self) -> u64 {
        self.buffer.size()
    }
}

impl Drop for StagedUpload {
    /// Remap the buffer, which completes once the GPU is done with it, then
    /// hand it back to the pool.
    fn drop(&mut self) {
        let Some(pool) = self.pool.take() else { return };
        let buffer = self.buffer.clone();
        self.buffer.slice(..).map_async(wgpu::MapMode::Write, move |mapped| {
            if mapped.is_ok() {
                pool.put(buffer);
            } else {
                // The device is gone; the buffer with it.
                pool.forget();
            }
        });
    }
}

/// Staging buffers recycled between frames, at most `capacity` in all: a
/// fresh frame-sized mappable buffer is expensive (zero-filled page by
/// page, about twice the staging time on Windows). Buffers come back
/// mapped once the GPU has copied out of them.
///
/// The cap bounds memory when decoding outruns the GPU (270–600 MB a frame
/// at 45–100 MP): with all out, [`Self::take`] waits for the GPU. A session
/// sizes the pool above the frames it holds itself, so the wait ends.
#[derive(Clone)]
pub struct StagingPool(Arc<StagingPoolInner>);

struct StagingPoolInner {
    state: Mutex<PoolState>,
    /// Signaled when a buffer comes back.
    returned: Condvar,
    capacity: usize,
}

#[derive(Default)]
struct PoolState {
    /// Buffers back from the GPU, ready to write.
    free: Vec<wgpu::Buffer>,
    /// Buffers handed out and not back yet.
    out: usize,
}

impl StagingPool {
    pub fn new(capacity: usize) -> Self {
        Self(Arc::new(StagingPoolInner {
            state: Mutex::new(PoolState::default()),
            returned: Condvar::new(),
            capacity: capacity.max(1),
        }))
    }

    /// A mapped `size`-byte buffer: a free one, a new one under `capacity`,
    /// or the next the GPU finishes with. The wait polls the device itself,
    /// since buffers come back only from a poll.
    fn take(&self, ctx: &GpuContext, size: u64) -> Result<wgpu::Buffer> {
        let inner = &*self.0;
        let mut state = inner.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            // On a lost device nothing comes back, and mapping a buffer
            // made on it would panic.
            ctx.check_lost()?;
            // A stack's frames share a size; drop others' buffers.
            state.free.retain(|b| b.size() == size);
            if let Some(buffer) = state.free.pop() {
                state.out += 1;
                return Ok(buffer);
            }
            if state.out < inner.capacity {
                state.out += 1;
                drop(state);
                let created = Self::create(ctx, size);
                if created.is_err() {
                    self.forget();
                }
                return created;
            }
            drop(state);
            ctx.wait_all()?;
            state = inner.state.lock().unwrap_or_else(|e| e.into_inner());
            // None came back from the poll: wait briefly for another's.
            if state.free.is_empty() && state.out >= inner.capacity {
                state = inner
                    .returned
                    .wait_timeout(state, Duration::from_millis(5))
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
        }
    }

    /// A new mapped staging buffer of `size` bytes (a storage buffer too
    /// where frames are read in place).
    fn create(ctx: &GpuContext, size: u64) -> Result<wgpu::Buffer> {
        Ok(ctx.create_mapped_buffer(&wgpu::BufferDescriptor {
            label: Some("staged_upload"),
            size,
            usage: ctx.staged_frames().staging_usage(),
            mapped_at_creation: true,
        })?)
    }

    fn put(&self, buffer: wgpu::Buffer) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.out = state.out.saturating_sub(1);
        state.free.push(buffer);
        self.0.returned.notify_one();
    }

    /// A buffer that will not come back.
    fn forget(&self) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.out = state.out.saturating_sub(1);
        self.0.returned.notify_one();
    }

    /// Free the idle buffers once no more will be staged; those still out
    /// come back as usual.
    pub fn release_idle(&self) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.free.clear();
    }

    /// Buffers handed out and not back yet.
    pub fn out(&self) -> usize {
        self.0.state.lock().unwrap_or_else(|e| e.into_inner()).out
    }
}

/// A frame as the stackers take it: a decoded image, or one already staged
/// for upload.
#[derive(Clone, Copy)]
pub enum FrameInput<'a> {
    Image(&'a DynamicImage),
    Staged(&'a StagedUpload),
}

impl FrameInput<'_> {
    pub fn width(&self) -> u32 {
        match self {
            FrameInput::Image(image) => image.width(),
            FrameInput::Staged(staged) => staged.width(),
        }
    }

    pub fn height(&self) -> u32 {
        match self {
            FrameInput::Image(image) => image.height(),
            FrameInput::Staged(staged) => staged.height(),
        }
    }

    /// How many bits the frame's channels carry; 8 for an image the uploader
    /// can't take, so preparing it is the error.
    pub fn depth(&self) -> FrameDepth {
        match self {
            FrameInput::Image(image) => {
                PixelLayout::of(image).map_or(FrameDepth::Eight, |(_, layout)| layout.depth())
            }
            FrameInput::Staged(staged) => staged.depth(),
        }
    }
}

impl<'a> From<&'a DynamicImage> for FrameInput<'a> {
    fn from(image: &'a DynamicImage) -> Self {
        FrameInput::Image(image)
    }
}

impl<'a> From<&'a StagedUpload> for FrameInput<'a> {
    fn from(staged: &'a StagedUpload) -> Self {
        FrameInput::Staged(staged)
    }
}
