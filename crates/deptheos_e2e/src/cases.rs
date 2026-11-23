//! The end-to-end cases from `e2e.toml`. The desktop runs them directly;
//! iOS through `script/e2e-ios.sh`, as JSON ([`Case::to_json`]).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use deptheos_app::library::export::ExportFormat;

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
        let dir = config.sets_dir().join(self.ios_set.as_ref().unwrap_or(&self.set));
        let set = crate::Set::open_checked(&dir)?;
        let mut json: serde_json::Value =
            serde_json::from_str(&self.to_json()).expect("a case is plain data");
        json["setDir"] = dir.display().to_string().into();
        json["frames"] = set.frames.iter().map(|f| f.display().to_string()).collect();
        Ok(json.to_string())
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
