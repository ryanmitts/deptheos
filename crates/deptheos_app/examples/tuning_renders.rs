//! Stack folders of frames into a Library, one render each, so `deptheos
//! redraw` can draw them under adjustments: the tuning set's renders.
//!
//! `cargo run --release -p deptheos_app --features examples --example tuning_renders -- LIBRARY FOLDER...`
//!
//! Each folder's images, sorted by name, stack with the pyramid method's
//! defaults and alignment on. The render's folder is printed with its
//! source folder. LIBRARY must not be one an app has open.

use std::path::{Path, PathBuf};

use deptheos_app::library::history::{History, Made, StackId};
use deptheos_app::{
    Adjustments, Cancel, Engine, Platform, PyramidConfig, SessionOptions, StackerConfig, Words,
};

/// The image files in `dir`, sorted by name.
fn frames(dir: &Path) -> Vec<PathBuf> {
    let image = |p: &Path| {
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        let kinds = ["jpg", "jpeg", "png", "tif", "tiff", "dng", "cr3", "cr2", "orf", "nef", "arw"];
        kinds.contains(&ext.as_str())
    };
    let mut frames: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && image(p))
        .collect();
    frames.sort();
    frames
}

fn main() {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let [library, folders @ ..] = args.as_slice() else {
        eprintln!("usage: tuning_renders LIBRARY FOLDER...");
        std::process::exit(2);
    };
    let words = Words::english(Platform::Desktop);
    let engine = Engine::new(words).expect("an engine");
    let history = History::open(library).expect("the Library");
    for folder in folders {
        let frames = frames(folder);
        let config = StackerConfig::Pyramid(PyramidConfig::default());
        let session =
            engine.session(frames.clone(), config, SessionOptions::default()).expect("a session");
        let stack = session.run(&mut |_, _| {}, &Cancel::root()).expect("the stack").stack;
        let paths = stack.paths.clone();
        let presented = engine.present_stack(stack, &Adjustments::NEUTRAL).expect("shown");
        let thumbnail = presented.thumbnail().expect("a thumbnail");
        let render = history
            .save_render(
                &paths,
                &presented.stack,
                &thumbnail,
                &StackId::new(),
                &Made::Stacked { base: None },
            )
            .expect("saved");
        println!("{}\t{}", history.render_path(&render.id).display(), folder.display());
    }
}
