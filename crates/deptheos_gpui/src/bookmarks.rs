//! Bookmarks of the Library's top roots on macOS (ADR-145), so a folder
//! renamed or moved on its volume is found again at launch. Plain, not
//! security-scoped: the app isn't sandboxed.
//!
//! Made when a root is added or moves
//! ([`deptheos_app::Frames::make_bookmarks_with`]) on the store's worker.
//! Resolved off the UI thread, each on its own thread, within
//! [`RESOLVE_WITHIN`], never mounting or asking; a stale one is remade.
//! Other platforms keep none.

use std::time::Duration;

use deptheos_app::frames::sources::{Bookmark, Resolution};
use deptheos_app::{RootId, Sources};

/// How long resolving all bookmarks may take before stragglers are given up:
/// a silent share mustn't hold up the Library.
pub const RESOLVE_WITHIN: Duration = Duration::from_secs(5);

/// A bookmark of `folder`, where the platform makes them.
pub fn make(folder: &std::path::Path) -> Option<Bookmark> {
    #[cfg(target_os = "macos")]
    {
        let made = mac::make(folder);
        if made.is_none() {
            tracing::debug!("bookmarks: none made for a root's folder");
        }
        made
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = folder;
        None
    }
}

/// Where each root's bookmark leads now, for
/// [`deptheos_app::Frames::find_again`]: those resolved within `within`,
/// stale ones remade. Blocks up to `within`; run on a worker.
pub fn resolve(sources: &Sources, within: Duration) -> Vec<Resolution> {
    let bookmarks: Vec<(RootId, Bookmark)> = sources
        .roots()
        .filter_map(|(id, _)| Some((id.clone(), sources.bookmark(id)?.clone())))
        .collect();
    if bookmarks.is_empty() {
        return Vec::new();
    }
    let asked = bookmarks.len();
    let (send, receive) = std::sync::mpsc::channel();
    for (root, bookmark) in bookmarks {
        let send = send.clone();
        // A thread each, so a hung one holds up no other; late ones finish
        // on their own.
        let spawned = std::thread::Builder::new().name("bookmark".into()).spawn(move || {
            let _ = send.send(resolve_one(root, &bookmark));
        });
        if let Err(e) = spawned {
            tracing::warn!("bookmarks: couldn't start resolving one: {e}");
        }
    }
    drop(send);
    let deadline = std::time::Instant::now() + within;
    let mut resolved = Vec::new();
    let mut answered = 0;
    while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
        match receive.recv_timeout(left) {
            Ok(one) => {
                answered += 1;
                resolved.extend(one);
            }
            Err(_) => break,
        }
    }
    if answered < asked {
        tracing::info!("bookmarks: {} of {asked} not resolved in time", asked - answered);
    }
    tracing::debug!("bookmarks: {} of {asked} resolved", resolved.len());
    resolved
}

/// Where `bookmark` leads, made again there when it was stale.
fn resolve_one(root: RootId, bookmark: &Bookmark) -> Option<Resolution> {
    #[cfg(target_os = "macos")]
    {
        let (path, stale) = mac::resolve(bookmark)?;
        let fresh = if stale { make(&path) } else { None };
        Some(Resolution { root, path, fresh })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (root, bookmark);
        None
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::{CStr, CString, c_char, c_void};
    use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
    use std::path::{Path, PathBuf};

    use deptheos_app::frames::sources::Bookmark;
    use objc2::rc::{Retained, autoreleasepool};
    use objc2::runtime::{AnyObject, Bool};
    use objc2::{class, msg_send};

    /// `NSURLBookmarkResolutionWithoutUI`: never ask the user.
    const WITHOUT_UI: usize = 1 << 8;
    /// `NSURLBookmarkResolutionWithoutMounting`: never mount a volume.
    const WITHOUT_MOUNTING: usize = 1 << 9;

    /// `NSURL`'s plain bookmark of `folder`.
    pub fn make(folder: &Path) -> Option<Bookmark> {
        let path = CString::new(folder.as_os_str().as_bytes()).ok()?;
        autoreleasepool(|_| {
            // SAFETY: Foundation's documented signatures; every pointer
            // passed lives through the call, and what comes back is
            // retained or copied out before the pool drains.
            unsafe {
                let url: Option<Retained<AnyObject>> = msg_send![
                    class!(NSURL),
                    fileURLWithFileSystemRepresentation: path.as_ptr(),
                    isDirectory: Bool::YES,
                    relativeToURL: None::<&AnyObject>
                ];
                let url = url?;
                let data: Option<Retained<AnyObject>> = msg_send![
                    &url,
                    bookmarkDataWithOptions: 0usize,
                    includingResourceValuesForKeys: None::<&AnyObject>,
                    relativeToURL: None::<&AnyObject>,
                    error: std::ptr::null_mut::<*mut AnyObject>()
                ];
                Some(Bookmark(bytes(&*data?)))
            }
        })
    }

    /// Where `bookmark` leads and whether stale; `None` if unreachable
    /// without mounting or asking.
    pub fn resolve(bookmark: &Bookmark) -> Option<(PathBuf, bool)> {
        autoreleasepool(|_| {
            // SAFETY: as in `make`; `bookmark.0` outlives the NSData made
            // from it, which copies the bytes.
            unsafe {
                let data: Option<Retained<AnyObject>> = msg_send![
                    class!(NSData),
                    dataWithBytes: bookmark.0.as_ptr().cast::<c_void>(),
                    length: bookmark.0.len()
                ];
                let mut stale = Bool::NO;
                let url: Option<Retained<AnyObject>> = msg_send![
                    class!(NSURL),
                    URLByResolvingBookmarkData: &*data?,
                    options: WITHOUT_UI | WITHOUT_MOUNTING,
                    relativeToURL: None::<&AnyObject>,
                    bookmarkDataIsStale: &mut stale,
                    error: std::ptr::null_mut::<*mut AnyObject>()
                ];
                let url = url?;
                let path: *const c_char = msg_send![&url, fileSystemRepresentation];
                if path.is_null() {
                    return None;
                }
                let path = CStr::from_ptr(path).to_bytes().to_vec();
                Some((PathBuf::from(std::ffi::OsString::from_vec(path)), stale.as_bool()))
            }
        })
    }

    /// An `NSData`'s bytes.
    ///
    /// # Safety
    /// `data` is an `NSData`.
    unsafe fn bytes(data: &AnyObject) -> Vec<u8> {
        // SAFETY: an NSData's `bytes` are `length` long while it lives.
        unsafe {
            let length: usize = msg_send![data, length];
            let start: *const c_void = msg_send![data, bytes];
            if start.is_null() || length == 0 {
                return Vec::new();
            }
            std::slice::from_raw_parts(start.cast::<u8>(), length).to_vec()
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// A folder renamed is where its bookmark leads.
        #[test]
        fn a_bookmark_follows_its_folder_renamed() {
            let dir = tempfile::tempdir().unwrap();
            let (was, now) = (dir.path().join("card"), dir.path().join("Bee"));
            std::fs::create_dir_all(&was).unwrap();
            let bookmark = make(&was).expect("a bookmark");
            std::fs::rename(&was, &now).unwrap();
            let (path, _) = resolve(&bookmark).expect("resolved");
            assert_eq!(std::fs::canonicalize(path).unwrap(), std::fs::canonicalize(&now).unwrap());
            std::fs::remove_dir(&now).unwrap();
            assert!(resolve(&bookmark).is_none(), "a folder gone leads nowhere");
            assert!(resolve(&Bookmark(vec![1, 2, 3])).is_none());
        }
    }
}
