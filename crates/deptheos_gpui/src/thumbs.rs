//! The file list's thumbnails, made on the engine's pool.
//!
//! [`Thumbs::request`] queues each file on the background lane, where
//! `deptheos_app::thumbnail` makes it (a RAW's embedded preview, a few ms)
//! and sends it back; the owner drains it with [`Thumbs::receive`] and
//! [`Thumbs::insert`]. A pending thumbnail holds its request, so a file
//! leaving the list cancels it. Thumbnails live in memory while their file
//! is listed, and [`Thumbs::retain`] drops the others from GPUI's atlas.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use deptheos_app::CpuImage;
use deptheos_app::engine::work::{self, Key, Lane, Request};
use futures::channel::mpsc;
use gpui::{App, RenderImage, Window};

use crate::images::render_image;

/// Pixels on a thumbnail's long edge: the list draws them 48 points wide,
/// so this covers a 2× display with room to spare.
pub const THUMB_SIZE: u32 = 128;

/// A finished thumbnail, or `None` when the file could not make one.
pub type Made = (PathBuf, Option<CpuImage>);

pub enum Thumb {
    /// Being made: dropping the request cancels it.
    Pending {
        _request: Request<()>,
    },
    Ready(Arc<RenderImage>),
    Failed,
}

pub struct Thumbs {
    entries: HashMap<PathBuf, Thumb>,
    sender: mpsc::UnboundedSender<Made>,
    /// Taken by the owner's drain task ([`Self::receive`]).
    receiver: Option<mpsc::UnboundedReceiver<Made>>,
}

impl Default for Thumbs {
    fn default() -> Self {
        let (sender, receiver) = mpsc::unbounded();
        Self { entries: HashMap::new(), sender, receiver: Some(receiver) }
    }
}

impl Thumbs {
    /// The finished thumbnails, once: the owner drains them and hands each
    /// to [`Self::insert`].
    pub fn receive(&mut self) -> Option<mpsc::UnboundedReceiver<Made>> {
        self.receiver.take()
    }

    pub fn get(&self, path: &Path) -> Option<&Thumb> {
        self.entries.get(path)
    }

    /// Make thumbnails for files that have none yet, in order.
    pub fn request(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        for path in paths {
            if self.entries.contains_key(&path) {
                continue;
            }
            let (sender, file) = (self.sender.clone(), path.clone());
            let key = Key::of("frame-thumbnail", &path);
            let request = work::pool().run(Lane::Background, key, work::root(), move || {
                let made = make(&file);
                let _ = sender.unbounded_send((file, made));
            });
            self.entries.insert(path, Thumb::Pending { _request: request });
        }
    }

    /// Store a finished thumbnail; one for a file no longer in the list is
    /// dropped. Returns whether it was kept.
    pub fn insert(&mut self, (path, image): Made) -> bool {
        match self.entries.get_mut(&path) {
            Some(entry @ Thumb::Pending { .. }) => {
                *entry = match image {
                    Some(image) => Thumb::Ready(render_image(image)),
                    None => Thumb::Failed,
                };
                true
            }
            _ => false,
        }
    }

    /// Keep only the files `keep` accepts: the rest are canceled, and
    /// their thumbnails leave the sprite atlas.
    pub fn retain(&mut self, keep: impl Fn(&Path) -> bool, window: &mut Window, cx: &mut App) {
        let gone: Vec<PathBuf> = self.entries.keys().filter(|p| !keep(p)).cloned().collect();
        for path in gone {
            if let Some(Thumb::Ready(image)) = self.entries.remove(&path) {
                cx.drop_image(image, Some(window));
            }
        }
    }
}

fn make(path: &Path) -> Option<CpuImage> {
    match deptheos_app::thumbnail(path, THUMB_SIZE) {
        Ok(thumb) => {
            tracing::debug!("thumbnail of {} from {:?}", path.display(), thumb.source);
            Some(thumb.image)
        }
        Err(e) => {
            tracing::debug!("no thumbnail for {}: {e}", path.display());
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workers_make_thumbnails_and_report_failures() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("frame.png");
        image::RgbImage::new(400, 300).save(&good).unwrap();
        let bad = dir.path().join("broken.cr3");
        std::fs::write(&bad, "not a raw file").unwrap();

        let mut thumbs = Thumbs::default();
        let mut made = thumbs.receive().unwrap();
        thumbs.request([good.clone(), bad.clone(), good.clone()]);
        let mut results: Vec<Made> = (0..2)
            .map(|_| futures::executor::block_on(futures::StreamExt::next(&mut made)).unwrap())
            .collect();
        results.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(results[0].0, bad);
        assert!(results[0].1.is_none());
        let image = results[1].1.as_ref().unwrap();
        assert_eq!((image.width, image.height), (THUMB_SIZE, THUMB_SIZE * 3 / 4));
        for result in results {
            assert!(thumbs.insert(result));
        }
        assert!(matches!(thumbs.get(&good), Some(Thumb::Ready(_))));
        assert!(matches!(thumbs.get(&bad), Some(Thumb::Failed)));
    }
}
