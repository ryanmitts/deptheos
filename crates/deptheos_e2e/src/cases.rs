//! The end-to-end cases from `e2e.toml`. The desktop runs them directly;
//! iOS through `script/e2e-ios.sh`, as JSON ([`Case::to_json`]).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use deptheos_adjust::reframe::Rect;
use deptheos_adjust::{
    Adjustments, Flip, Reframe, Sharpening, Turn, Vignette, WhiteBalance, slider,
};
use deptheos_app::library::export::ExportFormat;

use crate::Expect;

/// The file, as read.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The sets' folder (`~` allowed); `DEPTHEOS_E2E_SETS` overrides it.
    sets: String,
    #[serde(rename = "case")]
    pub cases: Vec<Case>,
}

/// Where a case runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Desktop,
    Ios,
}

/// How often a case runs: `just e2e` runs smoke and full, the others only
/// when asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    /// A few small cases, in a couple of minutes: `just e2e-smoke`.
    Smoke,
    #[default]
    Full,
    /// Edge cases of the set itself (two frames, hundreds, unaligned).
    Stress,
    /// Needs Adobe DNG Converter installed.
    Converter,
}

impl Tier {
    fn named(name: &str) -> Result<Tier, String> {
        serde_json::from_value(serde_json::Value::String(name.trim().into()))
            .map_err(|_| format!("tier {name}: smoke, full, stress or converter"))
    }
}

/// A stacking method, as the apps name it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Method {
    #[default]
    Pyramid,
    Depth,
    Wavelet,
}

/// The desktop's RAW decoders, as its Settings name them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum Decoder {
    #[serde(rename = "libraw")]
    LibRaw,
    #[serde(rename = "adobe-dng")]
    AdobeDng,
}

/// One case: a set, stacked one way, exported, maybe retouched.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    /// Its name, for `just e2e NAME` and its references' folder.
    pub name: String,
    /// Its frames' folder, under the sets' folder.
    pub set: String,
    #[serde(default)]
    pub method: Method,
    /// Pyramid's Smoothness, 1–10.
    pub smoothness: Option<f32>,
    /// Depth's Radius, 1–50.
    pub radius: Option<f32>,
    /// Depth's Smoothing, 0–20.
    pub smoothing: Option<f32>,
    /// Depth's Blend Range, 0–10.
    pub blend_range: Option<f32>,
    /// Wavelet's Denoise, 0–10.
    pub denoise: Option<f32>,
    /// The stack's bits a channel, 8 or 16 ([`Export::bits`]): 8 only when
    /// every frame is 8-bit and stacks as stored
    /// (`crates/deptheos_core/src/io/input.rs`, `plan`). Stated by the case
    /// so an app offering the wrong depth fails.
    pub stack_bits: u8,
    /// What the stack is exported as, in order.
    #[serde(default = "default_exports")]
    pub exports: Vec<Export>,
    /// Retouch it with one stroke across the middle and export a TIFF.
    #[serde(default)]
    pub paint: bool,
    /// The frame painted from, by file name. By default (and always on
    /// iOS) the app picks the one with the least detail under the stroke,
    /// so the stroke shows.
    pub paint_from: Option<String>,
    /// The desktop's RAW decoder, named so the case runs the same on every
    /// Mac (Automatic depends on Adobe DNG Converter being installed).
    /// Desktop only.
    pub decoder: Option<Decoder>,
    /// The set its exports are checked against, when not `set`: the same
    /// frames by name from another decoder (the converter's DNGs, whose
    /// color and crop differ from LibRaw's). Desktop only.
    pub check_set: Option<String>,
    /// Where it runs.
    #[serde(default = "both")]
    pub on: Vec<Platform>,
    /// How often it runs ([`Tier`]).
    #[serde(default)]
    pub tier: Tier,
    /// The set the Simulator stacks, when not `set`: its frames shrunk to
    /// what its GPU takes (about 16.7 MP, ADR-110) by script/e2e-sets.sh.
    pub ios_set: Option<String>,
    /// The sliders set before exporting ([`Adjust`]).
    pub adjust: Option<Adjust>,
    /// The output space of a camera RAW stack's rendered exports.
    pub space: Option<OutputSpace>,
    /// The crop, flip and turn of the exports ([`CaseReframe`]).
    pub reframe: Option<CaseReframe>,
}

/// A case's adjustments, each slider optional (its default otherwise).
/// The white balance is `kelvin` for a camera RAW stack or `temperature`
/// (the relative scale) for a standard one, each with its `tint`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Adjust {
    pub exposure: Option<f64>,
    pub kelvin: Option<f64>,
    pub temperature: Option<f64>,
    pub tint: Option<f64>,
    pub contrast: Option<f64>,
    pub highlights: Option<f64>,
    pub shadows: Option<f64>,
    pub whites: Option<f64>,
    pub blacks: Option<f64>,
    pub clarity: Option<f64>,
    pub texture: Option<f64>,
    pub sharpening: Option<f64>,
    pub sharpen_radius: Option<f64>,
    pub sharpen_detail: Option<f64>,
    pub sharpen_masking: Option<f64>,
    pub vibrance: Option<f64>,
    pub saturation: Option<f64>,
    pub vignette: Option<f64>,
    pub vignette_midpoint: Option<f64>,
}

impl Adjust {
    /// As the engine takes them.
    pub fn adjustments(&self) -> Adjustments {
        let n = Adjustments::NEUTRAL;
        let tint = self.tint.unwrap_or(0.0);
        let white_balance = match (self.kelvin, self.temperature) {
            (Some(temperature), _) => WhiteBalance::Kelvin { temperature, tint },
            (None, Some(temperature)) => WhiteBalance::Relative { temperature, tint },
            (None, None) => WhiteBalance::AsShot,
        };
        Adjustments {
            exposure: self.exposure.unwrap_or(n.exposure),
            white_balance,
            contrast: self.contrast.unwrap_or(n.contrast),
            highlights: self.highlights.unwrap_or(n.highlights),
            shadows: self.shadows.unwrap_or(n.shadows),
            whites: self.whites.unwrap_or(n.whites),
            blacks: self.blacks.unwrap_or(n.blacks),
            clarity: self.clarity.unwrap_or(n.clarity),
            texture: self.texture.unwrap_or(n.texture),
            sharpening: Sharpening {
                amount: self.sharpening.unwrap_or(n.sharpening.amount),
                radius: self.sharpen_radius.unwrap_or(n.sharpening.radius),
                detail: self.sharpen_detail.unwrap_or(n.sharpening.detail),
                masking: self.sharpen_masking.unwrap_or(n.sharpening.masking),
            },
            vibrance: self.vibrance.unwrap_or(n.vibrance),
            saturation: self.saturation.unwrap_or(n.saturation),
            vignette: Vignette {
                amount: self.vignette.unwrap_or(n.vignette.amount),
                midpoint: self.vignette_midpoint.unwrap_or(n.vignette.midpoint),
            },
            ..n
        }
    }

    fn validate(&self) -> Result<(), String> {
        let tint = match (self.kelvin, self.temperature) {
            (Some(_), Some(_)) => {
                return Err(
                    "adjust: kelvin (camera RAW) or temperature (standard), not both".into()
                );
            }
            (None, None) if self.tint.is_some() => {
                return Err("adjust: tint goes with kelvin or temperature".into());
            }
            (None, Some(_)) => slider::TINT_RELATIVE,
            _ => slider::TINT_RAW,
        };
        let sliders = [
            ("exposure", self.exposure, slider::EXPOSURE),
            ("kelvin", self.kelvin, slider::TEMPERATURE_KELVIN),
            ("temperature", self.temperature, slider::TEMPERATURE_RELATIVE),
            ("tint", self.tint, tint),
            ("contrast", self.contrast, slider::CONTRAST),
            ("highlights", self.highlights, slider::HIGHLIGHTS),
            ("shadows", self.shadows, slider::SHADOWS),
            ("whites", self.whites, slider::WHITES),
            ("blacks", self.blacks, slider::BLACKS),
            ("clarity", self.clarity, slider::CLARITY),
            ("texture", self.texture, slider::TEXTURE),
            ("sharpening", self.sharpening, slider::SHARPENING),
            ("sharpen_radius", self.sharpen_radius, slider::SHARPEN_RADIUS),
            ("sharpen_detail", self.sharpen_detail, slider::SHARPEN_DETAIL),
            ("sharpen_masking", self.sharpen_masking, slider::SHARPEN_MASKING),
            ("vibrance", self.vibrance, slider::VIBRANCE),
            ("saturation", self.saturation, slider::SATURATION),
            ("vignette", self.vignette, slider::VIGNETTE),
            ("vignette_midpoint", self.vignette_midpoint, slider::VIGNETTE_MIDPOINT),
        ];
        let part = self.sharpen_radius.or(self.sharpen_detail).or(self.sharpen_masking);
        if part.is_some() && !self.sharpening.is_some_and(|v| v != 0.0) {
            return Err(
                "adjust: sharpen_radius, sharpen_detail and sharpen_masking go with sharpening"
                    .into(),
            );
        }
        if self.vignette_midpoint.is_some() && !self.vignette.is_some_and(|v| v != 0.0) {
            return Err("adjust: vignette_midpoint goes with vignette".into());
        }
        for (name, value, spec) in sliders {
            let Some(value) = value else { continue };
            if !(spec.min..=spec.max).contains(&value) {
                return Err(format!("adjust: {name} {value} is outside {}–{}", spec.min, spec.max));
            }
        }
        Ok(())
    }
}

/// The output space of a camera RAW stack's JPEG, PNG and TIFF exports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum OutputSpace {
    #[serde(rename = "srgb")]
    Srgb,
    #[serde(rename = "display-p3")]
    DisplayP3,
    #[serde(rename = "adobe-rgb")]
    AdobeRgb,
    #[serde(rename = "prophoto")]
    ProPhoto,
}

/// A case's reframe: `crop` as left, top, width and height in the
/// stack's pixels, then `flip`, then `turn` clockwise in degrees.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CaseReframe {
    pub crop: Option<[u32; 4]>,
    pub flip: Option<CaseFlip>,
    #[serde(default)]
    pub turn: u32,
}

/// A mirror: left to right, or top to bottom (that and a half turn).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CaseFlip {
    Horizontal,
    Vertical,
}

impl CaseReframe {
    /// As the engine takes it (ADR-136).
    pub fn reframe(&self) -> Reframe {
        let crop = self.crop.map(|[x, y, width, height]| Rect { x, y, width, height });
        let turn = match self.turn {
            90 => Turn::Quarter,
            180 => Turn::Half,
            270 => Turn::ThreeQuarters,
            _ => Turn::None,
        };
        let (flip, turn) = match self.flip {
            None => (Flip::None, turn),
            Some(CaseFlip::Horizontal) => (Flip::Horizontal, turn),
            Some(CaseFlip::Vertical) => (Flip::Horizontal, Turn::Half.then(turn)),
        };
        Reframe { crop, flip, turn }
    }

    fn validate(&self) -> Result<(), String> {
        if !matches!(self.turn, 0 | 90 | 180 | 270) {
            return Err(format!("reframe: turn {}: 0, 90, 180 or 270", self.turn));
        }
        if let Some([_, _, width, height]) = self.crop
            && (width == 0 || height == 0)
        {
            return Err("reframe: a crop's width and height are more than 0".into());
        }
        Ok(())
    }
}

fn default_exports() -> Vec<Export> {
    vec![Export { format: ExportFormat::Jpeg, sixteen_bit: false }]
}

fn both() -> Vec<Platform> {
    vec![Platform::Desktop, Platform::Ios]
}

/// An export: "jpeg", "png-8", "png-16", "tiff-8", "tiff-16" or "dng".
/// 16 bits of an 8-bit stack comes out as 8, as the apps offer only that.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct Export {
    pub format: ExportFormat,
    pub sixteen_bit: bool,
}

impl Export {
    /// Its bits a channel, of a stack of `stack_bits`.
    pub fn bits(self, stack_bits: u8) -> u8 {
        match self.format {
            ExportFormat::Jpeg => 8,
            ExportFormat::Dng => 16,
            ExportFormat::Png | ExportFormat::Tiff => {
                if self.sixteen_bit {
                    stack_bits
                } else {
                    8
                }
            }
        }
    }
}

impl TryFrom<String> for Export {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        let (format, sixteen_bit) = match s.as_str() {
            "jpeg" => (ExportFormat::Jpeg, false),
            "png-8" => (ExportFormat::Png, false),
            "png-16" => (ExportFormat::Png, true),
            "tiff-8" => (ExportFormat::Tiff, false),
            "tiff-16" => (ExportFormat::Tiff, true),
            "dng" => (ExportFormat::Dng, true),
            _ => {
                return Err(format!(
                    "export \"{s}\": one of jpeg, png-8, png-16, tiff-8, tiff-16, dng"
                ));
            }
        };
        Ok(Export { format, sixteen_bit })
    }
}

impl From<Export> for String {
    fn from(e: Export) -> String {
        match (e.format, e.sixteen_bit) {
            (ExportFormat::Jpeg, _) => "jpeg",
            (ExportFormat::Png, false) => "png-8",
            (ExportFormat::Png, true) => "png-16",
            (ExportFormat::Tiff, false) => "tiff-8",
            (ExportFormat::Tiff, true) => "tiff-16",
            (ExportFormat::Dng, _) => "dng",
        }
        .into()
    }
}

/// The repository's `e2e.toml`.
pub fn default_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../e2e.toml")
}

impl Config {
    pub fn load(path: &Path) -> Result<Config, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let config: Config =
            toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut names = std::collections::HashSet::new();
        for case in &config.cases {
            if !names.insert(&case.name) {
                return Err(format!("{}: two cases named {}", path.display(), case.name));
            }
            case.validate().map_err(|e| format!("{}: case {}: {e}", path.display(), case.name))?;
        }
        Ok(config)
    }

    /// The sets' folder: `DEPTHEOS_E2E_SETS`, or the file's.
    pub fn sets_dir(&self) -> PathBuf {
        match std::env::var_os("DEPTHEOS_E2E_SETS") {
            Some(dir) => PathBuf::from(dir),
            None => expand_home(&self.sets),
        }
    }

    /// The case named `name`.
    pub fn case(&self, name: &str) -> Result<&Case, String> {
        self.cases.iter().find(|c| c.name == name).ok_or_else(|| {
            let names: Vec<&str> = self.cases.iter().map(|c| c.name.as_str()).collect();
            format!("no case named {name}; the cases: {}", names.join(", "))
        })
    }

    /// The cases on `platform`: those named in `only` (comma-separated),
    /// else those of `tiers` (smoke and full by default).
    pub fn cases_on(
        &self,
        platform: Platform,
        only: Option<&str>,
        tiers: Option<&str>,
    ) -> Result<Vec<&Case>, String> {
        let tiers: Vec<Tier> = match tiers.filter(|t| !t.trim().is_empty()) {
            Some(tiers) => tiers.split(',').map(Tier::named).collect::<Result<_, _>>()?,
            None => vec![Tier::Smoke, Tier::Full],
        };
        match only.filter(|o| !o.trim().is_empty()) {
            Some(only) => only
                .split(',')
                .map(|name| {
                    let case = self.case(name.trim())?;
                    if case.on.contains(&platform) {
                        Ok(case)
                    } else {
                        Err(format!("case {} doesn't run on {platform:?} (its `on`)", case.name))
                    }
                })
                .collect(),
            None => Ok(self
                .cases
                .iter()
                .filter(|c| c.on.contains(&platform) && tiers.contains(&c.tier))
                .collect()),
        }
    }
}

impl Case {
    /// Its frames' folder.
    pub fn set_dir(&self, config: &Config) -> PathBuf {
        config.sets_dir().join(&self.set)
    }

    /// The frames its exports are checked against.
    pub fn check_dir(&self, config: &Config) -> PathBuf {
        config.sets_dir().join(self.check_set.as_ref().unwrap_or(&self.set))
    }

    /// The case as JSON, for the iOS app and its test.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("a case is plain data")
    }

    /// The case for `script/e2e-ios.sh`: its JSON plus the Simulator's set
    /// folder (`setDir`) and that set's readable frames in name order
    /// (`frames`), which the script copies.
    pub fn for_ios(&self, config: &Config) -> Result<String, String> {
        self.ios_can_run()?;
        let dir = config.sets_dir().join(self.ios_set.as_ref().unwrap_or(&self.set));
        let set = crate::Set::open_checked(&dir)?;
        let mut json: serde_json::Value =
            serde_json::from_str(&self.to_json()).expect("a case is plain data");
        json["setDir"] = dir.display().to_string().into();
        json["frames"] = set.frames.iter().map(|f| f.display().to_string()).collect();
        Ok(json.to_string())
    }

    /// What its export as `format`, `bits` a channel, should be.
    pub fn expect(&self, format: ExportFormat, bits: u8) -> Expect {
        Expect {
            adjustments: self.adjust.map(|a| a.adjustments()).unwrap_or_default(),
            space: self.space,
            reframe: self.reframe.map(|r| r.reframe()).unwrap_or_default(),
            ..Expect::new(format, bits)
        }
    }

    /// Refuses a case that sets `adjust`, `space` or `reframe` on iOS, which
    /// has no Adjust to apply them; the desktop's driver applies them in its
    /// Adjust mode and Export sheet.
    pub fn ios_can_run(&self) -> Result<(), String> {
        let set: Vec<&str> = [
            ("adjust", self.adjust.is_some()),
            ("space", self.space.is_some()),
            ("reframe", self.reframe.is_some()),
        ]
        .into_iter()
        .filter_map(|(key, set)| set.then_some(key))
        .collect();
        let keys = match set.as_slice() {
            [] => return Ok(()),
            [one] => one.to_string(),
            [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
        };
        Err(format!(
            "case {} sets {keys}, which the iOS app can't apply: it has no Adjust",
            self.name
        ))
    }

    fn validate(&self) -> Result<(), String> {
        let settings: [Setting; 5] = [
            ("smoothness", self.smoothness, Method::Pyramid, (1.0, 10.0)),
            ("radius", self.radius, Method::Depth, (1.0, 50.0)),
            ("smoothing", self.smoothing, Method::Depth, (0.0, 20.0)),
            ("blend_range", self.blend_range, Method::Depth, (0.0, 10.0)),
            ("denoise", self.denoise, Method::Wavelet, (0.0, 10.0)),
        ];
        for (name, value, method, (low, high)) in settings {
            let Some(value) = value else { continue };
            if method != self.method {
                return Err(format!("{name} is a setting of {method:?}, not {:?}", self.method));
            }
            if !(low..=high).contains(&value) {
                return Err(format!("{name} {value} is outside {low}–{high}"));
            }
        }
        if !matches!(self.stack_bits, 8 | 16) {
            return Err(format!("stack_bits {}: 8 or 16", self.stack_bits));
        }
        if self.stack_bits == 8 && self.exports.iter().any(|e| e.format == ExportFormat::Dng) {
            return Err("dng is a linear stack's, which is 16-bit: give stack_bits = 16".into());
        }
        if self.exports.is_empty() && !self.paint {
            return Err("nothing to export: give `exports`, or `paint = true`".into());
        }
        if self.on.is_empty() {
            return Err("`on` names no app".into());
        }
        if self.paint_from.is_some() && !self.paint {
            return Err("paint_from without paint = true".into());
        }
        if self.ios_set.is_some() && !self.on.contains(&Platform::Ios) {
            return Err("ios_set without on = [\"ios\", …]".into());
        }
        if self.check_set.is_some() && self.on.contains(&Platform::Ios) {
            return Err("check_set is the desktop's: give on = [\"desktop\"]".into());
        }
        if self.decoder.is_some() && self.on.contains(&Platform::Ios) {
            return Err("decoder is the desktop's: give on = [\"desktop\"]".into());
        }
        let dng = self.exports.iter().any(|e| e.format == ExportFormat::Dng);
        if let Some(adjust) = &self.adjust {
            adjust.validate()?;
            if adjust.kelvin.is_some() && self.stack_bits != 16 {
                return Err("adjust: kelvin is a camera RAW stack's, which is 16-bit".into());
            }
            if adjust.temperature.is_some() && (dng || self.space.is_some()) {
                return Err(
                    "adjust: temperature is a standard stack's; a camera RAW one takes kelvin"
                        .into(),
                );
            }
        }
        if self.space.is_some() && self.stack_bits != 16 {
            return Err("space is a camera RAW stack's, which is 16-bit".into());
        }
        if let Some(reframe) = &self.reframe {
            reframe.validate()?;
            if dng {
                return Err(
                    "reframe: a DNG keeps every pixel, its crop a tag the checker doesn't read"
                        .into(),
                );
            }
        }
        Ok(())
    }
}

/// A setting's name, its value in a case, the method it belongs to and
/// its range.
type Setting = (&'static str, Option<f32>, Method, (f32, f32));

fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(rest),
        None => PathBuf::from(path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Config, String> {
        let dir = std::env::temp_dir().join(format!("e2e-cases-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{}.toml", text.len()));
        std::fs::write(&path, text).unwrap();
        Config::load(&path)
    }

    #[test]
    fn the_repositorys_file_reads() {
        let config = Config::load(&default_path()).unwrap();
        assert!(!config.cases.is_empty());
    }

    #[test]
    fn a_case_takes_its_defaults() {
        let config =
            parse("sets = \"~/s\"\n[[case]]\nname = \"a\"\nset = \"jpeg/a\"\nstack_bits = 8\n")
                .unwrap();
        let case = &config.cases[0];
        assert_eq!(case.method, Method::Pyramid);
        assert_eq!(case.exports, default_exports());
        assert!(!case.paint);
        assert_eq!(case.on, both());
        assert_eq!(case.ios_set, None);
    }

    #[test]
    fn mistakes_are_named() {
        let bad = [
            ("radius = 8.0", "radius is a setting of Depth"),
            ("method = \"depth\"\nradius = 80.0", "outside 1–50"),
            ("exports = [\"tiff\"]", "one of jpeg"),
            ("colour = 1", "unknown field"),
            ("paint_from = \"x.jpg\"", "paint_from without paint"),
            ("decoder = \"libraw\"", "decoder is the desktop's"),
            ("decoder = \"dcraw\"\non = [\"desktop\"]", "unknown variant"),
            ("stack_bits = 12", "8 or 16"),
            ("stack_bits = 8\nexports = [\"dng\"]", "linear stack's"),
            ("name = \"b\"", "missing field `stack_bits`"),
            ("adjust = { exposure = 6.0 }", "exposure 6 is outside -5–5"),
            ("adjust = { grain = 1 }", "unknown field `grain`"),
            ("adjust = { kelvin = 1500 }", "kelvin 1500 is outside 2000–50000"),
            ("adjust = { kelvin = 5000 }", "kelvin is a camera RAW stack's"),
            ("stack_bits = 16\nadjust = { kelvin = 5000, tint = 151 }", "outside -150–150"),
            ("adjust = { temperature = 20, tint = 101 }", "tint 101 is outside -100–100"),
            ("stack_bits = 16\nadjust = { kelvin = 5000, temperature = 1 }", "not both"),
            ("adjust = { tint = 5 }", "tint goes with kelvin or temperature"),
            ("adjust = { saturation = -101 }", "saturation -101 is outside"),
            ("adjust = { vignette = -101 }", "vignette -101 is outside"),
            ("adjust = { vignette_midpoint = 30 }", "vignette_midpoint goes with vignette"),
            (
                "stack_bits = 16\nexports = [\"dng\"]\nadjust = { temperature = 10 }",
                "a camera RAW one takes kelvin",
            ),
            ("space = \"srgb\"", "space is a camera RAW stack's"),
            ("stack_bits = 16\nspace = \"rec2020\"", "unknown variant"),
            ("reframe = { turn = 45 }", "turn 45: 0, 90, 180 or 270"),
            ("reframe = { crop = [0, 0, 0, 10] }", "more than 0"),
            ("reframe = { crop = [0, 0, 10] }", "invalid length"),
            ("reframe = { flip = \"diagonal\" }", "unknown variant"),
            ("stack_bits = 16\nexports = [\"dng\"]\nreframe = { turn = 90 }", "a DNG keeps every"),
        ];
        for (line, said) in bad {
            let bits = if line.contains("stack_bits") || line.contains("name") {
                ""
            } else {
                "stack_bits = 8\n"
            };
            let name = if line.contains("name") { "" } else { "name = \"a\"\n" };
            let text = format!("sets = \"s\"\n[[case]]\n{name}set = \"a\"\n{bits}{line}\n");
            let error = parse(&text).unwrap_err();
            assert!(error.contains(said), "{line}: {error}");
        }
    }

    /// A case's adjustments, space and reframe read as the engine takes
    /// them, and the iOS driver refuses it.
    #[test]
    fn adjustments_space_and_reframe_read_and_are_refused_on_ios() {
        let config = parse(
            "sets = \"s\"\n[[case]]\nname = \"a\"\nset = \"a\"\nstack_bits = 16\n\
             adjust = { exposure = 0.5, kelvin = 4300, tint = 12, vibrance = 20, clarity = 15, \
             sharpening = 40, sharpen_radius = 0.8, vignette = -25, vignette_midpoint = 60 }\n\
             space = \"display-p3\"\n\
             reframe = { crop = [10, 20, 300, 200], flip = \"vertical\", turn = 90 }\n\
             [[case]]\nname = \"b\"\nset = \"b\"\nstack_bits = 8\n\
             adjust = { temperature = -30 }\n",
        )
        .unwrap();
        let (a, b) = (&config.cases[0], &config.cases[1]);
        let expect = a.expect(ExportFormat::Jpeg, 8);
        assert_eq!(expect.space, Some(OutputSpace::DisplayP3));
        assert_eq!(
            expect.adjustments,
            Adjustments {
                exposure: 0.5,
                white_balance: WhiteBalance::Kelvin { temperature: 4300.0, tint: 12.0 },
                vibrance: 20.0,
                clarity: 15.0,
                sharpening: Sharpening { amount: 40.0, radius: 0.8, ..Sharpening::NONE },
                vignette: Vignette { amount: -25.0, midpoint: 60.0 },
                ..Adjustments::NEUTRAL
            }
        );
        // Top to bottom is the mirror and a half turn, then the case's turn.
        assert_eq!(
            expect.reframe,
            Reframe {
                crop: Some(Rect { x: 10, y: 20, width: 300, height: 200 }),
                flip: Flip::Horizontal,
                turn: Turn::ThreeQuarters,
            }
        );
        assert_eq!(
            b.expect(ExportFormat::Png, 8).adjustments.white_balance,
            WhiteBalance::Relative { temperature: -30.0, tint: 0.0 }
        );
        let refused = a.ios_can_run().unwrap_err();
        assert!(refused.contains("adjust, space and reframe"), "{refused}");
        assert!(a.for_ios(&config).unwrap_err().contains("can't apply"));
        assert!(b.ios_can_run().unwrap_err().contains("sets adjust,"));
        let back: Case = serde_json::from_str(&a.to_json()).unwrap();
        assert_eq!((back.adjust, back.space, back.reframe), (a.adjust, a.space, a.reframe));
        // The repository's iOS cases set none of them.
        let repository = Config::load(&default_path()).unwrap();
        for case in repository.cases.iter().filter(|c| c.on.contains(&Platform::Ios)) {
            case.ios_can_run().unwrap();
        }
    }

    #[test]
    fn a_case_goes_to_ios_and_back() {
        let config = parse(
            "sets = \"s\"\n[[case]]\nname = \"a\"\nset = \"a\"\nstack_bits = 16\nmethod = \"depth\"\nradius = 12.0\nexports = [\"png-16\", \"jpeg\"]\npaint = true\n",
        )
        .unwrap();
        let json = config.cases[0].to_json();
        assert!(json.contains("\"method\":\"depth\""), "{json}");
        assert!(json.contains("\"exports\":[\"png-16\",\"jpeg\"]"), "{json}");
        let back: Case = serde_json::from_str(&json).unwrap();
        assert_eq!(back.radius, Some(12.0));
    }

    #[test]
    fn cases_run_by_tier_or_by_name() {
        let config = parse(
            "sets = \"s\"\n\
             [[case]]\nname = \"a\"\nset = \"a\"\nstack_bits = 8\ntier = \"smoke\"\n\
             [[case]]\nname = \"b\"\nset = \"b\"\nstack_bits = 8\n\
             [[case]]\nname = \"c\"\nset = \"c\"\nstack_bits = 8\ntier = \"stress\"\n",
        )
        .unwrap();
        let names = |only: Option<&str>, tiers: Option<&str>| -> Vec<String> {
            let cases = config.cases_on(Platform::Desktop, only, tiers).unwrap();
            cases.iter().map(|c| c.name.clone()).collect()
        };
        assert_eq!(names(None, None), ["a", "b"]);
        assert_eq!(names(None, Some("smoke")), ["a"]);
        assert_eq!(names(None, Some("stress,full")), ["b", "c"]);
        assert_eq!(names(Some("c"), None), ["c"]);
        assert!(config.cases_on(Platform::Desktop, None, Some("nightly")).is_err());
    }

    #[test]
    fn an_export_has_the_bits_its_stack_gives() {
        let e = |s: &str| Export::try_from(s.to_string()).unwrap();
        assert_eq!(e("jpeg").bits(16), 8);
        assert_eq!(e("png-16").bits(8), 8);
        assert_eq!(e("tiff-16").bits(16), 16);
        assert_eq!(e("tiff-8").bits(16), 8);
        assert_eq!(e("dng").bits(16), 16);
    }

    #[test]
    fn ios_is_given_every_frame_the_checker_reads() {
        let sets = tempfile::tempdir().unwrap();
        let set = sets.path().join("raw");
        std::fs::create_dir(&set).unwrap();
        for name in ["a.NEF", "b.arw", "c.RAF", "d.cr3", "e.jpg", "Thumbs.db", "f.zip"] {
            std::fs::write(set.join(name), b"").unwrap();
        }
        crate::Set::open(&set).unwrap().write_manifest().unwrap();
        let text = format!(
            "sets = {:?}\n[[case]]\nname = \"a\"\nset = \"raw\"\nstack_bits = 16\n",
            sets.path()
        );
        let config = parse(&text).unwrap();
        let json: serde_json::Value =
            serde_json::from_str(&config.cases[0].for_ios(&config).unwrap()).unwrap();
        let frames: Vec<&str> = json["frames"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| Path::new(f.as_str().unwrap()).file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(frames, ["a.NEF", "b.arw", "c.RAF", "d.cr3", "e.jpg"]);
        assert_eq!(json["setDir"], set.display().to_string());
    }
}
