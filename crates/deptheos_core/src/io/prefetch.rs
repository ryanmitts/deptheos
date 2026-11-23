//! Decoding frames ahead of the consumer.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;

use crate::cancel::Cancel;
use crate::error::{CoreError, Result};

/// Decodes frames ahead of the consumer on background threads and hands
/// them out in order.
///
/// Decode is the CPU-bound half of a stack (a 44 MP JPEG ~110 ms, camera
/// RAW seconds), longer than the GPU's work per frame. `ahead` frames
/// decode concurrently, each up to ~270 MB at 44 MP 16-bit, so the session
/// picks `ahead` by source type (`SessionOptions::PREFETCH_*`). Dropping
/// the prefetcher cancels and joins the loads in flight, so none outlives
/// its session.
///
/// The loader is the caller's ([`with_loader`](Self::with_loader)), so
/// per-frame work (decode, alignment preparation, staging the upload) runs
/// on the decode threads.
pub struct FramePrefetcher<T: Send + 'static> {
    paths: Vec<PathBuf>,
    ahead: usize,
    next_spawn: usize,
    next_yield: usize,
    pending: VecDeque<std::io::Result<JoinHandle<Result<T>>>>,
    loader: Loader<T>,
    /// The loads' token, a child of the run's, canceled when the window is
    /// abandoned (drop, reorder).
    stop: Cancel,
    /// The run's token, for a window started again ([`Self::reorder`]).
    run: Cancel,
}

/// Loads one frame on a prefetch thread. Once the token is canceled the
/// loader may return early with any result, which is thrown away.
pub type Loader<T> = Arc<dyn Fn(&Path, &Cancel) -> Result<T> + Send + Sync>;

/// The name prefetch threads run under, for profilers and crash reports.
const THREAD_NAME: &str = "frame-prefetch";

impl<T: Send + 'static> FramePrefetcher<T> {
    /// Start loading the first `ahead` (at least 1) frames now, so the
    /// caller's setup overlaps them. The loads are canceled with `run`.
    pub fn with_loader(paths: Vec<PathBuf>, ahead: usize, loader: Loader<T>, run: &Cancel) -> Self {
        assert!(ahead >= 1, "a prefetch window of no frames");
        let mut frames = Self {
            paths,
            ahead,
            next_spawn: 0,
            next_yield: 0,
            pending: VecDeque::new(),
            loader,
            stop: run.child(),
            run: run.clone(),
        };
        frames.top_up();
        frames
    }

    /// Start over in a new order (the session's reverse check). Loads under
    /// way are thrown away, not kept: each holds a staging buffer from a
    /// pool sized for one window, so keeping them could deadlock the new
    /// window's next frame.
    pub fn reorder(self, paths: Vec<PathBuf>) -> Self {
        let (ahead, loader, run) = (self.ahead, self.loader.clone(), self.run.clone());
        drop(self);
        Self::with_loader(paths, ahead, loader, &run)
    }

    fn top_up(&mut self) {
        while self.pending.len() < self.ahead && self.next_spawn < self.paths.len() {
            let path = self.paths[self.next_spawn].clone();
            let (loader, stop) = (self.loader.clone(), self.stop.clone());
            let thread = crate::threads::thread(THREAD_NAME);
            self.pending.push_back(thread.spawn(move || loader(&path, &stop)));
            self.next_spawn += 1;
        }
    }
}

impl<T: Send + 'static> Iterator for FramePrefetcher<T> {
    type Item = Result<T>;

    /// The next frame in path order, or `None` past the end. A decode error
    /// is returned for the frame it belongs to; later frames keep decoding.
    fn next(&mut self) -> Option<Result<T>> {
        if self.next_yield >= self.paths.len() {
            return None;
        }
        self.top_up();
        let handle = self.pending.pop_front().expect("top_up spawned the next frame");
        // Keep the window full while the caller processes this frame.
        self.top_up();
        self.next_yield += 1;
        Some(join_load(handle))
    }
}

/// A load's result; a thread that could not be started fails its frame.
fn join_load<T>(handle: std::io::Result<JoinHandle<Result<T>>>) -> Result<T> {
    handle?
        .join()
        .unwrap_or_else(|_| Err(CoreError::InvalidState("frame decode thread panicked".into())))
}

impl<T: Send + 'static> Drop for FramePrefetcher<T> {
    /// Cancel and join the decodes in flight, so none keeps decoding (≈ 1 GB
    /// each for RAW) under the next session. They run concurrently, so this
    /// waits about one decode.
    fn drop(&mut self) {
        self.stop.cancel();
        for handle in self.pending.drain(..).flatten() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, Rgb};

    fn image_loader() -> Loader<DynamicImage> {
        Arc::new(|path: &Path, _: &Cancel| {
            crate::io::load_image(path, crate::io::CameraRaw::LibRaw)
        })
    }

    /// Dropping a prefetcher with decodes pending joins them.
    #[test]
    fn prefetcher_drop_joins_pending_decodes() {
        let dir = tempfile::tempdir().unwrap();
        let paths: Vec<PathBuf> = (0..4)
            .map(|i| {
                let path = dir.path().join(format!("f{i}.png"));
                image::ImageBuffer::<Rgb<u8>, Vec<u8>>::from_pixel(8, 8, Rgb([i as u8, 0, 0]))
                    .save(&path)
                    .unwrap();
                path
            })
            .collect();
        let mut frames = FramePrefetcher::with_loader(paths, 3, image_loader(), &Cancel::root());
        let first = frames.next().unwrap().unwrap();
        assert_eq!((first.width(), first.height()), (8, 8));
        assert_eq!(frames.next_yield, 1);
        assert_eq!(frames.pending.len(), 3, "window refilled behind the consumed frame");
        drop(frames);
        // Removable: every decode finished before the drop returned.
        dir.close().unwrap();
    }

    /// Loads run on named threads, and a drop cancels them before it waits.
    #[test]
    fn drop_stops_loads_that_check_the_flag() {
        let paths: Vec<PathBuf> = (0..3).map(|i| PathBuf::from(format!("f{i}"))).collect();
        let loader: Loader<String> = Arc::new(|_: &Path, stop: &Cancel| {
            let name = std::thread::current().name().unwrap_or_default().to_owned();
            if name != THREAD_NAME {
                return Ok(name);
            }
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            while !stop.is_canceled() {
                assert!(std::time::Instant::now() < deadline, "never told to stop");
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Err(CoreError::Canceled)
        });
        let frames = FramePrefetcher::with_loader(paths, 2, loader, &Cancel::root());
        let started = std::time::Instant::now();
        drop(frames);
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
    }

    /// `capacity` tokens, like the session's staging pool: a load takes one
    /// and holds it until its frame is dropped.
    #[derive(Clone)]
    struct Pool(Arc<(std::sync::Mutex<usize>, std::sync::Condvar)>);

    struct Held(String, Pool);

    impl Pool {
        fn new(capacity: usize) -> Self {
            Self(Arc::new((std::sync::Mutex::new(capacity), std::sync::Condvar::new())))
        }

        fn take(&self, name: String) -> Held {
            let (free, returned) = &*self.0;
            let mut free = free.lock().unwrap();
            while *free == 0 {
                free = returned.wait(free).unwrap();
            }
            *free -= 1;
            Held(name, self.clone())
        }
    }

    impl Drop for Held {
        fn drop(&mut self) {
            let (free, returned) = &*self.1.0;
            *free.lock().unwrap() += 1;
            returned.notify_all();
        }
    }

    /// A reversed restart with loads holding buffers from a pool of
    /// `window + 2` (as the session sizes it) doesn't deadlock.
    #[test]
    fn reorder_with_a_bounded_pool_does_not_hang() {
        let window = 4;
        let paths: Vec<PathBuf> = (0..10).map(|i| PathBuf::from(format!("f{i}"))).collect();
        let pool = Pool::new(window + 2);
        let loader: Loader<Held> = Arc::new(move |path: &Path, _: &Cancel| {
            let name = path.display().to_string();
            if name == "f9" {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Ok(pool.take(name))
        });
        let reversed: Vec<PathBuf> = paths.iter().rev().cloned().collect();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut frames = FramePrefetcher::with_loader(paths, window, loader, &Cancel::root());
            assert_eq!(frames.next().unwrap().unwrap().0, "f0");
            assert_eq!(frames.next().unwrap().unwrap().0, "f1");
            // f2..f5 are loading; restart reversed.
            let frames = frames.reorder(reversed);
            let order: Vec<String> = frames.map(|r| r.unwrap().0.clone()).collect();
            let _ = tx.send(order);
        });
        let order = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the reversed pass hung waiting for a buffer");
        assert_eq!(order, ["f9", "f8", "f7", "f6", "f5", "f4", "f3", "f2", "f1", "f0"]);
    }
}
