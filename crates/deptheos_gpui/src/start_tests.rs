use std::cell::RefCell;
use std::rc::Rc;

use deptheos_app::library::history::History;
use gpui::{Entity, TestAppContext};

use super::*;
use crate::test_support::{open as open_window, wait_until};

/// A start window over `root`, which failed with `error`, Settings naming
/// it; what it launches lands in the returned cell.
fn start_over(
    cx: &mut TestAppContext,
    root: &Path,
    error: StoreError,
) -> (gpui::AnyWindowHandle, Entity<LibraryStart>, Rc<RefCell<Option<Opened>>>) {
    let launched: Rc<RefCell<Option<Opened>>> = Rc::default();
    let into = launched.clone();
    let root = root.to_path_buf();
    let (window, start) = open_window(cx, |_, cx| {
        update_prefs(cx, |p| p.history_dir = Some(root.clone()));
        let refusal = Refusal::of(&root, &error, cx);
        let launch: Launch = Box::new(move |opened, _| *into.borrow_mut() = Some(opened));
        cx.new(|_| LibraryStart::new(refusal, launch))
    });
    (window, start, launched)
}

/// Call into the start window with its window.
fn act(
    cx: &mut TestAppContext,
    window: gpui::AnyWindowHandle,
    start: &Entity<LibraryStart>,
    f: impl FnOnce(&mut LibraryStart, &mut Window, &mut Context<LibraryStart>),
) {
    cx.update_window(window, |_, window, cx| start.update(cx, |s, cx| f(s, window, cx))).unwrap();
}

fn said(cx: &mut TestAppContext, start: &Entity<LibraryStart>) -> String {
    start.read_with(cx, |s, _| s.start.refusal().why.clone())
}

fn is_refused(cx: &mut TestAppContext, start: &Entity<LibraryStart>) -> bool {
    start.read_with(cx, |s, _| matches!(s.start, Start::Refused(_)))
}

/// A Library in another app's use says so; Try Again repeats it until it is
/// released, then opens it in the main window and the start window goes.
#[gpui::test]
fn try_again_opens_the_library_once_it_is_free(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Library");
    let held = History::open(&root).unwrap();
    let Err(in_use) = History::open(&root) else { panic!("opened twice") };
    let (window, start, launched) = start_over(cx, &root, in_use);
    assert!(
        said(cx, &start).contains("in another Deptheos on this computer"),
        "{}",
        said(cx, &start)
    );
    assert!(!start.read_with(cx, |s, _| s.start.refusal().newer));

    act(cx, window, &start, |s, window, cx| s.try_again(window, cx));
    assert!(!is_refused(cx, &start), "trying");
    wait_until(cx, "still in use", |cx| is_refused(cx, &start));
    assert!(launched.borrow().is_none());
    assert!(said(cx, &start).contains("in another Deptheos"), "{}", said(cx, &start));

    drop(held);
    act(cx, window, &start, |s, window, cx| s.try_again(window, cx));
    wait_until(cx, "the main window launched", |_| launched.borrow().is_some());
    let opened = launched.borrow_mut().take().unwrap();
    assert_eq!(opened.history.root(), root);
    cx.run_until_parked();
    assert!(cx.update_window(window, |_, _, _| ()).is_err(), "the start window is gone");
}

/// A Library a newer version made puts Get the Latest Version first, and
/// says which version last used it.
#[gpui::test]
fn a_newer_library_offers_the_latest_version(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Library");
    drop(History::open(&root).unwrap());
    let sources = root.join("sources.json");
    let mut json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&sources).unwrap()).unwrap();
    json["schema"] = 99.into();
    json["written_by"] = "0.9.0".into();
    std::fs::write(&sources, json.to_string()).unwrap();
    let Err(newer) = History::open(&root) else { panic!("opened") };
    let (window, start, _) = start_over(cx, &root, newer);
    assert!(start.read_with(cx, |s, _| s.start.refusal().newer));
    assert!(said(cx, &start).contains("Deptheos 0.9.0"), "{}", said(cx, &start));
    cx.update_window(window, |_, window, _| window.refresh()).unwrap();
    cx.run_until_parked();
    let drawn = gpui::VisualTestContext::from_window(window, cx).debug_bounds("library-start");
    assert!(drawn.is_some());
}

/// Choose Another Library… refuses a synced folder with why, keeping
/// Settings' folder, and opens any other, which Settings then names.
#[gpui::test]
fn choosing_another_library_opens_it(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Library");
    let held = History::open(&root).unwrap();
    let Err(in_use) = History::open(&root) else { panic!("opened twice") };
    let (window, start, launched) = start_over(cx, &root, in_use);
    let choosing = |cx: &mut TestAppContext| {
        start.update(cx, |s, cx| {
            let refusal = s.start.refusal().clone();
            s.set_start(Start::Choosing { refusal, _pick: Task::ready(()) }, cx)
        })
    };

    let synced = dir.path().join("Dropbox");
    std::fs::create_dir_all(&synced).unwrap();
    std::fs::write(synced.join(".dropbox"), b"{}").unwrap();
    let synced_library = synced.join("Library");
    choosing(cx);
    let picked = Some(synced_library.clone());
    act(cx, window, &start, |s, window, cx| s.chosen(picked, window, cx));
    assert!(is_refused(cx, &start));
    assert!(said(cx, &start).contains("Dropbox"), "{}", said(cx, &start));
    assert_eq!(cx.update(|cx| prefs(cx).history_dir.clone()), Some(root.clone()), "kept");
    assert!(!synced_library.exists(), "nothing made there");

    choosing(cx);
    act(cx, window, &start, |s, window, cx| s.chosen(None, window, cx));
    assert!(is_refused(cx, &start), "canceled");

    let other = dir.path().join("Other Library");
    choosing(cx);
    let picked = Some(other.clone());
    act(cx, window, &start, |s, window, cx| s.chosen(picked, window, cx));
    wait_until(cx, "the other Library launched", |_| launched.borrow().is_some());
    assert_eq!(launched.borrow().as_ref().unwrap().history.root(), other);
    assert_eq!(cx.update(|cx| prefs(cx).history_dir.clone()), Some(other));
    drop(held);
}

/// A `list.json` that can't be read is the Library not opening, and it is
/// left as it is.
#[cfg(unix)]
#[test]
fn an_unreadable_list_is_the_library_not_opening() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Library");
    let history = History::open(&root).unwrap();
    history.write_working(&Working::new(None)).unwrap();
    drop(history);
    let list = root.join("list.json");
    let bytes = std::fs::read(&list).unwrap();
    std::fs::set_permissions(&list, std::fs::Permissions::from_mode(0o000)).unwrap();
    let refused = open(&root);
    std::fs::set_permissions(&list, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(refused, Err(StoreError::Io { .. })), "{:?}", refused.err());
    assert_eq!(std::fs::read(&list).unwrap(), bytes);
    assert!(open(&root).is_ok(), "opens once it can be read");
}
