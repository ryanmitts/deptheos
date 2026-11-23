//! The strip's words: counts, days, ages, frame ranges and settings.

use std::path::PathBuf;
use std::time::SystemTime;

use chrono::{Datelike as _, NaiveDate};
use deptheos_app::{AlignMode, SessionOptions};

use crate::words::{DesktopWords, say};

/// "Today", "Yesterday", "Oct 28", or "Oct 28, 2025" in another year.
pub(super) fn day_label(w: &DesktopWords, day: NaiveDate, today: NaiveDate) -> String {
    if day == today {
        say!(w, "age-today")
    } else if today.pred_opt() == Some(day) {
        say!(w, "age-yesterday")
    } else if day.year() == today.year() {
        crate::dates::day(day)
    } else {
        crate::dates::day_with_year(day)
    }
}

/// What a render stacked, and how: "24 of 26 frames · RAW · Grid search ·
/// uncropped", or "· not aligned".
pub(super) fn frames_summary(
    w: &DesktopWords,
    included: &[bool],
    linear: bool,
    options: &SessionOptions,
) -> String {
    let stacked = included.iter().filter(|&&i| i).count();
    let total = included.len();
    let mut parts = vec![match (stacked, total) {
        (stacked, total) if stacked == total => say!(w, "frames-count", n = total),
        (stacked, total) => {
            say!(w, "render-frames-of", stacked = stacked, total = total)
        }
    }];
    if linear {
        parts.push("RAW".to_string());
    }
    match options.alignment.mode() {
        None => parts.push(say!(w, "render-unaligned")),
        Some(AlignMode::Grid) => parts.push(say!(w, "render-grid-search")),
        Some(AlignMode::Ecc) => {}
    }
    if options.alignment.is_on() && !options.alignment.crops() {
        parts.push(say!(w, "render-uncropped"));
    }
    parts.join(deptheos_app::words::SEPARATOR)
}

/// How long ago `created` was, coarsely: "just now", "5 minutes ago",
/// "3 weeks ago".
pub(super) fn age(w: &DesktopWords, created: SystemTime, now: SystemTime) -> String {
    let seconds = now.duration_since(created).unwrap_or_default().as_secs();
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    const WEEK: u64 = 7 * DAY;
    const MONTH: u64 = 30 * DAY;
    const YEAR: u64 = 365 * DAY;
    match seconds {
        s if s < MINUTE => say!(w, "age-just-now"),
        s if s < HOUR => say!(w, "age-minutes", n = (s / MINUTE)),
        s if s < DAY => say!(w, "age-hours", n = (s / HOUR)),
        s if s < WEEK => say!(w, "age-days", n = (s / DAY)),
        s if s < MONTH => say!(w, "age-weeks", n = (s / WEEK)),
        s if s < YEAR => say!(w, "age-months", n = (s / MONTH)),
        s => say!(w, "age-years", n = (s / YEAR)),
    }
}

/// "IMG_0120–160", "spider-01–46": the first and last frames stacked, by
/// file stem.
pub(super) fn frame_range(frames: &[PathBuf], included: &[bool]) -> String {
    let mut stacked = frames
        .iter()
        .zip(included)
        .filter(|&(_, &included)| included)
        .map(|(path, _)| path.file_stem().map(|s| s.to_string_lossy().into_owned()));
    let first = stacked.next().flatten().unwrap_or_default();
    let Some(last) = stacked.next_back().flatten() else { return first };
    deptheos_app::frames::names::range_name(&first, &last)
}
