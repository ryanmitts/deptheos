//! Stack each folder given through the FFI as the iOS app does: the `image`
//! crate stands in for Apple's decoder (non-DNG), the DNG SDK reads DNGs.
//! Each stack is exported in every format it allows, kept as a render and
//! loaded back. Every call is the bridge's async one, awaited as Swift does.

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use deptheos_app::library::export::ExportFormat;
use deptheos_app::{AlignMode, RawBackend};
use deptheos_ffi::*;
use futures::executor::block_on;

struct ImageCrateDecoder;

impl FrameDecoder for ImageCrateDecoder {
    fn open_frame(&self, path: std::path::PathBuf) -> Result<Arc<dyn OpenedFrame>, EngineError> {
        let image = image::open(&path)
            .map_err(|e| EngineError::Failed { message: e.to_string() })?
            .into_rgba8();
        Ok(Arc::new(Rgba8(image)))
    }
}

/// A frame the `image` crate read, as RGBA at 8 bits.
struct Rgba8(image::RgbaImage);

impl OpenedFrame for Rgba8 {
    fn header(&self) -> FrameHeader {
        FrameHeader {
            width: self.0.width(),
            height: self.0.height(),
            depth: SampleDepth::Eight,
            layout: FrameLayout::Rgbx,
            color: FrameColor::Untagged,
            orientation: 1,
        }
    }

    fn decode_into(&self, destination: u64, length: u64) -> Result<(), EngineError> {
        let bytes = self.0.as_raw();
        assert_eq!(bytes.len() as u64, length);
        // SAFETY: the engine passes a live buffer of `length` bytes.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), destination as *mut u8, bytes.len())
        };
        Ok(())
    }
}

struct Printer;

impl StackObserver for Printer {
    fn progress(&self, progress: StackProgress) {
        let StackProgress { phase, current, total, .. } = progress;
        if matches!(phase, StackPhase::Started | StackPhase::Finalizing | StackPhase::Completed)
            || current == total
        {
            println!("  {phase:?} {current}/{total}");
        }
    }

    fn live_map(&self, _image: DisplayImage) {}
}

fn main() {
    let folders: Vec<String> = std::env::args().skip(1).collect();
    let store = tempfile::tempdir().unwrap();
    // SMOKE_STORE keeps the renders in that history (a Simulator app's, to
    // test with); otherwise they are temporary.
    let store_path = std::env::var("SMOKE_STORE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| store.path().to_path_buf());
    // SMOKE_OUT keeps the exports for a look; otherwise they are temporary.
    let temp = tempfile::tempdir().unwrap();
    let out_dir = std::env::var("SMOKE_OUT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| temp.path().to_path_buf());
    let engine =
        block_on(Engine::new(Arc::new(ImageCrateDecoder), store_path, None, "en".into())).unwrap();
    // The app's preferences as a fresh install has them.
    let preferences = default_preferences();

    for folder in folders {
        let mut frames: Vec<std::path::PathBuf> = std::fs::read_dir(&folder)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| {
                p.extension().is_some_and(|e| {
                    ["dng", "jpg", "jpeg", "png", "tif"]
                        .contains(&e.to_ascii_lowercase().to_str().unwrap())
                })
            })
            .collect();
        frames.sort();
        frames.truncate(24);
        println!("{folder}: {} frames", frames.len());

        let before = block_on(engine.clone().list_renders(Request::new())).unwrap().len();
        // Both renders of a folder are one stack, #1 and #2.
        let stack = new_stack_id();
        for algorithm in [
            Algorithm::Pyramid { smoothness: 4.0 },
            Algorithm::Depth { radius: 8.0, smoothing: 5.0, blend_range: 3.0 },
        ] {
            let settings = StackSettings {
                algorithm,
                align: true,
                align_method: AlignMode::Ecc,
                crop: true,
                // The app defaults to Apple's decoder; here camera RAW goes
                // through LibRaw, which iOS offers too.
                raw_decoder: RawBackend::LibRaw,
            };
            let start = Instant::now();
            let result = block_on(engine.stack(
                frames.clone(),
                settings,
                preferences.alignment,
                1024,
                Request::new(),
                Arc::new(Printer),
            ))
            .unwrap();
            println!(
                "  {}: {}x{} dng={} in {:.1?}",
                engine.words().algorithm_summary(algorithm),
                result.width(),
                result.height(),
                result.export_formats().contains(&ExportFormat::Dng),
                start.elapsed()
            );
            let display =
                block_on(result.clone().picture(Request::new(), PictureOf::Display, 4096));
            let display = display.unwrap().unwrap().base.unwrap();
            assert_eq!(display.bgra.len(), (display.width * display.height * 4) as usize);
            let map = block_on(result.clone().picture(Request::new(), PictureOf::Map, 512));
            assert!(map.unwrap().is_some());
            for format in result.export_formats() {
                let name = format!(
                    "{}-{}",
                    Path::new(&folder).file_name().unwrap().to_string_lossy(),
                    engine.words().algorithm_summary(algorithm).split(' ').next().unwrap()
                );
                let path = block_on(result.clone().export(
                    Request::new(),
                    out_dir.clone(),
                    name,
                    format,
                    preferences.export.clone(),
                ))
                .unwrap();
                let size = std::fs::metadata(&path).unwrap().len();
                println!(
                    "    {format:?}: {} ({:.1} MB)",
                    Path::new(&path).display(),
                    size as f64 / 1e6
                );
            }
            let saving = engine.clone().save_render(
                frames.clone(),
                result,
                stack.clone(),
                deptheos_ffi::RenderMade::Stacked { base: None },
            );
            let render = block_on(saving).unwrap();
            let back = block_on(engine.clone().load_render(Request::new(), render.id.clone()));
            assert_eq!(back.unwrap().frames(), frames);
            block_on(engine.clone().render_thumbnail(Request::new(), render.id.clone())).unwrap();
            let working = block_on(engine.clone().working_of_render(Request::new(), render.id));
            assert_eq!(list_rows(working.unwrap().list).len(), frames.len());
        }
        let renders = block_on(engine.clone().list_renders(Request::new())).unwrap();
        assert_eq!(renders.len(), before + 2);
        let numbers: Vec<u32> = renders[before..].iter().map(|r| r.number).collect();
        assert_eq!(numbers, [1, 2]);
        let name = renders[before].stack_name.as_deref().unwrap_or("an untitled stack");
        println!("  {name} has {} renders", renders.len() - before);
    }
    println!("ok");
}
