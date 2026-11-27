//! The crate's error, worded for a person by [`crate::words::explain`].

use std::path::{Path, PathBuf};

use deptheos_aligner::AlignmentError;
use deptheos_core::CoreError;

use crate::RawBackend;

/// A stack's, export's or sequence's error. The text is for the log;
/// `InvalidState` is a fault (a misuse).
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error(transparent)]
    Core(#[from] CoreError),

    #[error("alignment: {0}")]
    Alignment(#[from] AlignmentError),

    #[error(transparent)]
    Dng(#[from] deptheos_dng::DngError),

    #[error(transparent)]
    Store(#[from] crate::StoreError),

    #[error("canceled")]
    Canceled,

    /// More frames failed than [`crate::runs::session::left_out_limit`]
    /// allows: likely not one focus stack. `first` names the first, with its
    /// [`crate::runs::session::LeftOut::reason`].
    #[error("{count} of {total} frames left out, the first {first}: {reason}")]
    TooManyLeftOut { count: usize, total: usize, first: String, reason: String },

    /// No frames to stack.
    #[error("no input frames")]
    NoFrames,

    /// Frame `file` (its name) is not the size of the stack's first frame.
    #[error("{file} is {}×{} but the stack's first frame is {}×{}", size.0, size.1, first.0, first.1)]
    FrameSize { file: String, size: (u32, u32), first: (u32, u32) },

    /// Frame `file` is of `kind`, but frame `first` set the other kind.
    #[error("{file} is {kind} but {first} is not")]
    MixedKinds { file: String, kind: FrameKind, first: String },

    /// A render of RAW frames and one of standard images, put together for
    /// `purpose`.
    #[error("a RAW render and a standard-image render can't be used together for {purpose}")]
    MixedRenders { purpose: RenderUse },

    /// Two renders compared share no frame to line them up through.
    #[error("the renders share no frame, so they can't be lined up")]
    NoSharedFrame,

    /// A DNG export of a stack of standard images.
    #[error("DNG output requires camera RAW or DNG source frames")]
    NotRaw,

    /// A grow sequence of a render made with another method.
    #[error("grow sequences need a pyramid render")]
    NeedsPyramid,

    /// A render's saved retouching couldn't be read.
    #[error("the render's retouching couldn't be read: {0}")]
    PaintUnread(crate::paint::store::Unread),

    /// A render's saved adjustments couldn't be read.
    #[error("the render's adjustments couldn't be read: {0}")]
    AdjustmentsUnread(crate::library::adjustments::Unread),

    /// A sequence's frames that aren't on disk (`stack` named when it has
    /// several).
    #[error("{}", missing_text(stack.as_deref(), files))]
    MissingFrames { stack: Option<String>, files: Vec<PathBuf> },

    /// A turntable sequence of no stacks.
    #[error("no stacks to make a turntable sequence of")]
    NoStacks,

    /// A turntable sequence whose stacks' crops share no area.
    #[error("the stacks share no common area")]
    NoCommonArea,

    /// A sequence needs `needed` bytes; `folder`'s volume has `free`.
    #[error("the sequence needs about {needed} bytes but {} has {free} free", folder.display())]
    NoSpace { folder: PathBuf, needed: u64, free: u64 },

    /// A stack's frame conversions need `needed` more bytes; their folder has
    /// `free`.
    #[error("converting the frames needs about {needed} bytes but {} has {free} free", folder.display())]
    NoConversionSpace { folder: PathBuf, needed: u64, free: u64 },

    /// A sample stack's frames need `needed` bytes; `free` are free.
    #[error("the sample stack needs {needed} bytes but {free} are free")]
    SampleNoSpace { needed: u64, free: u64 },

    #[error("{0}")]
    InvalidState(String),
}

fn missing_text(stack: Option<&str>, files: &[PathBuf]) -> String {
    let files: Vec<String> = files.iter().map(|f| f.display().to_string()).collect();
    match stack {
        Some(stack) => format!("{stack}: source frames missing: {}", files.join(", ")),
        None => format!("source frames missing: {}", files.join(", ")),
    }
}

/// The two kinds of frame a stack can be made of, never mixed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameKind {
    /// Camera RAW or DNG.
    Raw,
    /// JPEG, PNG, TIFF and the like.
    Standard,
}

impl FrameKind {
    /// The kind `path` decodes to under `backend`.
    pub fn of(path: &Path, backend: RawBackend) -> Self {
        if backend.decodes_linear(path) { Self::Raw } else { Self::Standard }
    }
}

impl std::fmt::Display for FrameKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Raw => "camera RAW/DNG",
            Self::Standard => "a standard image",
        })
    }
}

/// What two renders were put together for ([`Error::MixedRenders`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderUse {
    Compare,
    Paint,
}

impl std::fmt::Display for RenderUse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Compare => "comparing",
            Self::Paint => "painting",
        })
    }
}

pub type Result<T> = std::result::Result<T, Error>;
