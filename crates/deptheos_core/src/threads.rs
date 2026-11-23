//! The builder every engine thread is made by (ADR-143): a name and an
//! 8 MiB stack, a main thread's, since the decoders (LibRaw keeps a ≈ 750 KB
//! object) aren't measured against Rust's 2 MiB or Apple's 512 KB.
//!
//! Here, below `deptheos_app::engine::work`, because the prefetch threads
//! are this crate's.

use std::thread::Builder;

/// The stack every engine thread gets, rayon's included.
pub const STACK_SIZE: usize = 8 << 20;

/// A named thread with [`STACK_SIZE`].
pub fn thread(name: impl Into<String>) -> Builder {
    Builder::new().name(name.into()).stack_size(STACK_SIZE)
}

/// The CPU budget's `cores`: the threads the machine runs at once.
pub fn cores() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

#[cfg(test)]
mod tests {
    /// 4 MB of locals fit on an engine thread.
    #[test]
    fn threads_have_a_main_thread_stack() {
        #[inline(never)]
        fn deep() -> u8 {
            let buffer = std::hint::black_box([7u8; 4 << 20]);
            buffer[buffer.len() - 1]
        }
        let ran = super::thread("stack-test").spawn(deep).unwrap().join().unwrap();
        assert_eq!(ran, 7);
    }
}
