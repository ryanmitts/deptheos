//! Frames: adding, dropping, excluding and relinking them.

use super::*;

/// Frames added while #1 of two is shown keep #1 the render shown: the title
/// names it and the change line reads against it.
#[gpui::test]
fn frames_added_keep_the_render_shown(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let (bee, more) = (dir.path().join("bee"), dir.path().join("more"));
    std::fs::create_dir_all(&bee).unwrap();
    std::fs::create_dir_all(&more).unwrap();
    let paths = write_frames(&bee, 3);
    let added = write_frames(&more, 1);
    let (window, app) = launch(cx, &engine, paths, &dir.path().join("history"));
    let summary =
        |cx: &mut TestAppContext| app.read_with(cx, |app, cx| app.files.read(cx).summary());

    // #1, then #2 of two frames, then #1 shown.
    stack(cx, window, &app);
    let first = active(cx, &app).unwrap();
    exclude_first(cx, &app);
    stack(cx, window, &app);
    open_render(cx, window, &app, &first);

    act(cx, window, &app, |app, window, cx| app.add_paths(added.clone(), window, cx));
    wait_until(cx, "the frame added", |cx| summary(cx).as_ref() == "3 of 4 frames");
    assert_eq!(active(cx, &app).as_ref(), Some(&first), "#1 stays the render shown");
    app.read_with(cx, |app, cx| {
        assert!(app.has_result(cx));
        assert_eq!(app.title_parts(cx).what.as_deref(), Some("#1 · Pyramid · 3 frames"));
        let line = app.compared(cx).expect("the working version differs from #1");
        assert!(line.text.contains("#1"), "{line:?}");
        assert!(!line.edited && line.revert == Some(1), "{line:?}");
    });
    // The frame added is in the viewer, over #1.
    wait_until(cx, "the frame added shown", |cx| {
        app.read_with(cx, |app, cx| {
            let shown = app.viewer.read(cx).shown_file();
            shown.and_then(|f| app.files.read(cx).path(f)) == Some(added[0].as_path())
        })
    });
    assert_eq!(active(cx, &app).as_ref(), Some(&first));
}

/// Remove All Frames in a never-stacked stack asks, Cancel default: Return
/// keeps the frames.
#[gpui::test]
fn return_in_remove_all_frames_keeps_them(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    cx.update(|cx| cx.bind_keys(key_bindings()));
    wait_until(cx, "the images", |cx| app.read_with(cx, |app, cx| app.files.read(cx).can_stack()));
    act(cx, window, &app, |app, window, cx| app.set_screen(Screen::Stacking, window, cx));
    act(cx, window, &app, |app, window, cx| app.confirm_clear(window, cx));
    cx.run_until_parked();
    assert!(asking(cx, window), "a stack with no render asks");
    cx.simulate_keystrokes(window, "enter");
    cx.run_until_parked();
    assert!(!asking(cx, window));
    app.read_with(cx, |app, cx| assert_eq!(app.files.read(cx).summary().as_ref(), "3 frames"));
}

/// Files dropped on an empty window are listed and the first previews.
#[gpui::test]
fn dropping_files_adds_and_shows_them(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    write_frames(dir.path(), 2);
    let (window, app) = open(cx, |window, cx| {
        cx.new(|cx| {
            AppView::new(
                engine,
                Vec::new(),
                crate::test_support::opened(&dir.path().join("history")),
                window,
                cx,
            )
        })
    });
    let dropped = ExternalPaths([dir.path().to_path_buf()].into_iter().collect());
    act(cx, window, &app, |app, window, cx| app.drop_files(&dropped, window, cx));
    wait_until(cx, "the folder", |cx| {
        app.read_with(cx, |app, cx| app.files.read(cx).summary().as_ref() == "2 frames")
    });
    wait_until(cx, "the preview", |cx| {
        app.read_with(cx, |app, cx| app.viewer.read(cx).has_content())
    });
}

/// Files come from anywhere: two folders share the list, a folder re-added
/// adds nothing, settings stay.
#[gpui::test]
fn images_add_from_any_folder(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (dir.path().join("a"), dir.path().join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    write_frames(&a, 2);
    let loose = write_frames(&b, 3);
    let (window, app) = open(cx, |window, cx| {
        cx.new(|cx| {
            AppView::new(
                engine,
                Vec::new(),
                crate::test_support::opened(&dir.path().join("history")),
                window,
                cx,
            )
        })
    });
    // Listed off the UI thread: each add waits for the list it makes.
    let add = |cx: &mut TestAppContext, paths: Vec<PathBuf>, then: &str| {
        act(cx, window, &app, |app, window, cx| app.add_paths(paths, window, cx));
        wait_until(cx, then, |cx| {
            app.read_with(cx, |app, cx| app.files.read(cx).summary().as_ref() == then)
        });
    };
    add(cx, vec![a.clone()], "2 frames");
    add(cx, vec![loose[2].clone(), loose[0].clone()], "4 frames");
    add(cx, vec![a.clone()], "4 frames");
    app.read_with(cx, |app, cx| {
        let files = app.files.read(cx);
        assert_eq!(files.summary().as_ref(), "4 frames");
        let folders: Vec<_> =
            files.rows().map(|(_, p)| p.parent().unwrap().to_path_buf()).collect();
        assert_eq!(folders, [a.clone(), a.clone(), b.clone(), b.clone()]);
    });
}

/// ⌘O works in the Library too: the frames picked start a stack in Stacking.
#[gpui::test]
fn add_frames_opens_from_the_library(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let frames = write_frames(dir.path(), 2);
    let (window, app) = launch(cx, &engine, Vec::new(), &dir.path().join("history"));
    cx.update(|cx| cx.bind_keys(key_bindings()));
    assert_eq!(app.read_with(cx, |app, _| app.screen), Screen::Library);
    cx.simulate_keystrokes(window, "secondary-o");
    assert!(cx.did_prompt_for_paths(), "⌘O asks for frames in the Library");
    cx.simulate_path_prompt_response(|_| Some(frames.clone()));
    wait_until(cx, "the frames", |cx| {
        app.read_with(cx, |app, cx| app.files.read(cx).summary().as_ref() == "2 frames")
    });
    assert_eq!(app.read_with(cx, |app, _| app.screen), Screen::Stacking);
}

/// A folder renamed between launches is found by its bookmark: list and
/// render follow, nothing missing.
#[cfg(target_os = "macos")]
#[gpui::test]
fn a_folder_renamed_is_found_again_at_launch(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let (trip, moved) = (dir.path().join("trip"), dir.path().join("trip 2024"));
    std::fs::create_dir_all(&trip).unwrap();
    let paths = write_frames(&trip, 2);
    let root = dir.path().join("history");
    let (window, app) = launch(cx, &engine, paths, &root);
    stack(cx, window, &app);
    let render = active(cx, &app).unwrap();
    wait_until(cx, "the list saved", |cx| {
        crate::test_support::library(cx, &app).read_working().unwrap().base.as_ref()
            == Some(&render)
    });

    crate::test_support::quit(cx, window, app);
    std::fs::rename(&trip, &moved).unwrap();
    let (_, app) = launch(cx, &engine, Vec::new(), &root);
    let stored = deptheos_app::frames::sources::canonical(&moved).unwrap();
    wait_until(cx, "the folder found again", |cx| {
        app.read_with(cx, |app, cx| {
            let renders = app.renders.read(cx).renders();
            let files = app.files.read(cx);
            renders[0].frames.iter().all(|f| f.starts_with(&stored))
                && files.rows().all(|(_, p)| p.starts_with(&stored))
                && files.missing_count() == 0
        })
    });
}

/// A folder only partly matching a lost root is never offered or taken; the
/// frames stay missing until Locate….
#[gpui::test]
fn a_folder_that_only_partly_matches_is_not_offered(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let (trip, moved) = (dir.path().join("trip"), dir.path().join("trip 2024"));
    std::fs::create_dir_all(&trip).unwrap();
    let paths = write_frames(&trip, 2);
    let names = paths.clone();
    let root = dir.path().join("history");
    let (window, app) = launch(cx, &engine, paths, &root);
    stack(cx, window, &app);
    let render = active(cx, &app).unwrap();
    wait_until(cx, "the list saved", |cx| {
        crate::test_support::library(cx, &app).read_working().unwrap().base.as_ref()
            == Some(&render)
    });
    crate::test_support::quit(cx, window, app);

    // Copied, one frame changed on the way, and the original deleted.
    std::fs::create_dir_all(&moved).unwrap();
    for entry in std::fs::read_dir(&trip).unwrap() {
        let entry = entry.unwrap().path();
        std::fs::copy(&entry, moved.join(entry.file_name().unwrap())).unwrap();
    }
    let changed = moved.join(names[1].file_name().unwrap());
    std::fs::write(&changed, b"not the frame it was").unwrap();
    let was = deptheos_app::frames::sources::canonical(&trip).unwrap();
    std::fs::remove_dir_all(&trip).unwrap();
    let (_, app) = launch(cx, &engine, Vec::new(), &root);
    wait_until(cx, "the frames missing where they were", |cx| {
        app.read_with(cx, |app, cx| {
            let renders = app.renders.read(cx).renders();
            !renders.is_empty()
                && renders[0].frames.iter().all(|f| f.starts_with(&was))
                && app.files.read(cx).missing_count() == 2
        })
    });
    let store = app.read_with(cx, |app, cx| app.renders.read(cx).store().clone());
    let id = store.frames().sources().roots().next().unwrap().0.clone();
    assert!(store.frames().is_lost(&id), "not taken");
}

/// A frame on an absent volume shows as not connected (not missing), and its
/// render keeps it.
#[gpui::test]
fn a_frame_on_a_volume_not_there_is_not_connected(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let trip = dir.path().join("trip");
    std::fs::create_dir_all(&trip).unwrap();
    let paths = write_frames(&trip, 2);
    let root = dir.path().join("history");
    let (window, app) = launch(cx, &engine, paths, &root);
    stack(cx, window, &app);
    let render = active(cx, &app).unwrap();
    wait_until(cx, "the list saved", |cx| {
        crate::test_support::library(cx, &app).read_working().unwrap().base.as_ref()
            == Some(&render)
    });
    crate::test_support::quit(cx, window, app);

    // The folder's volume as another, not mounted.
    let file = root.join(deptheos_app::frames::sources::SOURCES_FILE);
    let mut json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    for (_, at) in json["roots"].as_object_mut().unwrap() {
        at["place"]["mount"] = "/Volumes/Nowhere".into();
        at["place"]["volume"]["id"] = "uuid:00000000-0000-0000-0000-000000000000".into();
        at["place"].as_object_mut().unwrap().remove("bookmark");
    }
    std::fs::write(&file, json.to_string()).unwrap();
    let (_, app) = launch(cx, &engine, Vec::new(), &root);
    wait_until(cx, "the file check", |cx| {
        app.read_with(cx, |app, cx| app.files.read(cx).not_connected_count() == 2)
    });
    app.read_with(cx, |app, cx| {
        assert_eq!(app.files.read(cx).missing_count(), 2);
        assert_eq!(app.renders.read(cx).renders()[0].frames.len(), 2);
    });
}

/// Locate… relinks the list and every render made from a moved folder
/// (copied elsewhere, the original deleted, so no bookmark leads to it).
#[gpui::test]
fn relinking_a_moved_folder(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let (trip, moved) = (dir.path().join("trip"), dir.path().join("trip 2024"));
    std::fs::create_dir_all(&trip).unwrap();
    let paths = write_frames(&trip, 2);
    let root = dir.path().join("history");
    let (window, app) = launch(cx, &engine, paths, &root);
    stack(cx, window, &app);
    // Wait for the render's base write, so it can't land over the relinked list.
    let render = active(cx, &app).unwrap();
    wait_until(cx, "the list saved", |cx| {
        crate::test_support::library(cx, &app).read_working().unwrap().base.as_ref()
            == Some(&render)
    });
    crate::test_support::quit(cx, window, app);

    std::fs::create_dir_all(&moved).unwrap();
    for entry in std::fs::read_dir(&trip).unwrap() {
        let entry = entry.unwrap().path();
        std::fs::copy(&entry, moved.join(entry.file_name().unwrap())).unwrap();
    }
    std::fs::remove_dir_all(&trip).unwrap();
    let (window, app) = launch(cx, &engine, Vec::new(), &root);
    wait_until(cx, "the file check", |cx| {
        app.read_with(cx, |app, cx| app.files.read(cx).missing_count() == 2)
    });
    let notices = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| window.notifications(cx).len()).unwrap()
    };
    let before = notices(cx);
    act(cx, window, &app, |app, window, cx| app.relink(trip.clone(), moved.clone(), window, cx));
    wait_until(cx, "the relink", |cx| {
        app.read_with(cx, |app, cx| {
            let renders = app.renders.read(cx).renders();
            renders.len() == 1 && renders[0].frames[0].starts_with(&moved)
        })
    });
    app.read_with(cx, |app, cx| assert_eq!(app.files.read(cx).missing_count(), 0));
    assert_eq!(notices(cx), before + 1, "found 2 of 2 is said");
    // Read again from the roots: via the temp folder's link or its target.
    let stored = deptheos_app::frames::sources::canonical(&moved).unwrap();
    wait_until(cx, "the list saved", |cx| {
        let first =
            crate::test_support::library(cx, &app).read_working().unwrap().list.files[0].clone();
        first.starts_with(&stored) || first.starts_with(&moved)
    });
}

/// After a relink the render shown is reopened, so its stack and Retouch
/// read frames where they are now.
#[gpui::test]
fn a_relink_opens_the_render_shown_and_retouch_again(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let (trip, moved) = (dir.path().join("trip"), dir.path().join("trip 2024"));
    std::fs::create_dir_all(&trip).unwrap();
    let paths = write_frames(&trip, 2);
    let (window, app) = launch(cx, &engine, paths, &dir.path().join("history"));
    stack(cx, window, &app);
    act(cx, window, &app, |app, _, cx| app.toggle_paint(cx));
    app.read_with(cx, |app, _| assert!(app.is_retouching()));

    std::fs::rename(&trip, &moved).unwrap();
    act(cx, window, &app, |app, window, cx| app.relink(trip.clone(), moved.clone(), window, cx));
    wait_until(cx, "the render opened again", |cx| {
        app.read_with(cx, |app, cx| {
            let shown = app.stacking.read(cx).result().map(|s| s.paths.clone());
            shown.is_some_and(|p| p.iter().all(|p| p.starts_with(&moved)))
                && app.painting().is_some_and(|p| p.frames.iter().all(|p| p.starts_with(&moved)))
        })
    });
    app.read_with(cx, |app, cx| {
        assert_eq!(app.stacking.read(cx).frame_index(&[moved.join("frame_1.png")]), Some(1));
    });
}

/// Adobe DNG Converter is suggested once, on the first camera RAW added
/// without it, never for other images.
#[gpui::test]
fn suggests_the_converter_for_camera_raw(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    if deptheos_app::converter_executable().is_none() {
        return; // Adobe does not ship it here: never suggested.
    }
    let dir = tempfile::tempdir().unwrap();
    let pngs = write_frames(dir.path(), 2);
    let raw = dir.path().join("shot.orf");
    std::fs::write(&raw, b"not really a RAW file").unwrap();
    let (window, app) = open(cx, |window, cx| {
        cx.new(|cx| {
            AppView::new(
                engine,
                Vec::new(),
                crate::test_support::opened(&dir.path().join("history")),
                window,
                cx,
            )
        })
    });
    // Not installed, wherever the test machine has it.
    deptheos_app::set_converter_executable(Some(dir.path().join("missing converter")));
    let suggested = |cx: &mut TestAppContext| cx.update(|cx| ui_state(cx).converter_suggested);
    let add = |cx: &mut TestAppContext, paths: Vec<PathBuf>, then: &str| {
        act(cx, window, &app, |app, window, cx| app.add_paths(paths, window, cx));
        wait_until(cx, then, |cx| {
            app.read_with(cx, |app, cx| app.files.read(cx).summary().as_ref() == then)
        });
    };
    add(cx, pngs, "2 frames");
    let after_pngs = suggested(cx);
    add(cx, vec![raw], "3 frames");
    let after_raw = suggested(cx);
    deptheos_app::set_converter_executable(None);
    assert!(!after_pngs, "no camera RAW, no suggestion");
    assert!(after_raw, "suggested, and remembered so it is not again");
}

/// Images added to one stack never land in another opened while listing;
/// two quick adds to one stack are both kept.
#[gpui::test]
fn an_add_listed_after_another_stack_opened_is_dropped(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let [a, b, c, d] = ["a", "b", "c", "d"].map(|name| {
        let folder = dir.path().join(name);
        std::fs::create_dir(&folder).unwrap();
        folder
    });
    for folder in [&b, &c, &d] {
        write_frames(folder, 2);
    }
    let first = write_frames(&a, 2);
    let (window, app) = launch(cx, &engine, first, &dir.path().join("history"));
    wait_until(cx, "the images", |cx| app.read_with(cx, |app, cx| app.files.read(cx).can_stack()));
    let frames =
        |cx: &mut TestAppContext| app.read_with(cx, |app, cx| app.files.read(cx).rows().count());
    let notices = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| window.notifications(cx).len()).unwrap()
    };

    // Two adds in a row, to the same stack: both land.
    act(cx, window, &app, |app, window, cx| {
        app.add_paths(vec![b.clone()], window, cx);
        app.add_paths(vec![c.clone()], window, cx);
    });
    wait_until(cx, "both adds", |cx| frames(cx) == 6);

    // An add whose stack is left before its folder is listed.
    let before = notices(cx);
    let left = app.read_with(cx, |app, _| app.stack.clone());
    act(cx, window, &app, |app, window, cx| {
        app.add_paths(vec![d.clone()], window, cx);
        app.new_stack(window, cx);
    });
    crate::test_support::switched(cx, &app);
    wait_until(cx, "the dropped add's notice", |cx| notices(cx) > before);
    app.read_with(cx, |app, cx| {
        assert_ne!(app.stack, left);
        assert!(app.files.read(cx).list().is_empty(), "nothing was added to the new stack");
    });
}

/// A left-out frame is marked on its row with why; the inspector counts them
/// and Show selects them.
#[gpui::test]
fn left_out_frames_are_marked_and_selected(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let mut paths = write_frames(dir.path(), 3);
    let broken = dir.path().join("frame_9.png");
    std::fs::write(&broken, b"not an image").unwrap();
    paths.push(broken.clone());
    let (window, app) = launch(cx, &engine, paths, &dir.path().join("history"));
    wait_until(cx, "the frames", |cx| {
        app.read_with(cx, |app, cx| app.files.read(cx).rows().count() == 4)
    });
    stack(cx, window, &app);
    cx.run_until_parked();
    let broken_row = app.read_with(cx, |app, cx| app.files.read(cx).row_of(&broken).unwrap());
    app.read_with(cx, |app, cx| {
        assert!(app.has_result(cx), "three frames stacked");
        assert_eq!(app.files.read(cx).left_out_rows(), [broken_row]);
    });
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    assert!(vcx.debug_bounds("left-out-mark").is_some(), "its row is marked");
    act(cx, window, &app, |app, window, cx| app.show_left_out(window, cx));
    app.read_with(cx, |app, cx| {
        assert_eq!(app.files.read(cx).selected_rows(), [broken_row], "Show selects it");
    });
    // The marks follow the result as it changes, not each frame drawn.
    app.update(cx, |app, cx| app.stacking.update(cx, |s, cx| s.clear(cx)));
    app.read_with(cx, |app, cx| assert!(app.files.read(cx).left_out_rows().is_empty()));
}

/// Two or more folders over the Library get two targets: One Stack adds them,
/// A Stack per Folder opens New Batch, a drop beside both adds nothing.
/// Anything else, and anything over Stacking, is an ordinary drop.
#[gpui::test]
fn folders_dropped_on_the_library_become_one_stack_or_a_batch(cx: &mut TestAppContext) {
    use gpui::{FileDropEvent, point, px};
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (dir.path().join("ants"), dir.path().join("bees"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    write_frames(&a, 2);
    write_frames(&b, 2);
    let (window, app) = launch(cx, &engine, Vec::new(), &dir.path().join("history"));
    act(cx, window, &app, |app, window, cx| app.set_screen(Screen::Library, window, cx));
    let folders = vec![a.clone(), b.clone()];
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    // Dragged in, then dropped where `at` says.
    let drag =
        |vcx: &mut gpui::VisualTestContext,
         paths: Vec<PathBuf>,
         at: &dyn Fn(&mut gpui::VisualTestContext) -> gpui::Point<gpui::Pixels>| {
            let start = point(px(640.0), px(400.0));
            vcx.simulate_event(FileDropEvent::Entered {
                position: start,
                paths: ExternalPaths(paths.into_iter().collect()),
            });
            vcx.simulate_event(FileDropEvent::Pending { position: start });
            vcx.run_until_parked();
            let position = at(vcx);
            vcx.simulate_event(FileDropEvent::Pending { position });
            vcx.simulate_event(FileDropEvent::Submit { position });
            vcx.run_until_parked();
        };
    let target = |name: &'static str| {
        move |vcx: &mut gpui::VisualTestContext| vcx.debug_bounds(name).expect(name).center()
    };

    // Beside both targets: nothing is added.
    drag(&mut vcx, folders.clone(), &|_| point(px(640.0), px(60.0)));
    app.read_with(&vcx, |app, cx| {
        assert_eq!(app.screen, Screen::Library);
        assert_eq!(app.files.read(cx).rows().count(), 0);
        assert!(app.file_drag.is_none());
    });

    // A Stack per Folder: New Batch, with both folders in it.
    drag(&mut vcx, folders.clone(), &target("drop-batch"));
    let listed = app.read_with(&vcx, |app, cx| batch_sheet(app).map(|d| d.read(cx).folders()));
    assert_eq!(listed, Some(folders.clone()));
    assert_eq!(app.read_with(&vcx, |app, _| app.screen), Screen::Library);
    vcx.update(|window, cx| window.close_dialog(cx));
    app.update(&mut vcx, |app, _| app.set_sheet(crate::app::Sheet::None));

    // One Stack: both folders' frames in a stack, in Stacking.
    drag(&mut vcx, folders.clone(), &target("drop-one-stack"));
    wait_until(&mut vcx, "the frames", |cx| {
        app.read_with(cx, |app, cx| app.files.read(cx).rows().count() == 4)
    });
    assert_eq!(app.read_with(&vcx, |app, _| app.screen), Screen::Stacking);

    // One folder on the Library drops as ever, anywhere: a stack of it.
    act(&mut vcx, window, &app, |app, window, cx| app.set_screen(Screen::Library, window, cx));
    drag(&mut vcx, vec![a.clone()], &|_| point(px(640.0), px(60.0)));
    assert!(vcx.debug_bounds("drop-one-stack").is_none());
    wait_until(&mut vcx, "the ants", |cx| {
        app.read_with(cx, |app, cx| app.files.read(cx).rows().count() == 2)
    });
}

/// Files handed over during a run aren't added, and a notice says why.
#[gpui::test]
fn files_handed_over_during_a_run_are_refused_with_a_notice(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let paths = write_frames(dir.path(), 3);
    let more = dir.path().join("more");
    std::fs::create_dir_all(&more).unwrap();
    write_frames(&more, 2);
    let (window, app) = launch(cx, &engine, paths, &dir.path().join("history"));
    wait_until(cx, "the images", |cx| app.read_with(cx, |app, cx| app.files.read(cx).can_stack()));
    act(cx, window, &app, |app, window, cx| {
        app.run_stacking(window, cx);
        assert!(app.is_processing(cx));
        let before = window.notifications(cx).len();
        app.add_paths_from(vec![more.clone()], AddedFrom::Outside, window, cx);
        assert_eq!(window.notifications(cx).len(), before + 1, "the refusal is said");
        // An empty hand-over says nothing.
        app.add_paths_from(Vec::new(), AddedFrom::Outside, window, cx);
        assert_eq!(window.notifications(cx).len(), before + 1);
    });
    wait_until(cx, "the stack", |cx| app.read_with(cx, |app, cx| !app.is_processing(cx)));
    cx.run_until_parked();
    app.read_with(cx, |app, cx| assert_eq!(app.files.read(cx).rows().count(), 3, "not added"));
}

/// Delete and Backspace in the list exclude the selection, as X does, never
/// removing; outside the list they leave the frames alone.
#[gpui::test]
fn delete_in_the_frame_list_excludes(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else {
        return;
    };
    cx.update(|cx| cx.bind_keys(key_bindings()));
    act(cx, window, &app, |app, window, cx| app.set_screen(Screen::Stacking, window, cx));
    wait_until(cx, "the frames", |cx| {
        app.read_with(cx, |app, cx| app.files.read(cx).rows().count() == 3)
    });
    let ids: Vec<FileId> = app
        .read_with(cx, |app, cx| app.files.read(cx).included().iter().map(|(id, _)| *id).collect());
    let state = |cx: &mut TestAppContext| {
        app.read_with(cx, |app, cx| {
            let files = app.files.read(cx);
            (files.rows().count(), files.included().len())
        })
    };
    let select = |cx: &mut TestAppContext, id: FileId| {
        cx.update_window(window, |_, window, cx| {
            let files = app.read(cx).files.clone();
            files.update(cx, |f, cx| {
                f.highlight(Some(id), window, cx);
                f.focus_rows(window, cx)
            })
        })
        .unwrap();
        cx.run_until_parked();
    };

    // Outside the list, Backspace leaves the frames alone.
    act(cx, window, &app, |app, window, cx| window.focus(&app.focus_handle, cx));
    cx.simulate_keystrokes(window, "backspace");
    assert_eq!(state(cx), (3, 3));
    // In it, Backspace and Delete exclude; the rows stay.
    select(cx, ids[0]);
    cx.simulate_keystrokes(window, "backspace");
    assert_eq!(state(cx), (3, 2));
    select(cx, ids[1]);
    cx.simulate_keystrokes(window, "delete");
    assert_eq!(state(cx), (3, 1));
    // An excluded frame stays excluded.
    select(cx, ids[0]);
    cx.simulate_keystrokes(window, "delete");
    assert_eq!(state(cx), (3, 1));
}
