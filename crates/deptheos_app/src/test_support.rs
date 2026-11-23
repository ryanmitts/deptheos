//! Shared test helpers (one GPU engine, checkerboard frames), for this crate's
//! tests and others' through the `test-support` feature.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use image::{ImageBuffer, Rgb};

use crate::{Engine, Platform, Words};

/// The engine, made once per test binary; `None` without a GPU, and the test
/// skips (`GpuContext::skip_test`).
pub fn engine() -> Option<Arc<Engine>> {
    static ENGINE: OnceLock<Option<Arc<Engine>>> = OnceLock::new();
    ENGINE
        .get_or_init(|| {
            Engine::new(Words::english(Platform::Desktop))
                .map_err(|e| crate::GpuContext::skip_test(&format!("no GPU engine ({e})")))
                .ok()
        })
        .clone()
}

/// `count` checkerboard frames of `width` × `height` in `dir`, named
/// `frame_0.png` on, each slightly brighter than the last.
pub fn checkerboard_frames(dir: &Path, count: u8, (width, height): (u32, u32)) -> Vec<PathBuf> {
    (0..count)
        .map(|i| {
            let path = dir.join(format!("frame_{i}.png"));
            ImageBuffer::from_fn(width, height, |x, y| {
                if ((x / 8) + (y / 8)).is_multiple_of(2) {
                    Rgb([200 + i * 10, 170, 60])
                } else {
                    Rgb([30, 60, 120])
                }
            })
            .save(&path)
            .unwrap();
            path
        })
        .collect()
}
