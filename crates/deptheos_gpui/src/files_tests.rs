use std::cell::RefCell;
use std::rc::Rc;

use gpui::{AnyWindowHandle, TestAppContext};

use super::*;
use crate::test_support::{open, wait_until};

/// Empty files named `names` in a new folder.
fn folder(names: &[&str]) -> (tempfile::TempDir, Vec<PathBuf>) {
    let dir = tempfile::tempdir().unwrap();
    let paths = names
        .iter()
        .map(|n| {
            let path = dir.path().join(n);
            std::fs::write(&path, b"").unwrap();
            path
        })
        .collect();
    (dir, paths)
}

fn on_window<R>(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    files: &Entity<FileList>,
    f: impl FnOnce(&mut FileList, &mut Window, &mut Context<FileList>) -> R,
) -> R {
    cx.update_window(window, |_, window, cx| files.update(cx, |list, cx| f(list, window, cx)))
        .unwrap()
}

fn record(cx: &mut TestAppContext, files: &Entity<FileList>) -> Rc<RefCell<Vec<FileListEvent>>> {
    let events = Rc::new(RefCell::new(Vec::new()));
    cx.update(|cx| {
        let events = events.clone();
        cx.subscribe(files, move |_, event: &FileListEvent, _| {
            events.borrow_mut().push(event.clone())
        })
        .detach();
    });
    events
}

/// Stack is off for frames of two kinds, before it runs, as for too few.
#[gpui::test]
fn mixed_kinds_cant_stack(cx: &mut TestAppContext) {
    let (window, files) = open(cx, |window, cx| {
        cx.new(|cx| FileList::new(crate::test_support::frames(), window, cx))
    });
    let (_dir, paths) = folder(&["a.cr2", "b.jpg", "c.jpg"]);
    let ids = on_window(cx, window, &files, |f, w, cx| f.add(paths.clone(), w, cx));
    files.read_with(cx, |f, _| {
        assert_eq!(f.stack_problem(), Some(StackProblem::MixedKinds(RawBackend::LibRaw)));
        assert!(!f.can_stack());
    });
    let raw = ids.iter().copied().find(|&id| path_of(&files, cx, id).ends_with("a.cr2")).unwrap();
    files.update(cx, |f, cx| f.set_included(raw, false, cx));
    files.read_with(cx, |f, _| assert!(f.can_stack()));
}

fn path_of(files: &Entity<FileList>, cx: &mut TestAppContext, id: FileId) -> PathBuf {
    files.read_with(cx, |f, _| f.rows().find(|(r, _)| *r == id).unwrap().1)
}

#[gpui::test]
fn adding_checking_and_selection_events(cx: &mut TestAppContext) {
    let (window, files) = open(cx, |window, cx| {
        cx.new(|cx| FileList::new(crate::test_support::frames(), window, cx))
    });
    let events = record(cx, &files);
    let (_dir, paths) = folder(&["b.jpg", "a.jpg", "c.jpg"]);

    let ids = on_window(cx, window, &files, |f, w, cx| f.add(paths.clone(), w, cx));
    assert_eq!(ids.len(), 3);
    files.read_with(cx, |f, _| {
        assert_eq!(f.summary().as_ref(), "3 frames");
        assert!(f.can_stack());
        let names: Vec<String> = f.rows().map(|(_, p)| file_label(&p)).collect();
        assert_eq!(names, ["a.jpg", "b.jpg", "c.jpg"]);
    });
    // Adding the same files again adds nothing.
    assert!(on_window(cx, window, &files, |f, w, cx| f.add(paths.clone(), w, cx)).is_empty());

    files.update(cx, |f, cx| f.set_included(ids[0], false, cx));
    files.read_with(cx, |f, _| {
        assert_eq!(f.included().len(), 2);
        assert_eq!(f.summary().as_ref(), "2 of 3 frames");
        assert_eq!(f.list().excluded.len(), 1);
    });

    // A click or arrow becomes `Selected` with the row's id.
    let view = files.read_with(cx, |f, _| f.view.clone());
    view.update(cx, |_, cx| cx.emit(ListEvent::Confirm(IndexPath::default().row(2))));
    cx.run_until_parked();
    assert_eq!(
        *events.borrow(),
        [FileListEvent::Changed, FileListEvent::Changed, FileListEvent::Selected(ids[2])]
    );
}

/// ⌘-click on the only selected row leaves none selected or highlighted, as
/// the Finder does.
#[gpui::test]
fn command_click_on_the_last_row_selected_leaves_none(cx: &mut TestAppContext) {
    let (window, files) = open(cx, |window, cx| {
        cx.new(|cx| FileList::new(crate::test_support::frames(), window, cx))
    });
    let (_dir, paths) = folder(&["a.jpg", "b.jpg"]);
    let ids = on_window(cx, window, &files, |f, w, cx| f.add(paths.clone(), w, cx));
    let view = files.read_with(cx, |f, _| f.view.clone());
    let click = |cx: &mut TestAppContext, row: usize, modifiers: Modifiers| {
        files.update(cx, |f, _| f.click = Some(modifiers));
        view.update(cx, |_, cx| cx.emit(ListEvent::Confirm(IndexPath::default().row(row))));
        cx.run_until_parked();
    };
    click(cx, 1, Modifiers::default());
    // The list highlights the row clicked itself.
    on_window(cx, window, &files, |f, w, cx| f.highlight(Some(ids[1]), w, cx));
    assert_eq!(files.read_with(cx, |f, cx| f.selected(cx)), Some(ids[1]));
    assert_eq!(files.read_with(cx, |f, _| f.selected_rows()), [ids[1]]);
    click(cx, 1, Modifiers::secondary_key());
    files.read_with(cx, |f, cx| {
        assert!(f.selected_rows().is_empty(), "none selected");
        assert_eq!(f.selected(cx), None, "none highlighted");
    });
}

/// Click, ⌘- and Shift-click select as in Finder; X excludes the selection,
/// then includes it. Stack uses the included frames regardless.
#[gpui::test]
fn selection_and_exclusion(cx: &mut TestAppContext) {
    let (window, files) = open(cx, |window, cx| {
        cx.new(|cx| FileList::new(crate::test_support::frames(), window, cx))
    });
    let (_dir, paths) = folder(&["a.jpg", "b.jpg", "c.jpg", "d.jpg", "e.jpg"]);
    let ids = on_window(cx, window, &files, |f, w, cx| f.add(paths.clone(), w, cx));
    let [a, b, c, d, e] = ids[..] else { unreachable!() };
    let view = files.read_with(cx, |f, _| f.view.clone());
    let click = |cx: &mut TestAppContext, row: usize, modifiers: Modifiers| {
        files.update(cx, |f, _| f.click = Some(modifiers));
        view.update(cx, |_, cx| cx.emit(ListEvent::Confirm(IndexPath::default().row(row))));
        cx.run_until_parked();
    };
    let selected = |cx: &mut TestAppContext| files.read_with(cx, |f, _| f.selected_rows());
    let (cmd, shift) = (Modifiers::secondary_key(), Modifiers::shift());

    click(cx, 1, Modifiers::default());
    assert_eq!(selected(cx), [b]);
    click(cx, 3, shift);
    assert_eq!(selected(cx), [b, c, d]);
    click(cx, 0, shift);
    assert_eq!(selected(cx), [a, b], "the anchor stays put");
    click(cx, 4, cmd);
    assert_eq!(selected(cx), [a, b, e]);
    files.read_with(cx, |f, _| {
        let (listed, stacked) = f.stack_input();
        assert_eq!(listed, paths, "a selection doesn't narrow the stack");
        assert_eq!(stacked, listed);
    });
    click(cx, 4, cmd);
    assert_eq!(selected(cx), [a, b]);

    files.update(cx, |f, cx| f.toggle_excluded(cx));
    let included = |cx: &mut TestAppContext| -> Vec<FileId> {
        files.read_with(cx, |f, _| f.included().into_iter().map(|r| r.0).collect())
    };
    assert_eq!(included(cx), [c, d, e]);
    files.update(cx, |f, cx| f.toggle_excluded(cx));
    assert_eq!(included(cx), [a, b, c, d, e]);
    files.read_with(cx, |f, _| {
        assert_eq!(f.targets(a), [a, b], "inside the selection: all of it");
        assert_eq!(f.targets(e), [e], "outside: that row");
    });
    // Exclude Frames After This, on the selection: what is below it.
    files.update(cx, |f, cx| f.set_included(d, false, cx));
    let after = files.read_with(cx, |f, _| f.included_after(&f.targets(a)));
    assert_eq!(after, [c, e], "the excluded row is left alone");
    files.update(cx, |f, cx| f.set_included_rows(&after, false, cx));
    assert_eq!(included(cx), [a, b]);

    // Include Only These, from the footer under the selection.
    click(cx, 2, Modifiers::default());
    click(cx, 3, shift);
    files.update(cx, |f, cx| {
        assert!(!f.selection_excludes(), "c and d are both excluded: Include");
        f.include_only_selection(cx)
    });
    assert_eq!(included(cx), [c, d]);
    files.read_with(cx, |f, _| assert_eq!(f.counts(), (2, 5)));
}

/// Remove from List takes rows out and shows the next; Clear List empties it
/// (not while locked, when nothing else changes either); a list handed in
/// replaces it and marks missing files.
#[gpui::test]
fn remove_clear_and_replace(cx: &mut TestAppContext) {
    let (window, files) = open(cx, |window, cx| {
        cx.new(|cx| FileList::new(crate::test_support::frames(), window, cx))
    });
    let events = record(cx, &files);
    let (_dir, paths) = folder(&["a.jpg", "b.jpg", "c.jpg"]);
    let ids = on_window(cx, window, &files, |f, w, cx| f.add(paths.clone(), w, cx));
    on_window(cx, window, &files, |f, w, cx| f.highlight(Some(ids[1]), w, cx));
    on_window(cx, window, &files, |f, w, cx| f.remove(&[ids[1]], w, cx));
    files.read_with(cx, |f, cx| {
        assert_eq!(f.rows().map(|r| r.0).collect::<Vec<_>>(), [ids[0], ids[2]]);
        assert_eq!(f.selected(cx), Some(ids[2]));
    });
    assert_eq!(events.borrow().last(), Some(&FileListEvent::Selected(ids[2])));

    files.update(cx, |f, cx| f.set_locked(true, cx));
    on_window(cx, window, &files, |f, w, cx| f.clear(w, cx));
    assert_eq!(files.read_with(cx, |f, _| f.rows.len()), 2);
    files.update(cx, |f, cx| {
        f.set_included(ids[0], false, cx);
        f.include_only_selection(cx);
        assert_eq!(f.counts(), (2, 2), "the running stack's list stays as it started");
    });
    files.update(cx, |f, cx| f.set_locked(false, cx));
    on_window(cx, window, &files, |f, w, cx| f.clear(w, cx));
    assert!(files.read_with(cx, |f, _| f.rows.is_empty()));
    assert_eq!(events.borrow().last(), Some(&FileListEvent::Cleared));

    let gone = paths[0].with_file_name("gone.jpg");
    let list = ImageList { files: vec![paths[0].clone(), gone], ..ImageList::default() };
    on_window(cx, window, &files, |f, w, cx| f.set_list(list, w, cx));
    // Which files are there is looked up off the UI thread.
    wait_until(cx, "the file check", |cx| files.read_with(cx, |f, _| f.missing_count() == 1));
    files.read_with(cx, |f, _| {
        assert_eq!(f.missing_count(), 1);
        assert_eq!(f.summary().as_ref(), "1 of 2 frames");
        assert!(!f.can_stack());
    });
    // The banner's Remove takes the missing frame out.
    on_window(cx, window, &files, |f, w, cx| f.remove_missing(w, cx));
    files.read_with(cx, |f, _| {
        assert_eq!(f.missing_count(), 0);
        assert_eq!(f.summary().as_ref(), "1 frame");
    });
}
/// A frame excluded during a capture-time sort stays excluded: the list is
/// sorted again, not replaced by the earlier one.
#[gpui::test]
fn an_edit_made_while_sorting_is_kept(cx: &mut TestAppContext) {
    let (window, files) = open(cx, |window, cx| {
        cx.new(|cx| FileList::new(crate::test_support::frames(), window, cx))
    });
    let (_dir, paths) = folder(&["a.jpg", "b.jpg", "c.jpg"]);
    let ids = on_window(cx, window, &files, |f, w, cx| f.add(paths.clone(), w, cx));
    on_window(cx, window, &files, |f, w, cx| {
        f.set_order(FrameOrder::CaptureTime, w, cx);
        f.set_included(ids[1], false, cx);
    });
    wait_until(cx, "the sort", |cx| {
        files.read_with(cx, |f, _| f.order() == FrameOrder::CaptureTime)
    });
    files.read_with(cx, |f, _| {
        assert_eq!(f.counts(), (2, 3), "b is still excluded");
        assert!(f.list().excluded.contains(&paths[1]));
    });
}

/// A list put in place during a capture-time sort drops the sort, leaving the
/// new list in its own order, unsaved.
#[gpui::test]
fn a_list_replaced_while_sorting_is_left_alone(cx: &mut TestAppContext) {
    let (window, files) = open(cx, |window, cx| {
        cx.new(|cx| FileList::new(crate::test_support::frames(), window, cx))
    });
    let (_dir, paths) = folder(&["a.jpg", "b.jpg", "c.jpg"]);
    let (_other_dir, other) = folder(&["x.jpg", "y.jpg"]);
    on_window(cx, window, &files, |f, w, cx| f.add(paths.clone(), w, cx));
    let events = record(cx, &files);
    let replacement = ImageList { files: other.clone(), ..ImageList::default() };
    on_window(cx, window, &files, |f, w, cx| {
        f.set_order(FrameOrder::CaptureTime, w, cx);
        f.set_list(replacement.clone(), w, cx);
    });
    // Long enough for the sort of three files to have come back.
    let until = std::time::Instant::now() + std::time::Duration::from_millis(500);
    while std::time::Instant::now() < until {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    files.read_with(cx, |f, _| {
        assert_eq!(f.order(), FrameOrder::Name, "the new list is not sorted");
        assert_eq!(f.list().files, other);
    });
    assert!(
        !events.borrow().contains(&FileListEvent::Changed),
        "nothing is saved over the new list's stack"
    );
}

/// Run tasks for `ms` milliseconds, enough for a small sort to return.
fn settle(cx: &mut TestAppContext, ms: u64) {
    let until = std::time::Instant::now() + std::time::Duration::from_millis(ms);
    while std::time::Instant::now() < until {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn names(cx: &mut TestAppContext, files: &Entity<FileList>) -> Vec<String> {
    files.read_with(cx, |f, _| f.rows().map(|(_, p)| file_label(&p)).collect())
}

/// In capture order, added files go after their folder's at once, then into
/// place when the times come back; Stack waits for them.
#[gpui::test]
fn adding_in_capture_order_sorts_off_the_ui_thread(cx: &mut TestAppContext) {
    let (window, files) = open(cx, |window, cx| {
        cx.new(|cx| FileList::new(crate::test_support::frames(), window, cx))
    });
    let (_dir, paths) = folder(&["a.jpg", "b.jpg", "c.jpg"]);
    let list = ImageList {
        files: paths[1..].to_vec(),
        order: FrameOrder::CaptureTime,
        ..ImageList::default()
    };
    on_window(cx, window, &files, |f, w, cx| f.set_list(list, w, cx));
    let events = record(cx, &files);
    let ids = on_window(cx, window, &files, |f, w, cx| f.add(vec![paths[0].clone()], w, cx));
    assert_eq!(ids.len(), 1);
    assert_eq!(names(cx, &files), ["b.jpg", "c.jpg", "a.jpg"]);
    files.read_with(cx, |f, _| {
        assert!(f.sorting());
        assert!(!f.can_stack(), "not until the frames are in their places");
    });
    wait_until(cx, "the sort", |cx| files.read_with(cx, |f, _| !f.sorting()));
    // Files without capture times stay in name order.
    assert_eq!(names(cx, &files), ["a.jpg", "b.jpg", "c.jpg"]);
    files.read_with(cx, |f, _| assert!(f.can_stack()));
    assert_eq!(*events.borrow(), [FileListEvent::Changed, FileListEvent::Changed]);
}

/// Choosing the current order during a sort to another drops it: the last
/// choice wins.
#[gpui::test]
fn the_order_chosen_last_wins(cx: &mut TestAppContext) {
    let (window, files) = open(cx, |window, cx| {
        cx.new(|cx| FileList::new(crate::test_support::frames(), window, cx))
    });
    let (_dir, paths) = folder(&["a.jpg", "b.jpg", "c.jpg"]);
    on_window(cx, window, &files, |f, w, cx| f.add(paths.clone(), w, cx));
    let events = record(cx, &files);
    on_window(cx, window, &files, |f, w, cx| {
        f.set_order(FrameOrder::CaptureTime, w, cx);
        assert!(f.sorting());
        f.set_order(FrameOrder::Name, w, cx);
        assert!(!f.sorting());
    });
    settle(cx, 500);
    files.read_with(cx, |f, _| assert_eq!(f.order(), FrameOrder::Name));
    assert!(events.borrow().is_empty(), "the list didn't change");
}

/// After Locate…, moved rows keep their ids: shown row and selection stay.
#[gpui::test]
fn following_a_relink_keeps_the_shown_row_and_the_selection(cx: &mut TestAppContext) {
    let (window, files) = open(cx, |window, cx| {
        cx.new(|cx| FileList::new(crate::test_support::frames(), window, cx))
    });
    let (dir, _) = folder(&[]);
    let (old, new) = (dir.path().join("card"), dir.path().join("Bee"));
    std::fs::create_dir_all(&old).unwrap();
    let paths: Vec<PathBuf> = ["a.jpg", "b.jpg", "c.jpg"]
        .iter()
        .map(|n| {
            std::fs::write(old.join(n), b"").unwrap();
            old.join(n)
        })
        .collect();
    let ids = on_window(cx, window, &files, |f, w, cx| f.add(paths.clone(), w, cx));
    on_window(cx, window, &files, |f, w, cx| f.select_rows(vec![ids[1], ids[2]], w, cx));
    std::fs::rename(&old, &new).unwrap();
    let moves = deptheos_app::Moves::new(vec![(old.clone(), new.clone())]);
    on_window(cx, window, &files, |f, w, cx| f.follow(&moves, w, cx));
    files.read_with(cx, |f, cx| {
        assert_eq!(f.rows().map(|r| r.0).collect::<Vec<_>>(), ids);
        assert_eq!(f.selected(cx), Some(ids[1]));
        assert_eq!(f.selected_rows(), [ids[1], ids[2]]);
        assert_eq!(f.path(ids[1]), Some(new.join("b.jpg").as_path()));
        assert_eq!(f.missing_count(), 0);
    });
}

/// The order notes follow the list through adds, removes and clears.
#[gpui::test]
fn the_order_notes_follow_the_list(cx: &mut TestAppContext) {
    let (window, files) = open(cx, |window, cx| {
        cx.new(|cx| FileList::new(crate::test_support::frames(), window, cx))
    });
    let (_one, first) = folder(&["a.jpg", "b.jpg"]);
    let (_two, second) = folder(&["c.jpg", "d.jpg"]);
    let list =
        ImageList { files: first.clone(), order: FrameOrder::CaptureTime, ..ImageList::default() };
    on_window(cx, window, &files, |f, w, cx| f.set_list(list, w, cx));
    let notes = |cx: &mut TestAppContext| files.read_with(cx, |f, _| f.order_notes().len());
    wait_until(cx, "the first folder's note", |cx| notes(cx) == 1);
    let added = on_window(cx, window, &files, |f, w, cx| f.add(second.clone(), w, cx));
    wait_until(cx, "the second folder's note", |cx| notes(cx) == 2);
    on_window(cx, window, &files, |f, w, cx| f.remove(&added, w, cx));
    wait_until(cx, "the second folder's note gone", |cx| notes(cx) == 1);
    on_window(cx, window, &files, |f, w, cx| f.clear(w, cx));
    wait_until(cx, "no notes", |cx| notes(cx) == 0);
}
