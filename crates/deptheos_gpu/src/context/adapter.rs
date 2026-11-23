//! Which device a context runs on: the GPUs on the machine, the adapter a
//! [`DeviceChoice`] means, and the features, limits and formats asked of it.

use super::{FormatSupport, GpuContext, StagedFrames};
use crate::error::{GpuError, Result};

/// The device a [`GpuContext`] runs on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum DeviceChoice {
    /// The fastest GPU, or the CPU renderer where there is none.
    #[default]
    Automatic,
    /// The CPU renderer (WARP on Windows), as on a PC without a GPU.
    Software,
    /// The GPU of this name ([`GpuContext::devices`]); Automatic when this
    /// machine has none.
    Named(String),
}

/// A GPU [`GpuContext::devices`] found, or the device a context runs on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    pub name: String,
    pub device_type: wgpu::DeviceType,
}

impl DeviceInfo {
    /// The CPU renderer rather than a GPU.
    pub fn is_software(&self) -> bool {
        self.device_type == wgpu::DeviceType::Cpu
    }

    /// What kind of device it is, where that helps choose.
    pub fn kind(&self) -> DeviceKind {
        match self.device_type {
            wgpu::DeviceType::DiscreteGpu => DeviceKind::Dedicated,
            wgpu::DeviceType::IntegratedGpu => DeviceKind::Integrated,
            wgpu::DeviceType::Cpu => DeviceKind::Software,
            _ => DeviceKind::Other,
        }
    }
}

/// What kind of device a [`DeviceInfo`] is. The front ends word it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceKind {
    /// A GPU of its own.
    Dedicated,
    /// A GPU that shares the CPU's memory.
    Integrated,
    /// The CPU renderer.
    Software,
    /// Anything else (a virtual GPU, or one the driver doesn't say).
    Other,
}

impl GpuContext {
    /// The GPUs on this machine, each once, discrete first; not the CPU
    /// renderer ([`DeviceChoice::Software`]). Takes tenths of a second: keep
    /// it off the UI thread.
    pub fn devices() -> Vec<DeviceInfo> {
        let instance = Self::instance(false);
        let mut found: Vec<DeviceInfo> = Vec::new();
        for adapter in pollster::block_on(instance.enumerate_adapters(wgpu::Backends::PRIMARY)) {
            let info = adapter.get_info();
            if info.device_type != wgpu::DeviceType::Cpu
                && !found.iter().any(|d| d.name == info.name)
            {
                found.push(DeviceInfo { name: info.name, device_type: info.device_type });
            }
        }
        found.sort_by_key(|d| match d.device_type {
            wgpu::DeviceType::DiscreteGpu => 0,
            wgpu::DeviceType::IntegratedGpu => 1,
            _ => 2,
        });
        found
    }

    /// The adapter `choice` means here. DEPTHEOS_SOFTWARE_GPU=1 forces the
    /// CPU renderer, to test what a PC without a GPU gets.
    pub(super) fn adapter(choice: &DeviceChoice) -> Result<wgpu::Adapter> {
        let software = std::env::var_os("DEPTHEOS_SOFTWARE_GPU").is_some_and(|v| v != "0");
        let adapter = match choice {
            _ if software => return Self::request_adapter(false, true),
            DeviceChoice::Software => return Self::request_adapter(false, true),
            DeviceChoice::Automatic => Self::request_adapter(true, false)?,
            DeviceChoice::Named(name) => {
                let named = |info: &wgpu::AdapterInfo| {
                    &info.name == name && info.device_type != wgpu::DeviceType::Cpu
                };
                match Self::find_adapter(true, named) {
                    Some(adapter) => adapter,
                    None => {
                        tracing::warn!("no GPU named {name:?}; using the fastest");
                        Self::request_adapter(true, false)?
                    }
                }
            }
        };
        // The budget guard is for a GPU with its own memory. On system RAM
        // (the CPU renderer, an integrated GPU) the reported budget can be
        // far below what is free (a 179 MB texture refused on a GPU that
        // took 7 GB unguarded), and running out fails the allocation
        // anyway, with no driver reset.
        let info = adapter.get_info();
        if matches!(info.device_type, wgpu::DeviceType::Cpu | wgpu::DeviceType::IntegratedGpu) {
            let same = |i: &wgpu::AdapterInfo| i.name == info.name && i.backend == info.backend;
            if let Some(unguarded) = Self::find_adapter(false, same) {
                return Ok(unguarded);
            }
            tracing::warn!("{} is not listed without the memory guard; keeping it", info.name);
        }
        Ok(adapter)
    }

    /// The first adapter `pick` accepts, in the order the backends list
    /// them (Vulkan before D3D12, as for the fastest), with `budget_guard`
    /// ([`Self::instance`]).
    fn find_adapter(
        budget_guard: bool,
        pick: impl Fn(&wgpu::AdapterInfo) -> bool,
    ) -> Option<wgpu::Adapter> {
        let instance = Self::instance(budget_guard);
        pollster::block_on(instance.enumerate_adapters(wgpu::Backends::PRIMARY))
            .into_iter()
            .find(|adapter| pick(&adapter.get_info()))
    }

    /// The CPU renderer when `software`, else the fastest GPU (the CPU
    /// renderer when there is none), with `budget_guard` ([`Self::instance`]).
    fn request_adapter(budget_guard: bool, software: bool) -> Result<wgpu::Adapter> {
        pollster::block_on(Self::instance(budget_guard).request_adapter(
            &wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: software,
            },
        ))
        .map_err(|_| GpuError::NoAdapter)
    }

    /// A wgpu instance. `budget_guard` makes D3D12 and Vulkan refuse an
    /// allocation past 95% of the driver's budget (an error for
    /// [`GpuContext::check`]) rather than overcommit, which can stall until
    /// the driver resets. Metal ignores it; [`MemoryBudget::check`] guards
    /// there.
    fn instance(budget_guard: bool) -> wgpu::Instance {
        wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds {
                for_resource_creation: budget_guard.then_some(95),
                for_device_loss: None,
            },
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        })
    }

    /// The optional features used, only those the adapter offers (asking
    /// for another fails device creation).
    pub(super) fn build_required_features(
        adapter: &wgpu::Adapter,
        staged_frames: StagedFrames,
    ) -> wgpu::Features {
        let supported = adapter.features();
        let mut features = wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES;
        for candidate in
            [wgpu::Features::TEXTURE_FORMAT_16BIT_NORM, wgpu::Features::TIMESTAMP_QUERY]
        {
            if supported.contains(candidate) {
                features |= candidate;
            }
        }
        if staged_frames == StagedFrames::InPlace {
            features |= wgpu::Features::MAPPABLE_PRIMARY_BUFFERS;
        }
        features
    }

    /// Limits negotiated from the adapter's: asking for more fails device
    /// creation.
    pub(super) fn build_limits(adapter: &wgpu::Adapter) -> wgpu::Limits {
        let adapter_limits = adapter.limits();
        // WebGPU's defaults, capped at the adapter's: some Apple GPUs fall
        // short of an unused default (`max_inter_stage_shader_variables`).
        let mut limits = wgpu::Limits::default()
            .using_resolution(adapter_limits.clone())
            .or_worse_values_from(&adapter_limits);
        // Full-resolution stacks (8K+ frames) need large buffers and bindings.
        limits.max_buffer_size = adapter_limits.max_buffer_size;
        limits.max_storage_buffer_binding_size = adapter_limits.max_storage_buffer_binding_size;
        // Some stacking passes bind several storage textures per dispatch.
        limits.max_storage_textures_per_shader_stage =
            adapter_limits.max_storage_textures_per_shader_stage.min(8);
        limits
    }

    pub(super) fn check_format_support(adapter: &wgpu::Adapter) -> FormatSupport {
        let storage = |format| {
            adapter
                .get_texture_format_features(format)
                .allowed_usages
                .contains(wgpu::TextureUsages::STORAGE_BINDING)
        };
        FormatSupport {
            rgba16unorm_storage: adapter
                .features()
                .contains(wgpu::Features::TEXTURE_FORMAT_16BIT_NORM)
                && storage(wgpu::TextureFormat::Rgba16Unorm),
            r8_storage: storage(wgpu::TextureFormat::R8Unorm),
        }
    }
}
