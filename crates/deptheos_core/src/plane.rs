//! Where a frame sits in a stack's frame map ([`FramePlane`]) and its
//! share of each map value ([`PlaneRamp`]): the CPU side of
//! `shaders/plane.wgsl`, which every stacker, Contribution and the depth
//! map's light place frames by.

/// Where a frame sits in a stack's frame map: frames spread evenly over
/// [0, 1] in accumulation order (a single frame at 0), so a map value names
/// a frame and the blend between neighbors. A left-out frame (`skip_frame`)
/// keeps its plane, unused, so later frames keep theirs. Every stacker and
/// the contribution overlay use this one placement.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FramePlane {
    /// The frame's own value.
    pub plane: f32,
    /// The distance between neighboring frames' planes.
    pub step: f32,
}

impl FramePlane {
    /// The plane of frame `index` (accumulation order) in a stack of
    /// `total` frames.
    pub fn new(index: u32, total: u32) -> Self {
        let last = (total.max(2) - 1) as f32;
        Self { plane: index as f32 / last, step: 1.0 / last }
    }
}

/// A stacked frame's share of each frame-map value: a linear ramp from 1
/// at its plane to 0 at the nearest stacked frame's plane either side (one
/// step out where there is none), so across a left-out frame neighbors
/// still sum to one. The blend (`depth.wgsl`) and contribution weigh by it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlaneRamp {
    /// The frame's plane ([`FramePlane::plane`]).
    pub plane: f32,
    /// How far the ramp reaches below the plane, and above it.
    pub below: f32,
    pub above: f32,
}

impl PlaneRamp {
    /// Frame `index`'s ramp (accumulation order) in a `total`-frame stack
    /// whose frames `stacked` says were stacked.
    pub fn new(index: u32, total: u32, stacked: impl Fn(u32) -> bool) -> Self {
        let FramePlane { plane, step } = FramePlane::new(index, total);
        let near = stacked_neighbors(total.max(index + 1), stacked);
        let below = index.checked_sub(1).and_then(|j| near[j as usize][0]).map_or(1, |j| index - j);
        let above = near.get(index as usize + 1).and_then(|n| n[1]).map_or(1, |j| j - index);
        Self { plane, below: below as f32 * step, above: above as f32 * step }
    }

    /// The frame's share at map value `z` (`plane_weight` in `shaders/plane.wgsl`).
    pub fn weight(&self, z: f32) -> f32 {
        let d = z - self.plane;
        let span = if d < 0.0 { self.below } else { self.above };
        (1.0 - d.abs() / span.max(0.00001)).max(0.0)
    }

    /// The frame's share of a 16-bit map value `v`: [`Self::weight`],
    /// snapped to 1 or 0 within the map's precision of its own or a
    /// neighbor's plane, so a neighbor's pixels carry no trace of it. What
    /// Contribution shows (`contribution_weight` in `contribution.wgsl`).
    pub fn share(&self, v: u16) -> f32 {
        let z = f32::from(v) / 65535.0;
        let d = (z - self.plane).abs();
        let span = if z < self.plane { self.below } else { self.above };
        if d <= HALF_LEVEL {
            1.0
        } else if d >= span - HALF_LEVEL {
            0.0
        } else {
            self.weight(z)
        }
    }
}

/// Half a 16-bit frame map's quantization level: how far a stored value
/// can sit from the plane it was quantized from.
pub(crate) const HALF_LEVEL: f32 = 0.5 / 65535.0;

/// Contribution's look: what the frame supplied as it is, the rest as gray
/// at this share of its brightness, so no color is added.
pub const UNLIT: f32 = 0.3;

/// For each of `total` frames, the nearest stacked frame at or below it
/// and at or above it: the one rule [`PlaneRamp`] and depth feathering use.
pub(crate) fn stacked_neighbors(
    total: u32,
    stacked: impl Fn(u32) -> bool,
) -> Vec<[Option<u32>; 2]> {
    let mut near = vec![[None; 2]; total as usize];
    let mut below = None;
    for i in 0..total {
        below = Some(i).filter(|&i| stacked(i)).or(below);
        near[i as usize][0] = below;
    }
    let mut above = None;
    for i in (0..total).rev() {
        above = Some(i).filter(|&i| stacked(i)).or(above);
        near[i as usize][1] = above;
    }
    near
}

#[cfg(test)]
mod tests {
    use super::{PlaneRamp, stacked_neighbors};

    /// The middle frame of three: whole at its own plane, none at either
    /// neighbor's, half between; a value one level off a plane snaps to it.
    #[test]
    fn a_share_is_the_ramp_snapped_to_the_map_s_precision() {
        let ramp = PlaneRamp::new(1, 3, |_| true);
        let own = (0.5f32 * 65535.0).round() as u16;
        assert_eq!(ramp.share(own), 1.0);
        assert_eq!(ramp.share(0), 0.0);
        assert_eq!(ramp.share(u16::MAX), 0.0);
        assert!((ramp.share(own / 2) - 0.5).abs() < 0.001);
        assert!(ramp.share(own + 1) < 1.0 && ramp.share(own + 1) > 0.99);
        // 1/3 is not a 16-bit value: the stored plane still has a whole share.
        let third = PlaneRamp::new(1, 4, |_| true);
        assert_eq!(third.share((65535.0f32 / 3.0).round() as u16), 1.0);
    }

    /// Frames 1 and 2 left out: each finds frame 0 below and 3 above, and
    /// the ends find nothing past them.
    #[test]
    fn stacked_neighbors_skip_left_out_frames() {
        let near = stacked_neighbors(5, |i| i != 1 && i != 2);
        assert_eq!(
            near,
            [
                [Some(0), Some(0)],
                [Some(0), Some(3)],
                [Some(0), Some(3)],
                [Some(3), Some(3)],
                [Some(4), Some(4)],
            ]
        );
        assert_eq!(stacked_neighbors(2, |i| i == 1), [[None, Some(1)], [Some(1), Some(1)]]);
    }
}
