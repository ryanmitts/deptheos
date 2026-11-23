//! What a batch and a turntable run share: planning folders and stacking
//! one job, so both agree on what a failed stack is (ADR-103).

use std::path::PathBuf;
use std::time::Instant;

use deptheos_app::runs::batch::{self, Input, Plan, Split};
use deptheos_app::{CompletedStack, Engine, RawBackend, SessionOptions, StackerConfig, Words};

use crate::args::{Order, SourceArg};
use crate::error::{Result, tell_engine};
use crate::ui::{self, progress, secs};

/// The plan for `folders`, in natural name order, warning of unreadable
/// folders and of each stack's order.
pub(crate) fn plan_folders(
    folders: &[PathBuf],
    split: Split,
    source: SourceArg,
    order: Order,
    backend: RawBackend,
    words: &Words,
) -> Plan {
    let mut inputs: Vec<Input> = folders.iter().cloned().map(Input::Folder).collect();
    batch::sort_folders(&mut inputs);
    let plan = batch::plan(
        &inputs,
        split,
        source.into(),
        order.into(),
        backend,
        batch::capture_info,
        words,
    );
    for (path, why) in &plan.unreadable {
        ui::warn(format!("couldn't read the folder {}: {why}", path.display()));
    }
    for note in &plan.order_notes {
        ui::warn(note);
    }
    plan
}

/// A first stack's line, printed when it ends, counts its frames and time;
/// a restack (the turntable's) prints the machine's line as it starts.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Attempt {
    First,
    Restack,
}

/// Stacks `frames` as the job `label`, printing how it went. A stack that
/// can't start fails like one that stops partway; only a cancel is an
/// error.
pub(crate) fn stack_one(
    engine: &Engine,
    config: StackerConfig,
    options: SessionOptions,
    attempt: Attempt,
    status: &str,
    label: &str,
    frames: Vec<PathBuf>,
) -> Result<std::result::Result<CompletedStack, String>> {
    let t = Instant::now();
    let line = match (attempt, frames.len()) {
        (Attempt::Restack, _) => status.to_string(),
        (Attempt::First, 1) => format!("{status} · 1 frame"),
        (Attempt::First, n) => format!("{status} · {n} frames"),
    };
    if attempt == Attempt::Restack {
        progress!("{line}");
    }
    let made = engine
        .session(frames, config, options)
        .and_then(|session| session.run(&mut |_, _| {}, crate::interrupt::token()));
    match made {
        Ok(output) => {
            if attempt == Attempt::First {
                progress!("{line} ({})", secs(t.elapsed()));
            }
            ui::warn_stack(&output.stack, engine.words(), Some(label));
            Ok(Ok(output.stack))
        }
        Err(deptheos_app::Error::Canceled) => Err(deptheos_app::Error::Canceled.into()),
        Err(e) => {
            let words = engine.words();
            let told = tell_engine(&e, words);
            eprintln!("error: {}", batch::stack_failure(words, label, &told.message));
            told.print_details();
            Ok(Err(told.message))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deptheos_app::PyramidConfig;

    /// A session that refuses to start is a failed stack, not an error;
    /// mixed DNG and standard frames are refused before any I/O.
    #[test]
    fn a_stack_that_cannot_start_is_a_failed_stack() {
        let Some(engine) = deptheos_app::test_support::engine() else { return };
        let dir = tempfile::tempdir().unwrap();
        let frames = vec![dir.path().join("a.dng"), dir.path().join("b.png")];
        let made = stack_one(
            &engine,
            StackerConfig::Pyramid(PyramidConfig::default()),
            SessionOptions::default(),
            Attempt::First,
            "1 of 1",
            "mixed",
            frames,
        );
        assert!(matches!(made, Ok(Err(_))), "a failed stack, not an error");
    }
}
