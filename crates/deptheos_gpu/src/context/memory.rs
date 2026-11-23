//! What GPU memory a piece of work may plan on: the budget, where it comes
//! from, and the figures the platform reports.

use tracing::info;

use super::GpuContext;
use crate::error::{GpuError, Result};

/// What a piece of work may plan on ([`GpuContext::memory_budget`]): the
/// bytes it may use, and whose allocations count against them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryBudget {
    /// Bytes the work may use, the reserve already left out.
    pub usable: u64,
    pub pool: MemoryPool,
}

impl MemoryBudget {
    /// Refuse work of `width`×`height` frames that needs `needed` bytes,
    /// more than [`Self::usable`].
    pub fn check(self, needed: u64, width: u32, height: u32) -> Result<()> {
        if needed > self.usable {
            return Err(GpuError::OverBudget { width, height, needed, budget: self.usable });
        }
        Ok(())
    }
}

/// Whose allocations a [`MemoryBudget`] is spent by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryPool {
    /// The GPU's alone: Metal's recommended working set on a Mac, past
    /// which the system pages but ends no app. CPU memory isn't counted.
    Gpu,
    /// The app's CPU and GPU memory together: on iOS, what the app has
    /// left before the system ends it (`deptheos_app::engine::memory`).
    Shared,
}

/// Where [`GpuContext::memory_budget`] comes from.
#[derive(Clone, Copy, Debug)]
pub(super) enum BudgetSource {
    /// Read from the device and the system each time it is asked.
    Device,
    /// Set once, by [`GpuContext::with_memory_budget`].
    Fixed(MemoryBudget),
}

impl GpuContext {
    /// This context planning on `budget` instead of the device's, for a
    /// test of a stack that barely fits.
    pub fn with_memory_budget(mut self, budget: MemoryBudget) -> Self {
        self.budget = BudgetSource::Fixed(budget);
        self
    }

    /// What the GPU may plan on: Metal's recommended working set on Apple
    /// GPUs ([`MemoryPool::Gpu`]); `None` elsewhere, where D3D12 and Vulkan
    /// report running out as an error. On iOS
    /// `deptheos_app::engine::memory::stack_budget` spends this from the
    /// app's limit.
    ///
    /// Not lessened by what the device already holds: the working set is a
    /// paging threshold, not a limit.
    pub fn memory_budget(&self) -> Option<MemoryBudget> {
        if let BudgetSource::Fixed(budget) = self.budget {
            return Some(budget);
        }
        let (working_set, _) = self.metal_memory()?;
        // The iOS Simulator reports 0: no figure, not no memory.
        (working_set > 0).then_some(MemoryBudget { usable: working_set, pool: MemoryPool::Gpu })
    }

    /// Bytes this device has allocated now, where the platform says
    /// (Metal). For measuring; [`Self::memory_budget`] is for planning.
    pub fn allocated_bytes(&self) -> Option<u64> {
        self.metal_memory().map(|(_, allocated)| allocated)
    }

    /// Metal's recommended working set and the bytes allocated now, on
    /// the Metal backend.
    #[cfg(target_vendor = "apple")]
    fn metal_memory(&self) -> Option<(u64, u64)> {
        use objc2_metal::MTLDevice as _;
        // SAFETY: only properties of the raw device are read; nothing is
        // created, freed or encoded through it behind wgpu's back.
        let hal = unsafe { self.device.as_hal::<wgpu::hal::api::Metal>() }?;
        let device = hal.raw_device();
        Some((device.recommendedMaxWorkingSetSize(), device.currentAllocatedSize() as u64))
    }

    #[cfg(not(target_vendor = "apple"))]
    fn metal_memory(&self) -> Option<(u64, u64)> {
        None
    }

    /// Log Metal's working set, its bytes allocated and what `budget` (the
    /// one the caller checks with) leaves usable. `when` says what is about
    /// to run.
    pub fn log_memory(&self, when: &str, budget: Option<MemoryBudget>) {
        let Some((working_set, allocated)) = self.metal_memory() else { return };
        let usable = budget.map(|b| b.usable >> 20);
        info!(
            "GPU memory {when}: working set {} MB, {} MB allocated, {usable:?} MB usable",
            working_set >> 20,
            allocated >> 20,
        );
    }
}

/// The memory budget D3D12 reports for the adapter and its use now, in
/// bytes: what the allocation guard compares against ([`GpuContext::instance`]).
#[cfg(windows)]
pub(super) fn d3d12_memory(adapter: &wgpu::Adapter) -> Option<(u64, u64)> {
    use windows::Win32::Graphics::Dxgi::DXGI_MEMORY_SEGMENT_GROUP_LOCAL;
    // SAFETY: only the adapter's memory figures are read.
    let hal = unsafe { adapter.as_hal::<wgpu::hal::api::Dx12>() }?;
    let info = hal.raw_adapter().query_video_memory_info(DXGI_MEMORY_SEGMENT_GROUP_LOCAL).ok()?;
    Some((info.Budget, info.CurrentUsage))
}

#[cfg(not(windows))]
pub(super) fn d3d12_memory(_adapter: &wgpu::Adapter) -> Option<(u64, u64)> {
    None
}

/// `bytes` for a message, in GB to one decimal ("6.8 GB").
pub(super) fn gigabytes(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}
