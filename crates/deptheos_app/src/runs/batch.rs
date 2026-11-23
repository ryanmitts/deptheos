//! Batch: what renders one run makes.
//!
//! A run makes one render per folder (or the list), or auto sorts each
//! folder into stacks by capture time, never mixing folders. Images it
//! can't place, the minority kind of a stack mixing RAW and other files,
//! and folders too small to stack are skipped and counted. Every render
//! uses the run's starting settings; a folder given twice is planned once.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub mod machine;
pub mod sources;

pub use machine::{
    Action, End, Event, JobNo, Machine, Phase, SequenceEnd, SequenceOutcome, SequenceStack,
    StackKind, StackNo, Summary, export_failure, stack_failure,
};

use crate::FrameKind;
use crate::RawBackend;
use crate::frames::autosort::{self, ShotTime, SplitRule};
use crate::frames::capture::CaptureInfo;
use crate::frames::folder::{self, Shot, Source};
use crate::frames::list::List;
use crate::frames::names::{file_label, natural_path_cmp, range_name};
use crate::frames::order::{FrameOrder, TimeKey};
use crate::words::{SEPARATOR, Words, say};

/// The fewest images a render is made from.
pub(crate) const MIN_FRAMES: usize = 2;

/// Where a run's images come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input {
    /// The images directly inside a folder.
    Folder(PathBuf),
    /// The list's included images, as one source, named `label`.
    List { list: List, label: String },
}

impl Input {
    /// What the run's summary and render names call it.
    pub fn name(&self) -> String {
        match self {
            Self::Folder(path) => file_label(path),
            Self::List { label, .. } => label.clone(),
        }
    }
}

/// How a run splits each input; render files name it by its serde spelling.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Split {
    /// One render per input, of all its images.
    #[default]
    PerFolder,
    /// Each input split into stacks by capture time, one render each.
    AutoSort,
}

impl Split {
    pub const ALL: [Self; 2] = [Self::PerFolder, Self::AutoSort];

    /// The run's header in the Library.
    pub fn label(self, words: &Words) -> String {
        match self {
            Self::PerFolder => say!(words, "split-per-folder"),
            Self::AutoSort => say!(words, "split-auto-sort"),
        }
    }
}

/// One render to make.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    /// What it is called while it runs and if it fails.
    pub label: String,
    /// The files to stack, in order, all included.
    pub frames: Vec<PathBuf>,
}

/// What a run will do.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub split: Split,
    pub jobs: Vec<Job>,
    /// How many folders it looked at, besides the list.
    pub folders: usize,
    /// Whether the list was one of its inputs.
    pub list: bool,
    /// How many images it leaves out.
    pub skipped: usize,
    /// How many inputs (or folders of the list) had too few images.
    pub too_few: usize,
    /// The folders it couldn't read, each with why.
    pub unreadable: Vec<(PathBuf, String)>,
    /// The folders its frames are in, links followed, each once, for
    /// [`Plan::frames_folder_holding`].
    pub frame_folders: Vec<PathBuf>,
    /// The order each render's frames are in.
    pub order: FrameOrder,
    /// A line per stack whose order needs a word: shot out of name order,
    /// or why it stayed in name order, or of its times.
    pub order_notes: Vec<String>,
}

impl Plan {
    /// "1 folder, 312 frames → 14 stacks, 9 frames skipped": inputs,
    /// outputs, then what it leaves out.
    pub fn summary(&self, words: &Words) -> String {
        let w = words;
        let frames: usize = self.jobs.iter().map(|j| j.frames.len()).sum::<usize>() + self.skipped;
        let from = match (self.folders, self.list, self.split) {
            (0, true, Split::PerFolder) => say!(w, "plan-from-list"),
            (0, true, Split::AutoSort) => say!(w, "plan-from-list-frames", frames = frames),
            (n, true, Split::PerFolder) => say!(w, "plan-from-list-and-folders", folders = n),
            (n, true, Split::AutoSort) => {
                say!(w, "plan-from-list-and-folders-frames", folders = n, frames = frames)
            }
            (n, false, Split::PerFolder) => say!(w, "plan-from-folders", folders = n),
            (n, false, Split::AutoSort) => {
                say!(w, "plan-from-folders-frames", folders = n, frames = frames)
            }
        };
        let made = self.jobs.len();
        let mut line = match self.split {
            Split::PerFolder => say!(w, "plan-to-renders", from = from, renders = made),
            Split::AutoSort => say!(w, "plan-to-stacks", from = from, stacks = made),
        };
        let mut also = Vec::new();
        if self.skipped > 0 {
            also.push(say!(w, "plan-skipped", count = self.skipped));
        }
        if self.too_few > 0 {
            also.push(say!(w, "plan-too-few", count = self.too_few));
        }
        if !self.unreadable.is_empty() {
            also.push(say!(w, "plan-unreadable", count = self.unreadable.len()));
        }
        for part in also {
            line = say!(w, "plan-and", line = line, part = part);
        }
        line
    }

    /// The run's header in the Library: "Auto sort · 9 frames skipped".
    pub fn label(&self, words: &Words) -> String {
        label(self.split, self.skipped, words)
    }

    /// The frames' folder that `folder` is or is inside, links followed: a
    /// batch refuses it for exports, which would be taken for frames next
    /// time ([`export_in_frames_text`]).
    pub fn frames_folder_holding(&self, folder: &Path) -> Option<PathBuf> {
        let folder = crate::library::history::frames::canonical_under(folder)?;
        self.frame_folders.iter().find(|frames| folder.starts_with(frames)).cloned()
    }
}

/// Why a batch won't export into `input`, a folder of its frames
/// ([`Plan::frames_folder_holding`]).
pub fn export_in_frames_text(words: &Words, input: &Path) -> String {
    let folder = crate::words::explain::quoted(&file_label(input), words);
    say!(words, "batch-export-in-frames", folder = folder)
}

/// A run's header in the Library: "Auto sort · 9 frames skipped".
pub(crate) fn label(split: Split, skipped: usize, words: &Words) -> String {
    let split = split.label(words);
    match skipped {
        0 => split,
        n => say!(words, "plan-label-skipped", split = split, count = n),
    }
}

/// The run `inputs` split by `split` would make: each pair's file as
/// `source` says, each render's frames in `order` and of one [`FrameKind`]
/// under `backend`. `read` reads a file's capture info ([`capture_info`]
/// outside tests), at most once per file.
pub fn plan(
    inputs: &[Input],
    split: Split,
    source: Source,
    order: FrameOrder,
    backend: RawBackend,
    read: impl Fn(&Path) -> CaptureInfo,
    words: &Words,
) -> Plan {
    let read = Cached { read, infos: RefCell::default() };
    let time = |path: &Path| ShotTime::of(path, read.info(path));
    let key = |path: &Path| TimeKey::from(read.info(path));
    let mut plan = Plan { split, order, ..Plan::default() };
    for (name, shots, source) in sources(&mut plan, inputs, split, source) {
        let stacks = split_source(&mut plan, name, &shots, source, &time);
        jobs_of(&mut plan, stacks, backend, &key, words);
    }
    let mut parents: Vec<&Path> =
        plan.jobs.iter().flat_map(|job| &job.frames).filter_map(|f| f.parent()).collect();
    parents.sort();
    parents.dedup();
    let mut folders: Vec<PathBuf> =
        parents.into_iter().filter_map(crate::library::history::frames::canonical_under).collect();
    folders.sort();
    folders.dedup();
    plan.frame_folders = folders;
    plan
}

/// [`plan`]'s reader: `path`'s capture info, or none.
pub fn capture_info(path: &Path) -> CaptureInfo {
    crate::frames::capture::read_capture_info(path).unwrap_or_default()
}

/// A plan's reader, reading each file once.
struct Cached<R> {
    read: R,
    infos: RefCell<HashMap<PathBuf, CaptureInfo>>,
}

impl<R: Fn(&Path) -> CaptureInfo> Cached<R> {
    fn info(&self, path: &Path) -> CaptureInfo {
        if let Some(info) = self.infos.borrow().get(path) {
            return info.clone();
        }
        let info = (self.read)(path);
        self.infos.borrow_mut().insert(path.to_path_buf(), info.clone());
        info
    }
}

/// Each source in `inputs` with its name, shots and pair choice; counts
/// folders, unreadable ones and the list into `plan`.
fn sources(
    plan: &mut Plan,
    inputs: &[Input],
    split: Split,
    source: Source,
) -> Vec<(String, Vec<Shot>, Source)> {
    let mut seen = std::collections::HashSet::new();
    let mut sources: Vec<(String, Vec<Shot>, Source)> = Vec::new();
    for input in inputs {
        match input {
            Input::Folder(path) => {
                if !seen.insert(path.clone()) {
                    continue;
                }
                plan.folders += 1;
                let files = match folder::list_folder(path) {
                    Ok(files) => files,
                    Err(e) => {
                        plan.unreadable.push((path.clone(), e.to_string()));
                        continue;
                    }
                };
                let shots = folder::shots(files);
                sources.push((input.name(), shots, source));
            }
            Input::List { list, label } => {
                plan.list = true;
                let shots: Vec<Shot> =
                    list.shots().into_iter().filter(|s| list.is_included(s)).collect();
                if split == Split::AutoSort {
                    // Auto sort splits each folder of the list on its own.
                    for (folder, shots) in by_folder(shots) {
                        sources.push((
                            format!("{label}{SEPARATOR}{}", file_label(&folder)),
                            shots,
                            source,
                        ));
                    }
                } else {
                    sources.push((label.clone(), shots, source));
                }
            }
        }
    }
    sources
}

/// The named stacks `shots` split into by `plan.split`; unplaced shots
/// count as skipped.
fn split_source(
    plan: &mut Plan,
    name: String,
    shots: &[Shot],
    source: Source,
    time: &impl Fn(&Path) -> ShotTime,
) -> Vec<(String, Vec<PathBuf>)> {
    let file = |shot: &Shot| shot.file(source).to_path_buf();
    match plan.split {
        Split::PerFolder => vec![(name, shots.iter().map(file).collect())],
        Split::AutoSort => {
            let times: Vec<ShotTime> = shots.iter().map(|s| time(s.file(source))).collect();
            let rule = SplitRule {
                min_shots: SplitRule::default().min_shots.max(MIN_FRAMES),
                ..SplitRule::default()
            };
            let stacks = autosort::split(shots, &times, &rule);
            let placed: usize = stacks.iter().map(Vec::len).sum();
            plan.skipped += shots.len() - placed;
            stacks
                .into_iter()
                .map(|stack| {
                    let (first, last) =
                        (&shots[stack[0]].stem, &shots[stack[stack.len() - 1]].stem);
                    let label = format!("{name}{SEPARATOR}{}", range_name(first, last));
                    (label, stack.iter().map(|&i| file(&shots[i])).collect())
                })
                .collect()
        }
    }
}

/// One source's `stacks` as jobs, of one kind and in `plan.order`; a stack
/// left too small is skipped, a source with no job counts as too few.
fn jobs_of(
    plan: &mut Plan,
    stacks: Vec<(String, Vec<PathBuf>)>,
    backend: RawBackend,
    key: &impl Fn(&Path) -> TimeKey,
    words: &Words,
) {
    let mut made = false;
    for (label, frames) in stacks {
        let (frames, odd) = one_kind(frames, backend);
        plan.skipped += odd;
        if frames.len() < MIN_FRAMES {
            plan.skipped += frames.len();
            continue;
        }
        let (frames, note) =
            crate::frames::order::order_stack(&label, frames, plan.order, key, words);
        plan.order_notes.extend(note);
        plan.jobs.push(Job { label, frames });
        made = true;
    }
    if !made {
        plan.too_few += 1;
    }
}

/// Put a turntable run's folders in natural path order (`s001` … `s050`),
/// the list first.
pub fn sort_folders(inputs: &mut [Input]) {
    inputs.sort_by(|a, b| match (a, b) {
        (Input::Folder(a), Input::Folder(b)) => natural_path_cmp(a, b),
        (Input::List { .. }, Input::Folder(_)) => std::cmp::Ordering::Less,
        (Input::Folder(_), Input::List { .. }) => std::cmp::Ordering::Greater,
        _ => std::cmp::Ordering::Equal,
    });
}

/// `shots` by the folder of their files, folders in order of first shot.
fn by_folder(shots: Vec<Shot>) -> Vec<(PathBuf, Vec<Shot>)> {
    let mut folders: Vec<(PathBuf, Vec<Shot>)> = Vec::new();
    for shot in shots {
        let folder = shot.files[0].parent().unwrap_or(Path::new("")).to_path_buf();
        match folders.iter_mut().find(|(f, _)| *f == folder) {
            Some((_, shots)) => shots.push(shot),
            None => folders.push((folder, vec![shot])),
        }
    }
    folders
}

/// `frames` of the more common [`FrameKind`] only (RAW on a tie), and how
/// many were left out: the session refuses a mix.
fn one_kind(frames: Vec<PathBuf>, backend: RawBackend) -> (Vec<PathBuf>, usize) {
    let kinds: Vec<FrameKind> = frames.iter().map(|f| FrameKind::of(f, backend)).collect();
    let raw = kinds.iter().filter(|&&k| k == FrameKind::Raw).count();
    let keep = if raw * 2 >= frames.len() { FrameKind::Raw } else { FrameKind::Standard };
    let before = frames.len();
    let kept: Vec<PathBuf> =
        frames.into_iter().zip(kinds).filter(|(_, k)| *k == keep).map(|(f, _)| f).collect();
    let odd = before - kept.len();
    (kept, odd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frames::capture::CaptureTime;

    fn words() -> Words {
        Words::english(crate::Platform::Desktop)
    }

    /// An export folder in a frames folder is found; one beside or above
    /// is not.
    #[test]
    fn an_export_folder_in_a_folder_of_frames_is_found() {
        let dir = tempfile::tempdir().unwrap();
        let s1 = folder(dir.path(), "shoot/s1", &["a.png", "b.png"]);
        let s2 = folder(dir.path(), "shoot/s2", &["a.png", "b.png"]);
        let inputs = [Input::Folder(s1.clone()), Input::Folder(s2.clone())];
        let plan = plan(
            &inputs,
            Split::PerFolder,
            Source::Raw,
            FrameOrder::Name,
            RawBackend::LibRaw,
            |_| CaptureInfo::default(),
            &words(),
        );
        assert_eq!(plan.jobs.len(), 2);
        let (s1, s2) = (s1.canonicalize().unwrap(), s2.canonicalize().unwrap());
        assert_eq!(plan.frames_folder_holding(&s2), Some(s2.clone()));
        assert_eq!(plan.frames_folder_holding(&s1.join("out")), Some(s1.clone()), "not made yet");
        assert_eq!(plan.frames_folder_holding(&s1.join("out/deeper")), Some(s1.clone()));
        assert_eq!(plan.frames_folder_holding(&dir.path().join("shoot")), None, "above them");
        assert_eq!(plan.frames_folder_holding(&dir.path().join("shoot/s10")), None, "beside");
        assert_eq!(plan.frames_folder_holding(&dir.path().join("out")), None);
        #[cfg(unix)]
        {
            let link = dir.path().join("link");
            std::os::unix::fs::symlink(&s1, &link).unwrap();
            assert_eq!(plan.frames_folder_holding(&link.join("out")), Some(s1.clone()));
        }
        assert_eq!(
            export_in_frames_text(&words(), &s1),
            "The exports can't go into \"s1\", which holds frames this batch stacks. Choose a \
             folder outside the folders being stacked."
        );
    }

    fn folder(root: &Path, name: &str, files: &[&str]) -> PathBuf {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        for f in files {
            std::fs::write(dir.join(f), b"").unwrap();
        }
        dir
    }

    /// Capture times from the name: `n0100` was shot at 100 s.
    fn by_name(path: &Path) -> CaptureInfo {
        let stem = path.file_stem().unwrap().to_str().unwrap();
        let seconds = stem.trim_start_matches(|c: char| !c.is_ascii_digit()).parse().ok();
        CaptureInfo {
            time: seconds.map(|seconds| CaptureTime { seconds, micros: None }),
            model: Some("OM-1".into()),
            ..CaptureInfo::default()
        }
    }

    fn no_info(_: &Path) -> CaptureInfo {
        CaptureInfo::default()
    }

    fn names(frames: &[PathBuf]) -> Vec<&str> {
        frames.iter().map(|f| f.file_name().unwrap().to_str().unwrap()).collect()
    }

    /// Auto sort reads the time from the file it will stack.
    #[test]
    fn auto_sort_reads_the_time_of_the_file_it_stacks() {
        let root = tempfile::tempdir().unwrap();
        let files: Vec<String> =
            (0..6).flat_map(|i| [format!("n{i:04}.ORF"), format!("n{i:04}.JPG")]).collect();
        let files: Vec<&str> = files.iter().map(String::as_str).collect();
        let inputs = [Input::Folder(folder(root.path(), "Bee", &files))];
        for (source, ext) in [(Source::Raw, "ORF"), (Source::Standard, "JPG")] {
            let time = |path: &Path| {
                assert_eq!(path.extension().unwrap(), ext, "read {}", path.display());
                by_name(path)
            };
            let plan = plan(
                &inputs,
                Split::AutoSort,
                source,
                FrameOrder::Name,
                RawBackend::LibRaw,
                time,
                &words(),
            );
            assert_eq!(plan.jobs.len(), 1);
            assert!(plan.jobs[0].frames.iter().all(|f| f.extension().unwrap() == ext));
        }
    }

    #[test]
    fn one_render_per_folder_and_too_few_are_named() {
        let root = tempfile::tempdir().unwrap();
        let bee = folder(root.path(), "Bee", &["b1.jpg", "b2.jpg", "b3.jpg"]);
        let ant = folder(root.path(), "Ant", &["a1.jpg"]);
        let inputs = [Input::Folder(bee), Input::Folder(ant)];
        let plan = plan(
            &inputs,
            Split::PerFolder,
            Source::Raw,
            FrameOrder::Name,
            RawBackend::LibRaw,
            no_info,
            &words(),
        );
        assert_eq!(plan.jobs.len(), 1);
        assert_eq!(plan.jobs[0].label, "Bee");
        assert_eq!(names(&plan.jobs[0].frames), ["b1.jpg", "b2.jpg", "b3.jpg"]);
        assert_eq!(plan.too_few, 1);
        assert_eq!(
            plan.summary(&words()),
            "2 folders → 1 render, 1 frame skipped, 1 folder with too few frames"
        );
    }

    #[test]
    fn an_unreadable_folder_is_named_not_counted_empty() {
        let root = tempfile::tempdir().unwrap();
        let bee = folder(root.path(), "Bee", &["b1.jpg", "b2.jpg", "b3.jpg"]);
        let gone = root.path().join("Gone");
        let inputs = [Input::Folder(bee), Input::Folder(gone.clone())];
        let plan = plan(
            &inputs,
            Split::PerFolder,
            Source::Raw,
            FrameOrder::Name,
            RawBackend::LibRaw,
            no_info,
            &words(),
        );
        assert_eq!(plan.jobs.len(), 1);
        assert_eq!(plan.too_few, 0);
        assert_eq!(plan.unreadable.len(), 1);
        assert_eq!(plan.unreadable[0].0, gone);
        assert_eq!(plan.summary(&words()), "2 folders → 1 render, 1 folder unreadable");
    }

    #[test]
    fn auto_sort_splits_each_folder_and_skips_strays() {
        let root = tempfile::tempdir().unwrap();
        let mut files: Vec<String> = (0..6).map(|i| format!("n{:04}.jpg", 100 + i)).collect();
        files.push("n0300.jpg".into());
        files.extend((0..5).map(|i| format!("n{:04}.jpg", 900 + i)));
        let refs: Vec<&str> = files.iter().map(String::as_str).collect();
        let card = folder(root.path(), "Card", &refs);
        let plan = plan(
            &[Input::Folder(card)],
            Split::AutoSort,
            Source::Raw,
            FrameOrder::Name,
            RawBackend::LibRaw,
            by_name,
            &words(),
        );
        let sizes: Vec<usize> = plan.jobs.iter().map(|j| j.frames.len()).collect();
        assert_eq!(sizes, [6, 5]);
        assert_eq!(plan.jobs[0].label, "Card · n0100–105");
        assert_eq!(plan.skipped, 1);
        assert_eq!(plan.summary(&words()), "1 folder, 12 frames → 2 stacks, 1 frame skipped");
    }

    /// A file's capture info is read once.
    #[test]
    fn auto_sort_reads_each_file_once() {
        let root = tempfile::tempdir().unwrap();
        let files: Vec<String> = (0..6).map(|i| format!("n{:04}.jpg", 100 + i)).collect();
        let refs: Vec<&str> = files.iter().map(String::as_str).collect();
        let card = folder(root.path(), "Card", &refs);
        let reads = std::cell::Cell::new(0);
        let read = |path: &Path| {
            reads.set(reads.get() + 1);
            by_name(path)
        };
        for order in [FrameOrder::Name, FrameOrder::CaptureTime] {
            reads.set(0);
            let inputs = [Input::Folder(card.clone())];
            let plan = plan(
                &inputs,
                Split::AutoSort,
                Source::Raw,
                order,
                RawBackend::LibRaw,
                read,
                &words(),
            );
            assert_eq!(plan.jobs[0].frames.len(), 6);
            assert_eq!(reads.get(), 6);
        }
    }

    #[test]
    fn the_list_source_leaves_out_excluded_images() {
        let mut list = List {
            files: (0..4).map(|i| PathBuf::from(format!("/x/p{i}.jpg"))).collect(),
            ..List::default()
        };
        let shots = list.shots();
        list.set_included(&shots[3], false);
        let input = Input::List { list, label: "The list".into() };
        let plan = plan(
            &[input],
            Split::PerFolder,
            Source::Raw,
            FrameOrder::Name,
            RawBackend::LibRaw,
            no_info,
            &words(),
        );
        assert_eq!(names(&plan.jobs[0].frames), ["p0.jpg", "p1.jpg", "p2.jpg"]);
        assert_eq!(plan.jobs[0].label, "The list");
        assert_eq!(plan.summary(&words()), "The list → 1 render");
    }

    #[test]
    fn auto_sort_never_mixes_the_lists_folders() {
        let root = tempfile::tempdir().unwrap();
        // Two cards shot at once: the same times, different folders.
        let names_of =
            |p: &str| (0..6).map(|i| format!("{p}{:04}.jpg", 100 + i)).collect::<Vec<_>>();
        let (a, b) = (names_of("a"), names_of("b"));
        let one = folder(root.path(), "One", &a.iter().map(String::as_str).collect::<Vec<_>>());
        let two = folder(root.path(), "Two", &b.iter().map(String::as_str).collect::<Vec<_>>());
        let mut list = List::default();
        list.add([one, two]);
        let input = Input::List { list, label: "The list".into() };
        let plan = plan(
            &[input],
            Split::AutoSort,
            Source::Raw,
            FrameOrder::Name,
            RawBackend::LibRaw,
            by_name,
            &words(),
        );
        let labels: Vec<&str> = plan.jobs.iter().map(|j| j.label.as_str()).collect();
        assert_eq!(labels, ["The list · One · a0100–105", "The list · Two · b0100–105"]);
    }

    #[test]
    fn a_stray_of_another_kind_is_skipped_and_a_folder_counts_once() {
        let root = tempfile::tempdir().unwrap();
        let bee = folder(root.path(), "Bee", &["b1.orf", "b2.orf", "b3.orf", "result.jpg"]);
        let inputs = [Input::Folder(bee.clone()), Input::Folder(bee)];
        let plan = plan(
            &inputs,
            Split::PerFolder,
            Source::Raw,
            FrameOrder::Name,
            RawBackend::LibRaw,
            no_info,
            &words(),
        );
        assert_eq!(plan.jobs.len(), 1);
        assert_eq!(names(&plan.jobs[0].frames), ["b1.orf", "b2.orf", "b3.orf"]);
        assert_eq!(plan.summary(&words()), "1 folder → 1 render, 1 frame skipped");
        assert_eq!(plan.label(&words()), "One stack per folder · 1 frame skipped");
    }

    #[test]
    fn capture_order_sorts_each_render_and_name_order_warns() {
        let root = tempfile::tempdir().unwrap();
        let s036 = folder(root.path(), "s036", &["IMG_8.JPG", "IMG_9.JPG", "IMG_10.JPG"]);
        // IMG_10 was shot first (the camera's handles wrapped).
        let key = |p: &Path| {
            let n: i64 = p.file_stem().unwrap().to_str().unwrap()[4..].parse().unwrap();
            let micros = Some(if n == 10 { 0 } else { n as u32 * 1000 });
            CaptureInfo { time: Some(CaptureTime { seconds: 1, micros }), ..CaptureInfo::default() }
        };
        let input = [Input::Folder(s036)];
        let by_name = plan(
            &input,
            Split::PerFolder,
            Source::Raw,
            FrameOrder::Name,
            RawBackend::LibRaw,
            key,
            &words(),
        );
        assert_eq!(names(&by_name.jobs[0].frames), ["IMG_8.JPG", "IMG_9.JPG", "IMG_10.JPG"]);
        assert_eq!(
            by_name.order_notes,
            ["s036: the frames were shot in a different order than their names"]
        );
        let by_time = plan(
            &input,
            Split::PerFolder,
            Source::Raw,
            FrameOrder::CaptureTime,
            RawBackend::LibRaw,
            key,
            &words(),
        );
        assert_eq!(names(&by_time.jobs[0].frames), ["IMG_10.JPG", "IMG_8.JPG", "IMG_9.JPG"]);
        assert!(by_time.order_notes.is_empty());
    }

    #[test]
    fn turntable_folders_sort_by_name() {
        let mut inputs: Vec<Input> =
            ["/r/s010", "/r/s002", "/r/s001"].iter().map(|p| Input::Folder(p.into())).collect();
        sort_folders(&mut inputs);
        let names: Vec<String> = inputs.iter().map(Input::name).collect();
        assert_eq!(names, ["s001", "s002", "s010"]);
    }

    #[test]
    fn folders_inside_look_one_level_or_all_the_way_down() {
        let root = tempfile::tempdir().unwrap();
        folder(root.path(), "a", &["1.jpg"]);
        folder(root.path(), "a/deep", &["2.jpg"]);
        folder(root.path(), "b", &[]);
        folder(root.path(), "b/inner", &["3.jpg"]);
        folder(root.path(), ".hidden", &["4.jpg"]);
        // With `/` between components on every platform.
        let rel = |v: Vec<PathBuf>| -> Vec<String> {
            v.iter()
                .map(|p| {
                    let rel = p.strip_prefix(root.path()).unwrap();
                    rel.components()
                        .map(|c| c.as_os_str().to_string_lossy())
                        .collect::<Vec<_>>()
                        .join("/")
                })
                .collect()
        };
        assert_eq!(rel(folder::folders_inside(root.path(), false)), ["a"]);
        assert_eq!(rel(folder::folders_inside(root.path(), true)), ["a", "a/deep", "b/inner"]);
    }
}
