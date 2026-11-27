//! The one GPU device ([`GpuContext`]): made on the adapter chosen
//! (`adapter`), with its faults and loss, its formats and resource helpers;
//! its memory budget is in `memory`, its shader composition in `shaders`.

use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tracing::info;
use wgpu::{Device, Queue};

use crate::error::{FrameLimit, GpuError, Result};

mod adapter;
mod memory;
mod pool;
mod shaders;

pub use adapter::{DeviceChoice, DeviceInfo, DeviceKind};
use memory::{BudgetSource, d3d12_memory, gigabytes};
pub use memory::{MemoryBudget, MemoryPool};
pub use pool::{Drawing, POOL_LIMIT, PooledTexture, trim_pools};

/// Optional storage-texture formats this adapter supports, each with a
/// fallback picked by the `*_format()` helpers on [`GpuContext`]; GPU
/// arithmetic is f32 either way.
#[derive(Debug, Clone, Copy)]
pub struct FormatSupport {
    /// Rgba16Unorm as a write-only storage texture.
    pub rgba16unorm_storage: bool,
    /// R8Unorm as a write-only storage texture: not a WebGPU guarantee but
    /// TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES (Metal reports it).
    pub r8_storage: bool,
}

/// How a frame staged in mappable memory (`deptheos_core::StagedUpload`)
/// reaches the shader that unpacks it, decided once per device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StagedFrames {
    /// Copied first into the uploader's own storage buffer. WebGPU's rule
    /// (a mappable buffer is for copying only), and right anyway on a GPU
    /// with its own memory, where a mappable buffer is slow system memory.
    Copied,
    /// Read by the unpack where it was staged, saving the copy: unified
    /// memory with `MAPPABLE_PRIMARY_BUFFERS`. Only on Apple GPUs, where it
    /// was measured.
    InPlace,
}

impl StagedFrames {
    /// `InPlace` on an integrated (unified-memory) Metal GPU with mappable
    /// storage buffers, else `Copied`.
    fn for_adapter(adapter: &wgpu::Adapter) -> Self {
        let info = adapter.get_info();
        if info.backend == wgpu::Backend::Metal
            && info.device_type == wgpu::DeviceType::IntegratedGpu
            && adapter.features().contains(wgpu::Features::MAPPABLE_PRIMARY_BUFFERS)
        {
            Self::InPlace
        } else {
            Self::Copied
        }
    }

    /// The usage a staging buffer is made with.
    pub fn staging_usage(self) -> wgpu::BufferUsages {
        let copy = wgpu::BufferUsages::MAP_WRITE | wgpu::BufferUsages::COPY_SRC;
        match self {
            Self::Copied => copy,
            Self::InPlace => copy | wgpu::BufferUsages::STORAGE,
        }
    }
}

/// The engine's one wgpu device (ADR-100): made at launch and held by the
/// engine; tests make their own.
pub struct GpuContext {
    pub device: Device,
    pub queue: Queue,
    pub adapter_info: wgpu::AdapterInfo,
    /// Timestamp queries for per-kernel GPU profiling (`profile`); optional.
    pub timestamp_queries: bool,
    pub format_support: FormatSupport,
    staged_frames: StagedFrames,
    budget: BudgetSource,
    /// Why the device was lost, once it has been ([`Self::check`]).
    lost: Arc<Mutex<Option<String>>>,
    /// Which context this is, in [`FAULT`].
    id: u64,
    /// What [`Self::programs`] has built on this device, by type.
    programs: Mutex<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>,
    /// Large textures kept for reuse (`pool`).
    pool: Arc<pool::TexturePool>,
}

static NEXT_CONTEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

thread_local! {
    /// The first GPU error this thread caused on each context since its
    /// last [`GpuContext::check`], by context id. wgpu reports an error on
    /// the calling thread, so a paint error cannot fail a stack beside it.
    static FAULT: RefCell<Vec<(u64, GpuError)>> = const { RefCell::new(Vec::new()) };
}

/// Take the error this thread caused on context `id`, if any.
fn take_fault(id: u64) -> Option<GpuError> {
    FAULT.with(|f| {
        let mut faults = f.borrow_mut();
        let ix = faults.iter().position(|(c, _)| *c == id)?;
        Some(faults.swap_remove(ix).1)
    })
}

/// Forget this thread's unchecked errors on every context, so they don't
/// fail the next job: called as a shared worker starts each job. A lost
/// device stays lost.
pub fn clear_thread_faults() {
    let stale = FAULT.with(|f| std::mem::take(&mut *f.borrow_mut()));
    for (_, fault) in stale {
        tracing::warn!("discarding an unchecked GPU error: {fault:?}");
    }
}

/// wgpu's error as a [`GpuError`], logged in full; the user is shown only
/// what the variant says.
fn fault_from(error: wgpu::Error) -> GpuError {
    tracing::error!("GPU error: {error}");
    match error {
        wgpu::Error::OutOfMemory { .. } => GpuError::OutOfMemory,
        other => GpuError::Internal(other.to_string()),
    }
}

impl GpuContext {
    /// A context on the fastest GPU.
    pub fn new() -> Result<Self> {
        Self::with_device(&DeviceChoice::Automatic)
    }

    /// The device this context runs on.
    pub fn device_info(&self) -> DeviceInfo {
        DeviceInfo {
            name: self.adapter_info.name.clone(),
            device_type: self.adapter_info.device_type,
        }
    }

    /// A context on the device `choice` means here. The stackers need no
    /// optional feature (ADR-110), so every adapter can stack, the iOS
    /// Simulator's included.
    pub fn with_device(choice: &DeviceChoice) -> Result<Self> {
        let adapter = Self::adapter(choice)?;
        let staged_frames = StagedFrames::for_adapter(&adapter);
        if let Some((budget, used)) = d3d12_memory(&adapter) {
            info!("D3D12 memory budget {} ({} in use)", gigabytes(budget), gigabytes(used));
        }

        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("deptheos device"),
                required_features: Self::build_required_features(&adapter, staged_frames),
                required_limits: Self::build_limits(&adapter),
                memory_hints: wgpu::MemoryHints::Performance,
                experimental_features: wgpu::ExperimentalFeatures::default(),
                trace: Default::default(),
            }))?;

        // wgpu's default handler panics. Record the first error for the
        // causing thread's next `check`; later ones are only logged.
        let id = NEXT_CONTEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        device.on_uncaptured_error(Arc::new(move |error| {
            let fault = fault_from(error);
            FAULT.with(|f| {
                let mut faults = f.borrow_mut();
                if !faults.iter().any(|(c, _)| *c == id) {
                    faults.push((id, fault));
                }
            });
        }));
        let lost = Arc::new(Mutex::new(None));
        device.set_device_lost_callback({
            let lost = lost.clone();
            move |reason, message| {
                // `Destroyed` is the device being dropped, not a failure.
                if reason != wgpu::DeviceLostReason::Destroyed {
                    tracing::error!("GPU device lost: {message}");
                    let mut lost = lost.lock().unwrap_or_else(|e| e.into_inner());
                    lost.get_or_insert(message);
                }
            }
        });

        let ctx = Self {
            timestamp_queries: device.features().contains(wgpu::Features::TIMESTAMP_QUERY),
            device,
            queue,
            adapter_info: adapter.get_info(),
            format_support: Self::check_format_support(&adapter),
            staged_frames,
            budget: BudgetSource::Device,
            lost,
            id,
            programs: Mutex::default(),
            pool: pool::TexturePool::registered(),
        };
        ctx.log_startup();
        Ok(ctx)
    }

    /// A shared context for a test, or `None` (a skip, [`Self::skip_test`])
    /// when there is no GPU. Any other failure panics.
    pub fn new_or_skip() -> Option<Arc<Self>> {
        match Self::new() {
            Ok(ctx) => Some(Arc::new(ctx)),
            Err(GpuError::NoAdapter) => {
                Self::skip_test("no GPU adapter available");
                None
            }
            Err(e) => panic!("GPU init failed: {e}"),
        }
    }

    /// A test's skip for want of a GPU, printed with `why`. With
    /// `DEPTHEOS_REQUIRE_GPU=1` (CI jobs with a GPU) it panics instead.
    pub fn skip_test(why: &str) {
        if std::env::var_os("DEPTHEOS_REQUIRE_GPU").is_some_and(|v| v != "0") {
            panic!("{why}, and DEPTHEOS_REQUIRE_GPU is set");
        }
        eprintln!("skipping: {why}");
    }

    /// Check a frame against the device limits before allocating, so an
    /// oversized input is an error, not a wgpu panic. `upload_bytes` is the
    /// packed source, bound as one storage buffer. Readbacks are banded
    /// (`download_texture_rows`), so no buffer holds the whole result.
    pub fn check_frame_fits(&self, width: u32, height: u32, upload_bytes: u64) -> Result<()> {
        frame_fits(&self.device.limits(), width, height, upload_bytes)
    }

    /// The first error this thread's work on this context caused since the
    /// last check (out of memory, or a validation bug), or the device's
    /// loss. Call it after allocating and before trusting a readback: work
    /// on a failed object fails silently. Each error is returned once; a
    /// lost device fails every check.
    pub fn check(&self) -> Result<()> {
        self.check_lost()?;
        match take_fault(self.id) {
            Some(fault) => Err(fault),
            None => Ok(()),
        }
    }

    /// Forget this thread's unchecked error on this context; call it where
    /// new work starts. A lost device stays lost.
    pub fn clear_fault(&self) {
        if let Some(stale) = take_fault(self.id) {
            tracing::warn!("discarding an unchecked GPU error: {stale:?}");
        }
    }

    /// The device's loss as an error, if it is lost. Unlike [`Self::check`]
    /// it leaves this thread's other errors for the next check.
    pub fn check_lost(&self) -> Result<()> {
        match self.lost.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            Some(message) => Err(GpuError::DeviceLost(message)),
            None => Ok(()),
        }
    }

    /// A command encoder labeled `label`; pair with [`Self::submit`].
    pub fn encoder(&self, label: &str) -> wgpu::CommandEncoder {
        self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) })
    }

    /// Finish `encoder` and submit it on its own; then the pooled textures
    /// this thread dropped before may be reused.
    pub fn submit(&self, encoder: wgpu::CommandEncoder) -> wgpu::SubmissionIndex {
        let index = self.queue.submit(std::iter::once(encoder.finish()));
        self.submitted();
        index
    }

    /// Block until the submission `index` (and every one before it) is
    /// done, so what it used and was dropped is freed.
    pub fn wait_for(&self, index: wgpu::SubmissionIndex) -> Result<()> {
        self.wait(Some(index))
    }

    /// Block until everything submitted on the device so far is done,
    /// other threads' work included.
    pub fn wait_all(&self) -> Result<()> {
        self.wait(None)
    }

    fn wait(&self, submission_index: Option<wgpu::SubmissionIndex>) -> Result<()> {
        let timeout = Some(crate::WAIT_LIMIT);
        let waited = self.device.poll(wgpu::PollType::Wait { submission_index, timeout });
        self.check_lost()?;
        waited.map(drop).map_err(crate::error::poll_error)
    }

    pub fn create_texture(
        &self,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
        usage: wgpu::TextureUsages,
        label: &str,
    ) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    }

    // =========================================================================
    // Format selection
    // =========================================================================

    /// Format of decoded source frames, read by every stacker's ingest.
    /// Rgba16Unorm when storable (exact for 8- and 16-bit inputs, half the
    /// bandwidth of f32), else Rgba32Float.
    pub fn source_format(&self) -> wgpu::TextureFormat {
        if self.format_support.rgba16unorm_storage {
            wgpu::TextureFormat::Rgba16Unorm
        } else {
            wgpu::TextureFormat::Rgba32Float
        }
    }

    /// How frames staged for upload reach the unpack shader.
    pub fn staged_frames(&self) -> StagedFrames {
        self.staged_frames
    }

    /// Format of the 8-bit map: R8Unorm when storable, else Rgba8Unorm with
    /// the gray in every channel. Shaders select theirs with `MAP_R8`.
    pub fn map_format(&self) -> wgpu::TextureFormat {
        if self.format_support.r8_storage {
            wgpu::TextureFormat::R8Unorm
        } else {
            wgpu::TextureFormat::Rgba8Unorm
        }
    }

    // =========================================================================
    // Uniform buffer helpers (encase handles WGSL struct layout rules)
    // =========================================================================

    pub fn create_uniform_buffer<T: encase::ShaderType + encase::internal::WriteInto>(
        &self,
        data: &T,
        label: &str,
    ) -> wgpu::Buffer {
        self.create_buffer_with_data(label, &uniform_bytes(data), wgpu::BufferUsages::UNIFORM)
    }

    /// A zeroed storage buffer of `size` bytes, with `usage` besides
    /// `STORAGE`. A failed allocation is an error for [`Self::check`].
    pub fn create_storage_buffer(
        &self,
        label: &str,
        size: u64,
        usage: wgpu::BufferUsages,
    ) -> wgpu::Buffer {
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage: wgpu::BufferUsages::STORAGE | usage,
            mapped_at_creation: false,
        })
    }

    /// A buffer holding `contents`, written through the queue. Unlike
    /// wgpu's `create_buffer_init`, which panics when the allocation fails,
    /// a failure here is an error for [`Self::check`].
    pub fn create_buffer_with_data(
        &self,
        label: &str,
        contents: &[u8],
        usage: wgpu::BufferUsages,
    ) -> wgpu::Buffer {
        let align = wgpu::COPY_BUFFER_ALIGNMENT as usize;
        let size = contents.len().next_multiple_of(align).max(align);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: size as u64,
            usage: usage | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        if contents.len() == size {
            self.queue.write_buffer(&buffer, 0, contents);
        } else {
            let mut padded = contents.to_vec();
            padded.resize(size, 0);
            self.queue.write_buffer(&buffer, 0, &padded);
        }
        buffer
    }

    /// A buffer mapped at creation, or the error creating it, caught here
    /// because mapping a failed buffer panics in wgpu. A lost device is
    /// reported to no error scope, so it is checked too.
    pub fn create_mapped_buffer(
        &self,
        descriptor: &wgpu::BufferDescriptor,
    ) -> Result<wgpu::Buffer> {
        debug_assert!(descriptor.mapped_at_creation);
        self.check_lost()?;
        let out_of_memory = self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let buffer = self.device.create_buffer(descriptor);
        // Popped innermost first; each resolves at once on native.
        let errors =
            [pollster::block_on(validation.pop()), pollster::block_on(out_of_memory.pop())];
        match errors.into_iter().flatten().next() {
            Some(error) => Err(fault_from(error)),
            // The device can be lost while the buffer is made.
            None => self.check_lost().map(|()| buffer),
        }
    }

    pub fn write_uniform_buffer<T: encase::ShaderType + encase::internal::WriteInto>(
        &self,
        buffer: &wgpu::Buffer,
        data: &T,
    ) {
        self.queue.write_buffer(buffer, 0, &uniform_bytes(data));
    }

    fn log_startup(&self) {
        let info = &self.adapter_info;
        // The driver is what a bug report most needs, beside the name.
        let driver = [info.driver.as_str(), info.driver_info.as_str()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let driver = if driver.is_empty() { String::new() } else { format!(", driver {driver}") };
        info!("GPU: {} ({:?} / {:?}){driver}", info.name, info.device_type, info.backend);
        info!(
            "timestamps={} source={:?} map={:?} staged frames {:?}",
            self.timestamp_queries,
            self.source_format(),
            self.map_format(),
            self.staged_frames,
        );
        // What stops a large frame before memory does.
        let limits = self.device.limits();
        info!(
            "limits: buffer {} MB, storage binding {} MB, texture side {}",
            limits.max_buffer_size >> 20,
            limits.max_storage_buffer_binding_size >> 20,
            limits.max_texture_dimension_2d,
        );
        self.log_memory("at start", self.memory_budget());
    }
}

/// `data` laid out by WGSL's uniform-buffer rules.
fn uniform_bytes<T: encase::ShaderType + encase::internal::WriteInto>(data: &T) -> Vec<u8> {
    let mut buffer = encase::UniformBuffer::new(Vec::new());
    buffer.write(data).expect("encase write failed");
    buffer.into_inner()
}

/// [`GpuContext::check_frame_fits`] against `limits`.
fn frame_fits(limits: &wgpu::Limits, width: u32, height: u32, upload_bytes: u64) -> Result<()> {
    let max_dim = limits.max_texture_dimension_2d;
    if width > max_dim || height > max_dim {
        return Err(GpuError::FrameTooLarge {
            width,
            height,
            limit: FrameLimit::Side { max: max_dim },
        });
    }
    let max = limits.max_storage_buffer_binding_size.min(limits.max_buffer_size);
    if upload_bytes > max {
        let limit = FrameLimit::Buffer { what: "upload", bytes: upload_bytes, max };
        return Err(GpuError::FrameTooLarge { width, height, limit });
    }
    Ok(())
}

#[cfg(test)]
mod frame_fits_tests {
    use super::*;

    /// A frame fits by its sides and its upload only: a 200 MP stack whose
    /// whole readback passes a 1 GB buffer limit fits (readbacks are banded).
    #[test]
    fn a_frame_fits_by_its_sides_and_its_upload() {
        let limits = wgpu::Limits {
            max_texture_dimension_2d: 16384,
            max_buffer_size: 1 << 30,
            max_storage_buffer_binding_size: 1 << 30,
            ..wgpu::Limits::default()
        };
        let (w, h) = (16384, 12288);
        assert!(frame_fits(&limits, w, h, u64::from(w * h) * 3).is_ok());
        assert!(matches!(
            frame_fits(&limits, 16385, 10, 0),
            Err(GpuError::FrameTooLarge { limit: FrameLimit::Side { max: 16384 }, .. })
        ));
        assert!(matches!(
            frame_fits(&limits, w, h, (1 << 30) + 1),
            Err(GpuError::FrameTooLarge { limit: FrameLimit::Buffer { what: "upload", .. }, .. })
        ));
        let bound = wgpu::Limits { max_storage_buffer_binding_size: 1 << 27, ..limits };
        assert!(frame_fits(&bound, 100, 100, (1 << 27) + 1).is_err(), "the binding's limit");
    }
}

#[cfg(test)]
mod wait_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Waiting for a submission returns once its work is done.
    #[test]
    fn waiting_for_a_submission_returns_once_it_is_done() {
        let Some(ctx) = GpuContext::new_or_skip() else { return };
        let buffer = ctx.create_storage_buffer("wait", 1 << 20, wgpu::BufferUsages::COPY_DST);
        let mut encoder = ctx.encoder("wait");
        encoder.clear_buffer(&buffer, 0, None);
        let index = ctx.submit(encoder);
        let done = Arc::new(AtomicBool::new(false));
        let flag = done.clone();
        ctx.queue.on_submitted_work_done(move || flag.store(true, Ordering::SeqCst));
        ctx.wait_for(index).unwrap();
        assert!(done.load(Ordering::SeqCst));
        ctx.wait_all().unwrap();
        ctx.check().unwrap();
    }
}
