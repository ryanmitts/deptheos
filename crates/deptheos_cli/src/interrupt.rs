//! Ctrl-C: the first press cancels the engine's root token, so the run
//! stops at once, keeping what it wrote and deleting its temporary files. A
//! second press quits at once; the next run sweeps what that left
//! (`TempStore`).

use deptheos_app::Cancel;

/// The token every run hangs from: the engine's root, canceled by Ctrl-C.
pub fn token() -> &'static Cancel {
    deptheos_app::engine::work::root()
}

/// Catches Ctrl-C and SIGTERM. The handler only writes to a pipe (a signal
/// handler may not lock); a thread reading it cancels the root.
#[cfg(unix)]
pub fn install() {
    use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

    static INTERRUPTED: AtomicBool = AtomicBool::new(false);
    static WAKE: AtomicI32 = AtomicI32::new(-1);

    extern "C" fn on_signal(_: libc::c_int) {
        // Only async-signal-safe calls here: an atomic, write and _exit.
        if INTERRUPTED.swap(true, Ordering::SeqCst) {
            unsafe { libc::_exit(130) };
        }
        let note = b"\nstopping (Ctrl-C again to quit now)\n";
        unsafe {
            libc::write(libc::STDERR_FILENO, note.as_ptr().cast(), note.len());
            libc::write(WAKE.load(Ordering::SeqCst), b"!".as_ptr().cast(), 1);
        }
    }
    let mut fds = [0; 2];
    // SAFETY: `fds` has room for the two descriptors.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        eprintln!("Ctrl-C can't stop a run: {}", std::io::Error::last_os_error());
        return;
    }
    let [read, write] = fds;
    WAKE.store(write, Ordering::SeqCst);
    let watcher = deptheos_app::engine::work::thread("interrupt").spawn(move || {
        let mut byte = 0u8;
        loop {
            // SAFETY: reading one byte into `byte` from the pipe made above.
            match unsafe { libc::read(read, (&raw mut byte).cast(), 1) } {
                1 => return token().cancel(),
                0 => return,
                _ => {
                    let error = std::io::Error::last_os_error();
                    if error.kind() != std::io::ErrorKind::Interrupted {
                        eprintln!("Ctrl-C can't stop a run: {error}");
                        return;
                    }
                }
            }
        }
    });
    if let Err(e) = watcher {
        eprintln!("Ctrl-C can't stop a run: {e}");
        return;
    }
    let handler = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
    // SAFETY: installing a handler that makes only async-signal-safe calls.
    unsafe {
        libc::signal(libc::SIGINT, handler);
        libc::signal(libc::SIGTERM, handler);
    }
}

/// Catches Ctrl-C and Ctrl-Break. Windows runs the handler on its own
/// thread, so it cancels the root itself; a second press falls through to
/// the system's handler, which ends the process.
#[cfg(windows)]
pub fn install() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use windows::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT, SetConsoleCtrlHandler};
    use windows::core::BOOL;

    static INTERRUPTED: AtomicBool = AtomicBool::new(false);

    unsafe extern "system" fn on_ctrl(event: u32) -> BOOL {
        if !matches!(event, CTRL_C_EVENT | CTRL_BREAK_EVENT)
            || INTERRUPTED.swap(true, Ordering::SeqCst)
        {
            return false.into();
        }
        eprintln!("\nstopping (Ctrl-C again to quit now)");
        token().cancel();
        true.into()
    }
    // SAFETY: a handler that only touches an atomic, stderr and the token,
    // all safe from any thread.
    if let Err(e) = unsafe { SetConsoleCtrlHandler(Some(on_ctrl), true) } {
        eprintln!("Ctrl-C can't stop a run: {e}");
    }
}

/// Elsewhere Ctrl-C ends the process; the next run sweeps its temporary
/// files.
#[cfg(not(any(unix, windows)))]
pub fn install() {}
