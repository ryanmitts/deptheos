//! The one memory policy (ADR-111): what the app may still use, the
//! budgets made from it, and shedding when memory runs short.
//!
//! The limit exists so iOS never ends the app: there CPU and GPU share one
//! pool, read live ([`left`]) less [`RESERVE`]. The desktop has none and
//! checks only what the GPU can hold, its budgets scaled to RAM so a Mac
//! doesn't page.
//!
//! Budgets read from here each time: [`stack_budget`], [`prefetch_bytes`],
//! [`Budget`] and [`DISPLAY_ATLAS`]. A [`shed`] asks every paint worker to
//! let go at a [`Shed`] level and empties the GPU's texture pool; the app
//! sheds its own memory beside it.

use deptheos_gpu::{GpuContext, MemoryBudget, MemoryPool};

/// Held back from what an iOS app has left when it plans, for what the
/// system and the rest of the app take that no estimate counts. Tuned from
/// devices' logs ([`log`]).
pub const RESERVE: u64 = 512 << 20;

/// What the app may have left before a stack stops: half the reserve, so a
/// stack within its estimate never reaches it (ADR-111).
pub const FLOOR: u64 = RESERVE / 2;

/// Bytes of display tiles in GPUI's atlas above which those not painted
/// lately are dropped (re-uploaded when painted again).
pub const DISPLAY_ATLAS: usize = 256 << 20;

/// What the app has left before iOS ends it, read now; `None` elsewhere,
/// the Simulator included (where it reads 0). Freed GPU memory shows only
/// once wgpu has been polled after its work.
pub fn left() -> Option<u64> {
    #[cfg(target_os = "ios")]
    {
        unsafe extern "C" {
            fn os_proc_available_memory() -> usize;
        }
        // SAFETY: no arguments or preconditions (iOS 13 and later).
        let left = unsafe { os_proc_available_memory() } as u64;
        (left > 0).then_some(left)
    }
    #[cfg(not(target_os = "ios"))]
    None
}

/// [`left`] less [`RESERVE`], where there is a limit.
pub fn limit() -> Option<u64> {
    left().map(|left| left.saturating_sub(RESERVE))
}

/// What a stack may plan on: the GPU's budget, capped by the app's limit
/// where there is one ([`MemoryPool::Shared`]). A test's budget is its own.
pub fn stack_budget(ctx: &GpuContext) -> Option<MemoryBudget> {
    let gpu = ctx.memory_budget()?;
    match (gpu.pool, limit()) {
        (MemoryPool::Gpu, Some(limit)) => {
            Some(MemoryBudget { usable: gpu.usable.min(limit), pool: MemoryPool::Shared })
        }
        _ => Some(gpu),
    }
}

/// Decoded bytes the read-ahead may hold, at most `asked`; under a limit,
/// a sixth of what is left, between two small frames and two large ones.
pub(crate) fn prefetch_bytes(asked: u64) -> u64 {
    match left() {
        Some(left) => asked.min((left / 6).clamp(128 << 20, 768 << 20)),
        None => asked,
    }
}

/// A RAM budget of paint's, read each time so it follows what is left.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Budget {
    /// Paint's source frames; past it, all but the current one go.
    PaintSources,
    /// Undo history held in RAM rather than read back from its log.
    PaintHistory,
    /// Set (tests).
    #[cfg_attr(not(test), allow(dead_code))]
    Fixed(usize),
}

impl Budget {
    /// The budget now for a holder of `held` bytes. Under a limit, a share
    /// of what is left plus `held`, so holding more doesn't shrink it
    /// (sources a quarter, at least 256 MB; history a sixteenth, at least
    /// 64 MB); elsewhere a share of RAM (a quarter, at least 1.5 GB; a
    /// sixteenth, within 512 MB..4 GB).
    pub(crate) fn bytes(self, held: usize) -> usize {
        let of_left = |share: u64, least: u64| {
            left().map(|left| ((left + held as u64) / share).max(least) as usize)
        };
        match self {
            Self::PaintSources => {
                of_left(4, 256 << 20).unwrap_or_else(|| (physical() / 4).max(3 << 29) as usize)
            }
            Self::PaintHistory => of_left(16, 64 << 20)
                .unwrap_or_else(|| (physical() / 16).clamp(512 << 20, 4 << 30) as usize),
            Self::Fixed(bytes) => bytes,
        }
    }
}

/// How much to let go of when memory runs short (an argument, not a state).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Shed {
    /// Short or about to be (gone to the background): what is cheap to
    /// remake goes (paint's other sources and spilled undo, thumbnails off
    /// screen).
    Moderate,
    /// About to be ended: also a map pyramid not shown, a replaced compare
    /// and paint's oldest unspillable undo steps.
    Critical,
}

/// Asks every paint worker to shed at `level`, without waiting, and frees
/// the GPU's pooled textures; callable from any thread.
pub fn shed(level: Shed) {
    log(&format!("shedding ({level:?})"));
    crate::paint::worker::shed(level);
    deptheos_gpu::trim_pools(0);
}

/// "footprint 812 MB, 2304 MB left": the physical footprint and, under a
/// limit, what is left; logged at each shed and transition.
pub fn reading() -> String {
    let mb = |b: Option<u64>| b.map_or_else(|| "?".to_owned(), |b| (b >> 20).to_string());
    match left() {
        Some(left) => format!("footprint {} MB, {} MB left", mb(footprint()), left >> 20),
        None => format!("footprint {} MB", mb(footprint())),
    }
}

/// [`reading`] at info, `when` saying what happens.
pub fn log(when: &str) {
    tracing::info!("memory {when}: {}", reading());
}

/// The physical footprint on Apple platforms: what iOS ends the app over
/// and what Activity Monitor shows.
pub fn footprint() -> Option<u64> {
    #[cfg(target_vendor = "apple")]
    {
        // `task_vm_info` up to `phys_footprint`, as its revision 1 lays it
        // out; the kernel fills the count it is given.
        #[repr(C, packed(4))]
        #[derive(Default)]
        struct VmInfo {
            sizes: [u64; 1],
            region_count: i32,
            page_size: i32,
            more: [u64; 16],
            phys_footprint: u64,
        }
        unsafe extern "C" {
            static mach_task_self_: u32;
            fn task_info(task: u32, flavor: u32, info: *mut i32, count: *mut u32) -> i32;
        }
        const TASK_VM_INFO: u32 = 22;
        let mut info = VmInfo::default();
        let mut count = (size_of::<VmInfo>() / size_of::<u32>()) as u32;
        // SAFETY: the task's own port; `info` is as large as `count` says,
        // and laid out as the kernel writes it.
        let status = unsafe {
            task_info(mach_task_self_, TASK_VM_INFO, (&raw mut info).cast(), &raw mut count)
        };
        (status == 0).then_some(info.phys_footprint)
    }
    #[cfg(not(target_vendor = "apple"))]
    None
}

/// Physical RAM, or 16 GB when it can't be read.
pub(crate) fn physical() -> u64 {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        let mut bytes: u64 = 0;
        let mut len = size_of::<u64>();
        // SAFETY: hw.memsize is a u64, and `len` says so.
        let ok = unsafe {
            libc::sysctlbyname(
                c"hw.memsize".as_ptr(),
                (&raw mut bytes).cast(),
                &raw mut len,
                std::ptr::null_mut(),
                0,
            )
        } == 0;
        if ok && bytes > 0 {
            return bytes;
        }
    }
    #[cfg(target_os = "linux")]
    {
        // SAFETY: sysconf has no preconditions.
        let (pages, size) =
            unsafe { (libc::sysconf(libc::_SC_PHYS_PAGES), libc::sysconf(libc::_SC_PAGESIZE)) };
        if pages > 0 && size > 0 {
            return pages as u64 * size as u64;
        }
    }
    #[cfg(windows)]
    if let Some(status) = windows::memory_status() {
        return status.total_phys;
    }
    16 << 30
}

/// RAM Windows can hand out now, the file cache's included; `None`
/// elsewhere. A session decides from it whether reads skip the file cache.
pub(crate) fn available_physical() -> Option<u64> {
    #[cfg(windows)]
    {
        windows::memory_status().map(|status| status.avail_phys)
    }
    #[cfg(not(windows))]
    {
        None
    }
}

#[cfg(windows)]
mod windows {
    #[repr(C)]
    pub(super) struct MemoryStatusEx {
        length: u32,
        memory_load: u32,
        pub(super) total_phys: u64,
        pub(super) avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }

    unsafe extern "system" {
        fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
    }

    pub(super) fn memory_status() -> Option<MemoryStatusEx> {
        // SAFETY: MEMORYSTATUSEX laid out as Win32 declares it, with its
        // length filled in as the call requires.
        let mut status: MemoryStatusEx = unsafe { std::mem::zeroed() };
        status.length = std::mem::size_of::<MemoryStatusEx>() as u32;
        let ok = unsafe { GlobalMemoryStatusEx(&mut status) } != 0;
        (ok && status.total_phys > 0).then_some(status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_desktop_has_no_limit_and_its_budgets_scale_with_ram() {
        #[cfg(not(target_os = "ios"))]
        {
            assert_eq!(limit(), None);
            assert_eq!(prefetch_bytes(1200 << 20), 1200 << 20);
            assert!(Budget::PaintSources.bytes(0) >= 3 << 29);
            assert!((512 << 20..=4 << 30).contains(&Budget::PaintHistory.bytes(0)));
        }
        assert_eq!(Budget::Fixed(7).bytes(1 << 30), 7);
        #[cfg(target_vendor = "apple")]
        assert!(footprint().is_some_and(|f| f > 1 << 20), "{}", reading());
    }
}
