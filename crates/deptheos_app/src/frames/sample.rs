//! The sample stacks the apps ship (`samples/hornet`, `samples/pcb`), for
//! Try a Sample Stack, and their copy into the desktop Library's Samples or
//! iOS's Imports.
//!
//! Frames are copied, since a stack keeps its frames' paths (ADR-151). A
//! copy is whole or nothing, made after a free-space check, and reused, so
//! each sample takes its space once.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::fsutil::io_error;
use crate::{Error, Result};

/// The sample stacks a build may carry, in the order they are offered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SampleStack {
    /// A hornet on a branch: 66 frames, handheld.
    Hornet,
    /// A circuit board: 200 frames, on a tripod with a focus rail.
    CircuitBoard,
}

impl SampleStack {
    pub const ALL: [Self; 2] = [Self::Hornet, Self::CircuitBoard];

    /// Its folder where a build carries it, under a `samples` folder.
    pub fn bundled_folder(self) -> &'static str {
        match self {
            Self::Hornet => "hornet",
            Self::CircuitBoard => "pcb",
        }
    }

    /// Its copy's folder name, and so its stacks' names; not translated.
    pub fn name(self) -> &'static str {
        match self {
            Self::Hornet => "Hornet",
            Self::CircuitBoard => "Circuit Board",
        }
    }

    /// Shot with the camera held in the hand; else on a tripod.
    pub fn handheld(self) -> bool {
        matches!(self, Self::Hornet)
    }
}

/// A sample stack a build carries: the JPEGs directly in its folder, by
/// name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sample {
    stack: SampleStack,
    dir: PathBuf,
    frames: Vec<Frame>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Frame {
    name: OsString,
    len: u64,
}

/// A sample's copy: its folder and frames, and whether it was there
/// already.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Copied {
    pub folder: PathBuf,
    pub frames: Vec<PathBuf>,
    pub reused: bool,
}

impl Sample {
    /// `stack`'s frames in `dir`: its JPEGs, two at least.
    pub fn read(stack: SampleStack, dir: &Path) -> Option<Self> {
        let mut frames = jpegs(dir)?;
        if frames.len() < 2 {
            return None;
        }
        frames.sort_by(|a, b| a.name.cmp(&b.name));
        Some(Self { stack, dir: dir.to_path_buf(), frames })
    }

    /// The first of `places` that holds `stack`.
    pub fn find(stack: SampleStack, places: &[PathBuf]) -> Option<Self> {
        places.iter().find_map(|dir| Self::read(stack, dir))
    }

    pub fn stack(&self) -> SampleStack {
        self.stack
    }

    /// Where the build carries it.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    /// The bytes its frames take.
    pub fn bytes(&self) -> u64 {
        self.frames.iter().map(|f| f.len).sum()
    }

    /// Its copy in `into`: a whole one already there ("Hornet", "Hornet 2",
    /// …), else a new one under the first free name, copied into a hidden
    /// folder and renamed into place when complete. Leftover partials are
    /// removed first. `free` overrides the measured free space. Blocks.
    pub fn copy_into(&self, into: &Path, free: Option<u64>) -> Result<Copied> {
        std::fs::create_dir_all(into).map_err(io_error(into))?;
        self.remove_partials_in(into);
        if let Some(folder) = self.copies_in(into).into_iter().find(|f| self.is_copy(f)) {
            tracing::info!("the sample stack's copy at {} is whole", folder.display());
            return Ok(self.copied(folder, true));
        }
        let needed = self.bytes();
        if let Some(free) = free.or_else(|| crate::fsutil::free_space(into))
            && needed > free
        {
            return Err(Error::SampleNoSpace { needed, free });
        }
        let folder = (1..)
            .map(|n| into.join(self.folder_name(n)))
            .find(|f| std::fs::symlink_metadata(f).is_err())
            .expect("a free name");
        let partial =
            into.join(format!("{}{}", self.partial_prefix(), crate::library::ids::new_id()));
        let made = self.copy_frames(&partial).and_then(|()| {
            std::fs::rename(&partial, &folder).map_err(io_error(&folder)).map_err(Error::from)
        });
        if let Err(e) = made {
            // Whole, or not at all: a copy short of frames is no sample.
            let _ = std::fs::remove_dir_all(&partial);
            return Err(e);
        }
        tracing::info!("copied the sample stack's frames to {}", folder.display());
        Ok(self.copied(folder, false))
    }

    /// Remove its cut-short copies in `into` (`.Name.partial-<id>`).
    fn remove_partials_in(&self, into: &Path) {
        let prefix = self.partial_prefix();
        let Ok(entries) = std::fs::read_dir(into) else { return };
        for entry in entries.filter_map(std::result::Result::ok) {
            if !entry.file_name().to_string_lossy().starts_with(&prefix) {
                continue;
            }
            let path = entry.path();
            match std::fs::remove_dir_all(&path) {
                Ok(()) => tracing::info!("removed a sample copy cut short, {}", path.display()),
                Err(e) => tracing::warn!("{} couldn't be removed: {e}", path.display()),
            }
        }
    }

    /// The start of the hidden folder's name a copy is made in.
    fn partial_prefix(&self) -> String {
        format!(".{}.partial-", self.stack.name())
    }

    /// Its `n`th copy's name: its name, then "Name 2", "Name 3".
    fn folder_name(&self, n: u32) -> String {
        match n {
            1 => self.stack.name().to_string(),
            n => format!("{} {n}", self.stack.name()),
        }
    }

    /// The folders in `into` named as its copies are.
    fn copies_in(&self, into: &Path) -> Vec<PathBuf> {
        let name = self.stack.name();
        let Ok(entries) = std::fs::read_dir(into) else { return Vec::new() };
        let mut found: Vec<PathBuf> = entries
            .filter_map(std::result::Result::ok)
            .filter(|e| {
                let file = e.file_name();
                let file = file.to_string_lossy();
                file == name
                    || file
                        .strip_prefix(name)
                        .and_then(|n| n.strip_prefix(' '))
                        .is_some_and(|n| n.parse::<u32>().is_ok())
            })
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        found.sort();
        found
    }

    /// `folder` holds its frames at their sizes, and nothing else visible.
    fn is_copy(&self, folder: &Path) -> bool {
        let Ok(entries) = std::fs::read_dir(folder) else { return false };
        let mut files: Vec<Frame> = entries
            .filter_map(std::result::Result::ok)
            .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
            .filter_map(|e| Some(Frame { name: e.file_name(), len: e.metadata().ok()?.len() }))
            .collect();
        files.sort_by(|a, b| a.name.cmp(&b.name));
        files == self.frames
    }

    fn copy_frames(&self, to: &Path) -> Result<()> {
        std::fs::create_dir_all(to).map_err(io_error(to))?;
        for frame in &self.frames {
            // On a volume that clones, no time and no space until changed.
            let from = self.dir.join(&frame.name);
            std::fs::copy(&from, to.join(&frame.name)).map_err(io_error(&from))?;
        }
        Ok(())
    }

    fn copied(&self, folder: PathBuf, reused: bool) -> Copied {
        let frames = self.frames.iter().map(|f| folder.join(&f.name)).collect();
        Copied { folder, frames, reused }
    }
}

/// The JPEG files directly in `dir`, unsorted.
fn jpegs(dir: &Path) -> Option<Vec<Frame>> {
    let frames = std::fs::read_dir(dir)
        .ok()?
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|e| e.eq_ignore_ascii_case("jpg")))
        .filter_map(|entry| {
            let meta = entry.metadata().ok().filter(|m| m.is_file())?;
            Some(Frame { name: entry.file_name(), len: meta.len() })
        });
    Some(frames.collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(dir: &Path, names: &[&str]) {
        std::fs::create_dir_all(dir).unwrap();
        for name in names {
            std::fs::write(dir.join(name), name.as_bytes()).unwrap();
        }
    }

    fn hornet(dir: &Path) -> Sample {
        let shipped = dir.join("shipped");
        frames(&shipped, &["hornet-01.jpg", "hornet-02.jpg", "hornet-03.jpg"]);
        Sample::read(SampleStack::Hornet, &shipped).unwrap()
    }

    /// A folder with fewer than two JPEGs is no sample.
    #[test]
    fn a_folder_without_frames_is_no_sample() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Sample::read(SampleStack::Hornet, &dir.path().join("none")), None);
        frames(dir.path(), &["one.jpg", "notes.txt"]);
        assert_eq!(Sample::read(SampleStack::Hornet, dir.path()), None);
    }

    /// A whole copy is reused, and nothing else is left behind.
    #[test]
    fn copies_once_and_uses_a_whole_copy_again() {
        let dir = tempfile::tempdir().unwrap();
        let sample = hornet(dir.path());
        let into = dir.path().join("Samples");
        let copied = sample.copy_into(&into, None).unwrap();
        let to = into.join("Hornet");
        assert_eq!((&copied.folder, copied.reused), (&to, false));
        let names = ["hornet-01.jpg", "hornet-02.jpg", "hornet-03.jpg"];
        assert_eq!(copied.frames, names.map(|n| to.join(n)));
        assert!(copied.frames.iter().all(|p| p.is_file()));
        let again = sample.copy_into(&into, None).unwrap();
        assert_eq!((again.folder, again.reused), (to, true));
        let entries: Vec<_> =
            std::fs::read_dir(&into).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(entries, ["Hornet"], "no partial folder left");
    }

    /// A partial or altered copy isn't reused: "Hornet 2" goes beside it.
    #[test]
    fn a_damaged_copy_is_left_for_a_new_one() {
        let dir = tempfile::tempdir().unwrap();
        let sample = hornet(dir.path());
        let into = dir.path().join("Imports");
        let first = sample.copy_into(&into, None).unwrap();
        std::fs::write(&first.frames[1], b"x").unwrap();
        let second = sample.copy_into(&into, None).unwrap();
        assert_eq!((second.folder.clone(), second.reused), (into.join("Hornet 2"), false));
        assert_eq!(sample.copy_into(&into, None).unwrap().folder, second.folder);
        std::fs::write(second.folder.join("notes.txt"), b"").unwrap();
        assert_eq!(sample.copy_into(&into, None).unwrap().folder, into.join("Hornet 3"));
    }

    /// The next copy removes its own sample's partial folder, not another's.
    #[test]
    fn a_copy_cut_short_is_removed_by_the_next() {
        let dir = tempfile::tempdir().unwrap();
        let sample = hornet(dir.path());
        let into = dir.path().join("Imports");
        frames(&into.join(".Hornet.partial-0123"), &["hornet-01.jpg"]);
        frames(&into.join(".Circuit Board.partial-0123"), &["pcb-001.jpg"]);
        sample.copy_into(&into, None).unwrap();
        let mut entries: Vec<_> =
            std::fs::read_dir(&into).unwrap().map(|e| e.unwrap().file_name()).collect();
        entries.sort();
        assert_eq!(entries, [".Circuit Board.partial-0123", "Hornet"]);
    }

    /// Too little space copies nothing; the error says needed and free.
    #[test]
    fn too_little_space_copies_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let sample = hornet(dir.path());
        let into = dir.path().join("Imports");
        let refused = sample.copy_into(&into, Some(10));
        assert!(
            matches!(refused, Err(Error::SampleNoSpace { needed, free: 10 }) if needed == sample.bytes()),
            "{refused:?}"
        );
        assert_eq!(std::fs::read_dir(&into).unwrap().count(), 0);
    }

    /// A frame that can't be read leaves no copy behind.
    #[test]
    fn a_failed_copy_leaves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let sample = hornet(dir.path());
        std::fs::remove_file(dir.path().join("shipped/hornet-02.jpg")).unwrap();
        let into = dir.path().join("Imports");
        assert!(sample.copy_into(&into, None).is_err());
        assert_eq!(std::fs::read_dir(&into).unwrap().count(), 0);
    }

    /// The circuit board's goes beside the hornet's, by its name.
    #[test]
    fn each_sample_has_its_own_folder() {
        let dir = tempfile::tempdir().unwrap();
        let into = dir.path().join("Samples");
        hornet(dir.path()).copy_into(&into, None).unwrap();
        let board = dir.path().join("board");
        frames(&board, &["pcb-001.jpg", "pcb-002.jpg"]);
        let board = Sample::read(SampleStack::CircuitBoard, &board).unwrap();
        let copied = board.copy_into(&into, None).unwrap();
        assert_eq!(copied.folder, into.join("Circuit Board"));
        assert_eq!(std::fs::read_dir(into.join("Hornet")).unwrap().count(), 3);
    }
}
