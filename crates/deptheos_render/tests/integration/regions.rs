//! A region draws as that region of the whole picture: across tiles, and
//! with a stage that reads neighbors through a margin and a whole-frame
//! input (`median.wgsl`).

use deptheos_dng::{DngImage, RenderParams};
use deptheos_gpu::GpuContext;
use deptheos_gpu::gpu_utils::download_texture_with;
use deptheos_render::dng_render::{
    DngRenderPipeline, NeighborStage, PreparedRender, Region, Source, TILE,
};
use image::{DynamicImage, ImageBuffer, Rgb};

/// Wider than a tile, so regions cross a tile's edge.
const FRAME: [u32; 2] = [TILE + 52, 96];

/// Regions inside, at the edges of and across tiles.
const REGIONS: [Region; 4] = [
    Region { x: 0, y: 0, width: 31, height: 17 },
    Region { x: TILE - 40, y: 13, width: 90, height: 70 },
    Region { x: TILE + 2, y: 50, width: 50, height: 46 },
    Region { x: 1, y: 1, width: TILE + 50, height: 94 },
];

/// Gradients with a hashed texture over them, so neighbors differ.
fn linear_frame() -> Vec<u16> {
    let [w, h] = FRAME;
    let mut pixels = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            for c in 0..3u32 {
                let hash = (x.wrapping_mul(73_856_093)
                    ^ y.wrapping_mul(19_349_663)
                    ^ c.wrapping_mul(83_492_791))
                    % 4096;
                let ramp = x * 40_000 / w + y * 20_000 / h + c * 3000;
                pixels.push((ramp + hash * 4).min(65_535) as u16);
            }
        }
    }
    pixels
}

/// The converter fixture's own color science, its profile's tables
/// included.
fn file_params() -> RenderParams {
    let path = crate::common::core_fixture("converter.dng");
    let read = DngImage::read_with_pixels(
        &path,
        deptheos_dng::FileCache::Use,
        deptheos_dng::Budget::unlimited(),
    )
    .expect("read DNG");
    let deptheos_dng::DngRead::Linear(source, _) = read else { panic!("a linear DNG") };
    source.render_params().expect("render params")
}

fn upload(ctx: &GpuContext) -> wgpu::Texture {
    let buffer = ImageBuffer::<Rgb<u16>, _>::from_raw(FRAME[0], FRAME[1], linear_frame()).unwrap();
    crate::common::upload(ctx, &DynamicImage::ImageRgb16(buffer))
}

/// A small whole-frame input of distinct values.
fn frame_input(ctx: &GpuContext) -> wgpu::Texture {
    let (w, h) = (21, 3);
    let texture = ctx.create_texture(
        w,
        h,
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        "frame_input",
    );
    let bytes: Vec<u8> = (0..w * h * 4).map(|i| (i * 37 % 251) as u8).collect();
    ctx.queue.write_texture(
        texture.as_image_copy(),
        &bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(w * 4),
            rows_per_image: Some(h),
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    texture
}

/// `region` of `texture`, whose top-left is at frame (`at`), as bytes.
fn read(ctx: &GpuContext, texture: &wgpu::Texture, region: Region, at: [u32; 2]) -> Vec<u8> {
    let mut bytes = Vec::new();
    let origin = [region.x - at[0], region.y - at[1]];
    download_texture_with(ctx, texture, origin, [region.width, region.height], |row| {
        bytes.extend_from_slice(row)
    })
    .unwrap();
    bytes
}

#[test]
fn a_region_draws_as_the_whole_picture_does() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let pipeline = DngRenderPipeline::new(&ctx);
    let prepared = PreparedRender::new(&ctx, &crate::common::constants(&file_params()));
    let source = upload(&ctx);
    let whole = Region::whole(FRAME[0], FRAME[1]);
    let full = DngRenderPipeline::create_output(&ctx, FRAME[0], FRAME[1]);
    pipeline.draw(&ctx, &prepared, Source::whole(&source), whole, &full);

    for region in REGIONS {
        let output = DngRenderPipeline::create_output(&ctx, region.width, region.height);
        pipeline.draw(&ctx, &prepared, Source::whole(&source), region, &output);
        let drawn = read(&ctx, &output, region, [region.x, region.y]);
        assert!(drawn == read(&ctx, &full, region, [0, 0]), "{region:?} differs");
    }
}

#[test]
fn a_region_reads_its_neighbors_as_the_whole_picture_does() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let pipeline = DngRenderPipeline::new(&ctx);
    let prepared = PreparedRender::new(&ctx, &crate::common::constants(&file_params()));
    let median = NeighborStage::new(&ctx, include_str!("median.wgsl"), "median", 1);
    let (source, input) = (upload(&ctx), frame_input(&ctx));
    let whole = Region::whole(FRAME[0], FRAME[1]);
    let draw = |region: Region| {
        let output = DngRenderPipeline::create_output(&ctx, region.width, region.height);
        pipeline.draw_with(
            &ctx,
            &prepared,
            Source::whole(&source),
            region,
            &median,
            &input,
            &output,
        );
        output
    };
    let full = draw(whole);

    // The stage changes the picture, so the comparison below means something.
    let plain = DngRenderPipeline::create_output(&ctx, FRAME[0], FRAME[1]);
    pipeline.draw(&ctx, &prepared, Source::whole(&source), whole, &plain);
    assert!(read(&ctx, &plain, whole, [0, 0]) != read(&ctx, &full, whole, [0, 0]));

    for region in REGIONS {
        let drawn = read(&ctx, &draw(region), region, [region.x, region.y]);
        assert!(drawn == read(&ctx, &full, region, [0, 0]), "{region:?} differs");
    }
}

/// The test stage's `NeighborPlace` has the render function's fields.
#[test]
fn the_neighbor_uniform_matches_its_wgsl() {
    deptheos_gpu::wgsl_mirror::assert_mirrors(
        include_str!("../../src/dng_render/draw.rs"),
        include_str!("median.wgsl"),
        &["NeighborPlace"],
    );
}
