//! The list: the images the next Stack is made from, one row per shot (a
//! RAW and its JPEG are one shot), each included or excluded. A render
//! records its list, which [`List::from_render`] loads back.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::frames::folder::{self, Shot, Source};
use crate::frames::names::natural_path_cmp;
use crate::frames::order::{FrameOrder, TimeKey, capture_order};
use crate::frames::raw_backend::RawBackend;
use crate::library::history::{Moves, Render};
use crate::runs::session::check_same_kind;
use crate::words::explain::Platform;
use crate::words::{Words, say};

pub mod add;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct List {
    /// Every file, both of a pair, in [`Self::order`].
    #[serde(default)]
    pub files: Vec<PathBuf>,
    /// Excluded shots, by any of their files.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub excluded: BTreeSet<PathBuf>,
    /// Which file of a pair is stacked.
    #[serde(default)]
    pub source: Source,
    /// How each folder's shots are ordered. In capture-time order,
    /// [`Self::files`] holds that order (see [`Self::sort`]).
    #[serde(default, skip_serializing_if = "FrameOrder::is_name")]
    pub order: FrameOrder,
}

/// What [`List::expand`] found: the image files, and the folders it
/// couldn't read (so they don't pass for empty).
#[derive(Debug, Default)]
pub struct Expanded {
    pub files: Vec<PathBuf>,
    pub unreadable: Vec<(PathBuf, std::io::Error)>,
}

impl List {
    /// The shots, by folder, then in [`Self::order`].
    pub fn shots(&self) -> Vec<Shot> {
        let shots = folder::shots(self.files.iter().cloned());
        match self.order {
            FrameOrder::Name => shots,
            FrameOrder::CaptureTime => {
                let position: std::collections::HashMap<&PathBuf, usize> =
                    self.files.iter().enumerate().map(|(i, f)| (f, i)).collect();
                let first =
                    |s: &Shot| s.files.iter().filter_map(|f| position.get(f)).min().copied();
                let mut shots = shots;
                shots.sort_by_key(|s| first(s));
                shots
            }
        }
    }

    /// Order each folder's shots by `order`, reading capture times from
    /// disk; a folder that can't be time-sorted stays in name order
    /// ([`Self::order_notes`] says why).
    pub fn set_order(&mut self, order: FrameOrder) {
        self.order = order;
        self.sort();
    }

    /// "s036: …" lines: in capture order, why folders stayed in name order
    /// and tied times; in name order, folders shot out of name order (for
    /// the Sort by Capture Time banner). Reads the disk: off the UI thread.
    pub fn order_notes(&self, words: &Words) -> Vec<String> {
        self.order_notes_with(crate::frames::order::time_key, words)
    }

    pub(crate) fn order_notes_with(
        &self,
        time: impl Fn(&Path) -> TimeKey,
        words: &Words,
    ) -> Vec<String> {
        let mut notes = Vec::new();
        for (folder, shots) in self.folders() {
            let paths: Vec<PathBuf> =
                shots.iter().map(|s| s.file(self.source).to_path_buf()).collect();
            let name = folder
                .file_name()
                .map_or_else(|| folder.display().to_string(), |n| n.to_string_lossy().into_owned());
            notes.extend(
                crate::frames::order::order_stack(&name, paths, self.order, &time, words).1,
            );
        }
        notes
    }

    /// Shots by folder, all in natural name order.
    fn folders(&self) -> Vec<(PathBuf, Vec<Shot>)> {
        let mut folders: Vec<(PathBuf, Vec<Shot>)> = Vec::new();
        for shot in folder::shots(self.files.iter().cloned()) {
            let dir = shot.files[0].parent().unwrap_or(Path::new("")).to_path_buf();
            match folders.last_mut() {
                Some((d, shots)) if *d == dir => shots.push(shot),
                _ => folders.push((dir, vec![shot])),
            }
        }
        folders
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn is_included(&self, shot: &Shot) -> bool {
        !shot.files.iter().any(|f| self.excluded.contains(f))
    }

    /// Add image files and the images directly inside folders, ignoring
    /// anything else and files already listed. Returns how many were added.
    pub fn add(&mut self, paths: impl IntoIterator<Item = PathBuf>) -> usize {
        let expanded = Self::expand(paths);
        for (folder, e) in &expanded.unreadable {
            tracing::warn!("couldn't read {}: {e}", folder.display());
        }
        self.add_files(expanded.files)
    }

    /// The image files `paths` stand for. Reads the disk (seconds on a
    /// share or card): a GUI runs it off the UI thread, then
    /// [`Self::add_files`].
    pub fn expand(paths: impl IntoIterator<Item = PathBuf>) -> Expanded {
        let mut expanded = Expanded::default();
        for path in paths {
            if path.is_dir() {
                match folder::list_folder(&path) {
                    Ok(files) => expanded.files.extend(files),
                    Err(e) => expanded.unreadable.push((path, e)),
                }
            } else if folder::is_image(&path) {
                expanded.files.push(path);
            }
        }
        expanded
    }

    /// Add unlisted files and [`Self::sort`], which may read the disk (a
    /// GUI uses [`Self::add_files_unsorted`]). Returns how many were added.
    pub fn add_files(&mut self, files: Vec<PathBuf>) -> usize {
        let added = self.add_files_unsorted(files);
        if added > 0 {
            self.sort();
        }
        added
    }

    /// Add unlisted files without reading the disk; in capture-time order
    /// they go after their folder's frames until [`Self::sort`]. Returns
    /// how many were added.
    pub fn add_files_unsorted(&mut self, files: Vec<PathBuf>) -> usize {
        let mut listed: BTreeSet<PathBuf> = self.files.iter().cloned().collect();
        let before = self.files.len();
        for file in files {
            if listed.insert(file.clone()) {
                self.files.push(file);
            }
        }
        self.place();
        self.files.len() - before
    }

    /// Take `shots` out of the list.
    pub fn remove(&mut self, shots: &[Shot]) {
        let gone: BTreeSet<&PathBuf> = shots.iter().flat_map(|s| &s.files).collect();
        self.files.retain(|f| !gone.contains(f));
        self.excluded.retain(|f| !gone.contains(f));
    }

    pub fn clear(&mut self) {
        *self = Self { source: self.source, order: self.order, ..Self::default() };
    }

    /// Include or exclude `shot`.
    pub fn set_included(&mut self, shot: &Shot, included: bool) {
        for file in &shot.files {
            if included {
                self.excluded.remove(file);
            } else {
                self.excluded.insert(file.clone());
            }
        }
    }

    /// Each shot's file to stack, in order, and whether it is included.
    pub fn frames(&self) -> Vec<(PathBuf, bool)> {
        self.shots()
            .iter()
            .map(|shot| (shot.file(self.source).to_path_buf(), self.is_included(shot)))
            .collect()
    }

    /// The list `render` was made from: a shot excluded when none of its
    /// files was stacked, pairs stacking their RAW when RAW was stacked.
    pub fn from_render(render: &Render) -> Self {
        let stacked: BTreeSet<&PathBuf> = render.stacked().collect();
        let raw = render.stacked().any(|f| folder::is_raw_file(f));
        let mut list = Self::of_files(render.frames.clone(), raw, render.options.order);
        for shot in list.shots() {
            if !shot.files.iter().any(|f| stacked.contains(f)) {
                list.excluded.extend(shot.files);
            }
        }
        list
    }

    /// `frames`, all included, pairs stacking their RAW when any frame is
    /// RAW. In capture-time `order` they are taken as already ordered.
    pub fn of_frames(frames: &[PathBuf], order: FrameOrder) -> Self {
        let raw = frames.iter().any(|f| folder::is_raw_file(f));
        Self::of_files(frames.to_vec(), raw, order)
    }

    /// `files`, all included. In capture-time order they are taken as
    /// already ordered (a render's transforms parallel them), so no capture
    /// time is read.
    fn of_files(files: Vec<PathBuf>, raw: bool, order: FrameOrder) -> Self {
        let source = if raw { Source::Raw } else { Source::Standard };
        let mut list = Self { files, excluded: BTreeSet::new(), source, order };
        if order == FrameOrder::Name {
            list.sort();
        }
        list
    }

    /// Follow moved folders ([`Moves`]) without reading capture times.
    /// Returns each moved file's old and new path.
    pub fn follow(&mut self, moves: &Moves) -> HashMap<PathBuf, PathBuf> {
        let mut moved = HashMap::new();
        for file in self.files.iter_mut() {
            if let Some(to) = moves.path(file) {
                moved.insert(file.clone(), to.clone());
                *file = to;
            }
        }
        if !moved.is_empty() {
            self.excluded =
                self.excluded.iter().map(|p| moves.path(p).unwrap_or_else(|| p.clone())).collect();
            self.place();
        }
        moved
    }

    /// Whether [`Self::sort`] reads the disk, so a GUI sorts off the UI
    /// thread.
    pub fn sorts_from_disk(&self) -> bool {
        self.order == FrameOrder::CaptureTime
    }

    /// Put [`Self::files`] in the list's order; capture order reads the
    /// stacked files' times from disk.
    pub fn sort(&mut self) {
        self.sort_with(crate::frames::order::time_key);
    }

    /// Order [`Self::files`] without reading the disk; in capture order,
    /// folders sort and each keeps its files' order.
    fn place(&mut self) {
        match self.order {
            FrameOrder::Name => self.sort_with(|_| TimeKey::default()),
            FrameOrder::CaptureTime => self
                .files
                .sort_by(|a, b| natural_path_cmp(a.parent().unwrap_or(a), b.parent().unwrap_or(b))),
        }
    }

    pub(crate) fn sort_with(&mut self, time: impl Fn(&Path) -> TimeKey) {
        self.files.sort_by(|a, b| {
            let (fa, fb) = (a.parent().unwrap_or(a), b.parent().unwrap_or(b));
            natural_path_cmp(fa, fb).then_with(|| natural_path_cmp(a, b))
        });
        if self.order == FrameOrder::Name {
            return;
        }
        let mut files = Vec::with_capacity(self.files.len());
        for (_, shots) in self.folders() {
            let paths: Vec<PathBuf> =
                shots.iter().map(|s| s.file(self.source).to_path_buf()).collect();
            let keys: Vec<TimeKey> = paths.iter().map(|p| time(p)).collect();
            for i in capture_order(&paths, &keys).0 {
                files.extend(shots[i].files.iter().cloned());
            }
        }
        self.files = files;
    }
}

/// Why Stack can't run, shown under Stack or in its alert.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StackProblem {
    /// The list has no frames.
    Empty,
    /// Fewer than two frames are included.
    TooFew,
    /// Included frames missing or on a disconnected volume.
    Missing(usize),
    /// Kinds the engine won't stack together under this decoder
    /// (`check_same_kind`).
    MixedKinds(RawBackend),
}

impl StackProblem {
    /// The sentence that says it.
    pub fn text(self, words: &Words) -> String {
        match self {
            Self::Empty => match words.platform() {
                Platform::Ios => say!(words, "stack-problem-empty-ios"),
                Platform::Desktop | Platform::CommandLine => say!(words, "stack-problem-empty"),
            },
            Self::TooFew => say!(words, "stack-problem-too-few"),
            Self::Missing(n) => say!(words, "stack-problem-missing", n = n),
            // Under Apple's decoder only a linear DNG stays linear (ADR-131).
            Self::MixedKinds(RawBackend::Apple) => say!(words, "stack-problem-mixed-linear-dng"),
            Self::MixedKinds(RawBackend::LibRaw | RawBackend::AdobeDng) => {
                say!(words, "stack-problem-mixed-raw")
            }
        }
    }
}

/// Why `included` (of `listed`, `missing` unreadable) can't be stacked
/// under `backend`. The kinds check, which reads DNG headers, runs last.
pub fn stack_problem(
    listed: usize,
    included: &[PathBuf],
    missing: usize,
    backend: RawBackend,
) -> Option<StackProblem> {
    if listed == 0 {
        Some(StackProblem::Empty)
    } else if included.len() < 2 {
        Some(StackProblem::TooFew)
    } else if missing > 0 {
        Some(StackProblem::Missing(missing))
    } else if check_same_kind(included, backend).is_err() {
        Some(StackProblem::MixedKinds(backend))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use super::*;
    use crate::library::history::{Moves, Render};
    use crate::{PyramidConfig, RenderId, SessionOptions, StackerConfig};

    #[test]
    fn stack_says_what_stops_it_first() {
        use StackProblem::*;
        let raw = RawBackend::LibRaw;
        let paths = |names: &[&str]| names.iter().map(PathBuf::from).collect::<Vec<_>>();
        let two = paths(&["a.jpg", "b.jpg"]);
        assert_eq!(stack_problem(0, &[], 0, raw), Some(Empty));
        assert_eq!(stack_problem(3, &[], 0, raw), Some(TooFew));
        assert_eq!(stack_problem(3, &two[..1], 1, raw), Some(TooFew));
        assert_eq!(stack_problem(3, &two, 1, raw), Some(Missing(1)));
        assert_eq!(stack_problem(2, &two, 0, raw), None);
        let mixed = paths(&["a.cr2", "b.jpg"]);
        assert_eq!(stack_problem(2, &mixed, 0, raw), Some(MixedKinds(raw)));
        assert_eq!(stack_problem(2, &mixed, 2, raw), Some(Missing(2)), "missing comes first");
        let w = Words::english(Platform::Desktop);
        assert_eq!(Missing(2).text(&w), "2 included frames can't be found.");
        assert_eq!(Empty.text(&w), "Add frames to stack.");
        let ios = Words::english(Platform::Ios);
        assert_eq!(Empty.text(&ios), "Add frames, or take a burst.");
    }

    fn touch(dir: &Path, names: &[&str]) -> Vec<PathBuf> {
        names
            .iter()
            .map(|n| {
                let path = dir.join(n);
                std::fs::write(&path, b"").unwrap();
                path
            })
            .collect()
    }

    #[test]
    fn folders_and_files_add_once_in_natural_order() {
        let root = tempfile::tempdir().unwrap();
        let (a, b) = (root.path().join("a"), root.path().join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(a.join("inner")).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        touch(&a, &["P10.jpg", "P9.jpg", "notes.txt"]);
        touch(&a.join("inner"), &["X1.jpg"]);
        let b_files = touch(&b, &["B1.jpg"]);

        let mut list = List::default();
        assert_eq!(list.add([b_files[0].clone(), a.clone()]), 3);
        assert_eq!(list.add([a.join("P9.jpg"), a.join("notes.txt")]), 0);
        let names: Vec<_> =
            list.files.iter().map(|f| f.strip_prefix(root.path()).unwrap()).collect();
        assert_eq!(names, [Path::new("a/P9.jpg"), Path::new("a/P10.jpg"), Path::new("b/B1.jpg")]);
    }

    #[test]
    fn excluding_a_pair_excludes_both_files() {
        let mut list = List {
            files: vec!["/f/P1.ORF".into(), "/f/P1.JPG".into(), "/f/P2.ORF".into()],
            ..List::default()
        };
        let shots = list.shots();
        assert_eq!(shots.len(), 2);
        list.set_included(&shots[0], false);
        assert_eq!(list.frames(), [("/f/P1.ORF".into(), false), ("/f/P2.ORF".into(), true)]);
        list.source = Source::Standard;
        assert_eq!(list.frames()[0], ("/f/P1.JPG".into(), false));
        list.set_included(&shots[0], true);
        assert!(list.excluded.is_empty());

        list.remove(&list.shots()[..1]);
        assert_eq!(list.files, [PathBuf::from("/f/P2.ORF")]);
    }

    #[test]
    fn adding_unsorted_keeps_the_capture_order_until_sorted() {
        use crate::frames::capture::CaptureTime;
        // Shot in the order of the number after the name's dash.
        let key = |p: &Path| {
            let stem = p.file_stem().unwrap().to_str().unwrap();
            let n: u32 = stem.split('-').nth(1).unwrap().parse().unwrap();
            TimeKey {
                time: Some(CaptureTime { seconds: 1, micros: Some(n) }),
                model: None,
                shot: None,
            }
        };
        let paths = |names: &[&str]| -> Vec<PathBuf> { names.iter().map(PathBuf::from).collect() };
        let mut list = List {
            files: paths(&["/a/b-1.jpg", "/a/a-2.jpg", "/c/x-1.jpg"]),
            order: FrameOrder::CaptureTime,
            ..List::default()
        };
        assert!(list.sorts_from_disk());
        // Unsorted, new files go after their folder's.
        let added = list.add_files_unsorted(paths(&["/b/y-1.jpg", "/a/c-0.jpg", "/a/b-1.jpg"]));
        assert_eq!(added, 2);
        assert_eq!(
            list.files,
            paths(&["/a/b-1.jpg", "/a/a-2.jpg", "/a/c-0.jpg", "/b/y-1.jpg", "/c/x-1.jpg"])
        );
        list.sort_with(key);
        assert_eq!(
            list.files,
            paths(&["/a/c-0.jpg", "/a/b-1.jpg", "/a/a-2.jpg", "/b/y-1.jpg", "/c/x-1.jpg"])
        );

        // In name order they go in their places at once.
        let mut list = List { files: paths(&["/a/2.jpg"]), ..List::default() };
        assert!(!list.sorts_from_disk());
        list.add_files_unsorted(paths(&["/a/10.jpg", "/a/1.jpg"]));
        assert_eq!(list.files, paths(&["/a/1.jpg", "/a/2.jpg", "/a/10.jpg"]));
    }

    #[test]
    fn following_a_move_takes_a_folder_whole_and_says_what_moved() {
        let (old, new, other) = (Path::new("/z-card"), Path::new("/a-Bee"), Path::new("/m"));
        // In capture order, 2 was shot before 1.
        let mut list = List {
            files: vec![other.join("1.jpg"), old.join("2.jpg"), old.join("1.jpg")],
            excluded: [old.join("1.jpg")].into(),
            order: FrameOrder::CaptureTime,
            ..List::default()
        };
        let moves = Moves::new(vec![(old.to_path_buf(), new.to_path_buf())]);
        let moved = list.follow(&moves);
        assert_eq!(moved.len(), 2);
        assert_eq!(moved[&old.join("2.jpg")], new.join("2.jpg"));
        assert_eq!(list.files, [new.join("2.jpg"), new.join("1.jpg"), other.join("1.jpg")]);
        assert_eq!(list.excluded, [new.join("1.jpg")].into());
        assert!(list.follow(&moves).is_empty());
    }

    #[test]
    fn a_render_of_pairs_loads_back_both_files_of_each() {
        let render = Render {
            frames: vec![
                "/f/P1.ORF".into(),
                "/f/P1.JPG".into(),
                "/f/P2.ORF".into(),
                "/f/P2.JPG".into(),
                "/f/P3.ORF".into(),
                "/f/P3.JPG".into(),
            ],
            included: vec![true, false, false, false, true, false],
            ..render_of(&[])
        };
        let list = List::from_render(&render);
        assert_eq!(list.files.len(), 6);
        assert_eq!(list.source, Source::Raw);
        assert_eq!(
            list.frames(),
            [("/f/P1.ORF".into(), true), ("/f/P2.ORF".into(), false), ("/f/P3.ORF".into(), true)]
        );
    }

    fn render_of(frames: &[(&str, bool)]) -> Render {
        Render {
            id: RenderId::parse(&"1".repeat(32)).unwrap(),
            created: SystemTime::UNIX_EPOCH,
            name: None,
            stack: crate::library::history::StackId::parse(&"1".repeat(32)).unwrap(),
            number: 1,
            config: StackerConfig::Pyramid(PyramidConfig::default()),
            options: SessionOptions::default(),
            frames: frames.iter().map(|(f, _)| PathBuf::from(f)).collect(),
            included: frames.iter().map(|(_, i)| *i).collect(),
            batch: None,
            picked: false,
            width: 1,
            height: 1,
            linear: false,
            left_out: Vec::new(),
        }
    }

    #[test]
    fn a_render_loads_back_its_frames_and_exclusions() {
        let render = render_of(&[("/f/2.jpg", true), ("/f/1.jpg", true), ("/f/3.jpg", false)]);
        let list = List::from_render(&render);
        assert_eq!(
            list.frames(),
            [("/f/1.jpg".into(), true), ("/f/2.jpg".into(), true), ("/f/3.jpg".into(), false)]
        );
        assert_eq!(list.source, Source::Standard);
    }

    #[test]
    fn capture_order_sorts_each_folder_and_a_render_keeps_its_order() {
        use crate::frames::capture::CaptureTime;
        let words = Words::english(crate::Platform::Desktop);
        // In each folder, 9 was shot last.
        let key = |p: &Path| {
            let n: u32 = p.file_stem().unwrap().to_str().unwrap().parse().unwrap();
            let micros = Some(if n == 9 { 999_999 } else { n });
            TimeKey { time: Some(CaptureTime { seconds: 1, micros }), model: None, shot: None }
        };
        let mut list = List {
            files: ["/a/9.jpg", "/a/10.jpg", "/a/11.jpg", "/b/9.jpg", "/b/10.jpg"]
                .iter()
                .map(PathBuf::from)
                .collect(),
            order: FrameOrder::CaptureTime,
            ..List::default()
        };
        list.sort_with(key);
        let frames: Vec<PathBuf> = list.frames().into_iter().map(|(f, _)| f).collect();
        let want: Vec<PathBuf> = ["/a/10.jpg", "/a/11.jpg", "/a/9.jpg", "/b/10.jpg", "/b/9.jpg"]
            .iter()
            .map(PathBuf::from)
            .collect();
        assert_eq!(frames, want);
        assert!(list.order_notes_with(key, &words).is_empty());
        list.order = FrameOrder::Name;
        // b is exactly reversed, which the stacker handles: no note.
        assert_eq!(
            list.order_notes_with(key, &words),
            ["a: the frames were shot in a different order than their names"]
        );

        // A render made in that order loads back in it, not by name.
        let mut render = render_of(&[("/a/10.jpg", true), ("/a/11.jpg", true), ("/a/9.jpg", true)]);
        render.options.order = FrameOrder::CaptureTime;
        let list = List::from_render(&render);
        let frames: Vec<PathBuf> = list.frames().into_iter().map(|(f, _)| f).collect();
        assert_eq!(frames, &want[..3]);
        assert_eq!(list.order, FrameOrder::CaptureTime);
    }
}
