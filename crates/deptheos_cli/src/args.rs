//! The command line: subcommands, shared option groups, value types, and
//! the checks that refuse a flag which would do nothing.
//!
//! Options that only matter with another (`--jpeg-quality` with a JPEG)
//! have no clap default, so `None` means "not given" for [`Checks`]; the
//! default is applied where the value is used and named in the help.

use clap::{Args, Parser, Subcommand, ValueEnum};
use deptheos_app::{
    AlignMode, AlignSettings, Alignment, BLEND_RANGE, DENOISE_RANGE, DcpPreference, DepthConfig,
    FrameOrder, GridConfig, MAX_CONSISTENCY, MAX_CONVERTER_JOBS, PyramidConfig, RADIUS_RANGE,
    RawBackend, SMOOTHING_RANGE, SMOOTHNESS_RANGE, SaveOptions, SessionOptions, StackerConfig,
    TiffCompression, WaveletConfig,
};
use std::path::PathBuf;

use crate::{align, batch, bench, sequence};
use deptheos_app::frames::folder::Source;
use deptheos_app::library::export::ExportFormat;
use deptheos_app::runs::sequence::FrameFormat;

const ABOUT: &str = "Deptheos: GPU focus stacking";

const LONG_ABOUT: &str = "Deptheos: GPU focus stacking.

Stack a focus bracket with one of three methods (pyramid, depth, wavelet), \
stack many folders at once (batch), align frames without stacking them \
(align), make grow and turntable sequences (sequence), or time the methods \
on this machine (bench).

The files written are listed on stdout, one path per line (bench prints its \
table there instead); progress, warnings and errors go to stderr.";

const EXAMPLES: &str = "\
Examples:
  deptheos pyramid ~/shots/bee -o bee.tif
  deptheos depth IMG_*.CR3 -o bee.dng --radius 12
  deptheos batch pyramid ~/shots/* -o ~/stacked
  deptheos align ~/shots/bee -o ~/bee-aligned
  deptheos sequence turntable ~/turn/s* -o ~/sequences";

const EXIT_STATUS: &str = "\
Exit status:
  0    everything was made
  1    an error stopped the run, or it asked for something Deptheos
       refuses (an output that is one of the frames, a folder with none)
  2    an option was unknown or conflicting, or a batch or turntable
       finished without the stacks that failed or couldn't be written
  130  interrupted (Ctrl-C); what was written is kept";

#[derive(Parser)]
#[command(
    name = "deptheos",
    version,
    about = ABOUT,
    long_about = LONG_ABOUT,
    after_help = EXAMPLES,
    after_long_help = format!("{EXAMPLES}\n\n{EXIT_STATUS}"),
    arg_required_else_help = true,
    propagate_version = true
)]
pub(crate) struct Cli {
    #[command(flatten)]
    pub(crate) log: LogArgs,

    #[command(subcommand)]
    pub(crate) command: Command,
}

/// How much goes to stderr.
#[derive(Args)]
pub(crate) struct LogArgs {
    /// Print only warnings and errors on stderr (the files written are
    /// still listed on stdout)
    #[arg(short, long, global = true, conflicts_with = "verbose")]
    pub(crate) quiet: bool,

    /// Log the engine's steps too (debug level). RUST_LOG, when set,
    /// takes precedence
    #[arg(short, long, global = true)]
    pub(crate) verbose: bool,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Stack with Pyramid: the all-around choice
    ///
    /// Blends the sharpest detail of every frame at each scale. An edge in
    /// front of a far background can get a soft glow.
    #[command(after_help = "\
Examples:
  deptheos pyramid ~/shots/bee -o bee.tif
  deptheos pyramid IMG_*.CR3 -o bee.dng --smoothness 2
  deptheos pyramid frames/ -o bee.jpg --align grid --grid-rotation 3")]
    Pyramid {
        #[command(flatten)]
        method: PyramidArgs,
        #[command(flatten)]
        stack: StackArgs,
    },

    /// Stack through a depth map: smooth backgrounds, clean bright edges
    ///
    /// Finds the sharpest frame at each spot, then blends between
    /// neighboring frames. Hairs and fine texture come out softer than with
    /// Pyramid.
    #[command(after_help = "\
Examples:
  deptheos depth ~/shots/bee -o bee.tif --radius 12 --smoothing 6
  deptheos depth ~/shots/bee -o sweep/ --sweep-radius 4,8,16 --sweep-smoothing 0,4")]
    Depth {
        #[command(flatten)]
        method: DepthArgs,
        #[command(flatten)]
        sweep: SweepArgs,
        #[command(flatten)]
        stack: StackArgs,
    },

    /// Stack with Wavelet: each pixel's color comes whole from one frame
    ///
    /// Merges the frames' detail scale by scale, then each pixel takes the
    /// color of the frame closest to the merged result. For fine crossing
    /// detail; a plain background can come out grainy. Denoise keeps noise
    /// in smooth areas from being picked up as detail.
    #[command(after_help = "\
Examples:
  deptheos wavelet ~/shots/bee -o bee.tif
  deptheos wavelet ~/shots/bee -o bee.tif --denoise 2 --consistency 2")]
    Wavelet {
        #[command(flatten)]
        method: WaveletArgs,
        #[command(flatten)]
        stack: StackArgs,
    },

    /// Stack many folders, one stack each (or split by capture time)
    #[command(
        subcommand,
        after_help = "\
Examples:
  deptheos batch pyramid ~/shots/* -o ~/stacked
  deptheos batch depth ~/card/DCIM/100CANON --split auto -o ~/stacked --format dng",
        after_long_help = EXIT_STATUS
    )]
    Batch(batch::BatchCommand),

    /// Align frames without stacking them: one aligned image per frame
    ///
    /// Aligns the frames exactly as a stack would (same aligner, reverse
    /// check and crop), then writes each one warped, all the same size, to
    /// stack or retouch elsewhere. A frame that can't be aligned is left
    /// out, with a warning.
    #[command(after_help = "\
Examples:
  deptheos align ~/shots/bee -o ~/bee-aligned
  deptheos align IMG_*.CR3 -o aligned --format dng
  deptheos align frames/ -o aligned --align grid --no-crop --format png")]
    Align(align::AlignFramesArgs),

    /// Time the methods and aligners on one set of frames
    ///
    /// Prints a Markdown table of the times on stdout; --json keeps the
    /// full record. With --raw-backend adobe-dng, camera RAW is converted
    /// before timing and the DNGs are what is timed.
    #[command(after_help = "\
Examples:
  deptheos bench ~/shots/bee
  deptheos bench ~/shots/bee --stacker pyramid --align ecc --repeat 3 --json bench.json")]
    Bench(bench::BenchArgs),

    /// Grow and turntable sequences: numbered frames for a video editor
    ///
    /// Each sequence is a folder of numbered frames (TIFF, PNG or JPEG) an
    /// editor imports as one clip, with a manifest.json.
    #[command(subcommand)]
    Sequence(sequence::SequenceCommand),

    /// Draw a saved render again, with its retouching, for tuning by eye
    ///
    /// Reads one render's folder in the Library and writes it through the
    /// path adjustments take, as the app would show it adjusted. It reads
    /// only that folder, so it runs while the app is open.
    #[command(after_help = "\
Examples:
  deptheos redraw ~/Library/Application\\ Support/Deptheos/Renders/<id> -o bee.tif
  deptheos redraw ~/Library/Application\\ Support/Deptheos/Renders/<id> -o bee.png --space srgb")]
    Redraw(crate::redraw::RedrawArgs),
}

// --- Value types, each mapping onto the engine's.

/// Which aligner, or none.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub(crate) enum Aligner {
    /// ECC: lines each frame up with the one before it (shift, rotation,
    /// size, stretch and slant)
    #[default]
    Ecc,
    /// Grid search: shift, rotation and magnification only (see --grid-*)
    Grid,
    /// No alignment, for frames already lined up
    None,
}

impl Aligner {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Ecc => AlignMode::Ecc.key(),
            Self::Grid => AlignMode::Grid.key(),
            Self::None => "none",
        }
    }

    /// This aligner's [`Alignment`]; `None` is off, resuming as ECC.
    pub(crate) fn alignment(self, crop: bool, grid: GridConfig) -> Alignment {
        let mode = match self {
            Self::Ecc | Self::None => AlignMode::Ecc,
            Self::Grid => AlignMode::Grid,
        };
        let settings = AlignSettings { mode, grid, crop };
        Alignment::new(self != Self::None, settings)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub(crate) enum Order {
    /// File name, numbers compared as numbers
    #[default]
    Name,
    /// Capture time with sub-seconds (a stack that can't be time-sorted
    /// stays in name order, with a warning)
    Time,
}

impl From<Order> for FrameOrder {
    fn from(o: Order) -> Self {
        match o {
            Order::Name => FrameOrder::Name,
            Order::Time => FrameOrder::CaptureTime,
        }
    }
}

/// A file format the stacks can be written in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum Format {
    #[value(alias = "tiff")]
    Tif,
    Png,
    #[value(alias = "jpeg")]
    Jpg,
    Dng,
}

impl Format {
    pub(crate) fn export(self) -> ExportFormat {
        match self {
            Self::Tif => ExportFormat::Tiff,
            Self::Png => ExportFormat::Png,
            Self::Jpg => ExportFormat::Jpeg,
            Self::Dng => ExportFormat::Dng,
        }
    }

    pub(crate) fn extension(self) -> &'static str {
        self.export().extension()
    }
}

/// A sequence's frame format: [`Format`] without DNG.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub(crate) enum FrameFormatArg {
    #[default]
    #[value(alias = "tiff")]
    Tif,
    Png,
    #[value(alias = "jpeg")]
    Jpg,
}

impl FrameFormatArg {
    pub(crate) fn format(self) -> FrameFormat {
        match self {
            Self::Tif => FrameFormat::Tiff,
            Self::Png => FrameFormat::Png,
            Self::Jpg => FrameFormat::Jpeg,
        }
    }

    pub(crate) fn export(self) -> ExportFormat {
        match self {
            Self::Tif => Format::Tif,
            Self::Png => Format::Png,
            Self::Jpg => Format::Jpg,
        }
        .export()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub(crate) enum SourceArg {
    /// The camera RAW or DNG
    #[default]
    Raw,
    /// The JPEG, TIFF or PNG
    Standard,
}

impl From<SourceArg> for Source {
    fn from(arg: SourceArg) -> Self {
        match arg {
            SourceArg::Raw => Self::Raw,
            SourceArg::Standard => Self::Standard,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum BitDepth {
    #[value(name = "8")]
    Eight,
    #[value(name = "16")]
    Sixteen,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub(crate) enum Compression {
    #[default]
    Zip,
    Lzw,
    None,
}

impl From<Compression> for TiffCompression {
    fn from(c: Compression) -> Self {
        match c {
            Compression::Zip => TiffCompression::Zip,
            Compression::Lzw => TiffCompression::Lzw,
            Compression::None => TiffCompression::None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub(crate) enum Backend {
    /// LibRaw, built in
    #[default]
    Libraw,
    /// Adobe DNG Converter, installed separately (see --dng-converter)
    AdobeDng,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub(crate) enum Profile {
    /// Adobe Standard
    #[default]
    #[value(alias = "adobe-standard", alias = "adobe_standard")]
    Adobe,
    /// Camera Standard (the camera maker's look)
    #[value(alias = "camera-standard", alias = "camera_standard")]
    Camera,
    /// The colors in the file, with no camera profile
    None,
}

impl From<Profile> for DcpPreference {
    fn from(p: Profile) -> Self {
        match p {
            Profile::Adobe => DcpPreference::AdobeStandard,
            Profile::Camera => DcpPreference::CameraStandard,
            Profile::None => DcpPreference::None,
        }
    }
}

/// An `f32` parser for the inclusive range `(lo, hi)`, rejecting NaN.
pub(crate) fn in_range((lo, hi): (f32, f32)) -> impl Fn(&str) -> Result<f32, String> + Clone {
    move |s| {
        let v: f32 = s.trim().parse().map_err(|_| format!("'{s}' isn't a number"))?;
        if (lo..=hi).contains(&v) { Ok(v) } else { Err(format!("must be from {lo} to {hi}")) }
    }
}

/// A JPEG quality, in the range a save takes.
fn jpeg_quality() -> clap::builder::RangedI64ValueParser<u8> {
    let range = deptheos_app::JPEG_QUALITY_RANGE;
    clap::value_parser!(u8).range(i64::from(*range.start())..=i64::from(*range.end()))
}

// --- Option groups.

/// The frames a stacking command or `align` reads, and their order.
#[derive(Args)]
pub(crate) struct FramesArgs {
    /// Frames (JPEG, PNG, TIFF, or camera RAW and DNG), or a folder
    /// of them. A stack is all RAW and DNG or all standard images
    #[arg(required = true)]
    pub(crate) inputs: Vec<PathBuf>,

    /// The order the frames stack in
    #[arg(long, value_enum, default_value_t, help_heading = "Input")]
    pub(crate) order: Order,
}

/// The session options from the order, align and decode groups.
pub(crate) fn session_options(
    order: Order,
    align: &AlignArgs,
    decode: &DecodeArgs,
) -> SessionOptions {
    let mut options = SessionOptions {
        raw_backend: decode.backend(),
        camera_profile: decode.profile(),
        order: order.into(),
        ..Default::default()
    };
    align.apply(&mut options);
    options
}

/// What the stacking commands share: inputs, output, alignment, decoding.
#[derive(Args)]
pub(crate) struct StackArgs {
    #[command(flatten)]
    pub(crate) frames: FramesArgs,

    /// The result: .tif, .png, .jpg, or .dng (camera RAW input only: a
    /// linear DNG with the first frame's camera metadata)
    #[arg(short, long, help_heading = "Output")]
    pub(crate) output: PathBuf,

    #[command(flatten)]
    pub(crate) save: SaveArgs,

    #[command(flatten)]
    pub(crate) align: AlignArgs,

    #[command(flatten)]
    pub(crate) decode: DecodeArgs,

    #[command(flatten)]
    pub(crate) debug: DebugArgs,
}

impl StackArgs {
    pub(crate) fn options(&self) -> SessionOptions {
        session_options(self.frames.order, &self.align, &self.decode)
    }
}

/// How images are written.
#[derive(Args)]
pub(crate) struct SaveArgs {
    /// Bits per channel for TIFF and PNG [default: 16 where the stack has
    /// 16; JPEG is always 8]
    #[arg(long, value_enum, help_heading = "Output")]
    pub(crate) bit_depth: Option<BitDepth>,

    /// JPEG quality, 1 to 100 [default: 92]
    #[arg(long, value_parser = jpeg_quality(), help_heading = "Output")]
    pub(crate) jpeg_quality: Option<u8>,

    /// TIFF compression [default: zip]
    #[arg(long, value_enum, help_heading = "Output")]
    pub(crate) tiff_compression: Option<Compression>,
}

impl SaveArgs {
    pub(crate) fn options(&self) -> SaveOptions {
        let default = SaveOptions::default();
        SaveOptions {
            jpeg_quality: self.jpeg_quality.unwrap_or(default.jpeg_quality),
            sixteen_bit: self.bit_depth.map_or(default.sixteen_bit, |b| b == BitDepth::Sixteen),
            tiff_compression: self.tiff_compression.map_or(default.tiff_compression, Into::into),
        }
    }

    /// Refuse what doesn't apply to files of these `formats`.
    pub(crate) fn check(&self, formats: &[ExportFormat], c: &mut Checks) {
        let any = |f: ExportFormat| formats.contains(&f);
        if self.jpeg_quality.is_some() && !any(ExportFormat::Jpeg) {
            c.refuse("--jpeg-quality applies only when writing a JPEG");
        }
        if self.tiff_compression.is_some() && !any(ExportFormat::Tiff) {
            c.refuse("--tiff-compression applies only when writing a TIFF");
        }
        if self.bit_depth.is_some() && !any(ExportFormat::Tiff) && !any(ExportFormat::Png) {
            c.refuse(
                "--bit-depth applies only to TIFF and PNG (JPEG is 8-bit, DNG floating point)",
            );
        }
    }
}

/// How the frames are aligned.
#[derive(Args)]
pub(crate) struct AlignArgs {
    /// The aligner
    #[arg(short, long, value_enum, default_value_t, help_heading = "Alignment")]
    pub(crate) align: Aligner,

    /// Keep the full frame instead of cropping to the area every frame
    /// covers
    #[arg(long, help_heading = "Alignment")]
    pub(crate) no_crop: bool,

    /// Grid: horizontal search limit in percent of the longer side, 0 to 25
    /// [default: 5]
    #[arg(long, value_parser = in_range(GridConfig::SHIFT_PCT_RANGE), help_heading = "Alignment")]
    pub(crate) grid_shift_x: Option<f32>,

    /// Grid: vertical search limit in percent of the longer side, 0 to 25
    /// [default: 5]
    #[arg(long, value_parser = in_range(GridConfig::SHIFT_PCT_RANGE), help_heading = "Alignment")]
    pub(crate) grid_shift_y: Option<f32>,

    /// Grid: rotation limit in degrees, 0 to 10, 0 = no rotation search
    /// [default: 1]
    #[arg(long, value_parser = in_range(GridConfig::ROTATION_DEG_RANGE), help_heading = "Alignment")]
    pub(crate) grid_rotation: Option<f32>,

    /// Grid: magnification limit in percent, 0 to 15, 0 = no magnification
    /// search [default: 3]
    #[arg(long, value_parser = in_range(GridConfig::MAGNIFICATION_PCT_RANGE), help_heading = "Alignment")]
    pub(crate) grid_magnification: Option<f32>,

    /// Grid: scale each frame to the first frame's brightness. For stacks
    /// shot at varying exposure only: on a constant-exposure stack the
    /// ratio measures vignetting and darkens the result
    #[arg(long, help_heading = "Alignment")]
    pub(crate) grid_brightness: bool,
}

impl AlignArgs {
    pub(crate) fn apply(&self, options: &mut SessionOptions) {
        let d = GridConfig::default();
        let grid = GridConfig {
            horizontal_pct: self.grid_shift_x.unwrap_or(d.horizontal_pct),
            vertical_pct: self.grid_shift_y.unwrap_or(d.vertical_pct),
            rotation_deg: self.grid_rotation.unwrap_or(d.rotation_deg),
            magnification_pct: self.grid_magnification.unwrap_or(d.magnification_pct),
            apply_brightness: self.grid_brightness,
            ..d
        };
        options.alignment = self.align.alignment(!self.no_crop, grid);
    }

    pub(crate) fn check(&self, c: &mut Checks) {
        let grid = self.grid_shift_x.is_some()
            || self.grid_shift_y.is_some()
            || self.grid_rotation.is_some()
            || self.grid_magnification.is_some()
            || self.grid_brightness;
        if grid && self.align != Aligner::Grid {
            c.refuse("the --grid-* options apply only with --align grid");
        }
        if self.align == Aligner::None && self.no_crop {
            c.refuse("--no-crop applies only to aligned frames, not with --align none");
        }
    }
}

/// How camera RAW is decoded.
#[derive(Args)]
pub(crate) struct DecodeArgs {
    /// RAW decoder
    #[arg(long, value_enum, default_value_t, help_heading = "Camera RAW")]
    pub(crate) raw_backend: Backend,

    /// Adobe DNG Converter's executable, where it isn't in the usual place
    #[arg(long, value_name = "PATH", help_heading = "Camera RAW")]
    pub(crate) dng_converter: Option<PathBuf>,

    /// How many files Adobe DNG Converter converts at once [default: a
    /// quarter of the CPU threads, 2 to 8]
    #[arg(short, long, value_name = "N", help_heading = "Camera RAW",
          value_parser = clap::value_parser!(u32).range(1..=MAX_CONVERTER_JOBS as i64))]
    pub(crate) jobs: Option<u32>,

    /// Write Adobe DNG Converter's files uncompressed: they decode about a
    /// quarter faster but take twice the disk (about 270 MB a 45 MP frame,
    /// all of a stack's at once)
    #[arg(long, help_heading = "Camera RAW")]
    pub(crate) dng_uncompressed: bool,

    /// Camera profile for RAW decoded by LibRaw [default: adobe]
    #[arg(long, value_enum, help_heading = "Camera RAW")]
    pub(crate) camera_profile: Option<Profile>,

    /// Look for camera profiles (.dcp) in this folder too, before Adobe's
    /// own folders; repeat it for more than one
    #[arg(long, value_name = "DIR", help_heading = "Camera RAW")]
    pub(crate) camera_profile_folder: Vec<PathBuf>,
}

impl DecodeArgs {
    pub(crate) fn backend(&self) -> RawBackend {
        match self.raw_backend {
            Backend::Libraw => RawBackend::LibRaw,
            Backend::AdobeDng => RawBackend::AdobeDng,
        }
    }

    pub(crate) fn profile(&self) -> DcpPreference {
        self.camera_profile.unwrap_or_default().into()
    }

    pub(crate) fn check(&self, c: &mut Checks) {
        let converter =
            self.dng_converter.is_some() || self.jobs.is_some() || self.dng_uncompressed;
        match self.raw_backend {
            Backend::Libraw if converter => c.refuse(
                "--dng-converter, --jobs and --dng-uncompressed apply only with --raw-backend adobe-dng",
            ),
            Backend::AdobeDng if self.camera_profile.is_some() => c.refuse(
                "--camera-profile applies only to LibRaw; Adobe DNG Converter's files carry their own",
            ),
            Backend::AdobeDng if !self.camera_profile_folder.is_empty() => c.refuse(
                "--camera-profile-folder applies only to LibRaw; Adobe DNG Converter's files carry their own profile",
            ),
            _ => {}
        }
        for dir in &self.camera_profile_folder {
            if !dir.is_dir() {
                c.refuse(format!("--camera-profile-folder {}: no such folder", dir.display()));
            }
        }
    }
}

/// Outputs for looking into a stack.
#[derive(Args)]
pub(crate) struct DebugArgs {
    /// Write every frame with the contribution overlay (what the result
    /// took from it, the rest gray and dim) into this folder, as
    /// "<number> <frame>_contribution.jpg" (a sweep gets a folder per run)
    #[arg(long, value_name = "DIR", help_heading = "Debug")]
    pub(crate) contributions: Option<PathBuf>,
}

// --- The methods' own options.

#[derive(Args, Clone)]
pub(crate) struct PyramidArgs {
    /// Smoothness, 1 (crisp, more noise) to 10 (calm, softer)
    #[arg(long, default_value_t = PyramidConfig::DEFAULT_SMOOTHNESS,
          value_parser = in_range(SMOOTHNESS_RANGE), help_heading = "Pyramid")]
    pub(crate) smoothness: f32,
}

impl PyramidArgs {
    pub(crate) fn config(&self) -> StackerConfig {
        StackerConfig::Pyramid(PyramidConfig { smoothness: self.smoothness })
    }
}

#[derive(Args, Clone)]
pub(crate) struct DepthArgs {
    /// Radius, 1 to 50: how large an area decides each spot's depth
    /// together
    #[arg(long, default_value_t = DepthConfig::DEFAULT_RADIUS,
          value_parser = in_range(RADIUS_RANGE), help_heading = "Depth")]
    pub(crate) radius: f32,

    /// Smoothing, 0 to 20: how much the depth map is smoothed; 0 = off
    #[arg(long, default_value_t = DepthConfig::DEFAULT_SMOOTHING,
          value_parser = in_range(SMOOTHING_RANGE), help_heading = "Depth")]
    pub(crate) smoothing: f32,

    /// Blend Range, 0 to 10: 10 leaves the depths as they are, lower pulls
    /// each pixel more steeply toward its nearest frame
    #[arg(long, default_value_t = DepthConfig::DEFAULT_BLEND_RANGE,
          value_parser = in_range(BLEND_RANGE), help_heading = "Depth")]
    pub(crate) blend_range: f32,
}

impl DepthArgs {
    pub(crate) fn config(&self) -> StackerConfig {
        depth(self.radius, self.smoothing, self.blend_range)
    }
}

pub(crate) fn depth(radius: f32, smoothing: f32, blend_range: f32) -> StackerConfig {
    StackerConfig::Depth(DepthConfig { radius, smoothing, blend_range })
}

#[derive(Args, Clone)]
pub(crate) struct WaveletArgs {
    /// Denoise, 0 to 10: drops merged detail weaker than this, so noise in
    /// smooth areas isn't taken for detail; 0 = off
    #[arg(long, default_value_t = WaveletConfig::DEFAULT_DENOISE,
          value_parser = in_range(DENOISE_RANGE), help_heading = "Wavelet")]
    pub(crate) denoise: f32,

    /// Consistency filter, 0 to 2: 1 = where two of a spot's three detail
    /// directions agree on a frame, the third follows; 2 = that, and a spot
    /// whose frame stands out from all four neighbors' takes theirs.
    /// Doubles the merge's memory
    #[arg(long, default_value_t = WaveletConfig::default().consistency, help_heading = "Wavelet",
          value_parser = clap::value_parser!(u32).range(0..=MAX_CONSISTENCY as i64))]
    pub(crate) consistency: u32,
}

impl WaveletArgs {
    pub(crate) fn config(&self) -> StackerConfig {
        StackerConfig::Wavelet(WaveletConfig {
            denoise: self.denoise,
            consistency: self.consistency,
        })
    }
}

/// A depth sweep: every radius with every smoothing.
#[derive(Args)]
pub(crate) struct SweepArgs {
    /// Sweep these radii (comma-separated). With a sweep, --output is a
    /// folder: each run is written as r<radius>_s<smoothing>.<ext> (with
    /// _b<blend range> when --blend-range isn't the default), and alignment
    /// runs once for all
    #[arg(long, value_name = "R,…", value_delimiter = ',',
          value_parser = in_range(RADIUS_RANGE), help_heading = "Sweep")]
    pub(crate) sweep_radius: Vec<f32>,

    /// Sweep these smoothings (comma-separated; see --sweep-radius)
    #[arg(long, value_name = "S,…", value_delimiter = ',',
          value_parser = in_range(SMOOTHING_RANGE), help_heading = "Sweep")]
    pub(crate) sweep_smoothing: Vec<f32>,

    /// The sweep's file format [default: jpg]
    #[arg(long, value_enum, help_heading = "Sweep")]
    pub(crate) sweep_format: Option<Format>,
}

impl SweepArgs {
    pub(crate) fn is_sweep(&self) -> bool {
        !self.sweep_radius.is_empty() || !self.sweep_smoothing.is_empty()
    }

    pub(crate) fn format(&self) -> Format {
        self.sweep_format.unwrap_or(Format::Jpg)
    }
}

// --- Checks across options.

/// The refusals found; reported as one usage error.
#[derive(Default)]
pub(crate) struct Checks(Vec<String>);

/// The subcommand names leading to the command given (`["batch", "depth"]`).
fn subcommand_path(cli: &Cli) -> Vec<&'static str> {
    match &cli.command {
        Command::Pyramid { .. } => vec!["pyramid"],
        Command::Depth { .. } => vec!["depth"],
        Command::Wavelet { .. } => vec!["wavelet"],
        Command::Batch(b) => vec!["batch", b.name()],
        Command::Align(_) => vec!["align"],
        Command::Bench(_) => vec!["bench"],
        Command::Sequence(s) => vec!["sequence", s.name()],
        Command::Redraw(_) => vec!["redraw"],
    }
}

impl Checks {
    pub(crate) fn refuse(&mut self, why: impl Into<String>) {
        self.0.push(why.into());
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Exits with a usage error (status 2, as clap does) showing the usage
    /// of the subcommand at `path`, if anything was refused.
    fn exit_on_refusal(self, path: &[&str]) {
        if self.0.is_empty() {
            return;
        }
        use clap::CommandFactory;
        let mut command = Cli::command();
        command.build();
        let mut command = &mut command;
        for name in path {
            command = command.find_subcommand_mut(name).expect("a subcommand of the CLI");
        }
        command.error(clap::error::ErrorKind::ArgumentConflict, self.0.join("\n       ")).exit();
    }
}

impl Cli {
    /// Exits with a usage error on options that would do nothing together.
    pub(crate) fn check(&self) {
        self.refusals().exit_on_refusal(&subcommand_path(self));
    }

    fn refusals(&self) -> Checks {
        let mut c = Checks::default();
        match &self.command {
            Command::Pyramid { stack, .. } | Command::Wavelet { stack, .. } => {
                stack.check(ExportFormat::of(&stack.output).as_slice(), &mut c);
            }
            Command::Depth { stack, sweep, .. } => {
                if sweep.is_sweep() {
                    stack.check(&[sweep.format().export()], &mut c);
                } else {
                    if sweep.sweep_format.is_some() {
                        c.refuse(
                            "--sweep-format applies only with --sweep-radius or --sweep-smoothing",
                        );
                    }
                    stack.check(ExportFormat::of(&stack.output).as_slice(), &mut c);
                }
            }
            Command::Batch(command) => command.check(&mut c),
            Command::Align(args) => args.check(&mut c),
            Command::Bench(args) => args.check(&mut c),
            Command::Sequence(command) => command.check(&mut c),
            Command::Redraw(args) => args.check(&mut c),
        }
        c
    }
}

impl StackArgs {
    fn check(&self, formats: &[ExportFormat], c: &mut Checks) {
        self.save.check(formats, c);
        self.align.check(c);
        self.decode.check(c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_is_well_formed() {
        Cli::command().debug_assert();
    }

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(["deptheos"].iter().chain(args))
    }

    /// Parses, and passes the checks across options.
    fn accepted(args: &[&str]) -> bool {
        parse(args).is_ok_and(|cli| cli.refusals().0.is_empty())
    }

    /// `--align`, `--no-crop` and `--grid-*` as the session takes them,
    /// none with `none`; the reverse check is always on.
    #[test]
    fn the_aligner_says_how_the_frames_align() {
        let grid = GridConfig { rotation_deg: 3.0, ..GridConfig::default() };
        assert!(matches!(
            Aligner::None.alignment(true, grid),
            Alignment::Off { resumes: AlignSettings { mode: AlignMode::Ecc, .. } }
        ));
        assert!(matches!(
            Aligner::Ecc.alignment(true, grid),
            Alignment::On(AlignSettings { mode: AlignMode::Ecc, crop: true, .. })
        ));
        assert!(parse(&["pyramid", "a.jpg", "-o", "o.jpg", "--no-reverse-check"]).is_err());
        let Alignment::On(AlignSettings { mode: AlignMode::Grid, grid: g, crop: false }) =
            Aligner::Grid.alignment(false, grid)
        else {
            panic!("Grid search, uncropped");
        };
        assert_eq!(g.rotation_deg, 3.0);
    }

    #[test]
    fn ranges_are_enforced() {
        assert!(parse(&["depth", "a.jpg", "-o", "o.jpg", "--radius", "8"]).is_ok());
        for bad in ["0.5", "51", "-1", "NaN", "inf", "x"] {
            assert!(parse(&["depth", "a.jpg", "-o", "o.jpg", "--radius", bad]).is_err(), "{bad}");
        }
        assert!(parse(&["depth", "a.jpg", "-o", "o", "--sweep-smoothing", "0,4,21"]).is_err());
        assert!(parse(&["pyramid", "a.jpg", "-o", "o.jpg", "--smoothness", "11"]).is_err());
        assert!(parse(&["depth", "a.jpg", "-o", "o.jpg", "--blend-range", "-0.1"]).is_err());
        assert!(parse(&["wavelet", "a.jpg", "-o", "o.jpg", "--denoise", "11"]).is_err());
        assert!(parse(&["wavelet", "a.jpg", "-o", "o.jpg", "--consistency", "3"]).is_err());
        assert!(
            parse(&["pyramid", "a", "-o", "o.jpg", "-a", "grid", "--grid-rotation", "11"]).is_err()
        );
    }

    #[test]
    fn a_value_out_of_range_gives_the_range_in_words() {
        let smoothness = in_range((1.0, 10.0));
        assert_eq!(smoothness("12").unwrap_err(), "must be from 1 to 10");
        assert_eq!(smoothness("x").unwrap_err(), "'x' isn't a number");
    }

    #[test]
    fn values_take_the_stored_spellings_too() {
        for s in ["adobe", "adobe-standard", "adobe_standard"] {
            assert!(accepted(&["pyramid", "a", "-o", "o.jpg", "--camera-profile", s]), "{s}");
        }
        assert!(!accepted(&["pyramid", "a", "-o", "o.jpg", "--camera-profile", "standard"]));
        assert!(accepted(&[
            "pyramid",
            "a",
            "-o",
            "o.jpg",
            "--raw-backend",
            "adobe-dng",
            "-j",
            "4"
        ]));
    }

    #[test]
    fn options_that_would_do_nothing_are_refused() {
        let base = ["pyramid", "a", "-o"];
        let with = |out: &str, more: &[&str]| {
            let mut v: Vec<&str> = base.to_vec();
            v.push(out);
            v.extend(more);
            accepted(&v)
        };
        assert!(with("o.jpg", &["--jpeg-quality", "80"]));
        assert!(!with("o.tif", &["--jpeg-quality", "80"]));
        assert!(with("o.tif", &["--tiff-compression", "lzw", "--bit-depth", "8"]));
        assert!(!with("o.png", &["--tiff-compression", "lzw"]));
        assert!(!with("o.jpg", &["--bit-depth", "16"]));
        // By the extension, whatever its case or spelling.
        assert!(with("o.JPEG", &["--jpeg-quality", "80"]));
        assert!(with("o.TIFF", &["--tiff-compression", "lzw", "--bit-depth", "16"]));
        assert!(with("o.PNG", &["--bit-depth", "16"]));
        assert!(!with("o.dng", &["--bit-depth", "16"]));
        assert!(!with("o.dng", &["--jpeg-quality", "80"]));
        assert!(!with("o.jpg", &["--grid-brightness"]));
        assert!(with("o.jpg", &["--align", "grid", "--grid-brightness", "--grid-shift-x", "10"]));
        assert!(!with("o.jpg", &["--align", "none", "--no-crop"]));
        assert!(!with("o.jpg", &["-j", "4"]));
        assert!(!with("o.jpg", &["--raw-backend", "adobe-dng", "--camera-profile", "camera"]));
        assert!(!accepted(&["depth", "a", "-o", "o.jpg", "--sweep-format", "png"]));
        assert!(accepted(&[
            "depth",
            "a",
            "-o",
            "o",
            "--sweep-radius",
            "4,8",
            "--sweep-format",
            "png"
        ]));
        // No stack writes a map beside its result.
        assert!(parse(&["pyramid", "a", "-o", "o.tif", "--map", "m.jpg"]).is_err());
    }

    #[test]
    fn sequence_options_are_checked() {
        assert!(accepted(&["sequence", "grow", "r", "-o", "out"]));
        assert!(accepted(&[
            "sequence",
            "grow",
            "a.jpg",
            "-o",
            "o",
            "--front-to-back",
            "--every",
            "2"
        ]));
        assert!(!accepted(&["sequence", "grow", "a.jpg", "-o", "o", "--format", "gif"]));
        assert!(!accepted(&["sequence", "grow", "a", "-o", "o", "--prores"]));
        assert!(!accepted(&["sequence", "grow", "a", "-o", "o", "--fps", "24"]));
        assert!(!accepted(&["sequence", "grow", "a", "-o", "o", "--jpeg-quality", "80"]));
        assert!(accepted(&[
            "sequence",
            "turntable",
            "s1",
            "s2",
            "-o",
            "o",
            "--mode",
            "both",
            "-N",
            "200",
            "--order",
            "time",
            "--name",
            "bee"
        ]));
        assert!(!accepted(&[
            "sequence",
            "turntable",
            "s1",
            "-o",
            "o",
            "--mode",
            "full",
            "-N",
            "9"
        ]));
        assert!(!accepted(&[
            "sequence",
            "turntable",
            "s1",
            "-o",
            "o",
            "--mode",
            "full",
            "--front-to-back"
        ]));
        assert!(!accepted(&["sequence", "turntable", "s1", "-o", "o", "--mode", "spin"]));
    }

    #[test]
    fn align_options_are_checked() {
        assert!(accepted(&["align", "a", "b", "-o", "out"]));
        assert!(accepted(&[
            "align",
            "a",
            "-o",
            "out",
            "--align",
            "grid",
            "--grid-rotation",
            "2",
            "--no-crop",
            "--order",
            "time"
        ]));
        assert!(accepted(&["align", "a", "-o", "out", "--format", "png", "--bit-depth", "8"]));
        assert!(accepted(&["align", "a", "-o", "out", "--format", "jpg", "--jpeg-quality", "95"]));
        // TIFF is the default format.
        assert!(accepted(&["align", "a", "-o", "out", "--tiff-compression", "lzw"]));
        assert!(!accepted(&["align", "a", "-o", "out", "--jpeg-quality", "95"]));
        assert!(!accepted(&["align", "a", "-o", "out", "--format", "dng", "--bit-depth", "16"]));
        assert!(!accepted(&["align", "a", "-o", "out", "--align", "none"]));
        assert!(!accepted(&["align", "a", "-o", "out", "--grid-brightness"]));
        // The output is a folder: there is no single-file form.
        assert!(parse(&["align", "a"]).is_err());
        // A stack's own options aren't the aligner's.
        assert!(parse(&["align", "a", "-o", "out", "--smoothness", "2"]).is_err());
    }

    #[test]
    fn camera_profile_folders_are_checked() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path().to_str().unwrap();
        let base = ["pyramid", "a", "-o", "o.tif", "--camera-profile-folder"];
        let with = |more: &[&str]| accepted(&[&base[..], more].concat());
        assert!(with(&[dir]));
        assert!(with(&[dir, "--camera-profile-folder", dir, "--camera-profile", "camera"]));
        assert!(!with(&["/no/such/folder"]));
        assert!(!with(&[dir, "--raw-backend", "adobe-dng"]));
        assert!(accepted(&["align", "a", "-o", "out", "--camera-profile-folder", dir]));
    }

    #[test]
    fn batch_and_bench_options_are_checked() {
        assert!(accepted(&["batch", "pyramid", "a", "b", "-o", "out"]));
        assert!(accepted(&[
            "batch", "depth", "a", "-o", "out", "--radius", "12", "--split", "auto"
        ]));
        assert!(!accepted(&["batch", "pyramid", "a", "-o", "out", "--jpeg-quality", "80"]));
        assert!(accepted(&[
            "batch",
            "pyramid",
            "a",
            "-o",
            "out",
            "--format",
            "jpg",
            "--jpeg-quality",
            "80"
        ]));
        assert!(accepted(&["bench", "a", "--stacker", "pyramid,wavelet", "--align", "ecc,none"]));
        assert!(!accepted(&["bench", "a", "--stacker", "fast"]));
    }
}
