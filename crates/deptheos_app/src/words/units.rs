//! Sizes and durations: the number formatted here (Rust's separators,
//! not the locale's), the unit from the messages (`size-*`, `duration-*`).

use crate::words::{Words, say};

/// A size on disk: "740 KB", "142 MB", "1.2 GB", "67 GB" (decimal units,
/// as the Finder and Files count them). At least 1 KB; GB to one decimal
/// below 10 GB.
pub fn size_text(bytes: u64, words: &Words) -> String {
    let bytes = bytes as f64;
    if bytes < 999.5e3 {
        say!(words, "size-kb", n = format!("{:.0}", (bytes / 1e3).max(1.0)))
    } else if bytes < 999.5e6 {
        say!(words, "size-mb", n = format!("{:.0}", bytes / 1e6))
    } else if bytes < 9.95e9 {
        say!(words, "size-gb", n = format!("{:.1}", bytes / 1e9))
    } else {
        say!(words, "size-gb", n = format!("{:.0}", bytes / 1e9))
    }
}

/// A duration in whole seconds: "45 s", "5 min", "4 min 10 s".
pub(crate) fn duration_text(seconds: u64, words: &Words) -> String {
    let (minutes, seconds) = (seconds / 60, seconds % 60);
    let (m, s) = (minutes.to_string(), seconds.to_string());
    match (minutes, seconds) {
        (0, _) => say!(words, "duration-seconds", seconds = s),
        (_, 0) => say!(words, "duration-minutes", minutes = m),
        _ => say!(words, "duration-minutes-seconds", minutes = m, seconds = s),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Platform;

    #[test]
    fn sizes_are_said_in_decimal_units() {
        let w = Words::english(Platform::Desktop);
        assert_eq!(size_text(0, &w), "1 KB");
        assert_eq!(size_text(740_000, &w), "740 KB");
        assert_eq!(size_text(142_400_000, &w), "142 MB");
        assert_eq!(size_text(1_240_000_000, &w), "1.2 GB");
        assert_eq!(size_text(9_940_000_000, &w), "9.9 GB");
        assert_eq!(size_text(67_100_000_000, &w), "67 GB");
    }

    #[test]
    fn durations_have_spaced_units() {
        let w = Words::english(Platform::Desktop);
        assert_eq!(duration_text(45, &w), "45 s");
        assert_eq!(duration_text(300, &w), "5 min");
        assert_eq!(duration_text(250, &w), "4 min 10 s");
    }
}
