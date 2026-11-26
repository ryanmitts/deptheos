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
use deptheos_app::{Adjustments, Engine, Look, OutputSpace, ProcessVersion, Words};

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
    /// camera RAW]
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

/// A process version this build knows.
fn parse_process(s: &str) -> std::result::Result<ProcessVersion, String> {
    let latest = ProcessVersion::LATEST.number();
    let known = s.trim().parse().ok().and_then(ProcessVersion::known);
    known.ok_or_else(|| format!("this build knows processes 1 to {latest}"))
}

impl RedrawArgs {
    /// The adjustments the flags give, the rest neutral.
    fn adjustments(&self) -> Adjustments {
        let neutral = Adjustments::NEUTRAL;
        let adjustments =
            Adjustments { exposure: self.exposure.unwrap_or(neutral.exposure), ..neutral };
        adjustments.in_process(self.process.unwrap_or(ProcessVersion::LATEST))
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
        if self.process.is_some() && self.adjustments().is_neutral() {
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
    let output = match (args.space, folder.stack.is_linear()) {
        (Some(Space::Srgb), _) | (None, true) => OutputSpace::Srgb,
        (Some(Space::Own), _) | (None, false) => OutputSpace::StackProfile,
    };
    let reframe = args.reframe();
    let picture = Size { width: folder.stack.width, height: folder.stack.height };
    if let Err(outside) = reframe.window(picture) {
        return Err(Refusal::CropOutside(outside).into());
    }
    let adjustments = args.adjustments();
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
                    Ok((args.adjustments(), checks.is_empty()))
                }
                Ok(_) => panic!("not redraw"),
                Err(e) => Err(e.kind()),
            }
        };
        assert_eq!(parsed(&[]), Ok((Adjustments::NEUTRAL, true)));
        let darker = Adjustments { exposure: -1.5, ..Adjustments::NEUTRAL };
        assert_eq!(parsed(&["--exposure", "-1.5", "--process", "1"]), Ok((darker, true)));
        assert_eq!(parsed(&["--process", "1"]).map(|(_, passes)| passes), Ok(false));
        for bad in [&["--exposure", "6"][..], &["--exposure", "nan"], &["--process", "0"]] {
            assert!(parsed(bad).is_err(), "{bad:?}");
        }
    }
}
