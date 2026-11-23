//! A stack and its renders: the result, the working version and the render shown.

use super::*;

/// The whole loop on a real engine: preview, stack, result, compare.
#[gpui::test]
fn preview_stack_and_compare(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let paths = write_frames(dir.path(), 2);
    let (window, app) = open(cx, |window, cx| {
        cx.new(|cx| {
            AppView::new(
                engine,
                paths,
                crate::test_support::opened(&dir.path().join("history")),
                window,
                cx,
            )
        })
    });

    // The first file previews on launch.
    wait_until(cx, "the preview", |cx| {
        app.read_with(cx, |app, cx| app.viewer.read(cx).has_content())
    });
    app.read_with(cx, |app, cx| {
        assert_eq!(app.files.read(cx).summary().as_ref(), "2 frames");
    });

    // Secondary-Enter stacks even with the file list (which binds it) focused.
    cx.update(|cx| cx.bind_keys(key_bindings()));
    cx.update_window(window, |_, window, cx| {
        let files = app.read(cx).files.clone();
        files.update(cx, |f, cx| f.focus_rows(window, cx))
    })
    .unwrap();
    cx.run_until_parked();
    cx.simulate_keystrokes(window, "secondary-enter");
    app.read_with(cx, |app, cx| {
        assert!(app.is_processing(cx));
        assert!(app.files.read(cx).locked(), "nothing is added while it runs");
    });
    wait_until(cx, "the stack", |cx| app.read_with(cx, |app, cx| !app.is_processing(cx)));
    app.read_with(cx, |app, cx| {
        assert!(app.has_result(cx), "and left a result");
        let viewer = app.viewer.read(cx);
        assert!(viewer.has_content() && !viewer.has_compare());
        assert_eq!(viewer.mode(), ViewMode::Result);
    });

    // M shows the depth map, M again the result.
    cx.simulate_keystrokes(window, "m");
    app.read_with(cx, |app, cx| assert!(app.viewer.read(cx).mode().is_map()));
    cx.simulate_keystrokes(window, "m");
    app.read_with(cx, |app, cx| assert_eq!(app.viewer.read(cx).mode(), ViewMode::Result));

    // The view bar's Compare (a click on the bar reaches it, not the photo)
    // turns on the last layout (Split first), loading a source frame, and
    // off again. The home control, rail, layouts menu and lenses appear
    // with a result.
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    for part in ["view-home", "focus-rail", "view-layouts", "contribution"] {
        assert!(vcx.debug_bounds(part).is_some(), "{part} is in the view bar");
    }
    assert!(vcx.debug_bounds("view-compare").is_some(), "Compare is in the view bar");
    // Compare waits for the render to be saved; then, its notice slid away, it is clicked.
    wait_until(cx, "the render saved", |cx| {
        app.read_with(cx, |app, cx| !app.renders.read(cx).is_saving())
    });
    click(cx, window, "view-compare");
    app.read_with(cx, |app, cx| assert_eq!(app.viewer.read(cx).mode().look(), Some(Look::Split)));
    wait_until(cx, "the compare source", |cx| {
        app.read_with(cx, |app, cx| !app.viewer.read(cx).is_loading())
    });
    click(cx, window, "view-compare");
    app.read_with(cx, |app, cx| assert_eq!(app.viewer.read(cx).mode(), ViewMode::Result));

    // A compare mode with nothing selected loads the stack's first frame.
    act(cx, window, &app, |app, window, cx| app.set_view(Look::SideBySide, window, cx));
    wait_until(cx, "the compare source", |cx| {
        app.read_with(cx, |app, cx| !app.viewer.read(cx).is_loading())
    });
    app.read_with(cx, |app, cx| {
        let viewer = app.viewer.read(cx);
        assert!(viewer.has_compare());
        assert_eq!(viewer.mode().look(), Some(Look::SideBySide));
        let files = app.files.read(cx);
        let first = app.stacking.read(cx).first_frame().and_then(|p| files.row_of(p));
        assert_eq!(files.selected(cx), first);
    });

    // PgDn steps to the next frame as the source, compared as it was.
    cx.simulate_keystrokes(window, "pagedown");
    wait_until(cx, "the next frame", |cx| {
        app.read_with(cx, |app, cx| !app.viewer.read(cx).is_loading())
    });
    app.read_with(cx, |app, cx| {
        let files = app.files.read(cx);
        let second = files.rows().nth(1).map(|(id, _)| id);
        let viewer = app.viewer.read(cx);
        assert_eq!(viewer.shown_file(), second);
        assert_eq!(viewer.mode().look(), Some(Look::SideBySide), "the comparison stays");
    });

    // ← steps back to the first frame, and → on to the second again.
    for (key, nth) in [("left", 0), ("right", 1)] {
        cx.simulate_keystrokes(window, key);
        wait_until(cx, "the frame", |cx| {
            app.read_with(cx, |app, cx| !app.viewer.read(cx).is_loading())
        });
        app.read_with(cx, |app, cx| {
            let files = app.files.read(cx);
            let frame = files.rows().nth(nth).map(|(id, _)| id);
            assert_eq!(app.viewer.read(cx).shown_file(), frame, "{key}");
        });
    }

    // With the result alone, a frame picked in the list shows alone, not
    // compared.
    act(cx, window, &app, |app, window, cx| app.set_view(Look::Result, window, cx));
    app.read_with(cx, |app, cx| assert_eq!(app.viewer.read(cx).mode(), ViewMode::Result));
    let first = app.read_with(cx, |app, cx| app.files.read(cx).rows().next().unwrap().0);
    act(cx, window, &app, |app, window, cx| app.show_file(first, window, cx));
    wait_until(cx, "the frame", |cx| {
        app.read_with(cx, |app, cx| !app.viewer.read(cx).is_loading())
    });
    app.read_with(cx, |app, cx| {
        let viewer = app.viewer.read(cx);
        assert_eq!(viewer.shown_file(), Some(first));
        assert!(matches!(viewer.mode(), ViewMode::Frame { .. }), "alone");
    });

    // Tab hides the panels around the photo, and brings them back.
    cx.simulate_keystrokes(window, "tab");
    app.read_with(cx, |app, _| assert!(app.chrome.is_focus()));
    cx.simulate_keystrokes(window, "tab");
    app.read_with(cx, |app, _| assert!(!app.chrome.is_focus()));
}

/// New Stack writes the switch on the store thread: until written the stack
/// stays and nothing starts over it; then the new stack is on screen and in
/// the list file.
#[gpui::test]
fn new_stack_switches_once_the_store_has_written_it(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    stack(cx, window, &app);
    let first = app.read_with(cx, |app, _| app.stack.clone());

    // The store thread held back, so the switch can be seen in flight.
    let (release, held) = std::sync::mpsc::channel::<()>();
    let _hold = deptheos_app::engine::store::store().write(move || held.recv().ok());
    act(cx, window, &app, |app, window, cx| {
        app.new_stack(window, cx);
        assert!(app.is_switching());
        assert_eq!(app.stack, first, "the screen changes once it is written");
        assert!(app.is_processing(cx), "nothing starts over it");
        assert!(!app.can_stack(cx));
    });
    release.send(()).unwrap();
    crate::test_support::switched(cx, &app);
    let second = app.read_with(cx, |app, cx| {
        assert!(app.files.read(cx).list().is_empty(), "the new stack is empty");
        app.stack.clone()
    });
    assert_ne!(second, first);
    deptheos_app::engine::store::store().flush(crate::test_support::STORE_DRAINS);
    assert_eq!(crate::test_support::library(cx, &app).read_working().unwrap().stack, second);
}

/// A strip click during a stack switch waits for it: nothing of the leaving
/// stack lands over the coming one.
#[gpui::test]
fn a_strip_click_during_a_switch_waits_for_it(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    stack(cx, window, &app);
    let first = active(cx, &app).unwrap();
    exclude_first(cx, &app);
    stack(cx, window, &app);
    assert_ne!(active(cx, &app).as_ref(), Some(&first));
    let leaving = app.read_with(cx, |app, _| app.stack.clone());

    let (release, held) = std::sync::mpsc::channel::<()>();
    let _hold = deptheos_app::engine::store::store().write(move || held.recv().ok());
    act(cx, window, &app, |app, window, cx| {
        app.new_stack(window, cx);
        assert!(app.is_switching());
        app.renders.update(cx, |r, cx| r.click_tile(&first, window, cx));
        assert!(!app.renders.read(cx).is_pending(), "the click waits for the switch");
    });
    release.send(()).unwrap();
    crate::test_support::switched(cx, &app);
    cx.run_until_parked();
    app.read_with(cx, |app, cx| {
        assert_ne!(app.stack, leaving, "the new stack is on screen");
        assert!(app.stacking.read(cx).shown_render().is_none(), "no render of the stack left");
        assert!(!app.renders.read(cx).is_locked(), "the strip takes clicks again");
    });
}

/// While something runs the strip opens nothing; after, a click opens again.
#[gpui::test]
fn the_strip_is_locked_while_something_runs(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    stack(cx, window, &app);
    let first = active(cx, &app).unwrap();
    exclude_first(cx, &app);
    stack(cx, window, &app);
    act(cx, window, &app, |app, window, cx| {
        let awake = crate::app::run::Awake::new("a test", cx);
        app.set_running(Running::Sequence { _awake: awake }, cx);
        assert!(app.renders.read(cx).is_locked());
        app.renders.update(cx, |r, cx| r.click_tile(&first, window, cx));
        assert!(!app.renders.read(cx).is_pending(), "no open while it runs");
        app.set_running(Running::Idle, cx);
        assert!(!app.renders.read(cx).is_locked());
        app.renders.update(cx, |r, cx| r.click_tile(&first, window, cx));
        assert!(app.renders.read(cx).is_pending(), "it opens once the run ends");
    });
    crate::test_support::wait_until(cx, "the render opened", |cx| {
        active(cx, &app).as_ref() == Some(&first)
    });
}

/// A new stack takes the render off screen as it starts (strip and paint
/// don't name it); a cancel shows it again.
#[gpui::test]
fn a_new_stack_takes_the_render_off_screen(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    stack(cx, window, &app);
    let first = active(cx, &app).expect("the stack is kept and shown as its render");

    act(cx, window, &app, |app, window, cx| {
        app.run_stacking(window, cx);
        assert!(app.is_processing(cx));
        assert_eq!(app.renders.read(cx).active(cx), None, "nothing is shown while it runs");
        assert!(app.result_doc(cx).is_none());
        assert!(app.can_cancel(cx));
        app.cancel_run(window, cx);
        // Canceling at once, and Cancel can't be asked twice.
        assert_eq!(app.stacking.read(cx).phase(), Some(crate::stacking::Phase::Canceling));
        assert!(!app.can_cancel(cx));
    });
    wait_until(cx, "the run to end", |cx| {
        app.read_with(cx, |app, cx| !app.is_processing(cx) && !app.renders.read(cx).is_saving())
    });
    // Canceled, the first render is back; done before the cancel, the new one.
    let made = app.read_with(cx, |app, cx| app.renders.read(cx).renders().len());
    if made == 1 {
        wait_until(cx, "the first render again", |cx| active(cx, &app) == Some(first.clone()));
    } else {
        assert_ne!(active(cx, &app), Some(first));
    }
}

/// A canceled stack's live map gives way to what was shown before: the
/// first frame's preview, as no render was made.
#[gpui::test]
fn a_canceled_stack_shows_the_frame_it_replaced(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    wait_until(cx, "the preview", |cx| {
        app.read_with(cx, |app, cx| {
            let viewer = app.viewer.read(cx);
            viewer.has_content() && !viewer.is_loading()
        })
    });
    let previewed = app.read_with(cx, |app, cx| app.viewer.read(cx).shown_file());
    assert!(previewed.is_some(), "a frame is previewed");
    act(cx, window, &app, |app, window, cx| {
        app.run_stacking(window, cx);
        assert_eq!(app.viewer.read(cx).shown_file(), None, "the run takes the viewer over");
        app.cancel_run(window, cx);
    });
    wait_until(cx, "the run to end", |cx| {
        app.read_with(cx, |app, cx| {
            !app.is_processing(cx)
                && !app.renders.read(cx).is_saving()
                && !app.viewer.read(cx).is_loading()
        })
    });
    if app.read_with(cx, |app, cx| app.renders.read(cx).renders().is_empty()) {
        app.read_with(cx, |app, cx| {
            let viewer = app.viewer.read(cx);
            assert_eq!(viewer.shown_file(), previewed, "the frame is shown again");
            assert!(viewer.has_content());
            assert!(!viewer.has_map(), "and the live map is gone");
        });
    }
}

/// Stack adds to the stack on screen; Clear starts a new one; opening a
/// render puts its stack on screen for the next Stack. Stack and base are
/// kept for the next launch.
#[gpui::test]
fn stacks_number_their_renders(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let paths = write_frames(dir.path(), 3);
    let root = dir.path().join("history");
    let (window, app) = launch(cx, &engine, paths.clone(), &root);
    let place = |cx: &mut TestAppContext, id: &RenderId| {
        app.read_with(cx, |app, cx| {
            let render = app.renders.read(cx).find(id).unwrap();
            (render.stack.clone(), render.number)
        })
    };

    stack(cx, window, &app);
    let first = active(cx, &app).unwrap();
    stack(cx, window, &app);
    let second = active(cx, &app).unwrap();
    let (bee, _) = place(cx, &first);
    assert_eq!(place(cx, &second), (bee.clone(), 2));
    app.read_with(cx, |app, _| {
        assert_eq!(app.stack, bee);
        assert_eq!(app.base.as_ref(), Some(&second), "the newest render is the base");
    });

    // Removing every frame stays in the stack, with its base.
    act(cx, window, &app, |app, window, cx| app.files.update(cx, |f, cx| f.clear(window, cx)));
    app.read_with(cx, |app, _| {
        assert_eq!(app.stack, bee);
        assert_eq!(app.base.as_ref(), Some(&second));
    });
    revert_and_confirm(cx, window, &app);
    app.read_with(cx, |app, cx| {
        assert_eq!(app.files.read(cx).summary().as_ref(), "3 frames", "Revert brings them back");
    });

    // New Stack, add the frames again: new work, #1 of a new stack.
    act(cx, window, &app, |app, window, cx| app.new_stack(window, cx));
    crate::test_support::switched(cx, &app);
    app.read_with(cx, |app, _| assert!(app.stack != bee && app.base.is_none()));
    act(cx, window, &app, |app, window, cx| app.add_paths(paths.clone(), window, cx));
    wait_until(cx, "the frames", |cx| {
        app.read_with(cx, |app, cx| app.files.read(cx).summary().as_ref() == "3 frames")
    });
    stack(cx, window, &app);
    let other = active(cx, &app).unwrap();
    let (fresh, number) = place(cx, &other);
    assert!(fresh != bee && number == 1);

    // Opening #1 brings its stack back, based on #2: the next Stack is #3.
    open_render(cx, window, &app, &first);
    app.read_with(cx, |app, _| {
        assert_eq!((&app.stack, app.base.as_ref()), (&bee, Some(&second)));
    });
    assert_eq!(active(cx, &app), Some(first.clone()), "#1 is shown");
    stack(cx, window, &app);
    let third = active(cx, &app).unwrap();
    assert_eq!(place(cx, &third), (bee.clone(), 3));

    // Kept for the next launch.
    wait_until(cx, "the list saved", |cx| {
        let working = crate::test_support::library(cx, &app).read_working().unwrap();
        working.stack == bee && working.base.as_ref() == Some(&third)
    });
    crate::test_support::quit(cx, window, app);
    let (_, relaunched) = launch(cx, &engine, Vec::new(), &root);
    relaunched.read_with(cx, |app, _| {
        assert_eq!((&app.stack, app.base.as_ref()), (&bee, Some(&third)));
    });
}

/// An open replaced before it lands is dropped (with its pool request), and
/// the last asked for is shown.
#[gpui::test]
fn a_replaced_render_open_stops(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    stack(cx, window, &app);
    let first = active(cx, &app).unwrap();
    stack(cx, window, &app);
    let second = active(cx, &app).unwrap();
    open_render(cx, window, &app, &first);
    act(cx, window, &app, |app, window, cx| {
        app.renders.update(cx, |r, cx| {
            r.open(&second, window, cx);
            r.open(&first, window, cx);
        })
    });
    wait_until(cx, "the last open", |cx| {
        app.read_with(cx, |app, cx| {
            let renders = app.renders.read(cx);
            !renders.is_pending() && renders.active(cx) == Some(&first)
        })
    });
}

/// The history end to end: each stack is kept with list and settings;
/// clicking a render shows it, leaving the working version editable, with
/// title and change line comparing them; a relaunch lists every render.
#[gpui::test]
fn showing_a_render_leaves_the_working_version_alone(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (dir.path().join("a"), dir.path().join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let paths = write_frames(&a, 3);
    // Folder `b` is added whole, further down.
    write_frames(&b, 2);
    let root = dir.path().join("history");
    let (window, app) = launch(cx, &engine, paths.clone(), &root);
    let summary =
        |cx: &mut TestAppContext| app.read_with(cx, |app, cx| app.files.read(cx).summary());
    let depth_shown = |cx: &mut TestAppContext| {
        app.read_with(cx, |app, cx| {
            matches!(app.params.read(cx).stacker_config(cx), StackerConfig::Depth(_))
        })
    };

    // All three, then two of them with the depth stacker.
    stack(cx, window, &app);
    let first = active(cx, &app).expect("the stack is kept and shown as its render");
    exclude_first(cx, &app);
    app.read_with(cx, |app, cx| assert_eq!(app.files.read(cx).stack_frames(), paths[1..]));
    let depth = StackerConfig::Depth(DepthConfig { radius: 6.0, smoothing: 2.0, blend_range: 3.0 });
    act(cx, window, &app, |app, window, cx| {
        app.params.update(cx, |p, cx| p.restore(&depth, &SessionOptions::default(), window, cx))
    });
    stack(cx, window, &app);
    let second = active(cx, &app).unwrap();
    assert_ne!(first, second);
    app.read_with(cx, |app, cx| {
        let renders = app.renders.read(cx).renders();
        assert_eq!(renders.len(), 2);
        assert_eq!(renders[1].frames, paths, "the whole list is recorded");
        assert_eq!(renders[1].included, [false, true, true]);
        assert!(matches!(renders[1].config, StackerConfig::Depth(_)));
        // One line of work: #1 and #2 of one stack.
        assert_eq!(renders[0].stack, renders[1].stack);
        assert_eq!((renders[0].number, renders[1].number), (1, 2));
    });

    // Another folder added: both listed, settings stay, the base's tile says
    // how much changed.
    act(cx, window, &app, |app, window, cx| app.add_paths(vec![b.clone()], window, cx));
    wait_until(cx, "the folder", |cx| summary(cx).as_ref() == "4 of 5 frames");
    assert_eq!(active(cx, &app).as_ref(), Some(&second), "the render shown stays shown");
    assert!(depth_shown(cx));
    app.read_with(cx, |app, cx| {
        let line = app.snapshot(cx).showing().base_tile_line(crate::words::of(cx));
        assert_eq!(line.as_deref(), Some("2 changes"));
    });

    // Clicking the first render shows it; list and settings stay the working
    // version's and say how they differ.
    open_render(cx, window, &app, &first);
    assert_eq!(summary(cx).as_ref(), "4 of 5 frames");
    assert!(depth_shown(cx));
    app.read_with(cx, |app, cx| {
        assert!(app.has_result(cx));
        assert_eq!(app.title_parts(cx).what.as_deref(), Some("#1 · Pyramid · 3 frames"));
        assert_eq!(app.title_parts(cx).note, Some(TitleNote::Edited("Edited".into())));
        let line = app.compared(cx).expect("the working version differs from #1");
        assert!(!line.edited && line.revert == Some(1), "{line:?}");
    });
    // The list can still change.
    exclude_first(cx, &app);
    assert_eq!(summary(cx).as_ref(), "3 of 5 frames");

    // The second: shown, the list still the working version's.
    open_render(cx, window, &app, &second);
    assert_eq!(summary(cx).as_ref(), "3 of 5 frames");
    assert!(depth_shown(cx));
    app.read_with(cx, |app, cx| {
        // Its frames are found by path in the list.
        let files = app.files.read(cx);
        let stacking = app.stacking.read(cx);
        let ids: Vec<FileId> = files.rows().map(|(id, _)| id).collect();
        assert_eq!(stacking.frame_index(files.shot_files(ids[1]).unwrap()), Some(0));
    });

    // It is kept: a relaunch shows it, and the history.
    wait_until(cx, "the list saved", |cx| {
        let Working { list, settings, .. } =
            crate::test_support::library(cx, &app).read_working().unwrap();
        list.files.len() == 5
            && list.excluded.len() == 2
            && matches!(settings, Some(Settings { config: StackerConfig::Depth(_), .. }))
    });
    crate::test_support::quit(cx, window, app);
    let (_, relaunched) = launch(cx, &engine, Vec::new(), &root);
    relaunched.read_with(cx, |app, cx| {
        assert_eq!(app.screen, Screen::Library, "the Library is home");
        assert_eq!(app.renders.read(cx).renders().len(), 2);
        let files = app.files.read(cx);
        assert_eq!(files.summary().as_ref(), "3 of 5 frames", "the stack left open is kept");
        assert!(matches!(app.params.read(cx).stacker_config(cx), StackerConfig::Depth(_)));
    });
}

/// Edits survive opening another stack's render and coming back. Revert
/// would lose them, so it asks: Esc keeps them, Revert restores the render's
/// frames. Stack stacks the working version, whatever is shown.
#[gpui::test]
fn the_working_version_survives_opening_another_stack(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (dir.path().join("a"), dir.path().join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let ants = write_frames(&a, 3);
    let bees = write_frames(&b, 2);
    let root = dir.path().join("history");
    let (window, app) = launch(cx, &engine, bees, &root);
    cx.update(|cx| cx.bind_keys(key_bindings()));
    let summary =
        |cx: &mut TestAppContext| app.read_with(cx, |app, cx| app.files.read(cx).summary());
    let note = |cx: &mut TestAppContext| app.read_with(cx, |app, cx| app.title_parts(cx).note);
    let in_window =
        |cx: &mut TestAppContext, f: &dyn Fn(&mut AppView, &mut Window, &mut Context<AppView>)| {
            act(cx, window, &app, |app, window, cx| f(app, window, cx));
        };

    // Bee #1, then Ant #1 in a new stack, then an edit to Ant.
    stack(cx, window, &app);
    let bee = active(cx, &app).unwrap();
    in_window(cx, &|app, window, cx| app.new_stack(window, cx));
    crate::test_support::switched(cx, &app);
    in_window(cx, &|app, window, cx| app.add_paths(ants.clone(), window, cx));
    wait_until(cx, "the ants", |cx| summary(cx).as_ref() == "3 frames");
    stack(cx, window, &app);
    let ant = active(cx, &app).unwrap();
    exclude_first(cx, &app);
    assert_eq!(summary(cx).as_ref(), "2 of 3 frames");

    // Bee: its working version, unedited.
    open_render(cx, window, &app, &bee);
    assert_eq!(summary(cx).as_ref(), "2 frames");
    assert_eq!(note(cx), None);

    // Back to Ant: #1 shown, the edit kept.
    open_render(cx, window, &app, &ant);
    assert_eq!(summary(cx).as_ref(), "2 of 3 frames");
    assert_eq!(note(cx), Some(TitleNote::Edited("Edited".into())));

    // Revert asks; Esc keeps the edit.
    in_window(cx, &|app, window, cx| app.revert_shown(window, cx));
    assert!(asking(cx, window));
    cx.simulate_keystrokes(window, "escape");
    cx.run_until_parked();
    assert!(!asking(cx, window));
    assert_eq!(summary(cx).as_ref(), "2 of 3 frames");

    // Return presses Cancel, the default: still kept.
    in_window(cx, &|app, window, cx| app.revert_shown(window, cx));
    assert!(asking(cx, window));
    cx.simulate_keystrokes(window, "enter");
    cx.run_until_parked();
    assert!(!asking(cx, window));
    assert_eq!(summary(cx).as_ref(), "2 of 3 frames");

    // Revert, confirmed: #1's list, unedited.
    revert_and_confirm(cx, window, &app);
    assert_eq!(summary(cx).as_ref(), "3 frames");
    assert_eq!(note(cx), None);
    assert!(app.read_with(cx, |app, cx| app.compared(cx).is_none()));

    // Stack stacks the working version: #2.
    exclude_first(cx, &app);
    stack(cx, window, &app);
    let second = active(cx, &app).unwrap();
    app.read_with(cx, |app, cx| {
        let render = app.renders.read(cx).find(&second).unwrap();
        assert_eq!((render.number, render.included.as_slice()), (2, &[false, true, true][..]));
        assert_eq!(app.base.as_ref(), Some(&second));
        assert!(!app.snapshot(cx).showing().is_edited());
    });
}

/// Revert runs at once when nothing unstacked would be lost. The change line
/// offers Revert only when it would change something.
#[gpui::test]
fn revert_runs_at_once_when_nothing_is_lost(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    let summary =
        |cx: &mut TestAppContext| app.read_with(cx, |app, cx| app.files.read(cx).summary());
    let line = |cx: &mut TestAppContext| {
        app.read_with(cx, |app, cx| app.compared(cx).map(|l| (l.text.to_string(), l.revert)))
    };

    // #1 of three, #2 of two: the working version is #2's, unedited.
    stack(cx, window, &app);
    let first = active(cx, &app).unwrap();
    exclude_first(cx, &app);
    stack(cx, window, &app);
    let second = active(cx, &app).unwrap();
    assert_eq!(line(cx), None, "the unedited base: no line");

    // #1 shown: how the working version differs from it, with Revert.
    open_render(cx, window, &app, &first);
    assert_eq!(line(cx), Some(("1 difference from #1 in the frames".into(), Some(1))));

    // Revert runs at once: #1's frames, #1 the base.
    act(cx, window, &app, |app, window, cx| app.revert_shown(window, cx));
    assert!(!asking(cx, window), "nothing unstacked to lose: no question");
    assert_eq!(summary(cx).as_ref(), "3 frames");
    app.read_with(cx, |app, _| assert_eq!(app.base.as_ref(), Some(&first)));
    assert_eq!(line(cx), None);

    // #2 shown, the working version made #2's again by hand: no Revert.
    open_render(cx, window, &app, &second);
    assert_eq!(line(cx), Some(("1 difference from #2 in the frames".into(), Some(2))));
    exclude_first(cx, &app);
    assert_eq!(line(cx), Some(("Same frames and settings as #2".into(), None)));
    let menu = app.read_with(cx, |app, cx| app.menu_state(cx));
    assert!(matches!(menu.revert, RevertItem::Revert { number: 2, enabled: false }));
}

/// A setting is written as it changes, as the list is, surviving a crash.
#[gpui::test]
fn a_changed_setting_is_written_as_it_changes(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 2) else { return };
    let store = app.read_with(cx, |app, cx| app.renders.read(cx).store().clone());
    let depth = StackerConfig::Depth(DepthConfig { smoothing: 7.0, ..DepthConfig::default() });
    let written = || match store.read_working().unwrap().settings {
        Some(Settings { config: StackerConfig::Depth(depth), .. }) => Some(depth.smoothing),
        _ => None,
    };
    wait_until(cx, "the list written", |_| !store.read_working().unwrap().list.is_empty());
    assert_ne!(written(), Some(7.0));

    act(cx, window, &app, |app, window, cx| {
        let options = app.params.read(cx).session_options(cx);
        app.params.update(cx, |p, cx| p.restore(&depth, &options, window, cx));
    });
    wait_until(cx, "the setting written", |_| written() == Some(7.0));
}

/// The title names the stack and render shown and marks edits; the settings
/// panel counts changes; Revert restores the render's frames and settings.
#[gpui::test]
fn the_title_and_inspector_follow_edits_and_revert(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let bee = dir.path().join("bee");
    std::fs::create_dir_all(&bee).unwrap();
    let paths = write_frames(&bee, 3);
    let (window, app) = launch(cx, &engine, paths, &dir.path().join("history"));
    let title = |cx: &mut TestAppContext| {
        app.read_with(cx, |app, cx| {
            let t = app.title_parts(cx);
            (t.name, t.what, t.note)
        })
    };
    let line = |cx: &mut TestAppContext| app.read_with(cx, |app, cx| app.compared(cx));
    let what = |text: &str| Some(text.to_string());

    wait_until(cx, "the frames", |cx| app.read_with(cx, |app, cx| app.files.read(cx).can_stack()));
    assert_eq!(
        title(cx),
        (
            what("bee"),
            what("Pyramid · 3 frames"),
            Some(TitleNote::NotStacked("Not stacked yet".into()))
        )
    );
    stack(cx, window, &app);
    let first = active(cx, &app).unwrap();
    assert_eq!(title(cx), (what("bee"), what("#1 · Pyramid · 3 frames"), None));
    assert_eq!(line(cx), None);

    // One frame left out and the depth stacker: two changes; the title still
    // names the render shown.
    exclude_first(cx, &app);
    let depth = StackerConfig::Depth(DepthConfig::default());
    act(cx, window, &app, |app, window, cx| {
        app.params.update(cx, |p, cx| p.restore(&depth, &SessionOptions::default(), window, cx))
    });
    assert_eq!(
        title(cx),
        (what("bee"), what("#1 · Pyramid · 3 frames"), Some(TitleNote::Edited("Edited".into())))
    );
    assert_eq!(
        line(cx),
        Some(crate::params::ChangeLine {
            text: "1 change to the settings and 1 to the frames since #1".into(),
            edited: true,
            revert: Some(1),
            retouching: false,
        })
    );

    // Revert: #1's frames and settings, nothing edited.
    revert_and_confirm(cx, window, &app);
    assert_eq!(title(cx).2, None);
    assert_eq!(line(cx), None);
    app.read_with(cx, |app, cx| {
        assert_eq!(app.files.read(cx).summary().as_ref(), "3 frames");
        assert!(matches!(app.params.read(cx).stacker_config(cx), StackerConfig::Pyramid(_)));
    });

    // #2 of two frames, then #1 shown: the title names #1, the working
    // version is #2's unedited, and the line compares it to #1.
    exclude_first(cx, &app);
    stack(cx, window, &app);
    assert_eq!(title(cx).1, what("#2 · Pyramid · 2 of 3 frames"));
    open_render(cx, window, &app, &first);
    assert_eq!(title(cx), (what("bee"), what("#1 · Pyramid · 3 frames"), None));
    assert_eq!(
        line(cx),
        Some(crate::params::ChangeLine {
            text: "1 difference from #1 in the frames".into(),
            edited: false,
            revert: Some(1),
            retouching: false,
        })
    );
}

/// Render Info opens on the render shown (⌘I, its tile, or over the photo
/// in Focus Mode) and marks rows the working version differs in; the
/// Library closes it.
#[gpui::test]
fn render_info_shows_the_record_of_the_render_shown(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    cx.update(|cx| cx.bind_keys(key_bindings()));
    let info =
        |cx: &mut TestAppContext| app.read_with(cx, |app, cx| app.renders.read(cx).info().cloned());
    let drawn = |cx: &mut TestAppContext, selector: &'static str| {
        cx.update_window(window, |_, window, _| window.refresh()).unwrap();
        cx.run_until_parked();
        let mut vcx = gpui::VisualTestContext::from_window(window, cx);
        vcx.debug_bounds(selector).is_some()
    };
    wait_until(cx, "the frames", |cx| app.read_with(cx, |app, cx| app.files.read(cx).can_stack()));
    act(cx, window, &app, |app, window, cx| window.focus(&app.focus_handle, cx));
    assert!(!app.read_with(cx, |app, cx| app.menu_state(cx).info), "no render: nothing to show");
    stack(cx, window, &app);
    let first = active(cx, &app).unwrap();
    exclude_first(cx, &app);
    assert!(app.read_with(cx, |app, cx| app.menu_state(cx).info));
    assert!(!drawn(cx, "render-info"), "only when asked");

    // ⌘I: on its tile, with its rows.
    act(cx, window, &app, |app, window, cx| window.focus(&app.focus_handle, cx));
    cx.simulate_keystrokes(window, "secondary-i");
    assert_eq!(info(cx), Some(first.clone()));
    assert!(drawn(cx, "render-info"));
    assert!(drawn(cx, "render-info-title"));

    // Focus Mode hides the strip: it is over the photo.
    act(cx, window, &app, |app, window, cx| app.toggle_focus_mode(window, cx));
    assert!(drawn(cx, "render-info"));
    act(cx, window, &app, |app, window, cx| app.toggle_focus_mode(window, cx));

    // The Library closes it.
    act(cx, window, &app, |app, window, cx| app.set_screen(Screen::Library, window, cx));
    assert_eq!(info(cx), None);
    act(cx, window, &app, |app, window, cx| app.set_screen(Screen::Stacking, window, cx));
    assert!(!drawn(cx, "render-info"));

    // What made #1, with the frame the working version left out marked.
    let sections = app.read_with(cx, |app, cx| {
        let now = app.snapshot(cx);
        let render = app.renders.read(cx).find(&first).unwrap().clone();
        deptheos_app::library::stacks::render_setup(
            &render,
            &now.working,
            now.compared,
            crate::words::of(cx),
        )
    });
    assert!(!sections.is_empty());
    assert!(sections.iter().flat_map(|s| &s.rows).any(|r| r.differs()), "{sections:?}");
}

/// A save landing after the result left the screen is kept but not made active.
#[gpui::test]
fn a_late_save_does_not_take_over(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    act(cx, window, &app, |app, window, cx| app.run_stacking(window, cx));
    wait_until(cx, "the stack", |cx| app.read_with(cx, |app, cx| !app.is_processing(cx)));
    act(cx, window, &app, |app, window, cx| app.forget_result(window, cx));
    wait_until(cx, "the save", |cx| app.read_with(cx, |app, cx| !app.renders.read(cx).is_saving()));
    assert_eq!(active(cx, &app), None);
    app.read_with(cx, |app, cx| assert_eq!(app.renders.read(cx).renders().len(), 1));
}

/// Deleting the base on screen: the newest render left becomes base and is
/// shown; the deleted number is not reused.
#[gpui::test]
fn deleting_the_base_render_falls_back_to_the_newest_left(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    stack(cx, window, &app);
    let first = active(cx, &app).unwrap();
    exclude_first(cx, &app);
    stack(cx, window, &app);
    let second = active(cx, &app).unwrap();
    app.read_with(cx, |app, _| assert_eq!(app.base, Some(second.clone())));

    act(cx, window, &app, |app, window, cx| {
        app.renders.update(cx, |r, cx| r.delete(vec![second], window, cx))
    });
    wait_until(cx, "#1 shown", |cx| active(cx, &app).as_ref() == Some(&first));
    app.read_with(cx, |app, _| assert_eq!(app.base, Some(first.clone())));

    stack(cx, window, &app);
    let numbers: Vec<u32> = app
        .read_with(cx, |app, cx| app.renders.read(cx).renders().iter().map(|r| r.number).collect());
    assert_eq!(numbers, [1, 3]);
}

/// Two stacks through a user's session (another stack's render opened, a
/// switch back, an off-screen render deleted), then a relaunch: every save
/// is there, deletes are not.
#[gpui::test]
fn a_session_across_two_stacks_is_kept(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (dir.path().join("ants"), dir.path().join("bees"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let ants = write_frames(&a, 3);
    let bees = write_frames(&b, 2);
    let root = dir.path().join("history");
    let (window, app) = launch(cx, &engine, bees, &root);
    cx.update(|cx| cx.bind_keys(key_bindings()));
    let summary =
        |cx: &mut TestAppContext| app.read_with(cx, |app, cx| app.files.read(cx).summary());
    let in_window =
        |cx: &mut TestAppContext, f: &dyn Fn(&mut AppView, &mut Window, &mut Context<AppView>)| {
            act(cx, window, &app, |app, window, cx| f(app, window, cx));
        };

    // bees #1 and #2; ants #1 in a new stack, then edited.
    stack(cx, window, &app);
    let bee1 = active(cx, &app).unwrap();
    stack(cx, window, &app);
    let bee2 = active(cx, &app).unwrap();
    let bees_stack = app.read_with(cx, |app, _| app.stack.clone());
    in_window(cx, &|app, window, cx| app.new_stack(window, cx));
    crate::test_support::switched(cx, &app);
    in_window(cx, &|app, window, cx| app.add_paths(ants.clone(), window, cx));
    wait_until(cx, "the ants", |cx| summary(cx).as_ref() == "3 frames");
    stack(cx, window, &app);
    let ant1 = active(cx, &app).unwrap();
    let ants_stack = app.read_with(cx, |app, _| app.stack.clone());
    exclude_first(cx, &app);

    // The other stack's render opened: bees on screen, #1 over its working
    // version based on #2.
    open_render(cx, window, &app, &bee1);
    app.read_with(cx, |app, _| {
        assert_eq!((&app.stack, app.base.as_ref()), (&bees_stack, Some(&bee2)));
    });
    assert_eq!(summary(cx).as_ref(), "2 frames");

    // Back to ants: its edit kept.
    open_render(cx, window, &app, &ant1);
    assert_eq!(app.read_with(cx, |app, _| app.stack.clone()), ants_stack);
    assert_eq!(summary(cx).as_ref(), "2 of 3 frames");

    // bees #2, an off-screen stack's base, deleted: ants stays as is.
    let doomed = bee2.clone();
    in_window(cx, &move |app, window, cx| {
        app.renders.update(cx, |r, cx| r.delete(vec![doomed.clone()], window, cx))
    });
    wait_until(cx, "the delete", |cx| {
        app.read_with(cx, |app, cx| app.renders.read(cx).find(&bee2).is_none())
    });
    assert_eq!(active(cx, &app), Some(ant1.clone()));
    assert_eq!(summary(cx).as_ref(), "2 of 3 frames");

    // All saved: the Library lists what is left, and the open stack's working
    // version is the edited one.
    wait_until(cx, "the Library saved", |cx| {
        let history = crate::test_support::library(cx, &app);
        let mut left: Vec<RenderId> = history.list().unwrap().into_iter().map(|r| r.id).collect();
        left.sort();
        let mut want = vec![bee1.clone(), ant1.clone()];
        want.sort();
        let working = history.read_working().unwrap();
        left == want && working.stack == ants_stack && working.list.excluded.len() == 1
    });

    // A relaunch: the Library, with ants open in Stacking as left.
    crate::test_support::quit(cx, window, app);
    let (window, relaunched) = launch(cx, &engine, Vec::new(), &root);
    let summary =
        |cx: &mut TestAppContext| relaunched.read_with(cx, |app, cx| app.files.read(cx).summary());
    relaunched.read_with(cx, |app, cx| {
        assert_eq!(app.screen, Screen::Library, "the Library is home");
        let renders = app.renders.read(cx);
        assert!(renders.find(&bee2).is_none(), "the deleted render is gone");
        assert!(renders.find(&bee1).is_some() && renders.find(&ant1).is_some());
        assert_eq!(app.stack, ants_stack);
    });
    assert_eq!(summary(cx).as_ref(), "2 of 3 frames");
    // bees again: its working version now based on #1, the render left.
    open_render(cx, window, &relaunched, &bee1);
    relaunched.read_with(cx, |app, _| {
        assert_eq!((&app.stack, app.base.as_ref()), (&bees_stack, Some(&bee1)));
    });
    assert_eq!(summary(cx).as_ref(), "2 frames");
}

/// Method picks by segment click, and the arrows step it once focused.
#[gpui::test]
fn the_method_control_picks_the_method(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 2) else { return };
    let config =
        |cx: &mut TestAppContext| app.read_with(cx, |a, cx| a.params.read(cx).stacker_config(cx));
    assert!(matches!(config(cx), StackerConfig::Pyramid(_)));
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    vcx.run_until_parked();
    let depth = vcx.debug_bounds("method-1").expect("the Depth segment");
    vcx.simulate_click(depth.center(), gpui::Modifiers::none());
    assert!(matches!(config(cx), StackerConfig::Depth(_)));
    cx.simulate_keystrokes(window, "right");
    assert!(matches!(config(cx), StackerConfig::Wavelet(_)));
}
