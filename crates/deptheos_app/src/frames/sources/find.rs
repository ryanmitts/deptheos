//! Finding roots again (ADR-145): by resolved bookmark (unless in a trash),
//! by volume id, or by volume name and last path as candidates accepted
//! only when a whole sample of frames matches. A partial match is never
//! offered. A root found at a new mount brings its volume's other roots.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::probe::{Probe, Same, in_trash};
use super::{
    Bookmark, FrameRef, Matched, Place, Relinked, Renamed, Result, Root, RootId, RootState,
    Sources, folder_state, join,
};

/// Where a root's bookmark resolved, and a remade one if it was stale.
#[derive(Clone, Debug)]
pub struct Resolution {
    pub root: RootId,
    pub path: PathBuf,
    pub fresh: Option<Bookmark>,
}

impl Sources {
    /// Look for every volume root not where it was: by `bookmarks`, volume
    /// id, then name-and-path candidates that all of its `samples` match.
    /// Writes `sources.json` once if anything moved.
    pub fn find_again(
        &mut self,
        probe: &dyn Probe,
        bookmarks: &[Resolution],
        samples: &HashMap<RootId, Vec<FrameRef>>,
    ) -> Result<Relinked> {
        let mounted = probe.volumes();
        let mut found = Relinked::default();
        for id in self.lost(probe) {
            if !self.is_lost(probe, &id) {
                continue; // brought back by another root of its volume
            }
            let resolved = bookmarks.iter().find(|r| r.root == id).filter(|r| {
                !in_trash(&r.path) && probe.stat(&r.path).ok().flatten().is_some_and(|s| s.folder)
            });
            if let Some(r) = resolved {
                let canon = self.folder(probe, &r.path)?;
                let bookmark = r.fresh.clone().or_else(|| self.bookmark(&id).cloned());
                self.move_root(probe, &id, &canon, bookmark, &mut found.moved)?;
                continue;
            }
            let Some(Root { place: Place::Volume { volume, path, mount, .. }, .. }) =
                self.roots.get(&id).cloned()
            else {
                continue;
            };
            let by_id = mounted
                .iter()
                .filter(|m| volume.same(&m.volume) == Same::Yes)
                .map(|m| join(&m.mount, &path))
                .find(|at| matches!(folder_state(probe, at.clone()), RootState::At(_)));
            if let Some(at) = by_id {
                self.keep_bookmark_and_move(probe, &id, &at, &mut found)?;
                continue;
            }
            let mut candidates: Vec<PathBuf> = mounted
                .iter()
                .filter(|m| volume.same(&m.volume) == Same::Maybe)
                .map(|m| join(&m.mount, &path))
                .collect();
            let last = join(&mount, &path);
            let ruled_out = probe.volume(&last).is_ok_and(|m| ruled_out(&volume, &m.volume));
            if !ruled_out {
                candidates.push(last);
            }
            self.try_candidates(probe, &id, candidates, samples, &mut found)?;
        }
        self.settle(probe, found)
    }

    /// The user picked `folder` on a volume (how iOS brings back an
    /// unmounted drive or share): its lost roots are looked for there,
    /// accepted when the volume is proved or the whole sample matches.
    pub fn found_volume(
        &mut self,
        probe: &dyn Probe,
        folder: &Path,
        samples: &HashMap<RootId, Vec<FrameRef>>,
    ) -> Result<Relinked> {
        let canon = self.folder(probe, folder)?;
        let picked = probe.volume(&canon).map_err(super::io_error(&canon))?;
        let mut found = Relinked::default();
        for id in self.lost(probe) {
            if !self.is_lost(probe, &id) {
                continue;
            }
            let Some(Place::Volume { volume, path, .. }) =
                self.roots.get(&id).map(|r| r.place.clone())
            else {
                continue;
            };
            let at = join(&picked.mount, &path);
            match volume.same(&picked.volume) {
                Same::Yes if matches!(folder_state(probe, at.clone()), RootState::At(_)) => {
                    self.keep_bookmark_and_move(probe, &id, &at, &mut found)?;
                }
                Same::Maybe => self.try_candidates(probe, &id, vec![at], samples, &mut found)?,
                _ => {}
            }
        }
        self.settle(probe, found)
    }

    /// The roots on a volume that aren't where they were, oldest first.
    fn lost(&self, probe: &dyn Probe) -> Vec<RootId> {
        self.roots
            .iter()
            .filter(|(_, root)| matches!(root.place, Place::Volume { .. }))
            .map(|(id, _)| id.clone())
            .filter(|id| self.is_lost(probe, id))
            .collect()
    }

    fn is_lost(&self, probe: &dyn Probe, id: &RootId) -> bool {
        !matches!(self.root_state(probe, id), RootState::At(_))
    }

    fn keep_bookmark_and_move(
        &mut self,
        probe: &dyn Probe,
        id: &RootId,
        at: &Path,
        found: &mut Relinked,
    ) -> Result<()> {
        let canon = self.folder(probe, at)?;
        let bookmark = self.bookmark(id).cloned();
        self.move_root(probe, id, &canon, bookmark, &mut found.moved)
    }

    /// Accept the first existing candidate the whole sample matches.
    fn try_candidates(
        &mut self,
        probe: &dyn Probe,
        id: &RootId,
        mut candidates: Vec<PathBuf>,
        samples: &HashMap<RootId, Vec<FrameRef>>,
        found: &mut Relinked,
    ) -> Result<()> {
        let mut seen = std::collections::HashSet::new();
        candidates.retain(|at| {
            seen.insert(at.clone()) && matches!(folder_state(probe, at.clone()), RootState::At(_))
        });
        let sample = samples.get(id).map(Vec::as_slice).unwrap_or_default();
        // Candidates by name come before the last path.
        for at in candidates {
            let matched: Matched = self.matched(probe, &at, id, sample);
            if matched.all() {
                tracing::debug!("sources: root {id} found by its frames at {}", at.display());
                return self.keep_bookmark_and_move(probe, id, &at, found);
            }
            tracing::debug!(
                "sources: root {id} not at {}: {} of {} of its frames there",
                at.display(),
                matched.found,
                matched.of
            );
        }
        Ok(())
    }

    /// Nest roots found inside others, and write once if anything moved.
    fn settle(&mut self, probe: &dyn Probe, mut found: Relinked) -> Result<Relinked> {
        let renamed: Vec<Renamed> = self.renest(probe);
        let changed = !found.moved.is_empty() || !renamed.is_empty();
        found.renamed.extend(renamed);
        if changed {
            self.save()?;
        }
        Ok(found)
    }
}

/// Whether `found` can't be `stored`: ids of one scheme differ, or kinds or
/// shares do. A different name alone may be a rename.
fn ruled_out(stored: &super::Volume, found: &super::Volume) -> bool {
    use super::VolumeKind;
    match (&stored.kind, &found.kind) {
        (VolumeKind::Disk, VolumeKind::Disk) => {
            let scheme = |id: &str| id.split_once(':').map(|(s, _)| s.to_string());
            matches!((&stored.id, &found.id), (Some(a), Some(b)) if scheme(a) == scheme(b) && !a.eq_ignore_ascii_case(b))
        }
        _ => stored.same(found) == Same::No,
    }
}
