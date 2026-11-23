//! CPU image alignment.
//!
//! Two aligners share one interface ([`Aligner`]):
//!
//! - **ECC** (`ecc/`): Enhanced Correlation Coefficient on a full affine,
//!   each frame against its predecessor. Sub-pixel precise; ignores a
//!   uniform gain and offset but not vignetting, and needs texture.
//! - **Grid** (`grid/`): coarse-to-fine grid search for a similarity
//!   transform, each frame against its predecessor, with a brightness
//!   factor. A frame that does not match its predecessor is an error.
//!
//! [`Aligner`] owns what both share (the composed transforms, brightness
//! and covered area), so each aligner computes one frame's step, and a
//! frame that fails changes nothing.
//!
//! Both run on the CPU, overlapping GPU stacking of the previous frame
//! (ADR-112). Rayon work runs on whatever pool the caller installs.

mod ecc;
mod error;
mod grid;
mod image_ops;
mod plane;
mod rect;
mod simd;
#[cfg(test)]
mod test_util;

pub use ecc::EccConfig;
pub use error::{AlignmentError, Motion};
pub use grid::GridConfig;

pub use nalgebra::Affine2;

/// Rec. 709 luma weights for red, green and blue, shared by every crate.
/// The shaders spell them out too (`wavelet.wgsl`, checked against these;
/// `contribution.wgsl` and the iOS focus-peaking shader, unchecked).
pub const REC709_LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

/// Below this standard deviation of its 0–1 gray, a frame has nothing to
/// match against, and both aligners refuse to measure against it.
pub(crate) const FEATURELESS_STD: f32 = 1.0 / 512.0;

/// One frame's alignment: the backward sampling matrix (reference → frame,
/// full-resolution pixels) and the gain that brings it to frame 0's
/// brightness.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AlignedFrame {
    pub transform: Affine2<f32>,
    pub brightness: f32,
}

impl AlignedFrame {
    /// No motion and no exposure change: what frame 0 aligns to.
    pub fn identity() -> Self {
        Self { transform: Affine2::identity(), brightness: 1.0 }
    }
}

use ecc::EccAligner;
use grid::GridAligner;
use image::DynamicImage;
use image::math::Rect;
use plane::Plane;
use rect::RectExt;

/// Which aligner a session runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AlignMode {
    /// Chain ECC, full affine (the default).
    #[default]
    Ecc,
    /// Grid search, similarity + brightness.
    Grid,
}

impl AlignMode {
    /// Every aligner, the default first.
    pub const ALL: [AlignMode; 2] = [AlignMode::Ecc, AlignMode::Grid];

    /// What the command line and the preferences call it
    /// ([`FromStr`](std::str::FromStr) accepts it in any case).
    pub fn key(self) -> &'static str {
        match self {
            AlignMode::Ecc => "ecc",
            AlignMode::Grid => "grid",
        }
    }
}

impl std::str::FromStr for AlignMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        AlignMode::ALL.into_iter().find(|m| m.key().eq_ignore_ascii_case(s)).ok_or_else(|| {
            let names: Vec<&str> = AlignMode::ALL.iter().map(|m| m.key()).collect();
            format!("unknown alignment mode '{s}'. Use: {}", names.join(" or "))
        })
    }
}

/// One aligner's answer for a frame, relative to the previous frame, in
/// full-resolution pixels. [`Aligner`] composes it onto the sequence.
pub(crate) struct Step {
    pub transform: Affine2<f32>,
    pub brightness: f32,
}

#[derive(Clone, Copy)]
enum Frame<'a> {
    Image(&'a DynamicImage),
    #[cfg_attr(not(test), allow(dead_code))]
    Gray(&'a Plane),
}

// Each aligner has its own gray: ECC converts the whole frame to Rec. 709
// luma; the grid reads the equal-weight mean while resizing to its map.
impl<'a> Frame<'a> {
    fn size(self) -> (u32, u32) {
        match self {
            Frame::Image(image) => (image.width(), image.height()),
            Frame::Gray(gray) => (gray.width as u32, gray.height as u32),
        }
    }

    /// ECC's gray, resized to `width`×`height`.
    fn ecc_gray(self, width: usize, height: usize) -> Result<Plane, AlignmentError> {
        match self {
            Frame::Image(image) => image_ops::image_to_grayscale_resized(image, width, height),
            Frame::Gray(gray) => image_ops::resize_plane(gray, width, height),
        }
    }

    fn grid_rows(self) -> image_ops::GrayRows<'a> {
        match self {
            Frame::Image(image) => image_ops::GrayRows::Image(image),
            Frame::Gray(gray) => image_ops::GrayRows::Plane(gray),
        }
    }
}

enum Method {
    Ecc(EccAligner),
    Grid(GridAligner),
}

/// The per-frame half of alignment (gray conversion and resampling, most of
/// an ECC step's time), which depends on the frame alone. Made with
/// [`Aligner::preparer`]; `Copy` and `Send`, so frames are prepared on the
/// threads that decode them.
#[derive(Clone, Copy, Debug)]
pub struct FramePreparer(Preparer);

#[derive(Clone, Copy, Debug)]
enum Preparer {
    Ecc(ecc::EccConfig),
    Grid,
}

impl FramePreparer {
    /// Prepare one frame; an empty frame is an error.
    pub fn prepare(&self, image: &DynamicImage) -> Result<PreparedFrame, AlignmentError> {
        self.prepare_frame(Frame::Image(image))
    }

    /// About the bytes a `width`×`height` frame holds once prepared, for
    /// planning memory. ECC holds 20 bytes a pixel per level (about 74 MB
    /// at the default 2048 px finest level); the grid a float pyramid.
    pub fn bytes(&self, width: u32, height: u32) -> u64 {
        let (w, h) = (u64::from(width), u64::from(height));
        match &self.0 {
            Preparer::Ecc(config) => {
                let src_max = width.max(height).max(1);
                config
                    .level_resolutions(src_max)
                    .into_iter()
                    .map(|resolution| {
                        let scale = (f64::from(resolution) / f64::from(src_max)).min(1.0);
                        let side = |n: u64| ((n as f64 * scale).round() as u64).max(1);
                        side(w) * side(h) * 20
                    })
                    .sum()
            }
            Preparer::Grid => {
                let scale = f64::from(grid::GridAligner::map_scale_for(width as usize));
                let map = ((w as f64 * scale) * (h as f64 * scale)) as u64 * 4;
                // A pyramid's halvings add a third.
                map + map / 3
            }
        }
    }

    fn prepare_frame(&self, frame: Frame<'_>) -> Result<PreparedFrame, AlignmentError> {
        let (width, height) = frame.size();
        if width == 0 || height == 0 {
            return Err(AlignmentError::ProcessingFailed(format!(
                "empty frame ({width}x{height})"
            )));
        }
        let data = match &self.0 {
            Preparer::Ecc(config) => Prepared::Ecc(EccAligner::prepare(
                config,
                width as usize,
                height as usize,
                |w, h| frame.ecc_gray(w, h),
            )?),
            Preparer::Grid => Prepared::Grid(GridAligner::prepare(frame.grid_rows())?),
        };
        Ok(PreparedFrame { width, height, data_area: None, data })
    }
}

/// A frame ready for [`Aligner::align_prepared`].
pub struct PreparedFrame {
    width: u32,
    height: u32,
    /// Where the frame has data ([`Self::with_data_area`]).
    data_area: Option<Rect>,
    data: Prepared,
}

impl PreparedFrame {
    /// The frame, with data only in `area` (a pre-aligned frame's border is
    /// empty); the crop keeps within it.
    pub fn with_data_area(self, area: Option<Rect>) -> Self {
        Self { data_area: area, ..self }
    }
}

enum Prepared {
    Ecc(ecc::EccLevels),
    Grid(grid::Pyramid),
}

/// Aligns a sequence of frames to its first, one of the aligners computing
/// each frame's step.
pub struct Aligner {
    method: Method,
    sequence: Sequence,
}

/// How far the sequence has come.
///
/// Lifecycle: `Empty` → `Running` when a frame 0 is accepted (a refused
/// one leaves it `Empty`); while `Running`, each frame that aligns becomes
/// the reference and moves `last` and `valid_area` on, and one that fails
/// changes nothing. Changed only by [`Aligner::align_prepared`].
enum Sequence {
    Empty,
    Running {
        /// Frame 0's size, which every frame must share.
        size: (u32, u32),
        /// Reference → last aligned frame.
        last: AlignedFrame,
        /// Region covered by every frame so far, in reference pixels.
        valid_area: Rect,
        /// The last frame that aligned.
        reference: Prepared,
    },
}

impl Aligner {
    /// An aligner with no frames yet, running `mode` with its defaults. The
    /// first frame it accepts becomes frame 0, the reference.
    pub fn new(mode: AlignMode) -> Self {
        Self::with_grid_config(mode, GridConfig::default())
    }

    /// `new`, with the grid aligner's search limits (ignored for ECC).
    pub fn with_grid_config(mode: AlignMode, grid: GridConfig) -> Self {
        let method = match mode {
            AlignMode::Ecc => Method::Ecc(EccAligner::new()),
            AlignMode::Grid => Method::Grid(GridAligner::new(grid)),
        };
        Self { method, sequence: Sequence::Empty }
    }

    fn mode(&self) -> AlignMode {
        match self.method {
            Method::Ecc(_) => AlignMode::Ecc,
            Method::Grid(_) => AlignMode::Grid,
        }
    }

    /// What prepares frames for this aligner.
    pub fn preparer(&self) -> FramePreparer {
        FramePreparer(match &self.method {
            Method::Ecc(a) => Preparer::Ecc(*a.config()),
            Method::Grid(_) => Preparer::Grid,
        })
    }

    /// Align the next frame ([`AlignedFrame`]); ECC always reports a
    /// brightness of 1. On an error the sequence is unchanged. For tests
    /// and examples; the engine prepares frames ahead ([`Self::preparer`]).
    #[cfg(any(test, feature = "examples"))]
    pub fn align(&mut self, image: &DynamicImage) -> Result<AlignedFrame, AlignmentError> {
        self.align_prepared(self.preparer().prepare(image)?)
    }

    /// [`Self::align`] for a frame already prepared by this aligner's
    /// [`Self::preparer`].
    pub fn align_prepared(&mut self, frame: PreparedFrame) -> Result<AlignedFrame, AlignmentError> {
        let PreparedFrame { width, height, data_area, data } = frame;
        let whole = Rect { x: 0, y: 0, width, height };
        let data_area = data_area.map_or(whole, |area| whole.intersection(area));
        let Sequence::Running { size, last, valid_area, reference } = &mut self.sequence else {
            match (&mut self.method, &data) {
                (Method::Ecc(a), Prepared::Ecc(levels)) => a.first(levels)?,
                (Method::Grid(a), Prepared::Grid(pyramid)) => a.first(pyramid)?,
                _ => return Err(mismatched_preparer()),
            }
            self.sequence = Sequence::Running {
                size: (width, height),
                last: AlignedFrame::identity(),
                valid_area: data_area,
                reference: data,
            };
            tracing::debug!("aligner: running");
            tracing::info!("{}: reference frame {width}x{height}", self.mode().key());
            return Ok(AlignedFrame::identity());
        };
        // A size mismatch would otherwise give a confident wrong answer.
        if (width, height) != *size {
            return Err(AlignmentError::ProcessingFailed(format!(
                "frame is {width}x{height}, reference is {}x{}",
                size.0, size.1
            )));
        }

        let step = match (&mut self.method, &*reference, &data) {
            (Method::Ecc(a), Prepared::Ecc(template), Prepared::Ecc(levels)) => {
                a.step(template, levels, width, height)?
            }
            (Method::Grid(a), Prepared::Grid(previous), Prepared::Grid(pyramid)) => {
                a.step(previous, pyramid, &last.transform, width)?
            }
            _ => return Err(mismatched_preparer()),
        };
        *reference = data;
        // Reference → previous applies first, then previous → this frame.
        *last = AlignedFrame {
            transform: step.transform * last.transform,
            brightness: step.brightness * last.brightness,
        };

        // Full precision, one line per frame, to diff a run's transforms
        // against another build's (`RUST_LOG=deptheos_aligner=debug`).
        {
            let m = last.transform.matrix();
            tracing::debug!(
                "TRANSFORM {} {} {} {} {} {}",
                m[(0, 0)],
                m[(0, 1)],
                m[(0, 2)],
                m[(1, 0)],
                m[(1, 1)],
                m[(1, 2)]
            );
        }
        if let Some(inverse) = last.transform.try_inverse() {
            let covered = whole.intersection(data_area.inside_after_transform(&inverse));
            *valid_area = valid_area.intersection(covered);
        }
        Ok(*last)
    }

    /// [`Self::align`] for a frame already converted to the method's gray
    /// (Rec. 709 luma for ECC, the equal-weight mean for the grid).
    #[cfg(test)]
    pub(crate) fn add_gray(&mut self, gray: &Plane) -> Result<AlignedFrame, AlignmentError> {
        self.align_prepared(self.preparer().prepare_frame(Frame::Gray(gray))?)
    }

    /// The region every frame covers, in reference pixels, when it is at
    /// least a quarter of the frame; otherwise, or before frame 0, `None`.
    pub fn crop(&self) -> Option<Rect> {
        match &self.sequence {
            Sequence::Empty => None,
            Sequence::Running { valid_area, size: (w, h), .. } => {
                let area = u64::from(valid_area.width) * u64::from(valid_area.height);
                (4 * area >= u64::from(*w) * u64::from(*h)).then_some(*valid_area)
            }
        }
    }
}

fn mismatched_preparer() -> AlignmentError {
    AlignmentError::ProcessingFailed("frame was prepared for the other alignment method".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_frames_are_rejected() {
        for mode in AlignMode::ALL {
            for (w, h) in [(0, 0), (0, 10), (10, 0)] {
                let image = DynamicImage::new_rgb8(w, h);
                let result = Aligner::new(mode).align(&image);
                assert!(result.is_err(), "{mode:?} {w}x{h}");
            }
        }
    }

    #[test]
    fn mode_names_parse() {
        for mode in AlignMode::ALL {
            assert_eq!(mode.key().parse::<AlignMode>(), Ok(mode));
            assert_eq!(mode.key().to_uppercase().parse::<AlignMode>(), Ok(mode));
        }
        assert!("affine".parse::<AlignMode>().is_err());
    }

    /// A frame with data in only part of it (binary alpha, ADR-132) keeps
    /// the crop within that part, as it maps.
    #[test]
    fn a_frames_data_area_bounds_the_area_every_frame_covers() {
        let (w, h) = (320, 240);
        let image = DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([(x * 7 % 251) as u8, (y * 13 % 241) as u8, ((x ^ y) % 239) as u8])
        }));
        let mut aligner = Aligner::new(AlignMode::Ecc);
        let prepare = |area| aligner_preparer().prepare(&image).unwrap().with_data_area(area);
        let reference = Rect { x: 10, y: 0, width: 300, height: 240 };
        aligner.align_prepared(prepare(Some(reference))).unwrap();
        assert_eq!(aligner.crop(), Some(reference));
        let later = Rect { x: 0, y: 20, width: 320, height: 200 };
        aligner.align_prepared(prepare(Some(later))).unwrap();
        let crop = aligner.crop().unwrap();
        assert!(
            crop.x >= 10
                && crop.y >= 20
                && crop.x + crop.width <= 310
                && crop.y + crop.height <= 220,
            "{crop:?}"
        );
    }

    fn aligner_preparer() -> FramePreparer {
        Aligner::new(AlignMode::Ecc).preparer()
    }

    #[test]
    fn crops_to_a_quarter_of_the_frame_or_more() {
        let (w, h) = (320, 240);
        let image = DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([(x * 7 % 251) as u8, (y * 13 % 241) as u8, ((x ^ y) % 239) as u8])
        }));
        let mut aligner = Aligner::new(AlignMode::Ecc);
        assert_eq!(aligner.crop(), None, "before frame 0");
        aligner.align(&image).unwrap();
        for ((width, height), crops) in [
            ((w, h), true),
            ((160, 120), true),
            ((320, 60), true),
            ((160, 119), false),
            ((0, 0), false),
        ] {
            let area = Rect { x: 0, y: 0, width, height };
            if let Sequence::Running { valid_area, .. } = &mut aligner.sequence {
                *valid_area = area;
            }
            assert_eq!(aligner.crop(), crops.then_some(area), "{width}x{height}");
        }
    }

    /// A prepared frame's planned bytes are what it holds, within 3 %.
    #[test]
    fn prepared_bytes_are_what_a_frame_holds() {
        let (w, h) = (3000, 2000);
        let image = DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([(x % 251) as u8, (y % 241) as u8, ((x ^ y) % 239) as u8])
        }));
        for mode in AlignMode::ALL {
            let preparer = Aligner::new(mode).preparer();
            let held: u64 = match preparer.prepare(&image).unwrap().data {
                Prepared::Ecc(levels) => levels
                    .0
                    .iter()
                    .map(|l| (l.blurred.data.len() * 4 + l.packed.len() * 16) as u64)
                    .sum(),
                Prepared::Grid(pyramid) => {
                    pyramid.layers.iter().map(|l| l.data.len() as u64 * 4).sum()
                }
            };
            let planned = preparer.bytes(w, h);
            let off = (planned as f64 / held as f64 - 1.0).abs();
            assert!(off < 0.03, "{mode:?}: planned {planned}, holds {held}");
        }
    }
}
