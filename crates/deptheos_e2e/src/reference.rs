//! A picture against the one signed off for its case.

use std::path::Path;

use rayon::prelude::*;

use crate::frames::{TILE, sobel, tile_means};
use crate::paint::CHANGED;
use crate::read::{Small, read};
use crate::{REFERENCE_PSNR, Report, SMALL_PSNR};

/// Against a reference (a 16-bit PNG at the measuring scale), which
/// `bless` writes only when every check before passed; none yet fails.
/// A difference is reported by size, PSNR, extent, sharpness and color.
pub(crate) fn compare_reference(report: &mut Report, small: &Small, reference: &Path, bless: bool) {
    if bless {
        // A picture that failed a check can't become the reference.
        if !report.passed() {
            report.check("reference", false, "not signed off: a check above failed");
            return;
        }
        let written = write_small(small, reference);
        report.check(
            "reference",
            written.is_ok(),
            match written {
                Ok(()) => format!("signed off ({})", reference.display()),
                Err(e) => e,
            },
        );
        return;
    }
    if !reference.exists() {
        // Until `just bless`, the case fails.
        report.check(
            "reference",
            false,
            format!(
                "not signed off yet: look at this export (`just e2e-review` gathers a run's), then sign it off with `just bless` and the case's name ({} is written then)",
                reference.display()
            ),
        );
        return;
    }
    let theirs = match read(reference, 1) {
        Ok(r) => r.small,
        Err(e) => {
            report.check("reference", false, e);
            return;
        }
    };
    if (theirs.w, theirs.h) != (small.w, small.h) {
        report.check(
            "reference",
            false,
            format!(
                "large change: {} × {} here, {} × {} signed off (at the measuring scale), the crop changed",
                small.w, small.h, theirs.w, theirs.h
            ),
        );
        return;
    }
    let mse: f64 = small
        .px
        .par_iter()
        .zip(&theirs.px)
        .map(|(a, b)| (0..3).map(|c| f64::from(a[c] - b[c]).powi(2)).sum::<f64>())
        .sum::<f64>()
        / (small.px.len() * 3) as f64;
    let psnr = if mse == 0.0 { f64::INFINITY } else { 10.0 * (1.0 / mse).log10() };
    let ok = psnr >= REFERENCE_PSNR;
    if psnr.is_infinite() {
        report.check("reference", true, "identical to the one signed off");
        return;
    }
    let change = Change::of(small, &theirs);
    let size = if ok {
        "trivial"
    } else if psnr >= SMALL_PSNR {
        "small"
    } else {
        "large"
    };
    let tail = if ok { "" } else { "; if the change is meant, bless it" };
    report.check(
        "reference",
        ok,
        format!(
            "{size} change: PSNR {psnr:.1} dB against the one signed off (at least {REFERENCE_PSNR}), {change}{tail}"
        ),
    );
}

/// How a picture differs from its reference, of the same size.
struct Change {
    /// The share of pixels off by more than [`CHANGED`] in some channel.
    changed: f32,
    /// Where they are, as a share of the picture: (x0, y0)–(x1, y1).
    span: Option<[f32; 4]>,
    /// The median sharpness ratio over the reference's detailed tiles.
    sharpness: f32,
    /// Each channel's mean against the reference's.
    color: [f32; 3],
}

impl Change {
    fn of(ours: &Small, theirs: &Small) -> Change {
        let (w, h) = (ours.w, ours.h);
        let mut count = 0usize;
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
        let mut color = [0f64; 3];
        for y in 0..h {
            for x in 0..w {
                let (a, b) = (ours.px[y * w + x], theirs.px[y * w + x]);
                for c in 0..3 {
                    color[c] += f64::from(a[c] - b[c]);
                }
                if (0..3).any(|c| (a[c] - b[c]).abs() > CHANGED) {
                    count += 1;
                    (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
                }
            }
        }
        let n = (w * h).max(1);
        let span = (count > 0).then(|| {
            [x0 as f32 / w as f32, y0 as f32 / h as f32, x1 as f32 / w as f32, y1 as f32 / h as f32]
        });
        let tiles = |s: &Small| tile_means(&sobel(&s.gray(), w, h), w, w / TILE, h / TILE, (0, 0));
        let (a, b) = (tiles(ours), tiles(theirs));
        // Skip flat tiles, where noise reads as detail.
        let mut sorted: Vec<f32> = b.iter().copied().filter(|v| v.is_finite()).collect();
        sorted.sort_by(f32::total_cmp);
        let p90 = sorted.get(sorted.len() * 9 / 10).copied().unwrap_or(0.0);
        let mut ratios: Vec<f32> = a
            .iter()
            .zip(&b)
            .filter(|(a, b)| a.is_finite() && b.is_finite() && **b > 1e-6 && **b >= 0.2 * p90)
            .map(|(a, b)| a / b)
            .collect();
        ratios.sort_by(f32::total_cmp);
        let sharpness = ratios.get(ratios.len() / 2).copied().unwrap_or(1.0);
        Change {
            changed: count as f32 / n as f32,
            span,
            sharpness,
            color: color.map(|c| (c / n as f64) as f32),
        }
    }
}

impl std::fmt::Display for Change {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{:.1}% of pixels changed", 100.0 * self.changed)?;
        if let Some([x0, y0, x1, y1]) = self.span {
            write!(
                f,
                " within ({:.0}%, {:.0}%)–({:.0}%, {:.0}%) of it",
                100.0 * x0,
                100.0 * y0,
                100.0 * x1,
                100.0 * y1
            )?;
        }
        write!(
            f,
            ", sharpness {:.0}% of the reference's in the median tile, color R {:+.3} G {:+.3} B {:+.3}",
            100.0 * self.sharpness,
            self.color[0],
            self.color[1],
            self.color[2]
        )
    }
}

fn write_small(small: &Small, path: &Path) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let raw: Vec<u16> = small
        .px
        .iter()
        .flat_map(|p| p.map(|v| (v.clamp(0.0, 1.0) * 65535.0).round() as u16))
        .collect();
    let buffer =
        image::ImageBuffer::<image::Rgb<u16>, _>::from_raw(small.w as u32, small.h as u32, raw)
            .ok_or("the reference's size")?;
    buffer.save(path).map_err(|e| format!("{}: {e}", path.display()))
}
