//! The turntable stack check: a stack that looks like it began with a
//! stray frame, restacked without it.

use std::collections::VecDeque;

use super::ManifestGap;
use crate::runs::session::StackGeometry;
use crate::words::{Words, say};

/// What the stray-frame check reads of a stack.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StackShape {
    pub reversed: bool,
    /// The magnification of the second stacked frame against the first.
    pub first_step: Option<f64>,
}

impl StackShape {
    pub fn of(geometry: &StackGeometry) -> Self {
        let first_step = geometry
            .stacked_order()
            .get(1)
            .and_then(|&i| geometry.transforms[i])
            .map(|t| f64::from(t.magnification()));
        Self { reversed: geometry.reversed, first_step }
    }
}

/// The turntable stacks that look like they began with a stray frame from
/// the previous bracket: `reversed` unlike most, or a first step far from
/// the others'. They would jump in scale against their neighbors.
pub(crate) fn flag_stacks(shapes: &[StackShape]) -> Vec<bool> {
    let reversed = shapes.iter().filter(|s| s.reversed).count();
    let majority = reversed * 2 > shapes.len();
    let steps: Vec<f64> = shapes.iter().filter_map(|s| s.first_step).collect();
    let (median, spread) = if steps.len() >= 3 {
        let median = median_of(steps.clone());
        let mad = median_of(steps.iter().map(|s| (s - median).abs()).collect());
        // 0.2% of scale: far above a bracket's steps, far below a stray's.
        (Some(median), (8.0 * 1.4826 * mad).max(0.002))
    } else {
        (None, 0.0)
    };
    shapes
        .iter()
        .map(|s| {
            let off_reverse = shapes.len() >= 3 && s.reversed != majority;
            let off_step = match (median, s.first_step) {
                (Some(m), Some(step)) => (step - m).abs() > spread,
                _ => false,
            };
            off_reverse || off_step
        })
        .collect()
}

/// A turntable run's check and fix: each stack [`flag_stacks`] flags is
/// restacked without its first frame and kept if it lines up; otherwise it
/// is dropped from the sequence.
#[derive(Clone, Debug)]
pub(crate) struct StackFix {
    shapes: Vec<StackShape>,
    queue: VecDeque<usize>,
}

/// What restacking one stack came to.
#[derive(Clone, Debug)]
pub(crate) enum Fixed {
    /// Keep the restack in the stack's place; the note goes in the manifest
    /// and closing message.
    Kept { note: String },
    /// Leave the stack out of the sequence.
    Dropped(ManifestGap),
}

impl StackFix {
    /// The run's stacks, shape and frame count each; one too short to lose
    /// a frame isn't restacked.
    pub(crate) fn new(stacks: impl IntoIterator<Item = (StackShape, usize)>) -> Self {
        let (shapes, frames): (Vec<StackShape>, Vec<usize>) = stacks.into_iter().unzip();
        let flagged = flag_stacks(&shapes);
        let queue = (0..shapes.len())
            .filter(|&i| flagged[i] && frames[i] > crate::runs::batch::MIN_FRAMES)
            .collect();
        Self { shapes, queue }
    }

    /// The next stack to restack without its first frame.
    pub(crate) fn next_to_fix(&mut self) -> Option<usize> {
        self.queue.pop_front()
    }

    /// Restack stack `i` again next: its restack was stopped unjudged.
    pub(crate) fn again(&mut self, i: usize) {
        self.queue.push_front(i);
    }

    /// Restacking stack `i` (`label`) made `restacked`, or failed.
    pub(crate) fn judge(
        &mut self,
        words: &Words,
        i: usize,
        label: &str,
        restacked: std::result::Result<StackShape, &str>,
    ) -> Fixed {
        let shape = match restacked {
            Ok(shape) => shape,
            Err(e) => {
                return Fixed::Dropped(ManifestGap {
                    what: label.into(),
                    reason: crate::words::sentences(
                        words,
                        [say!(words, "gap-restack-failed").as_str(), e],
                    ),
                });
            }
        };
        let mut shapes = self.shapes.clone();
        shapes[i] = shape;
        if flag_stacks(&shapes)[i] {
            return Fixed::Dropped(ManifestGap {
                what: label.into(),
                reason: say!(words, "gap-scale-off"),
            });
        }
        // Later stacks are judged against the fixed one.
        self.shapes = shapes;
        Fixed::Kept { note: say!(words, "note-restacked", stack = label) }
    }
}

/// "Checking stacks · restacking s006 without its first frame".
pub(crate) fn fixing_text(words: &Words, label: &str) -> String {
    say!(words, "checking-stacks", stack = label)
}

fn median_of(mut values: Vec<f64>) -> f64 {
    values.sort_by(|a, b| a.total_cmp(b));
    let mid = values.len() / 2;
    if values.len().is_multiple_of(2) { (values[mid - 1] + values[mid]) / 2.0 } else { values[mid] }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use deptheos_core::{Affine2, FrameAlignment};

    pub(crate) fn geometry(n: usize, reversed: bool, step: f32) -> StackGeometry {
        // Frame i of a forward stack is (step^i) magnified against frame 0.
        let transforms = (0..n)
            .map(|i| {
                let i = if reversed { n - 1 - i } else { i };
                let s = step.powi(i as i32);
                #[rustfmt::skip]
                let m = nalgebra::Matrix3::new(s, 0.0, 0.0, 0.0, s, 0.0, 0.0, 0.0, 1.0);
                Some(FrameAlignment::new(Affine2::from_matrix_unchecked(m), 1.0))
            })
            .collect();
        StackGeometry { transforms, crop: None, reversed, left_out: Vec::new() }
    }

    #[test]
    fn a_stray_first_frame_is_flagged() {
        let mut shapes: Vec<StackShape> =
            (0..6).map(|_| StackShape::of(&geometry(10, false, 0.999))).collect();
        assert!(flag_stacks(&shapes).iter().all(|f| !f));
        // Its reverse check tripped.
        shapes[2] = StackShape::of(&geometry(10, true, 0.999));
        // Or its first step is the whole bracket.
        shapes[4].first_step = Some(0.95);
        assert_eq!(flag_stacks(&shapes), [false, false, true, false, true, false]);
    }

    #[test]
    fn flagged_stacks_are_restacked_and_judged() {
        let words = Words::english(crate::Platform::Desktop);
        let ok = StackShape::of(&geometry(10, false, 0.999));
        let off = StackShape { first_step: Some(0.95), ..ok };
        let mut stacks = vec![(ok, 10); 6];
        stacks[1].0 = off;
        // Too few frames to lose one.
        stacks[4] = (off, 2);
        let mut fix = StackFix::new(stacks);
        assert_eq!(fix.next_to_fix(), Some(1));
        assert_eq!(fix.next_to_fix(), None);
        assert!(
            matches!(fix.judge(&words, 1, "s002", Ok(off)), Fixed::Dropped(g) if g.what == "s002")
        );
        let Fixed::Dropped(gap) = fix.judge(&words, 1, "s002", Err("The GPU ran out of memory."))
        else {
            panic!()
        };
        assert_eq!(
            gap.reason,
            "it couldn't be stacked again without its first frame. The GPU ran out of memory."
        );
        let Fixed::Kept { note } = fix.judge(&words, 1, "s002", Ok(ok)) else { panic!() };
        assert_eq!(note, "s002 restacked without its first frame");
    }
}
