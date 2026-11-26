//! A reframed export: its 16-bit pixels are the window of the same render
//! drawn whole, flipped and turned.

use deptheos_app::reframe::{Flip, Rect, Reframe, Size, Turn};
use deptheos_app::{Adjustments, Look, OutputSpace, SaveOptions};
use deptheos_gpu::GpuContext;

#[test]
fn a_reframed_export_is_the_window_of_the_whole() {
    if GpuContext::new_or_skip().is_none() {
        return;
    }
    let engine = deptheos_app::test_support::engine().unwrap();
    let (w, h): (u32, u32) = (97, 61);
    let pixels = (0..w * h * 3).map(|i| (i.wrapping_mul(2_654_435_761) >> 16) as u16).collect();
    let stack = deptheos_app::CompletedStack::srgb(w, h, pixels, Vec::new());
    let picture = Size { width: w, height: h };
    let dir = tempfile::tempdir().unwrap();
    let neutral = Adjustments::NEUTRAL;
    let draw = |reframe: Reframe, name: &str| {
        let path = dir.path().join(name);
        let look = Look::redrawn(&stack, &neutral, reframe, OutputSpace::StackProfile);
        engine.save(look, &path, SaveOptions::default()).unwrap();
        image::open(&path).unwrap().into_rgb16()
    };
    let whole = draw(Reframe::NONE, "whole.tif");
    let crop = Rect { x: 13, y: 7, width: 41, height: 29 };
    for (flip, turn) in [(Flip::None, Turn::None), (Flip::Horizontal, Turn::Quarter)] {
        let reframe = Reframe { crop: Some(crop), flip, turn };
        let framed = draw(reframe, "framed.tif");
        let size = reframe.output_size(picture).unwrap();
        assert_eq!(framed.dimensions(), (size.width, size.height));
        let (expected, _) = reframe.apply(whole.as_raw(), picture, 3).unwrap();
        assert!(framed.as_raw() == &expected, "{reframe:?}");
    }
}
