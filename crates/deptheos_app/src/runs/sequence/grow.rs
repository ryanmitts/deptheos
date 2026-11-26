//! Grow: a render's frames replayed nearest first or farthest first, a
//! snapshot after each.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::{
    Direction, FrameOptions, MANIFEST_VERSION, Manifest, ManifestFrame, ManifestGap, SequencePhase,
    SequenceProgress, SequenceReport, SequenceWriter, check_sizes, rect_array,
};
use deptheos_render::Adjustments;

use crate::engine::Engine;
use crate::engine::look::Look;
use crate::frames::names::file_label;
use crate::library::export_tags::{SequenceFrame, StackTags};
use crate::runs::session::{
    CompletedStack, LeftOut, SessionEvent, Snapshot, SnapshotPlan, StackGeometry,
};
use crate::runs::stacker::StackerConfig;
use crate::{Error, Result};

// --- Grow.

/// A render's frames in grow replay order with its geometry: unreversed,
/// left-out frames dropped, each with its index in the render's paths.
#[derive(Clone, Debug, PartialEq)]
pub struct Replay {
    pub paths: Vec<PathBuf>,
    pub geometry: StackGeometry,
    pub source: Vec<usize>,
    /// The frames the render left out, by index into its paths.
    pub left_out: Vec<LeftOut>,
}

impl Replay {
    /// The first `k` frames of the replay.
    pub fn first(&self, k: usize) -> Self {
        let k = k.min(self.paths.len());
        Self {
            paths: self.paths[..k].to_vec(),
            geometry: StackGeometry {
                transforms: self.geometry.transforms[..k].to_vec(),
                ..self.geometry.clone()
            },
            source: self.source[..k].to_vec(),
            left_out: self.left_out.clone(),
        }
    }
}

/// `paths` (a render's included frames) and its `geometry`, in
/// `direction`; the render stacked them nearest first.
pub fn replay_order(paths: &[PathBuf], geometry: &StackGeometry, direction: Direction) -> Replay {
    // Nearest first.
    let mut order = geometry.stacked_order();
    if direction == Direction::BackToFront {
        order.reverse();
    }
    Replay {
        paths: order.iter().map(|&i| paths[i].clone()).collect(),
        geometry: StackGeometry {
            transforms: order.iter().map(|&i| geometry.transforms[i]).collect(),
            crop: geometry.crop,
            reversed: false,
            left_out: Vec::new(),
        },
        source: order,
        left_out: geometry.left_out.clone(),
    }
}

/// The depths a grow of `n` frames writes: every `every`-th and the last.
pub fn grow_depths(n: usize, every: usize) -> Vec<usize> {
    let every = every.max(1);
    let mut depths: Vec<usize> = (1..=n).filter(|k| k % every == 0).collect();
    if n > 0 && depths.last() != Some(&n) {
        depths.push(n);
    }
    depths
}

/// The files of `paths` that are not there.
pub fn missing_sources(paths: &[PathBuf]) -> Vec<PathBuf> {
    paths.iter().filter(|p| !p.exists()).cloned().collect()
}

/// What a grow export is.
#[derive(Clone, Copy, Debug, Default)]
pub struct GrowOptions {
    pub direction: Direction,
    /// Write every this many frames; the last is always written.
    pub every: usize,
    pub frames: FrameOptions,
}

/// The grow sequence of pyramid `stack` under `adjustments` into a new
/// folder `<name> grow` in `parent`, one frame per [`grow_depths`] depth,
/// the last the render.
/// Refused when it won't fit ([`crate::check_space`]); Cancel keeps what is
/// written, marked incomplete, and returns [`Error::Canceled`].
#[allow(clippy::too_many_arguments)]
pub fn export_grow(
    engine: &Engine,
    stack: &CompletedStack,
    adjustments: &Adjustments,
    render: Option<&str>,
    name: &str,
    parent: &Path,
    options: &GrowOptions,
    progress: &mut dyn FnMut(&SequenceProgress),
    cancel: &deptheos_core::Cancel,
) -> Result<SequenceReport> {
    if !matches!(stack.config, StackerConfig::Pyramid(_)) {
        return Err(Error::NeedsPyramid);
    }
    let replay = replay_order(&stack.paths, &stack.geometry, options.direction);
    let missing = missing_sources(&replay.paths);
    if !missing.is_empty() {
        return Err(Error::MissingFrames { stack: None, files: missing });
    }
    let n = replay.paths.len();
    let depths = grow_depths(n, options.every);
    let sixteen_bit = options.frames.sixteen_bit(&stack.color);
    let bytes = super::estimate_bytes(
        depths.len(),
        stack.width,
        stack.height,
        &options.frames,
        sixteen_bit,
    );
    crate::check_space(parent, bytes)?;
    check_sizes(&replay.paths, stack.options.raw_backend.camera_raw())?;
    let look = Look::exported(stack, Some(adjustments));
    let base_tags = StackTags::read(look);
    let (out_w, out_h) = options.frames.output_size(stack.width, stack.height);
    let manifest = Manifest {
        version: MANIFEST_VERSION,
        kind: "grow".into(),
        direction: Some(options.direction),
        width: out_w,
        height: out_h,
        crop: stack.geometry.crop.map(rect_array),
        left_out: replay
            .left_out
            .iter()
            .map(|l| ManifestGap {
                what: file_label(&stack.paths[l.index]),
                reason: l.reason.clone(),
            })
            .collect(),
        ..Manifest::default()
    };
    let mut writer = SequenceWriter::create(
        engine,
        parent,
        &format!("{name} grow"),
        depths.len(),
        options.frames,
        manifest,
    )?;
    let file_of = |i: usize| file_label(&replay.paths[i]);
    let mut report = |written: usize| {
        progress(&SequenceProgress {
            phase: SequencePhase::Grow,
            stack: None,
            written,
            total: depths.len(),
        })
    };
    report(0);

    let write = |writer: &mut SequenceWriter, pixels: Cow<[u16]>, (w, h): (u32, u32), k: usize| {
        let frame = writer.finish(pixels, w, h, look)?;
        let tags = base_tags.metadata(Some(SequenceFrame { k, of: n, stack: None }));
        writer.write(&frame, &tags, |file| ManifestFrame {
            file,
            stack: None,
            folder: None,
            k,
            of: n,
            kind: if k == n { "full" } else { "partial" },
            render: render.map(str::to_string),
            from: Some(file_of(0)),
            to: Some(file_of(k - 1)),
        })
    };

    // Every depth but the last from one replay of the first n − 1 frames.
    let partial: BTreeSet<u32> = depths.iter().filter(|&&k| k < n).map(|&k| k as u32).collect();
    let ran = if partial.is_empty() {
        Ok(())
    } else {
        let max = *partial.last().expect("not empty") as usize;
        let part = replay.first(max);
        // A session that can't start ends through `close` like any failure,
        // writing the manifest.
        let session = engine
            .session(part.paths.clone(), stack.config, stack.options)
            .and_then(|session| session.with_alignment(part.geometry.clone()));
        session.and_then(|session| {
            let plan = SnapshotPlan { after: partial, crop: None };
            let mut on_snapshot = |shot: Snapshot| {
                let size = (shot.width, shot.height);
                // By the frame it was taken after: a left-out one keeps its place.
                write(&mut writer, Cow::Owned(shot.pixels), size, shot.frame as usize)?;
                report(writer.written());
                Ok(())
            };
            let run = session.run_snapshots(
                &plan,
                &mut on_snapshot,
                &mut |_: SessionEvent, _| {},
                cancel,
            )?;
            for l in run.left_out {
                writer.left_out(ManifestGap {
                    what: file_label(&part.paths[l.index]),
                    reason: l.reason,
                });
            }
            Ok(())
        })
    };
    // The last frame is the render.
    let ran = ran.and_then(|()| {
        crate::runs::session::check_cancel(cancel)?;
        write(&mut writer, Cow::Borrowed(&stack.pixels), (stack.width, stack.height), n)?;
        report(writer.written());
        Ok(())
    });
    writer.close(ran)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runs::sequence::check::tests::geometry;

    #[test]
    fn replays_start_from_the_chosen_end_without_left_out_frames() {
        let paths: Vec<PathBuf> = (1..=4).map(|i| PathBuf::from(format!("{i}.jpg"))).collect();
        let mut g = geometry(4, false, 0.99);
        g.left_out = vec![LeftOut { index: 1, reason: "unreadable".into() }];
        g.transforms[1] = None;
        let names = |r: &Replay| r.paths.iter().map(|p| file_label(p)).collect::<Vec<_>>();
        let back = replay_order(&paths, &g, Direction::BackToFront);
        assert_eq!(names(&back), ["4.jpg", "3.jpg", "1.jpg"]);
        assert_eq!(back.geometry.transforms, [g.transforms[3], g.transforms[2], g.transforms[0]]);
        assert!(!back.geometry.reversed && back.geometry.left_out.is_empty());
        assert_eq!(
            names(&replay_order(&paths, &g, Direction::FrontToBack)),
            ["1.jpg", "3.jpg", "4.jpg"]
        );
        // A reversed render's nearest frame is its last.
        g.reversed = true;
        assert_eq!(
            names(&replay_order(&paths, &g, Direction::BackToFront)),
            ["1.jpg", "3.jpg", "4.jpg"]
        );
        assert_eq!(names(&back.first(2)), ["4.jpg", "3.jpg"]);
    }

    #[test]
    fn grow_depths_keep_the_last() {
        assert_eq!(grow_depths(5, 1), [1, 2, 3, 4, 5]);
        assert_eq!(grow_depths(7, 3), [3, 6, 7]);
        assert_eq!(grow_depths(1, 4), [1]);
    }
}
