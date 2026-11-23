//! A retouched export, against the plain one and the frame it was painted from.

use std::path::Path;

use rayon::prelude::*;

use crate::frames::sobel;
use crate::metadata::metadata_checks;
use crate::read::read;
use crate::{Frames, Report, Set};

/// The scale paint is measured at: half the frames' `k`, or 1.
fn paint_scale(k: usize) -> usize {
    if k.is_multiple_of(2) { k / 2 } else { 1 }
}
/// What counts as a pixel paint changed, 0 to 1.
pub(crate) const CHANGED: f32 = 2.0 / 255.0;
/// The most of the picture a test's strokes may change.
const AREA: f32 = 0.10;
/// How far the painted area's sharpness may fall back from its source's
/// towards the stack's, on a log scale.
const SOURCE_SHARPNESS: f32 = 1.0 / 3.0;
/// The most the painted area's mean may be from its source's, 0 to 1.
const SOURCE_COLOR: f32 = 0.04;
/// How far from the stack's place a frame's pixels are searched, in
/// frame pixels.
const SEARCH: isize = 800;
/// The most the strokes' pixels may differ on average from the frame's
/// where found, 0 to 1.
const SOURCE_MATCH: f32 = 0.025;

/// The offset within `reach` of `guess` where `image`'s `pixels` best
/// match `source`, and the mean difference there; coarse, then fine.
fn find_patch(
    image: &[f32],
    w: usize,
    source: &[f32],
    sw: usize,
    pixels: &[usize],
    guess: (isize, isize),
    reach: isize,
) -> Option<((isize, isize), f32)> {
    let sh = source.len() / sw;
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for &i in pixels {
        let (x, y) = (i % w, i / w);
        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
    }
    let fits = |ox: isize, oy: isize| {
        ox + x0 as isize >= 0
            && oy + y0 as isize >= 0
            && ox + (x1 as isize) < sw as isize
            && oy + (y1 as isize) < sh as isize
    };
    let mad = |ox: isize, oy: isize, step: usize| -> f32 {
        let mut sum = 0f32;
        let mut n = 0;
        for &i in pixels.iter().step_by(step) {
            let (x, y) = ((i % w) as isize, (i / w) as isize);
            sum += (image[i] - source[((y + oy) * sw as isize + x + ox) as usize]).abs();
            n += 1;
        }
        sum / n.max(1) as f32
    };
    let best = |offsets: Vec<(isize, isize)>, step: usize| {
        offsets
            .into_par_iter()
            .filter(|&(ox, oy)| fits(ox, oy))
            .map(|(ox, oy)| (mad(ox, oy, step), (ox, oy)))
            .min_by(|a, b| a.0.total_cmp(&b.0))
    };
    let around = |c: (isize, isize), reach: isize, by: usize| -> Vec<(isize, isize)> {
        (-reach..=reach)
            .step_by(by)
            .flat_map(|dy| (-reach..=reach).step_by(by).map(move |dx| (c.0 + dx, c.1 + dy)))
            .collect()
    };
    let (_, coarse) = best(around(guess, reach, 4), 16)?;
    let (d, fine) = best(around(coarse, 4, 1), 1)?;
    Some((fine, d))
}

/// Check `painted` against `plain` (the export before painting) and
/// `source` (the frame painted from): only the strokes changed, and there
/// the picture has the source's sharpness and color.
pub fn check_paint(
    set: &Set,
    frames: &Frames,
    plain: &Path,
    painted: &Path,
    source: &Path,
) -> Report {
    let mut report = Report::new(format!("{} · retouched {}", set.name, painted.display()));
    let painted_path = painted;
    let paint_k = paint_scale(frames.k);
    let reads = (read(plain, paint_k), read(painted, paint_k), read(source, paint_k));
    let (plain, painted, source) = match reads {
        (Ok(a), Ok(b), Ok(c)) => (a, b, c),
        (a, b, c) => {
            for e in [a.err(), b.err(), c.err()].into_iter().flatten() {
                report.check("opens", false, e);
            }
            return report;
        }
    };
    report.check(
        "same size",
        (plain.width, plain.height) == (painted.width, painted.height),
        format!("{} × {}", painted.width, painted.height),
    );
    if (plain.width, plain.height) != (painted.width, painted.height) {
        return report;
    }
    let (w, h) = (painted.small.w, painted.small.h);
    let changed: Vec<bool> = painted
        .small
        .px
        .iter()
        .zip(&plain.small.px)
        .map(|(a, b)| (0..3).any(|c| (a[c] - b[c]).abs() > CHANGED))
        .collect();
    let count = changed.iter().filter(|c| **c).count();
    if count < 1000 {
        report.check("painted", false, format!("{count} pixels changed"));
        return report;
    }
    metadata_checks(&mut report, painted_path, frames);
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for (i, _) in changed.iter().enumerate().filter(|(_, c)| **c) {
        let (x, y) = (i % w, i / w);
        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
    }
    let area = ((x1 - x0 + 1) * (y1 - y0 + 1)) as f32 / (w * h) as f32;
    report.check(
        "in place",
        area <= AREA,
        format!(
            "{count} pixels changed, inside ({}, {})–({}, {}) of the frame, {:.1}% of it (at most {:.0}%)",
            x0 * paint_k,
            y0 * paint_k,
            x1 * paint_k,
            y1 * paint_k,
            100.0 * area,
            100.0 * AREA
        ),
    );

    // Inside the strokes, away from the brush's soft edge.
    let inner: Vec<usize> = (1..h - 1)
        .flat_map(|y| (1..w - 1).map(move |x| (x, y)))
        .filter(|&(x, y)| (0..9).all(|n| changed[(y + n / 3 - 1) * w + x + n % 3 - 1]))
        .map(|(x, y)| y * w + x)
        .collect();
    if inner.len() < 200 {
        report.check("painted", false, format!("only {} pixels inside the strokes", inner.len()));
        return report;
    }

    // Found by search: the aligner moved and scaled each frame onto the
    // stack, so its pixels can be hundreds away from where the stack sits.
    let at = frames.place(&plain.small.shrink(frames.k / paint_k));
    let guess = ((at.0 / paint_k) as isize, (at.1 / paint_k) as isize);
    let sw = source.small.w;
    let (painted_gray, source_gray) = (painted.small.gray(), source.small.gray());
    let Some(((ox, oy), mad)) =
        find_patch(&painted_gray, w, &source_gray, sw, &inner, guess, SEARCH / paint_k as isize)
    else {
        report.check("in source", false, "the strokes' area doesn't fit in the frame");
        return report;
    };
    let source_at = move |i: usize| -> usize {
        (((i / w) as isize + oy) * sw as isize + (i % w) as isize + ox) as usize
    };
    let plain_gray = plain.small.gray();
    let plain_mad =
        inner.iter().map(|&i| (plain_gray[i] - source_gray[source_at(i)]).abs()).sum::<f32>()
            / inner.len() as f32;
    report.check(
        "in source",
        mad <= SOURCE_MATCH && mad < plain_mad / 2.0,
        format!(
            "the strokes' pixels found in the frame painted from, {:+}, {:+} px from where the stack sits, differing by {mad:.3} (at most {SOURCE_MATCH}; the plain stack's there differ by {plain_mad:.3})",
            (ox - guess.0) * paint_k as isize,
            (oy - guess.1) * paint_k as isize,
        ),
    );

    let energy = |g: &[f32], gw: usize, at: &dyn Fn(usize) -> usize| -> f32 {
        let s = sobel(g, gw, g.len() / gw);
        inner.iter().map(|&i| s[at(i)]).sum::<f32>() / inner.len() as f32
    };
    let same = |i: usize| i;
    let e_painted = energy(&painted.small.gray(), w, &same);
    let e_plain = energy(&plain.small.gray(), w, &same);
    let e_source = energy(&source.small.gray(), sw, &source_at);
    // Log scale, 0 at the frame's sharpness, 1 at the stack's: a blurred
    // frame's is near the noise floor, so a ratio to it would mislead.
    let (to_source, to_plain) =
        ((e_painted / e_source.max(1e-12)).ln().abs(), (e_plain / e_source.max(1e-12)).ln().abs());
    let part = if to_plain > 0.0 { to_source / to_plain } else { 0.0 };
    report.check(
        "like source",
        part <= SOURCE_SHARPNESS,
        format!(
            "inside the strokes, sharpness {:.2}× the frame painted from's, the plain stack's {:.1}×: {:.0}% of the way back to the stack's (at most {:.0}%)",
            e_painted / e_source.max(1e-12),
            e_plain / e_source.max(1e-12),
            100.0 * part,
            100.0 * SOURCE_SHARPNESS
        ),
    );

    let mean = |px: &[[f32; 3]], at: &dyn Fn(usize) -> usize| -> [f32; 3] {
        let mut s = [0f32; 3];
        for &i in &inner {
            let p = px[at(i)];
            for c in 0..3 {
                s[c] += p[c];
            }
        }
        s.map(|v| v / inner.len() as f32)
    };
    let (a, b) = (mean(&painted.small.px, &same), mean(&source.small.px, &source_at));
    let worst = (0..3).map(|c| (a[c] - b[c]).abs()).fold(0f32, f32::max);
    report.check(
        "source color",
        worst <= SOURCE_COLOR,
        format!(
            "inside the strokes, R {:+.3} G {:+.3} B {:+.3} from the frame painted from (at most ±{SOURCE_COLOR})",
            a[0] - b[0],
            a[1] - b[1],
            a[2] - b[2]
        ),
    );
    report
}
