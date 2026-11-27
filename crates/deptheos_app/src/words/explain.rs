//! The one place typed errors become the user's sentences, for every front
//! end (docs/terms-and-usage.md, section 4). An error's technical text goes
//! to the log and Copy Details ([`Explanation::details`]).
//!
//! Our enums are matched without catch-alls, so a new variant must be
//! worded here or sent to the fault arm. A fault (an error the user can't
//! cause or fix) gets one fixed sentence. The reason has no lead ("The
//! stack didn't finish."); the caller adds it. Nothing here logs or does
//! I/O.

use std::io::ErrorKind;
use std::path::Path;

use deptheos_aligner::{AlignmentError, Motion};
use deptheos_core::io::Refused;
use deptheos_core::{ConverterError, CoreError, Setting};
use deptheos_dng::{Cause as DngCause, DngError};
use deptheos_gpu::{FrameLimit, GpuError};
use deptheos_raw::RawError;

use crate::frames::names::file_label;
use crate::frames::sources::SourcesError;
use crate::words::units::{duration_text, size_text};
use crate::words::{Words, say};
use crate::{Error, FrameKind, RenderUse};
use crate::{StoreError, frames::source_thumbnail::ThumbnailError};

/// The reason every fault gives: "Deptheos ran into an unexpected problem
/// and stopped."
pub fn fault(words: &Words) -> String {
    say!(words, "fault")
}

/// What a person is told about an error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Explanation {
    pub kind: Kind,
    /// Why, in the user's terms: one or more full sentences. For a fault,
    /// [`fault`].
    pub reason: String,
    /// What to do, in the imperative; empty when nothing helps, and for a
    /// fault.
    pub advice: String,
    /// The technical text with its sources, for Copy Details and the log.
    pub details: String,
    /// What stood in a file's way, for a front end's own advice.
    pub cause: Cause,
}

/// What stood in the way of a file or folder, as the system said it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Cause {
    /// Anything else, or no file at all.
    #[default]
    Other,
    /// The system refused access; the advice is the front end's.
    NotAllowed,
    /// It isn't there.
    NotFound,
}

/// Whose the error is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Something the user can cause and act on.
    User,
    /// A bug or a state the code didn't expect.
    Fault,
    /// The user stopped the work.
    Canceled,
}

/// Which front end the words are for, where advice differs (choosing a
/// GPU, rerunning a command). A [`Words`] carries it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Desktop,
    Ios,
    CommandLine,
}

impl Explanation {
    /// `reason`, then `advice` when there is some, as sentences.
    pub fn text(&self, words: &Words) -> String {
        crate::words::sentences(words, [&self.reason, &self.advice])
    }
}

/// An error that can be explained to a person.
pub trait Explain: std::error::Error {
    /// The [`Explanation`], in `words`' language and for its platform.
    fn explain(&self, words: &Words) -> Explanation;
}

/// Why a frame was left out: the lower-case clause a notice puts after its
/// name, without a full stop. The same on every platform, as it is kept
/// with the render.
pub(crate) fn left_out_reason(error: &Error, words: &Words) -> String {
    let w = words;
    match error {
        Error::Core(e) => core_clause(e, w),
        Error::Alignment(e) => alignment_clause(e, w),
        Error::Dng(e) => dng_clause(e, w),
        Error::Store(StoreError::Core(e)) => core_clause(e, w),
        _ => any_clause(&said_session(error, w), w),
    }
}

// ---------------------------------------------------------------- the words

/// What an error says, before its details are added.
enum Said {
    User { reason: String, advice: String, cause: Cause },
    Fault,
    Canceled,
}

fn user(reason: impl Into<String>, advice: impl Into<String>) -> Said {
    Said::User { reason: reason.into(), advice: advice.into(), cause: Cause::Other }
}

/// [`user`], about a file `cause` stood in the way of.
fn user_file(reason: impl Into<String>, advice: impl Into<String>, cause: Cause) -> Said {
    Said::User { reason: reason.into(), advice: advice.into(), cause }
}

fn explanation(said: Said, error: &dyn std::error::Error, w: &Words) -> Explanation {
    let details = chain(error);
    match said {
        Said::User { reason, advice, cause } => {
            Explanation { kind: Kind::User, reason, advice, details, cause }
        }
        Said::Fault => Explanation {
            kind: Kind::Fault,
            reason: fault(w),
            advice: String::new(),
            details,
            cause: Cause::Other,
        },
        Said::Canceled => Explanation {
            kind: Kind::Canceled,
            reason: say!(w, "canceled"),
            advice: String::new(),
            details,
            cause: Cause::Other,
        },
    }
}

/// `error`'s text and each source's that it doesn't already include.
fn chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(e) = source {
        let more = e.to_string();
        if !text.contains(&more) {
            text.push_str(": ");
            text.push_str(&more);
        }
        source = e.source();
    }
    text
}

/// The clause for a frame left out by an error that has none of its own.
fn any_clause(said: &Said, w: &Words) -> String {
    match said {
        Said::User { .. } => say!(w, "clause-unstackable"),
        Said::Fault => say!(w, "clause-unstackable-fault"),
        Said::Canceled => say!(w, "clause-unstacked-stopped"),
    }
}

/// A name in straight double quotes.
pub(crate) fn quoted(name: &str, w: &Words) -> String {
    say!(w, "quoted", name = name)
}

/// A path's file name in straight double quotes.
fn quoted_file(path: &Path, w: &Words) -> String {
    quoted(&file_label(path), w)
}

fn dimensions((width, height): (u32, u32)) -> String {
    format!("{width} × {height}")
}

/// `"a", "b" and "c"`, or the first five and how many more.
fn names_list(paths: &[std::path::PathBuf], w: &Words) -> String {
    const SHOWN: usize = 5;
    let mut names: Vec<String> = paths.iter().take(SHOWN).map(|p| quoted_file(p, w)).collect();
    let more = paths.len().saturating_sub(SHOWN);
    if more > 0 {
        return say!(w, "names-more", names = crate::words::list(w, &names), more = more);
    }
    match names.pop() {
        None => String::new(),
        Some(last) if names.is_empty() => last,
        Some(last) => say!(w, "names-and", names = crate::words::list(w, &names), last = last),
    }
}

// ---------------------------------------------------------------- session

impl Explain for Error {
    fn explain(&self, words: &Words) -> Explanation {
        explanation(said_session(self, words), self, words)
    }
}

fn said_session(error: &Error, w: &Words) -> Said {
    match error {
        Error::Core(e) => said_core(e, w),
        Error::Alignment(e) => said_alignment(e, w),
        Error::Dng(e) => said_dng(e, w),
        Error::Store(e) => said_store(e, w),
        Error::Canceled => Said::Canceled,
        Error::TooManyLeftOut { count, total, first, reason } => {
            too_many_left_out(*count, *total, first, reason, w)
        }
        Error::NoFrames => user(say!(w, "no-frames"), say!(w, "no-frames-advice")),
        Error::FrameSize { file, size, first } => user(
            say!(
                w,
                "frame-size",
                file = quoted(file, w),
                size = dimensions(*size),
                first = dimensions(*first)
            ),
            say!(w, "same-size-advice"),
        ),
        Error::MixedKinds { file, kind, first } => {
            let (file, first) = (quoted(file, w), quoted(first, w));
            let reason = match kind {
                FrameKind::Raw => say!(w, "mixed-kinds-raw", file = file, first = first),
                FrameKind::Standard => say!(w, "mixed-kinds-standard", file = file, first = first),
            };
            user(reason, say!(w, "mixed-kinds-advice"))
        }
        Error::MixedRenders { purpose: RenderUse::Compare } => {
            user(say!(w, "mixed-renders-compare"), "")
        }
        Error::MixedRenders { purpose: RenderUse::Paint } => {
            user(say!(w, "mixed-renders-paint"), "")
        }
        Error::NoSharedFrame => user(say!(w, "no-shared-frame"), ""),
        Error::NotRaw => user(say!(w, "not-raw"), say!(w, "export-as-tiff")),
        Error::NeedsPyramid => user(say!(w, "needs-pyramid"), ""),
        Error::PaintUnread(unread) => {
            use crate::paint::store::Unread;
            match unread {
                Unread::Damaged => user(say!(w, "paint-unread-damaged"), ""),
                Unread::Unreachable => user(
                    say!(w, "paint-unread-unreachable"),
                    say!(w, "paint-unread-unreachable-advice"),
                ),
            }
        }
        Error::AdjustmentsUnread(unread) => {
            use crate::library::adjustments::Unread;
            match unread {
                Unread::Damaged => user(say!(w, "adjust-export-damaged"), ""),
                Unread::Unreachable => user(
                    say!(w, "adjust-export-unreachable"),
                    say!(w, "paint-unread-unreachable-advice"),
                ),
            }
        }
        Error::MissingFrames { stack, files } => {
            let names = names_list(files, w);
            let count = files.len();
            let reason = match stack {
                Some(stack) => say!(
                    w,
                    "missing-frames-of-stack",
                    count = count,
                    stack = quoted(stack, w),
                    names = names
                ),
                None => say!(w, "missing-frames", count = count, names = names),
            };
            user_file(reason, say!(w, "missing-frames-advice", count = count), Cause::NotFound)
        }
        Error::NoStacks => user(say!(w, "no-stacks"), ""),
        Error::NoCommonArea => user(say!(w, "no-common-area"), ""),
        Error::NoSpace { folder, needed, free } => user(
            say!(
                w,
                "no-space",
                needed = size_text(*needed, w),
                folder = quoted_file(folder, w),
                free = size_text(*free, w)
            ),
            say!(w, "free-space-advice"),
        ),
        Error::SampleNoSpace { needed, free } => user(
            say!(w, "sample-no-space", needed = size_text(*needed, w), free = size_text(*free, w)),
            say!(w, "sample-no-space-advice"),
        ),
        Error::NoConversionSpace { needed, free, .. } => user(
            say!(
                w,
                "no-conversion-space",
                needed = size_text(*needed, w),
                free = size_text(*free, w)
            ),
            say!(w, "conversion-space-advice"),
        ),
        Error::InvalidState(_) => Said::Fault,
    }
}

/// `count` of `total` frames left out, the first `first` for `reason` (a
/// clause).
fn too_many_left_out(count: usize, total: usize, first: &str, reason: &str, w: &Words) -> Said {
    let first = quoted(first, w);
    if total <= 1 {
        return user(say!(w, "left-out-only", first = first, reason = reason), "");
    }
    let stacked = total.saturating_sub(count);
    let lead = match stacked {
        0 => say!(w, "left-out-none-stacked", total = total),
        1 => say!(w, "left-out-one-stacked", total = total),
        _ => say!(w, "left-out-some", count = count, total = total),
    };
    let why = match count {
        1 => say!(w, "left-out-only", first = first, reason = reason),
        _ => say!(w, "left-out-first", first = first, reason = reason),
    };
    user(crate::words::sentences(w, [lead, why]), say!(w, "left-out-advice"))
}

// ---------------------------------------------------------------- core

impl Explain for CoreError {
    fn explain(&self, words: &Words) -> Explanation {
        explanation(said_core(self, words), self, words)
    }
}

/// Why a frame couldn't be read: its name if known, the clause, the advice.
struct Unread {
    name: Option<String>,
    clause: String,
    advice: String,
}

impl Unread {
    fn new(name: Option<String>, clause: String, advice: impl Into<String>) -> Self {
        Self { name, clause, advice: advice.into() }
    }

    fn said(self, w: &Words) -> Said {
        let subject = match self.name.as_deref() {
            Some(name) => quoted(name, w),
            None => say!(w, "a-frame"),
        };
        user(say!(w, "frame-clause", subject = subject, clause = self.clause), self.advice)
    }
}

/// A [`CoreError`] that is about reading one frame.
fn unread_core(error: &CoreError, w: &Words) -> Option<Unread> {
    let name = |path: &Path| Some(file_label(path));
    Some(match error {
        CoreError::Raw(e) => unread_raw(e, w)?,
        CoreError::Dng(DngError::ReadFailed { path, cause }) => dng_unread(path, cause, w),
        CoreError::Decode { path, .. } => Unread::new(name(path), say!(w, "clause-picture"), ""),
        CoreError::Truncated { path } => {
            Unread::new(name(path), say!(w, "clause-truncated"), say!(w, "truncated-advice"))
        }
        CoreError::Refused { path, what } => {
            let (clause, advice) = match what {
                Refused::Cmyk => (say!(w, "clause-cmyk"), say!(w, "rgb-advice")),
                Refused::Lab => (say!(w, "clause-lab"), say!(w, "rgb-advice")),
                Refused::Hdr => (say!(w, "clause-hdr"), say!(w, "hdr-advice")),
                Refused::TiffLayout => {
                    (say!(w, "clause-tiff-layout"), say!(w, "tiff-layout-advice"))
                }
            };
            Unread::new(name(path), clause, advice)
        }
        CoreError::NoMemory { path, width, height } => Unread::new(
            name(path),
            say!(w, "clause-no-memory", size = dimensions((*width, *height))),
            say!(w, "close-or-smaller"),
        ),
        CoreError::TooLarge { path, width, height, max_pixels } => Unread::new(
            name(path),
            say!(
                w,
                "clause-too-large",
                size = dimensions((*width, *height)),
                max = (max_pixels / 1_000_000).to_string()
            ),
            "",
        ),
        CoreError::DngChannels { path, .. } => {
            Unread::new(name(path), say!(w, "clause-dng-channels"), "")
        }
        CoreError::PixelLayout(_) => {
            Unread::new(None, say!(w, "clause-pixel-layout"), say!(w, "pixel-layout-advice"))
        }
        CoreError::EmptyImage => Unread::new(None, say!(w, "clause-empty"), ""),
        CoreError::Image(image::ImageError::Decoding(_) | image::ImageError::Unsupported(_)) => {
            Unread::new(None, say!(w, "clause-picture"), "")
        }
        CoreError::Image(image::ImageError::Limits(_)) => {
            Unread::new(None, say!(w, "clause-too-large-to-read"), "")
        }
        _ => return None,
    })
}

fn unread_raw(error: &RawError, w: &Words) -> Option<Unread> {
    let name = |path: &str| Some(file_label(Path::new(path)));
    match error {
        RawError::DecodeFailed { path, .. } => {
            Some(Unread::new(name(path), say!(w, "clause-raw-decode"), ""))
        }
        RawError::InvalidPath(_) | RawError::Canceled => None,
    }
}

/// A slider's name.
pub(crate) fn setting_name(setting: Setting, w: &Words) -> String {
    match setting {
        Setting::Smoothness => say!(w, "setting-smoothness"),
        Setting::Radius => say!(w, "setting-radius"),
        Setting::Smoothing => say!(w, "setting-smoothing"),
        Setting::BlendRange => say!(w, "setting-blend-range"),
        Setting::Denoise => say!(w, "setting-denoise"),
        Setting::Consistency => say!(w, "setting-consistency"),
    }
}

fn said_core(error: &CoreError, w: &Words) -> Said {
    match error {
        CoreError::Gpu(e) => said_gpu(e, w),
        CoreError::InvalidState(_) | CoreError::NoFrames => Said::Fault,
        CoreError::Canceled => Said::Canceled,
        CoreError::Image(e) => said_image(e, w),
        CoreError::Raw(RawError::Canceled) => Said::Canceled,
        CoreError::Raw(e) => unread_raw(e, w).map_or(Said::Fault, |u| u.said(w)),
        CoreError::Dng(e) => said_dng(e, w),
        CoreError::Io(e) => said_io(Named::Unknown, e, w),
        CoreError::Converter(e) => said_converter(e, w),
        CoreError::FrameSize { size, first } => user(
            say!(w, "core-frame-size", size = dimensions(*size), first = dimensions(*first)),
            say!(w, "same-size-advice"),
        ),
        CoreError::OutOfRange { setting, min, max, .. } => user(
            say!(
                w,
                "out-of-range",
                setting = setting_name(*setting, w),
                min = min.to_string(),
                max = max.to_string()
            ),
            "",
        ),
        CoreError::FrameTooSmall { width, height, min_side } => user(
            say!(
                w,
                "frame-too-small",
                size = dimensions((*width, *height)),
                min = min_side.to_string()
            ),
            say!(w, "choose-another-method"),
        ),
        CoreError::JpegTooLarge { width, height } => user(
            say!(w, "jpeg-too-large", size = dimensions((*width, *height))),
            say!(w, "export-as-tiff-or-png"),
        ),
        CoreError::TiffTooLarge => {
            user(say!(w, "tiff-too-large"), say!(w, "tiff-too-large-advice"))
        }
        CoreError::Encode { format, .. } => {
            user(say!(w, "encode-failed", format = format.to_string()), "")
        }
        CoreError::SymlinkLoop { path } => user(
            say!(w, "symlink-loop", file = quoted_file(path, w)),
            say!(w, "choose-another-folder"),
        ),
        CoreError::NotAFile { path } => user(
            say!(w, "not-a-file", path = quoted(&path.display().to_string(), w)),
            say!(w, "not-a-file-advice"),
        ),
        CoreError::Decode { .. }
        | CoreError::Truncated { .. }
        | CoreError::Refused { .. }
        | CoreError::NoMemory { .. }
        | CoreError::TooLarge { .. }
        | CoreError::DngChannels { .. }
        | CoreError::PixelLayout(_)
        | CoreError::EmptyImage => unread_core(error, w).map_or(Said::Fault, |u| u.said(w)),
    }
}

fn core_clause(error: &CoreError, w: &Words) -> String {
    if let Some(unread) = unread_core(error, w) {
        return unread.clause;
    }
    match error {
        CoreError::Converter(e) => converter_clause(e, w),
        CoreError::Io(e) => io_clause(e, w),
        CoreError::Image(image::ImageError::IoError(e)) => io_clause(e, w),
        CoreError::Dng(e) => dng_clause(e, w),
        e => any_clause(&said_core(e, w), w),
    }
}

fn said_image(error: &image::ImageError, w: &Words) -> Said {
    match error {
        image::ImageError::Decoding(_) | image::ImageError::Unsupported(_) => {
            user(say!(w, "image-unreadable"), "")
        }
        image::ImageError::Encoding(_) => user(say!(w, "image-unwritable"), ""),
        image::ImageError::Limits(_) => user(say!(w, "image-too-large"), ""),
        image::ImageError::IoError(e) => said_io(Named::Unknown, e, w),
        image::ImageError::Parameter(_) => Said::Fault,
    }
}

// ---------------------------------------------------------------- files

/// Which file an I/O error is about, as it is told.
enum Named<'a> {
    /// A file or folder the user knows, by its name.
    File(&'a str),
    /// One of the Library's own files, named by an id.
    Library,
    /// Not known.
    Unknown,
}

/// An I/O error, about the file `named`.
fn said_io(named: Named, error: &std::io::Error, w: &Words) -> Said {
    let not_found = || say!(w, "not-found-advice");
    match (error.kind(), named) {
        (ErrorKind::NotFound, Named::File(n)) => {
            user_file(say!(w, "io-not-found", file = quoted(n, w)), not_found(), Cause::NotFound)
        }
        (ErrorKind::NotFound, Named::Library) => {
            user_file(say!(w, "io-library-not-found"), not_found(), Cause::NotFound)
        }
        (ErrorKind::NotFound, Named::Unknown) => {
            user_file(say!(w, "io-not-found-any"), not_found(), Cause::NotFound)
        }
        (ErrorKind::PermissionDenied, Named::File(n)) => {
            user_file(say!(w, "io-denied", file = quoted(n, w)), "", Cause::NotAllowed)
        }
        (ErrorKind::PermissionDenied, Named::Library) => {
            user_file(say!(w, "io-library-denied"), "", Cause::NotAllowed)
        }
        (ErrorKind::PermissionDenied, Named::Unknown) => {
            user_file(say!(w, "io-denied-any"), "", Cause::NotAllowed)
        }
        (ErrorKind::StorageFull, Named::File(n)) => {
            user(say!(w, "io-full", file = quoted(n, w)), say!(w, "free-space-advice"))
        }
        (ErrorKind::StorageFull, Named::Library) => {
            user(say!(w, "io-library-full"), say!(w, "free-space-advice"))
        }
        (ErrorKind::StorageFull, Named::Unknown) => {
            user(say!(w, "io-full-any"), say!(w, "free-space-advice"))
        }
        (ErrorKind::ReadOnlyFilesystem, Named::File(n)) => {
            user(say!(w, "io-read-only", file = quoted(n, w)), say!(w, "choose-another-folder"))
        }
        (ErrorKind::ReadOnlyFilesystem, Named::Library) => {
            user(say!(w, "io-library-read-only"), say!(w, "choose-another-folder"))
        }
        (ErrorKind::ReadOnlyFilesystem, Named::Unknown) => {
            user(say!(w, "io-read-only-any"), say!(w, "choose-another-folder"))
        }
        (_, Named::File(n)) => user(say!(w, "io-other", file = quoted(n, w)), ""),
        (_, Named::Library) => user(say!(w, "io-library-other"), ""),
        (_, Named::Unknown) => user(say!(w, "io-other-any"), ""),
    }
}

/// Whether `path` is one of the Library's own files: named by an id, or
/// the list.
fn in_library(path: &Path) -> bool {
    let id_named = |name: &str| crate::library::ids::is_id(name.split('.').next().unwrap_or(name));
    path.file_name().is_some_and(|n| n == "list.json" || n == crate::frames::sources::SOURCES_FILE)
        || path.components().any(|c| c.as_os_str().to_str().is_some_and(id_named))
}

/// A file error with no file named: why, in the user's terms.
impl Explain for std::io::Error {
    fn explain(&self, words: &Words) -> Explanation {
        explanation(said_io(Named::Unknown, self, words), self, words)
    }
}

fn io_clause(error: &std::io::Error, w: &Words) -> String {
    match error.kind() {
        ErrorKind::NotFound => say!(w, "clause-not-found"),
        ErrorKind::PermissionDenied => say!(w, "clause-denied"),
        _ => say!(w, "clause-unreadable"),
    }
}

impl Explain for StoreError {
    fn explain(&self, words: &Words) -> Explanation {
        explanation(said_store(self, words), self, words)
    }
}

fn said_store(error: &StoreError, w: &Words) -> Said {
    // Named by ids, so the user is told only that it is the Library's.
    match error {
        StoreError::Io { path, source } if in_library(path) => said_io(Named::Library, source, w),
        StoreError::Io { path, source } => said_io(Named::File(&file_label(path)), source, w),
        // The Library's format was checked at open, so this is damage.
        StoreError::Json { .. } | StoreError::Damaged { .. } | StoreError::Schema { .. } => {
            user(say!(w, "store-damaged"), "")
        }
        StoreError::NewerLibrary { written_by, .. } => user(
            match written_by {
                Some(version) => say!(w, "library-newer", version = version.as_str()),
                None => say!(w, "library-newer-unknown"),
            },
            say!(w, "library-newer-advice"),
        ),
        StoreError::InUse { holder, .. } => match holder {
            Some(holder) if holder.is_here() => {
                user(say!(w, "library-in-use-here"), say!(w, "library-in-use-here-advice"))
            }
            Some(holder) if !holder.host.is_empty() => user(
                say!(
                    w,
                    "library-in-use-on",
                    version = holder.version.as_str(),
                    computer = holder.host.as_str()
                ),
                say!(w, "library-in-use-on-advice", computer = holder.host.as_str()),
            ),
            _ => user(say!(w, "library-in-use"), say!(w, "library-in-use-advice")),
        },
        StoreError::NotConnected { library } => user_file(
            say!(w, "library-not-connected", folder = quoted_file(library, w)),
            say!(w, "library-not-connected-advice"),
            Cause::NotFound,
        ),
        StoreError::Synced { library, service } => user(
            say!(w, "library-synced", folder = quoted_file(library, w), service = service.name()),
            say!(w, "library-synced-advice"),
        ),
        StoreError::NoLocks { .. } => {
            user(say!(w, "library-no-locks"), say!(w, "library-no-locks-advice"))
        }
        StoreError::Invalid(_) => Said::Fault,
        StoreError::Core(e) => said_core(e, w),
        StoreError::Dng(e) => said_dng(e, w),
        StoreError::Image(e) => said_image(e, w),
        StoreError::Sources(e) => said_sources(e, w),
    }
}

impl Explain for SourcesError {
    fn explain(&self, words: &Words) -> Explanation {
        explanation(said_sources(self, words), self, words)
    }
}

fn said_sources(error: &SourcesError, w: &Words) -> Said {
    match error {
        SourcesError::Store(e) => said_store(e, w),
        SourcesError::NotFolder(path) => {
            user(say!(w, "sources-not-folder", folder = quoted_file(path, w)), "")
        }
        SourcesError::NotFile(path) => {
            user(say!(w, "sources-not-file", file = quoted_file(path, w)), "")
        }
        SourcesError::NotUnicode(path) => {
            user(say!(w, "sources-not-unicode", file = quoted_file(path, w)), "")
        }
        SourcesError::UnknownRoot(_) => user(say!(w, "sources-unknown-root"), ""),
        SourcesError::Damaged { path, .. } => user(
            say!(w, "sources-damaged", file = quoted_file(path, w)),
            say!(w, "sources-damaged-advice", file = quoted_file(path, w)),
        ),
        SourcesError::NotCopied(frames) => {
            let paths: Vec<std::path::PathBuf> = frames.iter().map(Into::into).collect();
            let (count, names) = (frames.len(), names_list(&paths, w));
            user(say!(w, "sources-not-copied", count = count, names = names), "")
        }
        SourcesError::NotInside { .. }
        | SourcesError::NotSubfolder(_)
        | SourcesError::NoBookmark(_) => Said::Fault,
    }
}

impl Explain for ThumbnailError {
    fn explain(&self, words: &Words) -> Explanation {
        let w = words;
        let said = match self {
            ThumbnailError::Raw(e) => unread_raw(e, w).map_or(Said::Fault, |u| u.said(w)),
            ThumbnailError::Image(e) => said_image(e, w),
            ThumbnailError::Io(e) => said_io(Named::Unknown, e, w),
            ThumbnailError::Dng(e) => said_dng(e, w),
            ThumbnailError::Core(e) => said_core(e, w),
            ThumbnailError::Unreadable(_) => user(say!(w, "thumbnail-failed"), ""),
        };
        explanation(said, self, w)
    }
}

// ---------------------------------------------------------------- RAW and DNG

impl Explain for RawError {
    fn explain(&self, words: &Words) -> Explanation {
        let said = unread_raw(self, words).map_or(Said::Fault, |u| u.said(words));
        explanation(said, self, words)
    }
}

impl Explain for DngError {
    fn explain(&self, words: &Words) -> Explanation {
        explanation(said_dng(self, words), self, words)
    }
}

/// A DNG that couldn't be read, said by why: the SDK's [`DngCause`].
fn dng_unread(path: &str, cause: &DngCause, w: &Words) -> Unread {
    let (clause, advice) = match cause {
        DngCause::Damaged => (say!(w, "clause-dng-damaged"), String::new()),
        DngCause::Unsupported => (say!(w, "clause-dng-not-supported"), String::new()),
        DngCause::OutOfMemory => (say!(w, "clause-dng-no-memory"), say!(w, "close-or-smaller")),
        DngCause::TooLarge => (
            say!(
                w,
                "clause-dng-too-large",
                max = (deptheos_dng::MAX_READ_PIXELS / 1_000_000).to_string()
            ),
            String::new(),
        ),
        DngCause::Other { .. } => (say!(w, "clause-dng-read"), String::new()),
    };
    Unread::new(Some(file_label(Path::new(path))), clause, advice)
}

fn said_dng(error: &DngError, w: &Words) -> Said {
    match error {
        DngError::ReadFailed { path, cause } => dng_unread(path, cause, w).said(w),
        e if e.cause() == Some(&DngCause::OutOfMemory) => {
            user(say!(w, "dng-no-memory"), say!(w, "close-or-smaller"))
        }
        // Only the Library's renders are JPEG XL.
        DngError::JxlFailed { cause: DngCause::Damaged, .. } => user(say!(w, "store-damaged"), ""),
        DngError::WriteFailed { path, .. } => user(
            say!(w, "dng-write", file = quoted_file(Path::new(path), w)),
            say!(w, "dng-write-advice"),
        ),
        DngError::MetadataFailed(_) => user(say!(w, "dng-metadata"), say!(w, "export-as-tiff")),
        DngError::Canceled => Said::Canceled,
        DngError::JxlWrite(e) => said_io(Named::Unknown, e, w),
        DngError::InvalidPath(_)
        | DngError::RenderFailed(_)
        | DngError::ParamsFailed(_)
        | DngError::PixelCountMismatch { .. }
        | DngError::JxlFailed { .. } => Said::Fault,
    }
}

fn dng_clause(error: &DngError, w: &Words) -> String {
    match error {
        DngError::ReadFailed { path, cause } => dng_unread(path, cause, w).clause,
        e => any_clause(&said_dng(e, w), w),
    }
}

impl Explain for ConverterError {
    fn explain(&self, words: &Words) -> Explanation {
        explanation(said_converter(self, words), self, words)
    }
}

fn said_converter(error: &ConverterError, w: &Words) -> Said {
    match error {
        ConverterError::NotFound(exe) => user(
            say!(w, "converter-not-found", path = quoted(&exe.display().to_string(), w)),
            say!(w, "converter-not-found-advice"),
        ),
        ConverterError::NotOnThisPlatform => {
            user(say!(w, "converter-not-here"), say!(w, "converter-libraw-advice"))
        }
        ConverterError::Start { .. } => {
            user(say!(w, "converter-start"), say!(w, "converter-start-advice"))
        }
        ConverterError::Stopped(_) => Said::Canceled,
        ConverterError::TimedOut { input, seconds } => user(
            say!(
                w,
                "converter-timed-out",
                file = quoted_file(input, w),
                time = duration_text(*seconds, w)
            ),
            "",
        ),
        ConverterError::Failed { input, .. } => {
            user(say!(w, "converter-failed", file = quoted_file(input, w)), "")
        }
        ConverterError::NoOutput(input) => user(
            say!(w, "converter-no-output", file = quoted_file(input, w)),
            say!(w, "converter-no-output-advice"),
        ),
    }
}

fn converter_clause(error: &ConverterError, w: &Words) -> String {
    match error {
        ConverterError::NotFound(exe) => {
            say!(w, "clause-converter-not-found", path = quoted(&exe.display().to_string(), w))
        }
        ConverterError::NotOnThisPlatform => say!(w, "clause-converter-not-here"),
        ConverterError::Start { .. } => say!(w, "clause-converter-start"),
        ConverterError::Stopped(_) => say!(w, "clause-converter-stopped"),
        ConverterError::TimedOut { seconds, .. } => {
            say!(w, "clause-converter-timed-out", time = duration_text(*seconds, w))
        }
        ConverterError::Failed { .. } => say!(w, "clause-converter-failed"),
        ConverterError::NoOutput(_) => say!(w, "clause-converter-no-output"),
    }
}

// ---------------------------------------------------------------- GPU

impl Explain for GpuError {
    fn explain(&self, words: &Words) -> Explanation {
        explanation(said_gpu(self, words), self, words)
    }
}

/// How to start over after the GPU failed.
fn start_again(w: &Words) -> String {
    match w.platform() {
        Platform::Desktop | Platform::Ios => say!(w, "start-again-app"),
        Platform::CommandLine => say!(w, "start-again-command"),
    }
}

fn said_gpu(error: &GpuError, w: &Words) -> Said {
    let platform = w.platform();
    match error {
        GpuError::NoAdapter => user(say!(w, "gpu-none"), ""),
        GpuError::DeviceCreationFailed(_) => user(
            say!(w, "gpu-start"),
            match platform {
                Platform::Desktop => say!(w, "gpu-start-advice-desktop"),
                Platform::Ios | Platform::CommandLine => start_again(w),
            },
        ),
        // A wait that timed out is a GPU that stopped responding.
        GpuError::DeviceLost(_) | GpuError::TimedOut(_) => {
            user(say!(w, "gpu-lost"), start_again(w))
        }
        GpuError::OutOfMemory => user(say!(w, "gpu-memory"), say!(w, "close-or-smaller")),
        GpuError::LowMemory { .. } => {
            user(say!(w, "gpu-low-memory"), say!(w, "gpu-too-large-advice"))
        }
        GpuError::OverBudget { width, height, needed, budget } => {
            let (size, needed, budget) =
                (dimensions((*width, *height)), size_text(*needed, w), size_text(*budget, w));
            match platform {
                Platform::Ios => user(
                    say!(w, "gpu-budget-ios", size = size, needed = needed, budget = budget),
                    say!(w, "gpu-too-large-advice"),
                ),
                Platform::Desktop | Platform::CommandLine => user(
                    say!(w, "gpu-budget", size = size, needed = needed, budget = budget),
                    say!(w, "close-or-smaller"),
                ),
            }
        }
        GpuError::FrameTooLarge { width, height, limit } => {
            let size = dimensions((*width, *height));
            let reason = match limit {
                FrameLimit::Side { max } => {
                    say!(w, "gpu-side", size = size, max = max.to_string())
                }
                FrameLimit::Buffer { .. } => say!(w, "gpu-buffer", size = size),
            };
            let advice = match platform {
                Platform::Desktop => say!(w, "gpu-too-large-advice-desktop"),
                Platform::Ios | Platform::CommandLine => say!(w, "gpu-too-large-advice"),
            };
            user(reason, advice)
        }
        GpuError::Internal(_)
        | GpuError::BufferMappingFailed(_)
        | GpuError::ShortReadback { .. }
        | GpuError::Unsupported(_) => Said::Fault,
    }
}

// ---------------------------------------------------------------- alignment

impl Explain for AlignmentError {
    fn explain(&self, words: &Words) -> Explanation {
        explanation(said_alignment(self, words), self, words)
    }
}

/// Why the aligner rejected a frame: the clause, the notice's hint after a
/// semicolon (if any), and the advice; `None` for a fault.
struct Misaligned {
    clause: String,
    hint: Option<String>,
    advice: String,
}

/// One decimal, as the clauses give a percent or an angle.
fn one_decimal(value: f64) -> String {
    format!("{value:.1}")
}

/// `value` (over `max`) with enough decimals not to read as the limit:
/// 5.04 over 5% is "5.04", never "5.0".
fn over_limit(value: f64, max: f64) -> String {
    (1..=6)
        .map(|digits| format!("{value:.digits$}"))
        .find(|text| text.parse::<f64>().is_ok_and(|v| v > max))
        .unwrap_or_else(|| value.to_string())
}

/// `value` beside a limit `max`: [`over_limit`] when over, else one
/// decimal.
fn beside_limit(value: f64, max: f64) -> String {
    if value > max { over_limit(value, max) } else { one_decimal(value) }
}

fn misaligned(error: &AlignmentError, w: &Words) -> Option<Misaligned> {
    let ecc = |clause: String| Misaligned {
        clause,
        hint: Some(say!(w, "hint-try-grid")),
        advice: say!(w, "try-grid-advice"),
    };
    Some(match error {
        AlignmentError::Featureless { .. } => Misaligned {
            clause: say!(w, "clause-featureless"),
            hint: None,
            advice: say!(w, "exclude-it"),
        },
        AlignmentError::LowCorrelation { .. } => ecc(say!(w, "clause-low-correlation")),
        AlignmentError::LittleOverlap { percent } => {
            ecc(say!(w, "clause-little-overlap", percent = format!("{percent:.0}")))
        }
        AlignmentError::Uncorrelated => ecc(say!(w, "clause-uncorrelated")),
        AlignmentError::Implausible { motion, .. } => ecc(match *motion {
            Motion::NotFinite => return None,
            Motion::Flips => say!(w, "clause-flips"),
            Motion::Shift { dx, dy, max } => say!(
                w,
                "clause-shift",
                dx = beside_limit(dx, max),
                dy = beside_limit(dy, max),
                max = max.to_string()
            ),
            Motion::Scale { percent, max } => {
                say!(w, "clause-scale", percent = over_limit(percent, max), max = max.to_string())
            }
            Motion::Shear { percent, max } => {
                say!(w, "clause-shear", percent = over_limit(percent, max), max = max.to_string())
            }
            Motion::Rotation { degrees, max } => say!(
                w,
                "clause-rotation",
                degrees = over_limit(degrees, max),
                max = max.to_string()
            ),
        }),
        AlignmentError::NoOverlap => Misaligned {
            clause: say!(w, "clause-no-overlap"),
            hint: Some(say!(w, "hint-wider-grid")),
            advice: say!(w, "wider-grid-advice"),
        },
        AlignmentError::NoMatch { .. } => Misaligned {
            clause: say!(w, "clause-no-match"),
            hint: None,
            advice: say!(w, "exclude-the-frame"),
        },
        AlignmentError::EccFailed(_)
        | AlignmentError::ProcessingFailed(_)
        | AlignmentError::ResizeFailed(_) => return None,
    })
}

fn said_alignment(error: &AlignmentError, w: &Words) -> Said {
    match misaligned(error, w) {
        Some(m) => {
            let subject = match error {
                AlignmentError::Featureless { .. } => say!(w, "the-first-frame"),
                _ => say!(w, "a-frame"),
            };
            user(say!(w, "frame-clause", subject = subject, clause = m.clause), m.advice)
        }
        None => Said::Fault,
    }
}

fn alignment_clause(error: &AlignmentError, w: &Words) -> String {
    match misaligned(error, w) {
        Some(Misaligned { clause, hint: None, .. }) => clause,
        Some(Misaligned { clause, hint: Some(hint), .. }) => {
            say!(w, "clause-with-hint", clause = clause, hint = hint)
        }
        // The numbers that broke down are in the log.
        None => say!(w, "clause-unaligned"),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// The clause a picture that can't be read gives.
    const PICTURE: &str = "couldn't be read as an image file";

    fn english(platform: Platform) -> Words {
        Words::english(platform)
    }

    fn io(kind: ErrorKind) -> std::io::Error {
        std::io::Error::new(kind, "os says no")
    }

    /// One of every error a person can cause, with its expected kind.
    fn user_errors() -> Vec<(&'static str, Error)> {
        let core = |e: CoreError| Error::Core(e);
        let gpu = |e: GpuError| Error::Core(CoreError::Gpu(e));
        let align = |e: AlignmentError| Error::Alignment(e);
        let conv = |e: ConverterError| Error::Core(CoreError::Converter(e));
        let store = |e: StoreError| Error::Store(e);
        let path = |p: &str| PathBuf::from(p);
        vec![
            (
                "left out, one of one",
                Error::TooManyLeftOut {
                    count: 1,
                    total: 1,
                    first: "a.jpg".into(),
                    reason: PICTURE.into(),
                },
            ),
            (
                "left out, one of two",
                Error::TooManyLeftOut {
                    count: 1,
                    total: 2,
                    first: "a.jpg".into(),
                    reason: PICTURE.into(),
                },
            ),
            (
                "left out, several",
                Error::TooManyLeftOut {
                    count: 3,
                    total: 10,
                    first: "a.CR3".into(),
                    reason:
                        "doesn't look enough like the frame before it to line up; try Grid search"
                            .into(),
                },
            ),
            (
                "left out, all",
                Error::TooManyLeftOut {
                    count: 4,
                    total: 4,
                    first: "a.jpg".into(),
                    reason: PICTURE.into(),
                },
            ),
            ("no frames", Error::NoFrames),
            (
                "frame size",
                Error::FrameSize {
                    file: "IMG_0412.CR3".into(),
                    size: (6000, 4000),
                    first: (8192, 5464),
                },
            ),
            (
                "mixed kinds",
                Error::MixedKinds {
                    file: "b.jpg".into(),
                    kind: FrameKind::Standard,
                    first: "a.CR3".into(),
                },
            ),
            ("compare mixed", Error::MixedRenders { purpose: RenderUse::Compare }),
            ("paint mixed", Error::MixedRenders { purpose: RenderUse::Paint }),
            ("no shared frame", Error::NoSharedFrame),
            ("not raw", Error::NotRaw),
            ("needs pyramid", Error::NeedsPyramid),
            ("paint damaged", Error::PaintUnread(crate::paint::store::Unread::Damaged)),
            ("paint unreachable", Error::PaintUnread(crate::paint::store::Unread::Unreachable)),
            (
                "adjustments damaged",
                Error::AdjustmentsUnread(crate::library::adjustments::Unread::Damaged),
            ),
            (
                "adjustments unreachable",
                Error::AdjustmentsUnread(crate::library::adjustments::Unread::Unreachable),
            ),
            ("missing, one", Error::MissingFrames { stack: None, files: vec![path("/x/a.CR3")] }),
            (
                "missing, three",
                Error::MissingFrames {
                    stack: Some("bee".into()),
                    files: vec![path("/x/a.CR3"), path("/x/b.CR3"), path("/x/c.CR3")],
                },
            ),
            (
                "missing, many",
                Error::MissingFrames {
                    stack: None,
                    files: (0..8).map(|i| path(&format!("/x/{i}.CR3"))).collect(),
                },
            ),
            ("no stacks", Error::NoStacks),
            ("no common area", Error::NoCommonArea),
            (
                "no space",
                Error::NoSpace {
                    folder: path("/Users/me/Movies"),
                    needed: 67_000_000_000,
                    free: 12_000_000_000,
                },
            ),
            ("sample no space", Error::SampleNoSpace { needed: 64_000_000, free: 12_000_000 }),
            (
                "no conversion space",
                Error::NoConversionSpace {
                    folder: path("/Users/me/Library/Caches/Deptheos"),
                    needed: 80_000_000_000,
                    free: 12_000_000_000,
                },
            ),
            ("gpu none", gpu(GpuError::NoAdapter)),
            ("gpu lost", gpu(GpuError::DeviceLost("hung".into()))),
            ("gpu timed out", gpu(GpuError::TimedOut(deptheos_gpu::WAIT_LIMIT))),
            ("gpu memory", gpu(GpuError::OutOfMemory)),
            ("gpu low memory", gpu(GpuError::LowMemory { left: 200_000_000 })),
            (
                "gpu budget",
                gpu(GpuError::OverBudget {
                    width: 8256,
                    height: 5504,
                    needed: 9_100_000_000,
                    budget: 8_000_000_000,
                }),
            ),
            (
                "gpu side",
                gpu(GpuError::FrameTooLarge {
                    width: 20000,
                    height: 15000,
                    limit: FrameLimit::Side { max: 16384 },
                }),
            ),
            (
                "gpu buffer",
                gpu(GpuError::FrameTooLarge {
                    width: 8192,
                    height: 5464,
                    limit: FrameLimit::Buffer { what: "upload", bytes: 1 << 31, max: 1 << 30 },
                }),
            ),
            ("featureless", align(AlignmentError::Featureless { std: 0.0 })),
            (
                "low correlation",
                align(AlignmentError::LowCorrelation {
                    correlation: 0.31,
                    min: 0.6,
                    level: (512, 341),
                    by_level: "[0.310]".into(),
                }),
            ),
            ("little overlap", align(AlignmentError::LittleOverlap { percent: 12.0 })),
            ("uncorrelated", align(AlignmentError::Uncorrelated)),
            (
                "shift",
                align(AlignmentError::Implausible {
                    motion: Motion::Shift { dx: 12.34, dy: 4.0, max: 5.0 },
                    by_level: "[0.9]".into(),
                }),
            ),
            (
                "scale",
                align(AlignmentError::Implausible {
                    motion: Motion::Scale { percent: 6.1, max: 5.0 },
                    by_level: "[0.9]".into(),
                }),
            ),
            (
                "shear",
                align(AlignmentError::Implausible {
                    motion: Motion::Shear { percent: 6.1, max: 5.0 },
                    by_level: "[0.9]".into(),
                }),
            ),
            (
                "rotation",
                align(AlignmentError::Implausible {
                    motion: Motion::Rotation { degrees: 7.25, max: 5.0 },
                    by_level: "[0.9]".into(),
                }),
            ),
            (
                "flips",
                align(AlignmentError::Implausible {
                    motion: Motion::Flips,
                    by_level: "[0.9]".into(),
                }),
            ),
            ("no overlap", align(AlignmentError::NoOverlap)),
            ("no match", align(AlignmentError::NoMatch { difference: 80.0, max: 50.0 })),
            ("core frame size", core(CoreError::FrameSize { size: (1, 2), first: (3, 4) })),
            (
                "smoothness",
                core(CoreError::OutOfRange {
                    setting: Setting::Smoothness,
                    value: 12.0,
                    min: 1.0,
                    max: 10.0,
                }),
            ),
            (
                "denoise",
                core(CoreError::OutOfRange {
                    setting: Setting::Denoise,
                    value: f32::NAN,
                    min: 0.0,
                    max: 10.0,
                }),
            ),
            ("too small", core(CoreError::FrameTooSmall { width: 20, height: 20, min_side: 64 })),
            ("pixel layout", core(CoreError::PixelLayout("La8".into()))),
            ("empty", core(CoreError::EmptyImage)),
            ("decode", core(CoreError::Decode { path: path("/x/a.heic"), message: "bad".into() })),
            ("truncated", core(CoreError::Truncated { path: path("/x/a.jpg") })),
            ("cmyk", core(CoreError::Refused { path: path("/x/a.tif"), what: Refused::Cmyk })),
            ("lab", core(CoreError::Refused { path: path("/x/a.tif"), what: Refused::Lab })),
            ("hdr", core(CoreError::Refused { path: path("/x/a.png"), what: Refused::Hdr })),
            (
                "tiff layout",
                core(CoreError::Refused { path: path("/x/a.tif"), what: Refused::TiffLayout }),
            ),
            (
                "no memory",
                core(CoreError::NoMemory { path: path("/x/a.heic"), width: 8192, height: 5464 }),
            ),
            (
                "too large",
                core(CoreError::TooLarge {
                    path: path("/x/a.heic"),
                    width: 30000,
                    height: 20000,
                    max_pixels: 1 << 29,
                }),
            ),
            ("dng channels", core(CoreError::DngChannels { path: path("/x/a.dng"), channels: 1 })),
            ("jpeg too large", core(CoreError::JpegTooLarge { width: 70000, height: 3000 })),
            ("tiff too large", core(CoreError::TiffTooLarge)),
            ("encode", core(CoreError::Encode { format: "PNG", message: "eh".into() })),
            ("symlinks", core(CoreError::SymlinkLoop { path: path("/x/out.tif") })),
            ("not a file", core(CoreError::NotAFile { path: path("/") })),
            ("io", core(CoreError::Io(io(ErrorKind::Other)))),
            ("io full", core(CoreError::Io(io(ErrorKind::StorageFull)))),
            (
                "raw decode",
                core(CoreError::Raw(RawError::DecodeFailed {
                    path: "/x/a.CR3".into(),
                    reason: "LibRaw -4".into(),
                })),
            ),
            (
                "dng read",
                core(CoreError::Dng(dng_read(DngCause::Other { code: 0, message: "".into() }))),
            ),
            ("dng damaged", core(CoreError::Dng(dng_read(DngCause::Damaged)))),
            ("dng not supported", core(CoreError::Dng(dng_read(DngCause::Unsupported)))),
            ("dng no memory", Error::Dng(dng_read(DngCause::OutOfMemory))),
            ("dng too large", core(CoreError::Dng(dng_read(DngCause::TooLarge)))),
            (
                "dng write",
                Error::Dng(DngError::WriteFailed {
                    path: "/x/out.dng".into(),
                    cause: DngCause::Other { code: 100_000, message: "eh".into() },
                }),
            ),
            ("dng metadata", Error::Dng(DngError::MetadataFailed(DngCause::Damaged))),
            ("render no memory", Error::Dng(DngError::RenderFailed(DngCause::OutOfMemory))),
            (
                "jxl damaged",
                store(StoreError::Dng(DngError::JxlFailed {
                    what: "decode",
                    cause: DngCause::Damaged,
                })),
            ),
            (
                "converter missing",
                conv(ConverterError::NotFound(path("/Applications/Adobe DNG Converter.app"))),
            ),
            ("converter platform", conv(ConverterError::NotOnThisPlatform)),
            (
                "converter start",
                conv(ConverterError::Start {
                    path: path("/x/conv"),
                    source: io(ErrorKind::PermissionDenied),
                }),
            ),
            (
                "converter slow",
                conv(ConverterError::TimedOut { input: path("/x/a.CR3"), seconds: 300 }),
            ),
            (
                "converter failed",
                conv(ConverterError::Failed {
                    input: path("/x/a.CR3"),
                    message: "exit status: 1".into(),
                }),
            ),
            ("converter nothing", conv(ConverterError::NoOutput(path("/x/a.CR3")))),
            ("image", core(CoreError::Image(image::ImageError::IoError(io(ErrorKind::NotFound))))),
            (
                "store io",
                store(StoreError::Io {
                    path: path("/lib/stack.json"),
                    source: io(ErrorKind::PermissionDenied),
                }),
            ),
            (
                "store read-only",
                store(StoreError::Io {
                    path: path("/lib/stack.json"),
                    source: io(ErrorKind::ReadOnlyFilesystem),
                }),
            ),
            ("store damaged", store(StoreError::Damaged { path: path("/lib/stack.json") })),
            (
                "store of another version",
                store(StoreError::Schema { path: path("/lib/stack.json"), found: 9, expected: 3 }),
            ),
            (
                "library newer",
                store(StoreError::NewerLibrary {
                    library: path("/lib"),
                    written_by: Some("0.6.0".into()),
                }),
            ),
            (
                "library newer, by an unknown version",
                store(StoreError::NewerLibrary { library: path("/lib"), written_by: None }),
            ),
            (
                "library in use here",
                store(StoreError::InUse {
                    library: path("/lib"),
                    holder: Some(crate::library::history::Holder::this_app()),
                }),
            ),
            (
                "library in use elsewhere",
                store(StoreError::InUse {
                    library: path("/lib"),
                    holder: Some(crate::library::history::Holder {
                        host: "Studio Mac".into(),
                        version: "0.3.0".into(),
                    }),
                }),
            ),
            ("library in use", store(StoreError::InUse { library: path("/lib"), holder: None })),
            (
                "library synced",
                store(StoreError::Synced {
                    library: path("/Users/me/Dropbox/Deptheos"),
                    service: crate::library::history::SyncService::Dropbox,
                }),
            ),
            (
                "library not connected",
                store(StoreError::NotConnected { library: path("/Volumes/Photos/Deptheos") }),
            ),
            (
                "library no locks",
                store(StoreError::NoLocks {
                    library: path("/lib"),
                    source: io(ErrorKind::Unsupported),
                }),
            ),
            ("sources not a folder", store(SourcesError::NotFolder(path("/x/Bee")).into())),
            ("sources not a file", store(SourcesError::NotFile(path("/x/Bee/a.tif")).into())),
            ("sources not unicode", store(SourcesError::NotUnicode(path("/x/Bee")).into())),
            (
                "sources not copied",
                store(SourcesError::NotCopied(vec!["a.tif".into(), "sub/b.tif".into()]).into()),
            ),
            (
                "sources unknown root",
                store(
                    SourcesError::UnknownRoot(
                        crate::RootId::parse("0190a1b2c3d47e8f9a0b1c2d3e4f5a6b").unwrap(),
                    )
                    .into(),
                ),
            ),
        ]
    }

    /// A measure just over its limit never rounds to the limit.
    #[test]
    fn a_measure_over_its_limit_never_reads_as_the_limit() {
        assert_eq!(over_limit(5.04, 5.0), "5.04");
        assert_eq!(over_limit(5.0004, 5.0), "5.0004");
        assert_eq!(over_limit(5.96, 5.0), "6.0");
        assert_eq!(over_limit(12.34, 5.0), "12.3");
        assert_eq!(over_limit(2.5000001, 2.5), "2.5000001");
        assert_eq!(beside_limit(4.98, 5.0), "5.0", "the axis under the limit");

        let w = english(Platform::Desktop);
        let said = |motion| {
            Error::Alignment(AlignmentError::Implausible { motion, by_level: String::new() })
                .explain(&w)
                .reason
        };
        let scale = said(Motion::Scale { percent: 5.03, max: 5.0 });
        assert!(scale.contains("by 5.03%") && scale.contains("the 5% allowed"), "{scale}");
        let shift = said(Motion::Shift { dx: 1.0, dy: 10.02, max: 10.0 });
        assert!(shift.contains("1.0% across and 10.02% up or down"), "{shift}");
        let turn = said(Motion::Rotation { degrees: 3.01, max: 3.0 });
        assert!(turn.contains("turn 3.01°"), "{turn}");
    }

    fn faults() -> Vec<Error> {
        vec![
            Error::InvalidState("no result texture".into()),
            Error::Core(CoreError::NoFrames),
            Error::Core(CoreError::Gpu(GpuError::Internal("validation".into()))),
            Error::Alignment(AlignmentError::EccFailed("Hessian matrix is singular".into())),
            Error::Alignment(AlignmentError::Implausible {
                motion: Motion::NotFinite,
                by_level: "[NaN]".into(),
            }),
            Error::Dng(DngError::JxlFailed {
                what: "decode",
                cause: DngCause::Other { code: 0, message: "".into() },
            }),
            Error::Store(StoreError::Invalid("short map".into())),
        ]
    }

    /// `\d+x\d+`, a dimension written the way the glossary rules out.
    fn has_bare_dimensions(text: &str) -> bool {
        let b = text.as_bytes();
        (1..b.len().saturating_sub(1))
            .any(|i| b[i] == b'x' && b[i - 1].is_ascii_digit() && b[i + 1].is_ascii_digit())
    }

    fn check_sentences(name: &str, text: &str, may_be_empty: bool) {
        if text.is_empty() {
            assert!(may_be_empty, "{name}: empty");
            return;
        }
        assert!(text.ends_with('.'), "{name}: no full stop: {text}");
        for banned in ["failed:", "Could not", "Failed to", "..=", "{", "Error:"] {
            assert!(!text.contains(banned), "{name}: {banned:?} in {text}");
        }
        assert!(!has_bare_dimensions(text), "{name}: bare dimensions in {text}");
    }

    #[test]
    fn every_user_error_is_worded_as_sentences() {
        for platform in [Platform::Desktop, Platform::Ios, Platform::CommandLine] {
            let words = english(platform);
            for (name, error) in user_errors() {
                let e = error.explain(&words);
                assert_eq!(e.kind, Kind::User, "{name}: {e:?}");
                // A capital, a quoted file name, or a count's numeral
                // ("1 frame can't be found").
                let first = e.reason.chars().next().expect("a reason");
                assert!(
                    first.is_uppercase() || first == '"' || first.is_ascii_digit(),
                    "{name}: {}",
                    e.reason
                );
                check_sentences(name, &e.reason, false);
                check_sentences(name, &e.advice, true);
                assert!(!e.details.is_empty(), "{name}: no details");
                if platform != Platform::Desktop {
                    let text = e.text(&words);
                    assert!(!text.contains("Settings ▸"), "{name}: {text}");
                }
            }
        }
    }

    #[test]
    fn faults_give_the_fixed_sentence() {
        let words = english(Platform::Desktop);
        for error in faults() {
            let e = error.explain(&words);
            assert_eq!(e.kind, Kind::Fault, "{error}");
            assert_eq!(e.reason, "Deptheos ran into an unexpected problem and stopped.");
            assert_eq!(e.reason, fault(&words));
            assert!(e.advice.is_empty());
            assert_eq!(e.details, error.to_string());
        }
    }

    #[test]
    fn a_cancel_is_canceled() {
        let e = Error::Canceled.explain(&english(Platform::Ios));
        assert_eq!(e.kind, Kind::Canceled);
        assert!(e.advice.is_empty());
        let stopped =
            Error::Core(CoreError::Converter(ConverterError::Stopped(PathBuf::from("a.CR3"))));
        assert_eq!(stopped.explain(&english(Platform::Desktop)).kind, Kind::Canceled);
    }

    #[test]
    fn counts_have_a_singular_and_a_plural() {
        let words = english(Platform::Desktop);
        let reason = |e: Error| e.explain(&words).reason;
        let missing = |n: usize| Error::MissingFrames {
            stack: None,
            files: (0..n).map(|i| PathBuf::from(format!("/x/{i}.CR3"))).collect(),
        };
        assert_eq!(reason(missing(1)), "1 frame can't be found: \"0.CR3\".");
        assert_eq!(reason(missing(2)), "2 frames can't be found: \"0.CR3\" and \"1.CR3\".");
        assert_eq!(
            reason(missing(7)),
            "7 frames can't be found: \"0.CR3\", \"1.CR3\", \"2.CR3\", \"3.CR3\", \"4.CR3\" \
             and 2 more."
        );
        let left_out = |count, total| Error::TooManyLeftOut {
            count,
            total,
            first: "a.jpg".into(),
            reason: PICTURE.into(),
        };
        assert_eq!(reason(left_out(1, 1)), "\"a.jpg\" couldn't be read as an image file.");
        assert_eq!(
            reason(left_out(1, 2)),
            "Only 1 of the 2 frames could be stacked. \"a.jpg\" couldn't be read as an image file."
        );
        assert_eq!(
            reason(left_out(2, 3)),
            "Only 1 of the 3 frames could be stacked. The first, \"a.jpg\", couldn't be read as \
             an image file."
        );
        assert_eq!(
            reason(left_out(3, 3)),
            "None of the 3 frames could be stacked. The first, \"a.jpg\", couldn't be read as an \
             image file."
        );
        assert_eq!(
            reason(left_out(4, 20)),
            "4 of 20 frames couldn't be stacked. The first, \"a.jpg\", couldn't be read as an \
             image file."
        );
    }

    #[test]
    fn gpu_advice_names_settings_on_the_desktop_only() {
        let e = Error::Core(CoreError::Gpu(GpuError::FrameTooLarge {
            width: 8192,
            height: 5464,
            limit: FrameLimit::Buffer { what: "upload", bytes: 2, max: 1 },
        }));
        let desktop = e.explain(&english(Platform::Desktop));
        assert_eq!(desktop.reason, "Frames of 8192 × 5464 are too large for this GPU.");
        assert_eq!(
            desktop.advice,
            "Stack smaller frames, or choose another GPU in Settings ▸ Advanced."
        );
        assert_eq!(e.explain(&english(Platform::Ios)).advice, "Stack smaller frames.");
    }

    /// On iOS closing other apps barely helps, so only smaller frames are
    /// advised.
    #[test]
    fn a_stack_over_budget_is_worded_for_ios_apart() {
        let e = Error::Core(CoreError::Gpu(GpuError::OverBudget {
            width: 8064,
            height: 6048,
            needed: 2_400_000_000,
            budget: 1_900_000_000,
        }));
        let ios = e.explain(&english(Platform::Ios));
        assert_eq!(
            ios.reason,
            "Frames of 8064 × 6048 need about 2.4 GB of memory, but Deptheos can use only 1.9 GB now."
        );
        assert_eq!(ios.advice, "Stack smaller frames.");
        for platform in [Platform::Desktop, Platform::CommandLine] {
            let said = e.explain(&english(platform));
            assert_eq!(
                said.reason,
                "Frames of 8064 × 6048 need about 2.4 GB of GPU memory, but this GPU has 1.9 GB."
            );
            assert_eq!(said.advice, "Close other apps, or stack smaller frames.");
        }
    }

    #[test]
    fn a_left_out_reason_is_a_lower_case_clause() {
        let reasons = [
            Error::Alignment(AlignmentError::LowCorrelation {
                correlation: 0.2,
                min: 0.6,
                level: (64, 48),
                by_level: "[0.2]".into(),
            }),
            Error::Alignment(AlignmentError::EccFailed("singular".into())),
            Error::Alignment(AlignmentError::NoMatch { difference: 90.0, max: 50.0 }),
            Error::Core(CoreError::Decode { path: "/x/a.heic".into(), message: "x".into() }),
            Error::Core(CoreError::Io(io(ErrorKind::NotFound))),
            Error::Core(CoreError::Raw(RawError::DecodeFailed {
                path: "/x/a.CR3".into(),
                reason: "-4".into(),
            })),
            Error::Core(CoreError::Converter(ConverterError::Stopped("a.CR3".into()))),
            Error::Core(CoreError::Dng(dng_read(DngCause::OutOfMemory))),
            Error::Core(CoreError::InvalidState("frame decode thread panicked".into())),
            Error::InvalidState("prefetcher ran out of frames".into()),
        ];
        let words = english(Platform::Desktop);
        for e in reasons {
            let clause = left_out_reason(&e, &words);
            let first = clause.chars().next().unwrap();
            assert!(first.is_lowercase(), "{clause}");
            assert!(!clause.ends_with('.'), "{clause}");
            for banned in ["failed", "ECC", "correlation", "{"] {
                assert!(!clause.contains(banned), "{banned:?} in {clause}");
            }
        }
        assert_eq!(
            left_out_reason(&Error::Alignment(AlignmentError::Uncorrelated), &words),
            "doesn't look like the frame before it, or barely overlaps it; try Grid search"
        );
    }

    fn dng_read(cause: DngCause) -> DngError {
        DngError::ReadFailed { path: "/x/a.dng".into(), cause }
    }

    /// The DNG SDK's reason is worded for the user, its text in details.
    #[test]
    fn a_dng_read_says_why() {
        let words = english(Platform::Desktop);
        let said = |cause| Error::Core(CoreError::Dng(dng_read(cause))).explain(&words);
        assert_eq!(
            said(DngCause::Damaged).reason,
            "\"a.dng\" couldn't be read as a DNG, because it's damaged or isn't one."
        );
        assert_eq!(
            said(DngCause::Unsupported).reason,
            "\"a.dng\" is a kind of DNG Deptheos can't read."
        );
        let memory = said(DngCause::OutOfMemory);
        assert_eq!(memory.reason, "\"a.dng\" couldn't be read, because there isn't enough memory.");
        assert_eq!(memory.advice, "Close other apps, or stack smaller frames.");
        assert_eq!(
            said(DngCause::TooLarge).reason,
            "\"a.dng\" is too large to read: a DNG can have at most 536 MP."
        );
        let other = said(DngCause::Other { code: 100_000, message: "bad thing".into() });
        assert_eq!(other.reason, "\"a.dng\" couldn't be read as a DNG.");
        assert!(other.details.contains("bad thing (SDK error 100000)"), "{}", other.details);
        let left_out = left_out_reason(&Error::Dng(dng_read(DngCause::Damaged)), &words);
        assert_eq!(left_out, "couldn't be read as a DNG, because it's damaged or isn't one");
    }

    #[test]
    fn details_keep_the_technical_text() {
        let e = Error::Store(StoreError::Io {
            path: PathBuf::from("/lib/render.json"),
            source: io(ErrorKind::PermissionDenied),
        });
        let words = english(Platform::Desktop);
        let explained = e.explain(&words);
        assert_eq!(explained.reason, "Deptheos isn't allowed to open \"render.json\".");
        assert_eq!(explained.details, "/lib/render.json: os says no");
        assert_eq!(explained.text(&words), explained.reason);
        assert_eq!(explained.cause, Cause::NotAllowed, "the front end adds where to allow it");
    }

    /// An error about a Library file names the Library, not the file.
    #[test]
    fn the_library_files_are_not_named() {
        let words = english(Platform::Desktop);
        let id = "0123456789abcdef0123456789abcdef";
        let render = PathBuf::from(format!("/lib/Renders/{id}/render.json"));
        let denied = Error::Store(StoreError::Io {
            path: render.clone(),
            source: io(ErrorKind::PermissionDenied),
        });
        let explained = denied.explain(&words);
        assert_eq!(explained.reason, "Deptheos isn't allowed to open the Library folder.");
        assert_eq!(explained.cause, Cause::NotAllowed);
        let gone = Error::Store(StoreError::Io {
            path: PathBuf::from(format!("/lib/Stacks/{id}.json")),
            source: io(ErrorKind::NotFound),
        });
        let explained = gone.explain(&words);
        assert_eq!(explained.reason, "Part of the Library folder can't be found.");
        assert_eq!(explained.cause, Cause::NotFound);
        let damaged = Error::Store(StoreError::Damaged { path: render });
        assert_eq!(
            damaged.explain(&words).reason,
            "A file in the Library folder is damaged and couldn't be read."
        );
        // A file of the user's keeps its name.
        let frame = Error::Store(StoreError::Io {
            path: PathBuf::from("/photos/bee/IMG_0412.CR3"),
            source: io(ErrorKind::NotFound),
        });
        let explained = frame.explain(&words);
        assert_eq!(explained.reason, "\"IMG_0412.CR3\" can't be found.");
        assert!(explained.advice.starts_with("Check that the drive"), "{}", explained.advice);
    }
}
