//! The batch sheet's folders, on both apps: folders and parents added,
//! folders removed, and whether deeper folders count. A pure reducer: the
//! apps list a parent's folders off the UI thread
//! (`folder::folders_inside`) and hand them in with [`Sources::listed`].

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// One folder the run stacks, and the parent it was found in, if it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub folder: PathBuf,
    pub parent: Option<PathBuf>,
}

/// The folders listed, in the order added, each once.
///
/// A removed folder stays out on relisting until added on its own. A parent
/// goes with its last folder; one that found none stays. Changing the depth
/// relists every parent; a listing at another depth, or of a parent gone,
/// is ignored.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sources {
    entries: Vec<Entry>,
    parents: Vec<PathBuf>,
    removed: HashSet<PathBuf>,
    deep: bool,
}

impl Sources {
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The parents added, in order: the ones to list.
    pub fn parents(&self) -> &[PathBuf] {
        &self.parents
    }

    /// Every folder below a parent counts, not only those directly in it.
    pub fn deep(&self) -> bool {
        self.deep
    }

    /// Add folders, each stacked on its own; one removed comes back.
    pub fn add_folders(&mut self, folders: impl IntoIterator<Item = PathBuf>) {
        for folder in folders {
            self.removed.remove(&folder);
            self.push(Entry { folder, parent: None });
        }
    }

    /// Add `parent`, whose folders are listed next ([`Self::listed`]).
    pub fn add_parent(&mut self, parent: PathBuf) {
        if !self.parents.contains(&parent) {
            self.parents.push(parent);
        }
    }

    /// `parent`'s folders at depth `deep`; those not removed join the list.
    pub fn listed(&mut self, parent: &Path, deep: bool, found: Vec<PathBuf>) {
        if deep != self.deep || !self.parents.iter().any(|p| p == parent) {
            return;
        }
        for folder in found {
            if !self.removed.contains(&folder) {
                self.push(Entry { folder, parent: Some(parent.to_path_buf()) });
            }
        }
    }

    /// Take `folder` out of the list.
    pub fn remove(&mut self, folder: &Path) {
        let Some(ix) = self.entries.iter().position(|e| e.folder == folder) else { return };
        let entry = self.entries.remove(ix);
        let Some(parent) = entry.parent else { return };
        self.removed.insert(entry.folder);
        if !self.entries.iter().any(|e| e.parent.as_ref() == Some(&parent)) {
            self.parents.retain(|p| *p != parent);
        }
    }

    /// Count deeper folders or not; `true` when changed, so the parents
    /// are to be listed again.
    pub fn set_deep(&mut self, deep: bool) -> bool {
        if self.deep == deep {
            return false;
        }
        self.deep = deep;
        self.entries.retain(|e| e.parent.is_none());
        true
    }

    fn push(&mut self, entry: Entry) {
        if !self.entries.iter().any(|e| e.folder == entry.folder) {
            self.entries.push(entry);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    fn folders(s: &Sources) -> Vec<&str> {
        s.entries().iter().map(|e| e.folder.to_str().unwrap()).collect()
    }

    #[test]
    fn folders_and_parents_list_in_the_order_added_each_once() {
        let mut s = Sources::default();
        s.add_folders([p("/a"), p("/b")]);
        s.add_parent(p("/shoot"));
        s.listed(Path::new("/shoot"), false, vec![p("/shoot/1"), p("/b"), p("/shoot/2")]);
        s.add_folders([p("/a")]);
        assert_eq!(folders(&s), ["/a", "/b", "/shoot/1", "/shoot/2"]);
        assert_eq!(s.entries()[2].parent.as_deref(), Some(Path::new("/shoot")));
        assert_eq!(s.entries()[1].parent, None, "added first on its own");
    }

    #[test]
    fn a_folder_removed_stays_out_until_added_on_its_own() {
        let mut s = Sources::default();
        s.add_parent(p("/shoot"));
        s.listed(Path::new("/shoot"), false, vec![p("/shoot/1"), p("/shoot/2")]);
        s.remove(Path::new("/shoot/1"));
        s.listed(Path::new("/shoot"), false, vec![p("/shoot/1"), p("/shoot/2")]);
        assert_eq!(folders(&s), ["/shoot/2"], "listed again, still out");
        s.add_folders([p("/shoot/1")]);
        assert_eq!(folders(&s), ["/shoot/2", "/shoot/1"]);
    }

    #[test]
    fn a_parent_goes_with_its_last_folder() {
        let mut s = Sources::default();
        s.add_parent(p("/shoot"));
        s.add_parent(p("/empty"));
        s.listed(Path::new("/shoot"), false, vec![p("/shoot/1")]);
        s.listed(Path::new("/empty"), false, vec![]);
        s.remove(Path::new("/shoot/1"));
        assert_eq!(s.parents(), [p("/empty")], "one that found none stays");
        s.listed(Path::new("/shoot"), false, vec![p("/shoot/9")]);
        assert!(folders(&s).is_empty(), "a listing of a parent gone is ignored");
    }

    #[test]
    fn a_new_depth_lists_the_parents_again() {
        let mut s = Sources::default();
        s.add_folders([p("/a")]);
        s.add_parent(p("/shoot"));
        s.listed(Path::new("/shoot"), false, vec![p("/shoot/1")]);
        assert!(s.set_deep(true));
        assert!(!s.set_deep(true), "unchanged");
        assert_eq!(folders(&s), ["/a"], "the parents' folders go");
        s.listed(Path::new("/shoot"), false, vec![p("/shoot/1")]);
        assert_eq!(folders(&s), ["/a"], "a listing at the old depth is ignored");
        s.listed(Path::new("/shoot"), true, vec![p("/shoot/1"), p("/shoot/1/x")]);
        assert_eq!(folders(&s), ["/a", "/shoot/1", "/shoot/1/x"]);
    }

    #[test]
    fn removing_a_folder_added_on_its_own_forgets_it() {
        let mut s = Sources::default();
        s.add_folders([p("/a")]);
        s.remove(Path::new("/a"));
        s.remove(Path::new("/nowhere"));
        assert!(s.entries().is_empty());
        s.add_parent(p("/"));
        s.listed(Path::new("/"), false, vec![p("/a")]);
        assert_eq!(folders(&s), ["/a"], "not kept out: it wasn't a parent's");
    }
}
