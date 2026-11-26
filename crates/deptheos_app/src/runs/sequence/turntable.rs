//! Turntable: one stack per angle of a turning object, written full,
//! grown, or both.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use image::math::Rect;

use super::{
    Direction, FrameOptions, MANIFEST_VERSION, Manifest, ManifestFrame, ManifestGap, SequencePhase,
    SequenceProgress, SequenceReport, SequenceWriter, missing_sources, rect_array, replay_order,
};
use deptheos_render::Adjustments;

use crate::engine::Engine;
use crate::engine::look::Look;
use crate::frames::names::file_label;
use crate::library::export_tags::{SequenceFrame, StackTags};
use crate::runs::session::{CompletedStack, SessionEvent, Snapshot, SnapshotPlan, StackGeometry};
use crate::words::{Words, say};
use crate::{Error, Result};

// --- Turntable.

/// What a turntable sequence shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TurntableMode {
    /// The object turns and comes into focus at once.
    #[default]
    Grow,
    /// The finished object turns: one render per angle.
    Full,
    Both,
}

impl TurntableMode {
    pub const ALL: [Self; 3] = [Self::Grow, Self::Full, Self::Both];

    pub fn grow(self) -> bool {
        matches!(self, Self::Grow | Self::Both)
    }

    pub fn full(self) -> bool {
        matches!(self, Self::Full | Self::Both)
    }

    pub fn label(self, words: &Words) -> String {
        match self {
            Self::Grow => say!(words, "turntable-grow"),
            Self::Full => say!(words, "turntable-full"),
            Self::Both => say!(words, "turntable-both"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TurntableOptions {
    pub mode: TurntableMode,
    /// Grow only.
    pub direction: Direction,
    /// Grow only: the frame count; `None` is the frames per stack.
    pub frames_n: Option<usize>,
    pub frames: FrameOptions,
}

/// One stack of a turntable run: its frames and geometry, without pixels
/// (a run's renders don't fit in memory together).
#[derive(Clone, Debug)]
pub struct TurntableStack {
    /// What it is called: its folder's name.
    pub label: String,
    /// Its render in the history, when there is one.
    pub render: Option<String>,
    /// Its included frames, in the order stacked (before any reversal).
    pub paths: Vec<PathBuf>,
    pub geometry: StackGeometry,
}

impl TurntableStack {
    /// How many frames it stacked, which a grow's depths count.
    pub(crate) fn stacked(&self) -> usize {
        // A render's manifest is checked for this.
        self.paths.len().saturating_sub(self.geometry.left_out.len())
    }
}

/// Where a turntable run's renders are read back from, one at a time: the
/// store may let go of the last one's pixels when asked for the next.
pub trait StackStore {
    /// Stack `index` whole, for the full sequence.
    fn load(&mut self, index: usize) -> Result<&CompletedStack>;
    /// Stack `index` for a grow's replay: settings, color science and
    /// geometry; its pixels and map may be empty.
    fn load_settings(&mut self, index: usize) -> Result<&CompletedStack>;
}

/// The frames per stack: the most common stack length.
pub fn frames_per_stack(lengths: &[usize]) -> usize {
    let mut counts = std::collections::BTreeMap::new();
    for &n in lengths {
        *counts.entry(n).or_insert(0usize) += 1;
    }
    counts.into_iter().max_by_key(|&(n, c)| (c, n)).map_or(0, |(n, _)| n)
}

/// How many frames a turntable of `mode` writes: the grow's `frames_n`
/// (else [`frames_per_stack`]) plus one per stack for the full sequence.
pub fn turntable_frame_count(
    mode: TurntableMode,
    frames_n: Option<usize>,
    lengths: &[usize],
) -> usize {
    let mut count = 0;
    if mode.grow() {
        count += frames_n.unwrap_or_else(|| frames_per_stack(lengths));
    }
    if mode.full() {
        count += lengths.len();
    }
    count
}

/// The bytes a kept render's pixels take: three 16-bit channels, raw.
pub const KEPT_BYTES_PER_PIXEL: u64 = 6;

/// What a turntable run writes, estimated ([`turntable_bytes`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TurntableBytes {
    /// The sequence's frames ([`turntable_frame_count`]).
    pub frames: usize,
    /// The sequence's files.
    pub sequence: u64,
    /// The renders' pixels kept for the full sequence; none for grow only.
    pub renders: u64,
}

impl TurntableBytes {
    pub fn total(&self) -> u64 {
        self.sequence + self.renders
    }
}

/// What a turntable run writes, each render `width`×`height`.
pub fn turntable_bytes(
    options: &TurntableOptions,
    lengths: &[usize],
    width: u32,
    height: u32,
) -> TurntableBytes {
    let frames = turntable_frame_count(options.mode, options.frames_n, lengths);
    let sixteen_bit = options.frames.save.sixteen_bit;
    let sequence = super::estimate_bytes(frames, width, height, &options.frames, sixteen_bit);
    let renders = match options.mode.full() {
        true => lengths.len() as u64 * u64::from(width) * u64::from(height) * KEPT_BYTES_PER_PIXEL,
        false => 0,
    };
    TurntableBytes { frames, sequence, renders }
}

/// [`turntable_bytes`] at the first frame's header size; `None` when it
/// can't be read.
pub fn estimate_turntable(
    jobs: &[crate::runs::batch::Job],
    options: &TurntableOptions,
) -> Option<TurntableBytes> {
    let first = jobs.first().and_then(|j| j.frames.first())?;
    let header = crate::header_info(first, crate::CameraRaw::LibRaw)?;
    let lengths: Vec<usize> = jobs.iter().map(|j| j.frames.len()).collect();
    Some(turntable_bytes(options, &lengths, header.width, header.height))
}

/// Refuse a turntable run whose sequence and kept renders won't fit in
/// `folder` ([`crate::check_space`], [`estimate_turntable`]). `None` when
/// the size can't be read: the run isn't checked.
pub fn check_turntable_space(
    folder: &Path,
    jobs: &[crate::runs::batch::Job],
    options: &TurntableOptions,
) -> Result<Option<TurntableBytes>> {
    let Some(bytes) = estimate_turntable(jobs, options) else { return Ok(None) };
    crate::check_space(folder, bytes.total())?;
    Ok(Some(bytes))
}

/// Which partial stack each frame of a turntable grow of `n` frames is,
/// over stacks of `lengths` frames, `frames_per_stack` frames to a full
/// stack: frame *i* (from 0) is stack *s* = round(*i* (*S* − 1) / (*N* − 1))
/// (from 0) at depth *k* = round((*i* + 1) *F* / *N*), kept between 1 and
/// that stack's length. With *N* = *F*, *k* = *i* + 1.
pub(crate) fn turntable_grow_frames(
    lengths: &[usize],
    n: usize,
    frames_per_stack: usize,
) -> Vec<(usize, usize)> {
    let stacks = lengths.len();
    if stacks == 0 || n == 0 {
        return Vec::new();
    }
    (0..n)
        .map(|i| {
            let s = if n == 1 {
                0
            } else {
                ((i * (stacks - 1)) as f64 / (n - 1) as f64).round() as usize
            };
            let k = (((i + 1) * frames_per_stack) as f64 / n as f64).round() as usize;
            (s, k.clamp(1, lengths[s].max(1)))
        })
        .collect()
}

/// Refuse a sequence of `paths` unless each header is the first's size,
/// before writing, as replays and crops would fail partway. An unreadable
/// header is left to fail where it is read.
pub(super) fn check_sizes<'a>(
    paths: impl IntoIterator<Item = &'a PathBuf>,
    raw: deptheos_core::io::CameraRaw,
) -> Result<()> {
    let mut first = None;
    for path in paths {
        let Some(header) = deptheos_core::io::header_info(path, raw) else { continue };
        let size = (header.width, header.height);
        match first {
            None => first = Some(size),
            Some(first) if size != first => {
                return Err(Error::FrameSize { file: file_label(path), size, first });
            }
            Some(_) => {}
        }
    }
    Ok(())
}

/// The intersection of every stack's crop (`None` is the whole frame);
/// `None` when every crop is.
pub(crate) fn common_crop(crops: &[Option<Rect>]) -> Option<Rect> {
    let mut common: Option<(u32, u32, u32, u32)> = None;
    for r in crops.iter().flatten() {
        let (x0, y0, x1, y1) = (r.x, r.y, r.x + r.width, r.y + r.height);
        common = Some(match common {
            None => (x0, y0, x1, y1),
            Some((a, b, c, d)) => (a.max(x0), b.max(y0), c.min(x1), d.min(y1)),
        });
    }
    common.map(|(x0, y0, x1, y1)| Rect {
        x: x0,
        y: y0,
        width: x1.saturating_sub(x0),
        height: y1.saturating_sub(y0),
    })
}

/// `pixels` of a result cropped at `own`, cut to `region` (both in
/// full-frame coordinates, `region` inside `own`).
pub(crate) fn cut(pixels: &[u16], width: u32, own: Option<Rect>, region: Rect) -> Result<Vec<u16>> {
    let (ox, oy) = own.map_or((0, 0), |r| (r.x, r.y));
    let (dx, dy) = (region.x.checked_sub(ox), region.y.checked_sub(oy));
    let (Some(dx), Some(dy)) = (dx, dy) else {
        return Err(Error::InvalidState("the common crop is outside a stack's crop".into()));
    };
    let height = pixels.len() as u32 / (3 * width.max(1));
    if dx + region.width > width || dy + region.height > height {
        return Err(Error::InvalidState("the common crop is outside a stack's crop".into()));
    }
    let mut out = Vec::with_capacity(region.width as usize * region.height as usize * 3);
    for y in dy..dy + region.height {
        let start = (y as usize * width as usize + dx as usize) * 3;
        out.extend_from_slice(&pixels[start..start + region.width as usize * 3]);
    }
    Ok(out)
}

/// The turntable sequences of `stacks`, into new folders `<label> grow` and
/// `<label> full` in `parent`. Full cuts each render to the shared crop;
/// grow replays each stack and writes snapshots. Cancel keeps what is
/// written.
#[allow(clippy::too_many_arguments)]
pub fn export_turntable(
    engine: &Engine,
    stacks: &[TurntableStack],
    store: &mut dyn StackStore,
    label: &str,
    parent: &Path,
    options: &TurntableOptions,
    skipped: &[ManifestGap],
    notes: &[String],
    progress: &mut dyn FnMut(&SequenceProgress),
    cancel: &deptheos_core::Cancel,
) -> Result<Vec<SequenceReport>> {
    if stacks.is_empty() {
        return Err(Error::NoStacks);
    }
    // A stack's frames are one size: its reference stands for it.
    let raw = store.load_settings(0)?.options.raw_backend.camera_raw();
    let references = stacks.iter().filter_map(|s| s.geometry.reference().map(|i| &s.paths[i]));
    check_sizes(references, raw)?;
    let crop = common_crop(&stacks.iter().map(|s| s.geometry.crop).collect::<Vec<_>>());
    if crop.is_some_and(|r| r.width == 0 || r.height == 0) {
        return Err(Error::NoCommonArea);
    }
    let base = |kind: &str| Manifest {
        version: MANIFEST_VERSION,
        kind: kind.into(),
        crop: crop.map(rect_array),
        skipped: skipped.to_vec(),
        notes: notes.to_vec(),
        ..Manifest::default()
    };
    let run = TurntableRun { engine, stacks, crop, options, cancel };
    let mut reports = Vec::new();

    if options.mode.full() {
        let total = stacks.len();
        let name = format!("{label} full");
        let manifest = base("turntable full");
        let mut writer =
            SequenceWriter::create(engine, parent, &name, total, options.frames, manifest)?;
        let ran = run.full(&mut writer, store, progress);
        reports.push(writer.close(ran)?);
    }

    if options.mode.grow() {
        let lengths: Vec<usize> = stacks.iter().map(TurntableStack::stacked).collect();
        let per_stack = frames_per_stack(&lengths);
        let n = options.frames_n.unwrap_or(per_stack).max(1);
        let mapping = turntable_grow_frames(&lengths, n, per_stack);
        let manifest = Manifest { direction: Some(options.direction), ..base("turntable grow") };
        let name = format!("{label} grow");
        let mut writer =
            SequenceWriter::create(engine, parent, &name, n, options.frames, manifest)?;
        let ran = run.grow(&mut writer, store, progress, &mapping, n);
        reports.push(writer.close(ran)?);
    }
    Ok(reports)
}

/// What [`export_turntable`]'s two sequences are written from.
struct TurntableRun<'a> {
    engine: &'a Engine,
    stacks: &'a [TurntableStack],
    /// The crop every stack shares ([`common_crop`]), `None` where none is cropped.
    crop: Option<Rect>,
    options: &'a TurntableOptions,
    cancel: &'a deptheos_core::Cancel,
}

impl TurntableRun<'_> {
    /// The full sequence: each stack's render, cut to the shared crop.
    fn full(
        &self,
        writer: &mut SequenceWriter,
        store: &mut dyn StackStore,
        progress: &mut dyn FnMut(&SequenceProgress),
    ) -> Result<()> {
        let TurntableRun { stacks, crop, cancel, .. } = *self;
        let total = stacks.len();
        for (i, entry) in stacks.iter().enumerate() {
            crate::runs::session::check_cancel(cancel)?;
            progress(&SequenceProgress {
                phase: SequencePhase::TurntableFull,
                stack: Some((i + 1, total)),
                written: writer.written(),
                total,
            });
            let stack = store.load(i)?;
            let (pixels, w, h) = match crop {
                Some(region) => (
                    Cow::Owned(cut(&stack.pixels, stack.width, stack.geometry.crop, region)?),
                    region.width,
                    region.height,
                ),
                None => (Cow::Borrowed(stack.pixels.as_slice()), stack.width, stack.height),
            };
            let look = neutral(stack);
            let frame = writer.finish(pixels, w, h, look)?;
            let n = entry.stacked();
            let frame_no = SequenceFrame { k: n, of: n, stack: Some(i + 1) };
            let tags = StackTags::read(look).metadata(Some(frame_no));
            writer.write(&frame, &tags, |file| ManifestFrame {
                file,
                stack: Some(i + 1),
                folder: Some(entry.label.clone()),
                k: n,
                of: n,
                kind: "full",
                render: entry.render.clone(),
                from: None,
                to: None,
            })?;
        }
        progress(&SequenceProgress {
            phase: SequencePhase::TurntableFull,
            stack: None,
            written: writer.written(),
            total,
        });
        Ok(())
    }

    /// The grow sequence: each stack replayed, a snapshot at each depth
    /// `mapping` asks, `n` in all.
    fn grow(
        &self,
        writer: &mut SequenceWriter,
        store: &mut dyn StackStore,
        progress: &mut dyn FnMut(&SequenceProgress),
        mapping: &[(usize, usize)],
        n: usize,
    ) -> Result<()> {
        let TurntableRun { engine, stacks, crop, options, cancel } = *self;
        for (s, entry) in stacks.iter().enumerate() {
            let wanted: Vec<usize> =
                mapping.iter().filter(|(stack, _)| *stack == s).map(|&(_, k)| k).collect();
            let Some(&deepest) = wanted.iter().max() else { continue };
            crate::runs::session::check_cancel(cancel)?;
            let stack = store.load_settings(s)?;
            let replay =
                replay_order(&entry.paths, &entry.geometry, options.direction).first(deepest);
            let missing = missing_sources(&replay.paths);
            if !missing.is_empty() {
                return Err(Error::MissingFrames {
                    stack: Some(entry.label.clone()),
                    files: missing,
                });
            }
            let base_tags = StackTags::read(neutral(stack));
            // The replay's frames, which its depths and snapshots count.
            let of = entry.stacked();
            let plan = SnapshotPlan {
                after: wanted.iter().map(|&k| k as u32).collect(),
                crop: crop.or(entry.geometry.crop),
            };
            progress(&SequenceProgress {
                phase: SequencePhase::TurntableGrow,
                stack: Some((s + 1, stacks.len())),
                written: writer.written(),
                total: n,
            });
            let replayed = engine
                .session(replay.paths.clone(), stack.config, stack.options)?
                .with_alignment(replay.geometry.clone())?;
            let mut on_snapshot = |shot: Snapshot| {
                // By the frame it was taken after: a left-out one keeps its place.
                let k = shot.frame as usize;
                let frame = writer.finish(
                    Cow::Owned(shot.pixels),
                    shot.width,
                    shot.height,
                    neutral(stack),
                )?;
                // A depth asked twice (a short stack's cap) is written twice.
                let repeats = wanted.iter().filter(|&&w| w == k).count();
                for _ in 0..repeats {
                    let frame_no = SequenceFrame { k, of, stack: Some(s + 1) };
                    let tags = base_tags.metadata(Some(frame_no));
                    writer.write(&frame, &tags, |file| ManifestFrame {
                        file,
                        stack: Some(s + 1),
                        folder: Some(entry.label.clone()),
                        k,
                        of,
                        kind: if k == of { "full" } else { "partial" },
                        render: entry.render.clone(),
                        from: replay.paths.first().map(|p| file_label(p)),
                        to: replay.paths.get(k - 1).map(|p| file_label(p)),
                    })?;
                    progress(&SequenceProgress {
                        phase: SequencePhase::TurntableGrow,
                        stack: Some((s + 1, stacks.len())),
                        written: writer.written(),
                        total: n,
                    });
                }
                Ok(())
            };
            let run = replayed.run_snapshots(
                &plan,
                &mut on_snapshot,
                &mut |_: SessionEvent, _| {},
                cancel,
            )?;
            for l in run.left_out {
                writer.left_out(ManifestGap {
                    what: format!("{}/{}", entry.label, file_label(&replay.paths[l.index])),
                    reason: l.reason,
                });
            }
        }
        Ok(())
    }
}

/// How a turntable's frames export: its renders come from a Batch run, so
/// they have no adjustments of their own.
fn neutral(stack: &CompletedStack) -> Look<'_> {
    Look::exported(stack, Some(&Adjustments::NEUTRAL))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 50 stacks of 200 (the last 199), N = 200.
    #[test]
    fn the_turntable_mapping_reproduces_scan1() {
        let mut lengths = vec![200; 50];
        lengths[49] = 199;
        let per = frames_per_stack(&lengths);
        assert_eq!(per, 200);
        let frames = turntable_grow_frames(&lengths, 200, per);
        assert_eq!(frames.len(), 200);
        // Stack 1 gives k = 1, 2, 3; stack 2 gives k = 4 to 7.
        assert_eq!(&frames[..8], [(0, 1), (0, 2), (0, 3), (1, 4), (1, 5), (1, 6), (1, 7), (2, 8)]);
        // s = 1 + round(i × 49 / 199), k = i + 1.
        for (i, &(s, k)) in frames.iter().enumerate() {
            assert_eq!(s, ((i * 49) as f64 / 199.0).round() as usize, "frame {i}");
            assert_eq!(k, (i + 1).min(lengths[s]), "frame {i}");
        }
        // The last stack's capped depth repeats, on purpose.
        assert_eq!(&frames[197..], [(49, 198), (49, 199), (49, 199)]);
    }

    /// The estimate: the frames, and the renders kept for the full one.
    #[test]
    fn a_turntable_estimates_its_sequence_and_the_renders_it_keeps() {
        let lengths = [20, 20, 21, 19];
        let (w, h) = (300, 200);
        let mut options = TurntableOptions::default();
        options.frames.format = super::super::FrameFormat::Png;
        options.frames.save.sixteen_bit = false;
        let grow = turntable_bytes(&options, &lengths, w, h);
        assert_eq!(grow, TurntableBytes { frames: 20, sequence: 20 * 300 * 200 * 3, renders: 0 });

        options.mode = TurntableMode::Both;
        options.frames.save.sixteen_bit = true;
        let both = turntable_bytes(&options, &lengths, w, h);
        assert_eq!(both.frames, 24);
        assert_eq!(both.sequence, 24 * 300 * 200 * 6);
        assert_eq!(both.renders, 4 * 300 * 200 * KEPT_BYTES_PER_PIXEL);
        assert_eq!(both.total(), both.sequence + both.renders);
    }

    /// A run whose first frame's size can't be read isn't checked.
    #[test]
    fn a_turntable_of_unreadable_frames_is_not_checked() {
        let dir = tempfile::tempdir().unwrap();
        let jobs = [crate::runs::batch::Job {
            label: "s001".into(),
            frames: vec![dir.path().join("missing.jpg")],
        }];
        let checked = check_turntable_space(dir.path(), &jobs, &TurntableOptions::default());
        assert_eq!(checked.unwrap(), None);
        assert_eq!(
            check_turntable_space(dir.path(), &[], &TurntableOptions::default()).unwrap(),
            None
        );
    }

    /// The grow counts a full stack's frames, not the longest stack's.
    #[test]
    fn a_turntable_counts_its_frames() {
        let lengths = [20, 20, 21, 19];
        assert_eq!(turntable_frame_count(TurntableMode::Grow, None, &lengths), 20);
        assert_eq!(turntable_frame_count(TurntableMode::Grow, Some(50), &lengths), 50);
        assert_eq!(turntable_frame_count(TurntableMode::Full, None, &lengths), 4);
        assert_eq!(turntable_frame_count(TurntableMode::Both, Some(8), &lengths), 12);
    }

    #[test]
    fn the_common_crop_is_every_crop_at_once() {
        let r = |x, y, width, height| Some(Rect { x, y, width, height });
        assert_eq!(common_crop(&[r(10, 5, 100, 50), None, r(12, 3, 100, 60)]), r(12, 5, 98, 50));
        assert_eq!(common_crop(&[None, None]), None);
        let pixels: Vec<u16> = (0..4 * 3 * 3).map(|v| v as u16).collect();
        // A 4×3 result cropped at (10, 20); cut (11, 21) 2×2.
        let own = Some(Rect { x: 10, y: 20, width: 4, height: 3 });
        let out = cut(&pixels, 4, own, Rect { x: 11, y: 21, width: 2, height: 2 }).unwrap();
        assert_eq!(out, [15, 16, 17, 18, 19, 20, 27, 28, 29, 30, 31, 32]);
    }
}
