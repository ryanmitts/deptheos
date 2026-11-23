//! The sentences only the iOS app shows, returned by the bridge
//! (`crates/deptheos_ffi`) and worded here in the app's language.

use crate::words::{Words, say};

/// One of the bridge's own sentences.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line {
    /// After the fault sentence: where to send the details.
    ReportFault,
    /// Send Diagnostics… found no log file for this launch.
    NoLogFile,
    /// The diagnostics couldn't be written: the device is full.
    DeviceFull,
    /// The diagnostics couldn't be written, for another reason.
    TryAgain,
    /// Leads, before why.
    RetouchedExportFailed,
    SaveFailed,
    SampleNotCopied,
    /// The engine didn't start: its GPU didn't.
    GpuDidntStart,
    /// The engine didn't start: its Library didn't open.
    LibraryDidntOpen,
}

impl Line {
    /// The sentence, in `words`' language.
    pub fn text(self, words: &Words) -> String {
        let w = words;
        match self {
            Self::ReportFault => say!(w, "ios-report-fault"),
            Self::NoLogFile => say!(w, "ios-no-log-file"),
            Self::DeviceFull => say!(w, "ios-device-full"),
            Self::TryAgain => say!(w, "ios-try-again"),
            Self::RetouchedExportFailed => say!(w, "ios-retouched-export-failed"),
            Self::SaveFailed => say!(w, "ios-save-failed"),
            Self::SampleNotCopied => say!(w, "ios-sample-not-copied"),
            Self::GpuDidntStart => say!(w, "ios-gpu-didnt-start"),
            Self::LibraryDidntOpen => say!(w, "ios-library-didnt-open"),
        }
    }
}
