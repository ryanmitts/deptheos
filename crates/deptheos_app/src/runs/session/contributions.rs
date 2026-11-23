//! Each frame's share of a finished stack's result, from its frame map.
//!
//! Each pixel's plane gives, through the blend's ramps
//! ([`deptheos_core::PlaneRamp`]), weights summing to one between two
//! stacked neighbors; a frame's share is its mean weight.
//!
//! One pass builds a histogram of the 65 536 values, a few ms at 45 MP, so
//! it is counted on load (`load_stack`) or stack rather than stored.

use std::sync::{Arc, OnceLock};

use deptheos_core::PlaneRamp;
use rayon::prelude::*;

use super::stack::{CompletedStack, Gray16Image, StackGeometry};

/// [`CompletedStack::frame_contributions`], counted on the first ask and
/// kept; the frame map and geometry must be final by then.
#[derive(Clone, Debug, Default)]
pub struct Contributions(OnceLock<Option<Arc<[f32]>>>);

impl CompletedStack {
    /// Each frame's share, 0 to 1, parallel to [`Self::paths`], summing to
    /// 1 (a left-out frame's is 0). `None` without a frame map. Counted on
    /// the first call and kept.
    pub fn frame_contributions(&self) -> Option<Arc<[f32]>> {
        self.contributions
            .0
            .get_or_init(|| {
                let map = self.frame_map.as_ref()?;
                let shares = count(map, &self.geometry, self.paths.len());
                tracing::debug!("frame contributions counted for {} frames", shares.len());
                Some(shares)
            })
            .clone()
    }
}

/// How many pixels hold each 16-bit value of `map`.
fn histogram(map: &Gray16Image) -> Vec<u64> {
    const CHUNK: usize = 1 << 20;
    map.as_raw()
        .par_chunks(CHUNK)
        .fold(
            || vec![0u32; 1 << 16],
            |mut counts, chunk| {
                for &v in chunk {
                    counts[usize::from(v)] += 1;
                }
                counts
            },
        )
        .map(|counts| counts.into_iter().map(u64::from).collect::<Vec<u64>>())
        .reduce(
            || vec![0u64; 1 << 16],
            |mut a, b| {
                a.iter_mut().zip(b).for_each(|(a, b)| *a += b);
                a
            },
        )
}

/// The shares of `total` frames (indexes into the paths) on `map`.
fn count(map: &Gray16Image, geometry: &StackGeometry, total: usize) -> Arc<[f32]> {
    let mut shares = vec![0f64; total];
    let pixels = map.as_raw().len();
    if total == 0 || pixels == 0 {
        return shares.into_iter().map(|s| s as f32).collect();
    }
    // In accumulation order, which the planes follow.
    let stacked: Vec<bool> =
        (0..total).map(|k| !geometry.is_left_out(geometry.accumulated(k))).collect();
    let ramps: Vec<PlaneRamp> = (0..total)
        .map(|k| PlaneRamp::new(k as u32, total as u32, |j| stacked[j as usize]))
        .collect();
    // Each accumulation index's nearest stacked one at or below, and above.
    let nearest = |order: &mut dyn Iterator<Item = usize>| {
        let mut near = vec![None; total];
        let mut last = None;
        for k in order {
            last = Some(k).filter(|&k| stacked[k]).or(last);
            near[k] = last;
        }
        near
    };
    let below = nearest(&mut (0..total));
    let above = nearest(&mut (0..total).rev());
    let steps = (total.max(2) - 1) as f32;
    for (value, &n) in histogram(map).iter().enumerate() {
        if n == 0 {
            continue;
        }
        let z = value as f32 / 65535.0;
        // Only the nearest stacked frame on either side of `z` reaches it.
        let k = (z * steps).clamp(0.0, steps);
        let (floor, ceil) =
            ((k.floor() as usize).min(total - 1), (k.ceil() as usize).min(total - 1));
        let near = [below[floor], above[ceil]];
        for (i, frame) in near.into_iter().enumerate() {
            let Some(frame) = frame else { continue };
            if i == 1 && near[0] == Some(frame) {
                continue;
            }
            let w = ramps[frame].weight(z);
            if w > 0.0 {
                shares[geometry.accumulated(frame)] += f64::from(w) * n as f64;
            }
        }
    }
    shares.into_iter().map(|s| (s / pixels as f64).clamp(0.0, 1.0) as f32).collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::runs::session::LeftOut;

    /// A `values.len()` × 1 stack whose frame map holds `values`.
    fn stack(frames: usize, values: &[u16]) -> CompletedStack {
        let paths = (0..frames).map(|i| PathBuf::from(format!("f{i}.jpg"))).collect();
        let w = values.len() as u32;
        let map = Gray16Image::from_raw(w, 1, values.to_vec()).unwrap();
        CompletedStack {
            frame_map: Some(map),
            ..CompletedStack::srgb(w, 1, vec![0; 3 * values.len()], paths)
        }
    }

    fn close(a: &[f32], b: &[f32]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-3)
    }

    /// The plane of frame `k` of `total`, as the frame map stores it.
    fn plane(k: u32, total: u32) -> u16 {
        (k as f32 / (total - 1) as f32 * 65535.0).round() as u16
    }

    #[test]
    fn each_frame_gets_the_share_of_pixels_it_won() {
        // Four frames; frame 1 wins two pixels, frames 0 and 3 one each.
        let s = stack(4, &[plane(0, 4), plane(1, 4), plane(1, 4), plane(3, 4)]);
        let shares = s.frame_contributions().unwrap();
        assert!(close(&shares, &[0.25, 0.5, 0.0, 0.25]), "{shares:?}");
        // Kept: the same list on the next ask.
        assert!(Arc::ptr_eq(&shares, &s.frame_contributions().unwrap()));
    }

    #[test]
    fn a_value_between_planes_splits_like_the_blend() {
        // Five frames, planes at quarters; 3/8 is between frames 1 and 2.
        let s = stack(5, &[(0.375 * 65535.0) as u16]);
        let shares = s.frame_contributions().unwrap();
        assert!(close(&shares, &[0.0, 0.5, 0.5, 0.0, 0.0]), "{shares:?}");
        assert!((shares.iter().sum::<f32>() - 1.0).abs() < 1e-3);
    }

    #[test]
    fn a_left_out_frame_has_none_and_its_neighbors_share_its_plane() {
        let mut s = stack(3, &[plane(1, 3), plane(0, 3)]);
        s.geometry.left_out = vec![LeftOut { index: 1, reason: "blurry".into() }];
        let shares = s.frame_contributions().unwrap();
        // Frame 1's plane is halfway between its stacked neighbors.
        assert!(close(&shares, &[0.75, 0.0, 0.25]), "{shares:?}");
    }

    #[test]
    fn a_reversed_stack_counts_by_the_order_given() {
        // Stacked last to first: plane 0 is the last path.
        let mut s = stack(3, &[plane(0, 3), plane(0, 3), plane(2, 3)]);
        s.geometry.reversed = true;
        let shares = s.frame_contributions().unwrap();
        assert!(close(&shares, &[1.0 / 3.0, 0.0, 2.0 / 3.0]), "{shares:?}");
    }

    #[test]
    fn one_frame_has_all_of_it() {
        let s = stack(1, &[0, 0]);
        assert!(close(&s.frame_contributions().unwrap(), &[1.0]));
    }

    #[test]
    fn nothing_to_count_without_a_frame_map() {
        let s = CompletedStack::srgb(1, 1, vec![0; 3], vec![PathBuf::from("a.jpg")]);
        assert_eq!(s.frame_contributions(), None);
    }

    #[test]
    fn a_large_map_counts_quickly() {
        // 45 MP of a 60-frame sweep, timed for the log.
        let (w, h, frames) = (8192u32, 5500u32, 60usize);
        let map = Gray16Image::from_fn(w, h, |x, _| {
            image::Luma([((x as u64 * 65535) / u64::from(w - 1)) as u16])
        });
        let s = CompletedStack {
            frame_map: Some(map),
            ..CompletedStack::srgb(1, 1, vec![0; 3], vec![PathBuf::new(); frames])
        };
        let start = std::time::Instant::now();
        let shares = s.frame_contributions().unwrap();
        eprintln!("45 MP counted in {:?}", start.elapsed());
        assert_eq!(shares.len(), frames);
        assert!((shares.iter().sum::<f32>() - 1.0).abs() < 1e-3);
    }
}
