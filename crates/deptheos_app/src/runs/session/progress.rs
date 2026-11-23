//! How far a stack run has come, from its [`SessionEvent`]s, for every
//! front end: the step, the frames taken in and how full the bar is.

use super::SessionEvent;

/// The step a run is at, as its last event put it.
///
/// - `Started` → `Aligning` (pass 1 takes in a frame) → `Aligning` …
/// - `Aligning` → `Reversed` (pass 1 starts over reversed) → `Aligning`
/// - `Started`, `Aligning` or `Reversed` → `Finalized` (pass 1 is done)
/// - `Finalized` → `Blending` (pass 2 blends a frame) → `Blending` …
/// - `Finalized` or `Blending` → `Completed` (the result is made)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Started,
    /// Pass 1 has taken in `current` of the run's frames.
    Aligning {
        current: u32,
    },
    Reversed,
    Finalized,
    /// Pass 2 has blended up to the `current`th frame.
    Blending {
        current: u32,
    },
    Completed,
}

impl Step {
    fn name(&self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Aligning { .. } => "aligning",
            Self::Reversed => "reversed",
            Self::Finalized => "finalized",
            Self::Blending { .. } => "blending",
            Self::Completed => "completed",
        }
    }
}

/// A run's progress.
///
/// Lifecycle: its [`Step`] (as that enum lays out), the frames taken in
/// and whether last first, and the bar; changed only by [`Progress::on`]
/// with each event, in order. Held by the desktop's `Track`
/// (`deptheos_gpui/src/stacking.rs`) and, through the bridge's
/// `StackProgress`, iOS's `StackingState.Progress`.
///
/// The bar: two-pass fills 0–0.5 taking frames in and 0.5–0.9 blending;
/// single-pass 0–0.95, then 0.98 once finalized; full once completed. It
/// goes back only when the run reverses.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Progress {
    step: Step,
    /// The run's own frame count, as its events say.
    frames: u32,
    two_pass: bool,
    /// The frames taken in, in the order the run takes them.
    taken: u32,
    /// The run takes its frames last first.
    reversed: bool,
    fraction: f32,
}

impl Progress {
    /// A run of `frames` about to start; `two_pass` per
    /// `StackerConfig::needs_blend`.
    pub fn new(frames: u32, two_pass: bool) -> Self {
        Self { step: Step::Started, frames, two_pass, taken: 0, reversed: false, fraction: 0.0 }
    }

    /// After `event`.
    pub fn on(&mut self, event: SessionEvent) {
        let next = match event {
            SessionEvent::Started { total } => {
                self.frames = total;
                self.taken = 0;
                self.fraction = 0.0;
                Step::Started
            }
            SessionEvent::Accumulated { current, total } => {
                self.frames = total;
                self.taken = current.min(total);
                let span = if self.two_pass { 0.5 } else { 0.95 };
                self.fraction = span * current as f32 / total.max(1) as f32;
                Step::Aligning { current }
            }
            SessionEvent::Reversed => {
                // The count starts over, from the last frame.
                self.taken = 0;
                self.reversed = true;
                self.fraction = 0.0;
                Step::Reversed
            }
            SessionEvent::Finalized => {
                // Every frame is taken in from here on.
                self.taken = self.frames;
                self.fraction = if self.two_pass { 0.5 } else { 0.98 };
                Step::Finalized
            }
            SessionEvent::Blended { current, total } => {
                self.fraction = 0.5 + 0.4 * current as f32 / total.max(1) as f32;
                Step::Blending { current }
            }
            SessionEvent::Completed => {
                self.taken = self.frames;
                self.fraction = 1.0;
                Step::Completed
            }
        };
        if std::mem::discriminant(&self.step) != std::mem::discriminant(&next) {
            tracing::debug!(
                from = self.step.name(),
                to = next.name(),
                taken = self.taken,
                "stack progress"
            );
        }
        self.step = next;
    }

    pub fn step(&self) -> Step {
        self.step
    }

    /// The run's frame count.
    pub fn frames(&self) -> u32 {
        self.frames
    }

    /// How many frames the run has taken in, in its order.
    pub fn taken(&self) -> u32 {
        self.taken
    }

    /// Whether the run takes its frames last first.
    pub fn reversed(&self) -> bool {
        self.reversed
    }

    /// How full the progress bar is, 0 to 1.
    pub fn fraction(&self) -> f32 {
        self.fraction
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use SessionEvent::*;

    /// Each event with the step, bar and frames taken in after it.
    fn table(frames: u32, two_pass: bool, rows: &[(SessionEvent, Step, f32, u32, bool)]) {
        let mut progress = Progress::new(frames, two_pass);
        let mut before = progress.fraction();
        for &(event, step, fraction, taken, reversed) in rows {
            progress.on(event);
            let seen = (progress.step(), progress.taken(), progress.reversed());
            assert_eq!(seen, (step, taken, reversed), "after {event:?}");
            let bar = progress.fraction();
            assert!((bar - fraction).abs() < 1e-5, "after {event:?}: {bar}, not {fraction}");
            if event != Reversed {
                assert!(bar >= before, "after {event:?}: back from {before} to {bar}");
            }
            before = bar;
        }
    }

    #[test]
    fn a_two_pass_run_fills_the_bar_once() {
        table(
            3,
            true,
            &[
                (Started { total: 3 }, Step::Started, 0.0, 0, false),
                (
                    Accumulated { current: 1, total: 3 },
                    Step::Aligning { current: 1 },
                    0.5 / 3.0,
                    1,
                    false,
                ),
                (
                    Accumulated { current: 3, total: 3 },
                    Step::Aligning { current: 3 },
                    0.5,
                    3,
                    false,
                ),
                (Finalized, Step::Finalized, 0.5, 3, false),
                (
                    Blended { current: 1, total: 3 },
                    Step::Blending { current: 1 },
                    0.5 + 0.4 / 3.0,
                    3,
                    false,
                ),
                (Blended { current: 3, total: 3 }, Step::Blending { current: 3 }, 0.9, 3, false),
                (Completed, Step::Completed, 1.0, 3, false),
            ],
        );
    }

    #[test]
    fn a_single_pass_run_fills_the_bar_in_its_pass() {
        table(
            4,
            false,
            &[
                (Started { total: 4 }, Step::Started, 0.0, 0, false),
                (
                    Accumulated { current: 2, total: 4 },
                    Step::Aligning { current: 2 },
                    0.475,
                    2,
                    false,
                ),
                (
                    Accumulated { current: 4, total: 4 },
                    Step::Aligning { current: 4 },
                    0.95,
                    4,
                    false,
                ),
                (Finalized, Step::Finalized, 0.98, 4, false),
                (Completed, Step::Completed, 1.0, 4, false),
            ],
        );
    }

    #[test]
    fn a_reversed_run_starts_over_from_the_last_frame() {
        table(
            4,
            true,
            &[
                (Started { total: 4 }, Step::Started, 0.0, 0, false),
                (
                    Accumulated { current: 2, total: 4 },
                    Step::Aligning { current: 2 },
                    0.25,
                    2,
                    false,
                ),
                (Reversed, Step::Reversed, 0.0, 0, true),
                (
                    Accumulated { current: 1, total: 4 },
                    Step::Aligning { current: 1 },
                    0.125,
                    1,
                    true,
                ),
                (Accumulated { current: 4, total: 4 }, Step::Aligning { current: 4 }, 0.5, 4, true),
                (Finalized, Step::Finalized, 0.5, 4, true),
                (Blended { current: 4, total: 4 }, Step::Blending { current: 4 }, 0.9, 4, true),
                (Completed, Step::Completed, 1.0, 4, true),
            ],
        );
    }

    /// The count is the run's own, as its events say.
    #[test]
    fn the_count_is_the_runs_own() {
        table(
            5,
            false,
            &[
                (Started { total: 3 }, Step::Started, 0.0, 0, false),
                (
                    Accumulated { current: 3, total: 3 },
                    Step::Aligning { current: 3 },
                    0.95,
                    3,
                    false,
                ),
                (Finalized, Step::Finalized, 0.98, 3, false),
            ],
        );
        assert_eq!(Progress::new(5, false).frames(), 5);
    }
}
