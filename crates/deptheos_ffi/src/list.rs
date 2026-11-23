//! The list of images the next Stack is made from, and which file of a RAW
//! + JPEG pair it stacks.

use std::path::PathBuf;
use std::sync::Arc;

use deptheos_app::engine::work::Lane;
use deptheos_app::frames::folder::{self, Source};
use deptheos_app::frames::list::add;
use deptheos_app::frames::list::{Expanded, List};
use deptheos_app::library::history::StackId;
use deptheos_app::{Explain, Platform, RawBackend, Words as AppWords};

use crate::request::{Ask, on_pool};
use crate::{Request, Result, Words, shown};

/// Which file of a RAW + JPEG pair to stack
/// (`deptheos_app::frames::folder::Source`).
#[uniffi::remote(Enum)]
pub enum Source {
    Raw,
    Standard,
}

// ---------------------------------------------------------------- the list

/// The images the next Stack is made from, each shot included or excluded
/// (`deptheos_app::frames::list`). The app keeps it and passes it to the
/// functions below, which return it changed.
#[derive(Clone, Debug, Default, PartialEq, Eq, uniffi::Record)]
pub struct ListInfo {
    /// Every file, both of a pair, in natural order of folder then name.
    pub files: Vec<PathBuf>,
    /// Excluded shots, by any of their files.
    pub excluded: Vec<PathBuf>,
    pub source: Source,
}

// A path crosses as a string: one from the app always is one, and one going
// back is a non-UTF-8 name's lossy form.
uniffi::custom_type!(PathBuf, String, {
    remote,
    lower: |path| path.to_string_lossy().into_owned(),
    try_lift: |path| Ok(PathBuf::from(path)),
});

impl From<&List> for ListInfo {
    fn from(list: &List) -> Self {
        Self {
            files: list.files.clone(),
            excluded: list.excluded.iter().cloned().collect(),
            source: list.source,
        }
    }
}

impl From<ListInfo> for List {
    fn from(list: ListInfo) -> Self {
        Self {
            files: list.files,
            excluded: list.excluded.into_iter().collect(),
            source: list.source,
            // iOS keeps name order: it has no order menu.
            order: deptheos_app::FrameOrder::Name,
        }
    }
}

fn with_list(list: ListInfo, f: impl FnOnce(&mut List)) -> ListInfo {
    let mut list = List::from(list);
    f(&mut list);
    ListInfo::from(&list)
}

/// One row of the list: a shot.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ListRow {
    pub stem: String,
    /// The file a stack reads: of the list's pair source when the shot has
    /// both.
    pub file: PathBuf,
    /// Every file of the shot.
    pub files: Vec<PathBuf>,
    pub included: bool,
    pub is_pair: bool,
}

/// The list's rows, in order.
#[uniffi::export]
pub fn list_rows(list: ListInfo) -> Vec<ListRow> {
    let list = List::from(list);
    list.shots()
        .into_iter()
        .map(|shot| ListRow {
            file: shot.file(list.source).to_path_buf(),
            files: shot.files.clone(),
            included: list.is_included(&shot),
            is_pair: shot.is_pair(),
            stem: shot.stem,
        })
        .collect()
}

/// Add image files and folders (each's images directly inside), read on
/// the pool inside their scope. Files already listed are left as they are.
#[uniffi::export]
pub async fn list_add(
    request: Arc<Request>,
    list: ListInfo,
    paths: Vec<PathBuf>,
) -> Result<ListInfo> {
    let read = move || {
        Ok(with_list(list, |list| {
            list.add(paths);
        }))
    };
    let words = AppWords::english(Platform::Ios);
    on_pool(&words, &request, Lane::Interactive, Ask::Own("list-add"), read).await
}

/// What [`list_expand`] found: the image files, and a sentence per folder
/// that couldn't be read (its name, then why).
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct Listed {
    pub files: Vec<PathBuf>,
    pub unreadable: Vec<String>,
}

/// The image files `paths` stand for (a folder's direct images, an image
/// itself), on the pool inside their scope, for [`add_plan`].
#[uniffi::export]
pub async fn list_expand(
    request: Arc<Request>,
    paths: Vec<PathBuf>,
    words: Arc<Words>,
) -> Result<Listed> {
    let english = AppWords::english(Platform::Ios);
    let read = move || {
        let w = &words.words;
        let expanded = List::expand(paths);
        let unreadable = expanded.unreadable.iter().map(|(folder, e)| {
            let lead = add::unreadable_lead(folder, e, w);
            let why = shown(&e.explain(w), w);
            deptheos_app::words::sentences(w, [lead, why])
        });
        let unreadable = unreadable.collect();
        Ok(Listed { files: expanded.files, unreadable })
    };
    on_pool(&english, &request, Lane::Interactive, Ask::Own("list-expand"), read).await
}

/// What adding `files`, listed for stack `into`, comes to with stack
/// `on_screen` showing `list` (ADR-144). iOS takes nothing from outside the
/// app, so it is always the app's own add.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum AddPlan {
    /// Nothing is added: `message` says why.
    Refused { message: String },
    /// None is new: `message` says so.
    Nothing { message: String },
    /// `list` with the new ones added; with `offer`, say where they went,
    /// with New Stack Instead.
    Add { list: ListInfo, offer: bool },
}

/// What adding `files`, listed for `into`, comes to now (ADR-144). Doesn't
/// block: iOS lists by name, so adding reads nothing.
#[uniffi::export]
pub fn add_plan(
    files: Vec<PathBuf>,
    into: String,
    on_screen: String,
    running: bool,
    list: ListInfo,
    words: Arc<Words>,
) -> AddPlan {
    let w = &words.words;
    let (into, stack) = (StackId::parse(&into), StackId::parse(&on_screen));
    let current = List::from(list);
    let outcome = match (into, stack) {
        (Some(into), Some(stack)) => {
            let now = add::OnScreen { stack: &stack, running, list: &current, has_renders: false };
            let read = Expanded { files, ..Default::default() };
            add::plan(read, &into, now, add::AddedFrom::Window).outcome
        }
        // A stack id that isn't one is never the stack on screen.
        _ => add::Outcome::Refused(add::Refused::OtherStack),
    };
    match outcome {
        add::Outcome::Refused(why) => AddPlan::Refused { message: why.text(w) },
        add::Outcome::Nothing => AddPlan::Nothing { message: add::nothing_text(w) },
        add::Outcome::Add { files, offer } => {
            let mut list = current;
            list.add_files(files);
            AddPlan::Add { list: ListInfo::from(&list), offer }
        }
    }
}

/// Include or exclude the shots that have any of `files`.
#[uniffi::export]
pub fn list_set_included(list: ListInfo, files: Vec<PathBuf>, included: bool) -> ListInfo {
    with_list(list, |list| {
        for shot in shots_with(list, &files) {
            list.set_included(&shot, included);
        }
    })
}

/// Take the shots that have any of `files` out of the list.
#[uniffi::export]
pub fn list_remove(list: ListInfo, files: Vec<PathBuf>) -> ListInfo {
    with_list(list, |list| {
        let shots = shots_with(list, &files);
        list.remove(&shots);
    })
}

/// Empty the list, keeping its pair source.
#[uniffi::export]
pub fn list_clear(list: ListInfo) -> ListInfo {
    with_list(list, List::clear)
}

/// Stack the RAW or the standard file of each RAW + JPEG pair.
#[uniffi::export]
pub fn list_set_source(list: ListInfo, source: Source) -> ListInfo {
    with_list(list, |list| list.source = source)
}

/// A folder a relink moved ([`Engine::locate`], [`Engine::find_again`],
/// [`Engine::take_copy`], …): a frame under `old` is under `new` now.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct FolderMove {
    pub old: PathBuf,
    pub new: PathBuf,
}

pub(crate) fn moves(moved: Vec<FolderMove>) -> deptheos_app::Moves {
    deptheos_app::Moves::new(moved.into_iter().map(|m| (m.old, m.new)).collect())
}

/// The list's frames under `moved` follow to their new places.
#[uniffi::export]
pub fn list_follow(list: ListInfo, moved: Vec<FolderMove>) -> ListInfo {
    let moves = moves(moved);
    with_list(list, |list| {
        list.follow(&moves);
    })
}

/// Why Stack can't run on `included` (of `listed`, `missing` unreadable,
/// RAW by `raw_decoder`); `None` when it can
/// (`deptheos_app::frames::list::stack_problem`): the engine's checks, so
/// the app can say so first. Under Apple's decoder a linear DNG mixes with
/// anything; under LibRaw, camera RAW and DNG with standard images. On the
/// pool: it reads each DNG header once, only after the other checks pass.
#[uniffi::export]
pub async fn stack_problem(
    request: Arc<Request>,
    listed: u32,
    included: Vec<PathBuf>,
    missing: u32,
    raw_decoder: RawBackend,
    words: Arc<Words>,
) -> Result<Option<String>> {
    let check = move || {
        let problem = deptheos_app::frames::list::stack_problem(
            listed as usize,
            &included,
            missing as usize,
            raw_decoder,
        );
        Ok(problem.map(|problem| problem.text(&words.words)))
    };
    let english = AppWords::english(Platform::Ios);
    on_pool(&english, &request, Lane::Interactive, Ask::Own("stack-problem"), check).await
}

/// Whether `paths` have a decoder to choose: camera RAW or a mosaic DNG
/// (ADR-131). On the pool: it reads each DNG header once.
#[uniffi::export]
pub async fn chooses_raw_decoder(request: Arc<Request>, paths: Vec<PathBuf>) -> Result<bool> {
    let check = move || Ok(paths.iter().any(|p| RawBackend::chooses(p)));
    let english = AppWords::english(Platform::Ios);
    on_pool(&english, &request, Lane::Interactive, Ask::Own("chooses-decoder"), check).await
}

/// Whether `path` is a frame the engine reads, by extension
/// (`deptheos_app::frames::folder::is_image`): the one rule for what the
/// app lists, sizes and copies. HEIC and camera RAW count once an
/// [`Engine`] has installed its decoder.
#[uniffi::export]
pub fn is_frame(path: PathBuf) -> bool {
    folder::is_image(&path)
}

fn shots_with(list: &List, files: &[PathBuf]) -> Vec<folder::Shot> {
    list.shots().into_iter().filter(|shot| shot.files.iter().any(|f| files.contains(f))).collect()
}
