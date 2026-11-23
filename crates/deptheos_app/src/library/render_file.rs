//! The on-disk format of one render: its manifest (`render.json`) and its
//! image codecs, over [`crate::fsutil`]. Mirror types keep the format
//! stable while the engine's types change.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use deptheos_aligner::{AlignMode, GridConfig};
use deptheos_core::FrameAlignment;
use deptheos_core::cancel::codec;
use deptheos_core::depth::DepthConfig;
use deptheos_core::pyramid::PyramidConfig;
use deptheos_core::transform::Affine2;
use deptheos_core::wavelet::WaveletConfig;
use deptheos_dng::{DngImage, jxl};
use image::math::Rect;
use image::{ImageEncoder, RgbImage};
use nalgebra::Matrix3;
use serde::{Deserialize, Serialize};

use crate::RawBackend;
use crate::frames::order::FrameOrder;
use crate::frames::sources::FrameRef;
use crate::fsutil::{
    PARTIAL_SUFFIX, Result, StoreError, Versioned, io_error, sync, sync_folder, write_file,
    write_json, write_replacing,
};
use crate::library::ids::StackId;
use crate::runs::batch::Split;
use crate::runs::session::{
    AlignSettings, Alignment, CompletedStack, DcpPreference, Gray16Image, LeftOut, SessionOptions,
    StackColor, StackGeometry, render_params_or_identity,
};
use crate::runs::stacker::StackerConfig;

/// The on-disk format version this build writes and reads.
pub(crate) const RENDER_SCHEMA: u32 = 1;
/// Longest side of a render's thumbnail.
pub const THUMBNAIL_SIDE: u32 = 512;

pub(crate) const RENDER_FILE: &str = "render.json";
pub(crate) const RESULT_FILE: &str = "result.jxl";
pub(crate) const FRAME_MAP_FILE: &str = "frame_map.jxl";
pub(crate) const THUMBNAIL_FILE: &str = "thumb.jpg";
pub(crate) const COLOR_FILE: &str = "color.dng";
pub(crate) const PROFILE_FILE: &str = "profile.icc";
pub(crate) const THUMBNAIL_QUALITY: u8 = 85;
/// The color-metadata carrier's pixels are never read; only its tags are.
pub(crate) const COLOR_CARRIER_SIDE: u32 = 16;

/// Flags parallel to `frames` marking the stack's `paths`, a subsequence.
pub(crate) fn included_frames(frames: &[PathBuf], paths: &[PathBuf]) -> Result<Vec<bool>> {
    let mut remaining = paths.iter().peekable();
    let included: Vec<bool> = frames
        .iter()
        .map(|frame| {
            let hit = remaining.peek().is_some_and(|p| *p == frame);
            if hit {
                remaining.next();
            }
            hit
        })
        .collect();
    match remaining.next() {
        None if !paths.is_empty() => Ok(included),
        None => Err(StoreError::Invalid("the stack has no frames".into())),
        Some(path) => Err(StoreError::Invalid(format!(
            "{} is not one of the listed frames (or is out of order)",
            path.display()
        ))),
    }
}

pub(crate) fn partial_of(dir: &Path) -> PathBuf {
    let mut name = dir.file_name().unwrap_or_default().to_os_string();
    name.push(PARTIAL_SUFFIX);
    dir.with_file_name(name)
}

/// A render's pictures, encoded on the pool into its `.partial` folder for
/// the store thread to finish (ADR-143). Dropped unsaved, the folder goes.
pub struct EncodedPictures {
    pub(crate) id: crate::library::history::RenderId,
    pub(crate) partial: PathBuf,
}

impl Drop for EncodedPictures {
    fn drop(&mut self) {
        // Saved, it was moved into place and nothing is left here.
        if self.partial.exists() {
            let _ = std::fs::remove_dir_all(&self.partial);
        }
    }
}

/// Encode `stack`'s pictures and `thumbnail` into `partial`, the slow part
/// of a save.
pub(crate) fn encode_pictures(
    id: crate::library::history::RenderId,
    partial: PathBuf,
    stack: &CompletedStack,
    thumbnail: &RgbImage,
) -> Result<EncodedPictures> {
    std::fs::create_dir_all(&partial).map_err(io_error(&partial))?;
    let pictures = EncodedPictures { id, partial };
    let dir = &pictures.partial;
    encode_into(&dir.join(RESULT_FILE), |file, budget| {
        jxl::encode_rgb16(
            file,
            &stack.pixels,
            stack.width,
            stack.height,
            jxl::FAST_EFFORT,
            stack.is_linear(),
            budget,
        )
    })?;
    // The frame map at its full 16 bits, for the contribution overlay; its
    // flat runs code small.
    if let Some(frames) = &stack.frame_map {
        let (w, h) = frames.dimensions();
        encode_into(&dir.join(FRAME_MAP_FILE), |file, budget| {
            jxl::encode_gray16(file, frames.as_raw(), w, h, jxl::FAST_EFFORT, budget)
        })?;
    }
    write_file(&dir.join(THUMBNAIL_FILE), &encode_thumbnail(thumbnail)?)?;
    Ok(pictures)
}

/// A picture as lossless JPEG XL at fast effort (about half raw size),
/// streamed to `path` and synced.
fn encode_into(
    path: &Path,
    encode: impl FnOnce(&mut std::fs::File, deptheos_dng::Budget) -> deptheos_dng::Result<()>,
) -> Result<()> {
    let mut file = std::fs::File::create(path).map_err(io_error(path))?;
    codec(|budget| encode(&mut file, budget)).map_err(|e| match e {
        deptheos_dng::DngError::JxlWrite(source) => StoreError::Io { path: path.into(), source },
        e => e.into(),
    })?;
    deptheos_core::io::sync_file(&file).map_err(io_error(path))
}

/// A thumbnail at most [`THUMBNAIL_SIDE`] px, as JPEG.
fn encode_thumbnail(image: &RgbImage) -> Result<Vec<u8>> {
    let small;
    let image = if image.width().max(image.height()) > THUMBNAIL_SIDE {
        let scale = THUMBNAIL_SIDE as f32 / image.width().max(image.height()) as f32;
        let w = ((image.width() as f32 * scale).round() as u32).max(1);
        let h = ((image.height() as f32 * scale).round() as u32).max(1);
        small = image::imageops::thumbnail(image, w, h);
        &small
    } else {
        image
    };
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, THUMBNAIL_QUALITY).write_image(
        image.as_raw(),
        image.width(),
        image.height(),
        image::ExtendedColorType::Rgb8,
    )?;
    Ok(bytes)
}

/// Write a tiny DNG carrying only the reference frame's color metadata,
/// which the DNG SDK holds opaquely; [`DngImage::read_metadata`] reads it
/// back without decoding. Atomic, and without GPS or serials (ADR-133).
pub(crate) fn write_color_carrier(path: &Path, metadata: &DngImage) -> Result<()> {
    let side = COLOR_CARRIER_SIDE;
    let pixels = vec![0x4000u16; (side * side * 3) as usize];
    write_replacing(path, |tmp| Ok(metadata.write(tmp, &pixels, side, side, 3)?))
}

pub(crate) fn to_unix_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

pub(crate) fn from_unix_ms(ms: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(ms)
}

pub(crate) fn read_frame_map(path: &Path, width: u32, height: u32) -> Result<Gray16Image> {
    let bytes = std::fs::read(path).map_err(io_error(path))?;
    let (pixels, w, h) = codec(|budget| jxl::decode_gray16(&bytes, budget))?;
    check_size(path, "frame map", (w, h), (width, height))?;
    Gray16Image::from_raw(w, h, pixels)
        .ok_or_else(|| StoreError::Invalid(format!("{}: short frame map", path.display())))
}

/// A render's color profile ([`PROFILE_FILE`]).
fn read_profile(path: &Path) -> Result<deptheos_core::io::IccProfile> {
    let bytes = std::fs::read(path).map_err(io_error(path))?;
    deptheos_core::io::IccProfile::from_bytes(bytes)
        .ok_or_else(|| StoreError::Invalid(format!("{} is not an RGB ICC profile", path.display())))
}

pub(crate) fn read_result(path: &Path, width: u32, height: u32) -> Result<Vec<u16>> {
    let bytes = std::fs::read(path).map_err(io_error(path))?;
    let (pixels, w, h) = codec(|budget| jxl::decode_rgb16(&bytes, budget))?;
    check_size(path, "result", (w, h), (width, height))?;
    Ok(pixels)
}

/// An image read back at the size its manifest gives.
pub(crate) fn check_size(
    path: &Path,
    what: &str,
    found: (u32, u32),
    expected: (u32, u32),
) -> Result<()> {
    if found == expected {
        return Ok(());
    }
    Err(StoreError::Invalid(format!(
        "{}: {what} is {}x{}, the manifest says {}x{}",
        path.display(),
        found.0,
        found.1,
        expected.0,
        expected.1
    )))
}

/// Write the rest of a render's files into its `.partial` folder `dir`,
/// the manifest last so a directory with one is complete. `included` is
/// parallel to `frames`. Returns the manifest written.
pub(crate) fn write_render(
    dir: &Path,
    frames: &[FrameRef],
    included: &[bool],
    stack: &CompletedStack,
    created: SystemTime,
    place: Place,
) -> Result<RenderFile> {
    let partial = dir;
    let color = match &stack.color {
        StackColor::Srgb { sixteen_bit, profile } => {
            if let Some(profile) = profile {
                let path = partial.join(PROFILE_FILE);
                std::fs::write(&path, profile.bytes()).map_err(io_error(&path))?;
                sync(&path)?;
            }
            ColorFile::Srgb { sixteen_bit: *sixteen_bit, profile: profile.is_some() }
        }
        StackColor::Linear { metadata, .. } => {
            write_color_carrier(&partial.join(COLOR_FILE), metadata)?;
            ColorFile::Linear
        }
    };
    let file = RenderFile {
        schema: RENDER_SCHEMA,
        created_unix_ms: to_unix_ms(created),
        name: None,
        frames: frames.to_vec(),
        included: included.to_vec(),
        batch: place.batch,
        stack: place.stack,
        number: place.number,
        picked: false,
        stacker: StackerFile::from(&stack.config),
        options: OptionsFile::from(&stack.options),
        alignment: stack.geometry.transforms.iter().map(|t| t.map(Into::into)).collect(),
        crop: stack.geometry.crop.map(Into::into),
        reversed: stack.geometry.reversed,
        left_out: stack
            .geometry
            .left_out
            .iter()
            .map(|l| LeftOutFile { index: l.index, reason: l.reason.clone() })
            .collect(),
        result: ResultFile { width: stack.width, height: stack.height },
        color,
        frame_map: stack.frame_map.is_some(),
    };
    // A manifest that would be refused when read is not kept.
    file.check()?;
    write_json(&partial.join(RENDER_FILE), &file)?;
    // Each file was synced as it was written; now the folder's entries.
    sync_folder(partial)?;
    Ok(file)
}

/// Rebuild the stack a render in `dir` was saved from, given its checked
/// manifest and the paths of the frames it included.
pub(crate) fn load_stack(
    dir: &Path,
    file: RenderFile,
    paths: Vec<PathBuf>,
) -> Result<CompletedStack> {
    let (width, height) = (file.result.width, file.result.height);
    // Decoded in parallel, since a lone decode leaves cores idle between
    // passes; each within this thread's work, so its token stops it.
    use deptheos_core::cancel::{codec_threads, current, within};
    let (cancel, threads) = (current(), codec_threads());
    let map_threads = (threads / 2).max(1);
    let result_threads = threads.saturating_sub(map_threads).max(1);
    let (pixels, frame_map, color) = std::thread::scope(|scope| {
        // Codec threads, with the stack a decoder needs (`work::thread`).
        let map = crate::engine::work::thread("read-frame-map").spawn_scoped(scope, || {
            within(&cancel, map_threads, || match file.frame_map {
                true => read_frame_map(&dir.join(FRAME_MAP_FILE), width, height).map(Some),
                false => Ok(None),
            })
        });
        let color = crate::engine::work::thread("read-color")
            .spawn_scoped(scope, || within(&cancel, 1, || read_color(dir, &file.color)));
        // A thread that couldn't start fails the read, as its file would.
        let (map, color) = (map.map_err(io_error(dir))?, color.map_err(io_error(dir))?);
        let pixels =
            within(&cancel, result_threads, || read_result(&dir.join(RESULT_FILE), width, height));
        let map = map.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic));
        let color = color.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic));
        Ok::<_, StoreError>((pixels, map, color))
    })?;
    let (pixels, frame_map, color) = (pixels?, frame_map?, color?);

    let stack = CompletedStack {
        width,
        height,
        pixels,
        frame_map,
        geometry: file.geometry(),
        color,
        config: file.stacker.config(),
        options: file.options.options(),
        notes: Vec::new(),
        paths,
        contributions: Default::default(),
    };
    // Counted with the read, off the thread that shows the render.
    stack.frame_contributions();
    Ok(stack)
}

/// [`load_stack`] without the result or the frame map: reads only the
/// color reference (`color.dng` or `profile.icc`).
pub(crate) fn load_settings(
    dir: &Path,
    file: RenderFile,
    paths: Vec<PathBuf>,
) -> Result<CompletedStack> {
    Ok(CompletedStack {
        width: file.result.width,
        height: file.result.height,
        pixels: Vec::new(),
        frame_map: None,
        geometry: file.geometry(),
        color: read_color(dir, &file.color)?,
        config: file.stacker.config(),
        options: file.options.options(),
        notes: Vec::new(),
        paths,
        contributions: Default::default(),
    })
}

/// A render's color model, from its color reference in `dir`.
fn read_color(dir: &Path, color: &ColorFile) -> Result<StackColor> {
    match color {
        ColorFile::Srgb { sixteen_bit, profile } => {
            let profile = match profile {
                true => Some(read_profile(&dir.join(PROFILE_FILE))?),
                false => None,
            };
            Ok(StackColor::Srgb { sixteen_bit: *sixteen_bit, profile })
        }
        ColorFile::Linear => {
            let metadata = DngImage::read_metadata(&dir.join(COLOR_FILE))?;
            let render_params = render_params_or_identity(&metadata);
            Ok(StackColor::Linear { metadata, render_params })
        }
    }
}

// --- Manifests.

/// Where a new render belongs: its stack, number and batch run.
pub(crate) struct Place {
    pub(crate) stack: StackId,
    pub(crate) number: u32,
    pub(crate) batch: Option<BatchFile>,
}

/// The batch run a render came from, for the history's headers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BatchFile {
    pub(crate) id: String,
    pub(crate) split: Split,
    pub(crate) skipped: u64,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct LeftOutFile {
    pub(crate) index: usize,
    pub(crate) reason: String,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct RenderFile {
    pub(crate) schema: u32,
    pub(crate) created_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<String>,
    /// Every frame listed when the stack ran, in order, by root (ADR-145).
    pub(crate) frames: Vec<FrameRef>,
    /// Parallel to `frames`.
    pub(crate) included: Vec<bool>,
    /// The batch run that made it. Optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) batch: Option<BatchFile>,
    /// The stack it is part of.
    pub(crate) stack: StackId,
    /// Its place in its stack, from 1.
    pub(crate) number: u32,
    /// Starred. Optional.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) picked: bool,
    pub(crate) stacker: StackerFile,
    pub(crate) options: OptionsFile,
    /// One per included frame; `null` when alignment was off.
    pub(crate) alignment: Vec<Option<AlignmentFile>>,
    pub(crate) crop: Option<RectFile>,
    pub(crate) reversed: bool,
    /// Included frames left out, by index among the included, and why.
    /// Optional.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) left_out: Vec<LeftOutFile>,
    pub(crate) result: ResultFile,
    pub(crate) color: ColorFile,
    /// Whether [`FRAME_MAP_FILE`] holds the frame map.
    pub(crate) frame_map: bool,
}

impl Versioned for RenderFile {
    const SCHEMA: u32 = RENDER_SCHEMA;
}

impl RenderFile {
    /// The paths of the included frames of `listed`, in order.
    pub(crate) fn included_paths(&self, listed: &[PathBuf]) -> Vec<PathBuf> {
        listed.iter().zip(&self.included).filter(|&(_, &i)| i).map(|(f, _)| f.clone()).collect()
    }

    /// Check the manifest is self-consistent; the error names what isn't.
    pub(crate) fn check(&self) -> Result<()> {
        let invalid = |what: String| Err(StoreError::Invalid(what));
        let frame_count = self.frames.len();
        let stacked = self.included.iter().filter(|&&i| i).count();
        if self.included.len() != frame_count || stacked == 0 {
            return invalid(format!(
                "render includes {stacked} of {} flags, for {frame_count} frames",
                self.included.len()
            ));
        }
        if self.alignment.len() != stacked {
            return invalid(format!(
                "render has {} alignments for {stacked} stacked frames",
                self.alignment.len()
            ));
        }
        // Each left out once, and at least one frame stacked.
        let left_out: std::collections::BTreeSet<usize> =
            self.left_out.iter().map(|l| l.index).collect();
        if left_out.len() != self.left_out.len()
            || left_out.len() >= stacked
            || left_out.last().is_some_and(|&last| last >= stacked)
        {
            return invalid(format!(
                "render leaves out {} of {stacked} stacked frames ({} distinct, last {:?})",
                self.left_out.len(),
                left_out.len(),
                left_out.last()
            ));
        }
        let ResultFile { width, height } = self.result;
        if width == 0 || height == 0 {
            return invalid(format!("render result is {width} by {height} pixels"));
        }
        // The crop (full-frame coordinates) is the result's size.
        if let Some(c) = self.crop
            && (c.width != width
                || c.height != height
                || c.x.checked_add(c.width).is_none()
                || c.y.checked_add(c.height).is_none())
        {
            return invalid(format!(
                "render crop is {} by {} at ({}, {}), for a {width} by {height} result",
                c.width, c.height, c.x, c.y
            ));
        }
        Ok(())
    }

    pub(crate) fn geometry(&self) -> StackGeometry {
        StackGeometry {
            transforms: self.alignment.iter().map(|a| a.map(Into::into)).collect(),
            crop: self.crop.map(Into::into),
            reversed: self.reversed,
            left_out: self
                .left_out
                .iter()
                .map(|l| LeftOut { index: l.index, reason: l.reason.clone() })
                .collect(),
        }
    }
}

/// The algorithm and its GUI parameters; sweep-only overrides aren't kept.
#[derive(PartialEq, Serialize, Deserialize)]
#[serde(tag = "algorithm", rename_all = "snake_case")]
pub(crate) enum StackerFile {
    Pyramid { smoothness: f32 },
    Depth { radius: f32, smoothing: f32, blend_range: f32 },
    Wavelet { denoise: f32, consistency: u32 },
}

impl From<&StackerConfig> for StackerFile {
    fn from(config: &StackerConfig) -> Self {
        match config {
            StackerConfig::Pyramid(c) => Self::Pyramid { smoothness: c.smoothness },
            StackerConfig::Depth(c) => {
                Self::Depth { radius: c.radius, smoothing: c.smoothing, blend_range: c.blend_range }
            }
            StackerConfig::Wavelet(c) => {
                Self::Wavelet { denoise: c.denoise, consistency: c.consistency }
            }
        }
    }
}

impl StackerFile {
    pub(crate) fn config(&self) -> StackerConfig {
        match *self {
            Self::Pyramid { smoothness } => StackerConfig::Pyramid(PyramidConfig { smoothness }),
            Self::Depth { radius, smoothing, blend_range } => {
                StackerConfig::Depth(DepthConfig { radius, smoothing, blend_range })
            }
            Self::Wavelet { denoise, consistency } => {
                StackerConfig::Wavelet(WaveletConfig { denoise, consistency })
            }
        }
    }
}

/// The session options that shape the result. Grid tuning beyond its limits
/// restores as defaults; the prefetch budget only changes speed.
#[derive(PartialEq, Serialize, Deserialize)]
pub(crate) struct OptionsFile {
    pub(crate) align: bool,
    pub(crate) align_mode: AlignModeFile,
    pub(crate) crop: bool,
    /// Always written `true`, ignored on read: every aligned stack checks.
    pub(crate) reverse_check: bool,
    pub(crate) raw_backend: RawBackend,
    pub(crate) grid: GridFile,
    pub(crate) camera_profile: CameraProfileFile,
    /// The order the frames were given in.
    pub(crate) order: FrameOrder,
}

/// The grid aligner's user-settable limits ([`GridConfig`]).
#[derive(PartialEq, Serialize, Deserialize)]
pub(crate) struct GridFile {
    pub(crate) horizontal_pct: f32,
    pub(crate) vertical_pct: f32,
    pub(crate) rotation_deg: f32,
    pub(crate) magnification_pct: f32,
    pub(crate) apply_brightness: bool,
}

impl From<&GridConfig> for GridFile {
    fn from(g: &GridConfig) -> Self {
        Self {
            horizontal_pct: g.horizontal_pct,
            vertical_pct: g.vertical_pct,
            rotation_deg: g.rotation_deg,
            magnification_pct: g.magnification_pct,
            apply_brightness: g.apply_brightness,
        }
    }
}

impl GridFile {
    pub(crate) fn config(&self) -> GridConfig {
        GridConfig {
            horizontal_pct: self.horizontal_pct,
            vertical_pct: self.vertical_pct,
            rotation_deg: self.rotation_deg,
            magnification_pct: self.magnification_pct,
            apply_brightness: self.apply_brightness,
            ..GridConfig::default()
        }
    }
}

#[derive(PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CameraProfileFile {
    AdobeStandard,
    CameraStandard,
    None,
}

#[derive(PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AlignModeFile {
    Ecc,
    Grid,
}

impl From<&SessionOptions> for OptionsFile {
    /// Alignment off writes the settings it resumes with.
    fn from(options: &SessionOptions) -> Self {
        let settings = options.alignment.settings();
        Self {
            align: options.alignment.is_on(),
            align_mode: match settings.mode {
                AlignMode::Ecc => AlignModeFile::Ecc,
                AlignMode::Grid => AlignModeFile::Grid,
            },
            crop: settings.crop,
            reverse_check: true,
            raw_backend: options.raw_backend,
            grid: GridFile::from(&settings.grid),
            camera_profile: match options.camera_profile {
                DcpPreference::AdobeStandard => CameraProfileFile::AdobeStandard,
                DcpPreference::CameraStandard => CameraProfileFile::CameraStandard,
                DcpPreference::None => CameraProfileFile::None,
            },
            order: options.order,
        }
    }
}

impl OptionsFile {
    /// The options these fields record; with `align` off, the aligner and
    /// crop are what it resumes with.
    pub(crate) fn options(&self) -> SessionOptions {
        let settings = AlignSettings {
            mode: match self.align_mode {
                AlignModeFile::Ecc => AlignMode::Ecc,
                AlignModeFile::Grid => AlignMode::Grid,
            },
            grid: self.grid.config(),
            crop: self.crop,
        };
        SessionOptions {
            alignment: Alignment::new(self.align, settings),
            raw_backend: self.raw_backend,
            camera_profile: match self.camera_profile {
                CameraProfileFile::AdobeStandard => DcpPreference::AdobeStandard,
                CameraProfileFile::CameraStandard => DcpPreference::CameraStandard,
                CameraProfileFile::None => DcpPreference::None,
            },
            order: self.order,
            ..Default::default()
        }
    }
}

/// A frame's alignment: the backward matrix `[a b tx; c d ty]` (reference
/// → frame coordinates) and its brightness factor.
#[derive(Clone, Copy, Serialize, Deserialize)]
pub(crate) struct AlignmentFile {
    pub(crate) a: f32,
    pub(crate) b: f32,
    pub(crate) c: f32,
    pub(crate) d: f32,
    pub(crate) tx: f32,
    pub(crate) ty: f32,
    pub(crate) brightness: f32,
}

impl From<FrameAlignment> for AlignmentFile {
    fn from(alignment: FrameAlignment) -> Self {
        let m = alignment.transform.matrix();
        Self {
            a: m[(0, 0)],
            b: m[(0, 1)],
            c: m[(1, 0)],
            d: m[(1, 1)],
            tx: m[(0, 2)],
            ty: m[(1, 2)],
            brightness: alignment.brightness,
        }
    }
}

impl From<AlignmentFile> for FrameAlignment {
    fn from(f: AlignmentFile) -> Self {
        #[rustfmt::skip]
        let matrix = Matrix3::new(
            f.a, f.b, f.tx,
            f.c, f.d, f.ty,
            0.0, 0.0, 1.0,
        );
        FrameAlignment::new(Affine2::from_matrix_unchecked(matrix), f.brightness)
    }
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub(crate) struct RectFile {
    pub(crate) x: u32,
    pub(crate) y: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

impl From<Rect> for RectFile {
    fn from(r: Rect) -> Self {
        Self { x: r.x, y: r.y, width: r.width, height: r.height }
    }
}

impl From<RectFile> for Rect {
    fn from(r: RectFile) -> Self {
        Rect { x: r.x, y: r.y, width: r.width, height: r.height }
    }
}

/// The result's size; its samples are in [`RESULT_FILE`].
#[derive(Serialize, Deserialize)]
pub(crate) struct ResultFile {
    pub(crate) width: u32,
    pub(crate) height: u32,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum ColorFile {
    /// A standard image's samples; `sixteen_bit` when a source frame was.
    /// sRGB, or with `profile` the ICC profile's space ([`PROFILE_FILE`]).
    Srgb { sixteen_bit: bool, profile: bool },
    /// Linear camera RGB, its color metadata in [`COLOR_FILE`].
    Linear,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn included_frames_follow_the_list_order() {
        let all = paths(&["a", "b", "c", "d"]);
        assert_eq!(
            included_frames(&all, &paths(&["a", "c", "d"])).unwrap(),
            [true, false, true, true]
        );
        assert_eq!(included_frames(&all, &paths(&["a", "b", "c", "d"])).unwrap(), [true; 4]);
        // Out of order, unknown, or empty: not a render of these frames.
        assert!(included_frames(&all, &paths(&["c", "a"])).is_err());
        assert!(included_frames(&all, &paths(&["a", "x"])).is_err());
        assert!(included_frames(&all, &[]).is_err());
    }

    /// A frame as a manifest names it.
    fn frame(path: &str) -> serde_json::Value {
        serde_json::json!({
            "root": "0190a1b2c3d47e8f9a0b1c2d3e4f5a6b", "path": path,
            "size": 1, "fingerprint": "0123456789abcdef",
        })
    }

    /// A manifest leaving a frame out twice, or every frame, is refused.
    #[test]
    fn left_out_frames_are_checked() {
        let with_left_out = |indices: &[usize]| {
            let left_out: Vec<_> =
                indices.iter().map(|i| serde_json::json!({"index": i, "reason": "x"})).collect();
            let manifest = serde_json::json!({
                "schema": RENDER_SCHEMA, "created_unix_ms": 5,
                "stack": "0190a1b2c3d47e8f9a0b1c2d3e4f5a6c", "number": 1,
                "frames": [frame("a"), frame("b"), frame("c")],
                "included": [true, true, true],
                "stacker": {"algorithm": "pyramid", "smoothness": 0.5},
                "options": OptionsFile::from(&SessionOptions::default()),
                "alignment": [null, null, null], "crop": null, "reversed": false,
                "result": {"width": 4, "height": 3},
                "color": {"kind": "srgb", "sixteen_bit": false, "profile": false},
                "frame_map": false, "left_out": left_out,
            });
            serde_json::from_value::<RenderFile>(manifest).unwrap()
        };
        assert!(with_left_out(&[]).check().is_ok());
        assert!(with_left_out(&[0, 2]).check().is_ok());
        assert!(with_left_out(&[1, 1]).check().is_err(), "twice");
        assert!(with_left_out(&[0, 1, 2]).check().is_err(), "every frame");
        assert!(with_left_out(&[3]).check().is_err(), "past the end");
    }

    /// The crop is the result's size at any offset.
    #[test]
    fn crop_is_the_size_of_the_result() {
        let with_crop = |crop: serde_json::Value| {
            let manifest = serde_json::json!({
                "schema": RENDER_SCHEMA, "created_unix_ms": 5,
                "stack": "0190a1b2c3d47e8f9a0b1c2d3e4f5a6c", "number": 1,
                "frames": [frame("a")],
                "included": [true],
                "stacker": {"algorithm": "pyramid", "smoothness": 0.5},
                "options": OptionsFile::from(&SessionOptions::default()),
                "alignment": [null], "crop": crop, "reversed": false,
                "result": {"width": 7853, "height": 4619},
                "color": {"kind": "srgb", "sixteen_bit": false, "profile": false},
                "frame_map": false,
            });
            serde_json::from_value::<RenderFile>(manifest).unwrap()
        };
        let crop = |x: u32, y: u32, width: u32, height: u32| {
            with_crop(serde_json::json!({"x": x, "y": y, "width": width, "height": height}))
        };
        assert!(with_crop(serde_json::Value::Null).check().is_ok());
        assert!(crop(0, 0, 7853, 4619).check().is_ok());
        assert!(crop(339, 845, 7853, 4619).check().is_ok(), "offset in the full frame");
        assert!(crop(339, 845, 7852, 4619).check().is_err(), "not the result's width");
        assert!(crop(339, 845, 7853, 0).check().is_err(), "no height");
        assert!(crop(u32::MAX, 0, 7853, 4619).check().is_err(), "past the edge of u32");
    }

    #[test]
    fn alignment_survives_json_exactly() {
        #[rustfmt::skip]
        let matrix = Matrix3::new(
            1.000_123_4, -0.002_345_6, 17.25,
            0.002_345_7, 0.999_876_5, -3.125_007,
            0.0, 0.0, 1.0,
        );
        let alignment = FrameAlignment::new(Affine2::from_matrix_unchecked(matrix), 0.987_654_3);
        let json = serde_json::to_string(&AlignmentFile::from(alignment)).unwrap();
        let back: AlignmentFile = serde_json::from_str(&json).unwrap();
        assert_eq!(FrameAlignment::from(back), alignment);
    }
}

#[cfg(test)]
mod options_tests {
    use super::*;

    /// The grid limits and camera profile come back as saved.
    #[test]
    fn grid_limits_and_camera_profile_round_trip() {
        let grid = GridConfig {
            horizontal_pct: 8.0,
            vertical_pct: 2.5,
            rotation_deg: 0.0,
            magnification_pct: 6.0,
            apply_brightness: true,
            ..GridConfig::default()
        };
        let options = SessionOptions {
            alignment: Alignment::On(AlignSettings {
                mode: AlignMode::Grid,
                grid,
                ..AlignSettings::default()
            }),
            camera_profile: DcpPreference::CameraStandard,
            ..SessionOptions::default()
        };
        let json = serde_json::to_string(&OptionsFile::from(&options)).unwrap();
        let back = serde_json::from_str::<OptionsFile>(&json).unwrap().options();
        let Alignment::On(AlignSettings { mode: AlignMode::Grid, grid: g, .. }) = back.alignment
        else {
            panic!("not Grid search: {:?}", back.alignment);
        };
        let h = grid;
        assert_eq!(
            (g.horizontal_pct, g.vertical_pct, g.rotation_deg, g.magnification_pct),
            (h.horizontal_pct, h.vertical_pct, h.rotation_deg, h.magnification_pct)
        );
        assert!(g.apply_brightness);
        assert_eq!(g.sample_points, GridConfig::default().sample_points);
        assert_eq!(back.camera_profile, DcpPreference::CameraStandard);
    }

    /// The enums are spelled as the preferences and command line spell them.
    #[test]
    fn leaf_enums_are_written_as_their_keys() {
        let written = |value: serde_json::Value| value.as_str().unwrap().to_owned();
        for backend in RawBackend::ALL {
            assert_eq!(written(serde_json::to_value(backend).unwrap()), backend.key());
        }
        for order in FrameOrder::ALL {
            assert_eq!(written(serde_json::to_value(order).unwrap()), order.as_str());
        }
    }

    /// The alignment fields round-trip, on and off; a reverse check written
    /// off is read and written on.
    #[test]
    fn alignment_reads_and_writes_the_same_fields() {
        let fields = |align: bool, mode: &str, crop: bool, reverse_check: bool| {
            serde_json::json!({
                "align": align, "align_mode": mode, "crop": crop,
                "reverse_check": reverse_check, "raw_backend": "libraw",
                "grid": {
                    "horizontal_pct": 7.5, "vertical_pct": 5.0, "rotation_deg": 2.0,
                    "magnification_pct": 3.0, "apply_brightness": true
                },
                "camera_profile": "adobe_standard", "order": "name"
            })
        };
        let read = |json: &serde_json::Value| {
            serde_json::from_value::<OptionsFile>(json.clone()).unwrap().options().alignment
        };
        let written = |alignment: Alignment| {
            let options = SessionOptions { alignment, ..SessionOptions::default() };
            serde_json::to_value(OptionsFile::from(&options)).unwrap()
        };

        let grid = fields(true, "grid", false, true);
        let Alignment::On(AlignSettings { mode: AlignMode::Grid, grid: g, crop: false, .. }) =
            read(&grid)
        else {
            panic!("{:?}", read(&grid));
        };
        assert_eq!((g.horizontal_pct, g.rotation_deg, g.apply_brightness), (7.5, 2.0, true));
        assert_eq!(written(read(&grid)), grid);

        // ECC keeps the limits that were in effect.
        let ecc = fields(true, "ecc", true, false);
        assert!(matches!(
            read(&ecc),
            Alignment::On(AlignSettings { mode: AlignMode::Ecc, crop: true, .. })
        ));
        assert_eq!(written(read(&ecc)), fields(true, "ecc", true, true));

        // Off, with Grid search uncropped remembered: off, resuming with it.
        let off = fields(false, "grid", false, true);
        let Alignment::Off {
            resumes: AlignSettings { mode: AlignMode::Grid, grid: g, crop: false },
        } = read(&off)
        else {
            panic!("{:?}", read(&off));
        };
        assert_eq!(g.rotation_deg, 2.0);
        assert!(!read(&off).crops() && read(&off).mode().is_none());
        assert_eq!(written(read(&off)), off);
    }
}
