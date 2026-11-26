//! Why a command stopped, and how `main` prints it.
//!
//! An engine error is worded by `deptheos_app::words::explain`, as in the
//! apps. A [`Refusal`] is worded here in clap's form: lower case, no closing
//! full stop, options literal. An [`Error::Fault`] prints its text on a
//! `details:` line.

use std::fmt;
use std::path::{Path, PathBuf};

use deptheos_app::{CoreError, Explain, ExplanationKind, StoreError, Words, words::explain::fault};

pub(crate) type Result<T> = std::result::Result<T, Error>;

/// Why a command stopped.
#[derive(Debug)]
pub(crate) enum Error {
    /// The engine's error. `deptheos_app::Error::Canceled` is Ctrl-C.
    Engine(deptheos_app::Error),
    /// Something the command line won't do with what it was given.
    Refused(Refusal),
    /// A state this crate didn't expect; the text is the `details:` line.
    Fault(String),
}

/// What the command line refuses, each a case a user can cause and fix.
#[derive(Debug)]
pub(crate) enum Refusal {
    /// A sweep's `--output` is a file, or is named like one.
    SweepOutputIsFile(PathBuf),
    /// A DNG result needs camera metadata from a RAW first frame.
    DngNeedsRaw { output: PathBuf, first: PathBuf },
    /// An output is one of the input frames.
    OutputIsInput { output: PathBuf, input: PathBuf },
    /// Two outputs are one file.
    SameOutput { first: PathBuf, second: PathBuf },
    /// An output whose extension Deptheos doesn't write; `formats` lists
    /// what it does.
    FileType { path: PathBuf, formats: &'static str },
    /// An output in a folder that doesn't exist.
    NoFolder { path: PathBuf, folder: PathBuf },
    /// A folder given as input holds no frames.
    NoFrames(PathBuf),
    /// `bench` was given fewer than two frames.
    TooFewFrames(usize),
    /// No folder of a batch or turntable holds frames.
    NoFolderHasFrames,
    /// A batch's `--output` is or is inside `input`, a folder of its frames.
    OutputInFrames { output: PathBuf, input: PathBuf },
    /// A batch's or `align`'s `--output` is a file; `holds` is what the
    /// folder was for ("stacks").
    OutputIsFile { path: PathBuf, holds: &'static str },
    /// `align`'s `--output` doesn't exist and is named like a file.
    FolderNamedLikeFile(PathBuf),
    /// Two frames `align` would write under one name.
    SameName { first: PathBuf, second: PathBuf, output: PathBuf },
    /// A file `align` would write is there already; `more` others are too.
    OutputExists { path: PathBuf, more: usize },
    /// Every stack of a turntable failed.
    EveryStackFailed,
    /// The turntable's temporary folder couldn't be locked.
    TempFolderLocked { folder: PathBuf, why: String },
    /// `redraw` was given a folder with no render.json.
    NotARender(PathBuf),
    /// `redraw`'s output is inside the render's folder, in the Library.
    OutputInRender { output: PathBuf, render: PathBuf },
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let p = Path::display;
        match self {
            Self::SweepOutputIsFile(dir) => write!(
                f,
                "with a sweep, --output names the folder each run is written into, not a file \
                 like {}",
                p(dir)
            ),
            Self::DngNeedsRaw { output, first } => write!(
                f,
                "writing {} needs camera metadata to copy, but the first frame {} isn't camera \
                 RAW or DNG; write a TIFF, PNG or JPEG instead",
                p(output),
                p(first)
            ),
            Self::OutputIsInput { output, input } => write!(
                f,
                "{} is the input frame {}, and writing it would replace that frame; choose an \
                 output path outside the inputs",
                p(output),
                p(input)
            ),
            Self::SameOutput { first, second } => write!(
                f,
                "{} and {} are the same file; give each output its own path",
                p(first),
                p(second)
            ),
            Self::FileType { path, formats } => {
                write!(f, "{}: Deptheos doesn't write this file type; use {formats}", p(path))
            }
            Self::NoFolder { path, folder } => {
                write!(f, "{}: the folder {} doesn't exist", p(path), p(folder))
            }
            Self::NoFrames(folder) => write!(f, "{} has no frames to stack", p(folder)),
            Self::TooFewFrames(1) => write!(f, "1 frame given; a stack needs at least 2"),
            Self::TooFewFrames(n) => write!(f, "{n} frames given; a stack needs at least 2"),
            Self::NoFolderHasFrames => write!(f, "no folder has frames to stack"),
            Self::NotARender(dir) => {
                write!(f, "{} isn't a render: give a folder of the Library's Renders", p(dir))
            }
            Self::OutputInRender { output, render } => write!(
                f,
                "{} is inside the render {}, which is the Library's; write it elsewhere",
                p(output),
                p(render)
            ),
            Self::OutputInFrames { output, input } if output == input => write!(
                f,
                "--output {} is a folder of the frames, where the stacks would be taken for \
                 frames the next time it is stacked; choose a folder outside the inputs",
                p(output)
            ),
            Self::OutputInFrames { output, input } => write!(
                f,
                "--output {} is inside {}, a folder of the frames, where the stacks would be \
                 taken for frames the next time it is stacked; choose a folder outside the inputs",
                p(output),
                p(input)
            ),
            Self::OutputIsFile { path, holds } => {
                write!(f, "--output names a folder for the {holds}, but {} is a file", p(path))
            }
            Self::FolderNamedLikeFile(dir) => write!(
                f,
                "--output names the folder the aligned frames are written into, not a file like {}",
                p(dir)
            ),
            Self::SameName { first, second, output } => write!(
                f,
                "{} and {} would both be written as {}; align frames with different names, or \
                 align each folder on its own",
                p(first),
                p(second),
                p(output)
            ),
            Self::OutputExists { path, more: 0 } => write!(
                f,
                "{} already exists, and align doesn't replace files; remove it or choose another \
                 folder",
                p(path)
            ),
            Self::OutputExists { path, more } => write!(
                f,
                "{} and {more} more of the files to write already exist, and align doesn't \
                 replace files; remove them or choose another folder",
                p(path)
            ),
            Self::EveryStackFailed => write!(f, "every stack failed, so no sequence was written"),
            Self::TempFolderLocked { folder, why } => {
                write!(f, "couldn't lock the temporary folder {}: {why}", p(folder))
            }
        }
    }
}

impl From<deptheos_app::Error> for Error {
    fn from(e: deptheos_app::Error) -> Self {
        Self::Engine(e)
    }
}

impl From<CoreError> for Error {
    fn from(e: CoreError) -> Self {
        Self::Engine(e.into())
    }
}

impl From<StoreError> for Error {
    fn from(e: StoreError) -> Self {
        Self::Engine(e.into())
    }
}

impl From<Refusal> for Error {
    fn from(r: Refusal) -> Self {
        Self::Refused(r)
    }
}

/// How an error is told on stderr.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Told {
    /// Follows `error: ` (or a stack's label after it).
    pub(crate) message: String,
    /// For a fault: the text of the `details:` line.
    pub(crate) details: Option<String>,
}

impl Told {
    /// `error: [{lead} ]{message}`, then any `details:` line, on stderr.
    pub(crate) fn print(&self, lead: Option<&str>) {
        match lead {
            Some(lead) => eprintln!("error: {lead} {}", self.message),
            None => eprintln!("error: {}", self.message),
        }
        self.print_details();
    }

    /// The `details:` line, for a fault, on stderr.
    pub(crate) fn print_details(&self) {
        if let Some(details) = &self.details {
            eprintln!("details: {details}");
        }
    }
}

/// How the engine's `error` is told on the command line: its reason and
/// advice; for a fault, the fixed sentence and the error's own text.
pub(crate) fn tell_engine(error: &deptheos_app::Error, words: &Words) -> Told {
    let explained = error.explain(words);
    match explained.kind {
        ExplanationKind::Fault => Told { message: fault(words), details: Some(explained.details) },
        ExplanationKind::User | ExplanationKind::Canceled => {
            tracing::debug!("{}", explained.details);
            Told { message: explained.text(words), details: None }
        }
    }
}

impl Error {
    /// What to print, the engine's sentences worded in `words`.
    pub(crate) fn tell(&self, words: &Words) -> Told {
        match self {
            Self::Engine(e) => tell_engine(e, words),
            Self::Refused(r) => Told { message: r.to_string(), details: None },
            Self::Fault(details) => Told { message: fault(words), details: Some(details.clone()) },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_is_one_line_in_clap_form() {
        let all = [
            Refusal::SweepOutputIsFile("o.jpg".into()),
            Refusal::DngNeedsRaw { output: "o.dng".into(), first: "a.jpg".into() },
            Refusal::OutputIsInput { output: "a.jpg".into(), input: "a.jpg".into() },
            Refusal::SameOutput { first: "a.jpg".into(), second: "./a.jpg".into() },
            Refusal::FileType { path: "o.xyz".into(), formats: ".tif, .png or .jpg" },
            Refusal::NoFolder { path: "d/o.jpg".into(), folder: "d".into() },
            Refusal::NoFrames("bee".into()),
            Refusal::TooFewFrames(1),
            Refusal::NoFolderHasFrames,
            Refusal::OutputIsFile { path: "out".into(), holds: "stacks" },
            Refusal::OutputInFrames { output: "bee/out".into(), input: "bee".into() },
            Refusal::OutputInFrames { output: "bee".into(), input: "bee".into() },
            Refusal::FolderNamedLikeFile("out.tif".into()),
            Refusal::SameName {
                first: "a/1.jpg".into(),
                second: "b/1.jpg".into(),
                output: "o/1.tif".into(),
            },
            Refusal::OutputExists { path: "o/1.tif".into(), more: 0 },
            Refusal::OutputExists { path: "o/1.tif".into(), more: 3 },
            Refusal::EveryStackFailed,
            Refusal::TempFolderLocked { folder: "t".into(), why: "busy".into() },
        ];
        for refusal in all {
            let line = refusal.to_string();
            assert!(!line.contains('\n') && !line.ends_with('.'), "{line}");
            assert!(!line.starts_with(char::is_uppercase), "{line}");
            assert!(!line.contains("..=") && !line.contains("directory"), "{line}");
        }
    }

    #[test]
    fn a_count_reads_whole() {
        assert_eq!(Refusal::TooFewFrames(1).to_string(), "1 frame given; a stack needs at least 2");
        assert_eq!(
            Refusal::TooFewFrames(0).to_string(),
            "0 frames given; a stack needs at least 2"
        );
    }

    #[test]
    fn only_a_fault_has_details() {
        let words = Words::english(deptheos_app::Platform::CommandLine);
        const FAULT: &str = "Deptheos ran into an unexpected problem and stopped.";
        let refused = Error::from(Refusal::EveryStackFailed).tell(&words);
        assert_eq!(
            refused,
            Told {
                message: "every stack failed, so no sequence was written".into(),
                details: None
            }
        );

        let fault = Error::Fault("stack 3 was exported twice".into()).tell(&words);
        assert_eq!(fault.message, FAULT);
        assert_eq!(fault.details.as_deref(), Some("stack 3 was exported twice"));

        // The engine's own faults print the same way, their text kept.
        let engine =
            Error::from(deptheos_app::Error::InvalidState("lost the worker".into())).tell(&words);
        assert_eq!(engine.message, FAULT);
        assert!(engine.details.unwrap().contains("lost the worker"));

        // A user's error prints the engine's sentences, capitals and stops kept.
        let user = Error::from(deptheos_app::Error::NoFrames).tell(&words);
        assert!(user.message.starts_with("There are no frames to stack."), "{}", user.message);
        assert!(user.details.is_none());
    }
}
