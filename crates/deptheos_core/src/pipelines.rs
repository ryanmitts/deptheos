//! The GPU programs around the stackers (upload, warp, contribution
//! tint, u16 pack), compiled once per context and shared.
//! Each method's kernels, with the modules in `SHARED_SHADER_MODULES`, are
//! compiled by its first stacker on a context (`GpuContext::programs`).

use std::sync::{Mutex, MutexGuard};

use deptheos_gpu::GpuContext;

use crate::contribution::ContributionOverlay;
use crate::pack::U16Packer;
use crate::upload::ImageUploader;
use crate::warp::TextureWarper;

/// The sRGB curves (`deptheos::color`), shared with the color render.
pub const COLOR_MODULE: (&str, &str) = (include_str!("shaders/color.wgsl"), "deptheos::color");

/// Shared composable shader modules registered for `#import` resolution.
/// A module comes after the modules it imports.
pub(crate) const SHARED_SHADER_MODULES: &[(&str, &str)] = &[
    COLOR_MODULE,
    (include_str!("shaders/metric.wgsl"), "deptheos::metric"),
    (include_str!("shaders/warp_common.wgsl"), "deptheos::warp"),
    (include_str!("shaders/resample.wgsl"), "deptheos::resample"),
    (include_str!("shaders/focus.wgsl"), "deptheos::focus"),
    (include_str!("shaders/laplacian.wgsl"), "deptheos::laplacian"),
    (include_str!("shaders/accum.wgsl"), "deptheos::accum"),
    (include_str!("shaders/plane.wgsl"), "deptheos::plane"),
];

pub struct CommonPipelines {
    pub warper: TextureWarper,
    pub contribution: ContributionOverlay,
    /// `None` where the adapter cannot store Rgba16Unorm.
    pub packer: Option<U16Packer>,
    /// Keeps its staging buffer between uploads; callers that upload a
    /// one-off (a save) release it afterwards.
    uploader: Mutex<ImageUploader>,
}

impl CommonPipelines {
    pub fn new(ctx: &GpuContext) -> Self {
        Self {
            warper: TextureWarper::new(ctx),
            contribution: ContributionOverlay::new(ctx),
            packer: U16Packer::new(ctx).ok(),
            uploader: Mutex::new(ImageUploader::new(ctx)),
        }
    }

    /// The shared uploader. A poisoned lock is ignored: the uploader holds
    /// no invariant a panic could break.
    pub fn uploader(&self) -> MutexGuard<'_, ImageUploader> {
        self.uploader.lock().unwrap_or_else(|e| e.into_inner())
    }
}
