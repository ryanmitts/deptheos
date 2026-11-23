//! `sequence grow`: images stacked, then replayed. The command line never
//! opens a Library, which only the app holding its lock writes.

use std::path::PathBuf;
use std::time::Instant;

use deptheos_app::runs::sequence::{self, GrowOptions};
use deptheos_app::{CompletedStack, Engine, Words};

use super::{GrowArgs, OutputArgs, direction, print_progress};
use crate::error::Result;
use crate::prepare;
use crate::ui::{self, progress, secs};
use deptheos_app::frames::names::file_label;

pub(super) fn grow(args: GrowArgs, words: &Words) -> Result<()> {
    let started = Instant::now();
    let engine = prepare::engine(&args.out.decode, words)?;
    let paths = prepare::ordered_frames(&args.inputs, args.out.order(), words)?;
    let name = paths[0].parent().map(file_label).unwrap_or_else(|| "sequence".to_string());
    progress!("stacking {} frames", paths.len());
    let stack = stack_frames(&engine, &args.out, paths)?;
    // What the stack left out, which the sequence skips, and its notes.
    ui::warn_stack(&stack, words, None);
    let name = args.name.clone().unwrap_or(name);
    let options = GrowOptions {
        direction: direction(args.front_to_back),
        every: args.every as usize,
        frames: args.out.frames(),
    };
    let report = sequence::export_grow(
        &engine,
        &stack,
        None,
        &name,
        &args.out.output,
        &options,
        &mut print_progress(words),
        crate::interrupt::token(),
    )?;
    progress!("wrote {} frames ({})", report.frames, secs(started.elapsed()));
    ui::wrote(&report.folder);
    Ok(())
}

/// Stack `paths`, aligned and cropped. The stack keeps the paths given,
/// not their conversions: a replay converts them again as it reads them.
fn stack_frames(engine: &Engine, out: &OutputArgs, paths: Vec<PathBuf>) -> Result<CompletedStack> {
    let session = engine.session(paths, out.config(), out.options())?;
    let output = session.run(&mut |_, _| {}, crate::interrupt::token())?;
    let mut stack = output.stack;
    stack.frame_map = None;
    Ok(stack)
}
