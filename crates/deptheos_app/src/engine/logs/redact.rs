//! Keeps the account out of what a user sends: the home folder written
//! `~`, the user's name out of Linux card mounts, and machine ids out of
//! crash reports.
//!
//! The home is matched however it is spelled: with `/private` before it,
//! with forward slashes, JSON-escaped (`\/Users\/name`) or `{:?}`-escaped
//! (`C:\\Users\\name`). Matching ignores ASCII case and must end where the
//! name does (`/Users/ann` is not in `/Users/anna`).

use std::borrow::Cow;
use std::sync::OnceLock;

/// What a user's mount of a card is written as, in place of their name.
const USER: &str = "<user>";

/// The needles a text is searched for, each with what replaces it.
pub(super) struct Redactor {
    /// (ASCII-lowercased needle, replacement), longest first, so
    /// `/private/var/…` goes before `/var/…`.
    needles: Vec<(String, String)>,
}

impl Redactor {
    /// For `home` and the account name `user`. A home of `/` is skipped, or
    /// every path would become `~`.
    pub(super) fn new(home: &str, user: Option<&str>) -> Self {
        let mut needles = Vec::new();
        let home = home.trim_end_matches(['/', '\\']);
        if home.len() >= 2 && (home.starts_with('/') || home.contains(':')) {
            let forward = home.replace('\\', "/");
            for spelled in [
                format!("/private{forward}"),
                forward.clone(),
                home.to_owned(),
                // JSON's escaped slashes.
                format!("/private{forward}").replace('/', "\\/"),
                forward.replace('/', "\\/"),
                // `{:?}`'s doubled backslashes.
                home.replace('\\', "\\\\"),
            ] {
                needles.push((spelled.to_ascii_lowercase(), "~".to_owned()));
            }
        }
        if let Some(user) = user.filter(|u| !u.is_empty() && !u.contains(['/', '\\'])) {
            // `/run/media/<user>` holds `/media/<user>`.
            needles.push((format!("/media/{user}").to_ascii_lowercase(), format!("/media/{USER}")));
        }
        needles.sort_by_key(|(needle, _)| std::cmp::Reverse(needle.len()));
        needles.dedup();
        Self { needles }
    }

    /// `text` with every needle replaced; borrowed when none is in it.
    pub(super) fn redact<'a>(&self, text: &'a str) -> Cow<'a, str> {
        if self.needles.is_empty() || !text.contains(['/', '\\']) {
            return Cow::Borrowed(text);
        }
        let lower = text.to_ascii_lowercase();
        if !self.needles.iter().any(|(needle, _)| lower.contains(needle.as_str())) {
            return Cow::Borrowed(text);
        }
        // ASCII lowercasing keeps byte offsets, so ranges carry over.
        let (bytes, lower) = (text.as_bytes(), lower.as_bytes());
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        let mut changed = false;
        'scan: while i < bytes.len() {
            for (needle, replacement) in &self.needles {
                let end = i + needle.len();
                if lower[i..].starts_with(needle.as_bytes()) && ends_a_name(bytes.get(end)) {
                    out.extend_from_slice(replacement.as_bytes());
                    i = end;
                    changed = true;
                    continue 'scan;
                }
            }
            out.push(bytes[i]);
            i += 1;
        }
        if !changed {
            return Cow::Borrowed(text);
        }
        // Needles start and end on whole characters, so this holds.
        Cow::Owned(
            String::from_utf8(out)
                .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()),
        )
    }
}

/// Whether the byte after a match ends the name, not a longer name's letter.
fn ends_a_name(next: Option<&u8>) -> bool {
    next.is_none_or(|b| !(b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.')))
}

/// The process's redactor: its home folder and account name.
fn redactor() -> &'static Redactor {
    static REDACTOR: OnceLock<Redactor> = OnceLock::new();
    REDACTOR.get_or_init(|| {
        let home = dirs::home_dir();
        let user = home.as_ref().and_then(|h| h.file_name()).map(|n| n.to_string_lossy());
        let home = home.as_ref().map(|h| h.to_string_lossy()).unwrap_or_default();
        Redactor::new(&home, user.as_deref())
    })
}

/// `text` with the home written `~` and card mounts without the account
/// name. On iOS the home is the app's container.
pub(super) fn redact(text: &str) -> Cow<'_, str> {
    redactor().redact(text)
}

/// Crash report keys whose values identify this machine or boot. Image
/// `uuid`s stay, as symbolication needs them.
const MACHINE_IDS: &[&str] = &[
    "crashReporterKey",
    "sleepWakeUUID",
    "bootSessionUUID",
    "deviceIdentifierForVendor",
    "deviceIdentifierForAdvertising",
];

/// A macOS `.ips` `report` with [`MACHINE_IDS`]' values blanked.
pub(super) fn blank_machine_ids(report: &str) -> Cow<'_, str> {
    let mut out: Cow<'_, str> = Cow::Borrowed(report);
    for key in MACHINE_IDS {
        let quoted = format!("\"{key}\"");
        let mut from = 0;
        while let Some(at) = out[from..].find(&quoted).map(|at| from + at) {
            let after = at + quoted.len();
            let Some(value) = string_value(&out[after..]) else {
                from = after;
                continue;
            };
            let (start, end) = (after + value.start, after + value.end);
            out.to_mut().replace_range(start..end, "");
            from = start;
        }
    }
    out
}

/// The quoted string after a key's `:`; `rest` begins after the key.
fn string_value(rest: &str) -> Option<std::ops::Range<usize>> {
    let colon = rest.len() - rest.trim_start().len();
    let rest_after_colon = rest[colon..].strip_prefix(':')?;
    let start = rest.len() - rest_after_colon.trim_start().len();
    if !rest[start..].starts_with('"') {
        return None;
    }
    let start = start + 1;
    let end = start + rest[start..].find('"')?;
    Some(start..end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_home_is_written_as_a_tilde() {
        let home = dirs::home_dir().unwrap();
        let path = home.join("Pictures").join("Hornet").display().to_string();
        let redacted = redact(&path);
        assert!(!redacted.contains(&*home.to_string_lossy()), "{redacted}");
        assert!(redacted.starts_with('~'), "{redacted}");
        assert!(matches!(redact("/Volumes/Card/DCIM"), Cow::Borrowed(_)));
    }

    #[test]
    fn macos_homes_in_every_spelling() {
        let r = Redactor::new("/Users/ann", Some("ann"));
        assert_eq!(r.redact("read /Users/ann/Pictures/a.cr3"), "read ~/Pictures/a.cr3");
        assert_eq!(r.redact("at /private/Users/ann/x"), "at ~/x");
        assert_eq!(r.redact("\"path\":\"\\/Users\\/ann\\/Library\""), "\"path\":\"~\\/Library\"");
        assert_eq!(r.redact("/USERS/Ann/Desktop"), "~/Desktop");
        assert_eq!(r.redact("\"/Users/ann\""), "\"~\"");
        // Another account whose name starts the same is not this one.
        assert_eq!(r.redact("/Users/anna/x"), "/Users/anna/x");
        assert!(matches!(r.redact("/Volumes/Card"), Cow::Borrowed(_)));
    }

    #[test]
    fn linux_card_mounts_lose_the_name() {
        let r = Redactor::new("/home/ann", Some("ann"));
        assert_eq!(r.redact("/media/ann/EOS_DIGITAL/DCIM"), "/media/<user>/EOS_DIGITAL/DCIM");
        assert_eq!(r.redact("/run/media/Ann/CARD"), "/run/media/<user>/CARD");
        assert_eq!(r.redact("/home/ann/.local/state"), "~/.local/state");
        assert_eq!(r.redact("/media/annex/CARD"), "/media/annex/CARD");
    }

    #[test]
    fn windows_homes_as_written_and_as_debug() {
        let r = Redactor::new("C:\\Users\\Ann", Some("Ann"));
        assert_eq!(r.redact("C:\\Users\\Ann\\Pictures\\a.cr3"), "~\\Pictures\\a.cr3");
        assert_eq!(r.redact("c:\\users\\ann\\Pictures"), "~\\Pictures");
        assert_eq!(r.redact("path: \"C:\\\\Users\\\\Ann\\\\Pictures\""), "path: \"~\\\\Pictures\"");
        assert_eq!(r.redact("C:/Users/Ann/Pictures"), "~/Pictures");
        assert_eq!(r.redact("\\\\?\\C:\\Users\\Ann\\x"), "\\\\?\\~\\x");
    }

    #[test]
    fn no_home_redacts_nothing() {
        let r = Redactor::new("/", None);
        assert!(matches!(r.redact("/Users/ann"), Cow::Borrowed(_)));
    }

    /// A trimmed macOS crash report, the home JSON-escaped.
    const IPS: &str = r#"{"app_name":"deptheos-gui","timestamp":"2026-11-10 12:00:00.00 -0800","app_version":"0.2.0","slice_uuid":"5f0c2a7e-1d3b-3c4f-9a8e-7b6d5c4e3f21","build_version":"12","platform":1,"bundleID":"com.deptheos.Deptheos","share_with_app_devs":0,"is_first_party":0,"bug_type":"309","os_version":"macOS 26.1 (25B78)","roots_installed":0,"name":"deptheos-gui","incident_id":"0F3A9C1E-2B4D-4E6F-8A1B-3C5D7E9F0A2B"}
{
  "uptime" : 12000,
  "procRole" : "Foreground",
  "version" : 2,
  "userID" : 501,
  "deployVersion" : 210,
  "modelCode" : "Mac15,8",
  "coalitionID" : 1234,
  "incident" : "0F3A9C1E-2B4D-4E6F-8A1B-3C5D7E9F0A2B",
  "pid" : 4321,
  "cpuType" : "ARM-64",
  "procName" : "deptheos-gui",
  "procPath" : "\/Users\/ann\/Applications\/Deptheos.app\/Contents\/MacOS\/deptheos-gui",
  "storeInfo" : {"deviceIdentifierForVendor":"A1B2C3D4-E5F6-4711-8899-AABBCCDDEEFF","thirdParty":true},
  "parentProc" : "launchd",
  "crashReporterKey" : "8C2A6B9E-4D1F-5A3B-9C7E-1F2D3E4A5B6C",
  "sleepWakeUUID" : "D9E8F7A6-B5C4-4D3E-8F2A-1B0C9D8E7F6A",
  "bootSessionUUID" : "11223344-5566-4778-8899-AABBCCDDEEFF",
  "sip" : "enabled",
  "exception" : {"codes":"0x0000000000000001, 0x0000000000000000","type":"EXC_CRASH","signal":"SIGABRT"},
  "usedImages" : [
  {
    "source" : "P",
    "arch" : "arm64",
    "base" : 4294967296,
    "size" : 52428800,
    "uuid" : "5f0c2a7e-1d3b-3c4f-9a8e-7b6d5c4e3f21",
    "path" : "\/Users\/ann\/Applications\/Deptheos.app\/Contents\/MacOS\/deptheos-gui",
    "name" : "deptheos-gui"
  }
  ]
}"#;

    #[test]
    fn a_crash_report_keeps_no_home_or_machine_id() {
        let r = Redactor::new("/Users/ann", Some("ann"));
        let report = blank_machine_ids(&r.redact(IPS)).into_owned();
        assert!(!report.contains("ann"), "{report}");
        assert!(report.contains(r#""procPath" : "~\/Applications\/Deptheos.app"#), "{report}");
        for id in [
            "8C2A6B9E-4D1F-5A3B-9C7E-1F2D3E4A5B6C",
            "D9E8F7A6-B5C4-4D3E-8F2A-1B0C9D8E7F6A",
            "11223344-5566-4778-8899-AABBCCDDEEFF",
            "A1B2C3D4-E5F6-4711-8899-AABBCCDDEEFF",
        ] {
            assert!(!report.contains(id), "{id} in {report}");
        }
        assert!(report.contains(r#""crashReporterKey" : """#), "{report}");
        assert!(report.contains(r#""deviceIdentifierForVendor":"""#), "{report}");
        // The build's uuid, which symbolication needs, stays.
        assert_eq!(report.matches("5f0c2a7e-1d3b-3c4f-9a8e-7b6d5c4e3f21").count(), 2);
        // Still a header line and a JSON body.
        let (header, body) = report.split_once('\n').unwrap();
        serde_json::from_str::<serde_json::Value>(header).unwrap();
        serde_json::from_str::<serde_json::Value>(body).unwrap();
    }
}
