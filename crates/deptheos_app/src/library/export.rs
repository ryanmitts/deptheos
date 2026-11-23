//! What an export is written as and called, for every front end: the
//! formats, a name cleaned into a file stem, the extension, and a taken
//! name avoided as "name-2.tif".

use std::path::{Path, PathBuf};

/// What a result is exported as.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ExportFormat {
    Tiff,
    /// What a stack of ordinary images starts as; RAW or DNG starts as DNG
    /// ([`Self::first_for`]).
    #[default]
    Jpeg,
    Png,
    /// Linear (RAW or DNG) stacks only.
    Dng,
}

impl ExportFormat {
    /// Every format, in the order they are offered.
    pub const ALL: [Self; 4] = [Self::Tiff, Self::Jpeg, Self::Png, Self::Dng];

    /// What the preferences file calls it.
    pub fn key(self) -> &'static str {
        match self {
            Self::Tiff => "tiff",
            Self::Jpeg => "jpeg",
            Self::Png => "png",
            Self::Dng => "dng",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Tiff => "TIFF",
            Self::Jpeg => "JPEG",
            Self::Png => "PNG",
            Self::Dng => "DNG",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Tiff => "tif",
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::Dng => "dng",
        }
    }

    /// The format of an extension, in any case; `None` for any other.
    pub fn of(path: &Path) -> Option<Self> {
        Self::from_extension(path.extension()?.to_str()?)
    }

    fn from_extension(extension: &str) -> Option<Self> {
        match extension.to_ascii_lowercase().as_str() {
            "tif" | "tiff" => Some(Self::Tiff),
            "jpg" | "jpeg" => Some(Self::Jpeg),
            "png" => Some(Self::Png),
            "dng" => Some(Self::Dng),
            _ => None,
        }
    }

    /// Whether a stack can be exported as this: DNG needs a linear stack.
    pub fn is_available(self, linear: bool) -> bool {
        self != Self::Dng || linear
    }

    /// Whether this format is written with 16 bits per channel: JPEG never,
    /// DNG always, TIFF and PNG when `asked` and `source` has 16 bits.
    pub fn depth(self, asked: bool, source: bool) -> bool {
        match self {
            Self::Jpeg => false,
            Self::Dng => true,
            Self::Tiff | Self::Png => asked && source,
        }
    }

    /// Whether the bit depth is fixed: by the format (JPEG, DNG) or by
    /// 8-bit frames (`source` false).
    pub fn is_depth_fixed(self, source: bool) -> bool {
        matches!(self, Self::Jpeg | Self::Dng) || !source
    }

    /// What a stack's export starts as: DNG for a linear stack, which keeps
    /// it editable, else JPEG.
    pub fn first_for(linear: bool) -> Self {
        if linear { Self::Dng } else { Self::default() }
    }

    /// This format, or TIFF in its place when the stack can't have it.
    pub fn for_stack(self, linear: bool) -> Self {
        if self.is_available(linear) { self } else { Self::Tiff }
    }

    /// The formats a stack can be exported as, in the order offered.
    pub fn offered(linear: bool) -> Vec<Self> {
        Self::ALL.into_iter().filter(|f| f.is_available(linear)).collect()
    }
}

/// The longest stem [`clean_stem`] gives, in bytes, leaving room in a
/// 255-byte name for an extension, a "-12" and a temporary suffix.
const STEM_BYTES: usize = 200;

/// `name` made into a file stem: refused and control characters become
/// dashes; stray separators and the trailing dots and spaces Windows drops
/// are trimmed; a Windows device name gets "-stack" ("com1-stack.raw") on
/// every platform; it is cut at [`STEM_BYTES`]. Empty becomes "stack".
pub fn clean_stem(name: &str) -> String {
    let clean: String = name.chars().map(|c| if refused(c) { '-' } else { c }).collect();
    let ends = |c: char| c.is_whitespace() || matches!(c, '-' | '_' | '.');
    let mut clean = clean.trim_matches(ends);
    if clean.len() > STEM_BYTES {
        let cut = (0..=STEM_BYTES).rev().find(|&at| clean.is_char_boundary(at)).unwrap_or(0);
        clean = clean[..cut].trim_end_matches(ends);
    }
    match clean {
        "" => "stack".into(),
        device if is_device_name(device) => {
            let (base, rest) = device.split_at(device.find('.').unwrap_or(device.len()));
            format!("{}-stack{rest}", base.trim_end())
        }
        clean => clean.to_string(),
    }
}

/// `name` without an extension the formats use.
pub fn strip_extension(name: &str) -> &str {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && ExportFormat::from_extension(ext).is_some() => {
            stem
        }
        _ => name,
    }
}

/// `name` ending in `format`'s extension; `None` for an empty name, a path,
/// or a Windows device name. Other refused characters become dashes, as in
/// [`clean_stem`], and trailing dots and spaces are trimmed, on every
/// platform so a name means one file everywhere.
pub fn with_extension(name: &str, format: ExportFormat) -> Option<String> {
    // ".tif" alone is an extension with no name.
    if name.rsplit_once('.').is_some_and(|(stem, _)| stem.trim().is_empty()) {
        return None;
    }
    let stem = strip_extension(name).trim();
    if stem.contains(['/', '\\']) {
        return None;
    }
    let stem: String = stem.chars().map(|c| if refused(c) { '-' } else { c }).collect();
    let stem = stem.trim_end_matches(['.', ' ']).trim();
    if stem.is_empty() || is_device_name(stem) {
        return None;
    }
    Some(format!("{stem}.{}", format.extension()))
}

/// Whether some file system refuses `c` in a name, or it is a control
/// character (`:` on NTFS names a hidden stream).
fn refused(c: char) -> bool {
    matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control()
}

/// A name Windows keeps for a device, with or without an extension:
/// "con", "NUL.backup", "com1", "COM²".
fn is_device_name(stem: &str) -> bool {
    let base = stem.split('.').next().unwrap_or(stem).trim_end().to_ascii_uppercase();
    // Windows keeps COM and LPT with a superscript 1, 2 or 3 too.
    if let Some(kind) = base.strip_suffix(['¹', '²', '³']) {
        return matches!(kind, "COM" | "LPT");
    }
    match base.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" => true,
        // By bytes: a name that is not ASCII has no boundary to split at.
        _ => match base.as_bytes() {
            [kind @ .., digit] => {
                matches!(kind, b"COM" | b"LPT") && digit.is_ascii_digit() && *digit != b'0'
            }
            [] => false,
        },
    }
}

/// The default `file_name_pattern` preference ([`crate::preferences`]).
pub(crate) const DEFAULT_PATTERN: &str = "{stack}-{render}";

/// What a file name pattern's tokens stand for.
#[derive(Clone, Copy, Debug)]
pub struct NameParts<'a> {
    /// The stack's name: "canon1".
    pub stack: &'a str,
    /// The render's number in its stack; none when it isn't a render yet.
    pub render: Option<u32>,
    /// The method, as worded: "Depth".
    pub method: &'a str,
    /// The day it is exported, as `YYYY-MM-DD` in local time: "2026-11-09".
    pub date: &'a str,
}

/// A file stem from `pattern`, its {stack}, {render}, {method} and {date}
/// replaced, cleaned by [`clean_stem`].
pub fn file_stem(pattern: &str, parts: &NameParts) -> String {
    let render = parts.render.map(|n| n.to_string()).unwrap_or_default();
    let name = pattern
        .replace("{stack}", parts.stack)
        .replace("{render}", &render)
        .replace("{method}", parts.method)
        .replace("{date}", parts.date);
    // The cleaning trims an empty token's separator.
    clean_stem(&name)
}

/// "name.tif", "name-2.tif", … without end; an empty `extension` names a
/// folder.
pub fn numbered(stem: &str, extension: &str) -> impl Iterator<Item = String> {
    let dot = if extension.is_empty() { "" } else { "." };
    (1..).map(move |n| match n {
        1 => format!("{stem}{dot}{extension}"),
        n => format!("{stem}-{n}{dot}{extension}"),
    })
}

/// The first of `<dir>/<stem>.<extension>`, `<stem>-2.<extension>`… that
/// `taken` says is free.
pub fn untaken_path(
    dir: &Path,
    stem: &str,
    extension: &str,
    taken: impl Fn(&Path) -> bool,
) -> PathBuf {
    numbered(stem, extension)
        .map(|name| dir.join(name))
        .find(|path| !taken(path))
        .expect("some name is free")
}

/// Where a batch exports `label` as `extension` into `folder`: over `slot`,
/// the replaced render's file, when its extension matches; else the first
/// free name.
pub fn batch_target(
    folder: &Path,
    label: &str,
    extension: &str,
    slot: Option<&Path>,
    taken: impl Fn(&Path) -> bool,
) -> PathBuf {
    match slot {
        Some(slot) if slot.extension().is_some_and(|e| e == extension) => slot.to_path_buf(),
        _ => untaken_path(folder, &clean_stem(label), extension, taken),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jpeg_is_8_bits_dng_16_and_an_8_bit_stack_has_no_16() {
        use ExportFormat::*;
        // (format, asked, source) → (written with 16, depth fixed)
        let table = [
            (Jpeg, true, true, false, true),
            (Jpeg, false, false, false, true),
            (Dng, false, true, true, true),
            (Dng, true, true, true, true),
            (Tiff, true, true, true, false),
            (Tiff, false, true, false, false),
            (Tiff, true, false, false, true),
            (Png, true, true, true, false),
            (Png, true, false, false, true),
        ];
        for (format, asked, source, sixteen, fixed) in table {
            assert_eq!(format.depth(asked, source), sixteen, "{format:?} {asked} {source}");
            assert_eq!(format.is_depth_fixed(source), fixed, "{format:?} {source}");
        }
    }

    #[test]
    fn a_batch_export_goes_over_its_slot_only_in_the_same_format() {
        let folder = Path::new("/out");
        let free = |_: &Path| false;
        assert_eq!(batch_target(folder, "s/036", "tif", None, free), folder.join("s-036.tif"));
        let taken = |p: &Path| p == folder.join("s-036.tif");
        assert_eq!(batch_target(folder, "s/036", "tif", None, taken), folder.join("s-036-2.tif"));
        let slot = folder.join("s-036-2.tif");
        assert_eq!(batch_target(folder, "s/036", "tif", Some(&slot), taken), slot);
        // A slot of another format isn't written over.
        let dng = folder.join("s-036.dng");
        assert_eq!(batch_target(folder, "s/036", "tif", Some(&dng), taken), slot);
    }

    #[test]
    fn names_follow_the_pattern() {
        let parts =
            |stack, render| NameParts { stack, render, method: "Depth", date: "2026-11-09" };
        assert_eq!(file_stem(DEFAULT_PATTERN, &parts("canon1", Some(4))), "canon1-4");
        assert_eq!(file_stem(DEFAULT_PATTERN, &parts("a/b: c", Some(2))), "a-b- c-2");
        assert_eq!(file_stem(DEFAULT_PATTERN, &parts("canon1", None)), "canon1");
        assert_eq!(file_stem("{render}", &parts("x", None)), "stack");
        assert_eq!(
            file_stem("{date} {stack} {method} #{render}", &parts("spider", Some(3))),
            "2026-11-09 spider Depth #3"
        );
    }

    #[test]
    fn formats_are_offered_tiff_first_and_dng_only_for_linear_stacks() {
        use ExportFormat::*;
        assert_eq!(ExportFormat::offered(false), [Tiff, Jpeg, Png]);
        assert_eq!(ExportFormat::offered(true), [Tiff, Jpeg, Png, Dng]);
        assert_eq!(Dng.for_stack(false), Tiff);
        assert_eq!(Dng.for_stack(true), Dng);
        assert_eq!(Jpeg.for_stack(false), Jpeg);
    }

    #[test]
    fn names_are_cleaned_into_stems() {
        assert_eq!(clean_stem("canon1-4"), "canon1-4");
        assert_eq!(clean_stem("a/b: c-2"), "a-b- c-2");
        assert_eq!(clean_stem("Bee/Wasp #2 stacked"), "Bee-Wasp #2 stacked");
        assert_eq!(clean_stem("tab\there"), "tab-here");
        assert_eq!(clean_stem("trailing. "), "trailing");
        assert_eq!(clean_stem(" . "), "stack");
        assert_eq!(clean_stem(""), "stack");
        // Names that are not ASCII are no device's (and split nowhere).
        assert_eq!(clean_stem("éé"), "éé");
        assert_eq!(with_extension("éé", ExportFormat::Tiff).as_deref(), Some("éé.tif"));
        // A folder named for a Windows device still exports to a file.
        assert_eq!(clean_stem("aux"), "aux-stack");
        assert_eq!(clean_stem("CON"), "CON-stack");
        assert_eq!(clean_stem("com1.raw"), "com1-stack.raw");
        assert_eq!(clean_stem("Lpt3 .tar.gz"), "Lpt3-stack.tar.gz");
        assert_eq!(clean_stem("console"), "console");
        // A very long name leaves room for an extension and a number.
        let long = clean_stem(&"é".repeat(300));
        assert_eq!(long, "é".repeat(100));
        assert_eq!(clean_stem(&format!("{} tail", "a".repeat(199))), "a".repeat(199));
    }

    /// A stem made from a device's name, in any form, is no device's name.
    #[test]
    fn a_device_name_stays_no_device() {
        let devices = ["CON", "prn", "Aux", "nul", "COM1", "com9", "LPT1", "lpt9", "COM²", "lpt³"];
        for device in devices {
            for tail in ["", ".raw", ".RAW", ".tar.gz", " .x", "..", "."] {
                let name = format!("{device}{tail}");
                let stem = clean_stem(&name);
                assert!(!is_device_name(&stem), "{name:?} gave {stem:?}");
                // The first name, and one numbered past it.
                let first = untaken_path(Path::new("/x"), &stem, "tif", |_| false);
                let path = untaken_path(Path::new("/x"), &stem, "tif", |p| *p == first);
                assert!(!is_device_name(first.file_name().unwrap().to_str().unwrap()));
                let file = path.file_name().unwrap().to_str().unwrap();
                assert!(!is_device_name(file), "{name:?} gave {file:?}");
                assert!(file.starts_with(device), "{name:?} gave {file:?}");
            }
        }
    }

    #[test]
    fn a_path_is_the_format_its_extension_names() {
        let of = |p: &str| ExportFormat::of(Path::new(p));
        assert_eq!(of("a/b.TIF"), Some(ExportFormat::Tiff));
        assert_eq!(of("b.tiff"), Some(ExportFormat::Tiff));
        assert_eq!(of("c.JPEG"), Some(ExportFormat::Jpeg));
        assert_eq!(of("d.jpg"), Some(ExportFormat::Jpeg));
        assert_eq!(of("e.Png"), Some(ExportFormat::Png));
        assert_eq!(of("f.dng"), Some(ExportFormat::Dng));
        for other in ["g.webp", "h", "i.tif.zip", ".png"] {
            assert_eq!(of(other), None, "{other}");
        }
        for format in ExportFormat::ALL {
            assert_eq!(of(&format!("x.{}", format.extension())), Some(format));
        }
    }

    #[test]
    fn extensions_follow_the_format() {
        assert_eq!(with_extension("canon1-4.tif", ExportFormat::Jpeg).unwrap(), "canon1-4.jpg");
        assert_eq!(with_extension("canon1-4", ExportFormat::Png).unwrap(), "canon1-4.png");
        assert_eq!(with_extension("v1.2", ExportFormat::Tiff).unwrap(), "v1.2.tif");
        assert_eq!(with_extension(".tif", ExportFormat::Tiff), None);
        assert_eq!(with_extension("a/b", ExportFormat::Tiff), None);
    }

    #[test]
    fn a_typed_name_means_one_plain_file_on_every_platform() {
        let tiff = |name| with_extension(name, ExportFormat::Tiff);
        assert_eq!(tiff("bee:wing").as_deref(), Some("bee-wing.tif"), "not an NTFS stream");
        assert_eq!(tiff("a<b>c|d?e*\"f\"").as_deref(), Some("a-b-c-d-e--f-.tif"));
        assert_eq!(tiff("tab\there").as_deref(), Some("tab-here.tif"));
        assert_eq!(tiff("trailing. .").as_deref(), Some("trailing.tif"));
        assert_eq!(tiff("..").as_deref(), None);
        assert_eq!(tiff("a\\b"), None);
        for reserved in ["CON", "nul", "Aux.tif", "com1", "LPT9", "con.backup"] {
            assert_eq!(tiff(reserved), None, "{reserved}");
        }
        for fine in ["console", "com10", "com0", "lpt", "null"] {
            assert!(tiff(fine).is_some(), "{fine}");
        }
    }

    #[test]
    fn a_taken_name_gets_a_dash_and_a_number() {
        let names: Vec<String> = numbered("s001", "png").take(3).collect();
        assert_eq!(names, ["s001.png", "s001-2.png", "s001-3.png"]);
        let folders: Vec<String> = numbered("scan1 grow", "").take(2).collect();
        assert_eq!(folders, ["scan1 grow", "scan1 grow-2"]);
        let dir = Path::new("/x");
        let taken = |p: &Path| p == dir.join("a.tif") || p == dir.join("a-2.tif");
        assert_eq!(untaken_path(dir, "a", "tif", taken), dir.join("a-3.tif"));
    }
}
