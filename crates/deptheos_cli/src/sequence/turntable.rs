//! `sequence turntable`: stack every folder, restack a stack with a stray
//! first frame, then write the sequence.

use std::collections::HashMap;
use std::path::PathBuf;

use deptheos_app::runs::batch::{self, Split};
use deptheos_app::runs::sequence::{
    self, ManifestGap, TurntableMode, TurntableOptions, TurntableStack,
};
use deptheos_app::{CompletedStack, CoreError, Engine, Words};

use super::store::TempStore;
use super::{TurntableArgs, direction, print_progress};
use crate::drive::{Made, Runner, drive};
use crate::error::{Error, Refusal, Result};
use crate::stacking::{Attempt, plan_folders, stack_one};
use crate::ui::{self, progress};
use crate::{Ended, prepare};
use deptheos_app::frames::names::file_label;

/// A turntable run: [`Ended::SomeFailed`] when the sequence was written
/// without stacks that failed.
pub(super) fn turntable(args: TurntableArgs, words: &Words) -> Result<Ended> {
    let out = &args.out;
    let order = out.order.unwrap_or_default();
    let plan = plan_folders(
        &args.folders,
        Split::PerFolder,
        args.source,
        order,
        out.decode.backend(),
        words,
    );
    let label = args.name.clone().unwrap_or_else(|| {
        args.folders[0].parent().map(file_label).unwrap_or_else(|| "turntable".into())
    });
    let summary = plan.summary(words);
    progress!(
        "{summary} + turntable {}",
        TurntableMode::from(args.mode).label(words).to_lowercase()
    );
    if plan.jobs.is_empty() {
        return Err(Refusal::NoFolderHasFrames.into());
    }
    let options = TurntableOptions {
        mode: args.mode.into(),
        direction: direction(args.front_to_back),
        frames_n: args.frames.map(|n| n as usize),
        frames: out.frames(),
    };
    // The sequence's size, plus the renders kept while it runs.
    if let Some(bytes) = sequence::check_turntable_space(&out.output, &plan.jobs, &options)? {
        let size = deptheos_app::words::units::size_text(bytes.total(), words);
        match bytes.renders {
            0 => progress!("{} frames, about {size}", bytes.frames),
            _ => progress!(
                "{} frames, about {size} with the renders kept while it runs",
                bytes.frames
            ),
        }
    }

    let engine = prepare::engine(&out.decode, words)?;
    std::fs::create_dir_all(&out.output).map_err(CoreError::from)?;
    let renders = Some(TempStore::new(&out.output, args.mode.into())?);
    tracing::debug!("turntable renders: collecting");

    // The machine stacks every folder, restacks those that look off, then
    // writes the sequence from the stored stacks.
    let machine_options = batch::machine::Options {
        save: false,
        export: false,
        turntable: Some(batch::machine::Turntable { fix: !args.no_fix }),
    };
    let mut machine =
        batch::Machine::new(plan.jobs.clone(), machine_options, engine.words().clone());
    let mut runner = TurntableRunner {
        engine: &engine,
        args: &args,
        words,
        label,
        options,
        renders,
        made: HashMap::new(),
    };
    drive(&mut machine, &mut runner)
}

/// `sequence turntable`'s side of the run: stack each folder into the
/// store, then write the sequence from it.
struct TurntableRunner<'a> {
    engine: &'a Engine,
    args: &'a TurntableArgs,
    words: &'a Words,
    /// What the sequence is called.
    label: String,
    options: TurntableOptions,
    /// The renders. Still here at the end means no sequence was written.
    ///
    /// Lifecycle: `Some` while collecting (`keep` pushes each stack) →
    /// `None` once `write_sequence` turns it into a `KeptStore`
    /// (`sequence/store.rs`); a stack or sequence after that is a fault.
    renders: Option<TempStore>,
    /// Each stack's index in the store and what the sequence needs of it.
    made: HashMap<batch::StackNo, (usize, TurntableStack)>,
}

impl Runner for TurntableRunner<'_> {
    type Kept = CompletedStack;

    fn stack(
        &mut self,
        status: &str,
        kind: batch::StackKind,
        label: &str,
        frames: Vec<PathBuf>,
    ) -> Result<std::result::Result<Made<CompletedStack>, String>> {
        let attempt = match kind {
            batch::StackKind::Restack => Attempt::Restack,
            batch::StackKind::Job => Attempt::First,
        };
        let out = &self.args.out;
        let made =
            stack_one(self.engine, out.config(), out.options(), attempt, status, label, frames)?;
        Ok(made.map(Made::of))
    }

    fn keep(&mut self, stack: batch::StackNo, label: String, kept: CompletedStack) -> Result<()> {
        let Some(store) = &mut self.renders else {
            return Err(Error::Fault("a stack was made after the sequence was written".into()));
        };
        let stacked = TurntableStack {
            label,
            render: None,
            paths: kept.paths.clone(),
            geometry: kept.geometry.clone(),
        };
        self.made.insert(stack, (store.push(kept)?, stacked));
        Ok(())
    }

    fn write_sequence(
        &mut self,
        stacks: Vec<batch::SequenceStack>,
        skipped: Vec<ManifestGap>,
        notes: Vec<String>,
    ) -> Result<String> {
        let (indices, stacks): (Vec<usize>, Vec<TurntableStack>) =
            stacks.iter().map(|s| self.made[&s.stack].clone()).unzip();
        let Some(store) = self.renders.take() else {
            return Err(Error::Fault("the sequence was written twice".into()));
        };
        tracing::debug!("turntable renders: written");
        let mut store = store.keep(&indices)?;
        let reports = sequence::export_turntable(
            self.engine,
            &stacks,
            &mut store,
            &self.label,
            &self.args.out.output,
            &self.options,
            &skipped,
            &notes,
            &mut print_progress(self.words),
            crate::interrupt::token(),
        )?;
        for report in &reports {
            ui::wrote(&report.folder);
        }
        let line = reports.iter().map(|r| r.line(self.words)).collect::<Vec<_>>();
        Ok(line.join(" · "))
    }

    fn finish(&mut self, summary: &batch::Summary) -> Result<Ended> {
        if self.renders.is_some() && summary.end != batch::End::Canceled {
            return Err(Refusal::EveryStackFailed.into());
        }
        // The time taken follows on its own line ("total 12.3 s").
        progress!("{}", summary.text(self.engine.words(), None));
        Ok(Ended::of(summary))
    }
}
