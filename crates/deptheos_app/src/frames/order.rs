//! A stack's frame order: by file name (the default, never switched on its
//! own, since some cameras' clocks are off by seconds) or by capture time,
//! for names that don't follow the shooting order (wrapped buffer
//! handles). A stack that can't be time-ordered stays wholly in name
//! order, with a warning.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::frames::capture::{CaptureInfo, CaptureTime};
use crate::frames::names::natural_path_cmp;
use crate::words::{Words, say};

/// How a stack's frames are ordered.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum FrameOrder {
    /// By file name, numbers compared as numbers.
    #[default]
    Name,
    /// By capture time ([`CaptureInfo::time`]), then shot number, then
    /// name.
    #[serde(rename = "time")]
    CaptureTime,
}

impl FrameOrder {
    pub const ALL: [Self; 2] = [Self::Name, Self::CaptureTime];

    /// What a menu calls it.
    pub fn label(self, words: &Words) -> String {
        match self {
            Self::Name => say!(words, "order-name"),
            Self::CaptureTime => say!(words, "order-capture-time"),
        }
    }

    /// How a manifest or the CLI spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::CaptureTime => "time",
        }
    }

    /// Whether it is the default, [`Self::Name`].
    pub fn is_name(&self) -> bool {
        *self == Self::Name
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "name" => Some(Self::Name),
            "time" | "capture" | "capture-time" => Some(Self::CaptureTime),
            _ => None,
        }
    }
}

/// What a capture-time order is decided from, per frame.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TimeKey {
    pub time: Option<CaptureTime>,
    pub model: Option<String>,
    pub shot: Option<u16>,
}

impl From<CaptureInfo> for TimeKey {
    fn from(info: CaptureInfo) -> Self {
        Self { time: info.time, model: info.model, shot: info.shot }
    }
}

/// `path`'s capture-time key; empty when unreadable.
pub(crate) fn time_key(path: &Path) -> TimeKey {
    crate::frames::capture::read_capture_info(path).map(TimeKey::from).unwrap_or_default()
}

/// A note on a stack's order: why it stayed in name order, a time-order
/// caveat, or that name order differs from capture order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OrderNote {
    /// In name order as asked, but shot in another ([`capture_differs`]).
    DiffersFromNames,
    /// Not every frame records a capture time.
    MissingTimes { missing: usize },
    /// The frames come from more than one camera model.
    MixedCameras,
    /// Whole-second times with several frames in some second.
    WholeSeconds,
    /// `count` frames lack the sub-seconds others record and share their
    /// second, so they can't be placed.
    MissingSubSeconds { count: usize },
    /// Frames with the same time, ordered by shot number then name.
    Ties { count: usize },
}

impl OrderNote {
    /// A clause with no capital or full stop ("3 frames have no capture
    /// time, so the frames stay in name order").
    pub fn message(&self, words: &Words) -> String {
        match self {
            Self::DiffersFromNames => say!(words, "order-differs"),
            Self::MissingTimes { missing } => {
                say!(words, "order-missing-times", count = missing.to_owned())
            }
            Self::MixedCameras => say!(words, "order-mixed-cameras"),
            Self::WholeSeconds => say!(words, "order-whole-seconds"),
            Self::MissingSubSeconds { count } => {
                say!(words, "order-missing-sub-seconds", count = count.to_owned())
            }
            Self::Ties { count } => say!(words, "order-ties", count = count.to_owned()),
        }
    }

    /// The note on stack `stack`: "s036: 3 frames have no capture time, …".
    pub(crate) fn on_stack(&self, stack: &str, words: &Words) -> String {
        say!(words, "order-on-stack", stack = stack, note = self.message(words))
    }
}

/// One stack's frames by capture time, or why they stay in name order.
#[derive(Clone, Debug, PartialEq, Eq)]
enum TimeOrder {
    /// Indices into the paths, and how many frames share a time.
    Sorted { order: Vec<usize>, ties: usize },
    /// Left in name order; never [`OrderNote::Ties`] or
    /// [`OrderNote::DiffersFromNames`].
    FellBack(OrderNote),
}

impl TimeOrder {
    /// The indices to use (`names` when it fell back) and the note.
    fn into_parts(self, names: Vec<usize>) -> (Vec<usize>, Option<OrderNote>) {
        match self {
            Self::Sorted { order, ties } => {
                (order, (ties > 0).then_some(OrderNote::Ties { count: ties }))
            }
            Self::FellBack(note) => (names, Some(note)),
        }
    }
}

/// Indices of `paths` in name order.
fn name_order(paths: &[PathBuf]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..paths.len()).collect();
    order.sort_by(|&a, &b| natural_path_cmp(&paths[a], &paths[b]));
    order
}

/// One stack's indices in capture order, `keys` parallel to `paths`.
/// Time-sorted only when every frame has a time, one camera model, and the
/// times tell frames apart; otherwise name order, with the note why.
///
/// A tied group goes by shot number only when all its frames have one,
/// else by name. The key (time, shot or none, distinct name rank) makes
/// the order total.
pub(crate) fn capture_order(
    paths: &[PathBuf],
    keys: &[TimeKey],
) -> (Vec<usize>, Option<OrderNote>) {
    let names = name_order(paths);
    time_order(paths, &names, keys).into_parts(names)
}

/// [`capture_order`], given `names`, the name order of `paths`.
fn time_order(paths: &[PathBuf], names: &[usize], keys: &[TimeKey]) -> TimeOrder {
    assert_eq!(paths.len(), keys.len(), "one key per path");
    let fall_back = TimeOrder::FellBack;
    let missing = keys.iter().filter(|k| k.time.is_none()).count();
    if missing > 0 {
        return fall_back(OrderNote::MissingTimes { missing });
    }
    if keys.windows(2).any(|w| w[0].model != w[1].model) {
        return fall_back(OrderNote::MixedCameras);
    }
    let time = |i: usize| keys[i].time.expect("checked above");
    let mut per_second: HashMap<i64, usize> = HashMap::new();
    for i in 0..keys.len() {
        *per_second.entry(time(i).seconds).or_default() += 1;
    }
    let shares_second = |i: usize| per_second[&time(i).seconds] > 1;
    let whole = (0..keys.len()).all(|i| time(i).micros.unwrap_or(0) == 0);
    if whole {
        if (0..keys.len()).any(shares_second) {
            return fall_back(OrderNote::WholeSeconds);
        }
    } else {
        // A frame without sub-seconds can't be placed in a shared second.
        let count =
            (0..keys.len()).filter(|&i| time(i).micros.is_none() && shares_second(i)).count();
        if count > 0 {
            return fall_back(OrderNote::MissingSubSeconds { count });
        }
    }
    let rank: Vec<usize> = {
        let mut rank = vec![0; paths.len()];
        for (r, &i) in names.iter().enumerate() {
            rank[i] = r;
        }
        rank
    };
    let key = |i: usize| (time(i).seconds, time(i).micros.unwrap_or(0));
    // Whether every frame of each time's group has a shot number.
    let mut all_shots: HashMap<(i64, u32), bool> = HashMap::new();
    for (i, k) in keys.iter().enumerate() {
        *all_shots.entry(key(i)).or_insert(true) &= k.shot.is_some();
    }
    let mut order: Vec<usize> = (0..paths.len()).collect();
    order.sort_by_key(|&i| {
        let shot = if all_shots[&key(i)] { keys[i].shot } else { None };
        (key(i), shot, rank[i])
    });
    // Every frame of every tied group.
    let tied = |n: usize| {
        (n > 0 && key(order[n - 1]) == key(order[n]))
            || (n + 1 < order.len() && key(order[n + 1]) == key(order[n]))
    };
    let ties = (0..order.len()).filter(|&n| tied(n)).count();
    TimeOrder::Sorted { order, ties }
}

/// One stack's `paths` in `order`, reading capture times from disk, with
/// the note a list or batch shows.
pub fn ordered(paths: Vec<PathBuf>, order: FrameOrder) -> (Vec<PathBuf>, Option<OrderNote>) {
    ordered_with(paths, order, time_key)
}

/// [`ordered`], with capture times from `time`.
fn ordered_with(
    paths: Vec<PathBuf>,
    order: FrameOrder,
    time: impl Fn(&Path) -> TimeKey,
) -> (Vec<PathBuf>, Option<OrderNote>) {
    let (indices, note) = match order {
        FrameOrder::Name => {
            let paths = sorted_by_name(paths);
            let note = capture_differs(&paths, time).then_some(OrderNote::DiffersFromNames);
            return (paths, note);
        }
        FrameOrder::CaptureTime => {
            let keys: Vec<TimeKey> = paths.iter().map(|p| time(p)).collect();
            capture_order(&paths, &keys)
        }
    };
    let sorted = indices.iter().map(|&i| paths[i].clone()).collect();
    (sorted, note)
}

fn sorted_by_name(mut paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths.sort_by(|a, b| natural_path_cmp(a, b));
    paths
}

/// Whether capture order differs from name order enough to warn (the Sort
/// by Capture Time banner). Not for an exact reversal (far to near), which
/// the stacker's reverse check handles, nor when it falls back.
fn capture_differs(paths: &[PathBuf], time: impl Fn(&Path) -> TimeKey) -> bool {
    let keys: Vec<TimeKey> = paths.iter().map(|p| time(p)).collect();
    let names = name_order(paths);
    match time_order(paths, &names, &keys) {
        TimeOrder::FellBack(_) => false,
        TimeOrder::Sorted { order, .. } => order != names && !order.iter().eq(names.iter().rev()),
    }
}

/// One stack (in name order) in `order`, with its "s036: …" note for a
/// list or batch. Reads each frame's time once through `time`.
pub(crate) fn order_stack(
    label: &str,
    paths: Vec<PathBuf>,
    order: FrameOrder,
    time: impl Fn(&Path) -> TimeKey,
    words: &Words,
) -> (Vec<PathBuf>, Option<String>) {
    match order {
        FrameOrder::Name => {
            let note = capture_differs(&paths, time)
                .then(|| say!(words, "order-differs-from-names", stack = label));
            (paths, note)
        }
        FrameOrder::CaptureTime => {
            let (paths, note) = ordered_with(paths, order, time);
            (paths, note.map(|note| note.on_stack(label, words)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(seconds: i64, micros: Option<u32>) -> TimeKey {
        TimeKey {
            time: Some(CaptureTime { seconds, micros }),
            model: Some("EOS R5".into()),
            shot: None,
        }
    }

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(|n| PathBuf::from(format!("/s/{n}"))).collect()
    }

    #[test]
    fn sub_seconds_order_wrapped_buffer_names() {
        // Buffer handles wrapped: 9 was shot first.
        let p = paths(&["IMG_1.JPG", "IMG_2.JPG", "IMG_9.JPG"]);
        let k = [key(10, Some(200_000)), key(10, Some(300_000)), key(10, Some(100_000))];
        let order = capture_order(&p, &k);
        assert_eq!(order, (vec![2, 0, 1], None));
    }

    #[test]
    fn a_missing_time_or_another_camera_keeps_name_order() {
        let p = paths(&["b.jpg", "a.jpg"]);
        let mut k = vec![key(2, Some(1)), TimeKey::default()];
        let order = capture_order(&p, &k);
        assert_eq!(order.0, [1, 0]);
        assert_eq!(order.1, Some(OrderNote::MissingTimes { missing: 1 }));
        k[1] = TimeKey { model: Some("OM-1".into()), ..key(1, Some(1)) };
        assert_eq!(capture_order(&p, &k).1, Some(OrderNote::MixedCameras));
    }

    #[test]
    fn whole_seconds_order_only_one_frame_a_second() {
        let p = paths(&["1.jpg", "2.jpg", "3.jpg"]);
        let slow = [key(30, None), key(10, Some(0)), key(20, None)];
        assert_eq!(capture_order(&p, &slow).0, [1, 2, 0]);
        let burst = [key(10, None), key(10, None), key(11, None)];
        let order = capture_order(&p, &burst);
        assert_eq!(order, (vec![0, 1, 2], Some(OrderNote::WholeSeconds)));
    }

    #[test]
    fn ties_go_by_shot_number_then_name_and_are_counted() {
        let p = paths(&["c.jpg", "a.jpg", "b.jpg"]);
        let mut k = vec![key(5, Some(1)), key(5, Some(1)), key(4, Some(1))];
        k[0].shot = Some(1);
        k[1].shot = Some(2);
        let order = capture_order(&p, &k);
        assert_eq!(order.0, [2, 0, 1]);
        assert_eq!(order.1, Some(OrderNote::Ties { count: 2 }));
    }

    #[test]
    fn ties_count_every_frame_of_every_tied_group() {
        let p = paths(&["1.jpg", "2.jpg", "3.jpg", "4.jpg", "5.jpg", "6.jpg"]);
        let k: Vec<TimeKey> = [1, 1, 2, 3, 3, 3].iter().map(|&s| key(s, Some(1))).collect();
        assert_eq!(capture_order(&p, &k).1, Some(OrderNote::Ties { count: 5 }));
    }

    #[test]
    fn ordered_sorts_by_name_or_by_time() {
        let p = paths(&["10.jpg", "9.jpg"]);
        let late_nine = |path: &Path| match path.ends_with("9.jpg") {
            true => key(2, Some(5)),
            false => key(1, Some(5)),
        };
        let (by_name, _) = ordered_with(p.clone(), FrameOrder::Name, late_nine);
        assert_eq!(by_name, paths(&["9.jpg", "10.jpg"]));
        let (by_time, note) = ordered_with(p, FrameOrder::CaptureTime, late_nine);
        assert_eq!((by_time, note), (paths(&["10.jpg", "9.jpg"]), None));
        assert!(capture_differs(&paths(&["8.jpg", "9.jpg", "10.jpg"]), late_nine));
    }

    /// Name order notes a different capture order, but not agreement or an
    /// exact reversal.
    #[test]
    fn name_order_notes_frames_shot_in_another() {
        let p = paths(&["8.jpg", "9.jpg", "10.jpg"]);
        let shot = |order: [u32; 3]| {
            let p = p.clone();
            move |path: &Path| key(1, Some(order[p.iter().position(|q| q == path).unwrap()] + 1))
        };
        let note = |order| ordered_with(p.clone(), FrameOrder::Name, shot(order)).1;
        assert_eq!(note([1, 2, 0]), Some(OrderNote::DiffersFromNames));
        assert_eq!(note([0, 1, 2]), None);
        assert_eq!(note([2, 1, 0]), None, "far to near");
        // No times: nothing known to differ.
        assert_eq!(ordered_with(p.clone(), FrameOrder::Name, |_| TimeKey::default()).1, None);
    }

    #[test]
    fn mixed_shot_numbers_in_a_tie_order_by_name_in_any_input_order() {
        // a has shot 2, b none, c shot 1, all at one time: by name.
        let k = |name: &str| {
            let mut k = key(5, Some(1));
            k.shot = match name {
                "a.jpg" => Some(2),
                "c.jpg" => Some(1),
                _ => None,
            };
            k
        };
        for names in [
            ["a.jpg", "b.jpg", "c.jpg"],
            ["c.jpg", "b.jpg", "a.jpg"],
            ["b.jpg", "c.jpg", "a.jpg"],
            ["c.jpg", "a.jpg", "b.jpg"],
        ] {
            let p = paths(&names);
            let keys: Vec<TimeKey> = names.iter().map(|n| k(n)).collect();
            let order = capture_order(&p, &keys);
            let sorted: Vec<&str> = order.0.iter().map(|&i| names[i]).collect();
            assert_eq!(sorted, ["a.jpg", "b.jpg", "c.jpg"], "from {names:?}");
            assert_eq!(order.1, Some(OrderNote::Ties { count: 3 }));
        }
    }

    #[test]
    fn a_tie_goes_by_shot_number_only_when_all_its_frames_have_one() {
        // One group all with shot numbers (by shot), one without (by name).
        let p = paths(&["1.jpg", "2.jpg", "3.jpg", "4.jpg", "5.jpg"]);
        let shots = [Some(3), Some(2), Some(1), Some(9), None];
        let seconds = [1, 1, 1, 2, 2];
        let k: Vec<TimeKey> =
            (0..5).map(|i| TimeKey { shot: shots[i], ..key(seconds[i], Some(5)) }).collect();
        assert_eq!(capture_order(&p, &k).0, [2, 1, 0, 3, 4]);
    }

    #[test]
    fn a_frame_without_sub_seconds_in_a_shared_second_keeps_name_order() {
        let p = paths(&["1.jpg", "2.jpg", "3.jpg"]);
        // 1 records no sub-seconds and shares second 10 with 2.
        let shared = [key(10, None), key(10, Some(500_000)), key(9, Some(100_000))];
        let order = capture_order(&p, &shared);
        assert_eq!(order.0, [0, 1, 2]);
        assert_eq!(order.1, Some(OrderNote::MissingSubSeconds { count: 1 }));
        assert!(
            !capture_differs(&p, |path| shared[p.iter().position(|q| q == path).unwrap()].clone())
        );
        // Alone in its second it is placed by the second.
        let alone = [key(11, None), key(10, Some(500_000)), key(10, Some(100_000))];
        let order = capture_order(&p, &alone);
        assert_eq!(order.0, [2, 1, 0]);
        assert_eq!(order.1, None);
        // A recorded zero is a time like any other.
        let zero = [key(10, Some(0)), key(10, Some(500_000)), key(9, Some(100_000))];
        assert_eq!(capture_order(&p, &zero).0, [2, 0, 1]);
    }

    /// Whether `names` warn when `shot[n]` is when the `n`th was taken.
    fn shot_in(names: &[&str], shot: &[u32]) -> bool {
        let p = paths(names);
        let time = |path: &Path| {
            let n = p.iter().position(|q| q == path).unwrap();
            key(1, Some(shot[n] + 1))
        };
        capture_differs(&p, time)
    }

    #[test]
    fn an_exact_reversal_is_not_a_difference_worth_a_warning() {
        let names = ["1.jpg", "2.jpg", "3.jpg", "4.jpg"];
        assert!(!shot_in(&names, &[3, 2, 1, 0]), "far to near, exactly reversed");
        assert!(shot_in(&names, &[3, 1, 2, 0]), "reversed with two swapped");
        assert!(shot_in(&names, &[2, 1, 0, 3]), "reversed but for one frame");
        assert!(!shot_in(&names[..2], &[1, 0]), "two frames reversed");
        assert!(!shot_in(&names, &[0, 1, 2, 3]), "in name order");
        // Sorting by capture time still reverses the list.
        let p = paths(&names);
        let k: Vec<TimeKey> = (0..4).map(|n| key(1, Some(4 - n))).collect();
        assert_eq!(capture_order(&p, &k).0, [3, 2, 1, 0]);
    }
}
