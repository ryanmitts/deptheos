//! Thumbnail time per file, and for a folder on a few threads.
//!
//! `cargo run --release -p deptheos_app --features examples --example thumb_bench -- SIZE FILE...`

use std::path::PathBuf;
use std::time::Instant;

use rayon::prelude::*;

fn main() {
    let mut args = std::env::args().skip(1);
    let size: u32 = args.next().and_then(|s| s.parse().ok()).expect("SIZE FILE...");
    let files: Vec<PathBuf> = args.map(PathBuf::from).collect();
    for path in files.iter().take(4) {
        let start = Instant::now();
        match deptheos_app::thumbnail(path, size) {
            Ok(t) => {
                // THUMB_OUT=dir saves these first few as PNG, to look at.
                if let Some(dir) = std::env::var_os("THUMB_OUT") {
                    let name = path.file_stem().unwrap().to_string_lossy().into_owned();
                    // Thumbnails are BGRA.
                    let mut rgba = t.image.pixels.clone();
                    rgba.as_chunks_mut::<4>().0.iter_mut().for_each(|p| p.swap(0, 2));
                    image::RgbaImage::from_raw(t.image.width, t.image.height, rgba)
                        .unwrap()
                        .save(PathBuf::from(dir).join(format!("{name}.png")))
                        .unwrap();
                }
                println!(
                    "{}: {}x{} {:?} in {:.1} ms",
                    path.display(),
                    t.image.width,
                    t.image.height,
                    t.source,
                    start.elapsed().as_secs_f64() * 1e3
                )
            }
            Err(e) => println!("{}: {e}", path.display()),
        }
    }
    let pool = rayon::ThreadPoolBuilder::new().num_threads(4).build().unwrap();
    let start = Instant::now();
    let ok = pool
        .install(|| files.par_iter().filter(|p| deptheos_app::thumbnail(p, size).is_ok()).count());
    let secs = start.elapsed().as_secs_f64();
    println!(
        "{ok}/{} on 4 threads in {secs:.2} s ({:.1} ms each)",
        files.len(),
        secs * 1e3 / files.len() as f64
    );
}
