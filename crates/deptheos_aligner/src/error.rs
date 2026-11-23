use thiserror::Error;

/// Why a frame could not be aligned. The text is technical, for the log;
/// `deptheos_app::words::explain` words it for a person. `EccFailed`,
/// `ProcessingFailed` and `ResizeFailed` are faults.
#[derive(Error, Debug)]
pub enum AlignmentError {
    /// The first frame has no contrast to align the others against: it
    /// may be blank or black.
    #[error("the first frame has no contrast to align against (standard deviation {std:.5})")]
    Featureless { std: f32 },

    /// ECC's correlation at one pyramid level was below the minimum.
    #[error(
        "correlation at {}×{} is {correlation:.3}, below {min} (ρ by level {by_level})",
        level.0, level.1
    )]
    LowCorrelation { correlation: f64, min: f64, level: (u32, u32), by_level: String },

    /// ECC's warp left too little of the frame over the one before it.
    #[error("frames overlap on {percent:.0}% of the template")]
    LittleOverlap { percent: f64 },

    /// ECC found no correlation it could improve: the frames are unrelated
    /// or barely overlap.
    #[error("images may be uncorrelated or non-overlapped")]
    Uncorrelated,

    /// The transform found is not plausible camera motion.
    #[error("implausible frame-to-frame motion: {motion} (ρ by level {by_level})")]
    Implausible { motion: Motion, by_level: String },

    /// The grid search sampled nothing of the frame before.
    #[error("the frame does not overlap the previous one where it was sampled")]
    NoOverlap,

    /// The grid search's best match is not well below an unrelated image's
    /// difference (`difference` and `max` in percent of it).
    #[error(
        "the frame does not match the previous one (difference {difference:.0}% of unrelated \
         images', at most {max:.0}% aligns)"
    )]
    NoMatch { difference: f64, max: f64 },

    #[error("ECC: {0}")]
    EccFailed(String),

    #[error("image processing: {0}")]
    ProcessingFailed(String),

    #[error("resize: {0}")]
    ResizeFailed(String),
}

/// How a frame-to-frame transform is not plausible camera motion. Each
/// measure is in percent of the frame (or degrees), with the most allowed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Motion {
    /// A matrix entry or a measure is not a finite number.
    NotFinite,
    /// The transform mirrors the frame or squashes it flat.
    Flips,
    Shift {
        dx: f64,
        dy: f64,
        max: f64,
    },
    Scale {
        percent: f64,
        max: f64,
    },
    Shear {
        percent: f64,
        max: f64,
    },
    Rotation {
        degrees: f64,
        max: f64,
    },
}

impl std::fmt::Display for Motion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::NotFinite => write!(f, "the transform is not finite"),
            Self::Flips => write!(f, "the transform flips or collapses the frame"),
            Self::Shift { dx, dy, max } => write!(f, "shift of {dx:.1}% × {dy:.1}% exceeds {max}%"),
            Self::Scale { percent, max } => {
                write!(f, "scale change of {percent:.1}% exceeds {max}%")
            }
            Self::Shear { percent, max } => write!(f, "shear of {percent:.1}% exceeds {max}%"),
            Self::Rotation { degrees, max } => {
                write!(f, "rotation of {degrees:.2}° exceeds {max}°")
            }
        }
    }
}
