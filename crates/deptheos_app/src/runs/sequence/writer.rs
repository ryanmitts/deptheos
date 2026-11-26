//! Writing a sequence's frames: their names and folder, the frames
//! themselves and the manifest.

use std::borrow::Cow;
use std::io;
use std::path::{Path, PathBuf};

use deptheos_core::io::metadata::ExportMetadata;
use serde::Serialize;

use super::{Direction, FrameOptions, MANIFEST_FILE, SequenceReport, io_error, scaled_size};
use crate::engine::Engine;
use crate::engine::look::Look;
use crate::library::export;
use crate::runs::session::export_pixels;
use crate::{Error, Result};

// --- Names and folders.

/// Make a new folder `name` (cleaned by [`export::clean_stem`]) in
/// `parent`, or `name-2`, `name-3`…: a sequence never writes into an
/// existing folder.
fn create_folder(parent: &Path, name: &str) -> io::Result<PathBuf> {
    std::fs::create_dir_all(parent)?;
    for name in export::numbered(&export::clean_stem(name), "") {
        let candidate = parent.join(name);
        match std::fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    unreachable!("the loop returns")
}

/// Frame `index` (from 1) of `total`'s file name: `<name>_0001.<ext>`, at
/// least four digits.
fn frame_file_name(name: &str, index: usize, total: usize, extension: &str) -> String {
    let digits = total.max(1).to_string().len().max(4);
    format!("{name}_{index:0digits$}.{extension}")
}

// --- Writing frames.

/// A frame finished but not yet written: in output color, cropped, resized.
pub(super) struct Frame<'p> {
    pixels: Cow<'p, [u16]>,
    width: u32,
    height: u32,
    sixteen_bit: bool,
}

/// One sequence being written: numbered frames ([`frame_file_name`]) in a
/// new folder with only [`MANIFEST_FILE`] beside them; closing writes the
/// manifest, recording whether the run got to the end.
pub(super) struct SequenceWriter<'a> {
    folder: PathBuf,
    /// The folder's name, which the frames are named after.
    name: String,
    total: usize,
    options: FrameOptions,
    /// For the color step of a linear stack.
    engine: &'a Engine,
    written: usize,
    manifest: Manifest,
}

impl<'a> SequenceWriter<'a> {
    /// A sequence of `total` frames in a new folder `name` in `parent`,
    /// recorded in `manifest`.
    pub(super) fn create(
        engine: &'a Engine,
        parent: &Path,
        name: &str,
        total: usize,
        options: FrameOptions,
        manifest: Manifest,
    ) -> Result<Self> {
        let folder = create_folder(parent, name).map_err(io_error(parent))?;
        let name = folder
            .file_name()
            .map_or_else(|| "sequence".into(), |n| n.to_string_lossy().into_owned());
        Ok(Self { folder, name, total, options, engine, written: 0, manifest })
    }

    /// `pixels` (interleaved RGB u16, stored as `look`'s stack's are)
    /// finished: drawn in `look`, then resized, the same for every frame so
    /// the sequence ends on the render. Borrowed pixels are copied only
    /// when they change.
    pub(super) fn finish<'p>(
        &self,
        pixels: Cow<'p, [u16]>,
        width: u32,
        height: u32,
        look: Look<'_>,
    ) -> Result<Frame<'p>> {
        let color = &look.stack().color;
        let pixels = match pixels {
            Cow::Borrowed(stored) => export_pixels(self.engine, look, stored, (width, height))?,
            Cow::Owned(stored) => match export_pixels(self.engine, look, &stored, (width, height))?
            {
                Cow::Borrowed(_) => Cow::Owned(stored),
                Cow::Owned(drawn) => Cow::Owned(drawn),
            },
        };
        let (pixels, width, height) = resize(pixels, width, height, self.options.long_edge)?;
        Ok(Frame { pixels, width, height, sixteen_bit: self.options.sixteen_bit(color) })
    }

    /// Write `frame` as the next file, tagged `tags`, listed as `entry`
    /// makes it from the file name.
    pub(super) fn write(
        &mut self,
        frame: &Frame<'_>,
        tags: &ExportMetadata,
        entry: impl FnOnce(String) -> ManifestFrame,
    ) -> Result<()> {
        let file = frame_file_name(
            &self.name,
            self.written + 1,
            self.total.max(self.written + 1),
            self.options.format.extension(),
        );
        deptheos_core::io::save_rgb16(
            &self.folder.join(&file),
            &frame.pixels,
            frame.width,
            frame.height,
            frame.sixteen_bit,
            self.options.save.encoding(),
            tags,
        )?;
        self.written += 1;
        (self.manifest.width, self.manifest.height) = (frame.width, frame.height);
        self.manifest.frames.push(entry(file));
        Ok(())
    }

    /// Record a frame the replay left out.
    pub(super) fn left_out(&mut self, gap: ManifestGap) {
        self.manifest.left_out.push(gap);
    }

    pub(super) fn written(&self) -> usize {
        self.written
    }

    /// Close the sequence after `ran`, writing the manifest either way so a
    /// canceled run keeps what it made. Returns `ran`'s error first, else
    /// the manifest's.
    pub(super) fn close(self, ran: Result<()>) -> Result<SequenceReport> {
        let Self { folder, written, mut manifest, .. } = self;
        manifest.complete = ran.is_ok();
        let closed = crate::fsutil::write_json_atomic(&folder.join(MANIFEST_FILE), &manifest);
        ran?;
        closed.map_err(Error::from)?;
        Ok(SequenceReport { folder, frames: written })
    }
}

/// Scale `pixels` down to `long_edge` with Lanczos-3: the frames are an
/// editor's master, so quality over speed.
pub(super) fn resize(
    pixels: Cow<'_, [u16]>,
    width: u32,
    height: u32,
    long_edge: Option<u32>,
) -> Result<(Cow<'_, [u16]>, u32, u32)> {
    use fast_image_resize as fr;
    let (w, h) = scaled_size(width, height, long_edge);
    if (w, h) == (width, height) {
        return Ok((pixels, width, height));
    }
    let failed = |e: String| Error::InvalidState(format!("resize: {e}"));
    let src = fr::images::ImageRef::new(
        width,
        height,
        bytemuck::cast_slice(&pixels),
        fr::PixelType::U16x3,
    )
    .map_err(|e| failed(e.to_string()))?;
    let mut dst = fr::images::Image::new(w, h, fr::PixelType::U16x3);
    fr::Resizer::new()
        .resize(
            &src,
            &mut dst,
            &fr::ResizeOptions {
                algorithm: fr::ResizeAlg::Convolution(fr::FilterType::Lanczos3),
                cropping: fr::SrcCropping::None,
                mul_div_alpha: false,
            },
        )
        .map_err(|e| failed(e.to_string()))?;
    Ok((Cow::Owned(bytemuck::pod_collect_to_vec(dst.buffer())), w, h))
}

// --- The manifest.

/// What a sequence's frames are: written beside them as `manifest.json`.
#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct Manifest {
    pub version: u32,
    /// "grow", "turntable grow" or "turntable full".
    pub kind: String,
    /// Whether every frame was written (false after Cancel or an error).
    pub complete: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direction: Option<Direction>,
    pub width: u32,
    pub height: u32,
    /// The crop every frame was cut to, in the source frames' coordinates.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crop: Option<[u32; 4]>,
    pub frames: Vec<ManifestFrame>,
    /// Stacks the sequence went on without, and why.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<ManifestGap>,
    /// Source frames that add nothing (left out or unreadable), and why.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub left_out: Vec<ManifestGap>,
    /// Stacks restacked without their first frame, and other fixes.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ManifestFrame {
    pub file: String,
    /// The turntable stack it is from, from 1 among the run's stacks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stack: Option<usize>,
    /// That stack's folder (or label).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// The replay frame it was taken after, from 1.
    pub k: usize,
    /// Of how many.
    pub of: usize,
    /// "partial" or "full".
    pub kind: &'static str,
    /// The render it came from, when there is one in the history.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub render: Option<String>,
    /// The first frame (the farthest, back to front) and the last it adds,
    /// by file name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ManifestGap {
    pub what: String,
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_names_are_padded_without_gaps() {
        assert_eq!(frame_file_name("hornet grow", 1, 66, "tif"), "hornet grow_0001.tif");
        assert_eq!(frame_file_name("x", 12345, 12345, "png"), "x_12345.png");
        assert_eq!(frame_file_name("x", 3, 10000, "png"), "x_00003.png");
    }

    #[test]
    fn a_new_folder_never_reuses_one() {
        let root = tempfile::tempdir().unwrap();
        let a = create_folder(root.path(), "scan1 grow").unwrap();
        let b = create_folder(root.path(), "scan1 grow").unwrap();
        assert_eq!(a.file_name().unwrap(), "scan1 grow");
        assert_eq!(b.file_name().unwrap(), "scan1 grow-2");
    }
}
