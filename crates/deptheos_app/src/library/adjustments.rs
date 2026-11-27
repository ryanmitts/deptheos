//! A render's adjustments and its reframe, saved in `adjustments.json` in
//! its folder (ADR-152). No file is neutral with no reframe, as every
//! render made before format 3 is. A damaged file reads as [`Unread`], so
//! an export fails rather than coming out unadjusted, and is set aside by
//! the next write. Writes run on the store thread, one per finished
//! change; a write a later one overtakes is skipped ([`Write::save`]).

pub mod rules;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use deptheos_render::reframe::{Flip, Rect, Size, Turn};
use deptheos_render::{
    Adjustments, ProcessVersion, Reframe, Sharpening, Vignette, WhiteBalance, slider,
};
use serde::{Deserialize, Serialize};

use crate::fsutil::{StoreError, Versioned, parse_versioned, read_if_present, write_json_atomic};

pub const ADJUSTMENTS_FILE: &str = "adjustments.json";
/// The file's format version (ADR-141).
pub(crate) const ADJUSTMENTS_SCHEMA: u32 = 1;

/// What a render is drawn under: its sliders and its reframe.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RenderAdjustments {
    pub adjustments: Adjustments,
    pub reframe: Reframe,
}

impl RenderAdjustments {
    /// Neutral, with no reframe: a render with no file.
    pub const NONE: Self = Self { adjustments: Adjustments::NEUTRAL, reframe: Reframe::NONE };
}

/// Why a render's adjustments couldn't be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Unread {
    /// Not this format, an unknown process version, or a crop outside the
    /// picture. Kept until the next write sets it aside.
    #[error("it is damaged")]
    Damaged,
    /// Reading failed for a reason unrelated to the file (access, device).
    #[error("it couldn't be read just now")]
    Unreachable,
}

/// The adjustments of the render in `dir`, whose stored picture is
/// `picture`.
pub fn read(dir: &Path, picture: Size) -> Result<RenderAdjustments, Unread> {
    let path = dir.join(ADJUSTMENTS_FILE);
    let bytes = match read_if_present(&path) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return Ok(RenderAdjustments::NONE),
        Err(e) => {
            tracing::warn!("{}: not read: {e}", path.display());
            return Err(Unread::Unreachable);
        }
    };
    parse(&path, &bytes, picture).map_err(|why| {
        tracing::warn!("{}: damaged: {why}", path.display());
        Unread::Damaged
    })
}

fn parse(path: &Path, bytes: &[u8], picture: Size) -> Result<RenderAdjustments, String> {
    let file: AdjustmentsFile = parse_versioned(path, bytes).map_err(|e| e.to_string())?;
    let value = file.value().ok_or_else(|| "unknown process version".to_string())?;
    value.reframe.window(picture).map_err(|o| format!("crop {:?} outside the picture", o.crop))?;
    Ok(value)
}

/// Write `value` as the adjustments of the render in `dir`, setting a
/// file not of this format aside first. Values that aren't finite numbers are
/// written as their sliders' defaults.
pub(crate) fn write(dir: &Path, value: &RenderAdjustments) -> Result<(), StoreError> {
    let path = dir.join(ADJUSTMENTS_FILE);
    if let Ok(Some(bytes)) = read_if_present(&path)
        && parse_versioned::<AdjustmentsFile>(&path, &bytes).ok().and_then(|f| f.value()).is_none()
    {
        crate::fsutil::set_aside(&path).map_err(crate::fsutil::io_error(&path))?;
    }
    write_json_atomic(&path, &AdjustmentsFile::of(value))?;
    tracing::debug!("{}: written", path.display());
    Ok(())
}

/// The writes of one render's adjustments, in the order sent. Clones
/// share the count, so a write any of them sent later overtakes one
/// sent before it.
#[derive(Clone, Debug)]
pub struct Writes {
    dir: PathBuf,
    sent: Arc<AtomicU64>,
}

/// One write, to [`Write::save`] on the store thread.
#[derive(Debug)]
pub struct Write {
    dir: PathBuf,
    value: RenderAdjustments,
    number: u64,
    sent: Arc<AtomicU64>,
}

/// What a [`Write::save`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Saved {
    Written,
    /// A later write was sent, which writes in its place.
    Overtaken,
}

impl Writes {
    /// The writes of the render in `dir`.
    pub fn of(dir: PathBuf) -> Self {
        Self { dir, sent: Arc::new(AtomicU64::new(0)) }
    }

    /// `value`, numbered after every write sent before it.
    pub fn send(&self, value: RenderAdjustments) -> Write {
        let number = self.sent.fetch_add(1, Ordering::AcqRel) + 1;
        Write { dir: self.dir.clone(), value, number, sent: self.sent.clone() }
    }
}

impl Write {
    /// Whether a later write was sent.
    pub fn is_overtaken(&self) -> bool {
        self.number < self.sent.load(Ordering::Acquire)
    }

    pub fn value(&self) -> &RenderAdjustments {
        &self.value
    }

    /// Write it, unless overtaken. Runs on the store thread (ADR-143).
    pub fn save(self) -> Result<Saved, StoreError> {
        if self.is_overtaken() {
            tracing::debug!("{}: write {} overtaken", self.dir.display(), self.number);
            return Ok(Saved::Overtaken);
        }
        write(&self.dir, &self.value)?;
        Ok(Saved::Written)
    }
}

#[derive(Serialize, Deserialize)]
struct AdjustmentsFile {
    schema: u32,
    /// The process version the values are read in.
    process: u32,
    exposure: f64,
    white_balance: WhiteBalanceFile,
    contrast: f64,
    highlights: f64,
    shadows: f64,
    whites: f64,
    blacks: f64,
    clarity: f64,
    texture: f64,
    sharpening: SharpeningFile,
    vibrance: f64,
    saturation: f64,
    vignette: VignetteFile,
    reframe: ReframeFile,
}

impl Versioned for AdjustmentsFile {
    const SCHEMA: u32 = ADJUSTMENTS_SCHEMA;
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WhiteBalanceFile {
    AsShot,
    Kelvin { temperature: f64, tint: f64 },
    Relative { temperature: f64, tint: f64 },
}

#[derive(Serialize, Deserialize)]
struct SharpeningFile {
    amount: f64,
    radius: f64,
    detail: f64,
    masking: f64,
}

#[derive(Serialize, Deserialize)]
struct VignetteFile {
    amount: f64,
    midpoint: f64,
}

#[derive(Serialize, Deserialize)]
struct ReframeFile {
    /// In stored pixels; none keeps the whole picture.
    crop: Option<CropFile>,
    flip: FlipFile,
    turn: TurnFile,
}

#[derive(Serialize, Deserialize)]
struct CropFile {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FlipFile {
    None,
    Horizontal,
}

/// Quarter-turns clockwise, after the flip.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TurnFile {
    None,
    Quarter,
    Half,
    ThreeQuarters,
}

impl AdjustmentsFile {
    fn of(value: &RenderAdjustments) -> Self {
        let a = &value.adjustments;
        let at = |spec: slider::SliderSpec, v: f64| spec.clamp(v);
        let white_balance = match a.white_balance {
            WhiteBalance::AsShot => WhiteBalanceFile::AsShot,
            WhiteBalance::Kelvin { temperature, tint } => WhiteBalanceFile::Kelvin {
                temperature: at(slider::TEMPERATURE_KELVIN, temperature),
                tint: at(slider::TINT_RAW, tint),
            },
            WhiteBalance::Relative { temperature, tint } => WhiteBalanceFile::Relative {
                temperature: at(slider::TEMPERATURE_RELATIVE, temperature),
                tint: at(slider::TINT_RELATIVE, tint),
            },
        };
        let Reframe { crop, flip, turn } = value.reframe;
        Self {
            schema: ADJUSTMENTS_SCHEMA,
            process: a.process.number(),
            exposure: at(slider::EXPOSURE, a.exposure),
            white_balance,
            contrast: at(slider::CONTRAST, a.contrast),
            highlights: at(slider::HIGHLIGHTS, a.highlights),
            shadows: at(slider::SHADOWS, a.shadows),
            whites: at(slider::WHITES, a.whites),
            blacks: at(slider::BLACKS, a.blacks),
            clarity: at(slider::CLARITY, a.clarity),
            texture: at(slider::TEXTURE, a.texture),
            sharpening: SharpeningFile {
                amount: at(slider::SHARPENING, a.sharpening.amount),
                radius: at(slider::SHARPEN_RADIUS, a.sharpening.radius),
                detail: at(slider::SHARPEN_DETAIL, a.sharpening.detail),
                masking: at(slider::SHARPEN_MASKING, a.sharpening.masking),
            },
            vibrance: at(slider::VIBRANCE, a.vibrance),
            saturation: at(slider::SATURATION, a.saturation),
            vignette: VignetteFile {
                amount: at(slider::VIGNETTE, a.vignette.amount),
                midpoint: at(slider::VIGNETTE_MIDPOINT, a.vignette.midpoint),
            },
            reframe: ReframeFile {
                crop: crop.map(|Rect { x, y, width, height }| CropFile { x, y, width, height }),
                flip: match flip {
                    Flip::None => FlipFile::None,
                    Flip::Horizontal => FlipFile::Horizontal,
                },
                turn: match turn {
                    Turn::None => TurnFile::None,
                    Turn::Quarter => TurnFile::Quarter,
                    Turn::Half => TurnFile::Half,
                    Turn::ThreeQuarters => TurnFile::ThreeQuarters,
                },
            },
        }
    }

    /// The value; `None` for a process version this build doesn't know.
    fn value(self) -> Option<RenderAdjustments> {
        let white_balance = match self.white_balance {
            WhiteBalanceFile::AsShot => WhiteBalance::AsShot,
            WhiteBalanceFile::Kelvin { temperature, tint } => {
                WhiteBalance::Kelvin { temperature, tint }
            }
            WhiteBalanceFile::Relative { temperature, tint } => {
                WhiteBalance::Relative { temperature, tint }
            }
        };
        let SharpeningFile { amount, radius, detail, masking } = self.sharpening;
        let adjustments = Adjustments {
            process: ProcessVersion::known(self.process)?,
            exposure: self.exposure,
            white_balance,
            contrast: self.contrast,
            highlights: self.highlights,
            shadows: self.shadows,
            whites: self.whites,
            blacks: self.blacks,
            clarity: self.clarity,
            texture: self.texture,
            sharpening: Sharpening { amount, radius, detail, masking },
            vibrance: self.vibrance,
            saturation: self.saturation,
            vignette: Vignette { amount: self.vignette.amount, midpoint: self.vignette.midpoint },
        };
        let ReframeFile { crop, flip, turn } = self.reframe;
        let reframe = Reframe {
            crop: crop.map(|CropFile { x, y, width, height }| Rect { x, y, width, height }),
            flip: match flip {
                FlipFile::None => Flip::None,
                FlipFile::Horizontal => Flip::Horizontal,
            },
            turn: match turn {
                TurnFile::None => Turn::None,
                TurnFile::Quarter => Turn::Quarter,
                TurnFile::Half => Turn::Half,
                TurnFile::ThreeQuarters => Turn::ThreeQuarters,
            },
        };
        Some(RenderAdjustments { adjustments, reframe })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PICTURE: Size = Size { width: 64, height: 48 };

    /// Every field off its default.
    fn every_slider_set() -> RenderAdjustments {
        RenderAdjustments {
            adjustments: Adjustments {
                process: ProcessVersion::V1,
                exposure: 0.75,
                white_balance: WhiteBalance::Kelvin { temperature: 4800.0, tint: 12.0 },
                contrast: 10.0,
                highlights: -40.0,
                shadows: 35.0,
                whites: 5.0,
                blacks: -8.0,
                clarity: 20.0,
                texture: 15.0,
                sharpening: Sharpening { amount: 40.0, radius: 1.5, detail: 30.0, masking: 10.0 },
                vibrance: 12.0,
                saturation: -6.0,
                vignette: Vignette { amount: -25.0, midpoint: 40.0 },
            },
            reframe: Reframe {
                crop: Some(Rect { x: 4, y: 2, width: 40, height: 30 }),
                flip: Flip::Horizontal,
                turn: Turn::Quarter,
            },
        }
    }

    #[test]
    fn no_file_is_neutral_with_no_reframe() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read(dir.path(), PICTURE), Ok(RenderAdjustments::NONE));
    }

    #[test]
    fn a_written_value_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        for value in [
            every_slider_set(),
            RenderAdjustments::NONE,
            RenderAdjustments {
                adjustments: Adjustments {
                    white_balance: WhiteBalance::Relative { temperature: -20.0, tint: 3.0 },
                    ..Adjustments::NEUTRAL
                },
                reframe: Reframe { turn: Turn::ThreeQuarters, ..Reframe::NONE },
            },
        ] {
            write(dir.path(), &value).unwrap();
            assert_eq!(read(dir.path(), PICTURE), Ok(value));
        }
    }

    #[test]
    fn every_exposure_step_is_written_as_its_decimal() {
        let dir = tempfile::tempdir().unwrap();
        let spec = slider::EXPOSURE;
        for n in 0..=1000 {
            let exposure = spec.snap(spec.min + f64::from(n) * spec.step);
            let value = RenderAdjustments {
                adjustments: Adjustments { exposure, ..Adjustments::NEUTRAL },
                ..RenderAdjustments::NONE
            };
            write(dir.path(), &value).unwrap();
            let text = std::fs::read_to_string(dir.path().join(ADJUSTMENTS_FILE)).unwrap();
            let written = format!("\"exposure\": {}", serde_json::to_string(&exposure).unwrap());
            assert!(text.contains(&written), "{exposure}: {text}");
            assert_eq!(read(dir.path(), PICTURE), Ok(value));
        }
    }

    #[test]
    fn a_value_not_a_number_is_written_as_its_default() {
        let dir = tempfile::tempdir().unwrap();
        let mut value = every_slider_set();
        value.adjustments.exposure = f64::NAN;
        value.adjustments.contrast = 1e9;
        write(dir.path(), &value).unwrap();
        let back = read(dir.path(), PICTURE).unwrap();
        assert_eq!((back.adjustments.exposure, back.adjustments.contrast), (0.0, 100.0));
    }

    #[test]
    fn a_damaged_file_reads_as_damaged_and_the_next_write_sets_it_aside() {
        let set = |json: &serde_json::Value| {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join(ADJUSTMENTS_FILE), json.to_string()).unwrap();
            dir
        };
        let good = serde_json::to_value(AdjustmentsFile::of(&every_slider_set())).unwrap();
        let edited = |edit: &dyn Fn(&mut serde_json::Value)| {
            let mut json = good.clone();
            edit(&mut json);
            json
        };
        let cases = [
            ("another schema", edited(&|j| j["schema"] = 2.into())),
            ("an unknown process", edited(&|j| j["process"] = 99.into())),
            ("no process", edited(&|j| drop(j.as_object_mut().unwrap().remove("process")))),
            ("no slider", edited(&|j| drop(j.as_object_mut().unwrap().remove("texture")))),
            ("a crop outside", edited(&|j| j["reframe"]["crop"]["width"] = 61.into())),
            ("not JSON", serde_json::Value::String("{".into())),
        ];
        for (what, json) in cases {
            let dir = set(&json);
            assert_eq!(read(dir.path(), PICTURE), Err(Unread::Damaged), "{what}");
            write(dir.path(), &RenderAdjustments::NONE).unwrap();
            assert_eq!(read(dir.path(), PICTURE), Ok(RenderAdjustments::NONE), "{what}");
            let aside = std::fs::read_dir(dir.path())
                .unwrap()
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().contains("unreadable"))
                .count();
            // A crop outside the picture is this format: the write replaces it.
            let kept = usize::from(what != "a crop outside");
            assert_eq!(aside, kept, "{what}: set aside");
        }
        // The good file reads.
        assert_eq!(read(set(&good).path(), PICTURE), Ok(every_slider_set()));
    }

    #[test]
    fn a_write_a_later_one_overtakes_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let writes = Writes::of(dir.path().to_path_buf());
        let first = writes.send(every_slider_set());
        let second = writes.clone().send(RenderAdjustments::NONE);
        assert!(first.is_overtaken() && !second.is_overtaken());
        assert_eq!(first.save().unwrap(), Saved::Overtaken);
        assert_eq!(read(dir.path(), PICTURE), Ok(RenderAdjustments::NONE), "nothing written");
        let third = writes.send(every_slider_set());
        assert_eq!(second.save().unwrap(), Saved::Overtaken);
        assert_eq!(third.save().unwrap(), Saved::Written);
        assert_eq!(read(dir.path(), PICTURE), Ok(every_slider_set()));
    }

    #[test]
    fn a_write_to_a_gone_render_fails() {
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("gone");
        assert!(Writes::of(gone).send(RenderAdjustments::NONE).save().is_err());
    }
}
