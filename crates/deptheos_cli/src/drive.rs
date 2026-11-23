//! The loop that drives a [`deptheos_app::runs::batch::Machine`] for
//! `batch` and `sequence turntable`, one action at a time through a
//! [`Runner`]. There is no Library, so it records and deletes nothing.
//!
//! Ctrl-C ([`crate::interrupt`]) cancels the running stack or sequence (an
//! export finishes first); the machine winds down and its summary is
//! printed before the command exits as interrupted.

use std::collections::VecDeque;
use std::path::PathBuf;

use deptheos_app::runs::batch::{
    self, Action, Event, Machine, SequenceEnd, SequenceStack, StackNo,
};
use deptheos_app::runs::sequence::{ManifestGap, StackShape};
use deptheos_app::{CompletedStack, Error as EngineError};

/// A stack a [`Runner`] made: its frames as stacked and its shape, for the
/// machine, and what the runner keeps of it.
pub(crate) struct Made<K> {
    pub(crate) paths: Vec<PathBuf>,
    pub(crate) shape: StackShape,
    pub(crate) kept: K,
}

impl Made<CompletedStack> {
    pub(crate) fn of(completed: CompletedStack) -> Self {
        Self {
            paths: completed.paths.clone(),
            shape: StackShape::of(&completed.geometry),
            kept: completed,
        }
    }
}

use crate::Ended;
use crate::error::{Error, Result};

/// What the command does for each machine action. An `Err` other than a
/// cancel ends the command.
pub(crate) trait Runner {
    /// What it keeps of a stack made.
    type Kept;

    /// Stacks `frames` as job `label`: the stack made, or why it failed in
    /// sentences. `Err(Canceled)` is Ctrl-C.
    fn stack(
        &mut self,
        status: &str,
        kind: batch::StackKind,
        label: &str,
        frames: Vec<PathBuf>,
    ) -> Result<std::result::Result<Made<Self::Kept>, String>>;

    /// Keeps `kept` for what the machine asks of `stack` next.
    fn keep(&mut self, stack: StackNo, label: String, kept: Self::Kept) -> Result<()>;

    /// Exports `stack` as `label`. A run that exports nothing never asks.
    fn export(&mut self, stack: StackNo, label: &str) -> Result<std::result::Result<(), String>> {
        let _ = (stack, label);
        Err(Error::Fault("a run that exports nothing asked for an export".into()))
    }

    /// Writes the turntable sequence, returning its short line ("48 frames
    /// written"). A run without a turntable never asks.
    fn write_sequence(
        &mut self,
        stacks: Vec<SequenceStack>,
        skipped: Vec<ManifestGap>,
        notes: Vec<String>,
    ) -> Result<String> {
        let _ = (stacks, skipped, notes);
        Err(Error::Fault("a run without a turntable asked for a sequence".into()))
    }

    /// Prints the summary and says how the command ends; a canceled run
    /// ends as interrupted regardless.
    fn finish(&mut self, summary: &batch::Summary) -> Result<Ended>;
}

/// The short part a sequence cut off by Ctrl-C adds to the summary.
const SEQUENCE_INTERRUPTED: &str = "sequence interrupted, its manifest marked incomplete";

/// Runs `machine` to its end through `runner`.
///
/// Lifecycle: the run's states are the [`Machine`]'s
/// (`crates/deptheos_app/src/runs/batch/machine.rs`, ADR-144), changed only
/// by `machine.on` and `machine.stacked` here. This loop holds only the
/// pending actions: `Event::Start`, then each action through `runner` and
/// its result back to the machine, until `Action::Finish`.
pub(crate) fn drive(machine: &mut Machine, runner: &mut impl Runner) -> Result<Ended> {
    let mut actions: VecDeque<Action> = machine.on(Event::Start).into();
    while let Some(action) = actions.pop_front() {
        let event = match action {
            // Queued before a failure or Ctrl-C ended the run: stopped.
            Action::Stack { .. } if machine.is_ending() => Event::StackStopped,
            Action::Stack { stack, label, frames, .. } => {
                let status = machine.status();
                let kind = machine.running().map_or(batch::StackKind::Job, |(_, kind)| kind);
                match runner.stack(&status, kind, &label, frames) {
                    Ok(Ok(Made { paths, shape, kept })) => {
                        let made = machine.stacked(paths, shape);
                        // No Library here: nothing is saved.
                        debug_assert!(made.save.is_none(), "the command line saves nothing");
                        runner.keep(stack, label, kept)?;
                        actions.extend(made.then);
                        continue;
                    }
                    Ok(Err(error)) => Event::StackFailed(error),
                    Err(Error::Engine(EngineError::Canceled)) => {
                        interrupted(machine, &mut actions);
                        Event::StackStopped
                    }
                    Err(e) => return Err(e),
                }
            }
            Action::Export { stack, label, .. } => {
                let result = runner.export(stack, &label)?;
                Event::Exported { stack, result }
            }
            Action::WriteSequence { stacks, skipped, notes } => {
                match runner.write_sequence(stacks, skipped, notes) {
                    Ok(line) => Event::SequenceEnded(SequenceEnd::Finished(line)),
                    Err(Error::Engine(EngineError::Canceled)) => {
                        interrupted(machine, &mut actions);
                        Event::SequenceEnded(SequenceEnd::Canceled(SEQUENCE_INTERRUPTED.into()))
                    }
                    Err(e) => return Err(e),
                }
            }
            // No Library to record in, and nothing runs concurrently to
            // cancel.
            Action::RecordUnstacked { .. }
            | Action::RecordFailure { .. }
            | Action::Delete(_)
            | Action::CancelStack
            | Action::CancelSequence => continue,
            Action::Finish(summary) => {
                let ended = runner.finish(&summary);
                return match summary.end {
                    batch::End::Canceled => ended.and(Err(EngineError::Canceled.into())),
                    _ => ended,
                };
            }
        };
        actions.extend(machine.on(event));
    }
    Err(Error::Fault("a run ended without its summary".into()))
}

/// Tells the machine of Ctrl-C, so it starts nothing more.
fn interrupted(machine: &mut Machine, actions: &mut VecDeque<Action>) {
    tracing::debug!("batch: interrupted");
    actions.extend(machine.on(Event::Cancel));
}

#[cfg(test)]
mod tests {
    use super::*;
    use deptheos_app::runs::batch::machine::Options;
    use deptheos_app::runs::batch::{Job, Summary};
    use deptheos_app::{Platform, Words};

    /// One result per stack: `Some(true)` made, `Some(false)` failed,
    /// `None` Ctrl-C.
    struct Fake {
        results: VecDeque<Option<bool>>,
        exported: Vec<String>,
        finished: Option<Summary>,
    }

    impl Runner for Fake {
        type Kept = ();

        fn stack(
            &mut self,
            _: &str,
            _: batch::StackKind,
            _: &str,
            frames: Vec<PathBuf>,
        ) -> Result<std::result::Result<Made<()>, String>> {
            let shape = StackShape { reversed: false, first_step: None };
            match self.results.pop_front().expect("a stack asked for too many") {
                Some(true) => Ok(Ok(Made { paths: frames, shape, kept: () })),
                Some(false) => Ok(Err("The frames are missing.".into())),
                None => Err(EngineError::Canceled.into()),
            }
        }

        fn keep(&mut self, _: StackNo, _: String, _: ()) -> Result<()> {
            Ok(())
        }

        fn export(&mut self, _: StackNo, label: &str) -> Result<std::result::Result<(), String>> {
            self.exported.push(label.to_string());
            Ok(Ok(()))
        }

        fn finish(&mut self, summary: &Summary) -> Result<Ended> {
            self.finished = Some(summary.clone());
            Ok(Ended::of(summary))
        }
    }

    fn run(results: &[Option<bool>]) -> (Result<Ended>, Fake) {
        let jobs: Vec<Job> = ["a", "b", "c"]
            .iter()
            .map(|l| Job { label: l.to_string(), frames: vec![PathBuf::from(format!("/{l}/1"))] })
            .collect();
        let options = Options { save: false, export: true, turntable: None };
        let mut machine = Machine::new(jobs, options, Words::english(Platform::CommandLine));
        let mut fake = Fake {
            results: results.iter().copied().collect(),
            exported: Vec::new(),
            finished: None,
        };
        let ended = drive(&mut machine, &mut fake);
        (ended, fake)
    }

    /// Every stack is made and exported in turn; one that fails is
    /// counted and the run goes on.
    #[test]
    fn a_run_stacks_and_exports_each_in_turn() {
        let (ended, fake) = run(&[Some(true), Some(false), Some(true)]);
        assert!(matches!(ended, Ok(Ended::SomeFailed)));
        assert_eq!(fake.exported, ["a", "c"]);
        assert_eq!(fake.finished.unwrap().made, 2);
    }

    /// Ctrl-C during the second stack: what finished is summed up, then
    /// the command ends as interrupted.
    #[test]
    fn ctrl_c_prints_the_summary_of_what_finished() {
        let (ended, fake) = run(&[Some(true), None]);
        assert!(matches!(ended, Err(Error::Engine(EngineError::Canceled))), "{ended:?}");
        assert_eq!(fake.exported, ["a"]);
        let summary = fake.finished.expect("the summary is printed");
        assert_eq!(summary.end, batch::End::Canceled);
        assert_eq!(
            summary.text(&Words::english(Platform::CommandLine), None),
            "Batch canceled: 1 of 3 made · 1 exported"
        );
    }
}
