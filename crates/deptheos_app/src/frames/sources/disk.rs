//! [`Disk`]: the [`Probe`] of the real file system, with each platform's
//! way of naming a volume.
//!
//! - **macOS and iOS:** `statfs` for the mount, source and file system;
//!   `getattrlist` for the name and UUID (`uuid:`, made up from the serial
//!   on FAT and exFAT).
//! - **Linux:** `/proc/self/mountinfo` for the mount; UUID or serial from
//!   `/dev/disk/by-uuid`, label from `/dev/disk/by-label`.
//! - **Windows:** `GetVolumePathNameW` (a share by its UNC path);
//!   `GetVolumeInformationW` for the name and serial (`serial:`);
//!   `GetFileInformationByHandleEx(FileIdInfo)` for file ids.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use super::probe::{FileId, Mounted, Probe, Stat};

/// The real file system.
#[derive(Clone, Copy, Debug, Default)]
pub struct Disk;

impl Probe for Disk {
    fn stat(&self, path: &Path) -> io::Result<Option<Stat>> {
        let meta = match std::fs::metadata(path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        Ok(Some(Stat { folder: meta.is_dir(), size: meta.len(), id: file_id(path, &meta) }))
    }

    fn canonical(&self, path: &Path) -> io::Result<PathBuf> {
        std::fs::canonicalize(path).map(|p| plain(&p))
    }

    fn volume(&self, path: &Path) -> io::Result<Mounted> {
        os::volume(path)
    }

    fn volumes(&self) -> Vec<Mounted> {
        os::volumes()
    }

    fn read(&self, path: &Path, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        let mut file = File::open(path)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = Vec::with_capacity(len);
        file.take(len as u64).read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    fn list(&self, dir: &Path) -> io::Result<Vec<String>> {
        Ok(std::fs::read_dir(dir)?
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect())
    }
}

/// `path` without Windows' verbatim prefix (`\\?\C:\a` is `C:\a`,
/// `\\?\UNC\server\share` is `\\server\share`).
fn plain(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        path.to_path_buf()
    }
}

#[cfg(unix)]
fn file_id(_path: &Path, meta: &std::fs::Metadata) -> Option<FileId> {
    use std::os::unix::fs::MetadataExt;
    Some(FileId { device: meta.dev(), file: u128::from(meta.ino()) })
}

#[cfg(windows)]
fn file_id(path: &Path, _meta: &std::fs::Metadata) -> Option<FileId> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx,
    };
    // Folders need backup semantics; asking no access opens files in use.
    let file = std::fs::OpenOptions::new()
        .access_mode(0)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0)
        .open(path)
        .ok()?;
    let mut info = FILE_ID_INFO::default();
    // SAFETY: the handle is open for the call, and `info` is a writable
    // FILE_ID_INFO of the size given.
    unsafe {
        GetFileInformationByHandleEx(
            HANDLE(file.as_raw_handle()),
            FileIdInfo,
            (&mut info as *mut FILE_ID_INFO).cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    }
    .ok()?;
    Some(FileId {
        device: info.VolumeSerialNumber,
        file: u128::from_le_bytes(info.FileId.Identifier),
    })
}

#[cfg(not(any(unix, windows)))]
fn file_id(_path: &Path, _meta: &std::fs::Metadata) -> Option<FileId> {
    None
}

/// Server and share from `//user@server/share` (SMB, percent-decoded),
/// `server:/export` (NFS) or a URL (WebDAV).
#[cfg_attr(not(unix), allow(dead_code))]
fn share_of(source: &str) -> Option<(String, String)> {
    if let Some(rest) = source.strip_prefix("//") {
        let (server, share) = rest.split_once('/')?;
        let server = server.rsplit_once('@').map_or(server, |(_, s)| s);
        let share = share.trim_end_matches('/');
        return Some((server.to_string(), percent_decoded(share)));
    }
    if let Some((_, rest)) = source.split_once("://") {
        let (server, share) = rest.split_once('/').unwrap_or((rest, ""));
        return Some((server.to_string(), percent_decoded(share.trim_end_matches('/'))));
    }
    let (server, export) = source.split_once(':')?;
    (!server.is_empty() && export.starts_with('/')).then(|| {
        (server.rsplit_once('@').map_or(server, |(_, s)| s).to_string(), export.to_string())
    })
}

#[cfg_attr(not(unix), allow(dead_code))]
fn percent_decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok());
        match (bytes[i], hex.and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_string())
}

/// The volume's name when the system gives none: its mount's last name.
#[cfg_attr(windows, allow(dead_code))]
fn name_of_mount(mount: &Path) -> String {
    mount.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "/".into())
}

#[cfg(target_vendor = "apple")]
mod os {
    use std::ffi::{CStr, CString};
    use std::io;
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Path, PathBuf};

    use super::{Mounted, name_of_mount, share_of};
    use crate::frames::sources::{Volume, VolumeKind};

    const SHARES: [&str; 4] = ["smbfs", "nfs", "afpfs", "webdav"];
    const NOT_VOLUMES: [&str; 3] = ["devfs", "autofs", "nullfs"];

    pub(super) fn volume(path: &Path) -> io::Result<Mounted> {
        let c = CString::new(path.as_os_str().as_bytes())?;
        // SAFETY: zeroed is a valid statfs; `c` is a C string and `stat` is
        // writable for the call.
        let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statfs(c.as_ptr(), &mut stat) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut mounted = of_statfs(&stat);
        // The boot volume's data is mounted at /System/Volumes/Data and
        // reached from / through firmlinks: its paths are under /.
        if !path.starts_with(&mounted.mount) {
            mounted.mount = PathBuf::from("/");
        }
        Ok(mounted)
    }

    pub(super) fn volumes() -> Vec<Mounted> {
        // SAFETY: a null buffer asks for the count only.
        let count = unsafe { libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT) };
        if count <= 0 {
            return Vec::new();
        }
        // SAFETY: zeroed is a valid statfs.
        let mut stats: Vec<libc::statfs> =
            (0..count + 4).map(|_| unsafe { std::mem::zeroed() }).collect();
        let size = (stats.len() * std::mem::size_of::<libc::statfs>()) as libc::c_int;
        // SAFETY: `stats` is writable for `size` bytes.
        let got = unsafe { libc::getfsstat(stats.as_mut_ptr(), size, libc::MNT_NOWAIT) };
        stats.truncate(got.max(0) as usize);
        stats
            .iter()
            .filter(|s| !NOT_VOLUMES.contains(&text(&s.f_fstypename).as_str()))
            .map(of_statfs)
            .collect()
    }

    fn text(chars: &[libc::c_char]) -> String {
        // SAFETY: the kernel NUL-terminates the names in a statfs.
        unsafe { CStr::from_ptr(chars.as_ptr()) }.to_string_lossy().into_owned()
    }

    fn of_statfs(stat: &libc::statfs) -> Mounted {
        let mount = PathBuf::from(text(&stat.f_mntonname));
        let fs = text(&stat.f_fstypename);
        let share = SHARES.contains(&fs.as_str()).then(|| share_of(&text(&stat.f_mntfromname)));
        let volume = match share.flatten() {
            Some((server, share)) => Volume {
                id: None,
                name: share.rsplit('/').next().unwrap_or(&share).to_string(),
                kind: VolumeKind::Share { server, share },
            },
            None => Volume {
                id: attribute(&mount, libc::ATTR_VOL_UUID).and_then(|b| uuid(&b)),
                name: attribute(&mount, libc::ATTR_VOL_NAME)
                    .and_then(|b| name(&b))
                    .unwrap_or_else(|| name_of_mount(&mount)),
                kind: VolumeKind::Disk,
            },
        };
        Mounted { volume, mount }
    }

    /// Attribute `which` of the volume at `mount`: `getattrlist`'s buffer
    /// after its length.
    fn attribute(mount: &Path, which: libc::attrgroup_t) -> Option<Vec<u8>> {
        let c = CString::new(mount.as_os_str().as_bytes()).ok()?;
        // SAFETY: zeroed is a valid attrlist.
        let mut list: libc::attrlist = unsafe { std::mem::zeroed() };
        list.bitmapcount = libc::ATTR_BIT_MAP_COUNT;
        list.volattr = libc::ATTR_VOL_INFO | which;
        let mut buf = vec![0u8; 1024];
        // SAFETY: `c` is a C string, `list` an attrlist and `buf` writable
        // for its length.
        let r = unsafe {
            libc::getattrlist(
                c.as_ptr(),
                (&mut list as *mut libc::attrlist).cast(),
                buf.as_mut_ptr().cast(),
                buf.len(),
                0,
            )
        };
        if r != 0 {
            return None;
        }
        let len = u32::from_ne_bytes(buf[..4].try_into().ok()?) as usize;
        buf.truncate(len.min(buf.len()));
        Some(buf)
    }

    fn uuid(buf: &[u8]) -> Option<String> {
        let b = buf.get(4..20)?;
        if b.iter().all(|&x| x == 0) {
            return None;
        }
        let hex: String = b.iter().map(|x| format!("{x:02X}")).collect();
        Some(format!(
            "uuid:{}-{}-{}-{}-{}",
            &hex[..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..]
        ))
    }

    fn name(buf: &[u8]) -> Option<String> {
        let offset = i32::from_ne_bytes(buf.get(4..8)?.try_into().ok()?);
        let len = u32::from_ne_bytes(buf.get(8..12)?.try_into().ok()?) as usize;
        let start = 4usize.checked_add_signed(offset as isize)?;
        let bytes = buf.get(start..start + len)?;
        let name = String::from_utf8_lossy(bytes).trim_end_matches('\0').to_string();
        (!name.is_empty()).then_some(name)
    }
}

#[cfg(all(unix, not(target_vendor = "apple")))]
mod os {
    use std::io;
    use std::path::{Path, PathBuf};

    use super::{Mounted, name_of_mount, share_of};
    use crate::frames::sources::{Volume, VolumeKind};

    const SHARES: [&str; 6] = ["cifs", "smb3", "smbfs", "nfs", "nfs4", "fuse.sshfs"];

    /// One line of `/proc/self/mountinfo`: the mount point, its file
    /// system and its source.
    struct Mount {
        point: PathBuf,
        fs: String,
        source: String,
    }

    fn mounts() -> io::Result<Vec<Mount>> {
        let table = std::fs::read_to_string("/proc/self/mountinfo")?;
        Ok(table
            .lines()
            .filter_map(|line| {
                let (before, after) = line.split_once(" - ")?;
                let point = before.split(' ').nth(4)?;
                let mut after = after.split(' ');
                let fs = after.next()?.to_string();
                let source = unescaped(after.next()?);
                Some(Mount { point: PathBuf::from(unescaped(point)), fs, source })
            })
            .collect())
    }

    /// A mountinfo field with its octal escapes (`\040` for a space) undone.
    fn unescaped(field: &str) -> String {
        let bytes = field.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            let octal = bytes.get(i + 1..i + 4).and_then(|o| std::str::from_utf8(o).ok());
            match (bytes[i], octal.and_then(|o| u8::from_str_radix(o, 8).ok())) {
                (b'\\', Some(byte)) => {
                    out.push(byte);
                    i += 4;
                }
                (byte, _) => {
                    out.push(byte);
                    i += 1;
                }
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    /// The name in `/dev/disk/<dir>` whose link leads to the device `source`.
    fn link_to(dir: &str, source: &Path) -> Option<String> {
        let device = std::fs::canonicalize(source).ok()?;
        std::fs::read_dir(Path::new("/dev/disk").join(dir)).ok()?.flatten().find_map(|entry| {
            let to = std::fs::canonicalize(entry.path()).ok()?;
            (to == device).then(|| udev_unescaped(&entry.file_name().to_string_lossy()))
        })
    }

    /// A name in `/dev/disk` with udev's hex escapes (`\x20` for a space)
    /// undone.
    fn udev_unescaped(name: &str) -> String {
        let bytes = name.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            let hex = (bytes.get(i + 1) == Some(&b'x'))
                .then(|| bytes.get(i + 2..i + 4))
                .flatten()
                .and_then(|h| std::str::from_utf8(h).ok())
                .and_then(|h| u8::from_str_radix(h, 16).ok());
            match (bytes[i], hex) {
                (b'\\', Some(byte)) => {
                    out.push(byte);
                    i += 4;
                }
                (byte, _) => {
                    out.push(byte);
                    i += 1;
                }
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    fn of_mount(mount: &Mount) -> Mounted {
        let share = SHARES.contains(&mount.fs.as_str()).then(|| share_of(&mount.source));
        let volume = match share.flatten() {
            Some((server, share)) => Volume {
                id: None,
                name: share.rsplit('/').next().unwrap_or(&share).to_string(),
                kind: VolumeKind::Share { server, share },
            },
            None => {
                let source = Path::new(&mount.source);
                let id = link_to("by-uuid", source).map(|id| {
                    // A UUID is 36 characters; FAT, exFAT and NTFS have a
                    // serial instead.
                    if id.len() == 36 {
                        format!("uuid:{}", id.to_uppercase())
                    } else {
                        format!("serial:{}", id.to_uppercase())
                    }
                });
                let name =
                    link_to("by-label", source).unwrap_or_else(|| name_of_mount(&mount.point));
                Volume { id, name, kind: VolumeKind::Disk }
            }
        };
        Mounted { volume, mount: mount.point.clone() }
    }

    pub(super) fn volume(path: &Path) -> io::Result<Mounted> {
        std::fs::metadata(path)?;
        let mounts = mounts()?;
        let mount = mounts
            .iter()
            .filter(|m| path.starts_with(&m.point))
            .max_by_key(|m| m.point.components().count())
            .ok_or_else(|| io::Error::other(format!("no mount holds {}", path.display())))?;
        Ok(of_mount(mount))
    }

    pub(super) fn volumes() -> Vec<Mounted> {
        mounts()
            .unwrap_or_default()
            .iter()
            .filter(|m| m.source.starts_with("/dev/") || SHARES.contains(&m.fs.as_str()))
            .map(of_mount)
            .collect()
    }
}

#[cfg(windows)]
mod os {
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};

    use windows::Win32::Storage::FileSystem::{
        GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW, GetVolumePathNameW,
    };
    use windows::core::PCWSTR;

    use super::Mounted;
    use crate::frames::sources::{Volume, VolumeKind};

    /// `GetDriveTypeW`'s answer for a network drive.
    const DRIVE_REMOTE: u32 = 4;

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    fn text(buf: &[u16]) -> String {
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..end])
    }

    fn of_root(root: &str) -> io::Result<Mounted> {
        if let Some(unc) = root.strip_prefix(r"\\") {
            let mut parts = unc.split('\\').filter(|p| !p.is_empty());
            let (Some(server), Some(share)) = (parts.next(), parts.next()) else {
                return Err(io::Error::other(format!("{root} names no share")));
            };
            let volume = Volume {
                id: None,
                name: share.to_string(),
                kind: VolumeKind::Share { server: server.to_string(), share: share.to_string() },
            };
            return Ok(Mounted { volume, mount: PathBuf::from(format!(r"\\{server}\{share}")) });
        }
        let root_w = wide(Path::new(root));
        let mut name = [0u16; 261];
        let mut serial = 0u32;
        // SAFETY: `root_w` is NUL-terminated and outlives the call; the
        // buffers are writable for their lengths.
        unsafe {
            GetVolumeInformationW(
                PCWSTR(root_w.as_ptr()),
                Some(&mut name),
                Some(&mut serial),
                None,
                None,
                None,
            )
        }
        .map_err(|_| io::Error::last_os_error())?;
        let volume = Volume {
            id: Some(format!("serial:{:04X}-{:04X}", serial >> 16, serial & 0xffff)),
            name: text(&name),
            kind: VolumeKind::Disk,
        };
        Ok(Mounted { volume, mount: PathBuf::from(root) })
    }

    pub(super) fn volume(path: &Path) -> io::Result<Mounted> {
        let path_w = wide(path);
        let mut root = [0u16; 1024];
        // SAFETY: `path_w` is NUL-terminated; `root` is writable.
        unsafe { GetVolumePathNameW(PCWSTR(path_w.as_ptr()), &mut root) }
            .map_err(|_| io::Error::last_os_error())?;
        of_root(&text(&root))
    }

    pub(super) fn volumes() -> Vec<Mounted> {
        // SAFETY: no arguments.
        let drives = unsafe { GetLogicalDrives() };
        (0..26u8)
            .filter(|i| drives & (1 << i) != 0)
            .map(|i| format!("{}:\\", (b'A' + i) as char))
            .filter(|root| {
                let w = wide(Path::new(root));
                // SAFETY: `w` is NUL-terminated and outlives the call.
                let kind = unsafe { GetDriveTypeW(PCWSTR(w.as_ptr())) };
                kind != DRIVE_REMOTE
            })
            .filter_map(|root| of_root(&root).ok())
            .collect()
    }
}

#[cfg(not(any(unix, windows)))]
mod os {
    use std::io;
    use std::path::Path;

    use super::Mounted;

    pub(super) fn volume(path: &Path) -> io::Result<Mounted> {
        Err(io::Error::other(format!("no volumes on this platform: {}", path.display())))
    }

    pub(super) fn volumes() -> Vec<Mounted> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shares_are_named_by_server_and_share() {
        let pairs = [
            ("//me@nas.local/Photos", Some(("nas.local", "Photos"))),
            ("//nas/My%20Photos/", Some(("nas", "My Photos"))),
            ("nas:/export/photos", Some(("nas", "/export/photos"))),
            ("https://dav.example.com/remote/photos", Some(("dav.example.com", "remote/photos"))),
            ("/dev/disk4s1", None),
        ];
        for (source, want) in pairs {
            let got = share_of(source);
            assert_eq!(got.as_ref().map(|(a, b)| (a.as_str(), b.as_str())), want, "{source}");
        }
    }

    #[test]
    fn verbatim_prefixes_are_dropped() {
        assert_eq!(plain(Path::new(r"\\?\C:\Photos")), PathBuf::from(r"C:\Photos"));
        assert_eq!(plain(Path::new(r"\\?\UNC\nas\share\a")), PathBuf::from(r"\\nas\share\a"));
        assert_eq!(plain(Path::new("/Users/a")), PathBuf::from("/Users/a"));
    }

    /// A folder and a link to it are one; a folder's volume is mounted above
    /// it.
    #[cfg(unix)]
    #[test]
    fn the_disk_sees_one_folder_through_a_link() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, dir.path().join("link")).unwrap();
        let a = Disk.stat(&real).unwrap().unwrap();
        let b = Disk.stat(&dir.path().join("link")).unwrap().unwrap();
        assert!(a.folder && a.id.is_some());
        assert_eq!(a.id, b.id);
        let canon = Disk.canonical(&dir.path().join("link")).unwrap();
        assert_eq!(canon, Disk.canonical(&real).unwrap());
        let mounted = Disk.volume(&canon).unwrap();
        assert!(
            canon.starts_with(&mounted.mount),
            "{} on {}",
            canon.display(),
            mounted.mount.display()
        );
        assert!(!Disk.volumes().is_empty());
        assert_eq!(Disk.stat(&dir.path().join("none")).unwrap(), None);
    }
}
