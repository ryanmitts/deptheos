//! Windows: a native crash leaves a minidump in [`dir`] for Save
//! Diagnostics…. The app writes its own, as Windows Error Reporting's
//! LocalDumps setting lives in `HKEY_LOCAL_MACHINE`, beyond a per-user
//! installer.
//!
//! [`install`] names the dump up front, prunes to [`KEPT`] and sets an
//! unhandled-exception filter. The filter writes a `MiniDumpNormal` with
//! thread info (under a megabyte) from its own thread, as the faulting one
//! may have no stack left, then passes the exception on. A Rust panic
//! writes no dump; the log has it.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// How many dumps are kept; a launch leaves at most one.
pub const KEPT: usize = 3;

/// `%LOCALAPPDATA%\Deptheos\crashes` on Windows; `None` elsewhere.
pub fn dir() -> Option<PathBuf> {
    #[cfg(windows)]
    return dirs::data_local_dir().map(|dir| dir.join("Deptheos").join("crashes"));
    #[allow(unreachable_code)]
    None
}

/// The dump's name, starting with the process's name, as Save
/// Diagnostics… finds crash reports.
fn file_name(process: &str, launched: SystemTime, pid: u32) -> String {
    let secs = launched.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    format!("{process}.{secs}.{pid}.dmp")
}

/// The dumps in `dir` past the `keep` newest, oldest last.
fn past(dir: &Path, keep: usize) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut dumps: Vec<(SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("dmp")))
        .filter_map(|e| {
            let meta = e.metadata().ok().filter(|m| m.is_file())?;
            Some((meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), e.path()))
        })
        .collect();
    dumps.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    dumps.into_iter().skip(keep).map(|(_, path)| path).collect()
}

/// Remove the dumps in `dir` past the `keep` newest.
fn prune(dir: &Path, keep: usize) {
    for path in past(dir, keep) {
        match std::fs::remove_file(&path) {
            Ok(()) => tracing::debug!("crash dumps: removed {}", path.display()),
            Err(e) => tracing::warn!("crash dumps: couldn't remove {}: {e}", path.display()),
        }
    }
}

/// Writes a minidump into [`dir`] if the process crashes. Call once, with
/// the log. Returns the dump's path, `None` without a folder.
pub fn install() -> Option<PathBuf> {
    let dir = dir()?;
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!("crash dumps: couldn't make {}: {e}", dir.display());
        return None;
    }
    prune(&dir, KEPT - 1);
    let process = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "deptheos".to_owned());
    let path = dir.join(file_name(&process, SystemTime::now(), std::process::id()));
    #[cfg(windows)]
    {
        windows::install(&path);
        tracing::info!("crash dumps: a crash is written to {}", path.display());
        Some(path)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        None
    }
}

#[cfg(windows)]
mod windows {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicBool, Ordering};

    use ::windows::Win32::Foundation::{CloseHandle, GENERIC_WRITE};
    use ::windows::Win32::Storage::FileSystem::{
        CREATE_ALWAYS, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_MODE,
    };
    use ::windows::Win32::System::Diagnostics::Debug::{
        EXCEPTION_POINTERS, LPTOP_LEVEL_EXCEPTION_FILTER, MINIDUMP_EXCEPTION_INFORMATION,
        MiniDumpNormal, MiniDumpWithThreadInfo, MiniDumpWriteDump, SetUnhandledExceptionFilter,
    };
    use ::windows::Win32::System::Threading::{
        CreateThread, GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId,
        THREAD_CREATION_FLAGS, WaitForSingleObject,
    };
    use ::windows::core::PCWSTR;

    /// `EXCEPTION_CONTINUE_SEARCH`: the exception goes on to Windows.
    const CONTINUE_SEARCH: i32 = 0;
    /// How long the faulting thread waits for the dump.
    const WRITE_WAIT_MS: u32 = 30_000;

    /// The dump's NUL-terminated path, made up front: the filter allocates
    /// nothing.
    static PATH: OnceLock<Vec<u16>> = OnceLock::new();
    /// The filter this one replaced, handed every exception after.
    static PREVIOUS: OnceLock<LPTOP_LEVEL_EXCEPTION_FILTER> = OnceLock::new();
    /// Set by the first crash: two threads faulting at once write one dump.
    static WRITING: AtomicBool = AtomicBool::new(false);

    /// What the writing thread is handed: the fault and its thread.
    struct Fault {
        pointers: *const EXCEPTION_POINTERS,
        thread: u32,
    }

    pub(super) fn install(path: &Path) {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        if PATH.set(wide).is_err() {
            return;
        }
        // SAFETY: `on_crash` has the filter's signature and reads only
        // statics set before it is installed.
        let previous = unsafe { SetUnhandledExceptionFilter(Some(on_crash)) };
        let _ = PREVIOUS.set(previous);
    }

    /// Writes the dump once, then passes the exception on.
    unsafe extern "system" fn on_crash(pointers: *const EXCEPTION_POINTERS) -> i32 {
        if !WRITING.swap(true, Ordering::SeqCst) {
            // SAFETY: Win32 calls; `fault` outlives the writing thread,
            // which is waited for (or given up on) before it goes.
            unsafe {
                let fault = Fault { pointers, thread: GetCurrentThreadId() };
                let param = (&raw const fault).cast::<c_void>();
                let started = CreateThread(
                    None,
                    0,
                    Some(write_dump),
                    Some(param),
                    THREAD_CREATION_FLAGS(0),
                    None,
                );
                if let Ok(thread) = started {
                    WaitForSingleObject(thread, WRITE_WAIT_MS);
                    let _ = CloseHandle(thread);
                }
            }
        }
        match PREVIOUS.get().copied().flatten() {
            // SAFETY: the filter Windows had before this one.
            Some(previous) => unsafe { previous(pointers) },
            None => CONTINUE_SEARCH,
        }
    }

    /// The writing thread, for the fault `param` points to; 0 once written.
    unsafe extern "system" fn write_dump(param: *mut c_void) -> u32 {
        let Some(path) = PATH.get() else { return 1 };
        // SAFETY: `param` is the `Fault` `on_crash` waits on; the path is
        // NUL-terminated.
        unsafe {
            let fault = &*param.cast::<Fault>();
            let Ok(file) = CreateFileW(
                PCWSTR(path.as_ptr()),
                GENERIC_WRITE.0,
                FILE_SHARE_MODE(0),
                None,
                CREATE_ALWAYS,
                FILE_ATTRIBUTE_NORMAL,
                None,
            ) else {
                return 1;
            };
            let exception = MINIDUMP_EXCEPTION_INFORMATION {
                ThreadId: fault.thread,
                ExceptionPointers: fault.pointers.cast_mut(),
                ClientPointers: false.into(),
            };
            let written = MiniDumpWriteDump(
                GetCurrentProcess(),
                GetCurrentProcessId(),
                file,
                MiniDumpNormal | MiniDumpWithThreadInfo,
                Some(&raw const exception),
                None,
                None,
            );
            let _ = CloseHandle(file);
            u32::from(written.is_err())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// A dump is named for its process first.
    #[test]
    fn a_dump_is_named_for_its_process() {
        let launched = SystemTime::UNIX_EPOCH + Duration::from_secs(1_763_600_000);
        let name = file_name("deptheos-gui.exe", launched, 4242);
        assert_eq!(name, "deptheos-gui.exe.1763600000.4242.dmp");
    }

    /// Pruning keeps the newest dumps and leaves every other file alone.
    #[test]
    fn pruning_keeps_the_newest_dumps() {
        let dir = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        for (i, name) in ["a.dmp", "b.dmp", "c.DMP", "d.dmp"].iter().enumerate() {
            let path = dir.path().join(name);
            std::fs::write(&path, b"dump").unwrap();
            let file = std::fs::File::options().write(true).open(&path).unwrap();
            file.set_modified(now - Duration::from_secs(60 * (4 - i as u64))).unwrap();
        }
        std::fs::write(dir.path().join("notes.txt"), b"kept").unwrap();
        prune(dir.path(), KEPT - 1);
        let mut left: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["c.DMP", "d.dmp", "notes.txt"]);
    }
}
