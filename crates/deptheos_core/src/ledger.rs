//! Which frames of a stack have been stacked and which were left out.
//!
//! Every stacker gives each frame an index in pass 1, whether it stacks the
//! frame or leaves it out, so the frames after a left-out one keep their
//! planes.

use crate::plane::PlaneRamp;
use crate::{CoreError, Result};

#[derive(Debug, Clone)]
pub(crate) struct FrameLedger {
    total: u32,
    /// One entry per frame seen so far: whether it was left out.
    left_out: Vec<bool>,
}

impl FrameLedger {
    pub(crate) fn new(total: u32) -> Self {
        Self { total, left_out: Vec::new() }
    }

    /// Refuses a stack of no frames.
    pub(crate) fn check_total(&self) -> Result<()> {
        if self.total == 0 {
            return Err(CoreError::InvalidState("a stack needs at least one frame".into()));
        }
        Ok(())
    }

    /// The next frame's index, refusing a frame past the end of the stack.
    pub(crate) fn check_room(&self) -> Result<u32> {
        if self.next() >= self.total {
            return Err(CoreError::InvalidState(format!(
                "frame {} of a {}-frame stack",
                self.next() + 1,
                self.total
            )));
        }
        Ok(self.next())
    }

    /// Frames stacked or left out so far: the next frame's index.
    pub(crate) fn next(&self) -> u32 {
        self.left_out.len() as u32
    }

    /// Record that frame `index`, the one [`Self::check_room`] gave, was
    /// stacked.
    pub(crate) fn push_stacked(&mut self, index: u32) {
        debug_assert!(index == self.next() && index < self.total);
        self.left_out.push(false);
    }

    /// Leave the next frame out, keeping its index.
    pub(crate) fn skip(&mut self) -> Result<()> {
        self.check_room()?;
        self.left_out.push(true);
        Ok(())
    }

    /// Whether frame `index` was stacked.
    pub(crate) fn stacked(&self, index: u32) -> bool {
        self.left_out.get(index as usize).is_some_and(|&l| !l)
    }

    /// How many frames were stacked.
    pub(crate) fn stacked_count(&self) -> u32 {
        self.left_out.iter().filter(|&&l| !l).count() as u32
    }

    /// The indices of the stacked frames, in order.
    pub(crate) fn stacked_indices(&self) -> impl Iterator<Item = u32> + '_ {
        (0..self.next()).filter(|&i| self.stacked(i))
    }

    /// Plane distances from frame `index` to the nearest stacked frames
    /// below and above it (one step where there is none).
    pub(crate) fn plane_ramp(&self, index: u32) -> PlaneRamp {
        PlaneRamp::new(index, self.total, |j| self.stacked(j))
    }
}

/// Pass 2's frames: which of the stacked frames have been through it.
///
/// Lifecycle: made from the pass-1 ledger when a two-pass stacker enters
/// pass 2 (inside depth's `Blending` and wavelet's `Reassigning`); each
/// stacked frame is marked once, in any order ([`Self::check`], then
/// [`Self::mark`]); [`Self::check_complete`] refuses to finish early; it
/// drops with that state at `finalize`.
#[derive(Debug, Clone)]
pub(crate) struct SecondPass {
    done: Vec<bool>,
}

impl SecondPass {
    pub(crate) fn new(ledger: &FrameLedger) -> Self {
        Self { done: vec![false; ledger.next() as usize] }
    }

    /// Refuses a frame pass 1 didn't stack, and one already through.
    pub(crate) fn check(&self, ledger: &FrameLedger, index: u32) -> Result<()> {
        if !ledger.stacked(index) {
            return Err(CoreError::InvalidState(format!(
                "frame {index} was not stacked in the first pass"
            )));
        }
        if self.done[index as usize] {
            return Err(CoreError::InvalidState(format!(
                "frame {index} has already been through the second pass"
            )));
        }
        Ok(())
    }

    /// Record frame `index` as through. Call after [`Self::check`].
    pub(crate) fn mark(&mut self, index: u32) {
        self.done[index as usize] = true;
    }

    /// Refuses to finish while a stacked frame has not been through.
    pub(crate) fn check_complete(&self, ledger: &FrameLedger) -> Result<()> {
        match ledger.stacked_indices().find(|&i| !self.done[i as usize]) {
            Some(missing) => Err(CoreError::InvalidState(format!(
                "frame {missing} has not been through the second pass"
            ))),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::plane::FramePlane;

    /// A pass-1 record from a pattern: `s` a stacked frame, `-` a left-out one.
    pub(crate) fn ledger(pattern: &str) -> FrameLedger {
        let mut frames = FrameLedger::new(pattern.len() as u32);
        for c in pattern.chars() {
            match c {
                's' => frames.push_stacked(frames.next()),
                _ => frames.skip().unwrap(),
            }
        }
        frames
    }

    /// A left-out frame's neighbors ramp to each other across its plane,
    /// so their weights still sum to one there.
    #[test]
    fn neighbors_reach_across_a_left_out_frame() {
        // Frames 0–2 stacked, 3 left out, 4–5 stacked.
        let frames = ledger("sss-ss");
        let step = FramePlane::new(0, 6).step;
        let spans = |i| {
            let ramp = frames.plane_ramp(i);
            (ramp.below, ramp.above)
        };
        assert_eq!(spans(1), (step, step));
        assert_eq!(spans(2), (step, 2.0 * step));
        assert_eq!(spans(4), (2.0 * step, step));
        assert!(!frames.stacked(3));
        // At frame 3's plane, frames 2 and 4 weigh one half each.
        let z = FramePlane::new(3, 6).plane;
        let weight = |i| frames.plane_ramp(i).weight(z);
        assert!((weight(2) + weight(4) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn counts_stacked_and_left_out_frames() {
        let mut ledger = FrameLedger::new(4);
        ledger.push_stacked(0);
        ledger.skip().unwrap();
        ledger.push_stacked(2);
        assert_eq!(ledger.next(), 3);
        assert_eq!(ledger.stacked_count(), 2);
        assert!(ledger.stacked(0));
        assert!(!ledger.stacked(1));
        assert!(!ledger.stacked(3), "a frame not yet seen is not stacked");
        assert_eq!(ledger.stacked_indices().collect::<Vec<_>>(), [0, 2]);
        assert_eq!(ledger.left_out, [false, true, false]);
    }

    #[test]
    fn refuses_a_frame_past_the_end() {
        let mut ledger = FrameLedger::new(1);
        ledger.skip().unwrap();
        assert!(ledger.check_room().is_err());
        assert!(ledger.skip().is_err());
        assert_eq!(ledger.next(), 1);
    }

    #[test]
    fn check_room_gives_the_next_index() {
        let mut ledger = FrameLedger::new(2);
        assert_eq!(ledger.check_room().unwrap(), 0);
        ledger.skip().unwrap();
        assert_eq!(ledger.check_room().unwrap(), 1);
    }

    #[test]
    fn the_second_pass_takes_each_stacked_frame_once() {
        let mut ledger = FrameLedger::new(4);
        ledger.push_stacked(0);
        ledger.skip().unwrap();
        ledger.push_stacked(2);
        let mut pass = SecondPass::new(&ledger);
        assert!(pass.check_complete(&ledger).is_err(), "nothing has been through");
        assert!(pass.check(&ledger, 1).is_err(), "left out of the first pass");
        assert!(pass.check(&ledger, 3).is_err(), "never seen by the first pass");
        pass.check(&ledger, 2).unwrap();
        pass.mark(2);
        assert!(pass.check(&ledger, 2).is_err(), "a second time would count it twice");
        assert!(pass.check_complete(&ledger).is_err(), "frame 0 is still to come");
        pass.check(&ledger, 0).unwrap();
        pass.mark(0);
        pass.check_complete(&ledger).unwrap();
    }

    #[test]
    fn refuses_an_empty_stack() {
        assert!(FrameLedger::new(0).check_total().is_err());
        assert!(FrameLedger::new(1).check_total().is_ok());
    }
}
