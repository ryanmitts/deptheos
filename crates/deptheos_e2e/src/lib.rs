//! What the end-to-end tests check of an exported file: its format and
//! bits, the camera's details kept and its location dropped, its frames'
//! color space and color, and the picture against its signed-off
//! reference. A retouched export is checked against the plain one and its
//! source frame.
//!
//! The desktop's test (`deptheos_gpui`, `app/tests/e2e.rs`) calls this
//! directly; iOS's exports are checked by `script/e2e-ios.sh` through
//! `e2e-check`.
//!
//! JPEG, PNG and TIFF are read by the `image` crate, independent of the
//! apps; RAW and DNG through the engine, rendered to sRGB by the DNG SDK.
//!
//! A person judges whether a stack is good (`just e2e-review`, then
//! `just bless`); until then a case fails. Later runs must match that
//! picture within a PSNR above the GPU's rounding, so a case passes on any
//! Mac and fails when the picture changes.

pub mod cases;
mod frames;
mod metadata;
mod paint;
mod read;
mod reference;
mod space;
mod tags;
#[cfg(test)]
mod tests;

use std::fmt;
use std::path::Path;

use deptheos_app::library::export::ExportFormat;

pub use frames::{Frames, MANIFEST, Set};
use metadata::metadata_checks;
pub use paint::check_paint;
pub use read::{Read, Small, read};
use reference::compare_reference;
use space::space_check;
use tags::color_tags;

// ---------------------------------------------------------------------
// The report
// ---------------------------------------------------------------------

pub struct Check {
    pub name: &'static str,
    pub ok: bool,
    pub detail: String,
}

/// One file's checks and whether each passed.
pub struct Report {
    pub title: String,
    pub checks: Vec<Check>,
}

impl Report {
    /// A single failed check, for a case that never got to a file.
    pub fn failed(title: impl Into<String>, name: &'static str, detail: impl Into<String>) -> Self {
        let mut report = Self::new(title);
        report.check(name, false, detail);
        report
    }

    fn new(title: impl Into<String>) -> Self {
        Self { title: title.into(), checks: Vec::new() }
    }

    fn check(&mut self, name: &'static str, ok: bool, detail: impl Into<String>) {
        self.checks.push(Check { name, ok, detail: detail.into() });
    }

    pub fn passed(&self) -> bool {
        self.checks.iter().all(|c| c.ok)
    }

    /// Print it, and panic with it unless every check passed.
    pub fn assert_passed(&self) {
        eprintln!("{self}");
        assert!(self.passed(), "{self}");
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let verdict = if self.passed() { "PASS" } else { "FAIL" };
        writeln!(f, "{verdict}  {}", self.title)?;
        for c in &self.checks {
            writeln!(f, "  {} {:<12} {}", if c.ok { "ok  " } else { "FAIL" }, c.name, c.detail)?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------
// A stack's export
// ---------------------------------------------------------------------

/// What an export should be.
pub struct Expect {
    pub format: ExportFormat,
    /// Its bits a channel, from the case ([`cases::Export::bits`]), not
    /// the app.
    pub bits: u8,
}

/// The least share of a frame's width and height a stack keeps, after the
/// aligner's crop (canon1 keeps 96% × 85%); the reference pins the size.
const MIN_SIDE: f32 = 0.70;
/// The most a channel's mean may move from the frames', 0 to 1.
const COLOR: f32 = 0.03;
/// The least PSNR (dB, at the measuring scale) against the reference. The
/// same stack comes to over 100 dB; another method, or a visible change,
/// to about 40.
const REFERENCE_PSNR: f64 = 50.0;
/// Above this but below the reference's PSNR is a small change; below it,
/// one as large as another method.
const SMALL_PSNR: f64 = 44.0;

/// Check `file`, an export of `set`, against `expect` and `frames`, and
/// against `reference` if given, which `bless` writes from this one.
pub fn check_stack(
    set: &Set,
    frames: &Frames,
    file: &Path,
    expect: &Expect,
    reference: Option<&Path>,
    bless: bool,
) -> Report {
    let mut report = Report::new(format!("{} · {}", set.name, file.display()));
    file_checks(&mut report, file, expect);
    metadata_checks(&mut report, file, frames);
    space_check(&mut report, file, expect.format, frames);
    let read = match read(file, frames.k) {
        Ok(r) => r,
        Err(e) => {
            report.check("opens", false, e);
            return report;
        }
    };
    report.check("opens", true, format!("{} × {}", read.width, read.height));
    if expect.format == ExportFormat::Dng {
        color_tags(&mut report, read.color.as_ref(), set, frames);
    }
    report.check(
        "bits",
        read.bits == expect.bits,
        format!("{} per channel, {} asked for", read.bits, expect.bits),
    );
    let (fw, fh) = (frames.width as f32, frames.height as f32);
    let (w, h) = (read.width as f32, read.height as f32);
    report.check(
        "size",
        w <= fw && h <= fh && w >= MIN_SIDE * fw && h >= MIN_SIDE * fh,
        format!(
            "{:.1}% × {:.1}% of the frames' {} × {} (at least {:.0}% each way)",
            100.0 * w / fw,
            100.0 * h / fh,
            frames.width,
            frames.height,
            100.0 * MIN_SIDE
        ),
    );

    let at = frames.place(&read.small);
    let at_small = (at.0 / frames.k, at.1 / frames.k);
    color(&mut report, frames, &read.small, at_small);
    if let Some(reference) = reference {
        compare_reference(&mut report, &read.small, reference, bless);
    }
    report.check("placed", true, format!("at ({}, {}) on the frames", at.0, at.1));
    report
}

/// The file itself: its type by its first bytes, and its metadata.
fn file_checks(report: &mut Report, file: &Path, expect: &Expect) {
    let head =
        std::fs::read(file).map(|b| b.into_iter().take(8).collect::<Vec<_>>()).unwrap_or_default();
    let is = match expect.format {
        ExportFormat::Jpeg => head.starts_with(&[0xFF, 0xD8, 0xFF]),
        ExportFormat::Png => head.starts_with(&[0x89, b'P', b'N', b'G']),
        ExportFormat::Tiff | ExportFormat::Dng => {
            head.starts_with(b"II*\0") || head.starts_with(b"MM\0*")
        }
    };
    report.check("format", is, format!("{:?} asked for", expect.format));
}

/// Each channel's mean against the frames' over the area it covers.
fn color(report: &mut Report, frames: &Frames, stack: &Small, at: (usize, usize)) {
    let mut a = [0f64; 3];
    let mut b = [0f64; 3];
    let w = stack.w.min(frames.mean.w.saturating_sub(at.0));
    let h = stack.h.min(frames.mean.h.saturating_sub(at.1));
    for y in 0..h {
        for x in 0..w {
            let s = stack.px[y * stack.w + x];
            let m = frames.mean.px[(y + at.1) * frames.mean.w + x + at.0];
            for c in 0..3 {
                a[c] += f64::from(s[c]);
                b[c] += f64::from(m[c]);
            }
        }
    }
    let n = (w * h).max(1) as f64;
    let d: Vec<f64> = (0..3).map(|c| (a[c] - b[c]) / n).collect();
    let worst = d.iter().fold(0f64, |m, v| m.max(v.abs()));
    report.check(
        "color",
        worst <= f64::from(COLOR),
        format!(
            "R {:+.3} G {:+.3} B {:+.3} from the frames' mean (at most ±{COLOR})",
            d[0], d[1], d[2]
        ),
    );
}
