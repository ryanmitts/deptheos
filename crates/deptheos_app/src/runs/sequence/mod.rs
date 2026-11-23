//! Grow and turntable sequences: numbered frames of a stack coming into
//! focus, or of an object turning, for an editor to import as one clip.
//!
//! - **Grow** (one stack): frame *k* is the stack of *k* frames, front to
//!   back or back to front, from one replay with a snapshot per frame
//!   ([`StackingSession::run_snapshots`]); the last frame is the render.
//! - **Turntable** (one stack per angle): *full* is each render cut to the
//!   shared crop; *grow* turns and focuses at once, frame *i* of *N* a
//!   partial stack of stack ≈ *i*/*N* ([`turntable_grow_frames`]).
//!
//! Every frame gets the render's steps (crop, a linear stack's color step,
//! an optional resize) and is named `<name>_0001.<ext>` in a new folder,
//! with a `manifest.json` saying what each is (gaps never show in the
//! numbering).

use std::io;
use std::path::{Path, PathBuf};

use image::math::Rect;
use serde::Serialize;

use crate::Error;
use crate::StoreError;
use crate::frames::names::file_label;
use crate::library::export::ExportFormat;
use crate::runs::session::{SaveOptions, StackColor};
use crate::words::{Words, say};

mod check;
mod grow;
mod turntable;
mod writer;

pub use check::*;
pub use grow::*;
pub use turntable::*;
pub use writer::*;

/// The manifest's own format version.
pub(crate) const MANIFEST_VERSION: u32 = 1;
pub const MANIFEST_FILE: &str = "manifest.json";

/// Which end of the bracket a grow starts from; "front" is the nearest
/// focus (the stack's reference), whichever end of the list that is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// The farthest frame first: focus arrives from behind the subject.
    #[default]
    BackToFront,
    FrontToBack,
}

impl Direction {
    pub const ALL: [Self; 2] = [Self::BackToFront, Self::FrontToBack];
}

/// The image format of a sequence's frames; no DNG, as frames are rendered
/// pictures.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FrameFormat {
    #[default]
    Tiff,
    Png,
    Jpeg,
}

/// A frame is written as the export of that format is, 16 bits included
/// ([`ExportFormat::depth`]).
impl From<FrameFormat> for ExportFormat {
    fn from(format: FrameFormat) -> Self {
        match format {
            FrameFormat::Tiff => Self::Tiff,
            FrameFormat::Png => Self::Png,
            FrameFormat::Jpeg => Self::Jpeg,
        }
    }
}

impl FrameFormat {
    pub const ALL: [Self; 3] = [Self::Tiff, Self::Jpeg, Self::Png];

    /// Its file extension, which is also how the CLI spells it.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Tiff => "tif",
            Self::Png => "png",
            Self::Jpeg => "jpg",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Tiff => "TIFF",
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
        }
    }
}

/// How each frame is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameOptions {
    pub format: FrameFormat,
    /// 16-bit and JPEG quality, as Export… has them.
    pub save: SaveOptions,
    /// The long side to scale down to; `None` keeps the full size.
    pub long_edge: Option<u32>,
}

impl Default for FrameOptions {
    fn default() -> Self {
        Self { format: FrameFormat::Tiff, save: SaveOptions::default(), long_edge: None }
    }
}

impl FrameOptions {
    /// Whether frames of a stack in `color` are written with 16 bits.
    pub fn sixteen_bit(&self, color: &StackColor) -> bool {
        self.sixteen_bit_from(match color {
            StackColor::Linear { .. } => true,
            StackColor::Srgb { sixteen_bit, .. } => *sixteen_bit,
        })
    }

    /// Whether frames are written with 16 bits: asked for, held by the
    /// format, and in the `source`.
    pub fn sixteen_bit_from(&self, source: bool) -> bool {
        ExportFormat::from(self.format).depth(self.save.sixteen_bit, source)
    }

    /// The size a `width`×`height` frame is written at.
    pub fn output_size(&self, width: u32, height: u32) -> (u32, u32) {
        scaled_size(width, height, self.long_edge)
    }
}

/// `width`×`height` scaled down to `long_edge` on its long side
/// ([`crate::engine::present::fit_long_edge`]); unchanged when `long_edge` is `None`.
fn scaled_size(width: u32, height: u32, long_edge: Option<u32>) -> (u32, u32) {
    long_edge
        .map_or((width, height), |edge| crate::engine::present::fit_long_edge(width, height, edge))
}

// --- Progress.

/// Which sequence is being written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SequencePhase {
    Grow,
    TurntableFull,
    TurntableGrow,
}

impl SequencePhase {
    /// "Grow sequence", "Turntable sequence · Full", ….
    pub fn label(self, words: &Words) -> String {
        match self {
            Self::Grow => say!(words, "sequence-grow"),
            Self::TurntableFull => say!(words, "sequence-turntable-full"),
            Self::TurntableGrow => say!(words, "sequence-turntable-grow"),
        }
    }
}

/// How far a sequence is, for a progress line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SequenceProgress {
    pub phase: SequencePhase,
    /// The stack being replayed, from 1, of how many.
    pub stack: Option<(usize, usize)>,
    pub written: usize,
    pub total: usize,
}

impl SequenceProgress {
    /// "Turntable sequence · … · stack 12 of 50 · frame 48 of 200 written".
    pub fn message(&self, words: &Words) -> String {
        let phase = self.phase.label(words);
        let (written, total) = (self.written, self.total);
        match self.stack {
            Some((stack, stacks)) => say!(
                words,
                "sequence-progress-stack",
                phase = phase,
                stack = stack,
                stacks = stacks,
                written = written,
                total = total
            ),
            None => {
                say!(words, "sequence-progress", phase = phase, written = written, total = total)
            }
        }
    }
}

/// What a sequence run made.
#[derive(Clone, Debug)]
pub struct SequenceReport {
    pub folder: PathBuf,
    pub frames: usize,
}

impl SequenceReport {
    /// "24 frames in \"bee grow\"", the folder by its name.
    pub fn line(&self, words: &Words) -> String {
        say!(words, "sequence-made", count = self.frames, folder = file_label(&self.folder))
    }
}

// --- Disk space.

/// About how many bytes `frames` frames take: uncompressed for TIFF and
/// PNG (an upper bound), a typical rate for JPEG.
pub fn estimate_bytes(
    frames: usize,
    width: u32,
    height: u32,
    options: &FrameOptions,
    sixteen_bit: bool,
) -> u64 {
    let (w, h) = options.output_size(width, height);
    let pixels = u64::from(w) * u64::from(h);
    let per_frame = match options.format {
        FrameFormat::Jpeg => pixels * 3 / 4,
        FrameFormat::Png | FrameFormat::Tiff => pixels * if sixteen_bit { 6 } else { 3 },
    };
    per_frame * frames as u64
}

fn rect_array(r: Rect) -> [u32; 4] {
    [r.x, r.y, r.width, r.height]
}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> Error + '_ {
    move |source| StoreError::Io { path: path.to_path_buf(), source }.into()
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::*;

    #[test]
    fn sizes_and_estimates() {
        assert_eq!(scaled_size(8192, 5464, Some(3840)), (3840, 2561));
        assert_eq!(scaled_size(100, 50, Some(400)), (100, 50));
        let tiff16 = FrameOptions::default();
        // 250 frames of scan1 as 16-bit TIFF, about 67 GB.
        let bytes = estimate_bytes(250, 8192, 5464, &tiff16, true);
        let words = crate::Words::english(crate::Platform::Desktop);
        assert_eq!(crate::words::units::size_text(bytes, &words), "67 GB");
        let (pixels, w, h) = resize(vec![1000; 40 * 20 * 3].into(), 40, 20, Some(10)).unwrap();
        assert_eq!((w, h, pixels.len()), (10, 5, 150));
        assert!(pixels.iter().all(|&v| v.abs_diff(1000) <= 1));
        // At its own size a borrowed frame is passed through, not copied.
        let render = vec![7u16; 40 * 20 * 3];
        let (same, ..) = resize(Cow::Borrowed(&render), 40, 20, Some(40)).unwrap();
        assert!(matches!(same, Cow::Borrowed(p) if p.as_ptr() == render.as_ptr()));
    }
}
