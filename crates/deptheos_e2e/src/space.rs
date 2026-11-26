//! An export's color space against its frames', or the output space a
//! case chooses.

use std::fmt;
use std::path::Path;

use deptheos_app::library::export::ExportFormat;
use image::ImageReader;

use crate::cases::OutputSpace;
use crate::metadata::tags_of;
use crate::read::is_raw;
use crate::{Frames, Report};

/// A color space by its primaries' chromaticities (x, y) as an ICC
/// profile states them, adapted to D50.
type Primaries = [[f64; 2]; 3];

/// sRGB's and Adobe RGB (1998)'s primaries, D50-adapted, from their ICC
/// profiles' colorants.
pub(crate) const SRGB: Primaries =
    primaries_of([[0.4361, 0.2225, 0.0139], [0.3851, 0.7169, 0.0971], [0.1431, 0.0606, 0.7141]]);
pub(crate) const ADOBE_RGB: Primaries =
    primaries_of([[0.6097, 0.3111, 0.0195], [0.2053, 0.6257, 0.0609], [0.1492, 0.0632, 0.7446]]);
/// How far a primary's x or y may differ within one space, as profiles
/// round their colorants differently.
const PRIMARIES: f64 = 0.003;

const fn primaries_of(xyz: [[f64; 3]; 3]) -> Primaries {
    let r = xyz[0][0] + xyz[0][1] + xyz[0][2];
    let g = xyz[1][0] + xyz[1][1] + xyz[1][2];
    let b = xyz[2][0] + xyz[2][1] + xyz[2][2];
    [[xyz[0][0] / r, xyz[0][1] / r], [xyz[1][0] / g, xyz[1][1] / g], [xyz[2][0] / b, xyz[2][1] / b]]
}

pub(crate) fn same_primaries(a: &Primaries, b: &Primaries) -> bool {
    a.iter()
        .zip(b)
        .all(|(p, q)| (p[0] - q[0]).abs() <= PRIMARIES && (p[1] - q[1]).abs() <= PRIMARIES)
}

/// A matrix profile's primaries, from its `rXYZ`, `gXYZ` and `bXYZ` tags;
/// `None` for one without them (a table-based profile).
pub(crate) fn icc_primaries(icc: &[u8]) -> Option<Primaries> {
    let u32_at = |at: usize| -> Option<u32> {
        Some(u32::from_be_bytes(icc.get(at..at + 4)?.try_into().ok()?))
    };
    // A gray frame stacks in sRGB's primaries
    // (`crates/deptheos_core/src/io/profile.rs`, `from_gray`).
    if icc.get(16..20) == Some(b"GRAY".as_slice()) {
        return Some(SRGB);
    }
    let count = u32_at(128)? as usize;
    let colorant = |sig: &[u8; 4]| -> Option<[f64; 3]> {
        let entry = (0..count.min(256))
            .map(|i| 132 + 12 * i)
            .find(|&at| icc.get(at..at + 4) == Some(sig.as_slice()))?;
        let offset = u32_at(entry + 4)? as usize;
        if icc.get(offset..offset + 4)? != b"XYZ " {
            return None;
        }
        let fixed = |i: usize| u32_at(offset + 8 + 4 * i).map(|v| f64::from(v as i32) / 65536.0);
        Some([fixed(0)?, fixed(1)?, fixed(2)?])
    };
    let xyz = [colorant(b"rXYZ")?, colorant(b"gXYZ")?, colorant(b"bXYZ")?];
    if xyz.iter().any(|p| p.iter().sum::<f64>() <= 0.0) {
        return None;
    }
    Some(primaries_of(xyz))
}

/// What a file says of its color space.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Space {
    /// An ICC profile: its description, and its primaries if it has
    /// colorants.
    Profile { name: String, primaries: Option<Primaries> },
    /// A PNG's `sRGB` chunk.
    SrgbChunk,
    /// An in-camera Adobe RGB JPEG: EXIF's interoperability index `R03`.
    DcfAdobeRgb,
    /// Nothing, which every reader takes as sRGB.
    Untagged,
}

impl Space {
    /// What `path` says; a RAW frame renders to sRGB.
    fn of(path: &Path) -> Result<Space, String> {
        if is_raw(path) {
            return Ok(Space::Untagged);
        }
        if let Some(icc) = icc_of(path)? {
            let primaries = icc_primaries(&icc);
            let name = deptheos_core::io::IccProfile::from_bytes(icc)
                .and_then(|p| p.name())
                .unwrap_or_else(|| "unnamed".into());
            return Ok(Space::Profile { name, primaries });
        }
        if ExportFormat::of(path) == Some(ExportFormat::Png) && png_has_chunk(path, b"sRGB") {
            return Ok(Space::SrgbChunk);
        }
        let interop = tags_of(path)
            .unwrap_or_default()
            .into_iter()
            .any(|(t, v)| t == exif::Tag::InteroperabilityIndex && v.contains("R03"));
        Ok(if interop { Space::DcfAdobeRgb } else { Space::Untagged })
    }

    /// A frame's space; one the checks can't compare is an error.
    pub(crate) fn of_frame(path: &Path) -> Result<Space, String> {
        let space = Space::of(path)?;
        match space.primaries() {
            Some(_) => Ok(space),
            None => Err(format!("{}: a color profile without primaries ({space})", path.display())),
        }
    }

    fn primaries(&self) -> Option<Primaries> {
        match self {
            Space::Profile { primaries, .. } => *primaries,
            Space::SrgbChunk | Space::Untagged => Some(SRGB),
            Space::DcfAdobeRgb => Some(ADOBE_RGB),
        }
    }
}

impl fmt::Display for Space {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Space::Profile { name, .. } => write!(f, "the profile \"{name}\""),
            Space::SrgbChunk => write!(f, "an sRGB chunk"),
            Space::DcfAdobeRgb => write!(f, "EXIF's Adobe RGB (R03)"),
            Space::Untagged => write!(f, "no profile, read as sRGB"),
        }
    }
}

/// The ICC profile embedded in the JPEG, PNG or TIFF at `path`. A TIFF's
/// is read by its own decoder: one from `ImageReader::into_decoder` has
/// had its limits set, after which it reads no profile.
fn icc_of(path: &Path) -> Result<Option<Vec<u8>>, String> {
    use image::ImageDecoder as _;
    let failed = |e: &dyn fmt::Display| format!("{}: {e}", path.display());
    let reader = ImageReader::open(path)
        .map_err(|e| failed(&e))?
        .with_guessed_format()
        .map_err(|e| failed(&e))?;
    if reader.format() == Some(image::ImageFormat::Tiff) {
        let file = std::fs::File::open(path).map_err(|e| failed(&e))?;
        let mut decoder = image::codecs::tiff::TiffDecoder::new(std::io::BufReader::new(file))
            .map_err(|e| failed(&e))?;
        return decoder.icc_profile().map_err(|e| failed(&e));
    }
    let mut decoder = reader.into_decoder().map_err(|e| failed(&e))?;
    decoder.icc_profile().map_err(|e| failed(&e))
}

/// Whether the PNG at `path` has a chunk of type `kind` before its pixels.
fn png_has_chunk(path: &Path, kind: &[u8; 4]) -> bool {
    let Ok(bytes) = std::fs::read(path) else { return false };
    let mut at = 8;
    while let Some(head) = bytes.get(at..at + 8) {
        let len = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as usize;
        if &head[4..8] == kind {
            return true;
        }
        if &head[4..8] == b"IDAT" {
            return false;
        }
        at += 12 + len;
    }
    false
}

/// The export is in the frames' color space: a profile with their
/// primaries, or for sRGB any way a reader takes it (a profile, PNG's
/// `sRGB` chunk, or nothing). A DNG's color is checked by
/// [`color_tags`](crate::tags::color_tags).
pub(crate) fn space_check(
    report: &mut Report,
    file: &Path,
    format: ExportFormat,
    frames: &Frames,
    chosen: Option<OutputSpace>,
) {
    if format == ExportFormat::Dng {
        return;
    }
    if let Some(chosen) = chosen {
        output_space_check(report, file, chosen);
        return;
    }
    let ours = match Space::of(file) {
        Ok(space) => space,
        Err(e) => {
            report.check("profile", false, e);
            return;
        }
    };
    let theirs = &frames.space;
    let wanted = theirs.primaries().unwrap_or(SRGB);
    let ok = match &ours {
        Space::Profile { primaries, .. } => primaries.is_some_and(|p| same_primaries(&p, &wanted)),
        // Frames not in sRGB need their profile embedded.
        Space::SrgbChunk | Space::Untagged => same_primaries(&wanted, &SRGB),
        Space::DcfAdobeRgb => false,
    };
    let detail = format!("the export says {ours}; the frames {theirs}");
    report.check("profile", ok, if ok { detail } else { format!("{detail}: another space") });
}

/// How far each of an export's colorants and its white (XYZ, as its
/// profile holds them) may be from those of moxcms's profile of the space
/// chosen. A profile stores each in steps of 1/65536 (s15Fixed16), so a
/// copy of moxcms's own comes within a few steps; 0.001 also allows a
/// profile rounded otherwise, and is under a tenth of what tells the
/// nearest two of the four spaces apart (`the_output_spaces_are_told_apart`).
pub(crate) const COLORANTS: f64 = 1e-3;

/// The space's own profile, which its exports carry; sRGB's carry none.
pub(crate) fn profile_of(space: OutputSpace) -> Option<moxcms::ColorProfile> {
    match space {
        OutputSpace::Srgb => None,
        OutputSpace::DisplayP3 => Some(moxcms::ColorProfile::new_display_p3()),
        OutputSpace::AdobeRgb => Some(moxcms::ColorProfile::new_adobe_rgb()),
        OutputSpace::ProPhoto => Some(moxcms::ColorProfile::new_pro_photo_rgb()),
    }
}

/// A profile's red, green and blue colorants and its media white, XYZ;
/// `None` without a white.
pub(crate) fn colorants(profile: &moxcms::ColorProfile) -> Option<[[f64; 3]; 4]> {
    let xyz = |c: moxcms::Xyzd| [c.x, c.y, c.z];
    let white = profile.media_white_point?;
    Some([
        xyz(profile.red_colorant),
        xyz(profile.green_colorant),
        xyz(profile.blue_colorant),
        xyz(white),
    ])
}

/// The largest difference between two profiles' colorants and whites.
pub(crate) fn colorants_off(a: &[[f64; 3]; 4], b: &[[f64; 3]; 4]) -> f64 {
    a.iter().flatten().zip(b.iter().flatten()).map(|(p, q)| (p - q).abs()).fold(0.0, f64::max)
}

/// The export's profile has the chosen space's primaries and white; for
/// sRGB, no profile or one with sRGB's primaries.
fn output_space_check(report: &mut Report, file: &Path, chosen: OutputSpace) {
    let name = serde_json::to_value(chosen).ok().and_then(|v| v.as_str().map(String::from));
    let name = name.unwrap_or_default();
    let icc = match icc_of(file) {
        Ok(icc) => icc,
        Err(e) => {
            report.check("profile", false, e);
            return;
        }
    };
    let (ok, detail) = match (profile_of(chosen), icc) {
        (None, None) => (true, "no profile, read as sRGB".to_string()),
        (None, Some(icc)) => {
            let ok = icc_primaries(&icc).is_some_and(|p| same_primaries(&p, &SRGB));
            (ok, format!("a profile with {} primaries", if ok { "sRGB's" } else { "other" }))
        }
        (Some(_), None) => (false, "no profile, which reads as sRGB".to_string()),
        (Some(wanted), Some(icc)) => match moxcms::ColorProfile::new_from_slice(&icc) {
            Err(e) => (false, format!("a profile that doesn't parse: {e:?}")),
            Ok(ours) => match (colorants(&ours), colorants(&wanted)) {
                (Some(ours), Some(theirs)) => {
                    let off = colorants_off(&ours, &theirs);
                    let detail = format!("its colorants and white within {off:.5} of {name}'s");
                    (off <= COLORANTS, format!("{detail} (at most {COLORANTS})"))
                }
                _ => (false, "a profile without a media white".to_string()),
            },
        },
    };
    report.check("profile", ok, format!("{detail}; {name} chosen"));
}
