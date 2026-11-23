//! Natural name order: digit runs compare as numbers, so "BEE - 99 of
//! 1317" precedes "BEE - 100 of 1317" (camera names are zero-padded;
//! exported names aren't).

use std::borrow::Cow;
use std::cmp::Ordering;
use std::ffi::OsStr;
use std::path::Path;

/// `path`'s last name for labels, or the whole path when it has none ("/").
pub fn file_label(path: &Path) -> String {
    path.file_name()
        .map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// Natural order: digit runs by value, the rest case-insensitive; ties
/// (`"01"`/`"1"`, `"a"`/`"A"`) fall back to plain order, so it is total.
pub(crate) fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a, b);
    loop {
        match (x.chars().next(), y.chars().next()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let (m, rest_x) = split_digits(x);
                let (n, rest_y) = split_digits(y);
                let order = compare_numbers(m, n);
                if order != Ordering::Equal {
                    return order;
                }
                (x, y) = (rest_x, rest_y);
            }
            (Some(c), Some(d)) => {
                let order = c.to_lowercase().cmp(d.to_lowercase());
                if order != Ordering::Equal {
                    return order;
                }
                (x, y) = (&x[c.len_utf8()..], &y[d.len_utf8()..]);
            }
        }
    }
}

/// Paths in natural order: by folder, then by file name, so a stack pooled
/// from `100CANON/IMG_9999` and `101CANON/IMG_0001` keeps the card's order.
pub(crate) fn natural_path_cmp(a: &Path, b: &Path) -> Ordering {
    natural_cmp(&text(a.parent().map(Path::as_os_str)), &text(b.parent().map(Path::as_os_str)))
        .then_with(|| natural_cmp(&text(a.file_name()), &text(b.file_name())))
}

fn text(s: Option<&OsStr>) -> Cow<'_, str> {
    s.map(OsStr::to_string_lossy).unwrap_or_default()
}

/// The leading run of ASCII digits in `s`, and what follows it.
fn split_digits(s: &str) -> (&str, &str) {
    let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    s.split_at(end)
}

/// Two digit runs by value, of any length.
fn compare_numbers(m: &str, n: &str) -> Ordering {
    let m = m.trim_start_matches('0');
    let n = n.trim_start_matches('0');
    m.len().cmp(&n.len()).then_with(|| m.cmp(n))
}

/// "P1010213–260", "spider-01–46": `first`, then `last` minus their common
/// start, keeping at least three digits of a cut number, as page ranges
/// are written. One name when they're equal.
pub fn range_name(first: &str, last: &str) -> String {
    if first == last || last.is_empty() {
        return first.to_string();
    }
    let l: Vec<char> = last.chars().collect();
    let shared = first.chars().zip(&l).take_while(|(a, b)| a == *b).count();
    let digit = |i: usize| l.get(i).is_some_and(char::is_ascii_digit);
    let mut start = shared;
    // The number the cut falls in: where it starts and ends in `last`.
    let mut run_start = start;
    while run_start > 0 && digit(run_start - 1) {
        run_start -= 1;
    }
    if run_start < start {
        let mut run_end = start;
        while digit(run_end) {
            run_end += 1;
        }
        start = start.min(run_end.saturating_sub(3).max(run_start));
    }
    if start >= l.len() {
        return format!("{first}–{last}");
    }
    let tail: String = l[start..].iter().collect();
    format!("{first}–{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_drop_the_shared_lead() {
        assert_eq!(range_name("IMG_0120", "IMG_0160"), "IMG_0120–160");
        assert_eq!(range_name("a9", "a10"), "a9–10");
        assert_eq!(range_name("spider-01", "spider-46"), "spider-01–46");
        assert_eq!(range_name("P1", "P1"), "P1");
    }

    fn sorted(names: &[&str]) -> Vec<String> {
        let mut v: Vec<String> = names.iter().map(|s| s.to_string()).collect();
        v.sort_by(|a, b| natural_cmp(a, b));
        v
    }

    #[test]
    fn numbers_compare_by_value() {
        assert_eq!(
            sorted(&["BEE - 100 of 1317.JPG", "BEE - 99 of 1317.JPG", "BEE - 9 of 1317.JPG"]),
            ["BEE - 9 of 1317.JPG", "BEE - 99 of 1317.JPG", "BEE - 100 of 1317.JPG"]
        );
        assert_eq!(
            sorted(&["spider-10", "spider-2", "spider-1"]),
            ["spider-1", "spider-2", "spider-10"]
        );
    }

    #[test]
    fn camera_names_keep_their_order() {
        let names =
            ["9U1A5299.CR3", "9U1A5300.CR3", "9U1A5301.CR3", "P6149815.ORF", "P6149816.ORF"];
        assert_eq!(sorted(&names), names);
    }

    #[test]
    fn case_is_ignored_and_ties_are_stable() {
        assert_eq!(sorted(&["b.jpg", "A.jpg", "a.jpg"]), ["A.jpg", "a.jpg", "b.jpg"]);
        assert_eq!(sorted(&["img1", "img01", "img001"]), ["img001", "img01", "img1"]);
        assert_eq!(natural_cmp("x", "x"), Ordering::Equal);
    }

    #[test]
    fn long_digit_runs_do_not_overflow() {
        assert_eq!(
            natural_cmp("a99999999999999999999999", "a100000000000000000000000"),
            Ordering::Less
        );
    }

    #[test]
    fn prefixes_come_first() {
        assert_eq!(sorted(&["IMG_0001-2", "IMG_0001"]), ["IMG_0001", "IMG_0001-2"]);
        assert_eq!(sorted(&["a1b", "a1"]), ["a1", "a1b"]);
    }

    #[test]
    fn non_ascii_names_compare() {
        assert_eq!(sorted(&["Équipe 10", "équipe 9"]), ["équipe 9", "Équipe 10"]);
    }

    #[test]
    fn paths_compare_by_folder_then_name() {
        let mut paths = vec![
            Path::new("d/101CANON/IMG_0001.CR3"),
            Path::new("d/100CANON/IMG_9999.CR3"),
            Path::new("d/100CANON/IMG_10.CR3"),
        ];
        paths.sort_by(|a, b| natural_path_cmp(a, b));
        assert_eq!(
            paths,
            [
                Path::new("d/100CANON/IMG_10.CR3"),
                Path::new("d/100CANON/IMG_9999.CR3"),
                Path::new("d/101CANON/IMG_0001.CR3")
            ]
        );
    }
}
