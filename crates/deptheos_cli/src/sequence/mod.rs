//! `deptheos sequence`: grow and turntable sequences as numbered frames
//! (see `deptheos_app::runs::sequence`).
//!
//! - `sequence grow <images…>`: the images stacked, then replayed.
//! - `sequence turntable <folders…>`: stack every folder, restack one with
//!   a stray first frame without it, then write the sequence.

use std::path::PathBuf;
use std::time::Instant;

use clap::{Args, Subcommand, ValueEnum};
use deptheos_app::runs::sequence::{self, Direction, FrameOptions, TurntableMode};
use deptheos_app::{PyramidConfig, SMOOTHNESS_RANGE, SessionOptions, StackerConfig, Words};

use crate::Ended;
use crate::args::{Checks, DecodeArgs, FrameFormatArg, Order, SaveArgs, SourceArg, in_range};
use crate::error::Result;
use crate::ui::{progress, secs};

mod grow;
mod store;
mod turntable;

use grow::grow;
use turntable::turntable;

#[derive(Subcommand)]
pub(crate) enum SequenceCommand {
    /// Frame k is the stack of k frames, so the picture comes into focus
    /// as it plays
    ///
    /// Back to front by default: focus arrives from behind. The last frame
    /// is the full stack. Takes the frames of one focus bracket, stacked
    /// here first.
    #[command(after_help = "\
Examples:
  deptheos sequence grow ~/shots/bee -o ~/sequences
  deptheos sequence grow ~/shots/bee -o ~/sequences --every 2")]
    Grow(GrowArgs),
    /// One stack per angle of a turning object
    ///
    /// "grow" turns and comes into focus at once, "full" turns the finished
    /// stacks. A stack that looks like it began with a stray frame is
    /// restacked without it (see --no-fix).
    #[command(
        after_help = "\
Examples:
  deptheos sequence turntable ~/turn/s* -o ~/sequences
  deptheos sequence turntable ~/turn/s* -o ~/sequences --mode both -N 120",
        after_long_help = "\
Examples:
  deptheos sequence turntable ~/turn/s* -o ~/sequences
  deptheos sequence turntable ~/turn/s* -o ~/sequences --mode both -N 120

Exit status:
  0    every folder stacked and the sequence was written
  1    an error stopped the run, or every stack failed
  2    the command line was refused, or the sequence was written without
       the stacks that failed
  130  interrupted (Ctrl-C); what was written is kept"
    )]
    Turntable(TurntableArgs),
}

impl SequenceCommand {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Grow(_) => "grow",
            Self::Turntable(_) => "turntable",
        }
    }

    pub(crate) fn check(&self, c: &mut Checks) {
        let out = match self {
            Self::Grow(args) => &args.out,
            Self::Turntable(args) => {
                if args.mode == ModeArg::Full {
                    if args.frames.is_some() {
                        c.refuse("-N/--frames applies only to a grow turntable, not --mode full");
                    }
                    if args.front_to_back {
                        c.refuse(
                            "--front-to-back applies only to a grow turntable, not --mode full",
                        );
                    }
                }
                &args.out
            }
        };
        out.save.check(&[out.format.export()], c);
        out.decode.check(c);
    }
}

/// What every sequence writes, and how.
#[derive(Args)]
pub(crate) struct OutputArgs {
    /// The folder to write into: each sequence goes in a new folder there,
    /// "<name> grow" or "<name> full", never over an existing one
    #[arg(short, long, value_name = "DIR", help_heading = "Output")]
    output: PathBuf,

    /// Frame format
    #[arg(long, value_enum, default_value_t, help_heading = "Output")]
    format: FrameFormatArg,

    #[command(flatten)]
    save: SaveArgs,

    /// Scale frames down to this many pixels on their long side [default:
    /// full size]
    #[arg(long, value_name = "PX", help_heading = "Output",
          value_parser = clap::value_parser!(u32).range(16..))]
    long_edge: Option<u32>,

    /// Smoothness of the stacks made here, 1 (crisp, more noise) to 10
    /// (calm, softer) [default: 4]
    #[arg(long, value_parser = in_range(SMOOTHNESS_RANGE), help_heading = "Stacking")]
    smoothness: Option<f32>,

    /// The order the frames of each stack made here stack in [default:
    /// name]
    #[arg(long, value_enum, help_heading = "Stacking")]
    order: Option<Order>,

    #[command(flatten)]
    decode: DecodeArgs,
}

impl OutputArgs {
    fn frames(&self) -> FrameOptions {
        FrameOptions {
            format: self.format.format(),
            save: self.save.options(),
            long_edge: self.long_edge,
        }
    }

    fn order(&self) -> deptheos_app::FrameOrder {
        self.order.unwrap_or_default().into()
    }

    fn config(&self) -> StackerConfig {
        StackerConfig::Pyramid(PyramidConfig {
            smoothness: self.smoothness.unwrap_or(PyramidConfig::DEFAULT_SMOOTHNESS),
        })
    }

    /// Stacks made here always align and crop: the sequence is built from
    /// where each frame landed.
    fn options(&self) -> SessionOptions {
        SessionOptions {
            raw_backend: self.decode.backend(),
            camera_profile: self.decode.profile(),
            order: self.order(),
            ..SessionOptions::default()
        }
    }
}

fn direction(front_to_back: bool) -> Direction {
    if front_to_back { Direction::FrontToBack } else { Direction::BackToFront }
}

#[derive(Args)]
pub(crate) struct GrowArgs {
    /// The frames of one focus bracket, or their folder
    #[arg(required = true)]
    inputs: Vec<PathBuf>,

    /// Write one frame in N (the last is always written)
    #[arg(long, default_value_t = 1, value_name = "N",
          value_parser = clap::value_parser!(u32).range(1..))]
    every: u32,

    /// Build from the nearest frame (front) to the farthest instead
    #[arg(long)]
    front_to_back: bool,

    /// The sequence's name [default: the frames' folder]
    #[arg(long)]
    name: Option<String>,

    #[command(flatten)]
    out: OutputArgs,
}

#[derive(Args)]
pub(crate) struct TurntableArgs {
    /// One folder per stack (s001, s002, …), taken in natural order of
    /// their names
    #[arg(required = true)]
    folders: Vec<PathBuf>,

    /// Which sequences to write
    #[arg(long, value_enum, default_value_t)]
    mode: ModeArg,

    /// Grow only: the sequence's frame count [default: the frames per
    /// stack]
    #[arg(short = 'N', long, value_parser = clap::value_parser!(u32).range(1..))]
    frames: Option<u32>,

    /// Grow only: build each stack from the nearest frame (front) to the
    /// farthest instead
    #[arg(long)]
    front_to_back: bool,

    /// Keep a stack that looks like it began with a stray frame as it is,
    /// rather than restacking it without that frame
    #[arg(long)]
    no_fix: bool,

    /// The run's name, for the sequence folders [default: the folders'
    /// parent]
    #[arg(long)]
    name: Option<String>,

    /// Which file of a RAW + JPEG pair to stack
    #[arg(long, value_enum, default_value_t, help_heading = "Input")]
    source: SourceArg,

    #[command(flatten)]
    out: OutputArgs,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
enum ModeArg {
    /// Turn and come into focus at once
    #[default]
    Grow,
    /// Turn the finished stacks
    Full,
    /// Both sequences
    Both,
}

impl From<ModeArg> for TurntableMode {
    fn from(m: ModeArg) -> Self {
        match m {
            ModeArg::Grow => TurntableMode::Grow,
            ModeArg::Full => TurntableMode::Full,
            ModeArg::Both => TurntableMode::Both,
        }
    }
}

pub(crate) fn run(command: SequenceCommand, words: &Words) -> Result<Ended> {
    match command {
        SequenceCommand::Grow(args) => grow(args, words).map(|()| Ended::Clean),
        SequenceCommand::Turntable(args) => turntable(args, words),
    }
}

/// A progress printer: one line per frame written.
fn print_progress(words: &Words) -> impl FnMut(&sequence::SequenceProgress) {
    let words = words.clone();
    let mut last = Instant::now();
    move |p| {
        if p.written > 0 {
            progress!("{} ({})", p.message(&words), secs(last.elapsed()));
            last = Instant::now();
        }
    }
}
