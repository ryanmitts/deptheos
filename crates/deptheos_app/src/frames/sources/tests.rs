//! Table tests of the roots against a fake file system.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};

use super::*;

/// An in-memory file system: folders, files, links and mounted volumes.
#[derive(Default)]
struct Fake {
    entries: BTreeMap<PathBuf, Entry>,
    links: Vec<(PathBuf, PathBuf)>,
    mounts: Vec<Mounted>,
    /// Mounts [`Probe::volumes`] doesn't list.
    unlisted: Vec<PathBuf>,
    /// Folders on another volume than their mount, like macOS's boot data
    /// under `/`.
    firmlinks: Vec<(PathBuf, Mounted)>,
    next_id: u128,
}

#[derive(Clone)]
struct Entry {
    bytes: Option<Vec<u8>>,
    id: FileId,
}

fn p(path: &str) -> PathBuf {
    PathBuf::from(path)
}

fn disk(name: &str, id: Option<&str>) -> Volume {
    Volume { id: id.map(str::to_string), name: name.into(), kind: VolumeKind::Disk }
}

fn share(server: &str, share: &str) -> Volume {
    let kind = VolumeKind::Share { server: server.into(), share: share.into() };
    Volume { id: None, name: share.into(), kind }
}

impl Fake {
    fn new() -> Self {
        let mut fake = Self::default();
        fake.mount("/", disk("Macintosh HD", Some("uuid:BOOT")));
        fake
    }

    /// Mount `volume` at `at`, an empty folder.
    fn mount(&mut self, at: &str, volume: Volume) {
        self.mounts.push(Mounted { volume, mount: p(at) });
        self.folder(at);
    }

    /// Unmount `at` with its contents, returned for remounting.
    fn unmount(&mut self, at: &str) -> (Volume, Vec<(PathBuf, Entry)>) {
        let i = self.mounts.iter().position(|m| m.mount == p(at)).unwrap();
        let volume = self.mounts.remove(i).volume;
        let gone: Vec<PathBuf> =
            self.entries.keys().filter(|k| k.starts_with(at)).cloned().collect();
        let entries = gone
            .into_iter()
            .map(|k| {
                let e = self.entries.remove(&k).unwrap();
                (k.strip_prefix(at).unwrap().to_path_buf(), e)
            })
            .collect();
        (volume, entries)
    }

    /// Mount at `at` what [`Self::unmount`] took away.
    fn remount(&mut self, at: &str, (volume, entries): (Volume, Vec<(PathBuf, Entry)>)) {
        self.mounts.push(Mounted { volume, mount: p(at) });
        for (rel, e) in entries {
            self.entries.insert(p(at).join(rel), e);
        }
    }

    fn id(&mut self) -> FileId {
        self.next_id += 1;
        FileId { device: 1, file: self.next_id }
    }

    fn folder(&mut self, path: &str) {
        let mut at = PathBuf::new();
        for c in Path::new(path).components() {
            at.push(c);
            if !self.entries.contains_key(&at) {
                let id = self.id();
                self.entries.insert(at.clone(), Entry { bytes: None, id });
            }
        }
    }

    fn file(&mut self, path: &str, bytes: &[u8]) {
        self.folder(p(path).parent().unwrap().to_str().unwrap());
        let id = self.id();
        self.entries.insert(p(path), Entry { bytes: Some(bytes.to_vec()), id });
    }

    /// Move the folder or file `from` to `to`, with what is inside.
    fn mv(&mut self, from: &str, to: &str) {
        self.folder(p(to).parent().unwrap().to_str().unwrap());
        let moving: Vec<PathBuf> =
            self.entries.keys().filter(|k| k.starts_with(from)).cloned().collect();
        for k in moving {
            let e = self.entries.remove(&k).unwrap();
            self.entries.insert(p(to).join(k.strip_prefix(from).unwrap()), e);
        }
    }

    fn link(&mut self, from: &str, to: &str) {
        self.links.push((p(from), p(to)));
    }

    fn resolved(&self, path: &Path) -> PathBuf {
        for (from, to) in &self.links {
            if let Ok(rest) = path.strip_prefix(from) {
                return to.join(rest);
            }
        }
        path.to_path_buf()
    }
}

impl Probe for Fake {
    fn stat(&self, path: &Path) -> io::Result<Option<Stat>> {
        Ok(self.entries.get(&self.resolved(path)).map(|e| Stat {
            folder: e.bytes.is_none(),
            size: e.bytes.as_ref().map_or(0, |b| b.len() as u64),
            id: Some(e.id),
        }))
    }

    fn canonical(&self, path: &Path) -> io::Result<PathBuf> {
        let path = self.resolved(path);
        match self.entries.contains_key(&path) {
            true => Ok(path),
            false => Err(io::ErrorKind::NotFound.into()),
        }
    }

    fn volume(&self, path: &Path) -> io::Result<Mounted> {
        let path = self.resolved(path);
        if !self.entries.contains_key(&path) {
            return Err(io::ErrorKind::NotFound.into());
        }
        if let Some((_, m)) = self.firmlinks.iter().find(|(at, _)| path.starts_with(at)) {
            return Ok(m.clone());
        }
        Ok(self
            .mounts
            .iter()
            .filter(|m| path.starts_with(&m.mount))
            .max_by_key(|m| m.mount.components().count())
            .unwrap()
            .clone())
    }

    fn volumes(&self) -> Vec<Mounted> {
        self.mounts.iter().filter(|m| !self.unlisted.contains(&m.mount)).cloned().collect()
    }

    fn read(&self, path: &Path, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        let bytes = self.entries[&self.resolved(path)].bytes.clone().unwrap();
        let start = (offset as usize).min(bytes.len());
        Ok(bytes[start..(start + len).min(bytes.len())].to_vec())
    }

    fn list(&self, dir: &Path) -> io::Result<Vec<String>> {
        let dir = self.resolved(dir);
        Ok(self
            .entries
            .keys()
            .filter(|k| k.parent() == Some(dir.as_path()))
            .map(|k| k.file_name().unwrap().to_str().unwrap().to_string())
            .collect())
    }
}

/// A Library at /Users/me/Library on a fake disk, its `sources.json` in a
/// real temporary folder.
struct Lib {
    fake: Fake,
    dir: tempfile::TempDir,
    sources: Sources,
}

impl Lib {
    fn new() -> Self {
        let mut fake = Fake::new();
        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().to_str().unwrap().to_string();
        fake.folder(&lib);
        fake.folder("/App");
        let places = Places { library: dir.path().to_path_buf(), app: Some(p("/App")) };
        let sources = Sources::open(&fake, places).unwrap();
        Self { fake, dir, sources }
    }

    fn library(&self) -> String {
        self.dir.path().to_str().unwrap().to_string()
    }

    fn add(&mut self, folder: &str, files: &[&str]) -> Vec<FrameRef> {
        let files: Vec<PathBuf> = files.iter().map(|f| p(folder).join(f)).collect();
        self.sources.add_folder(&self.fake, &p(folder), &files, None).unwrap().0
    }

    fn resolve(&self, frame: &FrameRef) -> Resolved {
        self.sources.resolve(&self.fake, frame, Check::Contents)
    }

    fn reopen(&mut self) {
        let places = self.sources.places.clone();
        self.sources = Sources::open(&self.fake, places).unwrap();
    }

    fn on_disk(&self) -> serde_json::Value {
        let bytes = std::fs::read(self.dir.path().join(SOURCES_FILE)).unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }
}

fn there(path: &str) -> Resolved {
    Resolved::There(p(path))
}

#[test]
fn each_kind_of_root_resolves() {
    let mut lib = Lib::new();
    lib.fake.mount("/Volumes/Drive", disk("Drive", Some("uuid:D")));
    lib.fake.file("/Volumes/Drive/bee/1.tif", b"one");
    let library = lib.library();
    lib.fake.file(&format!("{library}/Kept/2.tif"), b"two");
    lib.fake.file("/App/Imports/3.tif", b"three");

    let on_volume = lib.add("/Volumes/Drive/bee", &["1.tif"]);
    let in_library = lib.add(&format!("{library}/Kept"), &["2.tif"]);
    let in_app = lib.add("/App/Imports", &["3.tif"]);

    let place = |frame: &FrameRef| lib.sources.root(&frame.root).unwrap().place.clone();
    assert!(matches!(place(&on_volume[0]), Place::Volume { ref path, ref mount, .. }
        if path == "bee" && *mount == p("/Volumes/Drive")));
    assert_eq!(
        place(&in_library[0]),
        Place::Anchored { anchor: Anchor::Library, path: "Kept".into() }
    );
    assert_eq!(place(&in_app[0]), Place::Anchored { anchor: Anchor::App, path: "Imports".into() });
    assert_eq!(lib.resolve(&on_volume[0]), there("/Volumes/Drive/bee/1.tif"));
    assert_eq!(lib.resolve(&in_library[0]), Resolved::There(p(&library).join("Kept/2.tif")));
    assert_eq!(lib.resolve(&in_app[0]), there("/App/Imports/3.tif"));
    assert_eq!(on_volume[0].path, "1.tif");
    assert_eq!(on_volume[0].size, 3);
}

#[test]
fn a_folder_inside_a_root_is_named_from_it() {
    let mut lib = Lib::new();
    lib.fake.file("/Photos/trip/day1/a.tif", b"a");
    lib.fake.file("/Photos/trip/b.tif", b"b");
    let top = lib.add("/Photos/trip", &["b.tif"]);
    let inner = lib.add("/Photos/trip/day1", &["a.tif"]);
    assert_eq!(inner[0].root, top[0].root, "one root covers both");
    assert_eq!(inner[0].path, "day1/a.tif");
    assert_eq!(lib.sources.roots().count(), 1);
    assert_eq!(lib.resolve(&inner[0]), there("/Photos/trip/day1/a.tif"));
}

#[test]
fn a_parent_added_later_takes_its_child_in() {
    let mut lib = Lib::new();
    lib.fake.file("/Photos/trip/day1/a.tif", b"a");
    lib.fake.file("/Photos/trip/b.tif", b"b");
    let child = lib.add("/Photos/trip/day1", &["a.tif"]);
    let parent = lib.add("/Photos/trip", &["b.tif", "day1/a.tif"]);

    let child_root = lib.sources.root(&child[0].root).unwrap();
    assert_eq!(
        child_root.place,
        Place::Within { root: parent[0].root.clone(), path: "day1".into() }
    );
    // The parent's frame in the child's folder is named from the child.
    assert_eq!(parent[1], child[0]);
    assert_eq!(lib.resolve(&child[0]), there("/Photos/trip/day1/a.tif"));

    // Moving the parent moves the child with it: one entry rewritten.
    lib.fake.mv("/Photos/trip", "/Archive/trip");
    assert_eq!(lib.resolve(&child[0]), Resolved::Missing);
    let relinked =
        lib.sources.relink(&lib.fake, &parent[0].root, &p("/Archive/trip"), None).unwrap();
    assert_eq!(relinked.moved, [parent[0].root.clone()]);
    assert!(relinked.renamed.is_empty());
    assert_eq!(lib.resolve(&child[0]), there("/Archive/trip/day1/a.tif"));
    assert_eq!(lib.resolve(&parent[0]), there("/Archive/trip/b.tif"));
}

#[test]
fn a_nested_root_moved_to_another_volume_is_relinked_alone() {
    let mut lib = Lib::new();
    lib.fake.mount("/Volumes/Other", disk("Other", Some("uuid:O")));
    lib.fake.file("/Photos/trip/day1/a.tif", b"a");
    lib.fake.file("/Photos/trip/b.tif", b"b");
    let child = lib.add("/Photos/trip/day1", &["a.tif"]);
    let parent = lib.add("/Photos/trip", &["b.tif"]);
    lib.fake.mv("/Photos/trip/day1", "/Volumes/Other/day1");

    let relinked = lib
        .sources
        .relink(&lib.fake, &child[0].root, &p("/Volumes/Other/day1"), Some(Bookmark(vec![7])))
        .unwrap();
    assert!(relinked.renamed.is_empty(), "the child's frames keep their names");
    assert_eq!(lib.resolve(&child[0]), there("/Volumes/Other/day1/a.tif"));
    assert_eq!(lib.resolve(&parent[0]), there("/Photos/trip/b.tif"));
    assert_eq!(lib.sources.bookmark(&child[0].root), Some(&Bookmark(vec![7])));
}

#[test]
fn a_subfolder_moved_away_is_split_off() {
    let mut lib = Lib::new();
    lib.fake.mount("/Volumes/Other", disk("Other", Some("uuid:O")));
    lib.fake.file("/Photos/trip/day1/a.tif", b"a");
    lib.fake.file("/Photos/trip/day1/deep/c.tif", b"c");
    lib.fake.file("/Photos/trip/b.tif", b"b");
    let frames = lib.add("/Photos/trip", &["b.tif", "day1/a.tif", "day1/deep/c.tif"]);
    lib.fake.mv("/Photos/trip/day1", "/Volumes/Other/moved");
    assert_eq!(lib.resolve(&frames[1]), Resolved::Missing);

    let top = frames[0].root.clone();
    let relinked =
        lib.sources.split(&lib.fake, &top, "day1", &p("/Volumes/Other/moved"), None).unwrap();
    let new = relinked.moved[0].clone();
    assert_eq!(
        relinked.renamed,
        [Renamed { from: top.clone(), path: "day1".into(), to: new.clone() }]
    );
    // The stored names still resolve: only sources.json changed.
    assert_eq!(lib.resolve(&frames[1]), there("/Volumes/Other/moved/a.tif"));
    assert_eq!(lib.resolve(&frames[2]), there("/Volumes/Other/moved/deep/c.tif"));
    assert_eq!(lib.resolve(&frames[0]), there("/Photos/trip/b.tif"));
    assert_eq!(lib.sources.identity(&frames[1]).root, new);
    assert_eq!(relinked.renamed[0].apply(&frames[2]).unwrap().path, "deep/c.tif");
    assert_eq!(relinked.renamed[0].apply(&frames[0]), None);
    // A second split of it relinks the root it became.
    lib.fake.mv("/Volumes/Other/moved", "/Volumes/Other/again");
    let again =
        lib.sources.split(&lib.fake, &top, "day1", &p("/Volumes/Other/again"), None).unwrap();
    assert_eq!(again.moved, [new]);
    assert_eq!(lib.resolve(&frames[1]), there("/Volumes/Other/again/a.tif"));
    assert!(matches!(
        lib.sources.split(&lib.fake, &top, "../x", &p("/Volumes/Other"), None),
        Err(SourcesError::NotSubfolder(_))
    ));
}

#[test]
fn a_root_found_inside_another_is_nested_again() {
    let mut lib = Lib::new();
    lib.fake.file("/A/a.tif", b"a");
    lib.fake.file("/B/b.tif", b"b");
    let a = lib.add("/A", &["a.tif"]);
    let b = lib.add("/B", &["b.tif"]);
    lib.fake.mv("/B", "/A/b");
    let resolution = Resolution { root: b[0].root.clone(), path: p("/A/b"), fresh: None };
    let found = lib.sources.find_again(&lib.fake, &[resolution], &HashMap::new()).unwrap();
    assert_eq!(found.moved, [b[0].root.clone()]);
    assert_eq!(
        found.renamed,
        [Renamed { from: a[0].root.clone(), path: "b".into(), to: b[0].root.clone() }]
    );
    let b_root = lib.sources.root(&b[0].root).unwrap();
    assert_eq!(b_root.place, Place::Within { root: a[0].root.clone(), path: "b".into() });
    assert_eq!(lib.resolve(&b[0]), there("/A/b/b.tif"));
}

/// A root located between a top root and a nested one takes the nested
/// one; a frame there is one frame, named from either.
#[test]
fn a_root_located_between_two_holds_the_inner_one() {
    let mut lib = Lib::new();
    lib.fake.file("/A/x/y/c.tif", b"c");
    lib.fake.file("/A/a.tif", b"a");
    lib.fake.file("/B/y/c.tif", b"c");
    let inner = lib.add("/A/x/y", &["c.tif"]);
    let top = lib.add("/A", &["a.tif"]);
    let located = lib.add("/B", &["y/c.tif"]);
    let (inner, top, located) = (&inner[0].root, &top[0].root, &located[0]);
    assert_eq!(
        lib.sources.root(inner).unwrap().place,
        Place::Within { root: top.clone(), path: "x/y".into() }
    );
    lib.fake.mv("/B", "/Elsewhere");
    lib.sources.relink(&lib.fake, &located.root, &p("/A/x"), None).unwrap();
    assert_eq!(
        lib.sources.root(&located.root).unwrap().place,
        Place::Within { root: top.clone(), path: "x".into() }
    );
    assert_eq!(
        lib.sources.root(inner).unwrap().place,
        Place::Within { root: located.root.clone(), path: "y".into() }
    );
    assert_eq!(lib.sources.identity(located).root, *inner, "one identity");
    assert_eq!(lib.resolve(located), there("/A/x/y/c.tif"));
}

#[test]
fn one_folder_reached_two_ways_is_one_root() {
    let mut lib = Lib::new();
    lib.fake.file("/Real/photos/a.tif", b"a");
    lib.fake.link("/Link", "/Real");
    let one = lib.add("/Real/photos", &["a.tif"]);
    let two = lib.add("/Link/photos", &["a.tif"]);
    assert_eq!(one, two);
    assert_eq!(lib.sources.roots().count(), 1);

    // A share mounted twice: two devices, one file id, one share.
    lib.fake.mount("/Volumes/photos", share("nas", "photos"));
    lib.fake.file("/Volumes/photos/bee/1.tif", b"1");
    let first = lib.add("/Volumes/photos/bee", &["1.tif"]);
    lib.fake.mount("/Volumes/photos-1", share("NAS._smb._tcp.local", "photos"));
    let mut id = lib.fake.entries[&p("/Volumes/photos/bee")].id;
    id.device = 2;
    lib.fake.entries.insert(p("/Volumes/photos-1/bee"), Entry { bytes: None, id });
    lib.fake.file("/Volumes/photos-1/bee/1.tif", b"1");
    let second = lib.add("/Volumes/photos-1/bee", &["1.tif"]);
    assert_eq!(first[0].root, second[0].root);
    assert_eq!(lib.sources.roots().count(), 2);
}

#[test]
fn anchored_roots_follow_the_library_and_the_app() {
    let mut lib = Lib::new();
    let library = lib.library();
    lib.fake.file(&format!("{library}/Captures/1.tif"), b"1");
    lib.fake.file("/App/Imports/2.tif", b"2");
    let captured = lib.add(&format!("{library}/Captures"), &["1.tif"]);
    let imported = lib.add("/App/Imports", &["2.tif"]);

    // The app's container and the Library both move.
    lib.fake.mv("/App", "/NewApp");
    lib.fake.mv(&library, "/Moved/Library");
    let new_dir = tempfile::tempdir().unwrap();
    std::fs::copy(lib.dir.path().join(SOURCES_FILE), new_dir.path().join(SOURCES_FILE)).unwrap();
    lib.fake.mv("/Moved/Library", new_dir.path().to_str().unwrap());
    let places = Places { library: new_dir.path().to_path_buf(), app: Some(p("/NewApp")) };
    let sources = Sources::open(&lib.fake, places).unwrap();
    let resolve = |f: &FrameRef| sources.resolve(&lib.fake, f, Check::Size);
    assert_eq!(resolve(&captured[0]), Resolved::There(new_dir.path().join("Captures/1.tif")));
    assert_eq!(resolve(&imported[0]), there("/NewApp/Imports/2.tif"));
}

#[test]
fn a_card_is_taken_only_when_it_is_proved() {
    let mut lib = Lib::new();
    let card = "/Volumes/EOS_DIGITAL";
    lib.fake.mount(card, disk("EOS_DIGITAL", Some("uuid:CARD-A")));
    lib.fake.file("/Volumes/EOS_DIGITAL/DCIM/100CANON/IMG_0001.CR3", b"card a, frame 1");
    lib.fake.file("/Volumes/EOS_DIGITAL/DCIM/100CANON/IMG_0002.CR3", b"card a, frame 2");
    let frames = lib.add("/Volumes/EOS_DIGITAL/DCIM/100CANON", &["IMG_0001.CR3", "IMG_0002.CR3"]);
    let root = frames[0].root.clone();
    let samples = HashMap::from([(root.clone(), frames.clone())]);
    let card_a = lib.fake.unmount(card);
    assert_eq!(lib.resolve(&frames[0]), Resolved::NotConnected);

    // Another card, same name and layout, but a different known id: never
    // this root's.
    lib.fake.mount(card, disk("EOS_DIGITAL", Some("uuid:CARD-B")));
    lib.fake.file("/Volumes/EOS_DIGITAL/DCIM/100CANON/IMG_0001.CR3", b"card b, frame 1");
    lib.fake.file("/Volumes/EOS_DIGITAL/DCIM/100CANON/IMG_0002.CR3", b"card b, frame 2");
    assert_eq!(lib.resolve(&frames[0]), Resolved::NotConnected);
    let found = lib.sources.find_again(&lib.fake, &[], &samples).unwrap();
    assert_eq!(found, Relinked::default());

    // Without ids it is a candidate whose frames don't match: not taken.
    lib.fake.unmount(card);
    lib.fake.mount("/Volumes/EOS_DIGITAL 1", disk("EOS_DIGITAL", None));
    lib.fake.file("/Volumes/EOS_DIGITAL 1/DCIM/100CANON/IMG_0001.CR3", b"card b, frame 1");
    lib.fake.file("/Volumes/EOS_DIGITAL 1/DCIM/100CANON/IMG_0002.CR3", b"card b, frame 2");
    assert_eq!(lib.resolve(&frames[0]), Resolved::NotConnected);
    let found = lib.sources.find_again(&lib.fake, &[], &samples).unwrap();
    assert_eq!(found, Relinked::default());
    assert_eq!(lib.resolve(&frames[0]), Resolved::NotConnected);

    // The card itself, id lost, at a new mount: its frames match, taken.
    lib.fake.unmount("/Volumes/EOS_DIGITAL 1");
    lib.fake.remount("/Volumes/EOS_DIGITAL 2", (disk("EOS_DIGITAL", None), card_a.1));
    let found = lib.sources.find_again(&lib.fake, &[], &samples).unwrap();
    assert_eq!(found.moved, std::slice::from_ref(&root));
    assert_eq!(lib.resolve(&frames[1]), there("/Volumes/EOS_DIGITAL 2/DCIM/100CANON/IMG_0002.CR3"));
}

/// A partial match is never taken or offered.
#[test]
fn a_folder_that_only_partly_matches_is_not_taken() {
    let mut lib = Lib::new();
    lib.fake.mount("/Volumes/CARD", disk("CARD", None));
    lib.fake.file("/Volumes/CARD/DCIM/1.jpg", b"1");
    lib.fake.file("/Volumes/CARD/DCIM/2.jpg", b"2");
    let frames = lib.add("/Volumes/CARD/DCIM", &["1.jpg", "2.jpg"]);
    let samples = HashMap::from([(frames[0].root.clone(), frames.clone())]);
    let card = lib.fake.unmount("/Volumes/CARD");
    lib.fake.remount("/Volumes/CARD 1", card);
    lib.fake.file("/Volumes/CARD 1/DCIM/2.jpg", b"another 2");
    let found = lib.sources.find_again(&lib.fake, &[], &samples).unwrap();
    assert_eq!(found, Relinked::default());
    assert_eq!(lib.resolve(&frames[0]), Resolved::NotConnected);
    // With no sample at all, nothing is proved either.
    let found = lib.sources.find_again(&lib.fake, &[], &HashMap::new()).unwrap();
    assert_eq!(found, Relinked::default());
}

#[test]
fn a_share_found_at_a_new_mount_brings_its_other_roots() {
    let mut lib = Lib::new();
    lib.fake.mount("/Volumes/photos", share("nas.local", "photos"));
    lib.fake.file("/Volumes/photos/bee/1.tif", b"1");
    lib.fake.file("/Volumes/photos/ant/2.tif", b"2");
    let bee = lib.add("/Volumes/photos/bee", &["1.tif"]);
    let ant = lib.add("/Volumes/photos/ant", &["2.tif"]);
    let mounted = lib.fake.unmount("/Volumes/photos");
    lib.fake.remount("/Volumes/photos-1", mounted);
    // iOS lists no shares: this one is found through one bookmark.
    lib.fake.unlisted.push(p("/Volumes/photos-1"));

    let resolution = Resolution {
        root: bee[0].root.clone(),
        path: p("/Volumes/photos-1/bee"),
        fresh: Some(Bookmark(vec![1])),
    };
    let found = lib.sources.find_again(&lib.fake, &[resolution], &HashMap::new()).unwrap();
    assert_eq!(found.moved, [bee[0].root.clone(), ant[0].root.clone()]);
    assert_eq!(lib.resolve(&ant[0]), there("/Volumes/photos-1/ant/2.tif"));
    assert_eq!(lib.sources.bookmark(&bee[0].root), Some(&Bookmark(vec![1])));

    // Picking the share's top folder brings them back too.
    let mounted = lib.fake.unmount("/Volumes/photos-1");
    lib.fake.remount("/Volumes/photos-2", mounted);
    let found =
        lib.sources.found_volume(&lib.fake, &p("/Volumes/photos-2"), &HashMap::new()).unwrap();
    assert_eq!(found.moved.len(), 2);
    assert_eq!(lib.resolve(&bee[0]), there("/Volumes/photos-2/bee/1.tif"));
}

#[test]
fn changed_missing_and_not_connected_are_told_apart() {
    let mut lib = Lib::new();
    lib.fake.mount("/Volumes/Drive", disk("Drive", Some("uuid:D")));
    lib.fake.file("/Volumes/Drive/a/1.tif", b"one");
    lib.fake.file("/Volumes/Drive/a/2.tif", b"two");
    lib.fake.file("/Volumes/Drive/a/3.tif", b"three");
    let frames = lib.add("/Volumes/Drive/a", &["1.tif", "2.tif", "3.tif"]);

    lib.fake.file("/Volumes/Drive/a/1.tif", b"ONE"); // tags rewritten in place
    lib.fake.file("/Volumes/Drive/a/2.tif", b"two, longer");
    lib.fake.entries.remove(&p("/Volumes/Drive/a/3.tif"));
    assert_eq!(
        lib.sources.resolve(&lib.fake, &frames[0], Check::Size),
        there("/Volumes/Drive/a/1.tif")
    );
    assert_eq!(lib.resolve(&frames[0]), Resolved::Changed(p("/Volumes/Drive/a/1.tif")));
    assert_eq!(lib.resolve(&frames[1]), Resolved::Changed(p("/Volumes/Drive/a/2.tif")));
    assert_eq!(lib.resolve(&frames[2]), Resolved::Missing);

    lib.fake.mv("/Volumes/Drive/a", "/Volumes/Drive/b");
    assert_eq!(lib.sources.root_state(&lib.fake, &frames[0].root), RootState::Missing);
    lib.fake.unmount("/Volumes/Drive");
    assert_eq!(lib.resolve(&frames[0]), Resolved::NotConnected);
    assert_eq!(lib.sources.root_state(&lib.fake, &frames[0].root), RootState::NotConnected);
}

#[test]
fn a_folder_in_the_trash_is_missing() {
    let mut lib = Lib::new();
    lib.fake.file("/Users/me/Photos/a/1.tif", b"1");
    let frames = lib.add("/Users/me/Photos/a", &["1.tif"]);
    lib.fake.mv("/Users/me/Photos/a", "/Users/me/.Trash/a");
    // Its bookmark follows it into the trash: not taken.
    let resolution =
        Resolution { root: frames[0].root.clone(), path: p("/Users/me/.Trash/a"), fresh: None };
    let found = lib.sources.find_again(&lib.fake, &[resolution], &HashMap::new()).unwrap();
    assert!(found.moved.is_empty());
    assert_eq!(lib.resolve(&frames[0]), Resolved::Missing);
    // Relinked there by hand, it is still missing.
    lib.sources.relink(&lib.fake, &frames[0].root, &p("/Users/me/.Trash/a"), None).unwrap();
    assert_eq!(lib.resolve(&frames[0]), Resolved::Missing);
    for (path, trash) in [
        ("/Volumes/X/.Trashes/501/a", true),
        ("/home/me/.local/share/Trash/files/a", true),
        ("/media/x/.Trash-1000/files/a", true),
        (if cfg!(windows) { r"C:\$Recycle.Bin\S-1\a" } else { "/mnt/c/$Recycle.Bin/S-1/a" }, true),
        ("/Users/me/Trash talk/a", false),
    ] {
        assert_eq!(in_trash(Path::new(path)), trash, "{path}");
    }
}

#[test]
fn names_are_kept_exactly_and_matched_folded() {
    let nfc = "Caf\u{e9}";
    let nfd = "Cafe\u{301}";
    assert_ne!(nfc, nfd);
    assert_eq!(key(nfc), key(nfd));
    assert_eq!(key("IMG_0001.CR3"), key("img_0001.cr3"));

    let mut lib = Lib::new();
    // Names differing only by case on a case-sensitive volume: two frames.
    lib.fake.file(&format!("/Photos/{nfd}/a.tif"), b"lower");
    lib.fake.file(&format!("/Photos/{nfd}/A.tif"), b"upper");
    let frames = lib.add(&format!("/Photos/{nfd}"), &["a.tif", "A.tif"]);
    assert_ne!(frames[0].path, frames[1].path);
    let root = lib.sources.root(&frames[0].root).unwrap();
    assert!(matches!(&root.place, Place::Volume { path, .. } if path == &format!("Photos/{nfd}")));

    // Locate… onto a normalized, upper-cased copy: found by key.
    lib.fake.file(&format!("/Copy/{nfc}/A.TIF"), b"lower");
    let matched =
        lib.sources.matched(&lib.fake, &p(&format!("/Copy/{nfc}")), &frames[0].root, &frames[..1]);
    assert_eq!(matched, Matched { found: 1, of: 1 });
}

#[test]
fn prune_waits_for_the_whole_library() {
    let mut lib = Lib::new();
    lib.fake.file("/A/a.tif", b"a");
    lib.fake.file("/A/sub/s.tif", b"s");
    lib.fake.file("/B/b.tif", b"b");
    let sub = lib.add("/A/sub", &["s.tif"]);
    let a = lib.add("/A", &["a.tif"]);
    let b = lib.add("/B", &["b.tif"]);
    assert_eq!(lib.sources.prune(InUse::Unread).unwrap(), []);
    assert_eq!(lib.sources.roots().count(), 3);

    // Only the nested root is named: its parent stays with it.
    let named = HashSet::from([sub[0].root.clone()]);
    assert_eq!(lib.sources.prune(InUse::Read(&named)).unwrap(), [b[0].root.clone()]);
    assert!(lib.sources.root(&a[0].root).is_some());
    lib.reopen();
    assert_eq!(lib.sources.roots().count(), 2);
}

#[test]
fn sources_json_round_trips_and_a_damaged_one_is_refused() {
    let mut lib = Lib::new();
    lib.fake.mount("/Volumes/photos", share("nas", "photos"));
    lib.fake.file("/Volumes/photos/a/1.tif", b"1");
    let path = p("/Volumes/photos/a");
    let frames = lib
        .sources
        .add_folder(&lib.fake, &path, &[path.join("1.tif")], Some(Bookmark(vec![0xde, 0xad])))
        .unwrap()
        .0;
    let json = lib.on_disk();
    assert_eq!(json["schema"], SOURCES_SCHEMA);
    let entry = &json["roots"][frames[0].root.as_str()];
    assert_eq!(entry["place"]["at"], "volume");
    assert_eq!(entry["place"]["bookmark"], "dead");
    assert_eq!(entry["place"]["volume"]["kind"]["server"], "nas");

    // A field a newer build added is kept through a write.
    let mut edited = json.clone();
    edited["roots"][frames[0].root.as_str()]["note"] = "kept".into();
    std::fs::write(lib.dir.path().join(SOURCES_FILE), serde_json::to_vec(&edited).unwrap())
        .unwrap();
    lib.reopen();
    assert_eq!(lib.resolve(&frames[0]), there("/Volumes/photos/a/1.tif"));
    lib.sources.set_bookmark(&frames[0].root, Some(Bookmark(vec![1]))).unwrap();
    assert_eq!(lib.on_disk()["roots"][frames[0].root.as_str()]["note"], "kept");

    // A frame round-trips as JSON.
    let frame: FrameRef =
        serde_json::from_value(serde_json::to_value(&frames[0]).unwrap()).unwrap();
    assert_eq!(frame, frames[0]);

    // Damaged with no copy: refused, left byte for byte, nothing beside it.
    std::fs::remove_file(lib.dir.path().join(PREVIOUS_SOURCES_FILE)).unwrap();
    let names = || {
        let mut names: Vec<String> = std::fs::read_dir(lib.dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    };
    let before = names();
    let damaged = b"{ not json";
    std::fs::write(lib.dir.path().join(SOURCES_FILE), damaged).unwrap();
    let places = lib.sources.places.clone();
    let refused = Sources::open(&lib.fake, places);
    assert!(matches!(refused, Err(SourcesError::Damaged { .. })), "{refused:?}");
    assert_eq!(std::fs::read(lib.dir.path().join(SOURCES_FILE)).unwrap(), damaged);
    assert_eq!(names(), before);

    // A newer Library: refused whole, as one, and left as it is.
    let newer =
        format!(r#"{{"schema": {}, "written_by": "0.6.0", "roots": {{}}}}"#, SOURCES_SCHEMA + 1);
    std::fs::write(lib.dir.path().join(SOURCES_FILE), &newer).unwrap();
    let places = lib.sources.places.clone();
    let Err(refused) = Sources::open(&lib.fake, places) else { panic!("opened") };
    assert!(
        matches!(
            &refused,
            SourcesError::Store(StoreError::NewerLibrary { written_by: Some(v), .. }) if v == "0.6.0"
        ),
        "{refused:?}"
    );
    assert_eq!(std::fs::read_to_string(lib.dir.path().join(SOURCES_FILE)).unwrap(), newer);
}

#[test]
fn a_fingerprint_reads_the_ends() {
    let mut fake = Fake::new();
    let mut big = vec![0u8; 3 * FINGERPRINT_END];
    fake.file("/big", &big);
    let before = Fingerprint::of(&fake, &p("/big"), big.len() as u64).unwrap();
    big[FINGERPRINT_END + 5] = 1; // the middle is not read
    fake.file("/big", &big);
    assert_eq!(Fingerprint::of(&fake, &p("/big"), big.len() as u64).unwrap(), before);
    let last = big.len() - 1;
    big[last] = 1;
    fake.file("/big", &big);
    assert_ne!(Fingerprint::of(&fake, &p("/big"), big.len() as u64).unwrap(), before);

    fake.file("/small", b"hello");
    assert_eq!(
        Fingerprint::of(&fake, &p("/small"), 5).unwrap(),
        Fingerprint::of_ends(b"hello", b"")
    );
    let text: String = Fingerprint(0xabc).into();
    assert_eq!(text, "0000000000000abc");
    assert_eq!(Fingerprint::try_from(text).unwrap(), Fingerprint(0xabc));
}

#[test]
fn adding_refuses_what_it_cannot_keep() {
    let mut lib = Lib::new();
    lib.fake.file("/A/a.tif", b"a");
    let outside = lib.sources.add_folder(&lib.fake, &p("/A"), &[p("/B/b.tif")], None);
    assert!(matches!(outside, Err(SourcesError::NotInside { .. })));
    let gone = lib.sources.add_folder(&lib.fake, &p("/A"), &[p("/A/none.tif")], None);
    assert!(matches!(gone, Err(SourcesError::NotFile(_))));
    assert_eq!(lib.sources.roots().count(), 0, "nothing is added when a file is refused");
    let added = lib.sources.add_files(&lib.fake, &[p("/A/a.tif")]).unwrap().0;
    assert_eq!(added[0].path, "a.tif");
    let nested = lib.sources.set_bookmark(&RootId::new(), None);
    assert!(matches!(nested, Err(SourcesError::UnknownRoot(_))));
}

#[test]
fn a_root_reached_through_a_firmlink_is_on_its_own_volume() {
    let mut lib = Lib::new();
    let data = Mounted { volume: disk("Data", Some("uuid:DATA")), mount: p("/") };
    lib.fake.firmlinks.push((p("/Users"), data));
    lib.fake.file("/Users/me/Photos/1.tif", b"1");
    let frames = lib.add("/Users/me/Photos", &["1.tif"]);
    let root = lib.sources.root(&frames[0].root).unwrap();
    assert!(matches!(&root.place, Place::Volume { volume, mount, .. }
        if volume.name == "Data" && *mount == p("/")));
    assert_eq!(lib.resolve(&frames[0]), there("/Users/me/Photos/1.tif"));
    lib.fake.mv("/Users/me/Photos", "/Users/me/Elsewhere");
    assert_eq!(lib.resolve(&frames[0]), Resolved::Missing);
}

/// The real file system, end to end: added, resolved, changed, moved and
/// relinked, gone.
#[test]
fn the_disk_resolves_a_folder_added() {
    let dir = tempfile::tempdir().unwrap();
    let (library, photos) = (dir.path().join("Library"), dir.path().join("Photos"));
    std::fs::create_dir_all(&library).unwrap();
    std::fs::create_dir_all(&photos).unwrap();
    std::fs::write(photos.join("1.tif"), b"one").unwrap();
    let mut sources = Sources::open(&Disk, Places { library, app: None }).unwrap();
    let frames = sources.add_folder(&Disk, &photos, &[photos.join("1.tif")], None).unwrap().0;
    let canon = Disk.canonical(&photos).unwrap();
    assert_eq!(
        sources.resolve(&Disk, &frames[0], Check::Contents),
        Resolved::There(canon.join("1.tif"))
    );
    std::fs::write(photos.join("1.tif"), b"ONE").unwrap();
    assert_eq!(
        sources.resolve(&Disk, &frames[0], Check::Contents),
        Resolved::Changed(canon.join("1.tif"))
    );
    let moved = dir.path().join("Moved");
    std::fs::rename(&photos, &moved).unwrap();
    assert_eq!(sources.resolve(&Disk, &frames[0], Check::Size), Resolved::Missing);
    sources.relink(&Disk, &frames[0].root, &moved, None).unwrap();
    let canon = Disk.canonical(&moved).unwrap();
    assert_eq!(
        sources.resolve(&Disk, &frames[0], Check::Size),
        Resolved::There(canon.join("1.tif"))
    );
}

/// A frame's path needs no disk: a lost root's last place, a split
/// subfolder's new place, an anchor as the front end named it.
#[test]
fn a_path_is_worked_out_from_the_place_alone() {
    let mut lib = Lib::new();
    lib.fake.mount("/Volumes/Card", disk("Card", Some("uuid:C")));
    lib.fake.file("/Volumes/Card/DCIM/1.tif", b"one");
    lib.fake.file("/Volumes/Card/DCIM/day2/2.tif", b"two");
    lib.fake.file("/real/App/Imports/3.tif", b"three");
    lib.fake.link("/App", "/real/App");
    let card = lib.add("/Volumes/Card/DCIM", &["1.tif", "day2/2.tif"]);
    let mut sources = Sources::open(
        &lib.fake,
        Places { library: lib.dir.path().to_path_buf(), app: Some(p("/App")) },
    )
    .unwrap();
    let imported = sources.add_folder(
        &lib.fake,
        &p("/real/App/Imports"),
        &[p("/real/App/Imports/3.tif")],
        None,
    );
    let imported = imported.unwrap().0;
    assert_eq!(sources.path_of(&imported[0]), Some(p("/App/Imports/3.tif")), "as it was named");
    assert_eq!(sources.path_of(&card[0]), Some(p("/Volumes/Card/DCIM/1.tif")));

    let _gone = lib.fake.unmount("/Volumes/Card");
    assert_eq!(sources.resolve(&lib.fake, &card[0], Check::Size), Resolved::NotConnected);
    assert_eq!(sources.path_of(&card[0]), Some(p("/Volumes/Card/DCIM/1.tif")), "where it was");

    lib.fake.file("/Photos/day2/2.tif", b"two");
    sources.split(&lib.fake, &card[0].root, "day2", &p("/Photos/day2"), None).unwrap();
    assert_eq!(sources.path_of(&card[1]), Some(p("/Photos/day2/2.tif")));
    assert_eq!(sources.root_path(&card[0].root), Some(p("/Volumes/Card/DCIM")));

    let unknown = FrameRef { root: RootId::new(), ..card[0].clone() };
    assert_eq!(sources.path_of(&unknown), None);
}
