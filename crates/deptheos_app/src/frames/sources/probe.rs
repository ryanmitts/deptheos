//! What [`Sources`](super::Sources) asks of the file system, as a trait so
//! tests can fake it, and the facts derived from it: fingerprints, trash
//! detection and the name-matching key.

use std::io;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

use super::{Volume, VolumeKind};

/// The facts roots are resolved from, uncached. The real one is
/// [`Disk`](super::Disk).
pub trait Probe {
    /// What is at `path`, following links; `None` when nothing is.
    fn stat(&self, path: &Path) -> io::Result<Option<Stat>>;
    /// `path` with its links resolved, in its plain form (no `\\?\` on
    /// Windows). Called for a root's folder only.
    fn canonical(&self, path: &Path) -> io::Result<PathBuf>;
    /// The volume `path` is on, and where it is mounted.
    fn volume(&self, path: &Path) -> io::Result<Mounted>;
    /// The volumes mounted now.
    fn volumes(&self) -> Vec<Mounted>;
    /// Up to `len` bytes of the file `path` from `offset`.
    fn read(&self, path: &Path, offset: u64, len: usize) -> io::Result<Vec<u8>>;
    /// The names in the folder `dir` (any that aren't Unicode are left out).
    fn list(&self, dir: &Path) -> io::Result<Vec<String>>;
}

/// What a path is: its kind, its size and its file-system identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stat {
    pub folder: bool,
    pub size: u64,
    /// `None` where the file system doesn't say.
    pub id: Option<FileId>,
}

/// A file's identity, the same by any path. A share mounted twice has two
/// `device`s and one `file`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FileId {
    pub device: u64,
    pub file: u128,
}

/// A volume and the path it is mounted at now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mounted {
    pub volume: Volume,
    pub mount: PathBuf,
}

/// How much of each end of a file its fingerprint reads.
pub const FINGERPRINT_END: usize = 64 * 1024;

/// SeaHash (stable across versions and platforms) of a file's first and
/// last 64 KiB: evidence a found file is the one added, never a gate on
/// reading. Written as 16 hex digits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct Fingerprint(pub u64);

impl From<Fingerprint> for String {
    fn from(f: Fingerprint) -> Self {
        format!("{:016x}", f.0)
    }
}

impl TryFrom<String> for Fingerprint {
    type Error = String;
    fn try_from(hex: String) -> Result<Self, String> {
        match u64::from_str_radix(&hex, 16) {
            Ok(v) if hex.len() == 16 => Ok(Self(v)),
            _ => Err(format!("not a fingerprint: {hex:?}")),
        }
    }
}

impl Fingerprint {
    /// From a file's `head` and `tail` (empty when `head` is all of it).
    pub fn of_ends(head: &[u8], tail: &[u8]) -> Self {
        let mut bytes = Vec::with_capacity(head.len() + tail.len());
        bytes.extend_from_slice(head);
        bytes.extend_from_slice(tail);
        Self(seahash::hash(&bytes))
    }

    /// The fingerprint of `path`, read through `probe`.
    pub fn of(probe: &dyn Probe, path: &Path, size: u64) -> io::Result<Self> {
        let end = FINGERPRINT_END as u64;
        let head = probe.read(path, 0, size.min(end) as usize)?;
        let tail = if size > 2 * end {
            probe.read(path, size - end, FINGERPRINT_END)?
        } else if size > end {
            probe.read(path, end, (size - end) as usize)?
        } else {
            Vec::new()
        };
        Ok(Self::of_ends(&head, &tail))
    }
}

/// The key names match by (NFC, case folded), for Locate… and re-adds;
/// identity compares exactly.
pub fn key(name: &str) -> String {
    name.nfc().collect::<String>().to_lowercase()
}

/// A server's key: folded, without local-network suffixes
/// (`nas._smb._tcp.local`, `nas.local` and `NAS` are one).
fn server_key(server: &str) -> String {
    let server = key(server.trim_end_matches('.'));
    let bare = ["._smb._tcp.local", "._afpovertcp._tcp.local", ".local"]
        .iter()
        .find_map(|suffix| server.strip_suffix(suffix));
    bare.unwrap_or(&server).to_string()
}

/// Whether `path` is in a trash folder, which counts as missing.
pub fn in_trash(path: &Path) -> bool {
    let names: Vec<String> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(name) => Some(name.to_string_lossy().to_lowercase()),
            _ => None,
        })
        .collect();
    names.iter().enumerate().any(|(i, name)| {
        matches!(name.as_str(), ".trash" | ".trashes" | "$recycle.bin" | "recycler")
            || name.starts_with(".trash-")
            || (name == "trash" && i >= 2 && names[i - 1] == "share" && names[i - 2] == ".local")
    })
}

/// How sure it is that two volumes are one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Same {
    /// Their ids agree, or they are one share.
    Yes,
    /// Only their names agree: a candidate, proved by a sample of frames.
    Maybe,
    /// Known ids differ, different shares, or nothing agrees.
    No,
}

impl Volume {
    /// Ids compare only within a scheme (`uuid:` with `uuid:`), since
    /// systems name one card differently.
    pub fn same(&self, other: &Volume) -> Same {
        match (&self.kind, &other.kind) {
            (
                VolumeKind::Share { server: a, share: x },
                VolumeKind::Share { server: b, share: y },
            ) => {
                if server_key(a) == server_key(b) && key(x) == key(y) {
                    Same::Yes
                } else {
                    Same::No
                }
            }
            (VolumeKind::Disk, VolumeKind::Disk) => {
                let scheme = |id: &str| id.split_once(':').map(|(s, _)| s.to_string());
                match (&self.id, &other.id) {
                    (Some(a), Some(b)) if scheme(a) == scheme(b) => {
                        if a.eq_ignore_ascii_case(b) {
                            Same::Yes
                        } else {
                            Same::No
                        }
                    }
                    _ if key(&self.name) == key(&other.name) => Same::Maybe,
                    _ => Same::No,
                }
            }
            _ => Same::No,
        }
    }
}
