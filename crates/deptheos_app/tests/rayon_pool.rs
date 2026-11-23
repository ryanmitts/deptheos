//! The engine builds rayon's global pool with deep stacks: its own test
//! binary, since the pool is the process's and is built once.

use deptheos_app::{Engine, Platform, Words};
use deptheos_gpu::GpuContext;

/// Uses about `depth` × 64 KiB of stack.
fn deep(depth: usize) -> u8 {
    let frame = std::hint::black_box([depth as u8; 64 << 10]);
    if depth == 0 { frame[0] } else { deep(depth - 1).wrapping_add(frame[1]) }
}

#[test]
fn rayon_threads_have_deep_stacks() {
    let Some(gpu) = GpuContext::new_or_skip() else { return };
    let _engine = Engine::with_gpu(gpu, Words::english(Platform::Desktop));
    // About 6 MiB: more than rayon's default 2 MiB, inside the 8 MiB asked.
    let (a, b) = rayon::join(|| deep(96), || deep(96));
    assert_eq!(a, b);
}
