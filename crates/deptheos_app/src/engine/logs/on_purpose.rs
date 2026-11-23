//! Crashes on purpose, to check what a crash leaves behind
//! (`docs/crash-testing.md`). The desktop crashes when launched with
//! `DEPTHEOS_CRASH` set to a kind's name; the iOS app from Settings ▸
//! Diagnostics in Debug and TestFlight builds.

/// A kind of crash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A panic on the calling thread; on a UI thread it ends the app.
    Panic,
    /// A panic on an engine thread of its own; the app goes on.
    ThreadPanic,
    /// `abort()`: a native crash. On Windows it fails fast, past the
    /// minidump filter.
    Abort,
    /// A read of an unmapped address: `SIGSEGV`, or an access violation the
    /// minidump filter sees on Windows.
    Fault,
}

/// The environment variable the desktop reads at launch.
pub const VARIABLE: &str = "DEPTHEOS_CRASH";

impl Kind {
    /// Every kind, in the order a menu shows them.
    pub const ALL: [Self; 4] = [Self::Panic, Self::ThreadPanic, Self::Abort, Self::Fault];

    /// The kind's name, as [`VARIABLE`] takes it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Panic => "panic",
            Self::ThreadPanic => "thread-panic",
            Self::Abort => "abort",
            Self::Fault => "fault",
        }
    }

    /// The kind named `name`, ignoring case and surrounding space.
    pub fn parse(name: &str) -> Option<Self> {
        let name = name.trim();
        Self::ALL.into_iter().find(|kind| kind.name().eq_ignore_ascii_case(name))
    }

    /// The kind [`VARIABLE`] names; an unknown name is logged and ignored.
    pub fn from_environment() -> Option<Self> {
        let value = std::env::var(VARIABLE).ok()?;
        let kind = Self::parse(&value);
        if kind.is_none() {
            let names: Vec<&str> = Self::ALL.iter().map(|k| k.name()).collect();
            tracing::warn!("{VARIABLE}={value:?} is none of {}", names.join(", "));
        }
        kind
    }
}

/// Crashes as `kind` says; returns only for [`Kind::ThreadPanic`].
pub fn crash(kind: Kind) {
    tracing::warn!(target: "deptheos::session", "crashing on purpose: {}", kind.name());
    if let Some(logs) = super::installed() {
        logs.flush();
    }
    match kind {
        Kind::Panic => panic!("a crash on purpose ({})", kind.name()),
        Kind::ThreadPanic => {
            let spawned = crate::engine::work::thread("crash-on-purpose")
                .spawn(move || panic!("a crash on purpose ({})", kind.name()));
            if let Err(e) = spawned {
                tracing::warn!("no thread to crash on: {e}");
            }
        }
        Kind::Abort => std::process::abort(),
        Kind::Fault => fault(),
    }
}

/// Reads an address never mapped. A raised `SIGSEGV` would not do: Rust's
/// stack-overflow handler takes it and returns; a real bad access faults
/// again under the default action.
#[allow(clippy::manual_dangling_ptr)]
fn fault() {
    let address = std::hint::black_box(16usize);
    // SAFETY: none: this is the crash asked for, volatile so it stays.
    let byte = unsafe { std::ptr::read_volatile(std::ptr::without_provenance::<u8>(address)) };
    tracing::error!("read {byte} at {address:#x}, which should have crashed");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_is_found_by_its_name() {
        for kind in Kind::ALL {
            assert_eq!(Kind::parse(kind.name()), Some(kind));
        }
        assert_eq!(Kind::parse(" Thread-Panic\n"), Some(Kind::ThreadPanic));
        assert_eq!(Kind::parse("segfault"), None);
    }

    /// The one kind a test can cause: the thread panics, the caller goes on.
    #[test]
    fn a_thread_panic_leaves_the_caller_running() {
        crash(Kind::ThreadPanic);
    }
}
