//! The renders in a whole app on a real engine: what store work leaves on
//! screen.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use deptheos_app::Engine;
use deptheos_app::library::history::RenderId;
use gpui::{AnyWindowHandle, AppContext as _, Entity, TestAppContext};

use crate::app::AppView;
use crate::files::FileId;
use crate::test_support::{act, engine, open, wait_until, write_frames};
use crate::words::Wording as _;

fn launch(
    cx: &mut TestAppContext,
    engine: &Arc<Engine>,
    paths: Vec<PathBuf>,
    root: &Path,
) -> (AnyWindowHandle, Entity<AppView>) {
    let (engine, root) = (engine.clone(), crate::test_support::opened(root));
    let opened = open(cx, |window, cx| cx.new(|cx| AppView::new(engine, paths, root, window, cx)));
    let app = opened.1.clone();
    wait_until(cx, "the history", |cx| {
        app.read_with(cx, |app, cx| app.renders.read(cx).is_listed())
    });
    opened
}

fn stack(cx: &mut TestAppContext, window: AnyWindowHandle, app: &Entity<AppView>) {
    wait_until(cx, "the images", |cx| app.read_with(cx, |app, cx| app.files.read(cx).can_stack()));
    act(cx, window, app, |app, window, cx| app.run_stacking(window, cx));
    wait_until(cx, "the stack and its save", |cx| {
        app.read_with(cx, |app, cx| !app.is_processing(cx) && !app.renders.read(cx).is_saving())
    });
}

/// A working version kept while another stack shows is based on its render;
/// deleting that render bases it on none, in the Library's view too.
#[gpui::test]
fn deleting_a_kept_versions_base_reads_the_stacks_again(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let paths = write_frames(dir.path(), 3);
    let (window, app) = launch(cx, &engine, paths, &dir.path().join("history"));
    stack(cx, window, &app);
    let (render, stack_id): (RenderId, _) = app.read_with(cx, |app, cx| {
        let r = &app.renders.read(cx).renders()[0];
        (r.id.clone(), r.stack.clone())
    });
    // The Library's names, found in one pass, are the stack's own.
    app.read_with(cx, |app, cx| {
        let renders = app.renders.read(cx);
        let w = cx.wording();
        assert_eq!(renders.stack_names(w)[&stack_id], renders.stack_name(&stack_id, w));
    });
    // An edit, so the working version is kept when another stack is shown.
    let first: FileId = app.read_with(cx, |app, cx| app.files.read(cx).included()[0].0);
    app.update(cx, |app, cx| app.files.update(cx, |f, cx| f.set_included(first, false, cx)));
    act(cx, window, &app, |app, window, cx| app.new_stack(window, cx));
    crate::test_support::switched(cx, &app);
    let base = |cx: &mut TestAppContext| {
        app.read_with(cx, |app, cx| {
            let info = app.renders.read(cx).stack_info(&stack_id).cloned();
            info.and_then(|i| i.working).map(|w| w.base)
        })
    };
    wait_until(cx, "the kept version", |cx| base(cx) == Some(Some(render.clone())));

    act(cx, window, &app, |app, window, cx| {
        app.renders.update(cx, |r, cx| r.delete(vec![render.clone()], window, cx))
    });
    wait_until(cx, "the delete", |cx| {
        app.read_with(cx, |app, cx| app.renders.read(cx).renders().is_empty())
    });
    wait_until(cx, "the stacks read again", |cx| base(cx) == Some(None));
}

/// A delete whose worker dies part way still rereads the Library: a render
/// it took leaves the strip, and the failure is said.
#[gpui::test]
fn a_delete_that_panics_reads_the_library_again(cx: &mut TestAppContext) {
    use super::queue::Doomed;

    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let paths = write_frames(dir.path(), 3);
    let (window, app) = launch(cx, &engine, paths, &dir.path().join("history"));
    stack(cx, window, &app);
    let folder = app.read_with(cx, |app, cx| {
        let renders = app.renders.read(cx);
        renders.store().render_path(&renders.renders()[0].id)
    });
    act(cx, window, &app, |app, window, cx| {
        app.renders.update(cx, |r, cx| {
            r.run_delete(
                Doomed::Renders,
                || (),
                move |()| {
                    std::fs::remove_dir_all(&folder).unwrap();
                    panic!("died part way");
                },
                |_, _, _, _| unreachable!("it didn't finish"),
                window,
                cx,
            )
        })
    });
    wait_until(cx, "the Library read again", |cx| {
        app.read_with(cx, |app, cx| {
            let renders = app.renders.read(cx);
            renders.renders().is_empty() && !renders.is_deleting()
        })
    });
}

/// A delete runs on the store thread after earlier writes: one sent as its
/// paint closes keeps the render until done.
#[gpui::test]
fn a_delete_waits_for_the_writes_before_it(cx: &mut TestAppContext) {
    use super::queue::Doomed;

    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let paths = write_frames(dir.path(), 3);
    let (window, app) = launch(cx, &engine, paths, &dir.path().join("history"));
    stack(cx, window, &app);
    let (store, render) = app.read_with(cx, |app, cx| {
        let renders = app.renders.read(cx);
        (renders.store().clone(), renders.renders()[0].id.clone())
    });
    let folder = store.render_path(&render);
    let (release, held) = std::sync::mpsc::channel::<()>();
    let (started, running) = std::sync::mpsc::channel::<()>();
    let closing = (store.clone(), render.clone());
    act(cx, window, &app, |app, window, cx| {
        app.renders.update(cx, |r, cx| {
            r.run_delete(
                Doomed::Renders,
                move || {
                    let closed = closing.0.close_paint(std::slice::from_ref(&closing.1));
                    // A write sent meanwhile, still running.
                    drop(deptheos_app::engine::store::store().write(move || {
                        started.send(()).unwrap();
                        held.recv().ok();
                    }));
                    running.recv().unwrap();
                    closed
                },
                move |closed| store.delete_renders(&[render], closed),
                |_, _, _, _| {},
                window,
                cx,
            )
        })
    });
    for _ in 0..6 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        cx.run_until_parked();
    }
    assert!(folder.exists(), "it waits for the write before it");

    release.send(()).unwrap();
    wait_until(cx, "the delete", |cx| {
        !app.read_with(cx, |app, cx| app.renders.read(cx).is_deleting())
    });
    assert!(!folder.exists());
}

/// Deleting a render asks nothing (ADR-140): into Recently Deleted at once,
/// a notice says so, and its Undo restores it in the strip and on disk.
#[gpui::test]
fn deleting_a_render_asks_nothing_and_undo_brings_it_back(cx: &mut TestAppContext) {
    use gpui_kit::component::WindowExt as _;
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let paths = write_frames(dir.path(), 3);
    let (window, app) = launch(cx, &engine, paths, &dir.path().join("history"));
    stack(cx, window, &app);
    let render = app.read_with(cx, |app, cx| app.renders.read(cx).renders()[0].id.clone());
    let folder = app.read_with(cx, |app, cx| app.renders.read(cx).store().render_path(&render));
    let kept =
        |cx: &mut TestAppContext| app.read_with(cx, |app, cx| app.renders.read(cx).renders().len());

    act(cx, window, &app, |app, window, cx| {
        app.renders.update(cx, |r, cx| r.delete(vec![render.clone()], window, cx))
    });
    let asking = cx.update_window(window, |_, window, cx| window.has_active_dialog(cx)).unwrap();
    assert!(!asking, "it asks nothing");
    wait_until(cx, "the delete", |cx| kept(cx) == 0);
    assert!(!folder.exists(), "moved out of the Library's renders");
    let said = cx.update_window(window, |_, window, cx| crate::notices::last_said(window, cx));
    let said = said.unwrap().expect("a notice");
    assert!(said.ends_with(" #1 moved to Recently Deleted."), "{said}");
    wait_until(cx, "Recently Deleted read", |cx| {
        app.read_with(cx, |app, cx| app.renders.read(cx).trash().is_some_and(|(n, _)| n == 1))
    });

    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    vcx.run_until_parked();
    let undo = vcx.debug_bounds("notice-action-0").expect("Undo").center();
    vcx.simulate_click(undo, gpui::Modifiers::none());
    wait_until(cx, "the restore", |cx| {
        kept(cx) == 1 && !app.read_with(cx, |app, cx| app.renders.read(cx).is_deleting())
    });
    assert!(folder.exists(), "back where it was");
    app.read_with(cx, |app, cx| {
        assert_eq!(app.renders.read(cx).renders()[0].id, render, "the very render");
        assert!(app.renders.read(cx).strip().contains(&render), "in the strip again");
    });
    wait_until(cx, "Recently Deleted emptied by the restore", |cx| {
        app.read_with(cx, |app, cx| app.renders.read(cx).trash().is_some_and(|(n, _)| n == 0))
    });
}

/// A thumbnail unreadable at launch is read again once its file is back.
#[gpui::test]
fn a_thumbnail_that_failed_is_read_again(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("history");
    let paths = write_frames(dir.path(), 2);
    let (window, app) = launch(cx, &engine, paths, &root);
    stack(cx, window, &app);
    crate::test_support::quit(cx, window, app);
    // The render's thumbnail goes away for a while (a drive asleep).
    let render = std::fs::read_dir(root.join("Renders")).unwrap().next().unwrap().unwrap().path();
    let thumbnail = render.join("thumb.jpg");
    let aside = dir.path().join("thumbnail-aside");
    std::fs::rename(&thumbnail, &aside).unwrap();

    let (_window, app) = launch(cx, &engine, Vec::new(), &root);
    let renders = app.read_with(cx, |app, _| app.renders.clone());
    let id = renders.read_with(cx, |r, _| r.renders()[0].id.clone());
    wait_until(cx, "the failed read", |cx| renders.read_with(cx, |r, _| r.thumbnail_failed(&id)));
    // Not read again at once, however often it is drawn.
    renders.update(cx, |r, cx| r.request_thumbnails(vec![id.clone()], cx));
    cx.run_until_parked();
    assert!(renders.read_with(cx, |r, _| r.thumbnail(&id).is_none()));

    std::fs::rename(&aside, &thumbnail).unwrap();
    renders.update(cx, |r, cx| {
        r.age_thumbnail_failures();
        r.request_thumbnails(vec![id.clone()], cx)
    });
    wait_until(cx, "the thumbnail", |cx| renders.read_with(cx, |r, _| r.thumbnail(&id).is_some()));
}

/// Past the kept thumbnails, the longest-unwanted are dropped (never those
/// in view) and reread when next in view.
#[gpui::test]
fn thumbnails_past_the_cap_go_and_come_back(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("history");
    let paths = write_frames(dir.path(), 2);
    let (window, app) = launch(cx, &engine, paths, &root);
    stack(cx, window, &app);
    crate::test_support::quit(cx, window, app);
    // The one render, copied into a history of twelve.
    let renders = root.join("Renders");
    let first = std::fs::read_dir(&renders).unwrap().next().unwrap().unwrap().path();
    for n in 1..12 {
        let copy = renders.join(format!("{n:032x}"));
        std::fs::create_dir_all(&copy).unwrap();
        for file in std::fs::read_dir(&first).unwrap().flatten() {
            if file.path().is_file() {
                std::fs::copy(file.path(), copy.join(file.file_name())).unwrap();
            }
        }
    }

    let (window, app) = launch(cx, &engine, Vec::new(), &root);
    let renders = app.read_with(cx, |app, _| app.renders.clone());
    let ids: Vec<RenderId> =
        renders.read_with(cx, |r, _| r.renders().iter().map(|r| r.id.clone()).collect());
    assert_eq!(ids.len(), 12);
    let kept = |cx: &mut TestAppContext| renders.read_with(cx, |r, _| r.thumbnail_ids());
    wait_until(cx, "the launch's thumbnails", |cx| kept(cx).len() == 12);
    // The strip would keep all twelve in view, so view is declared here.
    cx.update_window(window, |_, window, _| window.remove_window()).unwrap();
    renders.update(cx, |r, _| r.set_thumbs_kept(4));
    let show = |cx: &mut TestAppContext, shown: &[RenderId]| {
        renders.update(cx, |r, cx| r.request_thumbnails(shown.to_vec(), cx));
        let want: std::collections::HashSet<RenderId> = shown.iter().cloned().collect();
        wait_until(cx, "the thumbnails in view", |cx| want.is_subset(&kept(cx)));
    };
    show(cx, &ids[0..4]);
    assert_eq!(kept(cx), ids[0..4].iter().cloned().collect());
    show(cx, &ids[4..8]);
    assert_eq!(kept(cx), ids[4..8].iter().cloned().collect());
    show(cx, &ids[0..2]);
    let now = kept(cx);
    assert!(now.len() <= 4, "{} kept", now.len());
    assert!(ids[0..2].iter().all(|id| now.contains(id)), "read again");
}

/// Space shows the selected cover large, Space or Esc closes, arrows move
/// on. A cover read after moving on or closing isn't shown; leaving the
/// Library closes it.
#[gpui::test]
fn space_previews_a_stacks_cover(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("history");
    let (a, b) = (dir.path().join("ants"), dir.path().join("bees"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let (ants, bees) = (write_frames(&a, 2), write_frames(&b, 2));
    // ants #1 and bees #1, in stacks of their own.
    let (window, app) = launch(cx, &engine, ants, &root);
    stack(cx, window, &app);
    act(cx, window, &app, |app, window, cx| app.new_stack(window, cx));
    crate::test_support::switched(cx, &app);
    act(cx, window, &app, |app, window, cx| app.add_paths(bees, window, cx));
    stack(cx, window, &app);
    let bee = app.read_with(cx, |app, cx| app.renders.read(cx).active(cx).cloned()).unwrap();
    crate::test_support::quit(cx, window, app);
    // Bees' thumbnail is away for a while: its preview waits for it.
    let thumbnail = root.join("Renders").join(bee.as_str()).join("thumb.jpg");
    let aside = dir.path().join("thumbnail-aside");
    std::fs::rename(&thumbnail, &aside).unwrap();

    let (window, app) = launch(cx, &engine, Vec::new(), &root);
    cx.update(|cx| cx.bind_keys(crate::app::actions::key_bindings()));
    act(cx, window, &app, |app, window, cx| {
        app.set_screen(crate::app::Screen::Library, window, cx)
    });
    let library = app.read_with(cx, |app, _| app.library.clone());
    let renders = app.read_with(cx, |app, _| app.renders.clone());
    let preview = |cx: &mut TestAppContext| library.read_with(cx, |l, cx| l.preview_text(cx));
    let ant = renders.read_with(cx, |r, _| {
        r.renders().iter().find(|r| r.id != bee).map(|r| r.id.clone()).unwrap()
    });
    wait_until(cx, "the thumbnails", |cx| {
        renders.read_with(cx, |r, _| r.thumbnail_ids().contains(&ant) && r.thumbnail_failed(&bee))
    });
    // The Library's order is newest first: bees, then ants.
    cx.simulate_keystrokes(window, "escape right");
    assert_eq!(library.read_with(cx, |l, cx| l.selected_titles(cx)), ["bees"]);

    cx.simulate_keystrokes(window, "space");
    assert!(preview(cx).ends_with("bees") || preview(cx).contains("bees ("), "{}", preview(cx));
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    assert!(vcx.debug_bounds("library-preview").is_some(), "shown over the Library");
    // On to ants before bees' read lands: bees' is dropped, and ants' stored
    // picture shows at its own size.
    cx.simulate_keystrokes(window, "right");
    assert_eq!(library.read_with(cx, |l, cx| l.selected_titles(cx)), ["ants"]);
    wait_until(cx, "ants' picture", |cx| {
        library.read_with(cx, |l, cx| l.preview_text(cx)) != "Loading ants"
    });
    assert_eq!(preview(cx), "Shown ants");
    let stored = root.join("Renders").join(ant.as_str()).join("thumb.jpg");
    let (w, h) = image::image_dimensions(stored).unwrap();
    assert_eq!(library.read_with(cx, |l, _| l.preview_picture_size()), Some((w as i32, h as i32)));
    // Bees' picture can't be read: its placeholder.
    cx.simulate_keystrokes(window, "left");
    wait_until(cx, "bees' read", |cx| {
        library.read_with(cx, |l, cx| l.preview_text(cx)) != "Loading bees"
    });
    assert_eq!(preview(cx), "Shown bees (placeholder)");
    cx.simulate_keystrokes(window, "right");
    wait_until(cx, "ants' picture", |cx| {
        library.read_with(cx, |l, cx| l.preview_text(cx)) == "Shown ants"
    });
    std::fs::rename(&aside, &thumbnail).unwrap();

    // Esc closes it, and the selection stays; Space again shows bees.
    cx.simulate_keystrokes(window, "escape");
    assert_eq!(preview(cx), "Closed");
    assert_eq!(library.read_with(cx, |l, cx| l.selected_titles(cx)), ["ants"]);
    cx.simulate_keystrokes(window, "left space");
    wait_until(cx, "bees' picture", |cx| {
        library.read_with(cx, |l, cx| l.preview_text(cx)) == "Shown bees"
    });
    cx.simulate_keystrokes(window, "space");
    assert_eq!(preview(cx), "Closed");

    // A click on it closes it too, and so does leaving the Library.
    cx.simulate_keystrokes(window, "space");
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    let shown = vcx.debug_bounds("library-preview").expect("the preview");
    vcx.simulate_click(shown.center(), gpui::Modifiers::none());
    assert_eq!(preview(cx), "Closed");
    assert_eq!(library.read_with(cx, |l, cx| l.selected_titles(cx)), ["bees"], "kept");
    cx.simulate_keystrokes(window, "space");
    wait_until(cx, "bees' picture", |cx| {
        library.read_with(cx, |l, cx| l.preview_text(cx)) == "Shown bees"
    });
    cx.simulate_keystrokes(window, "secondary-alt-1");
    assert_eq!(preview(cx), "Closed");
}

/// The details pane draws its render from the store's larger picture (a
/// portrait at stored size); moving on reads the next, and the pane never
/// keeps a picture for a stack it left.
#[gpui::test]
fn the_details_pane_reads_its_renders_picture(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("history");
    let (a, b) = (dir.path().join("ants"), dir.path().join("bees"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    // Ants upright, 400 tall: taller than the strip decodes.
    let ants = deptheos_app::test_support::checkerboard_frames(&a, 2, (266, 400));
    let bees = write_frames(&b, 2);
    let (window, app) = launch(cx, &engine, ants, &root);
    stack(cx, window, &app);
    let ant = app.read_with(cx, |app, cx| app.renders.read(cx).active(cx).cloned()).unwrap();
    act(cx, window, &app, |app, window, cx| app.new_stack(window, cx));
    crate::test_support::switched(cx, &app);
    act(cx, window, &app, |app, window, cx| app.add_paths(bees, window, cx));
    stack(cx, window, &app);
    let bee = app.read_with(cx, |app, cx| app.renders.read(cx).active(cx).cloned()).unwrap();
    crate::test_support::quit(cx, window, app);

    let (window, app) = launch(cx, &engine, Vec::new(), &root);
    cx.update(|cx| cx.bind_keys(crate::app::actions::key_bindings()));
    let library = app.read_with(cx, |app, _| app.library.clone());
    let cover = |cx: &mut TestAppContext| library.read_with(cx, |l, _| l.cover_picture());
    let stored = |id: &RenderId| {
        let path = root.join("Renders").join(id.as_str()).join("thumb.jpg");
        let (w, h) = image::image_dimensions(path).unwrap();
        (w as i32, h as i32)
    };
    // Newest first: bees, then ants, moved to before bees' picture is drawn.
    cx.simulate_keystrokes(window, "escape right");
    assert_eq!(library.read_with(cx, |l, cx| l.selected_titles(cx)), ["bees"]);
    cx.simulate_keystrokes(window, "right");
    assert_eq!(library.read_with(cx, |l, cx| l.selected_titles(cx)), ["ants"]);
    wait_until(cx, "ants' picture", |cx| cover(cx).is_some());
    let (shown, size) = cover(cx).unwrap();
    assert_eq!(shown, ant, "the stack selected, not the one left");
    assert_eq!(size, stored(&ant));
    assert!(size.1 > 168, "sharper than the strip's thumbnail: {size:?}");
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    assert!(vcx.debug_bounds("library-details-cover").is_some());

    // Back to bees: theirs.
    cx.simulate_keystrokes(window, "left");
    wait_until(cx, "bees' picture", |cx| cover(cx).is_some_and(|(id, _)| id == bee));
    assert_eq!(cover(cx).unwrap().1, stored(&bee));
}

/// Space over the details pane's stack reuses the pane's picture, which the
/// pane keeps after the preview closes.
#[gpui::test]
fn space_takes_the_picture_the_details_pane_shows(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let paths = write_frames(dir.path(), 2);
    let (window, app) = launch(cx, &engine, paths, &dir.path().join("history"));
    cx.update(|cx| cx.bind_keys(crate::app::actions::key_bindings()));
    stack(cx, window, &app);
    act(cx, window, &app, |app, window, cx| {
        app.set_screen(crate::app::Screen::Library, window, cx)
    });
    let library = app.read_with(cx, |app, _| app.library.clone());
    cx.simulate_keystrokes(window, "escape right");
    wait_until(cx, "the pane's picture", |cx| {
        library.read_with(cx, |l, _| l.cover_picture().is_some())
    });

    cx.simulate_keystrokes(window, "space");
    library.read_with(cx, |l, cx| {
        assert!(l.preview_text(cx).starts_with("Shown"), "at once: {}", l.preview_text(cx));
        assert!(l.preview_shares_cover(), "the pane's own picture");
    });
    cx.simulate_keystrokes(window, "space");
    cx.run_until_parked();
    assert!(library.read_with(cx, |l, _| l.cover_picture().is_some()), "the pane keeps it");
}

/// A details-pane render is selected by a click and opened by ↩ or a
/// double-click.
#[gpui::test]
fn a_render_in_the_details_selects_then_opens(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let paths = write_frames(dir.path(), 2);
    let (window, app) = launch(cx, &engine, paths, &dir.path().join("history"));
    cx.update(|cx| cx.bind_keys(crate::app::actions::key_bindings()));
    // #1, then #2.
    stack(cx, window, &app);
    let first = app.read_with(cx, |app, cx| app.renders.read(cx).active(cx).cloned()).unwrap();
    stack(cx, window, &app);
    let second = app.read_with(cx, |app, cx| app.renders.read(cx).active(cx).cloned()).unwrap();
    let screen = |cx: &mut TestAppContext| app.read_with(cx, |app, _| app.screen);
    let active = |cx: &mut TestAppContext| {
        app.read_with(cx, |app, cx| {
            let renders = app.renders.read(cx);
            (!renders.is_pending()).then(|| renders.active(cx).cloned()).flatten()
        })
    };
    cx.simulate_keystrokes(window, "secondary-alt-2");
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    let row = vcx.debug_bounds("library-render-1").expect("#1 in the details pane").center();

    // A click selects and stays; ↑ and ↓ step the renders (newest first),
    // ↩ opens the selected.
    vcx.simulate_click(row, gpui::Modifiers::none());
    assert_eq!(screen(cx), crate::app::Screen::Library);
    cx.simulate_keystrokes(window, "up");
    cx.simulate_keystrokes(window, "down");
    cx.simulate_keystrokes(window, "enter");
    wait_until(cx, "#1", |cx| active(cx) == Some(first.clone()));
    assert_eq!(screen(cx), crate::app::Screen::Stacking);

    // A double-click opens it at once.
    cx.simulate_keystrokes(window, "secondary-alt-2");
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    let row = vcx.debug_bounds("library-render-2").expect("#2 in the details pane").center();
    for count in [1, 2] {
        vcx.simulate_event(gpui::MouseDownEvent {
            position: row,
            modifiers: gpui::Modifiers::none(),
            button: gpui::MouseButton::Left,
            click_count: count,
            first_mouse: false,
        });
        vcx.simulate_event(gpui::MouseUpEvent {
            position: row,
            modifiers: gpui::Modifiers::none(),
            button: gpui::MouseButton::Left,
            click_count: count,
        });
    }
    wait_until(cx, "#2", |cx| active(cx) == Some(second.clone()));
    assert_eq!(screen(cx), crate::app::Screen::Stacking);
}
