//! Days and times as the desktop shows them, in American English: "Oct 28",
//! "Oct 28, 2026", "2:05 PM" (docs/terms-and-usage.md), each in one function
//! here through chrono. Dates in file names are ISO and not formatted here.

use chrono::{DateTime, Local, NaiveDate};

/// A day in this year: "Oct 28".
pub(crate) fn day(date: NaiveDate) -> String {
    date.format("%b %-d").to_string()
}

/// A day in another year: "Oct 28, 2026".
pub(crate) fn day_with_year(date: NaiveDate) -> String {
    date.format("%b %-d, %Y").to_string()
}

/// A time of day: "2:05 PM".
pub(crate) fn time(at: DateTime<Local>) -> String {
    at.format("%-I:%M %p").to_string()
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone as _;

    use super::*;

    #[test]
    fn days_and_times_are_american() {
        let date = NaiveDate::from_ymd_opt(2026, 10, 28).unwrap();
        assert_eq!(day(date), "Oct 28");
        assert_eq!(day_with_year(date), "Oct 28, 2026");
        let at = |h, m| time(Local.with_ymd_and_hms(2026, 10, 28, h, m, 0).unwrap());
        assert_eq!(at(14, 5), "2:05 PM");
        assert_eq!(at(9, 30), "9:30 AM");
        assert_eq!(at(0, 0), "12:00 AM");
        assert_eq!(at(12, 0), "12:00 PM");
    }
}
