//! Time one preview tick: the DNG color render, its readback, and the
//! display image the viewer is handed, at the sizes a drag would use.
//!
//! `cargo run --release -p deptheos_app --features examples --example preview_tick -- [RAW] [TICKS]`
//!
//! RAW (default: the e2e set `cr3/canon1`'s first frame) gives the render
//! params and the pixels, resized to each case's source. The render is per
//! pixel and cannot downscale, so each fit case renders a source already at
//! its size. Times are wall clock, each GPU step waited for.

use std::path::PathBuf;
use std::time::Instant;

use deptheos_app::engine::present::{ToDisplay, present_texture};
use deptheos_gpu::GpuContext;
use deptheos_gpu::gpu_utils::download_texture_rows;
use deptheos_render::dng_render::{DngRenderPipeline, PreparedRender, Region, Source};
use deptheos_render::{Color, Draw, OutputSpace, resolve};
use image::imageops::FilterType;

/// The full-size picture: 45 MP.
const FULL: [u32; 2] = [8256, 5504];
/// The viewport at 100%.
const VIEWPORT: [u32; 2] = [2560, 1600];
const WARM_UP: usize = 20;

struct Case {
    name: &'static str,
    /// The source texture's size.
    source: [u32; 2],
    /// The rendered region, from the source's top left.
    region: [u32; 2],
}

/// Median and 95th percentile of `ms`, sorted in place.
fn stats(ms: &mut [f64]) -> (f64, f64) {
    ms.sort_by(f64::total_cmp);
    let at = |q: f64| ms[((ms.len() - 1) as f64 * q).round() as usize];
    (at(0.5), at(0.95))
}

fn wait(ctx: &GpuContext) {
    let _ = ctx.device.poll(wgpu::PollType::wait_indefinitely());
}

/// `rgb` resized to `size` and uploaded in `ctx.source_format()`.
fn upload(
    ctx: &GpuContext,
    rgb: &image::ImageBuffer<image::Rgb<u16>, Vec<u16>>,
    size: [u32; 2],
) -> wgpu::Texture {
    let [w, h] = size;
    let resized = image::imageops::resize(rgb, w, h, FilterType::Triangle);
    let format = ctx.source_format();
    let bytes: Vec<u8> = match format {
        wgpu::TextureFormat::Rgba16Unorm => resized
            .pixels()
            .flat_map(|p| [p[0], p[1], p[2], u16::MAX])
            .flat_map(u16::to_le_bytes)
            .collect(),
        _ => resized
            .pixels()
            .flat_map(|p| [p[0], p[1], p[2], u16::MAX])
            .flat_map(|v| (f32::from(v) / 65535.0).to_le_bytes())
            .collect(),
    };
    let texture = ctx.create_texture(
        w,
        h,
        format,
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        "preview_tick_source",
    );
    let texel = format.block_copy_size(None).unwrap();
    ctx.queue.write_texture(
        texture.as_image_copy(),
        &bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(w * texel),
            rows_per_image: Some(h),
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    wait(ctx);
    texture
}

fn main() {
    let mut args = std::env::args().skip(1);
    let raw = args.next().map(PathBuf::from).unwrap_or_else(|| {
        std::env::home_dir().unwrap().join("DeptheosE2E/sets/cr3/canon1/9U1A5301.CR3")
    });
    let ticks: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(200);

    let ctx = GpuContext::new().expect("a GPU");
    let (linear, dng) =
        deptheos_app::decode_linear_with_metadata(&raw, Default::default()).expect("a RAW");
    let params = dng.render_params().expect("render params");
    let rgb = linear.to_rgb16();
    println!(
        "{} ({}x{}) on {} ({:?}), rendering to {:?}",
        raw.display(),
        rgb.width(),
        rgb.height(),
        ctx.adapter_info.name,
        ctx.adapter_info.backend,
        ctx.source_format()
    );

    let fit = |side: u32| [side, side * FULL[1] / FULL[0]];
    let cases = [
        Case { name: "fit 2048", source: fit(2048), region: fit(2048) },
        Case { name: "fit 1280", source: fit(1280), region: fit(1280) },
        Case { name: "100% 2560x1600 of 45 MP", source: FULL, region: VIEWPORT },
        Case { name: "full 45 MP", source: FULL, region: FULL },
    ];

    let pipeline = DngRenderPipeline::new(&ctx);
    let draw = resolve(Color::Linear(&params), None, OutputSpace::Srgb);
    let Draw::Rendered(constants) = draw else { unreachable!("a RAW renders on the GPU") };
    let prepared = PreparedRender::new(&ctx, &constants);
    println!(
        "{:<26} {:>12} {:>12} {:>12} {:>12} {:>12}",
        "case (median / p95 ms)", "pass", "readback", "display img", "pass+display", "tiles+levels"
    );
    for case in &cases {
        let source = upload(&ctx, &rgb, case.source);
        let [w, h] = case.region;
        let output = DngRenderPipeline::create_output(&ctx, w, h);
        let n = if case.region == FULL { ticks.min(30) } else { ticks };
        let (mut pass, mut readback, mut display, mut tick, mut tiles) =
            (vec![], vec![], vec![], vec![], 0usize);
        for i in 0..WARM_UP + n {
            let keep = i >= WARM_UP;
            let ms = |t: Instant| t.elapsed().as_secs_f64() * 1e3;

            let t = Instant::now();
            let region = Region { x: 0, y: 0, width: case.region[0], height: case.region[1] };
            pipeline.draw(&ctx, &prepared, Source::whole(&source), region, &output);
            wait(&ctx);
            let p = ms(t);

            let t = Instant::now();
            download_texture_rows(&ctx, &output, [0, 0], [w, h], |_, _, _, _| {}).unwrap();
            let r = ms(t);

            let t = Instant::now();
            let shown = present_texture(&ctx, &output, &ToDisplay::SRGB).unwrap();
            let d = ms(t);

            // One tick as the viewer would see it: render, then the
            // display image, with no wait between.
            let t = Instant::now();
            let region = Region { x: 0, y: 0, width: case.region[0], height: case.region[1] };
            pipeline.draw(&ctx, &prepared, Source::whole(&source), region, &output);
            let again = present_texture(&ctx, &output, &ToDisplay::SRGB).unwrap();
            let k = ms(t);

            tiles = shown.levels.iter().map(|l| l.tiles.len()).sum();
            drop((shown, again));
            if keep {
                pass.push(p);
                readback.push(r);
                display.push(d);
                tick.push(k);
            }
        }
        let cell = |v: &mut Vec<f64>| {
            let (m, p) = stats(v);
            format!("{m:.2} / {p:.2}")
        };
        println!(
            "{:<26} {:>12} {:>12} {:>12} {:>12} {:>12}",
            case.name,
            cell(&mut pass),
            cell(&mut readback),
            cell(&mut display),
            cell(&mut tick),
            tiles
        );
    }
    println!(
        "readback: the copy to CPU alone; display img: present_texture, the readback \
         converted to BGRA8 with its half-size levels and tiles (what Atlas is handed)"
    );
}
