//! Where the app's own code is loaded, for a panic's addresses: a release
//! build ships without debug info (kept per release), so the panic hook
//! writes each frame's image and offset and each image's load address, for
//! `atos -o <dSYM> -l <load address>` or `llvm-symbolizer`.

use std::fmt::Write as _;

/// A loaded image of the app's own: its file name and load address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Image {
    pub(super) name: String,
    pub(super) base: usize,
}

/// The images loaded from the app's own folder (the bundle on macOS and
/// iOS), the executable first. Only the executable on Windows.
pub(super) fn ours() -> Vec<Image> {
    let Ok(exe) = std::env::current_exe() else { return Vec::new() };
    // macOS: Contents/MacOS/<exe>, with Contents/Frameworks beside it.
    let folder = if cfg!(target_os = "macos") {
        exe.parent().and_then(|p| p.parent())
    } else {
        exe.parent()
    };
    let Some(folder) = folder else { return Vec::new() };
    let mut images = loaded(&exe);
    images.retain(|(path, _)| path.starts_with(folder) || path == &exe);
    images
        .into_iter()
        .map(|(path, base)| Image {
            name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            base,
        })
        .collect()
}

// libc marks dyld's calls deprecated in favor of the mach2 crate, which
// makes the same calls.
#[cfg(any(target_os = "macos", target_os = "ios"))]
#[allow(deprecated)]
fn loaded(_exe: &std::path::Path) -> Vec<(std::path::PathBuf, usize)> {
    let mut images = Vec::new();
    // SAFETY: dyld's own listing; an index past the count gives null.
    unsafe {
        for i in 0..libc::_dyld_image_count() {
            let (header, name) = (libc::_dyld_get_image_header(i), libc::_dyld_get_image_name(i));
            if header.is_null() || name.is_null() {
                continue;
            }
            let name = std::ffi::CStr::from_ptr(name).to_string_lossy().into_owned();
            images.push((name.into(), header as usize));
        }
    }
    images
}

#[cfg(target_os = "linux")]
fn loaded(exe: &std::path::Path) -> Vec<(std::path::PathBuf, usize)> {
    unsafe extern "C" fn each(
        info: *mut libc::dl_phdr_info,
        _size: libc::size_t,
        data: *mut libc::c_void,
    ) -> libc::c_int {
        // SAFETY: the loader hands a valid entry, and `data` is the Vec
        // passed below.
        unsafe {
            let images = &mut *data.cast::<Vec<(std::path::PathBuf, usize)>>();
            let info = &*info;
            let name = if info.dlpi_name.is_null() {
                String::new()
            } else {
                std::ffi::CStr::from_ptr(info.dlpi_name).to_string_lossy().into_owned()
            };
            images.push((name.into(), info.dlpi_addr as usize));
        }
        0
    }
    let mut images: Vec<(std::path::PathBuf, usize)> = Vec::new();
    // SAFETY: `each` matches the callback's type and only reads its entry.
    unsafe { libc::dl_iterate_phdr(Some(each), (&raw mut images).cast()) };
    // The executable comes first, unnamed.
    if let Some(first) = images.first_mut()
        && first.0.as_os_str().is_empty()
    {
        first.0 = exe.to_path_buf();
    }
    images
}

#[cfg(windows)]
fn loaded(exe: &std::path::Path) -> Vec<(std::path::PathBuf, usize)> {
    use ::windows::Win32::System::LibraryLoader::GetModuleHandleW;
    // SAFETY: a null name asks for the executable's own module.
    match unsafe { GetModuleHandleW(::windows::core::PCWSTR::null()) } {
        Ok(module) => vec![(exe.to_path_buf(), module.0 as usize)],
        Err(_) => Vec::new(),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "linux", windows)))]
fn loaded(_exe: &std::path::Path) -> Vec<(std::path::PathBuf, usize)> {
    Vec::new()
}

/// The frames' addresses with their image and offset, then the images'
/// load addresses.
pub(super) fn describe(frames: &[usize], images: &[Image]) -> String {
    let mut text = String::from("addresses:\n");
    for (i, &ip) in frames.iter().enumerate() {
        let image = images.iter().filter(|im| im.base <= ip).max_by_key(|im| im.base);
        let _ = match image {
            Some(im) => writeln!(text, "{i:4}: {ip:#x} {}+{:#x}", im.name, ip - im.base),
            None => writeln!(text, "{i:4}: {ip:#x}"),
        };
    }
    text.push_str("images:\n");
    for im in images {
        let _ = writeln!(text, "  {} {:#x}", im.name, im.base);
    }
    text
}

/// This thread's frames, as instruction addresses.
pub(super) fn frames() -> Vec<usize> {
    let mut frames = Vec::new();
    backtrace::trace(|frame| {
        frames.push(frame.ip() as usize);
        frames.len() < 128
    });
    frames
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_placed_in_their_images() {
        let images = [
            Image { name: "deptheos-gui".into(), base: 0x1_0000_0000 },
            Image { name: "libengine.dylib".into(), base: 0x1_2000_0000 },
        ];
        let text = describe(&[0x1_0000_3c2c, 0x1_2000_0010, 0x10], &images);
        assert_eq!(
            text,
            "addresses:\n   0: 0x100003c2c deptheos-gui+0x3c2c\n   1: 0x120000010 \
             libengine.dylib+0x10\n   2: 0x10\nimages:\n  deptheos-gui 0x100000000\n  \
             libengine.dylib 0x120000000\n"
        );
    }

    /// The test binary is found, loaded where this function's code is.
    #[test]
    fn the_executable_is_among_its_images() {
        let images = ours();
        let exe = std::env::current_exe().unwrap();
        let name = exe.file_name().unwrap().to_string_lossy();
        let found = images.iter().find(|im| im.name == name).expect("the executable");
        let here = frames_are_placed_in_their_images as fn() as usize;
        assert!(found.base <= here, "{images:?}");
        assert!(!frames().is_empty());
    }
}
