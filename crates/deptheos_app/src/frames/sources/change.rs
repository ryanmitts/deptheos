//! Changing the roots: add, relink, split, prune and renest. Each writes
//! `sources.json` before it returns.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::probe::{FileId, Fingerprint, Mounted, Probe, Same};
use super::{
    Anchor, Bookmark, FrameRef, InUse, Place, Relinked, Renamed, Result, Root, RootId, RootState,
    Sources, SourcesError, Under, depth_of, folder_state, io_error, join, join_rel, rel_string,
    strip_rel,
};

/// Only roots of one class nest, so a root under the Library stays anchored
/// even when a volume root holds the Library.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    Volume,
    Anchored(Anchor),
}

impl Sources {
    /// Add `files` from `folder`, which becomes a root unless one covers it;
    /// roots inside it are taken in. Returns the frames (by identity) and
    /// any renamed by renesting.
    pub fn add_folder(
        &mut self,
        probe: &dyn Probe,
        folder: &Path,
        files: &[PathBuf],
        bookmark: Option<Bookmark>,
    ) -> Result<(Vec<FrameRef>, Vec<Renamed>)> {
        let mut found = Vec::with_capacity(files.len());
        for file in files {
            let rest = file.strip_prefix(folder).map_err(|_| SourcesError::NotInside {
                file: file.clone(),
                folder: folder.to_path_buf(),
            })?;
            let rel = rel_string(rest, file)?;
            let stat = probe
                .stat(file)
                .map_err(io_error(file))?
                .filter(|s| !s.folder)
                .ok_or_else(|| SourcesError::NotFile(file.clone()))?;
            let fingerprint = Fingerprint::of(probe, file, stat.size).map_err(io_error(file))?;
            found.push((rel, stat.size, fingerprint));
        }
        let canon = self.folder(probe, folder)?;
        let (root, prefix) = match self.covering(probe, &canon) {
            Some((id, prefix)) => {
                let bare = matches!(
                    self.roots.get(&id).map(|r| &r.place),
                    Some(Place::Volume { bookmark: None, .. })
                );
                if prefix.is_empty()
                    && bare
                    && bookmark.is_some()
                    && let Some(Root { place: Place::Volume { bookmark: kept, .. }, .. }) =
                        self.roots.get_mut(&id)
                {
                    *kept = bookmark;
                }
                (id, prefix)
            }
            None => {
                let id = RootId::new();
                let place = self.place_for(probe, &canon, bookmark)?;
                tracing::debug!("sources: root {id} added at {}", canon.display());
                self.roots.insert(id.clone(), Root::at(place));
                (id, String::new())
            }
        };
        let renamed = self.renest(probe);
        self.save()?;
        let frames = found
            .into_iter()
            .map(|(rel, size, fingerprint)| {
                let path = join_rel(&prefix, &rel);
                self.identity(&FrameRef { root: root.clone(), path, size, fingerprint })
            })
            .collect();
        Ok((frames, renamed))
    }

    /// Add files picked one by one, by folder ([`Self::add_folder`]).
    pub fn add_files(
        &mut self,
        probe: &dyn Probe,
        files: &[PathBuf],
    ) -> Result<(Vec<FrameRef>, Vec<Renamed>)> {
        let mut by_folder: Vec<(PathBuf, Vec<(usize, PathBuf)>)> = Vec::new();
        for (i, file) in files.iter().enumerate() {
            let folder = file.parent().unwrap_or(Path::new("")).to_path_buf();
            match by_folder.iter_mut().find(|(f, _)| *f == folder) {
                Some((_, list)) => list.push((i, file.clone())),
                None => by_folder.push((folder, vec![(i, file.clone())])),
            }
        }
        let mut frames: Vec<Option<FrameRef>> = vec![None; files.len()];
        let mut renamed = Vec::new();
        for (folder, list) in by_folder {
            let paths: Vec<PathBuf> = list.iter().map(|(_, p)| p.clone()).collect();
            let (added, more) = self.add_folder(probe, &folder, &paths, None)?;
            for ((i, _), frame) in list.into_iter().zip(added) {
                frames[i] = Some(frame);
            }
            renamed.extend(more);
        }
        Ok((frames.into_iter().flatten().collect(), renamed))
    }

    /// Keep a folder the user picked (iOS) as [`Self::add_folder`] does. A
    /// top root takes the pick's newer bookmark; a nested folder keeps none.
    /// Returns the top root covering it.
    pub fn keep_folder(
        &mut self,
        probe: &dyn Probe,
        folder: &Path,
        bookmark: Bookmark,
    ) -> Result<RootId> {
        self.add_folder(probe, folder, &[], Some(bookmark.clone()))?;
        let canon = self.folder(probe, folder)?;
        let (id, prefix) = self
            .covering(probe, &canon)
            .ok_or_else(|| SourcesError::NotFolder(folder.to_path_buf()))?;
        let top = self.top_of(&id);
        let on_volume =
            matches!(self.roots.get(&top).map(|r| &r.place), Some(Place::Volume { .. }));
        if prefix.is_empty() && top == id && on_volume {
            self.set_bookmark(&top, Some(bookmark))?;
        }
        Ok(top)
    }

    /// The root `id` is within, at the top, or itself.
    pub fn top_of(&self, id: &RootId) -> RootId {
        let mut at = id.clone();
        for _ in 0..=self.roots.len() {
            match self.roots.get(&at).map(|r| &r.place) {
                Some(Place::Within { root, .. }) => at = root.clone(),
                _ => break,
            }
        }
        at
    }

    /// Point root `id` at `to` (Locate…, a found move, Copy into Deptheos).
    /// Its volume's other roots follow to the new mount, then roots renest.
    pub fn relink(
        &mut self,
        probe: &dyn Probe,
        id: &RootId,
        to: &Path,
        bookmark: Option<Bookmark>,
    ) -> Result<Relinked> {
        if !self.roots.contains_key(id) {
            return Err(SourcesError::UnknownRoot(id.clone()));
        }
        let canon = self.folder(probe, to)?;
        let mut relinked = Relinked::default();
        self.move_root(probe, id, &canon, bookmark, &mut relinked.moved)?;
        relinked.renamed = self.renest(probe);
        self.save()?;
        Ok(relinked)
    }

    /// Split `sub` of root `id` off as its own root at `to`, where it moved;
    /// one already a root is relinked. Roots under it follow.
    pub fn split(
        &mut self,
        probe: &dyn Probe,
        id: &RootId,
        sub: &str,
        to: &Path,
        bookmark: Option<Bookmark>,
    ) -> Result<Relinked> {
        if !self.roots.contains_key(id) {
            return Err(SourcesError::UnknownRoot(id.clone()));
        }
        if sub.is_empty() || sub.split('/').any(|n| matches!(n, "" | "." | "..")) {
            return Err(SourcesError::NotSubfolder(sub.to_string()));
        }
        let already = self.roots.iter().find_map(|(child, root)| {
            let nested =
                matches!(&root.place, Place::Within { root, path } if root == id && path == sub);
            let split = root.split_from.as_ref().is_some_and(|u| u.root == *id && u.path == sub);
            (nested || split).then(|| child.clone())
        });
        if let Some(child) = already {
            return self.relink(probe, &child, to, bookmark);
        }
        let canon = self.folder(probe, to)?;
        let new = RootId::new();
        let mut root = Root::at(self.place_for(probe, &canon, bookmark)?);
        root.split_from = Some(Under { root: id.clone(), path: sub.to_string() });
        for other in self.roots.values_mut() {
            if let Place::Within { root, path } = &mut other.place
                && let Some(rest) = (*root == *id).then(|| strip_rel(path, sub)).flatten()
            {
                (*root, *path) = (new.clone(), rest.to_string());
            }
            if let Some(under) = &mut other.split_from
                && let Some(rest) =
                    (under.root == *id).then(|| strip_rel(&under.path, sub)).flatten()
            {
                *under = Under { root: new.clone(), path: rest.to_string() };
            }
        }
        tracing::debug!("sources: {sub} of root {id} split off as {new} at {}", canon.display());
        self.roots.insert(new.clone(), root);
        let mut relinked = Relinked {
            moved: vec![new.clone()],
            renamed: vec![Renamed { from: id.clone(), path: sub.to_string(), to: new }],
        };
        relinked.renamed.extend(self.renest(probe));
        self.save()?;
        Ok(relinked)
    }

    /// Drop roots no Library file names, along with linked roots (within,
    /// split from, held) only when none of them is named. Returns those
    /// dropped.
    pub fn prune(&mut self, in_use: InUse) -> Result<Vec<RootId>> {
        let InUse::Read(named) = in_use else {
            tracing::debug!("sources: not pruned, the Library isn't read in full");
            return Ok(Vec::new());
        };
        let links = |id: &RootId| -> Vec<RootId> {
            let root = &self.roots[id];
            let mut to: Vec<RootId> = root.split_from.iter().map(|u| u.root.clone()).collect();
            if let Place::Within { root, .. } = &root.place {
                to.push(root.clone());
            }
            to.extend(self.roots.iter().filter_map(|(other, r)| {
                let within = matches!(&r.place, Place::Within { root, .. } if root == id);
                let split = r.split_from.as_ref().is_some_and(|u| u.root == *id);
                (within || split).then(|| other.clone())
            }));
            to
        };
        let mut kept: HashSet<RootId> = HashSet::new();
        let mut todo: Vec<RootId> =
            named.iter().filter(|id| self.roots.contains_key(*id)).cloned().collect();
        while let Some(id) = todo.pop() {
            if kept.insert(id.clone()) {
                todo.extend(links(&id).into_iter().filter(|l| self.roots.contains_key(l)));
            }
        }
        let dropped: Vec<RootId> =
            self.roots.keys().filter(|id| !kept.contains(*id)).cloned().collect();
        if dropped.is_empty() {
            return Ok(dropped);
        }
        for id in &dropped {
            tracing::debug!("sources: root {id} dropped, no file names it");
            self.roots.remove(id);
        }
        self.save()?;
        Ok(dropped)
    }

    /// The folder `path`, canonical, which must be a folder.
    pub(super) fn folder(&self, probe: &dyn Probe, path: &Path) -> Result<PathBuf> {
        let canon = probe.canonical(path).map_err(io_error(path))?;
        match probe.stat(&canon).map_err(io_error(&canon))? {
            Some(stat) if stat.folder => Ok(canon),
            _ => Err(SourcesError::NotFolder(path.to_path_buf())),
        }
    }

    pub(super) fn base(&self, anchor: Anchor) -> Option<&Path> {
        match anchor {
            Anchor::Library => Some(&self.places.library),
            Anchor::App => self.places.app.as_deref(),
        }
    }

    /// The deepest anchor containing `canon`, and the path from it.
    fn anchor_of(&self, canon: &Path) -> Option<(Anchor, String)> {
        [Anchor::Library, Anchor::App]
            .into_iter()
            .filter_map(|anchor| {
                let base = self.base(anchor)?;
                let rest = canon.strip_prefix(base).ok()?;
                Some((anchor, base.components().count(), rel_string(rest, canon).ok()?))
            })
            .max_by_key(|(_, depth, _)| *depth)
            .map(|(anchor, _, path)| (anchor, path))
    }

    /// The place for a root at `canon`: anchored, or on its volume.
    pub(super) fn place_for(
        &self,
        probe: &dyn Probe,
        canon: &Path,
        bookmark: Option<Bookmark>,
    ) -> Result<Place> {
        if let Some((anchor, path)) = self.anchor_of(canon) {
            return Ok(Place::Anchored { anchor, path });
        }
        let Mounted { volume, mount } = probe.volume(canon).map_err(io_error(canon))?;
        let rest = canon.strip_prefix(&mount).map_err(|_| SourcesError::NotInside {
            file: canon.to_path_buf(),
            folder: mount.clone(),
        })?;
        let path = rel_string(rest, canon)?;
        if mount.to_str().is_none() {
            return Err(SourcesError::NotUnicode(mount));
        }
        Ok(Place::Volume { volume, path, mount, bookmark })
    }

    fn class_of_place(&self, place: &Place, depth: usize) -> Class {
        match place {
            Place::Volume { .. } => Class::Volume,
            Place::Anchored { anchor, .. } => Class::Anchored(*anchor),
            Place::Within { root, .. } => match self.roots.get(root) {
                Some(parent) if depth < self.roots.len() => {
                    self.class_of_place(&parent.place, depth + 1)
                }
                _ => Class::Volume,
            },
        }
    }

    fn class_of(&self, id: &RootId) -> Class {
        self.roots.get(id).map_or(Class::Volume, |r| self.class_of_place(&r.place, 0))
    }

    fn class_of_path(&self, canon: &Path) -> Class {
        self.anchor_of(canon).map_or(Class::Volume, |(anchor, _)| Class::Anchored(anchor))
    }

    /// The deepest root covering `canon`, and the path from it.
    fn covering(&self, probe: &dyn Probe, canon: &Path) -> Option<(RootId, String)> {
        let class = self.class_of_path(canon);
        self.roots
            .keys()
            .filter(|id| self.class_of(id) == class)
            .filter_map(|id| match self.root_state(probe, id) {
                RootState::At(path) => Some((id.clone(), contains(probe, &path, canon)?)),
                _ => None,
            })
            .min_by_key(|(_, rel)| depth_of(rel))
    }

    /// Point root `id` at `canon`; its old volume's roots that are gone
    /// follow to the new mount where found. Each moved root goes in `moved`.
    pub(super) fn move_root(
        &mut self,
        probe: &dyn Probe,
        id: &RootId,
        canon: &Path,
        bookmark: Option<Bookmark>,
        moved: &mut Vec<RootId>,
    ) -> Result<()> {
        let old = match &self.roots[id].place {
            Place::Volume { volume, .. } => Some(volume.clone()),
            _ => None,
        };
        let place = self.place_for(probe, canon, bookmark)?;
        tracing::debug!("sources: root {id} is at {}", canon.display());
        let new = match &place {
            Place::Volume { volume, mount, .. } => Some((volume.clone(), mount.clone())),
            _ => None,
        };
        if let Some(root) = self.roots.get_mut(id) {
            root.place = place;
        }
        moved.push(id.clone());
        let (Some(old), Some((volume, mount))) = (old, new) else { return Ok(()) };
        if old.same(&volume) == Same::No {
            return Ok(());
        }
        let others: Vec<RootId> = self
            .roots
            .iter()
            .filter(|(other, root)| {
                *other != id
                    && matches!(&root.place, Place::Volume { volume: v, .. } if v.same(&old) == Same::Yes)
            })
            .map(|(other, _)| other.clone())
            .filter(|other| !matches!(self.root_state(probe, other), RootState::At(_)))
            .collect();
        for other in others {
            let Some(Root { place: Place::Volume { volume: v, path, mount: m, .. }, .. }) =
                self.roots.get_mut(&other)
            else {
                continue;
            };
            if let RootState::At(_) = folder_state(probe, join(&mount, path)) {
                tracing::debug!("sources: root {other} follows its volume to {}", mount.display());
                (*v, *m) = (volume.clone(), mount.clone());
                moved.push(other);
            }
        }
        Ok(())
    }

    /// Nest every top root found inside another of its class, so no folder
    /// has two roots; at one folder the older holds the newer.
    pub(super) fn renest(&mut self, probe: &dyn Probe) -> Vec<Renamed> {
        let tops: Vec<(RootId, Class, PathBuf)> = self
            .roots
            .iter()
            .filter(|(_, root)| !matches!(root.place, Place::Within { .. }))
            .filter_map(|(id, _)| match self.root_state(probe, id) {
                RootState::At(path) => Some((id.clone(), self.class_of(id), path)),
                _ => None,
            })
            .collect();
        let mut nests = Vec::new();
        for (inner, class, path) in &tops {
            let outer = tops
                .iter()
                .filter(|(outer, c, _)| outer != inner && c == class)
                .filter_map(|(outer, _, at)| Some((outer, contains(probe, at, path)?)))
                .filter(|(outer, rel)| !(rel.is_empty() && *outer > inner))
                .min_by_key(|(_, rel)| depth_of(rel));
            if let Some((outer, rel)) = outer {
                nests.push(Renamed { from: outer.clone(), path: rel, to: inner.clone() });
            }
        }
        for nest in &nests {
            tracing::debug!("sources: root {} is inside {} at {:?}", nest.to, nest.from, nest.path);
            if let Some(root) = self.roots.get_mut(&nest.to) {
                root.place = Place::Within { root: nest.from.clone(), path: nest.path.clone() };
            }
            self.reparent(nest);
        }
        nests
    }

    /// Move the outer root's descendants below `nest`'s folder under the
    /// nested root, as [`Self::split`] does, so each frame has one identity.
    fn reparent(&mut self, nest: &Renamed) {
        let Renamed { from, path: at, to } = nest;
        if at.is_empty() {
            // Two roots at one folder: what is inside stays with the older.
            return;
        }
        for (id, other) in self.roots.iter_mut().filter(|(id, _)| *id != to) {
            if let Place::Within { root, path } = &mut other.place
                && root == from
                && let Some(rest) = strip_rel(path, at).filter(|r| !r.is_empty())
            {
                tracing::debug!("sources: root {id} is inside {to} at {rest:?}");
                (*root, *path) = (to.clone(), rest.to_string());
            }
            if let Some(under) = &mut other.split_from
                && under.root == *from
                && let Some(rest) = strip_rel(&under.path, at).filter(|r| !r.is_empty())
            {
                *under = Under { root: to.clone(), path: rest.to_string() };
            }
        }
    }
}

/// `inner`'s path from `outer` when inside it, by path or file identity.
fn contains(probe: &dyn Probe, outer: &Path, inner: &Path) -> Option<String> {
    let outer_id = probe.stat(outer).ok().flatten().and_then(|s| s.id);
    let mut at = Some(inner);
    while let Some(ancestor) = at {
        if ancestor == outer || same_folder(probe, outer, outer_id, ancestor) {
            let rest = inner.strip_prefix(ancestor).ok()?;
            return rel_string(rest, inner).ok();
        }
        at = ancestor.parent();
    }
    None
}

/// Whether `other` is `outer` reached another way: one file id on one
/// device or volume (a share mounted twice).
fn same_folder(probe: &dyn Probe, outer: &Path, outer_id: Option<FileId>, other: &Path) -> bool {
    let Some(a) = outer_id else { return false };
    let Some(b) = probe.stat(other).ok().flatten().and_then(|s| s.id) else { return false };
    a.file == b.file
        && (a.device == b.device
            || matches!(
                (probe.volume(outer), probe.volume(other)),
                (Ok(x), Ok(y)) if x.volume.same(&y.volume) == Same::Yes
            ))
}
