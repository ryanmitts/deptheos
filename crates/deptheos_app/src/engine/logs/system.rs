//! What a log's header says about the machine: the OS and its version, the
//! model, CPUs, memory and locale. The GPU is logged by its context.

/// The header's lines, as (key, value).
pub fn describe() -> Vec<(&'static str, String)> {
    let mut lines = vec![("os", format!("{} ({})", os_version(), std::env::consts::ARCH))];
    if let Some(model) = model() {
        lines.push(("model", model));
    }
    let cpus = std::thread::available_parallelism().map_or(0, |n| n.get());
    lines.push(("cpus", cpus.to_string()));
    let memory = crate::engine::memory::physical() as f64 / f64::from(1u32 << 30);
    lines.push(("memory", format!("{memory:.0} GB")));
    lines.push(("locale", sys_locale::get_locale().unwrap_or_else(|| "unknown".into())));
    lines.push(("pid", std::process::id().to_string()));
    lines
}

/// "macOS 15.2", "iOS 18.1", "Windows 10.0.26100", "Ubuntu 24.04.1 LTS
/// (Linux 6.8.0-45-generic)".
pub fn os_version() -> String {
    #[cfg(target_os = "macos")]
    return format!("macOS {}", sysctl_string(c"kern.osproductversion").unwrap_or_default());
    #[cfg(target_os = "ios")]
    return format!("iOS {}", sysctl_string(c"kern.osproductversion").unwrap_or_default());
    #[cfg(windows)]
    return windows_version();
    #[cfg(target_os = "linux")]
    return linux_version();
    #[allow(unreachable_code)]
    std::env::consts::OS.to_owned()
}

/// The hardware model: "Mac15,8", "iPhone17,1". `None` elsewhere.
fn model() -> Option<String> {
    #[cfg(target_os = "macos")]
    return sysctl_string(c"hw.model");
    // On iOS `hw.model` is the board ("D47AP"); `hw.machine` the model.
    // The Simulator's `hw.machine` is the Mac's ("arm64"); it names the
    // device it simulates in the environment.
    #[cfg(target_os = "ios")]
    return std::env::var("SIMULATOR_MODEL_IDENTIFIER")
        .ok()
        .map(|model| format!("{model} (Simulator)"))
        .or_else(|| sysctl_string(c"hw.machine"));
    #[allow(unreachable_code)]
    None
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn sysctl_string(name: &std::ffi::CStr) -> Option<String> {
    let mut len = 0usize;
    // SAFETY: a null buffer asks for the length only.
    let ok = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            std::ptr::null_mut(),
            &raw mut len,
            std::ptr::null_mut(),
            0,
        )
    } == 0;
    if !ok || len == 0 {
        return None;
    }
    let mut buf = vec![0u8; len];
    // SAFETY: `buf` holds `len` bytes, and `len` says so.
    let ok = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            buf.as_mut_ptr().cast(),
            &raw mut len,
            std::ptr::null_mut(),
            0,
        )
    } == 0;
    if !ok {
        return None;
    }
    buf.truncate(len);
    let text = std::ffi::CStr::from_bytes_until_nul(&buf).ok()?.to_string_lossy().into_owned();
    (!text.is_empty()).then_some(text)
}

#[cfg(target_os = "linux")]
fn linux_version() -> String {
    let pretty = std::fs::read_to_string("/etc/os-release").ok().and_then(|text| {
        text.lines()
            .find_map(|line| line.strip_prefix("PRETTY_NAME="))
            .map(|v| v.trim().trim_matches('"').to_owned())
    });
    // SAFETY: `uname` fills the zeroed struct it is given.
    let kernel = unsafe {
        let mut name: libc::utsname = std::mem::zeroed();
        (libc::uname(&mut name) == 0)
            .then(|| std::ffi::CStr::from_ptr(name.release.as_ptr()).to_string_lossy().into_owned())
    };
    match (pretty, kernel) {
        (Some(pretty), Some(kernel)) => format!("{pretty} (Linux {kernel})"),
        (None, Some(kernel)) => format!("Linux {kernel}"),
        (Some(pretty), None) => pretty,
        (None, None) => "Linux".to_owned(),
    }
}

/// The real version (`GetVersionEx` says 6.2 without a manifest). Build
/// 22000 and up is Windows 11.
#[cfg(windows)]
fn windows_version() -> String {
    use ::windows::Wdk::System::SystemServices::RtlGetVersion;
    use ::windows::Win32::System::SystemInformation::OSVERSIONINFOW;
    let mut info = OSVERSIONINFOW {
        dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
        ..Default::default()
    };
    // SAFETY: `info` is an OSVERSIONINFOW with its size set, as asked.
    if unsafe { RtlGetVersion(&mut info) }.is_err() {
        return "Windows".to_owned();
    }
    format!("Windows {}.{}.{}", info.dwMajorVersion, info.dwMinorVersion, info.dwBuildNumber)
}

#[cfg(test)]
mod tests {
    #[test]
    fn describes_the_machine() {
        let lines = super::describe();
        let os = &lines.iter().find(|(k, _)| *k == "os").unwrap().1;
        assert!(os.contains(std::env::consts::ARCH), "{os}");
        #[cfg(target_os = "macos")]
        assert!(os.starts_with("macOS 1") || os.starts_with("macOS 2"), "{os}");
        assert!(lines.iter().any(|(k, _)| *k == "locale"));
    }
}
