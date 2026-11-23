//! Peak memory of memory-heavy steps on synthetic pictures: the heap's peak
//! (a counting allocator) and the process footprint before, at peak and after.
//!
//! `cargo run --release -p deptheos_app --features examples --example memory_probe -- CASE WIDTH HEIGHT`
//!
//! CASE: `present` (display pyramid from RGB u16), `from_rgb` (8-bit display
//! image), `jxl` (lossless JPEG XL save to the temp folder; `PROBE_SOURCE`
//! names a photo to resize instead), `readback` (GPU texture to display),
//! `thumbnail` (a JPEG without an EXIF thumbnail, against a whole decode).

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Counting;

static NOW: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            let now = NOW.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(now, Ordering::Relaxed);
        }
        p
    }

    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        unsafe { System.dealloc(p, layout) };
        NOW.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, layout, size) };
        if !q.is_null() {
            if size > layout.size() {
                let now =
                    NOW.fetch_add(size - layout.size(), Ordering::Relaxed) + size - layout.size();
                PEAK.fetch_max(now, Ordering::Relaxed);
            } else {
                NOW.fetch_sub(layout.size() - size, Ordering::Relaxed);
            }
        }
        q
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

fn mb(bytes: usize) -> String {
    format!("{:.0} MB", bytes as f64 / 1e6)
}

fn footprint() -> usize {
    deptheos_app::engine::memory::footprint().unwrap_or(0) as usize
}

/// Run `step`, printing the heap's peak and the footprint before, at peak
/// (sampled each millisecond, counting GPU and library allocations) and after.
fn measure<T>(what: &str, step: impl FnOnce() -> T) -> T {
    let start = NOW.load(Ordering::Relaxed);
    PEAK.store(start, Ordering::Relaxed);
    let before = footprint();
    let started = std::time::Instant::now();
    let done = std::sync::atomic::AtomicBool::new(false);
    let (out, top) = std::thread::scope(|s| {
        let sampler = s.spawn(|| {
            let mut top = 0;
            while !done.load(Ordering::Relaxed) {
                top = top.max(footprint());
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            top
        });
        let out = step();
        done.store(true, Ordering::Relaxed);
        (out, sampler.join().unwrap())
    });
    let took = started.elapsed().as_secs_f64() * 1e3;
    let peak = PEAK.load(Ordering::Relaxed) - start;
    let kept = NOW.load(Ordering::Relaxed).saturating_sub(start);
    println!(
        "{what} in {took:.0} ms: heap peak {}, kept {}; footprint {} -> peak {} -> {}",
        mb(peak),
        mb(kept),
        mb(before),
        mb(top),
        mb(footprint())
    );
    out
}

fn main() {
    let mut args = std::env::args().skip(1);
    let case = args.next().expect("CASE WIDTH HEIGHT");
    let width: u32 = args.next().and_then(|s| s.parse().ok()).expect("WIDTH");
    let height: u32 = args.next().and_then(|s| s.parse().ok()).expect("HEIGHT");
    let pixels = width as usize * height as usize;
    println!("{case} at {width}x{height} ({:.1} MP)", pixels as f64 / 1e6);
    match case.as_str() {
        "present" => {
            let rgb: Vec<u16> =
                (0..pixels * 3).map(|i| (i.wrapping_mul(2_654_435_761) >> 7) as u16).collect();
            let shown = measure("present_rgb", || {
                deptheos_app::engine::present::present_rgb(&rgb, width, height, |v: u16| {
                    (v >> 8) as u8
                })
            });
            println!("  the pyramid holds {}", mb(shown.bytes()));
        }
        "from_rgb" => {
            let rgb =
                image::RgbImage::from_fn(width, height, |x, y| image::Rgb([x as u8, y as u8, 7]));
            let image = measure("CpuImage::from_rgb", || deptheos_app::CpuImage::from_rgb(&rgb));
            println!("  the image holds {}", mb(image.pixels.len()));
        }
        "jxl" => {
            // Photo-like noise in the low bits, for a realistic file size.
            let rgb: Vec<u16> = match std::env::var_os("PROBE_SOURCE") {
                Some(photo) => {
                    let photo = image::open(photo).unwrap().into_rgb16();
                    image::imageops::resize(&photo, width, height, image::imageops::CatmullRom)
                        .into_raw()
                }
                None => (0..pixels * 3)
                    .map(|i| {
                        let smooth = (i / 3 % width as usize) * 5 + (i / 3 / width as usize) * 3;
                        let noise = (i.wrapping_mul(2_654_435_761) >> 13) & 0x3FF;
                        (smooth + noise + i % 3 * 4000) as u16
                    })
                    .collect(),
            };
            let path = std::env::temp_dir().join("memory_probe.jxl");
            measure("JPEG XL save", || {
                let mut file = std::fs::File::create(&path).unwrap();
                deptheos_dng::jxl::encode_rgb16(
                    &mut file,
                    &rgb,
                    width,
                    height,
                    deptheos_dng::jxl::FAST_EFFORT,
                    false,
                    deptheos_dng::Budget::unlimited(),
                )
                .unwrap();
            });
            println!("  {} bytes written", std::fs::metadata(&path).unwrap().len());
            let _ = std::fs::remove_file(path);
        }
        "readback" => {
            let ctx = deptheos_gpu::GpuContext::new().expect("a GPU");
            let texture = ctx.create_texture(
                width,
                height,
                wgpu::TextureFormat::Rgba16Unorm,
                wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
                "probe",
            );
            let _ = ctx.device.poll(wgpu::PollType::wait_indefinitely());
            let shown = measure("present_texture", || {
                deptheos_app::engine::present::present_texture(&ctx, &texture, None).unwrap()
            });
            println!("  the pyramid holds {}", mb(shown.bytes()));
        }
        "thumbnail" => {
            let path = std::env::temp_dir().join("memory_probe.jpg");
            image::RgbImage::from_fn(width, height, |x, y| image::Rgb([x as u8, y as u8, 7]))
                .save(&path)
                .unwrap();
            measure("decoded whole", || {
                let raw = deptheos_core::io::CameraRaw::here();
                let whole = deptheos_core::io::load_frame(
                    &path,
                    deptheos_core::io::FileCache::Use,
                    raw,
                    deptheos_core::io::Target::Own,
                );
                whole.unwrap().image.width()
            });
            measure("thumbnail", || deptheos_app::thumbnail(&path, 128).unwrap().image.width);
            let _ = std::fs::remove_file(path);
        }
        other => panic!("no case {other}"),
    }
}
