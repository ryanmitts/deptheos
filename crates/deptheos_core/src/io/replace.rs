//! Writing a file in place of another safely: through a temporary beside
//! it, moved into place once its data is on disk.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::{CoreError, Result};

/// Write `path` by having `write` fill a temporary beside it, then moving
/// it over `path`. A failed write leaves `path` untouched.
///
/// - A symbolic link at `path` is written through and stays.
/// - A replaced file keeps its permissions, and its extended attributes
///   (Apple) or attributes, ACL and streams (Windows, `ReplaceFileW`).
/// - The data is flushed before the move and the folder after (on Windows
///   only for a new file), so a crash leaves the old file or the new,
///   never an empty one.
pub fn write_replacing(path: &Path, write: impl FnOnce(&Path) -> Result<()>) -> Result<()> {
    let target = resolve_links(path)?;
    let tmp = temporary_beside(&target)?;
    if let Err(e) = write(&tmp) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    let replacing = target.exists();
    let moved = move_into_place(&tmp, &target);
    if moved.is_err() {
        after_failed_move(&tmp, &target, replacing);
    }
    moved
}

/// Clear up after a failed move: remove `tmp`, unless the file it was
/// `replacing` is gone and `tmp` is the only copy (Windows' `ReplaceFileW`
/// can fail after removing the old file); the log says where it is.
fn after_failed_move(tmp: &Path, target: &Path, replacing: bool) {
    if replacing && !target.exists() && tmp.exists() {
        // One more try, now that whatever held the file has had longer.
        std::thread::sleep(std::time::Duration::from_millis(500));
        if rename_retrying(tmp, target).is_ok() {
            if let Err(e) = sync_folder(folder_of(target)) {
                tracing::warn!("{}: folder not flushed: {e}", target.display());
            }
            tracing::warn!("{}: moved into place at the last try", target.display());
            return;
        }
        tracing::error!(
            "{} is gone and its replacement could not be moved into place; the new file is kept as {}",
            target.display(),
            tmp.display()
        );
        return;
    }
    let _ = std::fs::remove_file(tmp);
}

/// `path` with any symbolic links at its final component followed (a
/// dangling link gives the path it points to, which the save creates).
fn resolve_links(path: &Path) -> Result<PathBuf> {
    // As the kernel's own limit on a chain of links (MAXSYMLINKS).
    const MAX_LINKS: usize = 32;
    let mut at = path.to_path_buf();
    for _ in 0..MAX_LINKS {
        match std::fs::symlink_metadata(&at) {
            Ok(meta) if meta.file_type().is_symlink() => {
                let link = std::fs::read_link(&at)?;
                at = match at.parent() {
                    Some(parent) if link.is_relative() => parent.join(link),
                    _ => link,
                };
            }
            _ => return Ok(at),
        }
    }
    Err(CoreError::SymlinkLoop { path: path.to_path_buf() })
}

/// Make the finished temporary file `tmp` durable and move it over
/// `target`, keeping what the file it replaces carried.
fn move_into_place(tmp: &Path, target: &Path) -> Result<()> {
    // Flush the data first: a rename can reach the disk before the data it
    // names (ext4, APFS), leaving an empty file after a crash.
    sync_file(&std::fs::OpenOptions::new().write(true).open(tmp)?)?;
    let existing = std::fs::metadata(target).ok().filter(std::fs::Metadata::is_file);
    if let Some(existing) = &existing {
        keep_attributes(existing, target, tmp);
    }
    replace_file(tmp, target, existing.is_some())?;
    Ok(sync_folder(folder_of(target))?)
}

/// Give `tmp` the permissions and extended attributes of `target`, best
/// effort (logged, not an error).
fn keep_attributes(existing: &std::fs::Metadata, target: &Path, tmp: &Path) {
    #[cfg(unix)]
    if let Err(e) = std::fs::set_permissions(tmp, existing.permissions()) {
        tracing::debug!("{}: permissions not kept: {e}", target.display());
    }
    #[cfg(target_vendor = "apple")]
    copy_xattrs(target, tmp);
    #[cfg(not(unix))]
    let _ = (existing, target, tmp);
}

/// Copy the extended attributes of `from` onto `to`, but the resource fork
/// and quarantine, which describe the old content.
#[cfg(target_vendor = "apple")]
fn copy_xattrs(from: &Path, to: &Path) {
    use std::ffi::{CStr, CString};
    use std::os::unix::ffi::OsStrExt;
    const SKIP: &[&[u8]] = &[b"com.apple.ResourceFork", b"com.apple.quarantine"];
    let (Ok(from_c), Ok(to_c)) =
        (CString::new(from.as_os_str().as_bytes()), CString::new(to.as_os_str().as_bytes()))
    else {
        return;
    };
    let options = libc::XATTR_NOFOLLOW;
    // SAFETY: each call gets a NUL-terminated path and a buffer of the
    // length it is told; a size query passes a null buffer and zero.
    let size = unsafe { libc::listxattr(from_c.as_ptr(), std::ptr::null_mut(), 0, options) };
    if size <= 0 {
        return;
    }
    let mut names = vec![0u8; size as usize];
    let size = unsafe {
        libc::listxattr(from_c.as_ptr(), names.as_mut_ptr().cast(), names.len(), options)
    };
    if size <= 0 {
        return;
    }
    names.truncate(size as usize);
    for name in names.split_inclusive(|&b| b == 0) {
        let Ok(name) = CStr::from_bytes_with_nul(name) else { continue };
        if SKIP.contains(&name.to_bytes()) {
            continue;
        }
        let len = unsafe {
            libc::getxattr(from_c.as_ptr(), name.as_ptr(), std::ptr::null_mut(), 0, 0, options)
        };
        if len < 0 {
            continue;
        }
        let mut value = vec![0u8; len as usize];
        let len = unsafe {
            libc::getxattr(
                from_c.as_ptr(),
                name.as_ptr(),
                value.as_mut_ptr().cast(),
                value.len(),
                0,
                options,
            )
        };
        if len < 0 {
            continue;
        }
        let set = unsafe {
            libc::setxattr(
                to_c.as_ptr(),
                name.as_ptr(),
                value.as_ptr().cast(),
                len as usize,
                0,
                options,
            )
        };
        if set != 0 {
            tracing::debug!(
                "{}: extended attribute {name:?} not kept: {}",
                from.display(),
                std::io::Error::last_os_error()
            );
        }
    }
}

/// Move `tmp` over `target`: on Windows over an `existing` file through
/// `ReplaceFileW` (no write-through, so a crash may leave the old file),
/// else [`rename_retrying`].
fn replace_file(tmp: &Path, target: &Path, existing: bool) -> Result<()> {
    #[cfg(windows)]
    if existing {
        use windows::Win32::Storage::FileSystem::{
            REPLACEFILE_IGNORE_ACL_ERRORS, REPLACEFILE_IGNORE_MERGE_ERRORS, ReplaceFileW,
        };
        use windows::core::PCWSTR;
        let (replaced, replacement) = (wide(target), wide(tmp));
        let flags = REPLACEFILE_IGNORE_MERGE_ERRORS | REPLACEFILE_IGNORE_ACL_ERRORS;
        // SAFETY: both are NUL-terminated wide strings that outlive the call.
        let replaced_ok = unsafe {
            ReplaceFileW(
                PCWSTR(replaced.as_ptr()),
                PCWSTR(replacement.as_ptr()),
                PCWSTR::null(),
                flags,
                None,
                None,
            )
        };
        match replaced_ok {
            Ok(()) => return Ok(()),
            // A file system without ReplaceFileW's support (some network
            // shares) still renames.
            Err(e) => tracing::debug!("{}: ReplaceFileW failed ({e}); renaming", target.display()),
        }
    }
    let _ = existing;
    Ok(rename_retrying(tmp, target)?)
}

/// Rename attempts: on Windows a virus scanner or indexer can briefly hold
/// a new file, denying access.
const RENAME_ATTEMPTS: u32 = if cfg!(windows) { 6 } else { 1 };

/// Rename `from` over `to`, retried from 10 ms, doubling, while access is
/// denied ([`RENAME_ATTEMPTS`]). Durable after [`sync_folder`] on POSIX;
/// written through on Windows.
pub fn rename_retrying(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut delay = std::time::Duration::from_millis(10);
    for attempt in 1.. {
        match rename(from, to) {
            Err(e)
                if e.kind() == std::io::ErrorKind::PermissionDenied
                    && attempt < RENAME_ATTEMPTS =>
            {
                std::thread::sleep(delay);
                delay *= 2;
            }
            result => return result,
        }
    }
    unreachable!("the attempts are bounded")
}

/// One try at [`rename_retrying`]'s rename.
fn rename(from: &Path, to: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::WIN32_ERROR;
        use windows::Win32::Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        };
        use windows::core::PCWSTR;
        let (from, to) = (wide(from), wide(to));
        let flags = MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH;
        // SAFETY: both are NUL-terminated wide strings that outlive the call.
        unsafe { MoveFileExW(PCWSTR(from.as_ptr()), PCWSTR(to.as_ptr()), flags) }.map_err(|e| {
            // As the Win32 error, so access denied is PermissionDenied.
            let code = WIN32_ERROR::from_error(&e).map_or(e.code().0, |code| code.0 as i32);
            std::io::Error::from_raw_os_error(code)
        })
    }
    #[cfg(not(windows))]
    std::fs::rename(from, to)
}

/// `path` as a NUL-terminated wide string, for a Win32 call.
#[cfg(windows)]
fn wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

/// Whether `e` is a file system refusing a kind of flush (exFAT, SMB, NFS),
/// rather than a flush that failed.
fn refused(e: &std::io::Error) -> bool {
    #[cfg(unix)]
    return e
        .raw_os_error()
        .is_some_and(|c| [libc::EINVAL, libc::ENOTSUP, libc::EOPNOTSUPP].contains(&c));
    #[cfg(not(unix))]
    {
        let _ = e;
        false
    }
}

/// Flush `file` to disk (F_FULLFSYNC on Apple), falling back to a plain
/// `fsync` where the file system refuses it. Any other failure is an error.
pub fn sync_file(file: &std::fs::File) -> std::io::Result<()> {
    #[cfg(unix)]
    let plain = || {
        use std::os::fd::AsRawFd as _;
        // SAFETY: the descriptor is `file`'s, open for the whole call.
        match unsafe { libc::fsync(file.as_raw_fd()) } {
            0 => Ok(()),
            _ => Err(std::io::Error::last_os_error()),
        }
    };
    #[cfg(not(unix))]
    let plain = || file.sync_all();
    sync_falling_back(|| file.sync_all(), plain)
}

/// `full`, and when the file system refuses it, `plain`; one that refuses
/// that too counts as flushed, as far as it can be there.
fn sync_falling_back(
    full: impl FnOnce() -> std::io::Result<()>,
    plain: impl FnOnce() -> std::io::Result<()>,
) -> std::io::Result<()> {
    match full() {
        Err(e) if refused(&e) => {
            tracing::debug!("full flush refused ({e}): a plain one instead");
            match plain() {
                Err(e) if refused(&e) => {
                    tracing::debug!("file not flushed: {e}");
                    Ok(())
                }
                result => result,
            }
        }
        result => result,
    }
}

/// Flush `folder` so new or renamed entries survive a crash; a refusal
/// counts as flushed, any other failure is an error. POSIX only (on
/// Windows [`rename_retrying`] writes through).
pub fn sync_folder(folder: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let folder = if folder.as_os_str().is_empty() { Path::new(".") } else { folder };
        match std::fs::File::open(folder).and_then(|f| f.sync_all()) {
            Err(e) if refused(&e) => {
                tracing::debug!("{}: folder not flushed: {e}", folder.display())
            }
            result => return result,
        }
    }
    #[cfg(not(unix))]
    let _ = folder;
    Ok(())
}

/// The folder holding `path`.
fn folder_of(path: &Path) -> &Path {
    path.parent().unwrap_or(Path::new(""))
}

/// Whether the file name `name` is one of [`write_replacing`]'s
/// temporaries (`.<name>.<pid>-<n>.tmp`): a crash mid-save may leave one.
pub fn is_temporary(name: &str) -> bool {
    let Some(rest) = name.strip_prefix('.').and_then(|n| n.strip_suffix(".tmp")) else {
        return false;
    };
    rest.rsplit_once('.').and_then(|(_, tag)| tag.split_once('-')).is_some_and(|(pid, n)| {
        [pid, n].iter().all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// A hidden file name next to `path`, unique to this process and call
/// ([`is_temporary`] knows it).
fn temporary_beside(path: &Path) -> Result<PathBuf> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = path.file_name().ok_or_else(|| CoreError::NotAFile { path: path.to_path_buf() })?;
    let mut tmp = std::ffi::OsString::from(".");
    tmp.push(name);
    tmp.push(format!(".{}-{}.tmp", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    Ok(path.with_file_name(tmp))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temporaries_are_known_by_name() {
        let tmp = temporary_beside(Path::new("/x/paint.json")).unwrap();
        assert!(is_temporary(&tmp.file_name().unwrap().to_string_lossy()));
        for other in ["paint.json", ".paint.json", "a.tmp", ".a.tmp", ".a.1-.tmp", ".a.x-2.tmp"] {
            assert!(!is_temporary(other), "{other}");
        }
    }

    /// A refused full flush falls back to a plain one; a refused plain one
    /// counts as flushed; any other failure is an error, with no fallback.
    #[cfg(unix)]
    #[test]
    fn a_refused_full_flush_falls_back_to_a_plain_one() {
        use std::cell::Cell;
        let os = std::io::Error::from_raw_os_error;
        for refusal in [libc::ENOTSUP, libc::EINVAL] {
            let plain = Cell::new(false);
            let flushed = sync_falling_back(
                || Err(os(refusal)),
                || {
                    plain.set(true);
                    Ok(())
                },
            );
            assert!(flushed.is_ok() && plain.get(), "{refusal}");
            assert!(sync_falling_back(|| Err(os(refusal)), || Err(os(refusal))).is_ok());
            let gone = sync_falling_back(|| Err(os(refusal)), || Err(os(libc::EIO)));
            assert_eq!(gone.unwrap_err().raw_os_error(), Some(libc::EIO));
        }
        for gone in [libc::EIO, libc::ENXIO, libc::ENOENT] {
            let plain = Cell::new(false);
            let flushed = sync_falling_back(
                || Err(os(gone)),
                || {
                    plain.set(true);
                    Ok(())
                },
            );
            assert_eq!(flushed.unwrap_err().raw_os_error(), Some(gone));
            assert!(!plain.get(), "no plain flush after a failure");
        }
        let dir = tempfile::tempdir().unwrap();
        let file = std::fs::File::create(dir.path().join("a")).unwrap();
        sync_file(&file).unwrap();
    }

    /// A folder that can't be flushed is an error.
    #[test]
    fn a_folder_not_flushed_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        sync_folder(dir.path()).unwrap();
        sync_folder(Path::new("")).unwrap();
        #[cfg(unix)]
        assert!(sync_folder(&dir.path().join("gone")).is_err());
    }

    /// After a failed move the temporary goes, unless it's the only copy.
    #[test]
    fn a_failed_move_never_removes_the_only_copy() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("out.tif");
        let tmp = temporary_beside(&target).unwrap();

        // The old file is still there: the temporary one is removed.
        std::fs::write(&target, b"old").unwrap();
        std::fs::write(&tmp, b"new").unwrap();
        after_failed_move(&tmp, &target, true);
        assert!(!tmp.exists());
        assert_eq!(std::fs::read(&target).unwrap(), b"old");

        // A new file that could not be put in place: nothing to keep.
        std::fs::remove_file(&target).unwrap();
        std::fs::write(&tmp, b"new").unwrap();
        after_failed_move(&tmp, &target, false);
        assert!(!tmp.exists() && !target.exists());

        // The old file went and the new one is all there is: it is kept,
        // here by the last try at moving it into place.
        std::fs::write(&tmp, b"new").unwrap();
        after_failed_move(&tmp, &target, true);
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
    }
}
