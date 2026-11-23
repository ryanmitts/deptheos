//! An export's metadata: the camera's details kept, its location left out.

use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::{Frames, Report};

/// The tags an export carries over from its frames: the app's allowlist
/// (`COPIED_IFD0` and its Exif list in `deptheos_app::frames::capture`).
const KEPT: &[exif::Tag] = &[
    exif::Tag::Make,
    exif::Tag::Model,
    exif::Tag::Artist,
    exif::Tag::Copyright,
    exif::Tag::LensMake,
    exif::Tag::LensModel,
    exif::Tag::LensSpecification,
    exif::Tag::FocalLength,
    exif::Tag::FNumber,
    exif::Tag::ExposureTime,
    exif::Tag::PhotographicSensitivity,
    exif::Tag::DateTimeOriginal,
];

/// What identifies the camera, its owner or its location: never in an
/// export. GPS is every tag of its IFD.
const PRIVATE: &[exif::Tag] = &[
    exif::Tag::BodySerialNumber,
    exif::Tag::LensSerialNumber,
    exif::Tag::CameraOwnerName,
    exif::Tag::MakerNote,
    exif::Tag::ImageUniqueID,
];

/// A file's primary image's tags as text; none without EXIF. A file the
/// EXIF reader can't open (CR3, RAF) is read from the engine's
/// [`deptheos_app::frames::capture::metadata_streams`].
pub(crate) fn tags_of(path: &Path) -> Option<Vec<(exif::Tag, String)>> {
    let file = std::fs::File::open(path).ok()?;
    let reader = exif::Reader::new();
    let text = |f: &exif::Field| (f.tag, field_text(f));
    if let Ok(exif) = reader.read_from_container(&mut std::io::BufReader::new(file)) {
        return Some(exif.fields().filter(|f| f.ifd_num == exif::In::PRIMARY).map(text).collect());
    }
    if let Some(tiff) = olympus_as_tiff(path)
        && let Ok(exif) = reader.read_raw(tiff)
    {
        return Some(exif.fields().filter(|f| f.ifd_num == exif::In::PRIMARY).map(text).collect());
    }
    let mut tags = Vec::new();
    for stream in deptheos_app::frames::capture::metadata_streams(path).ok()? {
        let Ok(exif) = reader.read_raw(stream) else { continue };
        tags.extend(exif.fields().filter(|f| f.ifd_num == exif::In::PRIMARY).map(|f| {
            // A CR3's Exif IFD is a stream of its own, read as IFD0: an
            // unknown IFD0 field is the Exif tag of its number.
            let exif_tag = exif::Tag(exif::Context::Exif, f.tag.number());
            if f.tag.context() == exif::Context::Tiff
                && f.tag.description().is_none()
                && exif_tag.description().is_some()
            {
                text(&exif::Field { tag: exif_tag, ifd_num: f.ifd_num, value: f.value.clone() })
            } else {
                text(f)
            }
        }));
    }
    (!tags.is_empty()).then_some(tags)
}

/// An ORF as the TIFF it is: its `IIRO` (or `IIRS`) header, which the EXIF
/// reader refuses, read as `II*\0`.
fn olympus_as_tiff(path: &Path) -> Option<Vec<u8>> {
    let mut bytes = std::fs::read(path).ok()?;
    if !matches!(bytes.get(0..4)?, b"IIRO" | b"IIRS") {
        return None;
    }
    bytes[2..4].copy_from_slice(&[42, 0]);
    Some(bytes)
}

/// A field's value as text, as the checks compare it: strings trimmed of
/// a camera's padding (spaces, NULs), empty ones dropped, as an export
/// writes them.
fn field_text(f: &exif::Field) -> String {
    match &f.value {
        exif::Value::Ascii(strings) => {
            let strings: Vec<String> = strings
                .iter()
                .map(|s| String::from_utf8_lossy(s).trim_matches([' ', '\0']).to_string())
                .filter(|s| !s.is_empty())
                .map(|s| format!("{s:?}"))
                .collect();
            strings.join(", ")
        }
        _ => same_unknowns(f.tag, f.display_value().to_string()),
    }
}

/// A lens's unknown aperture as one text: the converter writes 0/0 (NaN),
/// an export 0/1 (ADR-133).
fn same_unknowns(tag: exif::Tag, value: String) -> String {
    if tag == exif::Tag::LensSpecification { value.replace("NaN", "0") } else { value }
}

/// An empty text tag (an unset Copyright), which an export may leave out.
fn is_blank(value: &str) -> bool {
    value.trim_matches('"').trim().is_empty()
}

/// Each kept tag's values across the frames (blank ones left out), and
/// which private tags any frame had.
pub(crate) struct FrameTags {
    pub(crate) kept: Vec<(exif::Tag, Vec<String>)>,
    pub(crate) private: Vec<String>,
}

impl FrameTags {
    pub(crate) fn read(frames: &[PathBuf]) -> FrameTags {
        let all: Vec<Vec<(exif::Tag, String)>> =
            frames.par_iter().map(|p| tags_of(p).unwrap_or_default()).collect();
        let kept = KEPT
            .iter()
            .map(|tag| {
                let mut values: Vec<String> = all
                    .iter()
                    .flat_map(|t| t.iter().filter(|(k, _)| k == tag).map(|(_, v)| v.clone()))
                    .filter(|v| !is_blank(v))
                    .collect();
                values.sort();
                values.dedup();
                (*tag, values)
            })
            .filter(|(_, values)| !values.is_empty())
            .collect();
        let mut private: Vec<String> = all
            .iter()
            .flatten()
            .filter(|(tag, _)| is_private(*tag))
            .map(|(tag, _)| private_name(*tag))
            .collect();
        private.sort();
        private.dedup();
        FrameTags { kept, private }
    }
}

fn is_private(tag: exif::Tag) -> bool {
    is_gps(tag) || PRIVATE.contains(&tag)
}

fn private_name(tag: exif::Tag) -> String {
    if is_gps(tag) { "GPS".into() } else { tag.to_string() }
}

/// The export's metadata against its frames': every kept tag with a
/// frame's value, nothing private, and Deptheos as its software.
pub(crate) fn metadata_checks(report: &mut Report, file: &Path, frames: &Frames) {
    let Some(tags) = tags_of(file) else {
        report.check("metadata", false, "no EXIF in the export");
        return;
    };
    if frames.tags.kept.is_empty() {
        // A reader that reads nothing would pass every check below.
        report.check("kept", false, "none of the frames' tags read: nothing to check against");
    }
    let value = |tag: exif::Tag| tags.iter().find(|(t, _)| *t == tag).map(|(_, v)| v.as_str());
    let mut kept = Vec::new();
    let mut wrong = Vec::new();
    for (tag, values) in &frames.tags.kept {
        match value(*tag) {
            Some(v) if values.iter().any(|f| f == v) => kept.push(tag.to_string()),
            Some(v) => wrong.push(format!("{tag} is {v}, the frames' {}", values.join(" or "))),
            None => wrong.push(format!("{tag} missing")),
        }
    }
    report.check(
        "kept",
        wrong.is_empty(),
        if wrong.is_empty() {
            format!("{} of the frames' tags, as they are: {}", kept.len(), kept.join(", "))
        } else {
            wrong.join("; ")
        },
    );
    let mut found: Vec<String> =
        tags.iter().filter(|(t, _)| is_private(*t)).map(|(t, _)| private_name(*t)).collect();
    found.dedup();
    let had = &frames.tags.private;
    let untested: Vec<String> = ["GPS"]
        .into_iter()
        .map(String::from)
        .chain(PRIVATE.iter().map(|t| t.to_string()))
        .filter(|name| !had.contains(name))
        .collect();
    report.check(
        "private",
        found.is_empty(),
        if found.is_empty() {
            let removed = if had.is_empty() { "none in the frames".into() } else { had.join(", ") };
            format!("removed: {removed}; not in the frames, so untested: {}", untested.join(", "))
        } else {
            format!("in the export: {}", found.join(", "))
        },
    );
    let software = value(exif::Tag::Software).unwrap_or("none");
    report.check(
        "software",
        software.trim_matches('"').starts_with("Deptheos"),
        format!("Software {software}"),
    );
}

fn is_gps(tag: exif::Tag) -> bool {
    tag.context() == exif::Context::Gps
}
