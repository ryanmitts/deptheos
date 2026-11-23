//! Whether a stack aligns its frames, and how: one value, so the aligner
//! and crop apply only while aligned; off remembers them. An aligned stack
//! shot with magnification growing restarts reversed, so the most magnified
//! frame sets the geometry and the crop loses nothing.

use deptheos_aligner::{AlignMode, GridConfig};

/// Whether a stack aligns its frames; the render file keeps it as `align`,
/// `align_mode`, `crop` and `grid` (`render_file.rs`).
#[derive(Clone, Copy, Debug)]
pub enum Alignment {
    /// Stacked where they lie: no crop, no order check. `resumes` is only
    /// what turning alignment back on uses; never used to stack.
    Off { resumes: AlignSettings },
    /// Each frame is aligned to the stack's reference with these settings.
    On(AlignSettings),
}

/// How frames are aligned, while they are.
#[derive(Clone, Copy, Debug)]
pub struct AlignSettings {
    /// Which aligner.
    pub mode: AlignMode,
    /// Grid search's limits; kept under ECC for the render file and
    /// switching back.
    pub grid: GridConfig,
    /// Crop the result to the area every aligned frame covers.
    pub crop: bool,
}

impl Default for AlignSettings {
    /// ECC, cropped (the CLI's default).
    fn default() -> Self {
        Self { mode: AlignMode::Ecc, grid: GridConfig::default(), crop: true }
    }
}

impl Default for Alignment {
    /// Aligned with the default settings.
    fn default() -> Self {
        Self::On(AlignSettings::default())
    }
}

impl Alignment {
    /// On with `settings`, or off remembering them.
    pub fn new(on: bool, settings: AlignSettings) -> Self {
        if on { Self::On(settings) } else { Self::Off { resumes: settings } }
    }

    /// Whether the frames are aligned.
    pub fn is_on(&self) -> bool {
        matches!(self, Self::On(_))
    }

    /// The settings it aligns with, or, off, those it resumes with.
    pub fn settings(&self) -> AlignSettings {
        match *self {
            Self::Off { resumes } => resumes,
            Self::On(settings) => settings,
        }
    }

    /// Which aligner, while aligning.
    pub fn mode(&self) -> Option<AlignMode> {
        match self {
            Self::Off { .. } => None,
            Self::On(settings) => Some(settings.mode),
        }
    }

    /// Whether the result is cropped to the aligned frames' common area.
    pub fn crops(&self) -> bool {
        matches!(self, Self::On(AlignSettings { crop: true, .. }))
    }

    /// This alignment, on or off, with `settings` changed by `change`.
    pub fn map(self, change: impl FnOnce(AlignSettings) -> AlignSettings) -> Self {
        match self {
            Self::Off { resumes } => Self::Off { resumes: change(resumes) },
            Self::On(settings) => Self::On(change(settings)),
        }
    }

    /// This alignment with `grid` as Grid search's limits.
    pub fn with_grid(self, grid: GridConfig) -> Self {
        self.map(|s| AlignSettings { grid, ..s })
    }
}

/// Edits for the tests of what counts as a change.
#[cfg(test)]
impl Alignment {
    /// Align Frames turned over, the settings kept.
    pub(crate) fn turned_over(self) -> Self {
        Self::new(!self.is_on(), self.settings())
    }

    /// The other aligner.
    pub(crate) fn other_aligner(self) -> Self {
        self.map(|s| AlignSettings {
            mode: match s.mode {
                AlignMode::Ecc => AlignMode::Grid,
                AlignMode::Grid => AlignMode::Ecc,
            },
            ..s
        })
    }

    /// The crop turned over.
    pub(crate) fn crop_turned_over(self) -> Self {
        self.map(|s| AlignSettings { crop: !s.crop, ..s })
    }
}
