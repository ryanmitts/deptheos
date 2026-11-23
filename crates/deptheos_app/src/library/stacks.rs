//! Stacks and their working versions.
//!
//! Renders of one line of work share a stack id ([`StackId`]) and are
//! numbered within it. Editing frames or settings stays in the stack; a new
//! one starts only when a front end asks (New Stack, frames added from the
//! Library, a camera burst, each stack of a batch).
//!
//! Every stack has a **working version**: the list and settings being
//! edited, and the render they started from, its base. The one on screen is
//! `list.json`; switching stacks keeps the outgoing one in
//! `Stacks/<stack id>.json`, unless it equals its newest render.
//!
//! ```text
//! list.json              schema, stack?, base?, the list (flattened), settings?
//! Stacks/<stack id>.json schema, name?, working? {base?, list, settings?}, last_number?
//! ```
//!
//! A stack lasts while it has a render or a kept working version with
//! frames; a stack file with neither a name nor a kept working version is
//! deleted.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::frames::folder::Source;
use crate::frames::list::List;
use crate::frames::order::FrameOrder;
use crate::frames::sources::FrameRef;
use crate::fsutil::{
    StoreError, Versioned, io_error, json_files_by_id, one_edit_at_a_time, read_versioned,
    read_versioned_if_present, remove_if_present, rename, set_aside, set_aside_as,
    write_json_atomic,
};
use crate::library::history::deleted::{DeletedItem, Doomed};
use crate::library::history::{Failure, Frames, History, PaintClosed, Render, Result};
use crate::library::ids::{RenderId, StackId, is_id};
use crate::library::render_file::*;
use crate::runs::session::{Alignment, SessionOptions};
use crate::words::{Words, say};
use crate::{DepthConfig, PyramidConfig, StackerConfig, WaveletConfig};

mod setup;
mod showing;

pub use setup::{SetupPart, SetupSection, render_setup, stacked_at};
pub use showing::{RevertAlert, Showing};

pub(crate) const LIST_FILE: &str = "list.json";
/// `list.json`'s own format version.
pub(crate) const LIST_SCHEMA: u32 = 1;
pub(crate) const STACKS_DIR: &str = "Stacks";
/// A stack file's own format version.
pub(crate) const STACK_SCHEMA: u32 = 1;

/// The settings a stack runs with: the algorithm and the session options.
#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub config: StackerConfig,
    pub options: SessionOptions,
}

/// A stack's working version: the list and settings being edited, and the
/// render they started from.
#[derive(Clone, Debug)]
pub struct Working {
    pub stack: StackId,
    /// The render it started from; none before the stack's first render.
    pub base: Option<RenderId>,
    pub list: List,
    /// The live settings; none to take the front end's as they are (a first
    /// launch, an unreadable list).
    pub settings: Option<Settings>,
}

impl PartialEq for Working {
    /// The settings as saved: an unsaved option (the prefetch budget) is no
    /// difference.
    fn eq(&self, other: &Self) -> bool {
        let settings = |w: &Self| {
            w.settings
                .as_ref()
                .map(|s| (StackerFile::from(&s.config), OptionsFile::from(&s.options)))
        };
        self.stack == other.stack
            && self.base == other.base
            && self.list == other.list
            && settings(self) == settings(other)
    }
}

/// Which settings a front end doesn't always offer count as changes on it.
/// "Edited" means Revert would change something, so a setting counts only
/// where the user can set it.
///
/// The frames, RAW or JPEG, the method with its values and the crop always
/// count; the Settings window's options (grid limits, camera profile) never
/// do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Compared {
    /// Align Frames on or off (iOS; the desktop always aligns).
    pub align: bool,
    /// Which aligner (the desktop's Alignment menu).
    pub align_mode: bool,
    /// The camera RAW decoder, where a picker for it is shown.
    pub raw_backend: bool,
    /// Frames by name or by capture time (the desktop's list menu).
    pub order: bool,
}

impl Compared {
    /// Everything a front end may offer: whether a working version is its
    /// render again.
    pub const ALL: Self = Self { align: true, align_mode: true, raw_backend: true, order: true };
    /// What the iOS app edits; the decoder only while its picker shows
    /// (ADR-131).
    pub fn ios(offers_decoder: bool) -> Self {
        Self { align: true, align_mode: true, raw_backend: offers_decoder, order: false }
    }

    /// What the desktop edits; the decoder only while its picker shows.
    pub fn desktop(offers_decoder: bool) -> Self {
        Self { align: false, align_mode: true, raw_backend: offers_decoder, order: true }
    }
}

/// How a working version differs from its base render.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// Shots added, removed, included or excluded, plus one for the other
    /// file of pairs (RAW or JPEG) and one for another frame order.
    pub frames: usize,
    /// Settings with another value. Another method counts as one.
    pub settings: usize,
}

impl Changes {
    /// How many, for "3 changes since #4".
    pub fn count(&self) -> usize {
        self.frames + self.settings
    }

    /// Whether it is edited: "Edited since #4".
    pub fn any(&self) -> bool {
        self.count() > 0
    }

    /// "3 changes since #4", for tiles and list rows. `None` when unchanged.
    pub fn since(&self, base: u32, words: &Words) -> Option<String> {
        let number = base.to_string();
        self.any().then(|| say!(words, "changes-since", count = self.count(), number = number))
    }

    /// What changed against render #`number`: "1 change to the settings and
    /// 2 to the frames since #4" for [`Against::Base`], "1 difference from
    /// #3 in the settings and 2 in the frames" for [`Against::Other`].
    /// `None` when unchanged.
    fn in_full(&self, against: Against, number: u32, words: &Words) -> Option<String> {
        let number = number.to_string();
        let (frames, settings) = (self.frames, self.settings);
        Some(match (against, frames, settings) {
            (_, 0, 0) => return None,
            (Against::Base, _, 0) => {
                say!(words, "changes-since-frames", frames = frames, number = number)
            }
            (Against::Base, 0, _) => {
                say!(words, "changes-since-settings", settings = settings, number = number)
            }
            (Against::Base, _, _) => say!(
                words,
                "changes-since-both",
                settings = settings,
                frames = frames,
                number = number
            ),
            (Against::Other, _, 0) => {
                say!(words, "differences-from-frames", frames = frames, number = number)
            }
            (Against::Other, 0, _) => {
                say!(words, "differences-from-settings", settings = settings, number = number)
            }
            (Against::Other, _, _) => say!(
                words,
                "differences-from-both",
                settings = settings,
                frames = frames,
                number = number
            ),
        })
    }

    /// "3 changes", on the base render's tile. `None` when unchanged.
    pub fn short(&self, words: &Words) -> Option<String> {
        self.any().then(|| say!(words, "changes", count = self.count()))
    }
}

/// What [`Changes::in_full`] counts against: the working version's base,
/// or another render shown.
#[derive(Clone, Copy)]
enum Against {
    Base,
    Other,
}

impl Working {
    /// A new stack, with nothing listed and `settings` kept from before.
    pub fn new(settings: Option<Settings>) -> Self {
        Self { stack: StackId::new(), base: None, list: List::default(), settings }
    }

    /// `render`'s list (left-out frames excluded) and settings, based on
    /// it: what Revert to #n makes.
    pub fn of_render(render: &Render) -> Self {
        Self {
            stack: render.stack.clone(),
            base: Some(render.id.clone()),
            list: List::from_render(render),
            settings: Some(Settings { config: render.config, options: render.options }),
        }
    }

    /// A failed batch stack's frames and settings, to try again.
    pub fn of_failure(failure: &Failure) -> Self {
        Self {
            stack: failure.stack.clone(),
            base: None,
            list: failure.list(),
            settings: Some(Settings { config: failure.config, options: failure.options }),
        }
    }

    /// How it differs from `base`, the render [`Self::base`] names, in what
    /// [`Compared`] counts. A frame the stack left out of `base` is no
    /// change, included or excluded.
    pub fn changes(&self, base: &Render, compared: Compared) -> Changes {
        let from = List::from_render(base);
        let left_out =
            |files: &[PathBuf]| files.iter().any(|f| base.left_out.iter().any(|l| &l.path == f));
        let order = usize::from(compared.order && from.order != self.list.order);
        Changes {
            frames: frame_changes(&from, &self.list, left_out) + order,
            settings: self.settings.as_ref().map_or(0, |settings| {
                let base = Settings { config: base.config, options: base.options };
                compared_changes(settings, &base, compared)
            }),
        }
    }

    /// Whether switching back would rebuild it exactly from `base`, left-out
    /// frames excluded. Stricter than [`Self::changes`], which lets a
    /// left-out frame be included.
    fn is_render_again(&self, base: &Render) -> bool {
        self.list == List::from_render(base) && self.changes(base, Compared::ALL).settings == 0
    }
}

/// Whether `a` and `b` differ in what `compared` counts, as
/// [`Working::changes`] does.
#[cfg(test)]
fn settings_differ(a: &Settings, b: &Settings, compared: Compared) -> bool {
    compared_changes(a, b, compared) > 0
}

/// How many settings `compared` counts differ between `a` and `b`.
fn compared_changes(a: &Settings, b: &Settings, compared: Compared) -> usize {
    let raw_backend = compared.raw_backend && a.options.raw_backend != b.options.raw_backend;
    method_changes(&a.config, &b.config)
        + usize::from(raw_backend)
        + alignment_changes(a.options.alignment, b.options.alignment, compared)
}

/// How many alignment settings differ: Align Frames on or off is one; the
/// aligner and the crop count only between two that align.
fn alignment_changes(a: Alignment, b: Alignment, compared: Compared) -> usize {
    match (a, b) {
        (Alignment::Off { .. }, Alignment::Off { .. }) => 0,
        (Alignment::On(_), Alignment::On(_)) => {
            usize::from(compared.align_mode && a.mode() != b.mode())
                + usize::from(a.crops() != b.crops())
        }
        _ => usize::from(compared.align),
    }
}

/// Shots in one list only, or included in one and excluded in the other,
/// plus one when the lists stack different files of pairs. A shot
/// `either_way` names may differ in inclusion.
fn frame_changes(from: &List, to: &List, either_way: impl Fn(&[PathBuf]) -> bool) -> usize {
    // `None` for a shot that counts as the same included or excluded.
    let included = |list: &List| -> BTreeMap<PathBuf, Option<bool>> {
        list.shots()
            .into_iter()
            .map(|s| {
                let state = (!either_way(&s.files)).then(|| list.is_included(&s));
                (s.files[0].clone(), state)
            })
            .collect()
    };
    let (from_shots, to_shots) = (included(from), included(to));
    let changed = from_shots.iter().filter(|(shot, inc)| to_shots.get(*shot) != Some(inc)).count();
    let added = to_shots.keys().filter(|shot| !from_shots.contains_key(*shot)).count();
    // RAW or JPEG only matters to a shot that has both.
    let pairs = to.shots().iter().any(|s| s.files.len() > 1);
    changed + added + usize::from(pairs && from.source != to.source)
}

/// How many method settings differ: another method is one; otherwise each
/// differing parameter.
fn method_changes(a: &StackerConfig, b: &StackerConfig) -> usize {
    let differ = |pairs: &[(f32, f32)]| pairs.iter().filter(|(a, b)| a != b).count();
    match (*a, *b) {
        (StackerConfig::Pyramid(a), StackerConfig::Pyramid(b)) => {
            let PyramidConfig { smoothness } = a;
            differ(&[(smoothness, b.smoothness)])
        }
        (StackerConfig::Depth(a), StackerConfig::Depth(b)) => {
            let DepthConfig { radius, smoothing, blend_range } = a;
            differ(&[(radius, b.radius), (smoothing, b.smoothing), (blend_range, b.blend_range)])
        }
        (StackerConfig::Wavelet(a), StackerConfig::Wavelet(b)) => {
            let WaveletConfig { denoise, consistency } = a;
            differ(&[(denoise, b.denoise)]) + usize::from(consistency != b.consistency)
        }
        _ => 1,
    }
}

/// Whether `render` is the newest of its stack, which a working version is
/// rebuilt from when none is kept.
fn is_newest(render: &Render, renders: &[Render]) -> bool {
    !renders.iter().any(|r| r.stack == render.stack && r.number > render.number)
}

/// The stack files [`History::stacks_reusing`] read, with the size and
/// modification time each had, so an unchanged one isn't read again.
#[derive(Debug, Default)]
pub struct StackReads {
    files: HashMap<PathBuf, ((std::time::SystemTime, u64), StackInfo)>,
    /// The [`Frames::generation`] they were read at.
    generation: u64,
}

/// What the store knows of a stack besides its renders.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StackInfo {
    /// The name the user gave it, if any.
    pub name: Option<String>,
    /// Its working version, kept while another stack is on screen.
    pub working: Option<Working>,
    /// The highest number given to a render of it; 0 before its first
    /// ([`next_number`]).
    pub last_number: u32,
}

impl StackInfo {
    /// It has a working version kept.
    pub fn kept(&self) -> bool {
        self.working.is_some()
    }

    /// The name of a stack with no render yet: the user's, else its kept
    /// working version's first frame's folder. `None` for neither.
    pub fn unstacked_name(&self) -> Option<String> {
        if let Some(name) = &self.name {
            return Some(name.clone());
        }
        let first = self.working.as_ref().and_then(|w| w.list.files.first());
        first.and_then(|f| f.parent()?.file_name()).map(|n| n.to_string_lossy().into_owned())
    }
}

/// The stacks with frames kept and no render yet, by name (unnamed last):
/// the Library's "Not stacked yet". The stack on screen isn't among them.
pub fn unstacked<'a>(
    stacks: &'a HashMap<StackId, StackInfo>,
    renders: &[Render],
) -> Vec<(&'a StackId, &'a StackInfo)> {
    let mut found: Vec<(&StackId, &StackInfo)> = stacks
        .iter()
        .filter(|(id, info)| {
            info.working.as_ref().is_some_and(|w| !w.list.is_empty())
                && !renders.iter().any(|r| r.stack == **id)
        })
        .collect();
    found.sort_by_key(|(id, info)| {
        let name = info.unstacked_name().map(|n| n.to_lowercase());
        (name.is_none(), name, id.as_str().to_string())
    });
    found
}

/// What a stack is called: the user's name, else its first render's first
/// frame's folder. `renders` holds at least this stack's renders. `None`
/// for neither.
pub fn stack_name(stack: &StackId, info: Option<&StackInfo>, renders: &[Render]) -> Option<String> {
    if let Some(name) = info.and_then(|i| i.name.clone()) {
        return Some(name);
    }
    renders.iter().filter(|r| r.stack == *stack).min_by_key(|r| r.number)?.folder_name()
}

/// A list as a file keeps it: its frames by root (ADR-145).
#[derive(Clone, Default, Serialize, Deserialize)]
struct ListFrames {
    files: Vec<FrameRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    excluded: Vec<FrameRef>,
    source: Source,
    order: FrameOrder,
}

impl ListFrames {
    fn of(list: &List, frames: &Frames) -> Result<Self> {
        let paths: Vec<PathBuf> = list.files.iter().chain(&list.excluded).cloned().collect();
        let mut named = frames.refs(&paths)?;
        let excluded = named.split_off(list.files.len());
        Ok(Self { files: named, excluded, source: list.source, order: list.order })
    }

    fn list(self, frames: &Frames) -> Result<List> {
        Ok(List {
            files: frames.paths(&self.files)?,
            excluded: frames.paths(&self.excluded)?.into_iter().collect(),
            source: self.source,
            order: self.order,
        })
    }
}

#[derive(Serialize, Deserialize)]
struct ListFile {
    schema: u32,
    stack: StackId,
    #[serde(flatten)]
    kept: KeptFile,
}

impl Versioned for ListFile {
    const SCHEMA: u32 = LIST_SCHEMA;
}

/// The on-screen working version's file: a kept one's, with its stack.
impl ListFile {
    fn of(working: &Working, frames: &Frames) -> Result<Self> {
        let kept = KeptFile::of(working, frames)?;
        Ok(Self { schema: LIST_SCHEMA, stack: working.stack.clone(), kept })
    }

    fn working(self, frames: &Frames) -> Result<Working> {
        self.kept.working(&self.stack, frames)
    }
}

#[derive(Serialize, Deserialize)]
struct SettingsFile {
    stacker: StackerFile,
    options: OptionsFile,
}

impl SettingsFile {
    fn of(Settings { config, options }: &Settings) -> Self {
        Self { stacker: StackerFile::from(config), options: OptionsFile::from(options) }
    }

    fn settings(&self) -> Settings {
        Settings { config: self.stacker.config(), options: self.options.options() }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct StackFile {
    schema: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    working: Option<KeptFile>,
    /// The highest number given to a render of the stack
    /// ([`History::claim_number`]), so none is given twice. 0 before the
    /// first.
    #[serde(default, skip_serializing_if = "is_zero")]
    last_number: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// The number the next render of `stack` gets, from `renders` and `info`
/// (which may predate a render saved since). Numbers never shift or repeat,
/// so "#5" in an exported file's name is one render for good.
pub fn next_number(stack: &StackId, renders: &[Render], info: Option<&StackInfo>) -> u32 {
    let listed = newest_of(stack, renders).map_or(0, |r| r.number);
    listed.max(info.map_or(0, |i| i.last_number)) + 1
}

/// The newest render of `stack` in `renders`: its highest number.
pub fn newest_of<'a>(stack: &StackId, renders: &'a [Render]) -> Option<&'a Render> {
    renders.iter().filter(|r| r.stack == *stack).max_by_key(|r| r.number)
}

/// Delete Other Renders: the renders of `keep`'s stack to delete, leaving
/// `keep` and the starred ones.
pub fn others_of<'a>(keep: &Render, renders: &'a [Render]) -> Vec<&'a Render> {
    renders.iter().filter(|r| r.stack == keep.stack && r.id != keep.id && !r.picked).collect()
}

/// Keep Only Starred: the unstarred renders of `stack`; none when none is
/// starred.
pub fn unstarred_of<'a>(stack: &StackId, renders: &'a [Render]) -> Vec<&'a Render> {
    unstarred(renders.iter().filter(|r| r.stack == *stack).collect(), |r| r.picked)
}

/// [`unstarred_of`] for one stack's `renders`, `starred` saying which are.
pub fn unstarred<R>(renders: Vec<R>, starred: impl Fn(&R) -> bool) -> Vec<R> {
    if !renders.iter().any(&starred) {
        return Vec::new();
    }
    renders.into_iter().filter(|r| !starred(r)).collect()
}

impl Versioned for StackFile {
    const SCHEMA: u32 = STACK_SCHEMA;
}

/// A working version kept while another stack is on screen.
#[derive(Serialize, Deserialize)]
struct KeptFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    base: Option<RenderId>,
    #[serde(flatten)]
    list: ListFrames,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    settings: Option<SettingsFile>,
}

impl KeptFile {
    fn of(working: &Working, frames: &Frames) -> Result<Self> {
        Ok(Self {
            base: working.base.clone(),
            list: ListFrames::of(&working.list, frames)?,
            settings: working.settings.as_ref().map(SettingsFile::of),
        })
    }

    fn working(self, stack: &StackId, frames: &Frames) -> Result<Working> {
        Ok(Working {
            stack: stack.clone(),
            base: self.base,
            list: self.list.list(frames)?,
            settings: self.settings.as_ref().map(SettingsFile::settings),
        })
    }
}

impl History {
    /// The working version on screen as last saved, else an empty one in a
    /// new stack. A damaged one is logged and starts empty; one that
    /// couldn't be read is [`StoreError::Io`], so nothing is written over it.
    pub fn read_working(&self) -> Result<Working> {
        let path = self.root().join(LIST_FILE);
        let read = read_versioned_if_present::<ListFile>(&path)
            .and_then(|file| file.map(|f| f.working(self.frames())).transpose());
        match read {
            Ok(Some(working)) => Ok(working),
            Ok(None) => Ok(Working::new(None)),
            Err(e @ StoreError::Io { .. }) => {
                tracing::warn!("{} couldn't be read: {e}", path.display());
                Err(e)
            }
            Err(e) => {
                tracing::warn!("{} is damaged, so the list starts empty: {e}", path.display());
                Ok(Working::new(None))
            }
        }
    }

    /// Keep `working` as the one on screen, on the store thread once it has
    /// stood unchanged for [`crate::engine::store::KEEP_AFTER`]. A switch of
    /// stacks drops it unwritten ([`Self::switch_stack`]); quit writes it at
    /// once ([`crate::engine::store::Store::flush`]).
    pub fn keep_working(&self, working: Working) {
        let history = self.clone();
        crate::engine::store::store().keep(self.working_key(), move || {
            if let Err(e) = history.write_working(&working) {
                tracing::warn!("could not save the list: {e}");
            }
        });
    }

    /// The store's key for this Library's kept working version.
    fn working_key(&self) -> String {
        format!("{LIST_FILE} {}", self.root().display())
    }

    /// Keep `working` as the one on screen, atomically.
    pub fn write_working(&self, working: &Working) -> Result<()> {
        let path = self.root().join(LIST_FILE);
        write_json_atomic(&path, &ListFile::of(working, self.frames())?)
    }

    /// Put stack `to` on screen in place of `outgoing` and return its
    /// working version: the kept one, else its newest render's, else
    /// `fallback` ([`Working::of_failure`]). `None`, changing nothing, when
    /// there is none. Within one stack nothing changes.
    pub fn switch_stack(
        &self,
        outgoing: &Working,
        to: &StackId,
        renders: &[Render],
        fallback: Option<Working>,
    ) -> Result<Option<Working>> {
        if outgoing.stack == *to {
            return Ok(Some(outgoing.clone()));
        }
        // An unreadable stack file opens from its newest render.
        let kept = self.read_stack_file(to).and_then(|file| {
            file.and_then(|f| f.working).map(|k| k.working(to, self.frames())).transpose()
        });
        let kept = match kept {
            Ok(kept) => kept,
            Err(e) => {
                tracing::warn!(
                    "{}: nothing kept is read from it: {e}",
                    self.stack_path(to).display()
                );
                None
            }
        };
        let took_kept = kept.is_some();
        let newest = || newest_of(to, renders);
        let Some(incoming) = kept.or_else(|| newest().map(Working::of_render)).or(fallback) else {
            return Ok(None);
        };
        // Kept first and taken last, so a crash leaves a copy.
        self.keep(outgoing, renders)?;
        // A pending keep would land over the stack switched to.
        crate::engine::store::store().forget(&self.working_key());
        self.write_working(&incoming)?;
        if took_kept {
            self.edit_stack_file(to, |f| f.working.take().is_some())?;
        }
        Ok(Some(incoming))
    }

    /// Keep `working` in its stack's file, unless it is its base render
    /// again or has no frames.
    fn keep(&self, working: &Working, renders: &[Render]) -> Result<()> {
        let base = working.base.as_ref().and_then(|id| renders.iter().find(|r| r.id == *id));
        let worth = match base {
            Some(base) => !working.is_render_again(base) || !is_newest(base, renders),
            // Never stacked, or its base was deleted: nothing rebuilds it.
            None => !working.list.is_empty(),
        };
        let kept = worth.then(|| KeptFile::of(working, self.frames())).transpose()?;
        self.edit_stack_file(&working.stack, |f| {
            f.working = kept;
            true
        })
    }

    /// Delete a stack: its `renders`, `failures`, kept working version and
    /// name, moved into Recently Deleted as one record
    /// ([`crate::library::history::deleted`]). `None` when nothing was
    /// there. The frames' files and the working version on screen are not
    /// touched. `closed` is the paint closed first ([`History::close_paint`]).
    pub fn delete_stack(
        &self,
        stack: &StackId,
        renders: &[RenderId],
        failures: &[RenderId],
        closed: PaintClosed,
    ) -> Result<Option<DeletedItem>> {
        let listed: Vec<Render> = renders.iter().filter_map(|id| self.render(id).ok()).collect();
        let failed: Vec<Failure> =
            self.failures()?.into_iter().filter(|f| failures.contains(&f.id)).collect();
        let doomed = Doomed {
            stack,
            name: self.name_of(stack, &listed, &failed),
            whole: true,
            renders: listed.iter().map(|r| (r.id.clone(), r.number)).collect(),
            failures: failed.iter().map(|f| f.id.clone()).collect(),
        };
        self.put_away(doomed, &closed)
    }

    /// Delete a stack for good, as [`History::delete_library`] does, not
    /// kept in Recently Deleted.
    pub(crate) fn remove_stack(
        &self,
        stack: &StackId,
        renders: &[RenderId],
        failures: &[RenderId],
        _closed: PaintClosed,
    ) -> Result<()> {
        // Its file goes whole below, so nothing of it is rebased first.
        self.delete_render_dirs(renders)?;
        self.delete_failures(failures)?;
        self.edit_stack_file(stack, |f| {
            *f = StackFile::default();
            true
        })
    }

    /// Put a stack's file back from Recently Deleted, `from`, merging into
    /// one made since: its name where none, the higher last number, the
    /// current kept working version first. A damaged one is set aside
    /// unmerged; an unreadable one fails the restore.
    pub(crate) fn restore_stack_file(&self, stack: &StackId, from: &Path) -> Result<()> {
        let to = self.stack_path(stack);
        {
            let _one_at_a_time = one_edit_at_a_time();
            if !to.exists() {
                let dir = self.stacks_dir();
                std::fs::create_dir_all(&dir).map_err(io_error(&dir))?;
                return rename(from, &to);
            }
        }
        match read_versioned::<StackFile>(from) {
            Ok(deleted) => self.edit_stack_file(stack, |f| {
                f.name = f.name.take().or(deleted.name);
                f.last_number = f.last_number.max(deleted.last_number);
                f.working = f.working.take().or(deleted.working);
                true
            })?,
            Err(e @ StoreError::Io { .. }) => return Err(e),
            Err(e) => {
                tracing::warn!("{}: not merged back: {e}", from.display());
                set_aside_as(from, &to).map_err(io_error(from))?;
                return Ok(());
            }
        }
        remove_if_present(from)
    }

    /// The last number the stack file at `path` holds; 0 when it can't be
    /// read.
    pub(crate) fn last_number_in(&self, path: &Path) -> u32 {
        read_versioned::<StackFile>(path).map_or(0, |f| f.last_number)
    }

    /// Forget each of `stacks` left with nothing (no render, failed run,
    /// kept working version, deleted record, or frames on screen), file and
    /// all. Run when Recently Deleted records go for good.
    pub(crate) fn forget_if_unused(&self, stacks: &[StackId]) {
        if stacks.is_empty() {
            return;
        }
        let Ok(renders) = self.list() else { return };
        let Ok(failures) = self.failures() else { return };
        let on_screen = self.stack_on_screen();
        for stack in stacks {
            let used = renders.iter().any(|r| r.stack == *stack)
                || failures.iter().any(|f| f.stack == *stack)
                || on_screen.as_ref() == Some(stack)
                || self.deleted_holds(stack);
            if used {
                continue;
            }
            let forgotten = self.edit_stack_file(stack, |f| {
                if f.working.is_some() {
                    return false;
                }
                tracing::debug!("stack {}: forgotten with the last of it", stack.as_str());
                *f = StackFile::default();
                true
            });
            if let Err(e) = forgotten {
                tracing::warn!("could not forget stack {}: {e}", stack.as_str());
            }
        }
    }

    /// Name a stack; `None` or blank goes back to its folder's name.
    pub fn rename_stack(&self, stack: &StackId, name: Option<&str>) -> Result<()> {
        let name = name.map(str::trim).filter(|n| !n.is_empty()).map(String::from);
        self.edit_stack_file(stack, |f| {
            let changed = f.name != name;
            f.name = name;
            changed
        })
    }

    /// Every stack with a name, a kept working version or a deleted number
    /// to skip. One whose file can't be read is logged and skipped.
    pub fn stacks(&self) -> Result<HashMap<StackId, StackInfo>> {
        self.stacks_reusing(&mut StackReads::default())
    }

    /// [`Self::stacks`], parsing again only files whose size or modification
    /// time changed since `reads`. Each write replaces a file whole
    /// ([`write_json_atomic`]), so a change shows in its time.
    pub fn stacks_reusing(&self, reads: &mut StackReads) -> Result<HashMap<StackId, StackInfo>> {
        // A relink since moves the frames of what was read.
        if reads.generation != self.frames().generation() {
            reads.files.clear();
            reads.generation = self.frames().generation();
        }
        let files = json_files_by_id(&self.stacks_dir(), StackId::parse)?;
        let mut stacks = HashMap::new();
        let mut seen = std::collections::HashSet::new();
        for (stack, path) in files {
            let stamp =
                std::fs::metadata(&path).ok().and_then(|m| Some((m.modified().ok()?, m.len())));
            if let (Some(stamp), Some((was, info))) = (stamp, reads.files.get(&path))
                && *was == stamp
            {
                stacks.insert(stack, info.clone());
                seen.insert(path);
                continue;
            }
            let read = read_versioned::<StackFile>(&path).and_then(|file| {
                let working = file.working.map(|k| k.working(&stack, self.frames())).transpose()?;
                Ok(StackInfo { name: file.name, working, last_number: file.last_number })
            });
            match read {
                Ok(info) => {
                    match stamp {
                        Some(stamp) => {
                            reads.files.insert(path.clone(), (stamp, info.clone()));
                            seen.insert(path);
                        }
                        None => {
                            reads.files.remove(&path);
                        }
                    }
                    stacks.insert(stack, info);
                }
                Err(e) => tracing::warn!("skipping stack {}: {e}", path.display()),
            }
        }
        reads.files.retain(|path, _| seen.contains(path));
        Ok(stacks)
    }

    /// The number a render of `stack` being saved gets, recorded at once so
    /// two saves never share one. Numbers in Recently Deleted count. A stack
    /// file with no number is seeded from its highest-numbered render.
    pub(crate) fn claim_number(&self, stack: &StackId) -> Result<u32> {
        let deleted = self.deleted_numbers_of(stack);
        let mut claimed = 0;
        self.edit_stack_file(stack, |f| {
            if f.last_number == 0 {
                f.last_number = self.highest_listed_number(stack);
            }
            f.last_number = f.last_number.max(deleted) + 1;
            claimed = f.last_number;
            true
        })?;
        Ok(claimed)
    }

    /// The highest number of a render of `stack` in Renders/, 0 for none;
    /// a render that can't be read is passed over.
    fn highest_listed_number(&self, stack: &StackId) -> u32 {
        let Ok(entries) = std::fs::read_dir(self.renders_dir()) else { return 0 };
        let highest = entries
            .flatten()
            .filter(|e| e.file_name().to_str().and_then(RenderId::parse).is_some())
            .filter_map(|e| read_versioned::<RenderFile>(&e.path().join(RENDER_FILE)).ok())
            .filter(|file| file.stack == *stack)
            .map(|file| file.number)
            .max()
            .unwrap_or(0);
        if highest > 0 {
            tracing::warn!(
                "stack {}: its file held no number; numbering on from #{highest}",
                stack.as_str()
            );
        }
        highest
    }

    /// `render` was saved after its stack left the screen, so its kept
    /// working version still has base `from`. Rebase it on `render`, or drop
    /// it when it is that render again; one rebased since is left alone.
    pub fn stacked_off_screen(&self, render: &Render, from: Option<&RenderId>) -> Result<()> {
        self.edit_stack_file(&render.stack, |f| {
            if f.working.as_ref().is_none_or(|k| k.base.as_ref() != from) {
                return false;
            }
            let Some(kept) = f.working.as_mut() else { return false };
            let again = kept.list.clone().list(self.frames()).is_ok_and(|list| {
                let settings = kept.settings.as_ref().map(SettingsFile::settings);
                let working = Working { stack: render.stack.clone(), base: None, list, settings };
                working.is_render_again(render)
            });
            tracing::debug!(
                "stack {}: its kept working version is {} #{}",
                render.stack.as_str(),
                if again { "dropped, the same as" } else { "now based on" },
                render.number
            );
            match again {
                true => f.working = None,
                false => kept.base = Some(render.id.clone()),
            }
            true
        })
    }

    /// The stack on screen as last saved, if it has frames, without
    /// touching the file.
    fn stack_on_screen(&self) -> Option<StackId> {
        let file = read_versioned::<ListFile>(&self.root().join(LIST_FILE)).ok()?;
        (!file.kept.list.files.is_empty()).then_some(file.stack)
    }

    /// The renders `deleted` are gone: rebase a kept working version on the
    /// stack's newest render left. The stack stays until they go for good
    /// ([`Self::forget_if_unused`]).
    pub(crate) fn renders_deleted(&self, deleted: &[Render]) {
        if deleted.is_empty() {
            return;
        }
        let Ok(renders) = self.list() else { return };
        let stacks: std::collections::BTreeSet<&StackId> =
            deleted.iter().map(|r| &r.stack).collect();
        for stack in stacks {
            let newest = newest_of(stack, &renders);
            let updated = self.edit_stack_file(stack, |f| {
                let Some(kept) = &mut f.working else { return false };
                if !kept.base.as_ref().is_some_and(|b| deleted.iter().any(|r| r.id == *b)) {
                    return false;
                }
                kept.base = newest.map(|r| r.id.clone());
                true
            });
            if let Err(e) = updated {
                tracing::warn!("could not update stack {}: {e}", stack.as_str());
            }
        }
    }

    fn read_stack_file(&self, stack: &StackId) -> Result<Option<StackFile>> {
        read_versioned_if_present(&self.stack_path(stack))
    }

    /// Change a stack's file, creating it or deleting it when empty; written
    /// only when `edit` says it changed. A damaged one is set aside and
    /// replaced. Callers are on several threads, so read and write are one
    /// edit of the store.
    fn edit_stack_file(
        &self,
        stack: &StackId,
        edit: impl FnOnce(&mut StackFile) -> bool,
    ) -> Result<()> {
        let _one_at_a_time = one_edit_at_a_time();
        let path = self.stack_path(stack);
        let (mut file, damaged) = match self.read_stack_file(stack) {
            Ok(file) => (file.unwrap_or_default(), false),
            // Unread, it may be whole (a volume that didn't answer);
            // replacing it would lose its name and numbering.
            Err(e @ StoreError::Io { .. }) => return Err(e),
            Err(e) => {
                tracing::warn!("replacing damaged {}: {e}", path.display());
                (StackFile::default(), true)
            }
        };
        if !edit(&mut file) {
            return Ok(());
        }
        // Kept aside: it may hold a name or working version no render has.
        if damaged {
            set_aside(&path).map_err(io_error(&path))?;
        }
        if file.name.is_none() && file.working.is_none() && file.last_number == 0 {
            return remove_if_present(&path);
        }
        file.schema = STACK_SCHEMA;
        let dir = self.stacks_dir();
        std::fs::create_dir_all(&dir).map_err(io_error(&dir))?;
        write_json_atomic(&path, &file)
    }

    /// Record `number` for a render put in `stack` by hand, as a claim would.
    #[cfg(test)]
    pub(crate) fn note_number(&self, stack: &StackId, number: u32) {
        let noted = self.edit_stack_file(stack, |f| {
            f.last_number = f.last_number.max(number);
            true
        });
        noted.unwrap();
    }

    pub(crate) fn stacks_dir(&self) -> PathBuf {
        self.root().join(STACKS_DIR)
    }

    /// The on-screen working version's file and its copies set aside as
    /// unreadable (`list.unreadable-<id>.json`), for
    /// [`History::delete_library`].
    pub(crate) fn working_files(&self) -> Vec<PathBuf> {
        let path = self.root().join(LIST_FILE);
        let stem = format!("{}.unreadable-", LIST_FILE.trim_end_matches(".json"));
        let aside = std::fs::read_dir(self.root()).into_iter().flatten().flatten().filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(&stem) && name.ends_with(".json"))
        });
        std::iter::once(path).chain(aside.map(|e| e.path())).collect()
    }

    /// The stack files set aside as damaged
    /// (`Stacks/<stack id>.unreadable-<id>.json`), for
    /// [`History::delete_library`].
    pub(crate) fn stack_files_set_aside(&self) -> Vec<PathBuf> {
        let entries = std::fs::read_dir(self.stacks_dir()).into_iter().flatten().flatten();
        let aside = entries.filter(|e| {
            e.file_name().to_str().is_some_and(|name| {
                name.split_once(".unreadable-")
                    .is_some_and(|(stack, rest)| is_id(stack) && rest.ends_with(".json"))
            })
        });
        aside.map(|e| e.path()).collect()
    }

    pub(crate) fn stack_path(&self, stack: &StackId) -> PathBuf {
        self.stacks_dir().join(format!("{}.json", stack.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::history::tests::{frame, id, put_render_in, store};
    use crate::{DepthConfig, PyramidConfig, SessionOptions, StackerConfig};

    #[test]
    fn changes_are_worded_short_and_in_full() {
        let words = Words::english(crate::words::explain::Platform::Desktop);
        let say = |frames, settings| {
            let changes = Changes { frames, settings };
            (changes.since(4, &words), changes.in_full(Against::Base, 4, &words))
        };
        assert_eq!(say(0, 0), (None, None));
        let line = |s: &str| Some(s.to_string());
        assert_eq!(say(1, 0), (line("1 change since #4"), line("1 change to the frames since #4")));
        assert_eq!(say(0, 2).1, line("2 changes to the settings since #4"));
        assert_eq!(
            say(2, 1),
            (
                line("3 changes since #4"),
                line("1 change to the settings and 2 to the frames since #4")
            )
        );
        assert_eq!(say(1, 3).1, line("3 changes to the settings and 1 to the frames since #4"));
    }

    fn stack(n: u8) -> StackId {
        StackId::parse(&format!("{n:02x}{}", "0".repeat(30))).unwrap()
    }

    fn render(history: &History, n: u8) -> Render {
        history.render(&RenderId::parse(&id(n)).unwrap()).unwrap()
    }

    /// Renders 1 and 2 in stack 1 (#1, #2), render 3 in stack 2 (#1), of
    /// frames in /a/Bee and /a/Ant.
    fn two_stacks() -> (tempfile::TempDir, History) {
        let (root, history) = store();
        let bee = ["/a/Bee/1.jpg", "/a/Bee/2.jpg", "/a/Bee/3.jpg"];
        put_render_in(&history, &id(1), &bee, None, Some((stack(1).as_str(), 1)));
        put_render_in(&history, &id(2), &bee, None, Some((stack(1).as_str(), 2)));
        put_render_in(&history, &id(3), &["/a/Ant/1.jpg"], None, Some((stack(2).as_str(), 1)));
        (root, history)
    }

    /// The stacks with a name or a working version kept.
    fn kept_or_named(history: &History) -> Vec<StackId> {
        let stacks = history.stacks().unwrap();
        stacks.into_iter().filter(|(_, i)| i.name.is_some() || i.kept()).map(|(s, _)| s).collect()
    }

    fn edited(history: &History) -> Working {
        let mut working = Working::of_render(&render(history, 2));
        let shot = working.list.shots()[1].clone();
        working.list.set_included(&shot, false);
        working.settings = Some(Settings {
            config: StackerConfig::Pyramid(PyramidConfig { smoothness: 0.9 }),
            options: SessionOptions::default(),
        });
        working
    }

    /// The on-screen working version's file is a kept one's fields after
    /// its schema and stack.
    #[test]
    fn the_working_version_file_keeps_its_field_order() {
        let (_root, history) = two_stacks();
        let working = edited(&history);
        history.write_working(&working).unwrap();
        let text = std::fs::read_to_string(history.root().join(LIST_FILE)).unwrap();
        let at = |key: &str| text.find(&format!("\"{key}\":")).unwrap_or_else(|| panic!("{key}"));
        let order = ["schema", "stack", "base", "files", "settings"].map(at);
        assert!(order.is_sorted(), "{text}");
        assert_eq!(history.read_working().unwrap(), working);
    }

    #[test]
    fn the_working_version_round_trips_and_starts_empty() {
        let (_root, history) = store();
        let empty = history.read_working().unwrap();
        assert!(empty.list.is_empty() && empty.settings.is_none() && empty.base.is_none());
        let working = Working {
            stack: stack(1),
            base: RenderId::parse(&id(4)),
            list: List {
                files: vec![frame(&history, "/a/1.jpg"), frame(&history, "/a/2.jpg")],
                excluded: [frame(&history, "/a/2.jpg")].into(),
                ..List::default()
            },
            settings: Some(Settings {
                config: StackerConfig::Depth(DepthConfig { radius: 7.0, ..Default::default() }),
                options: SessionOptions {
                    alignment: Alignment::default().crop_turned_over(),
                    ..SessionOptions::default()
                },
            }),
        };
        history.write_working(&working).unwrap();
        assert_eq!(history.read_working().unwrap(), working);
    }

    /// A damaged list (a bad id, frames by path, another schema, not JSON)
    /// starts the working version empty, logged, with nothing set aside.
    #[test]
    fn a_damaged_list_is_replaced() {
        let (_root, history) = store();
        let path = history.root().join(LIST_FILE);
        let files = history.frames().refs(&[frame(&history, "/a/1.jpg")]).unwrap();
        let good = stack(1).as_str().to_owned();
        let list = |stack: &str, base: &str| {
            serde_json::json!({
                "schema": LIST_SCHEMA, "files": files, "source": "raw", "order": "name",
                "stack": stack, "base": base,
            })
            .to_string()
        };
        let damaged = [
            list("../nope", &good),
            list(&good, "x"),
            r#"{"schema":1,"files":["/a/1.jpg"]}"#.to_string(),
            format!(r#"{{"schema": {}, "files": ["/a/1.jpg"]}}"#, LIST_SCHEMA + 1),
            "{ not json".to_string(),
        ];
        for bytes in damaged {
            std::fs::write(&path, &bytes).unwrap();
            let working = history.read_working().unwrap();
            assert!(working.list.is_empty() && working.settings.is_none(), "{bytes}");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), bytes, "not moved: {bytes}");
            let names = std::fs::read_dir(history.root()).unwrap();
            let names: Vec<_> = names.map(|e| e.unwrap().file_name()).collect();
            let aside = names.iter().any(|n| n.to_string_lossy().contains("unreadable"));
            assert!(!aside, "{bytes}: {names:?}");
            let mut edited = working.clone();
            edited.list.add_files(vec![frame(&history, "/b/1.jpg")]);
            history.write_working(&edited).unwrap();
            assert_eq!(history.read_working().unwrap(), edited, "replaced: {bytes}");
        }
    }

    #[test]
    fn changes_count_frames_and_settings_since_the_base() {
        let (_root, history) = two_stacks();
        let base = render(&history, 2);
        let mut working = Working::of_render(&base);
        assert_eq!(working.changes(&base, Compared::ALL), Changes::default());
        // RAW or JPEG is no change to a list without pairs.
        working.list.source = crate::frames::folder::Source::Raw;
        assert_eq!(working.changes(&base, Compared::ALL), Changes::default());

        let shot = working.list.shots()[0].clone();
        working.list.set_included(&shot, false);
        working.list.add_files(vec![frame(&history, "/a/Bee/4.jpg")]);
        let Settings { config, mut options } = working.settings.unwrap();
        options.alignment = options.alignment.crop_turned_over();
        working.settings = Some(Settings { config, options });
        assert_eq!(working.changes(&base, Compared::ALL), Changes { frames: 2, settings: 1 });

        // Another method is one change, however many values differ.
        working.settings =
            Some(Settings { config: StackerConfig::Depth(DepthConfig::default()), options });
        assert_eq!(working.changes(&base, Compared::ALL).settings, 2);

        // Revert: the base's list and settings again.
        let reverted = Working::of_render(&base);
        assert!(!reverted.changes(&base, Compared::ALL).any());
    }

    /// "Edited" is what Revert would change: Settings-window options never
    /// count, nor a setting a front end doesn't offer.
    #[test]
    fn only_what_the_user_chooses_counts_as_a_change() {
        let (_root, history) = two_stacks();
        let base = render(&history, 2);
        let mut working = Working::of_render(&base);
        let Settings { config, mut options } = working.settings.unwrap();

        // Settings-window options: no change anywhere.
        options.prefetch_bytes += 1;
        working.settings = Some(Settings { config, options });
        for compared in [Compared::ALL, Compared::ios(true), Compared::desktop(true)] {
            assert!(!working.changes(&base, compared).any(), "{compared:?}");
        }

        // The aligner and the decoder: where they are offered (the
        // decoder only while its picker shows).
        options.alignment = options.alignment.other_aligner();
        options.raw_backend = match options.raw_backend {
            crate::RawBackend::LibRaw => crate::RawBackend::AdobeDng,
            _ => crate::RawBackend::LibRaw,
        };
        working.settings = Some(Settings { config, options });
        assert_eq!(working.changes(&base, Compared::ios(false)).settings, 1);
        assert_eq!(working.changes(&base, Compared::ios(true)).settings, 2);
        assert_eq!(working.changes(&base, Compared::desktop(false)).settings, 1);
        assert_eq!(working.changes(&base, Compared::desktop(true)).settings, 2);

        // Align Frames is the iOS app's; the frame order the desktop's.
        let mut working = Working::of_render(&base);
        let Settings { config, mut options } = working.settings.unwrap();
        options.alignment = options.alignment.turned_over();
        working.settings = Some(Settings { config, options });
        working.list.order = crate::frames::order::FrameOrder::CaptureTime;
        assert_eq!(working.changes(&base, Compared::ios(true)), Changes { frames: 0, settings: 1 });
        let desktop = working.changes(&base, Compared::desktop(false));
        assert_eq!(desktop, Changes { frames: 1, settings: 0 });

        // Grid search's limits are the Settings window's too.
        let grid = |rotation_deg: f32| Settings {
            config,
            options: SessionOptions {
                alignment: Alignment::On(crate::AlignSettings {
                    mode: crate::AlignMode::Grid,
                    grid: crate::GridConfig { rotation_deg, ..Default::default() },
                    crop: false,
                }),
                ..options
            },
        };
        assert!(!settings_differ(&grid(1.0), &grid(4.0), Compared::ALL));
        // Align Frames turned off is one change, whatever the aligner and
        // the crop were: they don't apply to frames not aligned.
        let off = Settings {
            config,
            options: SessionOptions {
                alignment: grid(1.0).options.alignment.turned_over(),
                ..options
            },
        };
        assert_eq!(compared_changes(&off, &grid(1.0), Compared::ios(false)), 1);
        assert_eq!(compared_changes(&grid(1.0), &off, Compared::desktop(false)), 0);
    }

    /// A render's menu asks whether its settings differ by the same rule:
    /// the decoder only where its picker shows.
    #[test]
    fn settings_differ_in_what_the_user_chooses() {
        let (_root, history) = two_stacks();
        let base = render(&history, 2);
        let made = Settings { config: base.config, options: base.options };
        let Settings { config, mut options } = made;
        options.raw_backend = match options.raw_backend {
            crate::RawBackend::LibRaw => crate::RawBackend::AdobeDng,
            _ => crate::RawBackend::LibRaw,
        };
        assert!(!settings_differ(&made, &Settings { config, options }, Compared::desktop(false)));
        assert!(settings_differ(&made, &Settings { config, options }, Compared::desktop(true)));
        let other = crate::StackerConfig::Depth(crate::DepthConfig::default());
        assert!(settings_differ(
            &made,
            &Settings { config: other, options: base.options },
            Compared::desktop(false)
        ));
        assert!(!settings_differ(&made, &made, Compared::ALL));
    }

    /// Within one method each parameter that differs is a change; another
    /// method is one, whatever its values.
    #[test]
    fn each_parameter_of_a_method_is_a_change() {
        use StackerConfig::{Depth, Pyramid, Wavelet};
        let pyramid = |smoothness| Pyramid(PyramidConfig { smoothness });
        let depth =
            |radius, smoothing, blend_range| Depth(DepthConfig { radius, smoothing, blend_range });
        let wavelet = |denoise, consistency| Wavelet(crate::WaveletConfig { denoise, consistency });
        let cases = [
            (pyramid(4.0), pyramid(4.0), 0),
            (pyramid(4.0), pyramid(5.0), 1),
            (depth(8.0, 2.5, 3.0), depth(8.0, 2.5, 3.0), 0),
            (depth(8.0, 2.5, 3.0), depth(9.0, 2.5, 3.0), 1),
            (depth(8.0, 2.5, 3.0), depth(9.0, 3.5, 3.0), 2),
            (depth(8.0, 2.5, 3.0), depth(9.0, 3.5, 4.0), 3),
            (wavelet(1.0, 0), wavelet(1.0, 2), 1),
            (wavelet(1.0, 0), wavelet(2.0, 2), 2),
            (pyramid(4.0), wavelet(1.0, 2), 1),
            (depth(8.0, 2.5, 3.0), pyramid(4.0), 1),
        ];
        let options = SessionOptions::default();
        for (a, b, n) in cases {
            let (a, b) = (Settings { config: a, options }, Settings { config: b, options });
            assert_eq!(compared_changes(&a, &b, Compared::ALL), n, "{a:?} {b:?}");
        }
    }

    /// Two working versions are the same when what is saved of them is:
    /// an option that isn't saved (the prefetch budget) is no difference.
    #[test]
    fn working_versions_are_equal_in_what_is_saved() {
        let (_root, history) = two_stacks();
        let base = Working::of_render(&render(&history, 2));
        let with = |f: &dyn Fn(&mut Settings)| {
            let mut working = base.clone();
            f(working.settings.as_mut().unwrap());
            working
        };
        assert_eq!(base, with(&|s| s.options.prefetch_bytes += 1));
        let other_profile = |s: &mut Settings| {
            s.options.camera_profile = match s.options.camera_profile {
                crate::DcpPreference::None => crate::DcpPreference::CameraStandard,
                _ => crate::DcpPreference::None,
            }
        };
        let other_method =
            |s: &mut Settings| s.config = StackerConfig::Depth(DepthConfig::default());
        for changed in [with(&other_profile), with(&other_method)] {
            assert_ne!(base, changed);
        }
        assert_ne!(base, Working { settings: None, ..base.clone() });
    }

    /// A list with a frame the render left out is not edited, but only
    /// keeping it brings it back as it was.
    #[test]
    fn a_list_with_a_left_out_frame_included_is_kept() {
        let (_root, history) = two_stacks();
        let mut base = render(&history, 2);
        base.left_out = vec![crate::library::history::LeftOutFrame {
            path: frame(&history, "/a/Bee/2.jpg"),
            reason: "alignment failed".into(),
        }];
        let renders: Vec<Render> = history
            .list()
            .unwrap()
            .into_iter()
            .map(|r| if r.id == base.id { base.clone() } else { r })
            .collect();
        let mut stacked = Working::of_render(&base);
        let shot = stacked.list.shots()[1].clone();
        stacked.list.set_included(&shot, true);
        assert!(!stacked.changes(&base, Compared::ALL).any());

        let other = renders.iter().find(|r| r.stack != base.stack).unwrap().stack.clone();
        history.switch_stack(&stacked, &other, &renders, None).unwrap();
        let there = history.read_working().unwrap();
        let back = history.switch_stack(&there, &base.stack, &renders, None).unwrap();
        assert_eq!(back, Some(stacked), "with frame 2 still included");
    }

    /// Frame 2 left out of the render is no edit, included or excluded;
    /// excluding another frame is.
    #[test]
    fn a_frame_the_stack_left_out_is_no_change() {
        let (_root, history) = two_stacks();
        let mut base = render(&history, 2);
        base.left_out = vec![crate::library::history::LeftOutFrame {
            path: frame(&history, "/a/Bee/2.jpg"),
            reason: "alignment failed".into(),
        }];
        let given_back = Working::of_render(&base);
        let shots = given_back.list.shots();
        assert!(!given_back.list.is_included(&shots[1]), "what it left out comes back excluded");
        assert!(!given_back.changes(&base, Compared::ALL).any());

        let mut stacked = given_back.clone();
        stacked.list.set_included(&shots[1], true);
        assert!(!stacked.changes(&base, Compared::ALL).any(), "the list as it was stacked");

        stacked.list.set_included(&shots[0], false);
        assert_eq!(stacked.changes(&base, Compared::ALL).frames, 1);
    }

    #[test]
    fn switching_to_another_stack_and_back_restores_the_working_version_exactly() {
        let (_root, history) = two_stacks();
        let working = edited(&history);

        // To stack 2: its newest render's list and settings, on screen.
        let ant = history
            .switch_stack(&working, &stack(2), &history.list().unwrap(), None)
            .unwrap()
            .unwrap();
        assert_eq!(ant, Working::of_render(&render(&history, 3)));
        assert_eq!(history.read_working().unwrap(), ant);
        assert!(history.stacks().unwrap()[&stack(1)].kept(), "Working version kept");

        // And back: exactly as it was, and not kept aside.
        let back =
            history.switch_stack(&ant, &stack(1), &history.list().unwrap(), None).unwrap().unwrap();
        assert_eq!(back, working);
        assert_eq!(history.read_working().unwrap(), working);
        assert!(kept_or_named(&history).is_empty(), "nothing left to keep");
    }

    /// A damaged stack file doesn't stop its stack opening from its newest
    /// render; it is left until an edit replaces it.
    #[test]
    fn a_stack_file_that_cant_be_read_doesnt_block_its_stack() {
        let (_root, history) = two_stacks();
        let renders = history.list().unwrap();
        let ant = Working::of_render(&render(&history, 3));
        let path = history.stack_path(&stack(1));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let newer = format!(r#"{{"schema": {}, "name": "Bee"}}"#, STACK_SCHEMA + 1);
        for contents in ["not json", newer.as_str()] {
            std::fs::write(&path, contents).unwrap();
            let opened = history.switch_stack(&ant, &stack(1), &renders, None).unwrap().unwrap();
            assert_eq!(opened, Working::of_render(&render(&history, 2)));
            assert_eq!(std::fs::read_to_string(&path).unwrap(), contents);
        }
    }

    /// A stack file that couldn't be read (I/O, not damage) is left where it
    /// is and the edit fails.
    #[test]
    fn a_stack_file_that_fails_to_read_is_not_replaced() {
        let (_root, history) = two_stacks();
        let path = history.stack_path(&stack(1));
        std::fs::remove_file(&path).ok();
        // Reading a folder fails with an I/O error.
        std::fs::create_dir_all(&path).unwrap();
        let named = history.rename_stack(&stack(1), Some("Wasp"));
        assert!(matches!(named, Err(StoreError::Io { .. })), "{named:?}");
        assert!(path.is_dir(), "left as it was");
        let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert!(!names.iter().any(|n| n.to_string_lossy().contains("unreadable")), "{names:?}");
    }

    #[test]
    fn within_a_stack_the_working_version_stays() {
        let (_root, history) = two_stacks();
        let working = edited(&history);
        let renders = history.list().unwrap();
        assert_eq!(
            history.switch_stack(&working, &stack(1), &renders, None).unwrap(),
            Some(working)
        );
        assert!(kept_or_named(&history).is_empty());
    }

    /// A working version kept just before a switch of stacks never lands
    /// over the stack switched to.
    #[test]
    fn a_kept_working_version_never_lands_after_a_switch() {
        use crate::engine::store::store;
        let (_root, history) = two_stacks();
        let renders = history.list().unwrap();
        let working = edited(&history);
        history.keep_working(working.clone());
        let switch = {
            let history = history.clone();
            store().write(move || history.switch_stack(&working, &stack(2), &renders, None))
        };
        let incoming = switch.wait().unwrap().unwrap().unwrap();
        assert!(store().flush(std::time::Duration::from_secs(5)));
        assert_eq!(history.read_working().unwrap(), incoming);
        assert_eq!(incoming.stack, stack(2));
    }

    /// A switch that opens nothing leaves the working version kept before
    /// it to land.
    #[test]
    fn a_kept_working_version_lands_when_a_switch_opens_nothing() {
        use crate::engine::store::store;
        let (_root, history) = two_stacks();
        let renders = history.list().unwrap();
        let working = edited(&history);
        history.keep_working(working.clone());
        let switch = {
            let history = history.clone();
            let working = working.clone();
            store().write(move || history.switch_stack(&working, &stack(9), &renders, None))
        };
        assert_eq!(switch.wait().unwrap().unwrap(), None);
        assert!(store().flush(std::time::Duration::from_secs(5)));
        assert_eq!(history.read_working().unwrap(), working);
    }

    /// Only the newest of a quick run of changes is written.
    #[test]
    fn the_newest_working_version_kept_is_written() {
        use crate::engine::store::store;
        let (_root, history) = two_stacks();
        let working = edited(&history);
        history.keep_working(Working::of_render(&render(&history, 1)));
        history.keep_working(working.clone());
        assert!(store().flush(std::time::Duration::from_secs(5)));
        assert_eq!(history.read_working().unwrap(), working);
    }

    #[test]
    fn an_unedited_working_version_is_not_kept() {
        let (_root, history) = two_stacks();
        let renders = history.list().unwrap();
        let working = Working::of_render(&render(&history, 2));
        history.switch_stack(&working, &stack(2), &renders, None).unwrap();
        assert!(kept_or_named(&history).is_empty());
        // Coming back starts from the newest render, #2.
        let back = history
            .switch_stack(&history.read_working().unwrap(), &stack(1), &renders, None)
            .unwrap();
        assert_eq!(back.unwrap(), working);
    }

    #[test]
    fn one_reverted_to_an_older_render_is_kept() {
        let (_root, history) = two_stacks();
        let renders = history.list().unwrap();
        // Reverted to #1: unedited, but not what #2 would rebuild.
        let working = Working::of_render(&render(&history, 1));
        history.switch_stack(&working, &stack(2), &renders, None).unwrap();
        let back = history
            .switch_stack(&history.read_working().unwrap(), &stack(1), &renders, None)
            .unwrap();
        assert_eq!(back.unwrap(), working);
    }

    #[test]
    fn a_stack_never_stacked_is_kept() {
        let (_root, history) = two_stacks();
        let renders = history.list().unwrap();
        let mut fresh = Working::new(None);
        fresh.list.add_files(vec![frame(&history, "/a/Fly/1.jpg")]);
        history.switch_stack(&fresh, &stack(2), &renders, None).unwrap();
        assert!(history.stacks().unwrap()[&fresh.stack].kept());
        let back =
            history.switch_stack(&history.read_working().unwrap(), &fresh.stack, &renders, None);
        assert_eq!(back.unwrap(), Some(fresh));

        // One with no frames is nothing to come back to.
        let empty = Working::new(None);
        history.switch_stack(&empty, &stack(2), &renders, None).unwrap();
        assert!(!history.stacks().unwrap().contains_key(&empty.stack));
    }

    /// A stack never stacked is listed with its kept working version,
    /// named after its frames' folder, and the stacks with renders aren't.
    #[test]
    fn a_stack_never_stacked_is_listed_alone() {
        let (_root, history) = two_stacks();
        let renders = history.list().unwrap();
        let mut fly = Working::new(None);
        fly.list.add_files(vec![frame(&history, "/a/Fly/1.jpg"), frame(&history, "/a/Fly/2.jpg")]);
        history.switch_stack(&fly, &stack(1), &renders, None).unwrap().unwrap();
        let stacks = history.stacks().unwrap();
        let info = &stacks[&fly.stack];
        assert_eq!(info.working.as_ref(), Some(&fly));
        assert_eq!(info.unstacked_name().as_deref(), Some("Fly"));
        let listed: Vec<&StackId> = unstacked(&stacks, &renders).into_iter().map(|u| u.0).collect();
        assert_eq!(listed, [&fly.stack], "not the stacks with renders");
    }

    #[test]
    fn a_stack_with_no_render_or_kept_version_cannot_be_switched_to() {
        let (_root, history) = two_stacks();
        let renders = history.list().unwrap();
        let working = edited(&history);
        assert_eq!(history.switch_stack(&working, &stack(9), &renders, None).unwrap(), None);
        assert!(kept_or_named(&history).is_empty(), "nothing changed");
        assert!(history.read_working().unwrap().list.is_empty(), "list.json untouched");

        // A failed batch stack's frames are its working version.
        let mut failed = Working::new(None);
        failed.stack = stack(9);
        failed.list.add_files(vec![frame(&history, "/a/Fly/1.jpg")]);
        let retried = history.switch_stack(&working, &stack(9), &renders, Some(failed.clone()));
        assert_eq!(retried.unwrap(), Some(failed));
        assert!(history.stacks().unwrap()[&stack(1)].kept(), "the edits are kept");
    }

    #[test]
    fn stacks_are_named_after_their_folder_until_renamed() {
        let (_root, history) = two_stacks();
        let renders = history.list().unwrap();
        assert_eq!(stack_name(&stack(1), None, &renders).as_deref(), Some("Bee"));
        history.rename_stack(&stack(1), Some("  Bee on mint  ")).unwrap();
        let stacks = history.stacks().unwrap();
        assert_eq!(
            stack_name(&stack(1), stacks.get(&stack(1)), &renders).as_deref(),
            Some("Bee on mint")
        );
        assert!(!stacks[&stack(1)].kept());
        history.rename_stack(&stack(1), Some(" ")).unwrap();
        assert!(kept_or_named(&history).is_empty(), "no name, nothing kept");
    }

    /// Reading again with reuse matches a fresh read after a rename, a keep,
    /// a delete and damage.
    #[test]
    fn stacks_read_again_follow_every_change() {
        let (_root, history) = two_stacks();
        let mut reads = StackReads::default();
        let check = |reads: &mut StackReads| {
            assert_eq!(history.stacks_reusing(reads).unwrap(), history.stacks().unwrap());
        };
        check(&mut reads);
        history.rename_stack(&stack(1), Some("Bee")).unwrap();
        check(&mut reads);
        history.rename_stack(&stack(1), Some("Bees")).unwrap();
        check(&mut reads);
        assert_eq!(
            history.stacks_reusing(&mut reads).unwrap()[&stack(1)].name.as_deref(),
            Some("Bees")
        );
        let renders = history.list().unwrap();
        history.switch_stack(&edited(&history), &stack(2), &renders, None).unwrap();
        check(&mut reads);
        assert!(history.stacks_reusing(&mut reads).unwrap()[&stack(1)].kept());
        std::fs::write(history.stack_path(&stack(1)), b"{ not json").unwrap();
        check(&mut reads);
        history.rename_stack(&stack(1), None).unwrap();
        history.rename_stack(&stack(2), Some("Ant")).unwrap();
        check(&mut reads);
        assert_eq!(reads.files.len(), history.stacks().unwrap().len());
    }

    #[test]
    fn a_stack_whose_last_render_is_deleted_is_forgotten_once_it_goes_for_good() {
        let (_root, history) = two_stacks();
        history.rename_stack(&stack(1), Some("Bee")).unwrap();
        history.rename_stack(&stack(2), Some("Ant")).unwrap();
        history
            .delete_renders(
                &[RenderId::parse(&id(1)).unwrap()],
                history.close_paint(&[RenderId::parse(&id(1)).unwrap()]),
            )
            .unwrap();
        assert_eq!(history.stacks().unwrap().len(), 2, "stack 1 still has #2");

        // Stack 1 has edits kept; stack 2 has nothing but its name.
        let renders = history.list().unwrap();
        history.switch_stack(&edited(&history), &stack(2), &renders, None).unwrap();
        // Neither is on screen: the list was cleared.
        history.write_working(&Working::new(None)).unwrap();
        let two = [RenderId::parse(&id(2)).unwrap(), RenderId::parse(&id(3)).unwrap()];
        history.delete_renders(&two, history.close_paint(&two)).unwrap();
        // In Recently Deleted, to be restored to them: both kept, named.
        let stacks = history.stacks().unwrap();
        assert_eq!(stacks[&stack(2)].name.as_deref(), Some("Ant"));
        history.empty_deleted().unwrap();
        let stacks = history.stacks().unwrap();
        assert_eq!(stacks.len(), 1);
        assert_eq!(stacks[&stack(1)].name.as_deref(), Some("Bee"));
        assert!(stacks[&stack(1)].kept());
    }

    /// A save landing after its stack left the screen rebases the kept
    /// working version on it, or drops it when it is that render.
    #[test]
    fn a_render_saved_off_screen_is_its_kept_working_versions_base() {
        let (_root, history) = two_stacks();
        let renders = history.list().unwrap();
        let from = render(&history, 2);
        // Stacked with frame 1 excluded, then stack 2 opened before the
        // save: kept as edited since #2.
        let stacked = edited(&history);
        history.switch_stack(&stacked, &stack(2), &renders, None).unwrap().unwrap();
        assert_eq!(history.stacks().unwrap()[&stack(1)].working, Some(stacked.clone()));

        // The save lands: a render with that list and those settings.
        let mut saved = from.clone();
        saved.id = RenderId::parse(&id(9)).unwrap();
        saved.number = 3;
        saved.included[1] = false;
        Settings { config: saved.config, options: saved.options } = stacked.settings.unwrap();
        assert!(stacked.is_render_again(&saved), "the test's render is what was stacked");

        // Reverted to another render meanwhile: left alone.
        history.stacked_off_screen(&saved, None).unwrap();
        assert_eq!(history.stacks().unwrap()[&stack(1)].working, Some(stacked.clone()));

        history.stacked_off_screen(&saved, Some(&from.id)).unwrap();
        assert!(history.stacks().unwrap()[&stack(1)].working.is_none(), "the same as #3: dropped");

        // Edited again after Stack and before the switch: based on #3.
        let mut more = stacked.clone();
        more.list.add_files(vec![frame(&history, "/a/Bee/9.jpg")]);
        history
            .edit_stack_file(&stack(1), |f| {
                f.working = Some(KeptFile::of(&more, history.frames()).unwrap());
                true
            })
            .unwrap();
        history.stacked_off_screen(&saved, Some(&from.id)).unwrap();
        let kept = history.stacks().unwrap().remove(&stack(1)).unwrap().working.unwrap();
        assert_eq!(kept.base, Some(saved.id.clone()));
        assert_eq!(kept.list, more.list);
    }

    /// Edits of one stack's file from several threads all land.
    #[test]
    fn edits_of_a_stack_file_from_several_threads_all_land() {
        let (_root, history) = two_stacks();
        let (threads, each) = (8, 25);
        std::thread::scope(|scope| {
            for _ in 0..threads {
                scope.spawn(|| {
                    for _ in 0..each {
                        history
                            .edit_stack_file(&stack(1), |f| {
                                f.last_number += 1;
                                true
                            })
                            .unwrap();
                    }
                });
            }
        });
        let last = history.read_stack_file(&stack(1)).unwrap().unwrap().last_number;
        assert_eq!(last, 2 + threads * each);
    }

    /// The stack on screen keeps its name when its last render is deleted:
    /// its frames are still in the list, to stack again.
    #[test]
    fn deleting_the_last_render_of_the_stack_on_screen_keeps_its_name() {
        let (_root, history) = two_stacks();
        history.rename_stack(&stack(2), Some("Ant")).unwrap();
        let ant_render = render(&history, 3);
        let ant = Working::of_render(&ant_render);
        history.write_working(&ant).unwrap();
        history
            .delete_renders(
                &[RenderId::parse(&id(3)).unwrap()],
                history.close_paint(&[RenderId::parse(&id(3)).unwrap()]),
            )
            .unwrap();
        assert_eq!(history.stacks().unwrap()[&stack(2)].name.as_deref(), Some("Ant"));
        history.empty_deleted().unwrap();
        assert_eq!(history.stacks().unwrap()[&stack(2)].name.as_deref(), Some("Ant"));
        drop(ant_render);

        // Cleared, it is off screen: the next purge forgets it.
        history.write_working(&Working::new(None)).unwrap();
        history.forget_if_unused(&[stack(2)]);
        assert!(!history.stacks().unwrap().contains_key(&stack(2)));
    }

    #[test]
    fn delete_others_keeps_the_render_and_the_starred() {
        let (_root, history) = two_stacks();
        history.set_render_picked(&RenderId::parse(&id(1)).unwrap(), true).unwrap();
        put_render_in(&history, &id(4), &["/a/Bee/1.jpg"], None, Some((stack(1).as_str(), 3)));
        let renders = history.list().unwrap();
        let keep = render(&history, 4);
        let others: Vec<u32> = others_of(&keep, &renders).iter().map(|r| r.number).collect();
        assert_eq!(others, [2], "not #3 itself, starred #1, or the other stack's");
    }

    #[test]
    fn keep_only_starred_deletes_the_unstarred_of_a_stack_with_a_starred_one() {
        let (_root, history) = two_stacks();
        put_render_in(&history, &id(4), &["/a/Bee/1.jpg"], None, Some((stack(1).as_str(), 3)));
        let numbers = |renders: &[Render]| -> Vec<u32> {
            unstarred_of(&stack(1), renders).iter().map(|r| r.number).collect()
        };
        assert!(numbers(&history.list().unwrap()).is_empty(), "none starred: nothing to keep");
        history.set_render_picked(&RenderId::parse(&id(1)).unwrap(), true).unwrap();
        assert_eq!(
            numbers(&history.list().unwrap()),
            [2, 3],
            "not starred #1 or the other stack's"
        );
    }

    #[test]
    fn every_render_has_a_size() {
        let (_root, history) = two_stacks();
        let sizes = history.render_sizes();
        assert_eq!(sizes.len(), 3);
        assert!(sizes.values().all(|bytes| *bytes > 0), "at least their render.json");
    }

    /// The number the next render of `stack(n)` gets, as a front end
    /// tells it from the store's renders and stacks.
    fn next_of(history: &History, n: u8) -> u32 {
        next_number(&stack(n), &history.list().unwrap(), history.stacks().unwrap().get(&stack(n)))
    }

    /// Deleting the newest render doesn't free its number, and a kept
    /// working version is rebased on the newest left.
    #[test]
    fn a_deleted_number_is_not_given_again() {
        let (_root, history) = two_stacks();
        let renders = history.list().unwrap();
        assert_eq!(next_of(&history, 1), 3);
        // Stack 1 edited since #2, then left for stack 2.
        let kept = edited(&history);
        history.switch_stack(&kept, &stack(2), &renders, None).unwrap().unwrap();

        history
            .delete_renders(
                &[RenderId::parse(&id(2)).unwrap()],
                history.close_paint(&[RenderId::parse(&id(2)).unwrap()]),
            )
            .unwrap();
        assert_eq!(next_of(&history, 1), 3, "#2 is not given again");
        let now = history.stacks().unwrap().remove(&stack(1)).unwrap().working.unwrap();
        assert_eq!(now.base, RenderId::parse(&id(1)), "based on #1, the newest left");
        assert_eq!(now.list, kept.list);

        // An older one deleted changes nothing; the last one leaves the
        // kept working version, never stacked, and still no #1 or #2.
        history
            .delete_renders(
                &[RenderId::parse(&id(1)).unwrap()],
                history.close_paint(&[RenderId::parse(&id(1)).unwrap()]),
            )
            .unwrap();
        assert_eq!(next_of(&history, 1), 3);
        assert_eq!(history.claim_number(&stack(1)).unwrap(), 3);
        assert_eq!(history.stacks().unwrap()[&stack(1)].working.as_ref().unwrap().base, None);
    }

    /// A stack whose file is gone, with its renders still listed, numbers
    /// on from the highest of them, never #1 again.
    #[test]
    fn a_lost_stack_file_numbers_on_from_the_renders_listed() {
        let (_root, history) = two_stacks();
        put_render_in(&history, &id(5), &["/a/Bee/1.jpg"], None, Some((stack(1).as_str(), 7)));
        std::fs::remove_file(history.stack_path(&stack(1))).unwrap();
        assert_eq!(history.claim_number(&stack(1)).unwrap(), 8);
        assert_eq!(history.claim_number(&stack(1)).unwrap(), 9, "from the file again");
    }

    /// A save numbers from the stack's file and numbers only go up across
    /// deletes; a stack whose last render goes for good is forgotten.
    #[test]
    fn saves_number_from_the_stack_file_and_never_go_back() {
        let (_root, history) = two_stacks();
        assert_eq!(history.claim_number(&stack(1)).unwrap(), 3);
        put_render_in(&history, &id(5), &["/a/Bee/1.jpg"], None, Some((stack(1).as_str(), 3)));
        assert_eq!(history.read_stack_file(&stack(1)).unwrap().unwrap().last_number, 3);
        // A render the store didn't number (written behind its back) shows
        // the file is what a save reads.
        let manifest = history.renders_dir().join(id(1)).join(RENDER_FILE);
        let edited =
            std::fs::read_to_string(&manifest).unwrap().replace("\"number\":1", "\"number\":9");
        std::fs::write(&manifest, edited).unwrap();
        assert_eq!(history.claim_number(&stack(1)).unwrap(), 4);

        // The newest (#4's claim never saved, #3) deleted: neither is given
        // again.
        history
            .delete_renders(
                &[RenderId::parse(&id(5)).unwrap()],
                history.close_paint(&[RenderId::parse(&id(5)).unwrap()]),
            )
            .unwrap();
        assert_eq!(history.claim_number(&stack(1)).unwrap(), 5);
        assert_eq!(history.claim_number(&stack(1)).unwrap(), 6, "a claim is never given twice");
        let renders = history.list().unwrap();

        // Every render of stack 2 deleted, nothing kept, and gone for good
        // from Recently Deleted: forgotten, and its file with it.
        let of_two: Vec<RenderId> =
            renders.iter().filter(|r| r.stack == stack(2)).map(|r| r.id.clone()).collect();
        history.delete_renders(&of_two, history.close_paint(&of_two)).unwrap();
        assert!(history.read_stack_file(&stack(2)).unwrap().is_some(), "kept while deleted");
        history.empty_deleted().unwrap();
        assert!(history.read_stack_file(&stack(2)).unwrap().is_none());
        assert_eq!(next_of(&history, 2), 1);
        assert_eq!(history.claim_number(&stack(2)).unwrap(), 1);
    }

    /// A damaged stack file is set aside whole by the next edit and removed
    /// with the Library; an unreadable one is never replaced.
    #[test]
    fn a_damaged_stack_file_is_set_aside_and_replaced() {
        let (_root, history) = two_stacks();
        let path = history.stack_path(&stack(1));
        let newer = format!("{{\"schema\": {}, \"name\": \"Bee\"}}", STACK_SCHEMA + 1);
        for (n, damaged) in ["not json", newer.as_str()].into_iter().enumerate() {
            std::fs::write(&path, damaged).unwrap();
            history.rename_stack(&stack(1), Some("Ant")).unwrap();
            assert_eq!(history.stacks().unwrap()[&stack(1)].name.as_deref(), Some("Ant"));
            let aside = history.stack_files_set_aside();
            assert_eq!(aside.len(), n + 1);
            let kept: Vec<_> = aside.iter().map(|p| std::fs::read_to_string(p).unwrap()).collect();
            assert!(kept.iter().any(|k| k == damaged), "{kept:?}");
        }
        // An edit that changes nothing leaves a damaged file where it is.
        std::fs::write(&path, "not json").unwrap();
        history.edit_stack_file(&stack(1), |_| false).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not json");
        assert_eq!(history.stack_files_set_aside().len(), 2);

        history.delete_library(history.close_all_paint()).unwrap();
        assert!(history.stack_files_set_aside().is_empty());
    }

    /// Relinking a folder reaches the renders and kept working versions,
    /// though neither file is written.
    #[test]
    fn relinking_reaches_the_kept_working_versions() {
        let (root, history) = two_stacks();
        let renders = history.list().unwrap();
        let (old, new) = (root.path().join("old"), root.path().join("new"));
        let one = frame(&history, "/old/Fly/1.jpg");
        put_render_in(&history, &id(7), &["/old/Fly/1.jpg"], None, None);
        let mut fly = Working::new(None);
        fly.list.add_files(vec![one]);
        history.switch_stack(&fly, &stack(1), &renders, None).unwrap().unwrap();

        // A copy: the frames are still where they were.
        frame(&history, "/new/Fly/1.jpg");
        history.relink_folder(&old, &new).unwrap();
        let moved = [new.join("Fly/1.jpg")];
        assert_eq!(render(&history, 7).frames, moved);
        let kept = history.stacks().unwrap().remove(&fly.stack).unwrap().working.unwrap();
        assert_eq!(kept.list.files, moved);
    }

    /// A render whose stack isn't an id this store writes can't be read:
    /// it is skipped.
    #[test]
    fn a_render_with_a_bad_stack_id_is_skipped() {
        let (_root, history) = store();
        put_render_in(&history, &id(1), &["/a/Bee/1.jpg"], None, Some(("../../etc", 3)));
        assert!(history.list().unwrap().is_empty());
    }
}
