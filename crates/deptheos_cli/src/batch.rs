//! `deptheos batch <method> <folders…> -o <dir>`: one stack per folder, or
//! each folder split into stacks by capture time, as the desktop's Batch.
//! [`deptheos_app::runs::batch::Machine`] runs it, driven by `drive.rs`;
//! this stacks and exports, going on past a failed stack (exit status 2).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand, ValueEnum};
use deptheos_app::library::export::batch_target;
use deptheos_app::runs::batch::{self, Split};
use deptheos_app::{
    Adjustments, CompletedStack, CoreError, Engine, Look, SaveOptions, SessionOptions,
    StackerConfig, Words,
};

use crate::args::{
    AlignArgs, Checks, DecodeArgs, DepthArgs, Format, Order, PyramidArgs, SaveArgs, SourceArg,
    WaveletArgs,
};
use crate::drive::{Made, Runner, drive};
use crate::error::{Error, Refusal, Result, tell_engine};
use crate::stacking::{Attempt, plan_folders, stack_one};
use crate::ui::{self, progress};
use crate::{Ended, prepare};

#[derive(Subcommand)]
pub(crate) enum BatchCommand {
    /// Stack each with Pyramid
    Pyramid {
        #[command(flatten)]
        method: PyramidArgs,
        #[command(flatten)]
        batch: BatchArgs,
    },
    /// Stack each through a depth map
    Depth {
        #[command(flatten)]
        method: DepthArgs,
        #[command(flatten)]
        batch: BatchArgs,
    },
    /// Stack each with Wavelet
    Wavelet {
        #[command(flatten)]
        method: WaveletArgs,
        #[command(flatten)]
        batch: BatchArgs,
    },
}

impl BatchCommand {
    fn parts(&self) -> (&BatchArgs, StackerConfig) {
        match self {
            Self::Pyramid { batch, method } => (batch, method.config()),
            Self::Depth { batch, method } => (batch, method.config()),
            Self::Wavelet { batch, method } => (batch, method.config()),
        }
    }

    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Pyramid { .. } => "pyramid",
            Self::Depth { .. } => "depth",
            Self::Wavelet { .. } => "wavelet",
        }
    }

    pub(crate) fn check(&self, c: &mut Checks) {
        let (args, _) = self.parts();
        args.save.check(&[args.format.export()], c);
        args.align.check(c);
        args.decode.check(c);
    }
}

#[derive(Args)]
pub(crate) struct BatchArgs {
    /// Folders of frames; each is one stack, or several with --split auto
    #[arg(required = true, value_name = "FOLDERS")]
    folders: Vec<PathBuf>,

    /// The folder the stacks are written into, each named for its folder
    /// (or its frames' range); an existing file is never replaced
    #[arg(short, long, value_name = "DIR", help_heading = "Output")]
    output: PathBuf,

    /// The stacks' file format; dng needs camera RAW (other stacks are
    /// written as TIFF)
    #[arg(long, value_enum, default_value_t = Format::Tif, help_heading = "Output")]
    format: Format,

    #[command(flatten)]
    save: SaveArgs,

    /// One stack per folder, or each folder split into stacks by capture
    /// time (files it can't place are skipped)
    #[arg(long, value_enum, default_value_t, help_heading = "Input")]
    split: SplitArg,

    /// Which file of a RAW + JPEG pair to stack
    #[arg(long, value_enum, default_value_t, help_heading = "Input")]
    source: SourceArg,

    /// The order each stack's frames stack in
    #[arg(long, value_enum, default_value_t, help_heading = "Input")]
    order: Order,

    #[command(flatten)]
    align: AlignArgs,

    #[command(flatten)]
    decode: DecodeArgs,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
enum SplitArg {
    /// One stack per folder
    #[default]
    Folder,
    /// Split each folder into stacks by capture time
    Auto,
}

/// Refuses an `--output` that is a file, or is or is inside a folder of
/// `plan`'s frames (the stacks would be taken for frames next time).
fn check_output(output: &Path, plan: &batch::Plan) -> Result<()> {
    if output.is_file() {
        return Err(Refusal::OutputIsFile { path: output.to_path_buf(), holds: "stacks" }.into());
    }
    match plan.frames_folder_holding(output) {
        Some(input) => Err(Refusal::OutputInFrames { output: output.to_path_buf(), input }.into()),
        None => Ok(()),
    }
}

pub(crate) fn run(command: BatchCommand, words: &Words) -> Result<Ended> {
    let (args, config) = command.parts();
    let split = match args.split {
        SplitArg::Folder => Split::PerFolder,
        SplitArg::Auto => Split::AutoSort,
    };
    let plan =
        plan_folders(&args.folders, split, args.source, args.order, args.decode.backend(), words);
    progress!("{}", plan.summary(words));
    if plan.jobs.is_empty() {
        return Err(Refusal::NoFolderHasFrames.into());
    }
    check_output(&args.output, &plan)?;
    std::fs::create_dir_all(&args.output).map_err(CoreError::from)?;

    let engine = prepare::engine(&args.decode, words)?;
    let options = crate::args::session_options(args.order, &args.align, &args.decode);
    let save = args.save.options();

    let machine_options = batch::machine::Options { save: false, export: true, turntable: None };
    let mut machine =
        batch::Machine::new(plan.jobs.clone(), machine_options, engine.words().clone());
    let mut runner =
        BatchRunner { engine: &engine, config, options, save, args, made: HashMap::new() };
    drive(&mut machine, &mut runner)
}

/// `deptheos batch`'s side of the run: stack, then export each render.
struct BatchRunner<'a> {
    engine: &'a Engine,
    config: StackerConfig,
    options: SessionOptions,
    save: SaveOptions,
    args: &'a BatchArgs,
    /// Each render waits here only until it is exported, right after.
    made: HashMap<batch::StackNo, CompletedStack>,
}

impl Runner for BatchRunner<'_> {
    type Kept = CompletedStack;

    fn stack(
        &mut self,
        status: &str,
        _: batch::StackKind,
        label: &str,
        frames: Vec<PathBuf>,
    ) -> Result<std::result::Result<Made<CompletedStack>, String>> {
        let (engine, config, options) = (self.engine, self.config, self.options);
        let made = stack_one(engine, config, options, Attempt::First, status, label, frames)?;
        Ok(made.map(Made::of))
    }

    fn keep(&mut self, stack: batch::StackNo, _: String, kept: CompletedStack) -> Result<()> {
        self.made.insert(stack, kept);
        Ok(())
    }

    // No turntable here, so no export replaces another.
    fn export(
        &mut self,
        stack: batch::StackNo,
        label: &str,
    ) -> Result<std::result::Result<(), String>> {
        let (engine, args) = (self.engine, self.args);
        let completed = self
            .made
            .remove(&stack)
            .ok_or_else(|| Error::Fault(format!("stack {label} was exported twice")))?;
        let format = args.format.export().for_stack(completed.is_linear());
        if format != args.format.export() {
            ui::warn(format!(
                "{label}: DNG needs camera RAW frames; writing a {} instead",
                format.label()
            ));
        }
        let path = batch_target(&args.output, label, format.extension(), None, |p| p.exists());
        // An unwritten stack is warned of here; the summary and exit status
        // count it.
        let result = engine
            .save(Look::exported(&completed, Some(&Adjustments::NEUTRAL)), &path, self.save)
            .map_err(|e| {
                let told = tell_engine(&e, engine.words());
                ui::warn(batch::export_failure(engine.words(), label, &told.message));
                told.print_details();
                told.message
            });
        if result.is_ok() {
            ui::wrote(&path);
        }
        Ok(result)
    }

    fn finish(&mut self, summary: &batch::Summary) -> Result<Ended> {
        // The time taken follows on its own line ("total 12.3 s").
        progress!("{}", summary.text(self.engine.words(), Some(&self.args.output)));
        Ok(Ended::of(summary))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `--output` inside a folder of the frames, or that folder, is
    /// refused; one beside them is not.
    #[test]
    fn the_output_is_outside_the_frames() {
        let dir = tempfile::tempdir().unwrap();
        let bee = dir.path().join("bee");
        std::fs::create_dir_all(&bee).unwrap();
        for name in ["1.png", "2.png"] {
            std::fs::write(bee.join(name), b"").unwrap();
        }
        let words = Words::english(deptheos_app::Platform::CommandLine);
        let plan = plan_folders(
            std::slice::from_ref(&bee),
            Split::PerFolder,
            SourceArg::default(),
            Order::default(),
            deptheos_app::RawBackend::LibRaw,
            &words,
        );
        assert_eq!(plan.jobs.len(), 1);
        let refused = |output: &Path| {
            matches!(
                check_output(output, &plan),
                Err(Error::Refused(Refusal::OutputInFrames { .. }))
            )
        };
        assert!(refused(&bee));
        assert!(refused(&bee.join("out")));
        assert!(!refused(&dir.path().join("out")));
        assert!(check_output(&dir.path().join("out"), &plan).is_ok());
    }
}
