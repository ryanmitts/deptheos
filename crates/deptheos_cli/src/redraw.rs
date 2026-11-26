//! `deptheos redraw <render folder> -o <file>`: a saved render drawn again
//! through the adjusted path, with its paint, for tuning by eye. It reads
//! the one folder ([`deptheos_app::library::render_folder`]), never the
//! Library around it, so it runs while the app has the Library open.

use std::path::{Path, PathBuf};

use clap::{Args, ValueEnum};
use deptheos_app::library::export::ExportFormat;
use deptheos_app::library::render_folder::RenderFolder;
use deptheos_app::{Adjustments, Engine, Look, OutputSpace, Words};

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

impl RedrawArgs {
    pub(crate) fn check(&self, c: &mut Checks) {
        self.save.check(ExportFormat::of(&self.output).as_slice(), c);
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
    let adjustments = Adjustments::NEUTRAL;
    let look = Look::redrawn(&folder.stack, &adjustments, output);
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
}
