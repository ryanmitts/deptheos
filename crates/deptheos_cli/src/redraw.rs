//! `deptheos redraw <render folder> -o <file>`: a saved render drawn again
//! through the adjusted path, with its paint, for tuning by eye. It reads
//! the one folder ([`deptheos_app::library::render_folder`]), never the
//! Library around it, so it runs while the app has the Library open.

use std::path::{Path, PathBuf};

use clap::{Args, ValueEnum};
use deptheos_app::library::export::ExportFormat;
use deptheos_app::library::render_folder::RenderFolder;
use deptheos_app::reframe::{Flip, Rect, Reframe, Size, Turn};
use deptheos_app::slider::{self, SliderSpec};
use deptheos_app::{
    Adjustments, Engine, Look, OutputSpace, ProcessVersion, Temperature, WhiteBalance, Words,
};

use crate::args::{Checks, SaveArgs};
use crate::error::{Refusal, Result};
use crate::ui::{self, progress, secs};

#[derive(Args)]
pub(crate) struct RedrawArgs {
    /// A render's folder in the Library (Renders/<id>), read only
    #[arg(value_name = "RENDER")]
    render: PathBuf,

    /// The file to write: .tif, .png or .jpg. It can't be in the render's
    /// folder
    #[arg(short, long, value_name = "FILE", help_heading = "Output")]
    output: PathBuf,

    /// The color space written [default: own for standard images, srgb for
    /// camera RAW]. display-p3, adobe-rgb and prophoto are for camera RAW;
    /// prophoto writes 16 bits only
    #[arg(long, value_enum, help_heading = "Output")]
    space: Option<Space>,

    /// Keep this rectangle of the stored picture, in its pixels: left, top,
    /// width, height. Adjust's Crop, not stacking's --no-crop
    #[arg(long, value_name = "X,Y,W,H", value_parser = parse_rect, help_heading = "Reframe")]
    reframe: Option<Rect>,

    /// Turn the picture clockwise, by degrees, after any flip
    #[arg(long, value_enum, help_heading = "Reframe")]
    turn: Option<TurnArg>,

    /// Mirror the picture
    #[arg(long, value_enum, help_heading = "Reframe")]
    flip: Option<FlipArg>,

    /// Exposure, in stops, -5 to 5
    #[arg(long, value_name = "STOPS", allow_hyphen_values = true,
          value_parser = |s: &str| parse_slider(s, &slider::EXPOSURE), help_heading = "Adjust")]
    exposure: Option<f64>,

    /// White balance: kelvins on a camera RAW render (2000 to 50000), -100
    /// to 100 on a standard one [default: as shot]
    #[arg(long, value_name = "T", allow_hyphen_values = true, value_parser = parse_number,
          help_heading = "Adjust")]
    temperature: Option<f64>,

    /// Tint, positive toward magenta: -150 to 150 on a camera RAW render,
    /// -100 to 100 on a standard one [default: as shot]
    #[arg(long, value_name = "T", allow_hyphen_values = true, value_parser = parse_number,
          help_heading = "Adjust")]
    tint: Option<f64>,

    /// Contrast, -100 to 100
    #[arg(long, value_name = "N", allow_hyphen_values = true,
          value_parser = |s: &str| parse_slider(s, &slider::CONTRAST), help_heading = "Adjust")]
    contrast: Option<f64>,

    /// Highlights, -100 to 100
    #[arg(long, value_name = "N", allow_hyphen_values = true,
          value_parser = |s: &str| parse_slider(s, &slider::HIGHLIGHTS), help_heading = "Adjust")]
    highlights: Option<f64>,

    /// Shadows, -100 to 100
    #[arg(long, value_name = "N", allow_hyphen_values = true,
          value_parser = |s: &str| parse_slider(s, &slider::SHADOWS), help_heading = "Adjust")]
    shadows: Option<f64>,

    /// Whites, -100 to 100
    #[arg(long, value_name = "N", allow_hyphen_values = true,
          value_parser = |s: &str| parse_slider(s, &slider::WHITES), help_heading = "Adjust")]
    whites: Option<f64>,

    /// Blacks, -100 to 100
    #[arg(long, value_name = "N", allow_hyphen_values = true,
          value_parser = |s: &str| parse_slider(s, &slider::BLACKS), help_heading = "Adjust")]
    blacks: Option<f64>,

    /// Vibrance, -100 to 100
    #[arg(long, value_name = "N", allow_hyphen_values = true,
          value_parser = |s: &str| parse_slider(s, &slider::VIBRANCE), help_heading = "Adjust")]
    vibrance: Option<f64>,

    /// Saturation, -100 to 100
    #[arg(long, value_name = "N", allow_hyphen_values = true,
          value_parser = |s: &str| parse_slider(s, &slider::SATURATION), help_heading = "Adjust")]
    saturation: Option<f64>,

    /// The process version the adjustments are read in [default: the
    /// latest]
    #[arg(long, value_name = "N", value_parser = parse_process, help_heading = "Adjust")]
    process: Option<ProcessVersion>,

    #[command(flatten)]
    save: SaveArgs,
}

/// The space a redraw is written in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum Space {
    Srgb,
    /// The stack's own profile (standard images only; a table-based one
    /// writes sRGB)
    Own,
    #[value(name = "display-p3")]
    DisplayP3,
    #[value(name = "adobe-rgb")]
    AdobeRgb,
    Prophoto,
}

impl Space {
    /// A camera RAW render's space other than sRGB.
    fn wide(self) -> Option<(OutputSpace, &'static str)> {
        match self {
            Self::Srgb | Self::Own => None,
            Self::DisplayP3 => Some((OutputSpace::DisplayP3, "display-p3")),
            Self::AdobeRgb => Some((OutputSpace::AdobeRgb, "adobe-rgb")),
            Self::Prophoto => Some((OutputSpace::ProPhoto, "prophoto")),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum TurnArg {
    #[value(name = "90")]
    Quarter,
    #[value(name = "180")]
    Half,
    #[value(name = "270")]
    ThreeQuarters,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum FlipArg {
    /// Left to right
    Horizontal,
    /// Top to bottom
    Vertical,
}

/// `X,Y,W,H` in pixels, the width and height above zero.
fn parse_rect(s: &str) -> std::result::Result<Rect, String> {
    let parts: Vec<&str> = s.split(',').map(str::trim).collect();
    let numbers: Option<Vec<u32>> = parts.iter().map(|p| p.parse().ok()).collect();
    match numbers.as_deref() {
        Some(&[x, y, width, height]) if width > 0 && height > 0 => Ok(Rect { x, y, width, height }),
        _ => Err("expected four whole numbers, left,top,width,height, the last two above 0".into()),
    }
}

/// A slider's value, within its range.
fn parse_slider(s: &str, spec: &SliderSpec) -> std::result::Result<f64, String> {
    match s.trim().parse::<f64>() {
        Ok(v) if v.is_finite() && (spec.min..=spec.max).contains(&v) => Ok(v),
        _ => Err(format!("expected a number from {} to {}", spec.min, spec.max)),
    }
}

/// A finite number, its range checked against the render.
fn parse_number(s: &str) -> std::result::Result<f64, String> {
    s.trim().parse::<f64>().ok().filter(|v| v.is_finite()).ok_or_else(|| "expected a number".into())
}

/// The white-balance scale a render takes.
#[derive(Clone, Copy, Debug)]
pub(crate) enum WhiteScale {
    /// A camera RAW render's kelvins, from its as-shot white (`None`: a
    /// monochrome render, which has none to set).
    Kelvin(Option<Temperature>),
    /// A standard render's relative scale.
    Relative,
}

/// A process version this build knows.
fn parse_process(s: &str) -> std::result::Result<ProcessVersion, String> {
    let latest = ProcessVersion::LATEST.number();
    let known = s.trim().parse().ok().and_then(ProcessVersion::known);
    known.ok_or_else(|| format!("this build knows processes 1 to {latest}"))
}

impl RedrawArgs {
    /// The adjustments the flags give on a render of `scale`, the rest
    /// neutral.
    fn adjustments(&self, scale: WhiteScale) -> std::result::Result<Adjustments, Refusal> {
        let neutral = Adjustments::NEUTRAL;
        let adjustments = Adjustments {
            exposure: self.exposure.unwrap_or(neutral.exposure),
            white_balance: self.white_balance(scale)?,
            contrast: self.contrast.unwrap_or(neutral.contrast),
            highlights: self.highlights.unwrap_or(neutral.highlights),
            shadows: self.shadows.unwrap_or(neutral.shadows),
            whites: self.whites.unwrap_or(neutral.whites),
            blacks: self.blacks.unwrap_or(neutral.blacks),
            vibrance: self.vibrance.unwrap_or(neutral.vibrance),
            saturation: self.saturation.unwrap_or(neutral.saturation),
            ..neutral
        };
        Ok(adjustments.in_process(self.process.unwrap_or(ProcessVersion::LATEST)))
    }

    /// The white balance `--temperature` and `--tint` give on a render of
    /// `scale`, the one not given as shot.
    fn white_balance(&self, scale: WhiteScale) -> std::result::Result<WhiteBalance, Refusal> {
        if self.temperature.is_none() && self.tint.is_none() {
            return Ok(WhiteBalance::AsShot);
        }
        let within = |flag: &'static str, spec: &SliderSpec, v: f64| {
            let (min, max) = (spec.min, spec.max);
            match (min..=max).contains(&v) {
                true => Ok(v),
                false => Err(Refusal::SliderRange { flag, min, max }),
            }
        };
        let (temperature_spec, tint_spec, (temperature, tint)) = match scale {
            WhiteScale::Kelvin(None) => {
                return Err(Refusal::NoColor(self.color_flag().unwrap_or("--temperature")));
            }
            WhiteScale::Kelvin(Some(shot)) => {
                (&slider::TEMPERATURE_KELVIN, &slider::TINT_RAW, (shot.kelvin, shot.tint))
            }
            WhiteScale::Relative => {
                (&slider::TEMPERATURE_RELATIVE, &slider::TINT_RELATIVE, (0.0, 0.0))
            }
        };
        let temperature = self.temperature.unwrap_or(temperature);
        let tint = self.tint.unwrap_or(tint);
        let temperature = within("--temperature", temperature_spec, temperature)?;
        let tint = within("--tint", tint_spec, tint)?;
        Ok(match scale {
            WhiteScale::Kelvin(_) => WhiteBalance::Kelvin { temperature, tint },
            WhiteScale::Relative => WhiteBalance::Relative { temperature, tint },
        })
    }

    /// The first color slider's flag given, which a render with no color
    /// refuses.
    fn color_flag(&self) -> Option<&'static str> {
        let given = [
            ("--temperature", self.temperature),
            ("--tint", self.tint),
            ("--vibrance", self.vibrance),
            ("--saturation", self.saturation),
        ];
        given.into_iter().find_map(|(flag, v)| v.is_some().then_some(flag))
    }

    /// Whether a slider's flag is given off its default.
    fn sets_a_slider(&self) -> bool {
        let off = |v: Option<f64>| v.is_some_and(|v| v != 0.0);
        off(self.exposure)
            || off(self.contrast)
            || off(self.highlights)
            || off(self.shadows)
            || off(self.whites)
            || off(self.blacks)
            || off(self.vibrance)
            || off(self.saturation)
            || self.temperature.is_some()
            || self.tint.is_some()
    }

    /// The reframe the flags give.
    fn reframe(&self) -> Reframe {
        let turn = match self.turn {
            None => Turn::None,
            Some(TurnArg::Quarter) => Turn::Quarter,
            Some(TurnArg::Half) => Turn::Half,
            Some(TurnArg::ThreeQuarters) => Turn::ThreeQuarters,
        };
        // Top to bottom is left to right and a half turn.
        let (flip, turn) = match self.flip {
            None => (Flip::None, turn),
            Some(FlipArg::Horizontal) => (Flip::Horizontal, turn),
            Some(FlipArg::Vertical) => (Flip::Horizontal, Turn::Half.then(turn)),
        };
        Reframe { crop: self.reframe, flip, turn }
    }

    pub(crate) fn check(&self, c: &mut Checks) {
        self.save.check(ExportFormat::of(&self.output).as_slice(), c);
        let eight = ExportFormat::of(&self.output) == Some(ExportFormat::Jpeg)
            || self.save.bit_depth == Some(crate::args::BitDepth::Eight);
        if self.space == Some(Space::Prophoto) && eight {
            c.refuse("--space prophoto writes 16 bits: a TIFF or PNG, not --bit-depth 8");
        }
        if self.process.is_some() && !self.sets_a_slider() {
            c.refuse("--process needs an adjustment to read");
        }
    }
}

pub(crate) fn run(args: RedrawArgs, words: &Words) -> Result<()> {
    check_paths(&args.render, &args.output)?;
    // Before the read, which decodes on the engine's threads.
    let engine = crate::prepare::plain_engine(words)?;
    let started = std::time::Instant::now();
    let folder = RenderFolder::read(&args.render)?;
    progress!(
        "read the render, {} × {}{} ({})",
        folder.stack.width,
        folder.stack.height,
        if folder.painted.is_some() { ", retouched" } else { "" },
        secs(started.elapsed())
    );
    let output = match (args.space.map(|s| (s, s.wide())), folder.stack.is_linear()) {
        (Some((_, Some((wide, _)))), true) => wide,
        (Some((_, Some((_, name)))), false) => return Err(Refusal::WideOnStandard(name).into()),
        (Some((Space::Srgb, _)), _) | (None, true) => OutputSpace::Srgb,
        (Some(_), _) | (None, false) => OutputSpace::StackProfile,
    };
    let reframe = args.reframe();
    let picture = Size { width: folder.stack.width, height: folder.stack.height };
    if let Err(outside) = reframe.window(picture) {
        return Err(Refusal::CropOutside(outside).into());
    }
    if !folder.stack.has_color()
        && let Some(flag) = args.color_flag()
    {
        return Err(Refusal::NoColor(flag).into());
    }
    let scale = match folder.stack.is_linear() {
        true => WhiteScale::Kelvin(folder.stack.as_shot()),
        false => WhiteScale::Relative,
    };
    if let WhiteScale::Kelvin(Some(shot)) = scale {
        progress!("as shot: {:.0} K, tint {:+.0}", shot.kelvin, shot.tint);
    }
    let adjustments = args.adjustments(scale)?;
    let look = Look::redrawn(&folder.stack, &adjustments, reframe, output);
    let drawn = std::time::Instant::now();
    Engine::save_pixels(&engine, look, folder.pixels(), &args.output, args.save.options())?;
    progress!("drew and wrote it ({})", secs(drawn.elapsed()));
    ui::wrote(&args.output);
    Ok(())
}

/// Refuses a `render` that isn't a render's folder, and an `output` that
/// isn't a TIFF, PNG or JPEG, whose folder is missing, or that is inside
/// the render's folder.
fn check_paths(render: &Path, output: &Path) -> Result<()> {
    if !RenderFolder::is_render(render) {
        return Err(Refusal::NotARender(render.to_path_buf()).into());
    }
    if !matches!(
        ExportFormat::of(output),
        Some(ExportFormat::Tiff | ExportFormat::Png | ExportFormat::Jpeg)
    ) {
        let formats = ".tif, .png or .jpg";
        return Err(Refusal::FileType { path: output.to_path_buf(), formats }.into());
    }
    let parent = output.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    if !parent.is_dir() {
        let (path, folder) = (output.to_path_buf(), parent.to_path_buf());
        return Err(Refusal::NoFolder { path, folder }.into());
    }
    let inside = match (parent.canonicalize(), render.canonicalize()) {
        (Ok(parent), Ok(render)) => parent.starts_with(render),
        _ => false,
    };
    if inside {
        let (output, render) = (output.to_path_buf(), render.to_path_buf());
        return Err(Refusal::OutputInRender { output, render }.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_what_it_would_not_write() {
        let dir = tempfile::tempdir().unwrap();
        let render = dir.path().join("render");
        std::fs::create_dir(&render).unwrap();
        let refused = |output: &Path| match check_paths(&render, output) {
            Err(crate::error::Error::Refused(refusal)) => refusal.to_string(),
            other => panic!("{output:?}: {other:?}"),
        };
        assert!(refused(&dir.path().join("a.tif")).contains("isn't a render"));

        std::fs::write(render.join("render.json"), "{}").unwrap();
        assert!(check_paths(&render, &dir.path().join("a.tif")).is_ok());
        assert!(refused(&dir.path().join("a.dng")).contains("use .tif, .png or .jpg"));
        assert!(refused(&dir.path().join("none/a.png")).contains("doesn't exist"));
        assert!(refused(&render.join("a.jpg")).contains("inside the render"));
    }

    #[test]
    fn the_flags_make_one_reframe() {
        use clap::Parser;
        let reframe = |flags: &[&str]| {
            let line = [&["deptheos", "redraw", "r", "-o", "a.tif"], flags].concat();
            match crate::args::Cli::try_parse_from(line).map(|cli| cli.command) {
                Ok(crate::args::Command::Redraw(args)) => Ok(args.reframe()),
                Ok(_) => panic!("not redraw"),
                Err(e) => Err(e.kind()),
            }
        };
        assert_eq!(reframe(&[]), Ok(Reframe::NONE));
        let crop = Rect { x: 10, y: 20, width: 300, height: 200 };
        assert_eq!(
            reframe(&["--reframe", "10,20,300,200", "--turn", "90"]),
            Ok(Reframe { crop: Some(crop), flip: Flip::None, turn: Turn::Quarter })
        );
        let vertical = Reframe { crop: None, flip: Flip::Horizontal, turn: Turn::ThreeQuarters };
        assert_eq!(reframe(&["--flip", "vertical", "--turn", "90"]), Ok(vertical));
        for bad in ["1,2,3", "1,2,0,4", "a,b,c,d", "-1,0,4,4"] {
            assert!(reframe(&["--reframe", bad]).is_err(), "{bad}");
        }
        assert!(reframe(&["--turn", "45"]).is_err());
    }

    #[test]
    fn the_flags_make_the_adjustments() {
        use clap::Parser;
        let parsed = |flags: &[&str]| {
            let line = [&["deptheos", "redraw", "r", "-o", "a.tif"], flags].concat();
            match crate::args::Cli::try_parse_from(line).map(|cli| cli.command) {
                Ok(crate::args::Command::Redraw(args)) => {
                    let mut checks = Checks::default();
                    args.check(&mut checks);
                    Ok((args.adjustments(WhiteScale::Relative).unwrap(), checks.is_empty()))
                }
                Ok(_) => panic!("not redraw"),
                Err(e) => Err(e.kind()),
            }
        };
        assert_eq!(parsed(&[]), Ok((Adjustments::NEUTRAL, true)));
        let darker = Adjustments { exposure: -1.5, ..Adjustments::NEUTRAL };
        assert_eq!(parsed(&["--exposure", "-1.5", "--process", "1"]), Ok((darker, true)));
        let curve =
            Adjustments { contrast: 20.0, whites: -5.0, blacks: 10.0, ..Adjustments::NEUTRAL };
        let flags = ["--contrast", "20", "--whites", "-5", "--blacks", "10"];
        assert_eq!(parsed(&flags), Ok((curve, true)));
        let tone = Adjustments { highlights: -40.0, shadows: 35.0, ..Adjustments::NEUTRAL };
        assert_eq!(parsed(&["--highlights", "-40", "--shadows", "35"]), Ok((tone, true)));
        assert_eq!(parsed(&["--process", "1"]).map(|(_, passes)| passes), Ok(false));
        let bad: [&[&str]; 5] = [
            &["--exposure", "6"],
            &["--exposure", "nan"],
            &["--process", "0"],
            &["--blacks", "101"],
            &["--shadows", "-101"],
        ];
        for bad in bad {
            assert!(parsed(bad).is_err(), "{bad:?}");
        }
    }

    /// ProPhoto writes 16 bits only.
    #[test]
    fn prophoto_needs_sixteen_bits() {
        use clap::Parser;
        let passes = |output: &str, flags: &[&str]| {
            let line = [&["deptheos", "redraw", "r", "-o", output, "--space", "prophoto"], flags];
            let Ok(crate::args::Command::Redraw(args)) =
                crate::args::Cli::try_parse_from(line.concat()).map(|cli| cli.command)
            else {
                panic!("not redraw")
            };
            let mut checks = Checks::default();
            args.check(&mut checks);
            checks.is_empty()
        };
        assert!(passes("a.tif", &[]) && passes("a.png", &["--bit-depth", "16"]));
        assert!(!passes("a.jpg", &[]) && !passes("a.tif", &["--bit-depth", "8"]));
    }

    #[test]
    fn white_balance_takes_the_renders_scale() {
        use clap::Parser;
        let white = |flags: &[&str], scale| {
            let line = [&["deptheos", "redraw", "r", "-o", "a.tif"], flags].concat();
            match crate::args::Cli::try_parse_from(line).unwrap().command {
                crate::args::Command::Redraw(args) => {
                    args.white_balance(scale).map_err(|e| e.to_string())
                }
                _ => panic!("not redraw"),
            }
        };
        let shot = WhiteScale::Kelvin(Some(Temperature { kelvin: 5200.0, tint: 7.0 }));
        assert_eq!(white(&[], shot), Ok(WhiteBalance::AsShot));
        let warmer = WhiteBalance::Kelvin { temperature: 6000.0, tint: 7.0 };
        assert_eq!(white(&["--temperature", "6000"], shot), Ok(warmer));
        let greener = WhiteBalance::Relative { temperature: 0.0, tint: -20.0 };
        assert_eq!(white(&["--tint", "-20"], WhiteScale::Relative), Ok(greener));
        assert!(white(&["--temperature", "6000"], WhiteScale::Relative).is_err());
        assert!(white(&["--temperature", "-20"], shot).is_err());
        assert!(white(&["--tint", "1"], WhiteScale::Kelvin(None)).is_err());
    }
}
