//! Auto sort: a folder's stacks, cut where a capture-time gap is long for
//! the pace around it ([`SplitRule`]) or the camera changes. Runs too short
//! are left out (Batch counts them skipped). Time only splits: within a
//! stack the order is by name, unless Batch is asked for
//! [`crate::FrameOrder::CaptureTime`].

use std::cmp::Ordering;
use std::path::Path;
use std::time::UNIX_EPOCH;

use crate::frames::capture::CaptureInfo;
use crate::frames::folder::Shot;
use crate::frames::names::natural_cmp;

/// What is known of one shot's capture.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ShotTime {
    /// Camera-clock seconds (else the file's modified time); `None` leaves
    /// the shot out.
    pub seconds: Option<f64>,
    /// The camera model, when recorded.
    pub model: Option<String>,
}

/// Where one stack ends and the next begins.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SplitRule {
    /// No gap this short splits. At least 2 s, so whole-second stamps (0 or
    /// 1 s apart in a burst) never split.
    pub min_gap: f64,
    /// A gap splits at this multiple of the median gap around it, so a
    /// rail stack at 5 s a frame isn't cut at 2 s.
    pub pace_factor: f64,
    /// How many gaps on each side make "the gaps around it".
    pub window: usize,
    /// Runs with fewer shots are left out.
    pub min_shots: usize,
}

impl Default for SplitRule {
    fn default() -> Self {
        Self { min_gap: 2.0, pace_factor: 4.0, window: 5, min_shots: 5 }
    }
}

/// The stacks in `shots` given parallel `times`, in capture order, each as
/// indices into `shots` in `shots`' order.
pub(crate) fn split(shots: &[Shot], times: &[ShotTime], rule: &SplitRule) -> Vec<Vec<usize>> {
    assert_eq!(shots.len(), times.len(), "one time per shot");
    let mut timed: Vec<usize> = (0..shots.len()).filter(|&i| times[i].seconds.is_some()).collect();
    timed.sort_by(|&a, &b| {
        let (ta, tb) = (times[a].seconds.unwrap_or(0.0), times[b].seconds.unwrap_or(0.0));
        ta.partial_cmp(&tb)
            .unwrap_or(Ordering::Equal)
            .then_with(|| natural_cmp(&shots[a].stem, &shots[b].stem))
    });
    let seconds: Vec<f64> = timed.iter().map(|&i| times[i].seconds.unwrap_or(0.0)).collect();
    let gaps: Vec<f64> = seconds.windows(2).map(|w| w[1] - w[0]).collect();

    let mut runs: Vec<Vec<usize>> = Vec::new();
    for (k, &shot) in timed.iter().enumerate() {
        let cut =
            k == 0 || splits(&gaps, k - 1, rule) || times[timed[k - 1]].model != times[shot].model;
        if cut {
            runs.push(Vec::new());
        }
        runs.last_mut().expect("a run was started").push(shot);
    }

    let mut runs: Vec<Vec<usize>> =
        runs.into_iter().filter(|run| run.len() >= rule.min_shots).collect();
    for run in &mut runs {
        run.sort_unstable();
    }
    runs
}

impl ShotTime {
    /// The EXIF time, else the file's modified time.
    pub(crate) fn of(path: &Path, info: CaptureInfo) -> Self {
        let modified = || {
            let time = std::fs::metadata(path).ok()?.modified().ok()?;
            Some(time.duration_since(UNIX_EPOCH).ok()?.as_secs_f64())
        };
        let seconds = info.time.map(|t| t.as_secs_f64()).or_else(modified);
        Self { seconds, model: info.model }
    }
}

/// Whether gap `k` (between timed shots `k` and `k + 1`) ends a stack.
fn splits(gaps: &[f64], k: usize, rule: &SplitRule) -> bool {
    let gap = gaps[k];
    if gap <= rule.min_gap {
        return false;
    }
    let from = k.saturating_sub(rule.window);
    let to = (k + 1 + rule.window).min(gaps.len());
    let mut around: Vec<f64> = gaps[from..k].iter().chain(&gaps[k + 1..to]).copied().collect();
    if around.is_empty() {
        return true;
    }
    around.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    let mid = around.len() / 2;
    let median = if around.len().is_multiple_of(2) {
        (around[mid - 1] + around[mid]) / 2.0
    } else {
        around[mid]
    };
    gap > rule.pace_factor * median
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// [`split`] as lists of shot stems.
    fn suggest(shots: &[Shot], times: &[ShotTime], rule: &SplitRule) -> Vec<Vec<String>> {
        split(shots, times, rule)
            .into_iter()
            .map(|run| run.into_iter().map(|i| shots[i].stem.clone()).collect())
            .collect()
    }

    /// Shots named `n0000…` with times at `seconds`, one camera.
    fn folder(seconds: &[f64]) -> (Vec<Shot>, Vec<ShotTime>) {
        let shots = crate::frames::folder::shots(
            (0..seconds.len()).map(|i| PathBuf::from(format!("f/n{i:04}.ORF"))),
        );
        let times = seconds
            .iter()
            .map(|&s| ShotTime { seconds: Some(s), model: Some("OM-1".into()) })
            .collect();
        (shots, times)
    }

    fn sizes(stacks: &[Vec<String>]) -> Vec<usize> {
        stacks.iter().map(Vec::len).collect()
    }

    /// `n` frames from `start`, `step` seconds apart.
    fn burst(start: f64, step: f64, n: usize) -> Vec<f64> {
        (0..n).map(|i| start + step * i as f64).collect()
    }

    #[test]
    fn bursts_minutes_apart_are_stacks() {
        let seconds = [burst(0.0, 0.1, 46), burst(135.0, 0.1, 84), burst(379.0, 0.1, 20)].concat();
        let (shots, times) = folder(&seconds);
        assert_eq!(sizes(&suggest(&shots, &times, &SplitRule::default())), [46, 84, 20]);
    }

    #[test]
    fn whole_second_stamps_do_not_split_a_burst() {
        // Ten frames at 10 fps, stamped in whole seconds: 0 s and 1 s gaps.
        let seconds = [7.0, 7.0, 7.0, 7.0, 7.0, 7.0, 7.0, 8.0, 8.0, 8.0];
        let (shots, times) = folder(&seconds);
        assert_eq!(sizes(&suggest(&shots, &times, &SplitRule::default())), [10]);
    }

    #[test]
    fn a_slow_rail_stack_is_not_cut_at_two_seconds() {
        // A frame every 5 s, then another subject a minute later.
        let seconds = [burst(0.0, 5.0, 30), burst(205.0, 5.0, 12)].concat();
        let (shots, times) = folder(&seconds);
        assert_eq!(sizes(&suggest(&shots, &times, &SplitRule::default())), [30, 12]);
    }

    #[test]
    fn short_runs_and_untimed_shots_stay_unsorted() {
        let mut seconds = [burst(0.0, 0.1, 10), burst(60.0, 0.1, 3), burst(120.0, 0.1, 6)].concat();
        seconds.push(500.0);
        let (shots, mut times) = folder(&seconds);
        times[19].seconds = None;
        let stacks = suggest(&shots, &times, &SplitRule::default());
        assert_eq!(sizes(&stacks), [10, 6]);
    }

    #[test]
    fn a_camera_change_splits() {
        let seconds = burst(0.0, 0.1, 12);
        let (shots, mut times) = folder(&seconds);
        for t in &mut times[6..] {
            t.model = Some("Canon EOS R5m2".into());
        }
        assert_eq!(sizes(&suggest(&shots, &times, &SplitRule::default())), [6, 6]);
    }

    /// A stack lists its shots in natural order, however they were taken.
    #[test]
    fn stacks_are_in_name_order_never_time() {
        let (shots, times) = folder(&[0.0, 0.4, 0.1, 0.2, 0.3]);
        assert_eq!(
            suggest(&shots, &times, &SplitRule::default())[0],
            ["n0000", "n0001", "n0002", "n0003", "n0004"]
        );
    }

    #[test]
    fn nothing_to_suggest_in_an_empty_folder() {
        assert!(suggest(&[], &[], &SplitRule::default()).is_empty());
    }
}
