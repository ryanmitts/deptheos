use clap::Parser;
use deptheos_app::{
    Adjustments, CompletedStack, CoreError, Engine, Look, Overlay, Platform, SessionEvent,
    SessionOutput, StackGeometry, StackerConfig, StackingSession, Words,
};
use error::{Error, Result};
use std::path::{Path, PathBuf};
use std::time::Instant;

mod align;
mod args;
mod batch;
mod bench;
mod drive;
mod error;
mod interrupt;
mod outputs;
mod prepare;
mod sequence;
mod stacking;
mod ui;

use args::{Cli, Command, StackArgs};
use outputs::{check_outputs, check_sweep_dir};
use ui::{progress, secs};

// mimalloc: see the workspace manifest for why.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    // wild expands globs on Windows, where the shell leaves them to us.
    let cli = Cli::parse_from(wild::args_os());
    cli.check();
    install_logs(&cli.log);
    // The engine's sentences in the system's language; the CLI's own stay
    // English (ADR-161).
    let words = Words::system(Platform::CommandLine);

    let ended = run(cli.command, &words);
    prepare::keep_engine_until_exit();
    match ended {
        Ok(Ended::Clean) => {}
        // The run went on without what failed, and said so in its summary.
        Ok(Ended::SomeFailed) => std::process::exit(2),
        // Ctrl-C: what was written is kept; 130 is the shell's code for it.
        Err(Error::Engine(deptheos_app::Error::Canceled)) => {
            eprintln!(
                "interrupted: what was written so far is kept (a sequence's manifest is marked incomplete)"
            );
            std::process::exit(130);
        }
        Err(e) => {
            e.tell(&words).print(None);
            std::process::exit(1);
        }
    }
}

/// Logs on stderr, so stdout is the results alone. `RUST_LOG` wins over
/// `--quiet` and `--verbose`.
fn install_logs(log: &args::LogArgs) {
    ui::set_quiet(log.quiet);
    let filter = match (log.quiet, log.verbose) {
        (true, _) => "warn,wgpu_core=error,wgpu_hal=error,naga=error",
        (_, true) => "info,deptheos=debug,wgpu_core=warn,wgpu_hal=warn,naga=warn",
        _ => "info,wgpu_core=warn,wgpu_hal=warn,naga=warn",
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| filter.into()),
        )
        .with_writer(std::io::stderr)
        .init();
    deptheos_app::route_logs_to_tracing();
}

/// How a run that didn't stop on an error ended.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Ended {
    Clean,
    /// A batch's or turntable's stack failed, wasn't written or was left
    /// out of the sequence, and the run went on without it. Exit code 2.
    SomeFailed,
}

impl Ended {
    /// How a batch or turntable run with this summary ended: any missing
    /// output is [`Ended::SomeFailed`].
    pub(crate) fn of(summary: &deptheos_app::runs::batch::Summary) -> Self {
        let unwritten = summary.exports.as_ref().is_some_and(|e| !e.failed.is_empty());
        let left_out = !summary.skipped.is_empty();
        if summary.failed > 0 || unwritten || left_out { Self::SomeFailed } else { Self::Clean }
    }
}

fn run(command: Command, words: &Words) -> Result<Ended> {
    let total = Instant::now();
    interrupt::install();
    let ended = match command {
        Command::Pyramid { stack, method } => {
            stack_runs(&stack, Output::one(&stack, method.config()), words)?
        }
        Command::Wavelet { stack, method } => {
            stack_runs(&stack, Output::one(&stack, method.config()), words)?
        }
        Command::Depth { stack, method, sweep } => {
            stack_runs(&stack, Output::depth(&stack, &method, &sweep), words)?
        }
        Command::Batch(command) => batch::run(command, words)?,
        Command::Align(args) => {
            align::run(args, words)?;
            Ended::Clean
        }
        Command::Bench(args) => return bench::run(args, words).map(|()| Ended::Clean),
        Command::Sequence(command) => sequence::run(command, words)?,
    };
    progress!("total {}", secs(total.elapsed()));
    Ok(ended)
}

/// What a stacking command writes: one stack to `--output`, or a depth
/// sweep's runs into the folder `--output` names.
enum Output {
    One(Run),
    /// The first run aligns; the rest replay its alignment.
    Sweep(Vec<Run>),
}

impl Output {
    /// The one run of a command that is not a sweep.
    fn one(stack: &StackArgs, config: StackerConfig) -> Self {
        Self::One(Run { config, output: stack.output.clone() })
    }

    /// The Depth command's runs: one, or a sweep of every radius with every
    /// smoothing, written into the folder `stack.output` as
    /// `r{radius}_s{smoothing}` (plus `_b{blend range}` when not default).
    fn depth(stack: &StackArgs, method: &args::DepthArgs, sweep: &args::SweepArgs) -> Self {
        if !sweep.is_sweep() {
            return Self::one(stack, method.config());
        }
        let or = |v: &Vec<f32>, d: f32| if v.is_empty() { vec![d] } else { v.clone() };
        let (radii, smoothings) =
            (or(&sweep.sweep_radius, method.radius), or(&sweep.sweep_smoothing, method.smoothing));
        let (format, blend_range) = (sweep.format(), method.blend_range);
        let blend = if blend_range == deptheos_app::DepthConfig::DEFAULT_BLEND_RANGE {
            String::new()
        } else {
            format!("_b{blend_range}")
        };
        let mut runs = Vec::new();
        for &radius in &radii {
            for &smoothing in &smoothings {
                let stem = format!("r{radius}_s{smoothing}{blend}");
                runs.push(Run {
                    config: args::depth(radius, smoothing, blend_range),
                    output: stack.output.join(format!("{stem}.{}", format.extension())),
                });
            }
        }
        Self::Sweep(runs)
    }

    fn runs(&self) -> &[Run] {
        match self {
            Self::One(run) => std::slice::from_ref(run),
            Self::Sweep(runs) => runs,
        }
    }
}

/// One stack to run and where to write it; a sweep is a list of these.
struct Run {
    config: StackerConfig,
    output: PathBuf,
}

/// Stacks and writes `output`'s runs. A sweep's folder is made here and
/// removed again if a check refuses an output before any work; its frames
/// are converted once and the first run's alignment replayed by the rest.
fn stack_runs(stack: &StackArgs, output: Output, words: &Words) -> Result<Ended> {
    if let Output::Sweep(_) = output {
        check_sweep_dir(&stack.output)?;
    }
    let frames = prepare::ordered_frames(&stack.frames.inputs, stack.frames.order.into(), words)?;
    let made_dir = match output {
        Output::Sweep(_) if !stack.output.exists() => {
            std::fs::create_dir_all(&stack.output).map_err(CoreError::from)?;
            Some(&stack.output)
        }
        _ => None,
    };
    if let Err(e) = check_outputs(&frames, output.runs()) {
        if let Some(dir) = made_dir {
            let _ = std::fs::remove_dir(dir);
        }
        return Err(e.into());
    }
    let engine = prepare::engine(&stack.decode, words)?;
    let runner = Runner { engine: &engine, frames: &frames, stack, words };
    match output {
        Output::One(run) => {
            let contributions = stack.debug.contributions.clone();
            runner.run(run, None, contributions)?;
        }
        Output::Sweep(runs) => {
            let n_runs = runs.len();
            let _converted = prepare::convert_first(&engine, &frames, stack.options().raw_backend)?;
            let mut geometry: Option<StackGeometry> = None;
            for (i, run) in runs.into_iter().enumerate() {
                let start = Instant::now();
                progress!("=== run {} of {n_runs}: {}", i + 1, run.output.display());
                // Each run's overlays in a folder named for it.
                let contributions = stack.debug.contributions.as_ref().map(|dir| {
                    run.output.file_stem().map_or_else(|| dir.clone(), |stem| dir.join(stem))
                });
                let made = runner.run(run, geometry.clone(), contributions)?;
                progress!("run {}", secs(start.elapsed()));
                geometry.get_or_insert(made);
            }
        }
    }
    Ok(Ended::Clean)
}

/// What every run of a stacking command shares.
struct Runner<'a> {
    engine: &'a Engine,
    frames: &'a [PathBuf],
    stack: &'a StackArgs,
    words: &'a Words,
}

impl Runner<'_> {
    /// Stacks and writes `run` (and its contribution overlays when asked),
    /// replaying `geometry` when given; returns the geometry it used.
    fn run(
        &self,
        run: Run,
        geometry: Option<StackGeometry>,
        contributions: Option<PathBuf>,
    ) -> Result<StackGeometry> {
        let engine = self.engine;
        let mut session = engine.session(self.frames.to_vec(), run.config, self.stack.options())?;
        if let Some(geometry) = geometry {
            session = session.with_alignment(geometry)?;
        }
        let completed = run_with_progress(session)?.stack;
        ui::warn_stack(&completed, self.words, None);
        engine.save(
            Look::exported(&completed, Some(&Adjustments::NEUTRAL)),
            &run.output,
            self.stack.save.options(),
        )?;
        ui::wrote(&run.output);
        if let Some(dir) = contributions {
            write_contributions(engine, &completed, &dir)?;
        }
        Ok(completed.geometry)
    }
}

/// Every frame of `stack` as the GUI's compare source with the
/// contribution overlay, written into `dir` ([`contribution_name`]).
fn write_contributions(engine: &Engine, stack: &CompletedStack, dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).map_err(CoreError::from)?;
    for (index, path) in stack.paths.iter().enumerate() {
        let image =
            engine.compare_rgb(path, Some(index), Look::stored(stack), Overlay::Contribution)?;
        image.save(dir.join(contribution_name(index, path))).map_err(CoreError::from)?;
    }
    match stack.paths.len() {
        1 => progress!("wrote 1 contribution overlay"),
        n => progress!("wrote {n} contribution overlays"),
    }
    ui::wrote(dir);
    Ok(())
}

/// The overlay file name for frame `index` (from 0), e.g.
/// `0003 bee_contribution.jpg`: numbered so frames of one name don't
/// overwrite each other.
fn contribution_name(index: usize, path: &Path) -> String {
    let stem = path.file_stem().map(|s| s.to_string_lossy()).unwrap_or_default();
    format!("{:04} {stem}_contribution.jpg", index + 1)
}

/// Run a session, printing a line per frame and phase.
fn run_with_progress(session: StackingSession) -> Result<SessionOutput> {
    let mut last = Instant::now();
    let mut print = |event: SessionEvent, _: &deptheos_app::LiveMap| {
        let step = last.elapsed();
        last = Instant::now();
        match event {
            // The session logs the reverse check itself.
            SessionEvent::Started { .. } | SessionEvent::Reversed => {}
            SessionEvent::Accumulated { current, total } => {
                progress!("aligning {current} of {total} ({})", secs(step))
            }
            SessionEvent::Finalized => progress!("finalizing ({})", secs(step)),
            SessionEvent::Blended { current, total } => {
                progress!("blending {current} of {total} ({})", secs(step))
            }
            SessionEvent::Completed => progress!("finishing ({})", secs(step)),
        }
    };
    let output = session.run(&mut print, interrupt::token())?;
    if let Some(crop) = output.stack.geometry.crop {
        progress!(
            "cropped to the area every frame covers: {} × {} at x {}, y {}",
            crop.width,
            crop.height,
            crop.x,
            crop.y
        );
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    /// A turntable whose check left a stack out of the sequence exits 2,
    /// as one with a failed stack does; a clean one exits 0.
    #[test]
    fn a_stack_left_out_of_the_sequence_is_a_failure() {
        use deptheos_app::runs::batch::Summary;

        use super::Ended;
        let clean = Summary { total: 3, made: 3, ..Summary::default() };
        assert_eq!(Ended::of(&clean), Ended::Clean);
        let left_out = Summary { skipped: vec!["s3".into()], ..clean.clone() };
        assert_eq!(Ended::of(&left_out), Ended::SomeFailed);
        let failed = Summary { made: 2, failed: 1, ..clean };
        assert_eq!(Ended::of(&failed), Ended::SomeFailed);
    }

    use super::*;

    #[test]
    fn a_sweep_writes_into_a_folder() {
        let dir = tempfile::tempdir().unwrap();
        assert!(check_sweep_dir(&dir.path().join("sweep")).is_ok());
        assert!(check_sweep_dir(&dir.path().join("out.jpg")).is_err());
        let file = dir.path().join("file");
        std::fs::write(&file, b"x").unwrap();
        assert!(check_sweep_dir(&file).is_err());
        // An existing folder may have any name.
        let odd = dir.path().join("v1.png");
        std::fs::create_dir(&odd).unwrap();
        assert!(check_sweep_dir(&odd).is_ok());
    }

    #[test]
    fn frames_of_one_name_write_their_own_contributions() {
        let paths = ["a/bee.jpg", "a/bee.tif", "b/bee.jpg"].map(PathBuf::from);
        let names: Vec<String> =
            paths.iter().enumerate().map(|(i, p)| contribution_name(i, p)).collect();
        assert_eq!(
            names,
            ["0001 bee_contribution.jpg", "0002 bee_contribution.jpg", "0003 bee_contribution.jpg"]
        );
    }
}
