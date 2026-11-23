//! What the CLI prints: the files it writes on stdout, one path a line, so
//! a script can read them; progress and warnings on stderr, progress
//! silenced by `--quiet`.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

static QUIET: AtomicBool = AtomicBool::new(false);

pub(crate) fn set_quiet(quiet: bool) {
    QUIET.store(quiet, Ordering::Relaxed);
}

pub(crate) fn quiet() -> bool {
    QUIET.load(Ordering::Relaxed)
}

/// A progress line on stderr, unless `--quiet`.
macro_rules! progress {
    ($($arg:tt)*) => {
        if !$crate::ui::quiet() {
            eprintln!($($arg)*);
        }
    };
}
pub(crate) use progress;

/// A warning on stderr, `--quiet` or not.
pub(crate) fn warn(message: impl std::fmt::Display) {
    eprintln!("warning: {message}");
}

/// A stack's warnings: the frames it left out and its notes, each led by
/// the stack's `label` when the run makes more than one.
pub(crate) fn warn_stack(
    stack: &deptheos_app::CompletedStack,
    words: &deptheos_app::Words,
    label: Option<&str>,
) {
    let say = |line: &dyn std::fmt::Display| match label {
        Some(label) => warn(format!("{label}: {line}")),
        None => warn(line),
    };
    if let Some(left_out) = stack.left_out_message(words) {
        say(&left_out);
    }
    for note in &stack.notes {
        say(note);
    }
}

/// A file or folder written: its path on stdout.
pub(crate) fn wrote(path: &Path) {
    println!("{}", path.display());
}

/// A time taken, in seconds with a spaced unit: "12.3 s".
pub(crate) fn secs(d: Duration) -> String {
    format!("{:.1} s", d.as_secs_f64())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_have_a_spaced_unit() {
        assert_eq!(secs(Duration::from_millis(12_340)), "12.3 s");
        assert_eq!(secs(Duration::from_millis(850)), "0.8 s");
    }
}
