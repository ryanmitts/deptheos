//! The staging pool caps the frame-sized buffers a stack holds: with all of
//! them out, staging another frame waits for one to come back.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use deptheos_core::{StagedUpload, StagingPool};
use deptheos_gpu::GpuContext;
use image::{DynamicImage, RgbImage};

#[test]
fn staging_waits_for_a_buffer_once_all_are_out() {
    let Some(ctx) = GpuContext::new_or_skip() else { return };
    let image = DynamicImage::ImageRgb8(RgbImage::new(256, 256));
    let pool = StagingPool::new(2);

    let a = StagedUpload::with_pool(&ctx, &image, Some(&pool)).unwrap();
    let b = StagedUpload::with_pool(&ctx, &image, Some(&pool)).unwrap();
    assert_eq!(pool.out(), 2);

    let staged = AtomicBool::new(false);
    std::thread::scope(|s| {
        let third = s.spawn(|| {
            let c = StagedUpload::with_pool(&ctx, &image, Some(&pool)).unwrap();
            staged.store(true, Ordering::SeqCst);
            c
        });
        std::thread::sleep(Duration::from_millis(100));
        assert!(!staged.load(Ordering::SeqCst), "a third buffer past the cap of two");

        // Dropping a frame hands its buffer back once the GPU is done with
        // it, and the waiting stage takes it.
        drop(a);
        let c = third.join().unwrap();
        assert!(staged.load(Ordering::SeqCst));
        assert_eq!(pool.out(), 2);
        drop((b, c));
    });
}
