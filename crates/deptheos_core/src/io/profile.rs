//! The color space of a stack of standard images.
//!
//! Standard frames stack on their stored values, so a stack is in its first
//! frame's color space and its exports must say so. A frame's space comes
//! from its [`ColorTag`]; sRGB or no space is `None`. A gray frame's space
//! is RGB from its gray curve, a linear frame's its primaries on sRGB's
//! curve (ADR-132, [`super::input`]).

use std::fmt;
use std::sync::Arc;

use super::input::{ColorTag, Refused};

/// An RGB ICC profile, not sRGB, that a stack's pixels are encoded in.
#[derive(Clone, PartialEq, Eq)]
pub struct IccProfile(Arc<[u8]>);

impl fmt::Debug for IccProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "IccProfile({:?}, {} bytes)", self.name(), self.0.len())
    }
}

/// The profile header's color space for RGB data.
const RGB_SPACE: &[u8; 4] = b"RGB ";

impl IccProfile {
    /// `bytes` as a profile when they are an ICC profile for RGB data;
    /// `None` for anything else (grayscale, CMYK, Lab, malformed).
    pub fn from_bytes(bytes: Vec<u8>) -> Option<Self> {
        let header_ok = bytes.len() >= 128
            && &bytes[36..40] == b"acsp"
            && &bytes[16..20] == RGB_SPACE
            && u32::from_be_bytes(bytes[0..4].try_into().ok()?) as usize <= bytes.len();
        header_ok.then(|| Self(bytes.into()))
    }

    /// The profile's bytes, as embedded in a file.
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }

    /// The profile's description, for messages; `None` when it has none.
    pub fn name(&self) -> Option<String> {
        profile_description(&self.0)
    }

    /// Whether `self` and `other` describe the same color space: the same
    /// bytes, or the same primaries and tone curves (one space written by
    /// two programs differs in its description and dates).
    pub(crate) fn same_space(&self, other: &Self) -> bool {
        self.0 == other.0
            || matches!(
                (Colorimetry::of_bytes(&self.0), Colorimetry::of_bytes(&other.0)),
                (Some(a), Some(b)) if a.matches(&b)
            )
    }

    /// A profile for Adobe RGB (1998), for in-camera Adobe RGB JPEGs, which
    /// carry none. Written from the published primaries, white and gamma,
    /// since Adobe's may not be redistributed; version 2, which every
    /// color-managed reader takes (Firefox ignores version 4).
    pub fn adobe_rgb_compatible() -> Self {
        Self(ADOBE_RGB_V2.into())
    }

    /// Whether this profile is sRGB's space, under another name.
    fn is_srgb(&self) -> bool {
        Colorimetry::of_bytes(&self.0).is_some_and(|c| c.matches(&Colorimetry::srgb()))
    }

    /// The profile parsed, for a conversion into its space.
    pub(super) fn parsed(&self) -> Option<moxcms::ColorProfile> {
        moxcms::ColorProfile::new_from_slice(&self.0).ok()
    }
}

/// The description an ICC profile (of any color space) gives itself, for
/// messages; `None` when it gives none (the front ends word that).
fn profile_description(bytes: &[u8]) -> Option<String> {
    use moxcms::ProfileText;
    let description =
        moxcms::ColorProfile::new_from_slice(bytes).ok().and_then(|p| match p.description? {
            ProfileText::PlainString(s) => Some(s),
            ProfileText::Localizable(strings) => strings.into_iter().next().map(|s| s.value),
            ProfileText::Description(d) => Some(d.ascii_string),
        });
    description.map(|s| s.trim_end_matches('\0').trim().to_owned()).filter(|s| !s.is_empty())
}

/// A 16-bit RGB conversion from a profile's space to sRGB, for the screen
/// only (relative colorimetric, clipping); exports keep the profile.
#[derive(Clone)]
pub struct ToSrgb(Arc<moxcms::Transform16BitExecutor>);

impl fmt::Debug for ToSrgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ToSrgb")
    }
}

impl IccProfile {
    /// The conversion from this profile to sRGB; `None` when the profile
    /// cannot be built into one (then its pixels are shown as they are).
    pub fn to_srgb(&self) -> Option<ToSrgb> {
        let source = moxcms::ColorProfile::new_from_slice(&self.0).ok()?;
        let options = moxcms::TransformOptions {
            rendering_intent: moxcms::RenderingIntent::RelativeColorimetric,
            ..Default::default()
        };
        let srgb = moxcms::ColorProfile::new_srgb();
        let layout = moxcms::Layout::Rgb;
        source.create_transform_16bit(layout, &srgb, layout, options).ok().map(ToSrgb)
    }
}

impl ToSrgb {
    /// `src`, interleaved RGB, converted into `dst` (the same length).
    pub fn convert(&self, src: &[u16], dst: &mut [u16]) {
        if self.0.transform(src, dst).is_err() {
            dst.copy_from_slice(src);
        }
    }
}

/// What a frame's [`ColorTag`] makes of its stored values, from its
/// header alone ([`source_space`]).
pub(super) struct SourceSpace {
    /// The stored values' space, gray expanded to RGB.
    pub source: moxcms::ColorProfile,
    /// Linear light (identity curves), which the stage encodes to sRGB's
    /// curve.
    pub linear: bool,
    /// The space the frame comes out in unconverted (a linear one on sRGB's
    /// curve); `None` for sRGB.
    pub encoded: Option<IccProfile>,
}

/// The [`SourceSpace`] of a frame tagged `color`. An unreadable or
/// mismatched tag counts as none; an HDR transfer is refused.
pub(super) fn source_space(
    color: &ColorTag,
    gray: bool,
    float: bool,
) -> std::result::Result<SourceSpace, Refused> {
    use moxcms::{ColorProfile, DataColorSpace};
    let untagged = || if float { linear_srgb() } else { ColorProfile::new_srgb() };
    let (source, kept) = match color {
        ColorTag::Icc(bytes) => match ColorProfile::new_from_slice(bytes) {
            Ok(profile) => {
                refuse_hdr(profile.cicp.map(|c| c.transfer_characteristics as u8))?;
                match (profile.color_space, gray) {
                    (DataColorSpace::Rgb, false) => (profile, Some(bytes.clone())),
                    (DataColorSpace::Gray, true) => match profile.gray_trc {
                        Some(trc) => (from_gray(trc), None),
                        None => (untagged(), None),
                    },
                    (space, _) => {
                        tracing::debug!(
                            "ignoring an embedded {space:?} profile on a frame of other data"
                        );
                        (untagged(), None)
                    }
                }
            }
            Err(e) => {
                tracing::debug!("ignoring an embedded ICC profile that can't be read: {e}");
                (untagged(), None)
            }
        },
        &ColorTag::Cicp { primaries, transfer, full_range } => {
            refuse_hdr(Some(transfer))?;
            match cicp_profile(primaries, transfer, full_range) {
                Some(profile) if gray => (from_gray(profile.red_trc.expect("a curve")), None),
                Some(profile) => (profile, None),
                None => {
                    tracing::debug!("ignoring cICP {primaries}/{transfer}: no space this reads");
                    (untagged(), None)
                }
            }
        }
        &ColorTag::PngChunks { srgb, gamma, chromaticities } => {
            (png_chunk_profile(srgb, gamma, if gray { None } else { chromaticities }), None)
        }
        ColorTag::DcfAdobeRgb if !gray => {
            let bytes = IccProfile::adobe_rgb_compatible().bytes().to_vec();
            (ColorProfile::new_from_slice(&bytes).expect("the shipped profile reads"), Some(bytes))
        }
        ColorTag::DcfAdobeRgb | ColorTag::Untagged => (untagged(), None),
    };
    let linear = [&source.red_trc, &source.green_trc, &source.blue_trc]
        .iter()
        .all(|trc| trc.as_ref().is_some_and(is_identity));
    let encoded = if linear {
        let mut encoded = ColorProfile::new_srgb();
        encoded.red_colorant = source.red_colorant;
        encoded.green_colorant = source.green_colorant;
        encoded.blue_colorant = source.blue_colorant;
        encoded.media_white_point = source.media_white_point;
        encoded.cicp = None;
        made(encoded, "Linear frame, encoded to the sRGB curve")
    } else {
        match kept {
            Some(bytes) => IccProfile::from_bytes(bytes).filter(|p| !p.is_srgb()),
            None => made(source.clone(), "Frame color space"),
        }
    };
    Ok(SourceSpace { source, linear, encoded })
}

/// An HDR transfer (PQ, HLG) refused.
fn refuse_hdr(transfer: Option<u8>) -> std::result::Result<(), Refused> {
    use moxcms::TransferCharacteristics::{Hlg, Smpte2084};
    match transfer {
        Some(t) if t == Smpte2084 as u8 || t == Hlg as u8 => Err(Refused::Hdr),
        _ => Ok(()),
    }
}

/// The profile of a PNG's `cICP` (an RGB one; full range, the only kind a
/// PNG holds); `None` for code points with no primaries or curve here.
fn cicp_profile(primaries: u8, transfer: u8, full_range: bool) -> Option<moxcms::ColorProfile> {
    let cicp = moxcms::CicpProfile {
        color_primaries: primaries.try_into().ok()?,
        transfer_characteristics: transfer.try_into().ok()?,
        matrix_coefficients: moxcms::MatrixCoefficients::Bt709,
        full_range,
    };
    // Code points this reads: primaries and a curve.
    let _: moxcms::ColorPrimaries = cicp.color_primaries.try_into().ok()?;
    let _: moxcms::ToneReprCurve = cicp.transfer_characteristics.try_into().ok()?;
    let mut profile = moxcms::ColorProfile::new_srgb();
    profile.update_rgb_colorimetry_from_cicp(cicp);
    // The curve made, not the code points, says what it is from here on.
    profile.cicp = None;
    Some(profile)
}

/// sRGB's primaries with a gray curve on each channel, so gray expanded to
/// R = G = B keeps its meaning.
fn from_gray(trc: moxcms::ToneReprCurve) -> moxcms::ColorProfile {
    let mut profile = moxcms::ColorProfile::new_srgb();
    profile.red_trc = Some(trc.clone());
    profile.green_trc = Some(trc.clone());
    profile.blue_trc = Some(trc);
    profile.cicp = None;
    profile
}

/// `profile` as one a stack carries, described as `description`; `None`
/// when it is sRGB, or can't be written.
fn made(mut profile: moxcms::ColorProfile, description: &str) -> Option<IccProfile> {
    if Colorimetry::of(&profile).is_some_and(|c| c.matches(&Colorimetry::srgb())) {
        return None;
    }
    profile.description = Some(moxcms::ProfileText::PlainString(description.into()));
    IccProfile::from_bytes(profile.encode().ok()?)
}

/// Whether a tone curve is the identity, to the tolerance spaces are
/// matched to.
fn is_identity(trc: &moxcms::ToneReprCurve) -> bool {
    (0..Colorimetry::SAMPLES).all(|i| {
        let x = i as f32 / (Colorimetry::SAMPLES - 1) as f32;
        eval_trc(trc, x).is_some_and(|y| (y - x).abs() <= Colorimetry::CURVE_TOLERANCE)
    })
}

/// A PNG's `sRGB`, `gAMA` and `cHRM` chunks as a profile, `sRGB` first by
/// the PNG specification. Gamma 1/2.2 (to 1%) counts as sRGB's curve.
fn png_chunk_profile(
    srgb: bool,
    gamma: Option<f32>,
    chromaticities: Option<[(f32, f32); 4]>,
) -> moxcms::ColorProfile {
    use moxcms::{Chromaticity, ColorPrimaries, ColorProfile};
    let mut profile = ColorProfile::new_srgb();
    if srgb {
        return profile;
    }
    // A file gamma of 0 or past 10 is no gamma at all.
    let gamma = gamma.filter(|g| *g > 0.0 && *g <= 10.0);
    let chromaticities = chromaticities.filter(|c| {
        c.iter()
            .all(|&(x, y)| x.is_finite() && y.is_finite() && (0.0..=1.0).contains(&x) && y > 0.0)
    });
    if let Some([white, red, green, blue]) = chromaticities {
        let xy = |(x, y): (f32, f32)| Chromaticity::new(x, y);
        let primaries = ColorPrimaries { red: xy(red), green: xy(green), blue: xy(blue) };
        profile.update_rgb_colorimetry(xy(white).to_xyyb(), primaries);
        profile.media_white_point = Some(xy(white).to_xyzd());
    }
    // sRGB's own curve stands for the gamma 1/2.2 beside it.
    if let Some(gamma) = gamma.filter(|g| (g * 2.2 - 1.0).abs() > 0.01) {
        // The file gamma is the encoding exponent.
        let curve = moxcms::curve_from_gamma(1.0 / gamma);
        profile.red_trc = Some(curve.clone());
        profile.green_trc = Some(curve.clone());
        profile.blue_trc = Some(curve);
    }
    profile.cicp = None;
    profile
}

/// The profile assumed for floating-point values that come without one:
/// linear light with sRGB's primaries, as 32-bit editors write them.
fn linear_srgb() -> moxcms::ColorProfile {
    let mut profile = moxcms::ColorProfile::new_srgb();
    let linear = moxcms::curve_from_gamma(1.0);
    profile.red_trc = Some(linear.clone());
    profile.green_trc = Some(linear.clone());
    profile.blue_trc = Some(linear);
    profile.cicp = None;
    profile
}

/// What makes a matrix/tone-curve profile's color space: its colorant
/// matrix and its three tone curves, sampled.
struct Colorimetry {
    colorants: [[f64; 3]; 3],
    curves: [[f32; Self::SAMPLES]; 3],
}

impl Colorimetry {
    const SAMPLES: usize = 17;
    /// Primaries match to this (in XYZ), curves to this (0–1). Far looser
    /// than the profiles' fixed-point rounding, far tighter than any two
    /// distinct working spaces.
    const COLORANT_TOLERANCE: f64 = 2e-3;
    const CURVE_TOLERANCE: f32 = 2e-3;

    fn srgb() -> Self {
        Self::of(&moxcms::ColorProfile::new_srgb()).expect("sRGB is a matrix and tone curves")
    }

    fn of_bytes(bytes: &[u8]) -> Option<Self> {
        Self::of(&moxcms::ColorProfile::new_from_slice(bytes).ok()?)
    }

    /// `None` for a profile that is not a plain matrix and tone curves.
    fn of(profile: &moxcms::ColorProfile) -> Option<Self> {
        let trcs = [&profile.red_trc, &profile.green_trc, &profile.blue_trc];
        let mut curves = [[0.0; Self::SAMPLES]; 3];
        for (curve, trc) in curves.iter_mut().zip(trcs) {
            let trc = trc.as_ref()?;
            for (i, sample) in curve.iter_mut().enumerate() {
                *sample = eval_trc(trc, i as f32 / (Self::SAMPLES - 1) as f32)?;
            }
        }
        Some(Self { colorants: profile.colorant_matrix().v, curves })
    }

    fn matches(&self, other: &Self) -> bool {
        let colorants = self.colorants.iter().flatten().zip(other.colorants.iter().flatten());
        let curves = self.curves.iter().flatten().zip(other.curves.iter().flatten());
        colorants.into_iter().all(|(a, b)| (a - b).abs() <= Self::COLORANT_TOLERANCE)
            && curves.into_iter().all(|(a, b)| (a - b).abs() <= Self::CURVE_TOLERANCE)
    }
}

/// A tone curve at `x` (0–1).
fn eval_trc(trc: &moxcms::ToneReprCurve, x: f32) -> Option<f32> {
    use moxcms::ToneReprCurve::{Lut, Parametric};
    Some(match trc {
        Lut(table) if table.is_empty() => x,
        Lut(table) if table.len() == 1 => x.powf(f32::from(table[0]) / 256.0),
        Lut(table) => {
            let at = x * (table.len() - 1) as f32;
            let i = (at.floor() as usize).min(table.len() - 2);
            let t = at - i as f32;
            (f32::from(table[i]) * (1.0 - t) + f32::from(table[i + 1]) * t) / 65535.0
        }
        Parametric(params) => moxcms::ParametricCurve::new(params)?.eval(x),
    })
}

/// A version 2 ICC profile for Adobe RGB (1998): moxcms's D50-adapted
/// colorants, D65 media white, one gamma of 563/256, no date.
const ADOBE_RGB_V2: &[u8] = include_bytes!("adobe_rgb_v2.icc");

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded(profile: moxcms::ColorProfile) -> Vec<u8> {
        profile.encode().expect("moxcms encodes its own profiles")
    }

    /// The shipped Adobe RGB profile's bytes and contents are pinned.
    #[test]
    fn the_shipped_profile_is_pinned() {
        let bytes = ADOBE_RGB_V2;
        let fnv1a = bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
            (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
        });
        assert_eq!((bytes.len(), fnv1a), (484, 0x9931_2edc_ca68_05ee));
        assert_eq!(&bytes[0..4], &484u32.to_be_bytes());
        assert_eq!(&bytes[8..24], b"\x02\x10\0\0mntrRGB XYZ ");
        let parsed = moxcms::ColorProfile::new_from_slice(bytes).expect("a valid profile");
        let adobe = moxcms::ColorProfile::new_adobe_rgb();
        let s15 = |c: moxcms::Xyzd| [c.x, c.y, c.z].map(|v| (v * 65536.0).round());
        assert_eq!(s15(parsed.red_colorant), s15(adobe.red_colorant));
        assert_eq!(s15(parsed.green_colorant), s15(adobe.green_colorant));
        assert_eq!(s15(parsed.blue_colorant), s15(adobe.blue_colorant));
        assert_eq!(
            s15(parsed.media_white_point.unwrap()),
            s15(moxcms::Xyzd::new(0.95045, 1.0, 1.08905))
        );
        for trc in [&parsed.red_trc, &parsed.green_trc, &parsed.blue_trc] {
            assert_eq!(trc, &Some(moxcms::ToneReprCurve::Lut(vec![563])));
        }
    }

    #[test]
    fn the_adobe_rgb_profile_reads_back_as_adobe_rgb() {
        let ours = IccProfile::adobe_rgb_compatible();
        let parsed = moxcms::ColorProfile::new_from_slice(ours.bytes()).expect("a valid profile");
        assert_eq!(parsed.color_space, moxcms::DataColorSpace::Rgb);
        assert_eq!(ours.name().as_deref(), Some("Compatible with Adobe RGB (1998)"));
        let moxcms_adobe = IccProfile::from_bytes(encoded(moxcms::ColorProfile::new_adobe_rgb()));
        assert!(ours.same_space(&moxcms_adobe.unwrap()), "same primaries and gamma");
        assert_eq!(ours.bytes(), IccProfile::adobe_rgb_compatible().bytes(), "deterministic");
    }

    #[test]
    fn display_p3_converts_to_srgb() {
        let p3 = IccProfile::from_bytes(encoded(moxcms::ColorProfile::new_display_p3())).unwrap();
        let to_srgb = p3.to_srgb().expect("a matrix profile converts");
        let from_u8 = |v: [u16; 3]| v.map(|c| c * 257);
        // sRGB (219, 61, 77) is (202, 74, 81) in Display P3; gray stays gray.
        let src = [from_u8([202, 74, 81]), from_u8([128, 128, 128])].concat();
        let mut dst = [0u16; 6];
        to_srgb.convert(&src, &mut dst);
        let got: Vec<i32> = dst.iter().map(|&v| (v as f32 / 257.0).round() as i32).collect();
        for (got, want) in got.iter().zip([219, 61, 77, 128, 128, 128]) {
            assert!((got - want).abs() <= 1, "{got:?} vs {want}: {dst:?}");
        }
    }

    /// The space a frame tagged `color` comes out in when not converted.
    fn encoded_space(color: ColorTag) -> Option<IccProfile> {
        source_space(&color, false, false).ok().unwrap().encoded
    }

    #[test]
    fn srgb_and_no_profile_are_none() {
        assert_eq!(encoded_space(ColorTag::Untagged), None);
        assert_eq!(encoded_space(ColorTag::Icc(encoded(moxcms::ColorProfile::new_srgb()))), None);
    }

    #[test]
    fn wide_gamut_profiles_are_kept_and_told_apart() {
        let p3_bytes = encoded(moxcms::ColorProfile::new_display_p3());
        let p3 = encoded_space(ColorTag::Icc(p3_bytes.clone())).expect("Display P3 is not sRGB");
        assert_eq!(p3.bytes(), p3_bytes.as_slice(), "an embedded profile is kept as it is");
        let prophoto =
            encoded_space(ColorTag::Icc(encoded(moxcms::ColorProfile::new_pro_photo_rgb())))
                .expect("ProPhoto is not sRGB");
        assert!(!p3.same_space(&prophoto));
        assert!(!p3.same_space(&IccProfile::adobe_rgb_compatible()));
        assert!(p3.same_space(&p3.clone()));
        let adobe = encoded_space(ColorTag::DcfAdobeRgb);
        assert_eq!(adobe, Some(IccProfile::adobe_rgb_compatible()), "an in-camera Adobe RGB JPEG");
    }

    /// A linear profile is linear, and comes out with its primaries on
    /// sRGB's curve: sRGB itself for sRGB's primaries.
    #[test]
    fn a_linear_profile_comes_out_on_the_srgb_curve() {
        let mut linear = moxcms::ColorProfile::new_srgb();
        let identity = moxcms::curve_from_gamma(1.0);
        linear.red_trc = Some(identity.clone());
        linear.green_trc = Some(identity.clone());
        linear.blue_trc = Some(identity);
        let space = source_space(&ColorTag::Icc(encoded(linear)), false, false).unwrap();
        assert!(space.linear);
        assert_eq!(space.encoded, None);
        let float = source_space(&ColorTag::Untagged, false, true).unwrap();
        assert!(float.linear && float.encoded.is_none(), "float without a tag: linear sRGB");
    }

    #[test]
    fn profiles_for_other_data_are_ignored() {
        let mut cmyk = encoded(moxcms::ColorProfile::new_srgb());
        cmyk[16..20].copy_from_slice(b"CMYK");
        assert_eq!(encoded_space(ColorTag::Icc(cmyk)), None);
        assert_eq!(encoded_space(ColorTag::Icc(vec![0; 40])), None, "malformed");
    }

    /// HDR transfers are refused wherever they are declared.
    #[test]
    fn hdr_transfers_are_refused() {
        let pq = ColorTag::Cicp { primaries: 9, transfer: 16, full_range: true };
        assert!(matches!(source_space(&pq, false, false), Err(Refused::Hdr)));
        let hlg = ColorTag::Cicp { primaries: 9, transfer: 18, full_range: true };
        assert!(matches!(source_space(&hlg, true, false), Err(Refused::Hdr)));
        let mut profile = moxcms::ColorProfile::new_bt2020_pq();
        profile.description = None;
        let bytes = encoded(profile);
        assert!(matches!(source_space(&ColorTag::Icc(bytes), false, false), Err(Refused::Hdr)));
        // Display P3 by code points is P3.
        let p3 = ColorTag::Cicp { primaries: 12, transfer: 13, full_range: true };
        let p3 = encoded_space(p3).expect("not sRGB");
        let ours = IccProfile::from_bytes(encoded(moxcms::ColorProfile::new_display_p3())).unwrap();
        assert!(p3.same_space(&ours));
    }

    fn chunks(srgb: bool, gamma: Option<f32>, chromaticities: Option<[(f32, f32); 4]>) -> ColorTag {
        ColorTag::PngChunks { srgb, gamma, chromaticities }
    }

    #[test]
    fn png_chunks_that_are_srgb_in_all_but_name_are_srgb() {
        assert_eq!(encoded_space(chunks(true, Some(1.0), None)), None, "sRGB chunk wins");
        assert_eq!(encoded_space(chunks(false, None, None)), None, "nothing declared");
        assert_eq!(encoded_space(chunks(false, Some(0.45455), None)), None, "gamma 1/2.2");
        let srgb = [(0.3127, 0.3290), (0.64, 0.33), (0.30, 0.60), (0.15, 0.06)];
        assert_eq!(encoded_space(chunks(false, Some(0.45455), Some(srgb))), None);
    }

    /// A PNG with Adobe RGB's primaries gets a profile of its own, which
    /// the stack then keeps.
    #[test]
    fn png_chunks_for_other_spaces_make_a_profile() {
        let adobe = [(0.3127, 0.3290), (0.64, 0.33), (0.21, 0.71), (0.15, 0.06)];
        let profile = encoded_space(chunks(false, Some(1.0 / 2.2), Some(adobe))).expect("not sRGB");
        let parsed = profile.parsed().unwrap();
        let ours = IccProfile::adobe_rgb_compatible().parsed().unwrap();
        for (a, b) in [
            (parsed.red_colorant, ours.red_colorant),
            (parsed.green_colorant, ours.green_colorant),
            (parsed.blue_colorant, ours.blue_colorant),
        ] {
            for (a, b) in [(a.x, b.x), (a.y, b.y), (a.z, b.z)] {
                assert!((a - b).abs() < 2e-3, "Adobe RGB colorants, adapted to D50");
            }
        }
        let linear = source_space(&chunks(false, Some(1.0), None), false, false).unwrap();
        assert!(linear.linear, "gamma 1.0");
    }
}
