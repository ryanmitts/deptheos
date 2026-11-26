//! Frames aligned as a stack aligns them, each written in the stack's
//! geometry and color (the CLI's `align`).
//!
//! [`Engine::align`] runs an ordinary Pyramid stack and drops its result,
//! so frames are aligned, reversed, left out and cropped exactly as a stack
//! does. [`Engine::save_aligned`] re-reads one frame, warps it on the GPU
//! as a compare source is (`Engine::aligned_source`), and saves it as the
//! stack's result would be. One frame is in memory at a time.

use std::path::{Path, PathBuf};

use image::math::Rect;

use crate::engine::Engine;
use crate::runs::session::{
    CompletedStack, LiveMap, SaveOptions, SessionEvent, SessionOptions, StackGeometry, download_rgb,
};
use crate::runs::stacker::StackerConfig;
use crate::words::Words;
use crate::{Error, Result};

/// What [`Engine::align`] decided, for [`Engine::save_aligned`]: geometry,
/// crop, what was left out and the color.
pub struct AlignedFrames {
    /// What the stack decided, its pixels and maps let go of.
    stack: CompletedStack,
}

impl AlignedFrames {
    /// The frames, in the order given.
    pub fn paths(&self) -> &[PathBuf] {
        &self.stack.paths
    }

    pub fn geometry(&self) -> &StackGeometry {
        &self.stack.geometry
    }

    /// The size aligned frames are written at: the crop's, else the
    /// reference's.
    pub fn size(&self) -> (u32, u32) {
        (self.stack.width, self.stack.height)
    }

    /// Whether the frames are linear (RAW or DNG), as DNG output needs.
    pub fn is_linear(&self) -> bool {
        self.stack.is_linear()
    }

    /// The frames aligned (indexes into [`Self::paths`]), in order.
    pub fn aligned(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.stack.paths.len()).filter(|&i| !self.stack.geometry.is_left_out(i))
    }

    /// What was left out and why, as a stack's notice says it.
    pub fn left_out_message(&self, words: &Words) -> Option<String> {
        self.stack.left_out_message(words)
    }

    /// What loading the frames had to assume or could not keep.
    pub fn notes(&self) -> &[String] {
        &self.stack.notes
    }
}

impl Engine {
    /// Align `paths` as a stack with `options` would, on the calling
    /// thread; `cancel` stops it between frames ([`Error::Canceled`]). Hold
    /// Adobe conversions ([`Self::convert_first`]) so [`Self::save_aligned`]
    /// doesn't convert each frame twice.
    pub fn align(
        &self,
        paths: Vec<PathBuf>,
        options: SessionOptions,
        on_event: &mut dyn FnMut(SessionEvent, &LiveMap),
        cancel: &deptheos_core::Cancel,
    ) -> Result<AlignedFrames> {
        if !options.alignment.is_on() {
            return Err(Error::InvalidState("aligning frames with alignment off".into()));
        }
        // Pyramid reads its frames once.
        let config = StackerConfig::Pyramid(Default::default());
        let output = self.session(paths, config, options)?.run(on_event, cancel)?;
        let stack = CompletedStack { pixels: Vec::new(), frame_map: None, ..output.stack };
        tracing::debug!(
            "aligned {} frames to {}x{} (crop {:?}, reversed {})",
            stack.paths.len(),
            stack.width,
            stack.height,
            stack.geometry.crop,
            stack.geometry.reversed
        );
        Ok(AlignedFrames { stack })
    }

    /// Frame `index` re-read, aligned and saved at `path` as
    /// [`CompletedStack::save`] would, with its own camera metadata.
    pub fn save_aligned(
        &self,
        frames: &AlignedFrames,
        index: usize,
        path: &Path,
        save: SaveOptions,
    ) -> Result<()> {
        let stack = &frames.stack;
        let frame = stack.paths.get(index).ok_or_else(|| {
            Error::InvalidState(format!("no frame {index} of {}", stack.paths.len()))
        })?;
        if stack.geometry.is_left_out(index) {
            return Err(Error::InvalidState(format!(
                "{} was left out, so it has no alignment",
                frame.display()
            )));
        }
        // Linear for a linear stack, rendered or written as DNG on save.
        let warped = self.aligned_source(frame, Some(index), stack)?;
        let size = (warped.width(), warped.height());
        if size != frames.size() {
            return Err(Error::InvalidState(format!(
                "{} warped to {}x{}, not the stack's {}x{}",
                frame.display(),
                size.0,
                size.1,
                stack.width,
                stack.height
            )));
        }
        let rect = Rect { x: 0, y: 0, width: size.0, height: size.1 };
        let pixels = download_rgb(self.gpu(), &warped, rect)?;
        drop(warped);
        let tags = crate::library::export_tags::aligned_frame_metadata(stack, index);
        stack.save_tagged(self, &pixels, path, save, &tags)
    }
}
