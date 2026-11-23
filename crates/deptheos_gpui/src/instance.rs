//! One running app per user (Windows and Linux; macOS does this itself).
//!
//! The Library, list and settings are rewritten as they change, so two apps
//! would overwrite each other. The first launch takes a lock in the config
//! folder and listens on a loopback port; a later launch finds the lock
//! taken, hands its files over and exits, and the first adds them and comes
//! to the front.
//!
//! `instance.json` beside the lock holds the port, a launch token and the
//! pid; a hand-off must say the token first, so only something that can
//! read the user's config folder can hand files over, and it carries only
//! paths. If any of this can't be set up, the app starts alone.

// Built and tested everywhere, used where the system doesn't do it.
#![cfg_attr(target_os = "macos", allow(dead_code))]

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

const LOCK_FILE: &str = "instance.lock";
const INFO_FILE: &str = "instance.json";

/// How long a later launch waits for the first to say where it listens (it
/// may be starting too) and to answer.
const PATIENCE: Duration = Duration::from_secs(3);
/// How long the first waits for a hand-off's line.
const READ_TIMEOUT: Duration = Duration::from_secs(2);
/// The longest hand-off read: thousands of long paths.
const MAX_HAND_OFF: u64 = 4 << 20;

/// How this launch stands to the others, decided once at start:
///
/// - `First`: none running; holds the lock until exit and [`First::listen`]s.
/// - `HandedOff`: another took this launch's files; this one exits.
/// - `Alone`: couldn't tell; runs without the lock, the sentence saying why.
///
/// Claimed before the log is installed, so a hand-off makes (and prunes) no
/// log file; the caller logs `Alone`'s reason later.
pub(crate) enum Claim {
    First(First),
    HandedOff,
    Alone(String),
}

/// The first launch's lock, held while this lives, and its listener.
pub(crate) struct First {
    _lock: File,
    listener: TcpListener,
    token: String,
}

/// What `instance.json` holds.
#[derive(Serialize, Deserialize)]
struct Info {
    port: u16,
    token: String,
    pid: u32,
}

/// One hand-off, a line of JSON.
#[derive(Serialize, Deserialize)]
struct HandOff {
    token: String,
    paths: Vec<String>,
}

/// The folder the lock lives in: the app's config folder.
pub(crate) fn default_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("Deptheos"))
}

/// Become the one app over `dir`, or hand `paths` to the one that is.
pub(crate) fn claim(dir: &Path, paths: &[PathBuf]) -> Claim {
    let lock = match open_lock(dir) {
        Ok(lock) => lock,
        Err(e) => return Claim::Alone(format!("no instance lock in {}: {e}", dir.display())),
    };
    match lock.try_lock() {
        Ok(()) => match First::start(lock, dir) {
            Ok(first) => Claim::First(first),
            Err(e) => Claim::Alone(format!("cannot listen for later launches: {e}")),
        },
        Err(std::fs::TryLockError::WouldBlock) => match hand_off(dir, paths) {
            Ok(()) => Claim::HandedOff,
            Err(e) => {
                Claim::Alone(format!("another Deptheos holds the lock but did not answer: {e}"))
            }
        },
        Err(std::fs::TryLockError::Error(e)) => {
            Claim::Alone(format!("cannot lock {}: {e}", dir.join(LOCK_FILE).display()))
        }
    }
}

fn open_lock(dir: &Path) -> std::io::Result<File> {
    std::fs::create_dir_all(dir)?;
    File::options().read(true).write(true).create(true).truncate(false).open(dir.join(LOCK_FILE))
}

impl First {
    fn start(lock: File, dir: &Path) -> std::io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let info = Info {
            port: listener.local_addr()?.port(),
            token: new_token(),
            pid: std::process::id(),
        };
        write_info(dir, &info)?;
        Ok(Self { _lock: lock, listener, token: info.token })
    }

    /// Take later launches' files while the app runs, each hand-off's paths
    /// to `opened` (none: "come to the front"). Answered only once `opened`
    /// takes it, so an unserved launch starts on its own.
    pub(crate) fn listen(self, opened: impl Fn(Vec<PathBuf>) -> bool + Send + 'static) {
        let spawned = crate::work::thread("instance").spawn(move || {
            // The lock lives as long as this thread, i.e. the process:
            // `accept` never ends.
            let Self { _lock, listener, token } = self;
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                match receive(&stream, &token) {
                    Ok(paths) => {
                        tracing::info!("a later launch handed over {} paths", paths.len());
                        if opened(paths) {
                            (&stream).write_all(b"ok\n").ok();
                        }
                    }
                    Err(e) => tracing::warn!("a hand-off was not taken: {e}"),
                }
            }
        });
        if let Err(e) = spawned {
            tracing::warn!("cannot listen for later launches: {e}");
        }
    }
}

/// Read one hand-off from `stream`: its paths, when it says `token`.
fn receive(stream: &TcpStream, token: &str) -> std::io::Result<Vec<PathBuf>> {
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    let mut line = String::new();
    BufReader::new(stream.take(MAX_HAND_OFF)).read_line(&mut line)?;
    let hand_off: HandOff = serde_json::from_str(&line)?;
    if hand_off.token != token {
        return Err(std::io::Error::other("wrong token"));
    }
    Ok(hand_off.paths.into_iter().map(PathBuf::from).collect())
}

/// Hand `paths` to the app holding the lock over `dir`, waiting a little
/// for it to say where it listens.
fn hand_off(dir: &Path, paths: &[PathBuf]) -> std::io::Result<()> {
    let started = Instant::now();
    loop {
        match try_hand_off(dir, paths) {
            Ok(()) => return Ok(()),
            Err(e) if started.elapsed() >= PATIENCE => return Err(e),
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn try_hand_off(dir: &Path, paths: &[PathBuf]) -> std::io::Result<()> {
    let info: Info = serde_json::from_slice(&std::fs::read(dir.join(INFO_FILE))?)?;
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, info.port));
    let mut stream = TcpStream::connect_timeout(&address, PATIENCE)?;
    stream.set_read_timeout(Some(PATIENCE))?;
    // The running app's window may come to the front for this launch.
    let_front(info.pid);
    let hand_off = HandOff {
        token: info.token,
        // Relative to this launch's folder, not the running app's.
        paths: paths
            .iter()
            .map(|p| std::path::absolute(p).unwrap_or_else(|_| p.clone()))
            .filter_map(|p| p.to_str().map(str::to_owned))
            .collect(),
    };
    let mut line = serde_json::to_vec(&hand_off)?;
    line.push(b'\n');
    stream.write_all(&line)?;
    let mut answer = String::new();
    BufReader::new(&stream).read_line(&mut answer)?;
    if answer.trim_end() == "ok" {
        Ok(())
    } else {
        Err(std::io::Error::other("the running app did not take the hand-off"))
    }
}

/// Write `instance.json` whole (temp file then rename), user-only where the
/// system has file modes.
fn write_info(dir: &Path, info: &Info) -> std::io::Result<()> {
    let tmp = dir.join(format!("{INFO_FILE}.tmp"));
    let mut options = File::options();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    file.write_all(&serde_json::to_vec(info)?)?;
    drop(file);
    std::fs::rename(&tmp, dir.join(INFO_FILE))
}

/// 128 bits from the system's randomness, as hex.
fn new_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    // Each `RandomState` is keyed from the system's random source.
    let word = || std::collections::hash_map::RandomState::new().build_hasher().finish();
    format!("{:016x}{:016x}", word(), word())
}

/// Windows lets a process take the foreground only when the one in front
/// allows it: this freshly started launch allows the running app.
#[cfg(windows)]
fn let_front(pid: u32) {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn AllowSetForegroundWindow(process_id: u32) -> i32;
    }
    // SAFETY: takes a process id by value; an unknown id fails harmlessly.
    unsafe { AllowSetForegroundWindow(pid) };
}

#[cfg(not(windows))]
fn let_front(_pid: u32) {}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    fn first(dir: &Path) -> First {
        match claim(dir, &[]) {
            Claim::First(first) => first,
            _ => panic!("the first launch is first"),
        }
    }

    /// A second launch hands absolute paths to the first and is told to exit;
    /// a third, without files, still reaches it.
    #[test]
    fn a_later_launch_hands_its_files_to_the_first() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        first(dir.path()).listen(move |paths| tx.send(paths).is_ok());

        let folder = dir.path().join("bee");
        assert!(matches!(claim(dir.path(), &[folder.clone(), "f1.png".into()]), Claim::HandedOff));
        let got = rx.recv_timeout(PATIENCE).unwrap();
        assert_eq!(got, [folder, std::path::absolute("f1.png").unwrap()]);

        assert!(matches!(claim(dir.path(), &[]), Claim::HandedOff));
        assert_eq!(rx.recv_timeout(PATIENCE).unwrap(), Vec::<PathBuf>::new());
    }

    /// A hand-off without the launch's token is not taken.
    #[test]
    fn a_hand_off_needs_the_token() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        first(dir.path()).listen(move |paths| tx.send(paths).is_ok());
        let info: Info =
            serde_json::from_slice(&std::fs::read(dir.path().join(INFO_FILE)).unwrap()).unwrap();
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, info.port)).unwrap();
        stream.write_all(b"{\"token\":\"guess\",\"paths\":[\"/x.png\"]}\n").unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).unwrap();
        assert_eq!(answer, "");
        assert!(rx.try_recv().is_err());
    }

    /// An app that can't serve a hand-off (no window) doesn't answer, so the
    /// later launch starts on its own.
    #[test]
    fn a_launch_the_first_cannot_serve_starts_alone() {
        let dir = tempfile::tempdir().unwrap();
        first(dir.path()).listen(|_| false);
        assert!(matches!(claim(dir.path(), &[]), Claim::Alone(_)));
    }

    /// The lock goes with the app: the next launch after it is first.
    #[test]
    fn the_launch_after_the_first_has_gone_is_first() {
        let dir = tempfile::tempdir().unwrap();
        drop(first(dir.path()));
        drop(first(dir.path()));
    }

    #[test]
    fn tokens_differ() {
        assert_ne!(new_token(), new_token());
        assert_eq!(new_token().len(), 32);
    }
}
