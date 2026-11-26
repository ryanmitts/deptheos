//! Display pixels for a rect of the canvas, as the whole result displays:
//! sRGB rounded on the CPU, linear rendered on the GPU and read back.
//!
//! Paint owns its uploader, as `CommonPipelines::uploader`'s lock is held
//! through a preview's full-frame upload. Buffers are prepared once per
//! [`Constants`] and textures pooled, so a dab costs one small upload,
//! dispatch and readback.

use std::sync::Arc;

use deptheos_core::io;
use deptheos_core::upload::ImageUploader;
use deptheos_gpu::GpuContext;
use deptheos_gpu::gpu_utils::download_texture_with;
use deptheos_render::Draw;
use deptheos_render::dng_render::{Constants, DngRenderPipeline, PreparedRender, Region, Source};

use crate::engine::look::Look;
use crate::engine::present::{CpuImage, conversion, display_pixel, display_rgb16, source_rgb};
use crate::{Error, Result};

/// A rect of interleaved RGB u16 pixels, `width`×`height`.
#[derive(Clone, Copy)]
pub(crate) struct RectPixels<'a> {
    pub pixels: &'a [u16],
    pub width: u32,
    pub height: u32,
}

pub(crate) struct RectRenderer {
    uploader: ImageUploader,
    prepared: Option<(Box<Constants>, PreparedRender)>,
    /// The upload target and the render output, grown to the largest rect
    /// lately; a rect uses their top-left corner.
    pool: Option<(wgpu::Texture, wgpu::Texture)>,
    /// Renders in a row that used a small corner of the pool.
    oversized: u32,
}

/// Pooled textures grow in steps of this many pixels a side.
const POOL_STEP: u32 = 256;
/// Slack past which a pool shrinks, so one huge rect doesn't keep
/// full-frame textures all session.
const POOL_SLACK: u64 = 1 << 22;
/// How many renders in a row must fit a quarter of the pool before it
/// shrinks.
const SHRINK_AFTER: u32 = 64;

impl RectRenderer {
    pub(crate) fn new(ctx: &GpuContext) -> Self {
        Self { uploader: ImageUploader::new(ctx), prepared: None, pool: None, oversized: 0 }
    }

    /// The display pixels of `rect`, equal to that rect of the whole
    /// display in `look`: rendered on the GPU, or rounded and converted on
    /// the CPU.
    pub(crate) fn render(
        &mut self,
        ctx: &Arc<GpuContext>,
        color: &DngRenderPipeline,
        rect: RectPixels<'_>,
        look: Look<'_>,
    ) -> Result<CpuImage> {
        let RectPixels { pixels, width, height } = rect;
        let (w, h) = (width as usize, height as usize);
        if pixels.len() != w * h * 3 || w == 0 || h == 0 {
            return Err(Error::InvalidState(format!(
                "{} values for a {width}x{height} RGB rect",
                pixels.len()
            )));
        }
        let constants = match look.resolve() {
            Draw::Rendered(constants) => constants,
            Draw::Stored { profile, output } => {
                let mut out = vec![0; w * h * 4];
                display_rgb16(pixels, &mut out, conversion(profile, output).as_ref());
                return Ok(CpuImage { width, height, pixels: out });
            }
        };

        if self.prepared.as_ref().is_none_or(|(c, _)| *c != constants) {
            let prepared = PreparedRender::new(ctx, &constants);
            self.prepared = Some((constants, prepared));
        }
        let (input, output) = self.textures(ctx, width, height);
        let prep = self.uploader.prepare_rgb16(ctx, pixels, width, height).map_err(Error::Core)?;
        let mut encoder = ctx.encoder("paint_upload");
        prep.dispatch_to_rgba(ctx, &mut encoder, &input);
        ctx.submit(encoder);
        let prepared = &self.prepared.as_ref().expect("prepared above").1;
        // The rect is its own frame: paint's draws read no neighbors.
        let whole = Region::whole(width, height);
        color.draw(ctx, prepared, Source::part(&input, whole, [width, height]), whole, &output);

        let mut out = Vec::with_capacity(w * h * 4);
        let texel = output.format().block_copy_size(None).unwrap_or(8) as usize;
        download_texture_with(ctx, &output, [0, 0], [width, height], |row| {
            out.extend(
                row.chunks_exact(texel)
                    .flat_map(|t| display_pixel(source_rgb(t).map(io::u16_to_u8))),
            );
        })
        .map_err(|e| Error::Core(e.into()))?;
        Ok(CpuImage { width, height, pixels: out })
    }

    /// Pooled textures at least `width`×`height`, shrunk after a run of
    /// small renders.
    fn textures(
        &mut self,
        ctx: &GpuContext,
        width: u32,
        height: u32,
    ) -> (wgpu::Texture, wgpu::Texture) {
        let (pool_w, pool_h) = self.pool.as_ref().map_or((0, 0), |(t, _)| (t.width(), t.height()));
        let fits = pool_w >= width && pool_h >= height;
        let (want_w, want_h) =
            (width.next_multiple_of(POOL_STEP), height.next_multiple_of(POOL_STEP));
        let (pool_area, want_area) =
            (u64::from(pool_w) * u64::from(pool_h), u64::from(want_w) * u64::from(want_h));
        if fits && pool_area > 4 * want_area && pool_area - want_area > POOL_SLACK {
            self.oversized += 1;
        } else {
            self.oversized = 0;
        }
        let (w, h) = if !fits {
            (want_w.max(pool_w), want_h.max(pool_h))
        } else if self.oversized >= SHRINK_AFTER {
            (want_w, want_h)
        } else {
            return self.pool.clone().expect("fits, so the pool is there");
        };
        self.oversized = 0;
        self.pool = Some((
            ImageUploader::create_target(ctx, w, h, "paint_upload"),
            DngRenderPipeline::create_output(ctx, w, h),
        ));
        self.pool.clone().expect("pool filled above")
    }
}
