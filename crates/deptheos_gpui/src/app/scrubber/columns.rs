//! Where the focus rail draws each frame, in whole device pixels: pure
//! arithmetic from width and count, tested without a window.
//!
//! The [`Density`] is chosen per layout, never mid-drag. Nothing merges
//! away: a [`Density::Columns`] column is as tall as its tallest frame, and
//! the left-out lane, dots and marker are per frame (ADR-149: a short tick
//! can be the only sharp frame for a small feature).

/// How the rail draws its frames at its width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Density {
    /// p ≥ 6 px per frame: a tick half its room wide (at most 3 points),
    /// centered.
    Spaced { tick: u32 },
    /// 3 ≤ p < 6: a tick per frame, a pixel between each.
    Packed { tick: u32 },
    /// p < 3: a gapless column per device pixel, as tall as its tallest
    /// frame, a little lighter.
    Columns,
}

/// Device pixels per frame from which ticks pack; below, columns.
pub(crate) const SPACED_FROM: f32 = 6.0;
pub(crate) const PACKED_FROM: f32 = 3.0;

/// A run of device pixels, `[start, end)` from the rail's left edge.
pub(crate) type Run = (u32, u32);

/// A bar the rail draws: a tick, or in [`Density::Columns`] a column.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Bar {
    pub(crate) x: u32,
    pub(crate) width: u32,
    /// In points, as the frame's height ([`super::RailModel::height`]).
    pub(crate) height: f32,
    /// The frame drawn: in a column its tallest (first of equals), whose
    /// map color it takes.
    pub(crate) frame: usize,
}

/// The rail laid out at one width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Columns {
    /// The rail's width in device pixels.
    pub(crate) width: u32,
    pub(crate) count: usize,
    /// Device pixels per point.
    pub(crate) scale: f32,
    pub(crate) density: Density,
}

impl Columns {
    /// `count` frames across `width` device pixels at `scale` pixels a point.
    pub(crate) fn new(width: u32, count: usize, scale: f32) -> Self {
        let p = width as f32 / count.max(1) as f32;
        let density = if p >= SPACED_FROM {
            let most = (3.0 * scale).round().max(1.0);
            Density::Spaced { tick: (0.5 * p).round().min(most) as u32 }
        } else if p >= PACKED_FROM {
            Density::Packed { tick: p.floor() as u32 - 1 }
        } else {
            Density::Columns
        };
        Self { width, count, scale, density }
    }

    /// Device pixels per frame.
    pub(crate) fn per_frame(&self) -> f32 {
        self.width as f32 / self.count.max(1) as f32
    }

    /// Where frame `i`'s room starts, snapped to a whole pixel.
    pub(crate) fn start(&self, i: usize) -> u32 {
        ((i as f32 * self.per_frame()).round() as u32).min(self.width)
    }

    /// Frame `i`'s room to the next frame's start, at least a pixel.
    pub(crate) fn span(&self, i: usize) -> Run {
        let start = self.start(i).min(self.width.saturating_sub(1));
        (start, self.start(i + 1).max(start + 1))
    }

    /// Frame `i`'s exact center, in device pixels.
    pub(crate) fn center(&self, i: usize) -> f32 {
        (i as f32 + 0.5) * self.per_frame()
    }

    /// The bars to draw, from each frame's height in points.
    pub(crate) fn bars(&self, heights: &[f32]) -> Vec<Bar> {
        let count = self.count.min(heights.len());
        let tick = |tick: u32| {
            (0..count)
                .map(|i| {
                    let (start, end) = self.span(i);
                    let width = tick.min(end - start).max(1);
                    let x = start + (end - start - width) / 2;
                    Bar { x, width, height: heights[i], frame: i }
                })
                .collect()
        };
        match self.density {
            Density::Spaced { tick: t } | Density::Packed { tick: t } => tick(t),
            Density::Columns => {
                let mut columns: Vec<Option<Bar>> = vec![None; self.width as usize];
                for (i, &height) in heights.iter().enumerate().take(count) {
                    let (start, end) = self.span(i);
                    for x in start..end.min(self.width) {
                        let column = &mut columns[x as usize];
                        if column.is_none_or(|c| height > c.height) {
                            *column = Some(Bar { x, width: 1, height, frame: i });
                        }
                    }
                }
                columns.into_iter().flatten().collect()
            }
        }
    }

    /// Runs for the `flagged` frames (left-out lane): each frame's room,
    /// neighbors joined.
    pub(crate) fn lane(&self, flagged: &[bool]) -> Vec<Run> {
        let spans = flagged.iter().enumerate().take(self.count).filter(|(_, f)| **f);
        merge(spans.map(|(i, _)| self.span(i)), true)
    }

    /// Dots of `diameter` at the `flagged` frames' centers, overlaps joined
    /// into capsules.
    pub(crate) fn dots(&self, flagged: &[bool], diameter: u32) -> Vec<Run> {
        let diameter = diameter.max(1);
        let dots = flagged.iter().enumerate().take(self.count).filter(|(_, f)| **f);
        let dots = dots.map(|(i, _)| self.around(i, diameter));
        merge(dots, false)
    }

    /// A run `width` pixels wide centered on frame `i`, within the rail.
    pub(crate) fn around(&self, i: usize, width: u32) -> Run {
        let width = width.min(self.width).max(1);
        let x = (self.center(i) - width as f32 / 2.0).round().max(0.0) as u32;
        let x = x.min(self.width.saturating_sub(width));
        (x, x + width)
    }

    /// The current-frame marker's width: its tick's, at least two points.
    pub(crate) fn marker_width(&self) -> u32 {
        let least = (2.0 * self.scale).round() as u32;
        match self.density {
            Density::Spaced { tick } | Density::Packed { tick } => tick.max(least),
            Density::Columns => least,
        }
        .max(1)
    }
}

/// `runs` in order joined where they overlap, or also where they touch.
fn merge(runs: impl Iterator<Item = Run>, touching: bool) -> Vec<Run> {
    let mut merged: Vec<Run> = Vec::new();
    for (start, end) in runs {
        match merged.last_mut() {
            Some(last) if start < last.1 || (touching && start == last.1) => {
                last.1 = last.1.max(end)
            }
            _ => merged.push((start, end)),
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_density_follows_the_pixels_per_frame() {
        // 600 px: 100 frames get 6 px, 101 under 6.
        assert_eq!(Columns::new(600, 100, 2.0).density, Density::Spaced { tick: 3 });
        assert_eq!(Columns::new(600, 101, 2.0).density, Density::Packed { tick: 4 });
        // 3 px a frame packs, under it columns.
        assert_eq!(Columns::new(300, 100, 2.0).density, Density::Packed { tick: 2 });
        assert_eq!(Columns::new(300, 101, 2.0).density, Density::Columns);
        // The spaced tick is half the room, at most 3 points.
        assert_eq!(Columns::new(1000, 100, 2.0).density, Density::Spaced { tick: 5 });
        assert_eq!(Columns::new(2000, 100, 2.0).density, Density::Spaced { tick: 6 });
        assert_eq!(Columns::new(2000, 100, 1.0).density, Density::Spaced { tick: 3 });
    }

    #[test]
    fn frames_start_on_whole_pixels_and_cover_the_rail() {
        let c = Columns::new(100, 7, 1.0);
        assert_eq!(c.start(0), 0);
        assert_eq!(c.start(1), 14, "14.29 snaps down");
        assert_eq!(c.start(3), 43, "42.86 snaps up");
        assert_eq!(c.start(7), 100);
        // Each frame's room starts where the last one's ends.
        for i in 0..6 {
            assert_eq!(c.span(i).1, c.span(i + 1).0);
        }
        // Denser than a pixel, every frame still has one.
        let dense = Columns::new(100, 400, 1.0);
        assert!((0..400).all(|i| dense.span(i).1 > dense.span(i).0));
        assert_eq!(dense.span(399), (99, 100));
    }

    #[test]
    fn ticks_are_centered_in_their_room_with_a_gap() {
        let spaced = Columns::new(100, 10, 2.0).bars(&[4.0; 10]);
        assert_eq!(spaced.len(), 10);
        assert_eq!((spaced[0].x, spaced[0].width), (2, 5), "half of 10, centered");
        assert_eq!(spaced[9].x, 92);
        let packed = Columns::new(45, 10, 2.0);
        assert_eq!(packed.density, Density::Packed { tick: 3 });
        let bars = packed.bars(&[4.0; 10]);
        for pair in bars.windows(2) {
            assert!(pair[0].x + pair[0].width < pair[1].x, "a pixel between: {pair:?}");
        }
    }

    #[test]
    fn a_column_is_as_tall_as_its_tallest_frame() {
        // Four frames a pixel: each column takes its tallest, never an average.
        let mut heights = vec![4.0; 8];
        heights[5] = 16.0;
        heights[1] = 9.0;
        let c = Columns::new(2, 8, 1.0);
        assert_eq!(c.density, Density::Columns);
        let bars = c.bars(&heights);
        assert_eq!(bars.len(), 2, "a column per pixel");
        assert_eq!((bars[0].height, bars[0].frame), (9.0, 1));
        assert_eq!((bars[1].height, bars[1].frame), (16.0, 5));
        // Two pixels a frame: each frame fills both of its columns.
        let wide = Columns::new(8, 4, 1.0).bars(&[4.0, 16.0, 4.0, 8.0]);
        assert_eq!(wide.len(), 8);
        assert_eq!(wide.iter().map(|b| b.frame).collect::<Vec<_>>(), [0, 0, 1, 1, 2, 2, 3, 3]);
    }

    #[test]
    fn left_out_frames_keep_a_pixel_each_and_join_their_neighbors() {
        let mut out = vec![false; 400];
        out[10] = true;
        out[200] = true;
        out[201] = true;
        out[399] = true;
        let c = Columns::new(100, 400, 1.0);
        assert_eq!(c.lane(&out), [(3, 4), (50, 51), (99, 100)], "a pixel each, the pair joined");
        let spaced = Columns::new(100, 10, 1.0);
        let mut out = vec![false; 10];
        out[3] = true;
        out[4] = true;
        out[7] = true;
        assert_eq!(spaced.lane(&out), [(30, 50), (70, 80)]);
    }

    #[test]
    fn dots_that_overlap_become_one_capsule() {
        let c = Columns::new(100, 50, 1.0);
        let mut painted = vec![false; 50];
        painted[10] = true;
        painted[11] = true;
        painted[30] = true;
        // 2 px a frame, 3 px dots: 10 and 11 overlap, 30 stands alone.
        assert_eq!(c.dots(&painted, 3), [(20, 25), (60, 63)]);
        // A dot at the end stays on the rail.
        let mut last = vec![false; 50];
        last[49] = true;
        assert_eq!(c.dots(&last, 3), [(97, 100)]);
        // Dots that only touch stay apart.
        let spaced = Columns::new(60, 10, 1.0);
        let mut both = vec![false; 10];
        both[0] = true;
        both[1] = true;
        assert_eq!(spaced.dots(&both, 6), [(0, 6), (6, 12)]);
    }

    #[test]
    fn the_marker_is_centered_on_its_frame() {
        let c = Columns::new(1000, 400, 2.0);
        assert_eq!(c.marker_width(), 4, "two points");
        assert_eq!(c.around(200, 4), (499, 503), "frame 200's center is 501.25");
        assert_eq!(c.around(0, 4), (0, 4), "kept on the rail");
    }
}
