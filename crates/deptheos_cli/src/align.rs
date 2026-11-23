//! `deptheos align <frames…> -o <dir>`: the frames aligned as a stack would
//! align them, each written into a folder ([`deptheos_app::runs::aligned`]):
//! an ordinary stack aligns, its result thrown away, then each frame is
//! read again, warped and saved.

use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::Args;
use deptheos_app::{CoreError, SessionEvent, Words};

use crate::args::{
    AlignArgs, Aligner, Checks, DecodeArgs, Format, FramesArgs, SaveArgs, session_options,
};
use crate::error::{Refusal, Result};
use crate::outputs::same_file;
use crate::prepare;
use crate::ui::{self, progress, secs};

#[derive(Args)]
pub(crate) struct AlignFramesArgs {
    #[command(flatten)]
    frames: FramesArgs,

    /// The folder the aligned frames are written into, each named for its
    /// frame (IMG_0001.CR3 is written as IMG_0001.tif), made if need be. A
    /// file already there is never replaced: the command stops before any
    /// work
    #[arg(short, long, value_name = "DIR", help_heading = "Output")]
    output: PathBuf,

    /// The aligned frames' file format; dng needs camera RAW frames (a
    /// linear DNG with the reference frame's color science, as a stack's)
    #[arg(long, value_enum, default_value_t = Format::Tif, help_heading = "Output")]
    format: Format,

    #[command(flatten)]
    save: SaveArgs,

    #[command(flatten)]
    align: AlignArgs,

    #[command(flatten)]
    decode: DecodeArgs,
}

impl AlignFramesArgs {
    pub(crate) fn check(&self, c: &mut Checks) {
        self.save.check(&[self.format.export()], c);
        if self.align.align == Aligner::None {
            c.refuse(
                "align needs an aligner: with --align none every frame would be written unchanged",
            );
        } else {
            self.align.check(c);
        }
        self.decode.check(c);
    }
}

pub(crate) fn run(args: AlignFramesArgs, words: &Words) -> Result<()> {
    let frames = prepare::ordered_frames(&args.frames.inputs, args.frames.order.into(), words)?;
    let outputs = outputs(&frames, &args.output, args.format)?;
    std::fs::create_dir_all(&args.output).map_err(CoreError::from)?;

    let engine = prepare::engine(&args.decode, words)?;
    let options = session_options(args.frames.order, &args.align, &args.decode);
    let save = args.save.options();
    // Each frame is read twice, to align it and to write it: convert once.
    let _converted = prepare::convert_first(&engine, &frames, options.raw_backend)?;

    let mut last = Instant::now();
    let mut print = |event: SessionEvent, _: &deptheos_app::LiveMap| {
        // The session logs the reverse check; the stack's own steps aren't
        // wanted here.
        if let SessionEvent::Accumulated { current, total } = event {
            progress!("aligning {current} of {total} ({})", secs(last.elapsed()));
            last = Instant::now();
        }
    };
    let aligned = engine.align(frames, options, &mut print, crate::interrupt::token())?;
    if let Some(left_out) = aligned.left_out_message(words) {
        ui::warn(left_out);
    }
    for note in aligned.notes() {
        ui::warn(note);
    }
    let (width, height) = aligned.size();
    if let Some(crop) = aligned.geometry().crop {
        progress!(
            "cropped to the area every frame covers: {} × {} at x {}, y {}",
            crop.width,
            crop.height,
            crop.x,
            crop.y
        );
    }

    let indexes: Vec<usize> = aligned.aligned().collect();
    let total = indexes.len();
    for (n, index) in indexes.into_iter().enumerate() {
        if crate::interrupt::token().is_canceled() {
            return Err(deptheos_app::Error::Canceled.into());
        }
        let t = Instant::now();
        let path = &outputs[index];
        engine.save_aligned(&aligned, index, path, save)?;
        progress!("writing {} of {total} ({})", n + 1, secs(t.elapsed()));
        ui::wrote(path);
    }
    match total {
        1 => progress!("wrote 1 aligned frame, {width} × {height}"),
        n => progress!("wrote {n} aligned frames, each {width} × {height}"),
    }
    Ok(())
}

/// Where each of `frames` is written: `dir/<stem>.<format>`. Refuses a
/// `dir` that is or looks like a file, DNG without camera RAW, two frames of
/// one name, and a file already there.
fn outputs(frames: &[PathBuf], dir: &Path, format: Format) -> Result<Vec<PathBuf>> {
    let named_like_a_file = deptheos_app::is_dng(dir) || image::ImageFormat::from_path(dir).is_ok();
    if dir.is_file() {
        return Err(
            Refusal::OutputIsFile { path: dir.to_path_buf(), holds: "aligned frames" }.into()
        );
    }
    if !dir.exists() && named_like_a_file {
        return Err(Refusal::FolderNamedLikeFile(dir.to_path_buf()).into());
    }
    let outputs: Vec<PathBuf> = frames
        .iter()
        .map(|frame| {
            let stem = frame.file_stem().map(|s| s.to_string_lossy()).unwrap_or_default();
            dir.join(format!("{stem}.{}", format.extension()))
        })
        .collect();
    if format == Format::Dng
        && let Some(first) = frames
            .first()
            .filter(|f| !deptheos_app::is_raw_source(f, deptheos_app::CameraRaw::LibRaw))
    {
        return Err(
            Refusal::DngNeedsRaw { output: outputs[0].clone(), first: first.clone() }.into()
        );
    }
    // File systems that ignore case would write two such names to one file.
    let mut seen: Vec<(String, &Path)> = Vec::new();
    for (frame, output) in frames.iter().zip(&outputs) {
        let key = output.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
        if let Some((_, first)) = seen.iter().find(|(k, _)| *k == key) {
            return Err(Refusal::SameName {
                first: first.to_path_buf(),
                second: frame.clone(),
                output: output.clone(),
            }
            .into());
        }
        seen.push((key, frame));
    }
    let taken: Vec<&PathBuf> = outputs.iter().filter(|o| o.exists()).collect();
    if let Some(&first) = taken.first() {
        if let Some(input) = frames.iter().find(|f| same_file(first, f)) {
            return Err(
                Refusal::OutputIsInput { output: first.clone(), input: input.clone() }.into()
            );
        }
        return Err(Refusal::OutputExists { path: first.clone(), more: taken.len() - 1 }.into());
    }
    Ok(outputs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path) -> PathBuf {
        std::fs::write(path, b"frame").unwrap();
        path.to_path_buf()
    }

    #[test]
    fn each_frame_is_written_under_its_own_name() {
        let dir = tempfile::tempdir().unwrap();
        let frames = vec![dir.path().join("IMG_1.CR3"), dir.path().join("IMG_2.CR3")];
        let out = dir.path().join("aligned");
        let tiffs = outputs(&frames, &out, Format::Tif).unwrap();
        assert_eq!(tiffs, [out.join("IMG_1.tif"), out.join("IMG_2.tif")]);
        assert_eq!(outputs(&frames, &out, Format::Dng).unwrap()[1], out.join("IMG_2.dng"));
        let jpegs = [dir.path().join("a.jpg")];
        assert!(matches!(
            outputs(&jpegs, &out, Format::Dng),
            Err(crate::error::Error::Refused(Refusal::DngNeedsRaw { .. }))
        ));
    }

    #[test]
    fn nothing_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let frames = vec![touch(&dir.path().join("a.tif")), touch(&dir.path().join("b.tif"))];
        // Into the frames' own folder, as TIFF: the frames themselves.
        let refusal = outputs(&frames, dir.path(), Format::Tif).unwrap_err();
        assert!(matches!(refusal, crate::error::Error::Refused(Refusal::OutputIsInput { .. })));
        // As PNG beside them, until one is there.
        assert!(outputs(&frames, dir.path(), Format::Png).is_ok());
        touch(&dir.path().join("b.png"));
        let refusal = outputs(&frames, dir.path(), Format::Png).unwrap_err();
        assert!(matches!(
            refusal,
            crate::error::Error::Refused(Refusal::OutputExists { more: 0, .. })
        ));
    }

    #[test]
    fn two_frames_of_one_name_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let frames = [dir.path().join("x/IMG_1.jpg"), dir.path().join("y/img_1.jpg")];
        let refusal = outputs(&frames, &dir.path().join("out"), Format::Tif).unwrap_err();
        assert!(matches!(refusal, crate::error::Error::Refused(Refusal::SameName { .. })));
    }

    #[test]
    fn the_output_is_a_folder() {
        let dir = tempfile::tempdir().unwrap();
        let frames = [dir.path().join("a.jpg")];
        assert!(outputs(&frames, &dir.path().join("aligned.tif"), Format::Tif).is_err());
        let file = touch(&dir.path().join("out"));
        assert!(outputs(&frames, &file, Format::Tif).is_err());
        // An existing folder may have any name.
        let odd = dir.path().join("v1.png");
        std::fs::create_dir(&odd).unwrap();
        assert!(outputs(&frames, &odd, Format::Tif).is_ok());
    }
}
