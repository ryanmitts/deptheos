//! The Library: its stacks and renders, its selection, reading and deleting.

use super::*;

/// A render opened from the Library lands after the user went back: the
/// working version its open kept shows there as kept.
#[gpui::test]
fn a_switch_landing_in_the_library_reads_the_stacks_again(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (dir.path().join("a"), dir.path().join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let ants = write_frames(&a, 3);
    let bees = write_frames(&b, 3);
    let (window, app) = launch(cx, &engine, bees, &dir.path().join("history"));
    let summary =
        |cx: &mut TestAppContext| app.read_with(cx, |app, cx| app.files.read(cx).summary());

    // Bee #1 and #2, then Ant #1 in a new stack, then an edit to Ant.
    stack(cx, window, &app);
    let bee = active(cx, &app).unwrap();
    exclude_first(cx, &app);
    stack(cx, window, &app);
    act(cx, window, &app, |app, window, cx| app.new_stack(window, cx));
    crate::test_support::switched(cx, &app);
    act(cx, window, &app, |app, window, cx| app.add_paths(ants.clone(), window, cx));
    wait_until(cx, "the ants", |cx| summary(cx).as_ref() == "3 frames");
    stack(cx, window, &app);
    let ant_stack = app.read_with(cx, |app, _| app.stack.clone());
    exclude_first(cx, &app);

    // Open Bee #1 (not the base, so it loads) and go back before it lands.
    act(cx, window, &app, |app, window, cx| {
        app.set_screen(Screen::Library, window, cx);
        app.open_from_library(bee.clone(), window, cx);
        app.set_screen(Screen::Library, window, cx);
    });
    wait_until(cx, "Bee #1", |cx| {
        app.read_with(cx, |app, cx| {
            let renders = app.renders.read(cx);
            !renders.is_pending() && renders.active(cx) == Some(&bee)
        })
    });
    assert_eq!(app.read_with(cx, |app, _| app.screen), Screen::Library);
    wait_until(cx, "Ant's edit kept, in the Library", |cx| {
        app.read_with(cx, |app, cx| {
            app.renders.read(cx).stack_info(&ant_stack).is_some_and(|i| i.kept())
        })
    });
}

/// The Library lists renders under their stack's name; Rename names a
/// stack; opening a render opens its stack's working version showing it.
#[gpui::test]
fn the_library_opens_stacks_and_renders(cx: &mut TestAppContext) {
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
    let titles = |cx: &mut TestAppContext| {
        app.read_with(cx, |app, cx| app.library.read(cx).shown_titles(cx))
    };
    let in_window =
        |cx: &mut TestAppContext, f: &dyn Fn(&mut AppView, &mut Window, &mut Context<AppView>)| {
            act(cx, window, &app, |app, window, cx| f(app, window, cx));
        };
    let from_library = |cx: &mut TestAppContext, id: &RenderId| {
        let opened = id.clone();
        in_window(cx, &move |app, window, cx| {
            app.set_screen(Screen::Library, window, cx);
            app.open_from_library(opened.clone(), window, cx)
        });
        wait_until(cx, "the render", |cx| {
            app.read_with(cx, |app, cx| {
                let renders = app.renders.read(cx);
                !renders.is_pending() && renders.active(cx) == Some(id)
            })
        });
        assert_eq!(app.read_with(cx, |app, _| app.screen), Screen::Stacking);
    };

    // bees #1, then ants #1 in a new stack, edited.
    stack(cx, window, &app);
    let bee = active(cx, &app).unwrap();
    in_window(cx, &|app, window, cx| app.new_stack(window, cx));
    crate::test_support::switched(cx, &app);
    // Dropped on the Library, the frames show where they went: Stacking.
    in_window(cx, &|app, window, cx| app.set_screen(Screen::Library, window, cx));
    in_window(cx, &|app, window, cx| app.add_paths(ants.clone(), window, cx));
    assert_eq!(app.read_with(cx, |app, _| app.screen), Screen::Stacking);
    wait_until(cx, "the ants", |cx| summary(cx).as_ref() == "3 frames");
    stack(cx, window, &app);
    let ant = active(cx, &app).unwrap();
    exclude_first(cx, &app);
    // A Library tile is titled by its stack's name.
    assert_eq!(titles(cx), ["ants", "bees"]);
    // The strip shows this stack only.
    let strip = |cx: &mut TestAppContext| {
        app.update(cx, |app, cx| {
            let on_screen = app.on_screen_in(&app.snapshot(cx));
            app.renders.update(cx, |r, _| {
                r.set_on_screen(on_screen);
                r.strip()
            })
        })
    };
    assert_eq!(strip(cx), std::slice::from_ref(&ant));

    // Renaming a stack renames its renders, and is kept.
    let bees_stack =
        app.read_with(cx, |app, cx| app.renders.read(cx).find(&bee).unwrap().stack.clone());
    // Two quick renames land in order on the store thread (ADR-143).
    app.update(cx, |app, cx| {
        app.renders.update(cx, |r, cx| {
            r.rename_stack(&bees_stack, "Bees", cx);
            r.rename_stack(&bees_stack, "Bee", cx)
        })
    });
    assert_eq!(titles(cx), ["ants", "Bee"]);
    cx.run_until_parked();
    deptheos_app::engine::store::store().write(|| ()).wait();
    let kept = app.read_with(cx, |app, cx| {
        let store = app.renders.read(cx).store().clone();
        store.stacks().unwrap().get(&bees_stack).and_then(|i| i.name.clone())
    });
    assert_eq!(kept.as_deref(), Some("Bee"));

    // Bee #1 is its stack's newest: its working version opens, editable.
    // ⌥⌘2 shows the Library; a click selects, ↩ opens.
    cx.simulate_keystrokes(window, "secondary-alt-2");
    assert_eq!(app.read_with(cx, |app, _| app.screen), Screen::Library);
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    let tile = vcx.debug_bounds("library-tile-Bee").expect("the Library shows Bee");
    vcx.simulate_click(tile.center(), gpui::Modifiers::none());
    assert_eq!(
        app.read_with(cx, |app, cx| app.library.read(cx).selected_stack(cx)),
        Some(bees_stack.clone())
    );
    cx.simulate_keystrokes(window, "enter");
    wait_until(cx, "Bee #1", |cx| {
        app.read_with(cx, |app, cx| {
            let renders = app.renders.read(cx);
            !renders.is_pending() && renders.active(cx) == Some(&bee)
        })
    });
    assert_eq!(app.read_with(cx, |app, _| app.screen), Screen::Stacking);
    assert_eq!(summary(cx).as_ref(), "2 frames");
    app.read_with(cx, |app, cx| assert_eq!(app.title_parts(cx).name.as_deref(), Some("Bee")));
    assert_eq!(strip(cx), std::slice::from_ref(&bee));

    // ants #1 is the base of ants' kept edited version: the edit comes back.
    from_library(cx, &ant);
    assert_eq!(summary(cx).as_ref(), "2 of 3 frames");

    // With ants #2 the base, ants #1 opens shown, the working version as is.
    stack(cx, window, &app);
    from_library(cx, &ant);
    assert_eq!(summary(cx).as_ref(), "2 of 3 frames");
    app.read_with(cx, |app, cx| {
        assert_eq!(app.title_parts(cx).what.as_deref(), Some("#1 · Pyramid · 3 frames"));
    });
}

/// A shown tile's cover is read and stays while in view, with nothing kept
/// past what is asked for.
#[gpui::test]
fn the_library_reads_the_covers_of_the_tiles_in_view(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let bee = dir.path().join("bee");
    std::fs::create_dir_all(&bee).unwrap();
    let paths = write_frames(&bee, 2);
    let (window, app) = launch(cx, &engine, paths, &dir.path().join("history"));
    stack(cx, window, &app);
    let cover = active(cx, &app).unwrap();
    let renders = app.read_with(cx, |app, _| app.renders.clone());
    act(cx, window, &app, |app, window, cx| app.set_screen(Screen::Library, window, cx));
    // Nothing selected, so the inspector asks for no render's picture.
    act(cx, window, &app, |app, _, cx| app.library.update(cx, |l, cx| l.deselect(cx)));
    assert_eq!(app.read_with(cx, |app, cx| app.library.read(cx).selected_len(cx)), 0);
    renders.update(cx, |r, _| r.set_thumbs_kept(0));
    // Drawn again and again: only what the Library asks for survives.
    for _ in 0..4 {
        act(cx, window, &app, |_, window, _| window.refresh());
        cx.run_until_parked();
    }
    wait_until(cx, "the cover", |cx| renders.read_with(cx, |r, _| r.thumbnail(&cover).is_some()));
    act(cx, window, &app, |_, window, _| window.refresh());
    cx.run_until_parked();
    assert!(renders.read_with(cx, |r, _| r.thumbnail(&cover).is_some()), "kept while in view");
}

/// A stack shows once with its render count; ⌘- and ⇧-click select several;
/// deleting a stack removes it whole, the open one leaving an empty stack.
#[gpui::test]
fn the_library_shows_stacks_and_deletes_them_whole(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (dir.path().join("ants"), dir.path().join("bees"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let ants = write_frames(&a, 3);
    let bees = write_frames(&b, 2);
    let (window, app) = launch(cx, &engine, bees, &dir.path().join("history"));
    cx.update(|cx| cx.bind_keys(key_bindings()));
    let in_window =
        |cx: &mut TestAppContext, f: &dyn Fn(&mut AppView, &mut Window, &mut Context<AppView>)| {
            act(cx, window, &app, |app, window, cx| f(app, window, cx));
        };
    let library = |cx: &mut TestAppContext| {
        app.read_with(cx, |app, cx| {
            let l = app.library.read(cx);
            (l.shown_badges(cx), l.count_text(cx), l.selected_len(cx))
        })
    };

    // bees #1 and #2, then ants #1 in a stack of its own.
    stack(cx, window, &app);
    stack(cx, window, &app);
    let bees_stack = app.read_with(cx, |app, _| app.stack.clone());
    in_window(cx, &|app, window, cx| app.new_stack(window, cx));
    crate::test_support::switched(cx, &app);
    in_window(cx, &|app, window, cx| app.add_paths(ants.clone(), window, cx));
    wait_until(cx, "the ants", |cx| {
        app.read_with(cx, |app, cx| app.files.read(cx).summary().as_ref() == "3 frames")
    });
    stack(cx, window, &app);
    let ants_stack = app.read_with(cx, |app, _| app.stack.clone());
    assert!(ants_stack != bees_stack);

    // One tile a stack: bees says it holds two renders.
    cx.simulate_keystrokes(window, "secondary-alt-2");
    let (badges, count, selected) = library(cx);
    assert_eq!(badges, [("ants".to_string(), None), ("bees".to_string(), Some("2".to_string()))]);
    assert!(count.starts_with("2 stacks · 3 renders"), "{count}");
    assert_eq!(selected, 1, "the stack open in Stacking is selected");

    // A click selects one, ⇧-click the run to it, ⌘-click takes one out.
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    let ants_tile = vcx.debug_bounds("library-tile-ants").expect("ants").center();
    let bees_tile = vcx.debug_bounds("library-tile-bees").expect("bees").center();
    vcx.simulate_click(ants_tile, gpui::Modifiers::none());
    vcx.simulate_click(bees_tile, gpui::Modifiers::shift());
    assert_eq!(library(cx).2, 2);
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    vcx.simulate_click(ants_tile, gpui::Modifiers::secondary_key());
    assert_eq!(
        app.read_with(cx, |app, cx| app.library.read(cx).selected_stack(cx)),
        Some(bees_stack.clone())
    );
    cx.simulate_keystrokes(window, "escape");
    assert_eq!(library(cx).2, 0);
    cx.simulate_keystrokes(window, "secondary-a");
    assert_eq!(library(cx).2, 2);

    // ants, open in Stacking, deleted whole with its frames there.
    let doomed = ants_stack.clone();
    in_window(cx, &move |app, window, cx| {
        app.renders.update(cx, |r, cx| r.delete_stacks(vec![doomed.clone()], window, cx))
    });
    wait_until(cx, "the delete", |cx| library(cx).0.len() == 1);
    assert_eq!(library(cx).0, [("bees".to_string(), Some("2".to_string()))]);
    app.read_with(cx, |app, cx| {
        assert!(app.stack != ants_stack, "an empty stack took its place");
        assert_eq!(app.files.read(cx).rows().count(), 0);
        assert!(app.base.is_none());
    });
}

/// ⌘⌫ with a render selected in the details pane deletes just it, unasked.
#[gpui::test]
fn command_backspace_deletes_the_render_selected(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let paths = write_frames(dir.path(), 3);
    let (window, app) = launch(cx, &engine, paths, &dir.path().join("history"));
    cx.update(|cx| cx.bind_keys(key_bindings()));
    stack(cx, window, &app);
    let first = active(cx, &app).unwrap();
    stack(cx, window, &app);
    let second = active(cx, &app).unwrap();
    let stack_id = app.read_with(cx, |app, _| app.stack.clone());
    cx.simulate_keystrokes(window, "secondary-alt-2");
    app.update(cx, |app, cx| {
        app.library.update(cx, |l, cx| l.select_render(&stack_id, &first, cx))
    });

    cx.simulate_keystrokes(window, "secondary-backspace");
    let asking = cx.update_window(window, |_, window, cx| window.has_active_dialog(cx)).unwrap();
    assert!(!asking, "it asks nothing");
    wait_until(cx, "the delete", |cx| {
        app.read_with(cx, |app, cx| {
            let renders = app.renders.read(cx);
            !renders.is_deleting() && renders.find(&first).is_none()
        })
    });
    app.read_with(cx, |app, cx| {
        let left: Vec<RenderId> =
            app.renders.read(cx).renders().iter().map(|r| r.id.clone()).collect();
        assert_eq!(left, [second], "only the render selected went");
        assert_eq!(app.stack, stack_id, "its stack stays");
    });
}

/// Deleting a stack asks nothing and lists it in Recently Deleted, where
/// ⌘⌫ does nothing; Restore puts it back whole; Empty Recently Deleted…
/// asks, then removes all for good.
#[gpui::test]
fn recently_deleted_lists_restores_and_empties(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (dir.path().join("ants"), dir.path().join("bees"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let ants = write_frames(&a, 3);
    let bees = write_frames(&b, 2);
    let (window, app) = launch(cx, &engine, bees, &dir.path().join("history"));
    cx.update(|cx| cx.bind_keys(key_bindings()));
    let asking = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| window.has_active_dialog(cx)).unwrap()
    };
    let badges = |cx: &mut TestAppContext| {
        app.read_with(cx, |app, cx| app.library.read(cx).shown_badges(cx))
    };
    let listed = |cx: &mut TestAppContext| {
        app.read_with(cx, |app, cx| app.library.read(cx).deleted_titles(cx))
    };
    let idle =
        |cx: &mut TestAppContext| !app.read_with(cx, |app, cx| app.renders.read(cx).is_deleting());

    // bees #1 and #2, then ants #1 in a stack of its own.
    stack(cx, window, &app);
    stack(cx, window, &app);
    act(cx, window, &app, |app, window, cx| app.new_stack(window, cx));
    crate::test_support::switched(cx, &app);
    act(cx, window, &app, |app, window, cx| app.add_paths(ants.clone(), window, cx));
    wait_until(cx, "the ants", |cx| {
        app.read_with(cx, |app, cx| app.files.read(cx).summary().as_ref() == "3 frames")
    });
    stack(cx, window, &app);
    let bees_render = app.read_with(cx, |app, cx| {
        let renders = app.renders.read(cx);
        let bees: Vec<_> = renders.renders().iter().filter(|r| r.number == 2).collect();
        bees[0].id.clone()
    });

    // bees, selected in the Library and deleted with ⌘⌫: no question.
    cx.simulate_keystrokes(window, "secondary-alt-2");
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    let bees_tile = vcx.debug_bounds("library-tile-bees").expect("bees").center();
    vcx.simulate_click(bees_tile, gpui::Modifiers::none());
    cx.simulate_keystrokes(window, "secondary-backspace");
    assert!(!asking(cx), "it asks nothing");
    wait_until(cx, "the delete", |cx| badges(cx).len() == 1 && idle(cx));
    let said = cx.update_window(window, |_, window, cx| crate::notices::last_said(window, cx));
    assert_eq!(said.unwrap().as_deref(), Some("bees moved to Recently Deleted."));

    // Recently Deleted, last in the left bar, lists it.
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    let row = vcx.debug_bounds("library-show-deleted").expect("its row").center();
    vcx.simulate_click(row, gpui::Modifiers::none());
    wait_until(cx, "the list", |cx| listed(cx).is_some());
    assert_eq!(listed(cx).unwrap(), ["bees"]);
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("library-deleted").is_some(), "in the grid's place");
    let tile = vcx.debug_bounds("library-deleted-bees").expect("bees' record").center();
    vcx.simulate_click(tile, gpui::Modifiers::none());

    // ⌘⌫ deletes nothing there.
    cx.simulate_keystrokes(window, "secondary-backspace");
    cx.run_until_parked();
    assert!(!asking(cx) && idle(cx));
    assert_eq!(listed(cx).unwrap(), ["bees"]);

    // Restore puts bees back whole, both renders.
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    let restore = vcx.debug_bounds("deleted-restore").expect("Restore");
    vcx.simulate_click(restore.center(), gpui::Modifiers::none());
    wait_until(cx, "the restore", |cx| idle(cx) && listed(cx).is_some_and(|l| l.is_empty()));
    app.read_with(cx, |app, cx| {
        assert!(app.renders.read(cx).find(&bees_render).is_some(), "bees #2 is back");
    });
    app.update(cx, |app, cx| {
        app.library.update(cx, |l, cx| l.show_only(crate::renders::Filter::All, cx))
    });
    wait_until(cx, "bees named again", |cx| {
        badges(cx) == [("ants".to_string(), None), ("bees".to_string(), Some("2".to_string()))]
    });

    // ants and bees deleted; Empty asks first, then removes them for good.
    let stacks: Vec<StackId> = app.read_with(cx, |app, cx| {
        app.renders.read(cx).renders().iter().map(|r| r.stack.clone()).collect()
    });
    act(cx, window, &app, |app, window, cx| {
        app.renders.update(cx, |r, cx| r.delete_stacks(stacks.clone(), window, cx))
    });
    wait_until(cx, "the delete", |cx| badges(cx).is_empty() && idle(cx));
    app.update(cx, |app, cx| app.library.update(cx, |l, cx| l.show_deleted(cx)));
    wait_until(cx, "the list", |cx| listed(cx).is_some_and(|l| l.len() == 2));
    let deleted = dir.path().join("history").join("Deleted");
    assert_eq!(std::fs::read_dir(&deleted).unwrap().count(), 2);
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    let empty = vcx.debug_bounds("deleted-empty").expect("Empty").center();
    vcx.simulate_click(empty, gpui::Modifiers::none());
    vcx.run_until_parked();
    assert!(asking(cx), "Empty asks");
    let confirm = vcx.debug_bounds("alert-destructive").expect("Empty").center();
    vcx.simulate_click(confirm, gpui::Modifiers::none());
    wait_until(cx, "emptied", |cx| idle(cx) && listed(cx).is_some_and(|l| l.is_empty()));
    assert_eq!(std::fs::read_dir(&deleted).map_or(0, |d| d.count()), 0, "gone for good");
}

/// Recently Deleted selects as the grid does: ⇧-click ranges, Restore puts
/// them all back.
#[gpui::test]
fn recently_deleted_selects_a_range(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 2) else { return };
    for _ in 0..3 {
        stack(cx, window, &app);
    }
    let renders = |cx: &mut TestAppContext| {
        app.read_with(cx, |app, cx| {
            app.renders.read(cx).renders().iter().map(|r| r.id.clone()).collect::<Vec<_>>()
        })
    };
    let idle =
        |cx: &mut TestAppContext| !app.read_with(cx, |app, cx| app.renders.read(cx).is_deleting());
    // Each render deleted alone: three records.
    for id in renders(cx) {
        act(cx, window, &app, |app, window, cx| {
            app.renders.update(cx, |r, cx| r.delete(vec![id.clone()], window, cx))
        });
        wait_until(cx, "the delete", |cx| idle(cx));
    }
    assert!(renders(cx).is_empty());
    act(cx, window, &app, |app, window, cx| app.set_screen(Screen::Library, window, cx));
    let library = app.read_with(cx, |app, _| app.library.clone());
    library.update(cx, |l, cx| l.show_deleted(cx));
    let listed = |cx: &mut TestAppContext| library.read_with(cx, |l, cx| l.deleted_titles(cx));
    wait_until(cx, "the list", |cx| listed(cx).is_some_and(|l| l.len() == 3));
    let titles = listed(cx).unwrap();

    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    vcx.run_until_parked();
    let at = |vcx: &mut gpui::VisualTestContext, title: &str| {
        vcx.debug_bounds(Box::leak(format!("library-deleted-{title}").into_boxed_str()))
            .expect("its record")
            .center()
    };
    let first = at(&mut vcx, &titles[0]);
    vcx.simulate_click(first, gpui::Modifiers::none());
    let last = at(&mut vcx, &titles[2]);
    vcx.simulate_click(last, gpui::Modifiers::shift());
    let restore = vcx.debug_bounds("deleted-restore").expect("Restore").center();
    vcx.simulate_click(restore, gpui::Modifiers::none());
    wait_until(cx, "the restore", |cx| idle(cx) && listed(cx).is_some_and(|l| l.is_empty()));
    assert_eq!(renders(cx).len(), 3, "all three back");
}

/// One page at a time: Recently Deleted replaces the filter's grid, a filter
/// chosen there returns to it, and leaving the Library lets its list go.
#[gpui::test]
fn the_library_shows_a_filter_or_recently_deleted(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 2) else { return };
    stack(cx, window, &app);
    act(cx, window, &app, |app, window, cx| app.set_screen(Screen::Library, window, cx));
    let library = app.read_with(cx, |app, _| app.library.clone());
    let showing = |cx: &mut TestAppContext| library.read_with(cx, |l, _| l.is_showing_deleted());
    let count = |cx: &mut TestAppContext| library.read_with(cx, |l, cx| l.count_text(cx));
    let listed = |cx: &mut TestAppContext| library.read_with(cx, |l, cx| l.deleted_titles(cx));

    library.update(cx, |l, cx| l.show_only(crate::renders::Filter::Starred, cx));
    assert_eq!(count(cx), "No starred renders");
    assert!(!showing(cx));

    library.update(cx, |l, cx| l.show_deleted(cx));
    assert!(showing(cx), "in the grid's place");
    wait_until(cx, "the list", |cx| listed(cx).is_some());

    library.update(cx, |l, cx| l.show_only(crate::renders::Filter::All, cx));
    assert!(!showing(cx), "a filter goes back to the grid");
    assert_eq!(listed(cx), None, "the list let go of");
    assert!(count(cx).starts_with("1 stack"), "{}", count(cx));

    library.update(cx, |l, cx| l.show_deleted(cx));
    wait_until(cx, "the list", |cx| listed(cx).is_some());
    act(cx, window, &app, |app, window, cx| app.set_screen(Screen::Stacking, window, cx));
    assert!(!showing(cx), "leaving the Library hides it");
    assert_eq!(listed(cx), None);
}

/// Behind Recently Deleted the title bar counts the whole Library, and
/// returning shows every stack.
#[gpui::test]
fn recently_deleted_leaves_to_every_stack(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 2) else { return };
    stack(cx, window, &app);
    act(cx, window, &app, |app, window, cx| app.set_screen(Screen::Library, window, cx));
    let library = app.read_with(cx, |app, _| app.library.clone());
    let count = |cx: &mut TestAppContext| library.read_with(cx, |l, cx| l.count_text(cx));

    library.update(cx, |l, cx| l.show_only(crate::renders::Filter::Starred, cx));
    library.update(cx, |l, cx| l.show_deleted(cx));
    assert!(count(cx).starts_with("1 stack"), "{}", count(cx));
    act(cx, window, &app, |app, window, cx| app.set_screen(Screen::Stacking, window, cx));
    act(cx, window, &app, |app, window, cx| app.set_screen(Screen::Library, window, cx));
    assert!(count(cx).starts_with("1 stack"), "{}", count(cx));
}

/// Library arrows move the selection by grid and rows (⇧ extends) and
/// scroll to it; a click on empty grid clears it; ⌘F focuses search until Esc.
#[gpui::test]
fn the_library_moves_its_selection_by_key(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let names = ["a", "b", "c", "d", "e", "f"];
    let frames: Vec<Vec<PathBuf>> = names
        .iter()
        .map(|name| {
            let folder = dir.path().join(name);
            std::fs::create_dir_all(&folder).unwrap();
            write_frames(&folder, 1)
        })
        .collect();
    let (window, app) = launch(cx, &engine, frames[0].clone(), &dir.path().join("history"));
    cx.update(|cx| cx.bind_keys(key_bindings()));
    wait_until(cx, "a", |cx| app.read_with(cx, |app, cx| app.files.read(cx).rows().count() == 1));
    // Six stacks never stacked, a to f.
    for paths in &frames[1..] {
        act(cx, window, &app, |app, window, cx| app.new_stack(window, cx));
        crate::test_support::switched(cx, &app);
        let paths = paths.clone();
        act(cx, window, &app, |app, window, cx| app.add_paths(paths, window, cx));
        wait_until(cx, "the frame", |cx| {
            app.read_with(cx, |app, cx| app.files.read(cx).rows().count() == 1)
        });
    }
    cx.simulate_keystrokes(window, "secondary-alt-2");
    wait_until(cx, "the Library's stacks", |cx| {
        app.read_with(cx, |app, cx| app.library.read(cx).unstacked_names(cx) == names)
    });
    let selected = |cx: &mut TestAppContext| {
        app.read_with(cx, |app, cx| app.library.read(cx).selected_titles(cx))
    };
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    let bounds: Vec<gpui::Bounds<gpui::Pixels>> = [
        "library-unstacked-a",
        "library-unstacked-b",
        "library-unstacked-c",
        "library-unstacked-d",
        "library-unstacked-e",
        "library-unstacked-f",
    ]
    .into_iter()
    .map(|tile| vcx.debug_bounds(tile).expect("the tile"))
    .collect();
    let grid = vcx.debug_bounds("library-grid").expect("the grid");

    // From nothing, the first; then along the grid.
    cx.simulate_keystrokes(window, "escape");
    assert!(selected(cx).is_empty());
    cx.simulate_keystrokes(window, "right");
    assert_eq!(selected(cx), ["a"]);
    cx.simulate_keystrokes(window, "right right left");
    assert_eq!(selected(cx), ["b"]);
    // ↓ goes to the tile under it, as wide as the window lays them out.
    let below = (0..names.len())
        .filter(|&i| bounds[i].left() == bounds[1].left() && bounds[i].top() > bounds[1].top())
        .min_by(|&i, &j| bounds[i].top().partial_cmp(&bounds[j].top()).unwrap())
        .expect("a second row");
    cx.simulate_keystrokes(window, "down");
    assert_eq!(selected(cx), [names[below]]);
    cx.simulate_keystrokes(window, "up");
    assert_eq!(selected(cx), ["b"]);
    // ⇧ extends from the anchor, both ways.
    cx.simulate_keystrokes(window, "shift-right shift-right");
    assert_eq!(selected(cx), ["b", "c", "d"]);
    cx.simulate_keystrokes(window, "shift-left shift-left shift-left");
    assert_eq!(selected(cx), ["a", "b"]);

    // A click on a tile selects it; one on empty grid, nothing.
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    vcx.simulate_click(bounds[2].center(), gpui::Modifiers::none());
    assert_eq!(selected(cx), ["c"]);
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    let empty = gpui::point(grid.left() + gpui::px(40.0), grid.bottom() - gpui::px(20.0));
    vcx.simulate_click(empty, gpui::Modifiers::none());
    assert!(selected(cx).is_empty());

    // ⌘F: the keys type in the search field; Esc gives them back.
    let search_focused = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| {
            let search = app.read(cx).library.read(cx).search.clone();
            gpui::Focusable::focus_handle(search.read(cx), cx).is_focused(window)
        })
        .unwrap()
    };
    cx.simulate_keystrokes(window, "secondary-f");
    assert!(search_focused(cx));
    cx.simulate_keystrokes(window, "right");
    assert!(selected(cx).is_empty(), "the arrow moved in the field");
    cx.simulate_keystrokes(window, "escape");
    assert!(!search_focused(cx));
    cx.simulate_keystrokes(window, "right");
    assert_eq!(selected(cx), ["a"]);

    // In a short window, ↓ scrolls the selected tile into view.
    let scrolled =
        |cx: &mut TestAppContext| app.read_with(cx, |app, cx| app.library.read(cx).scrolled());
    let vcx = gpui::VisualTestContext::from_window(window, cx);
    vcx.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(260.0)));
    cx.run_until_parked();
    assert_eq!(scrolled(cx), gpui::px(0.0));
    cx.simulate_keystrokes(window, "down");
    cx.run_until_parked();
    assert!(scrolled(cx) > gpui::px(0.0), "the second row scrolled into view");
}

/// Frames added and never stacked are kept when another stack opens, and
/// listed as not stacked yet, reopening with their exclusions.
#[gpui::test]
fn a_stack_never_stacked_is_in_the_library(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (dir.path().join("ants"), dir.path().join("bees"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let ants = write_frames(&a, 3);
    let bees = write_frames(&b, 2);
    let (window, app) = launch(cx, &engine, bees, &dir.path().join("history"));
    let summary =
        |cx: &mut TestAppContext| app.read_with(cx, |app, cx| app.files.read(cx).summary());
    let in_window =
        |cx: &mut TestAppContext, f: &dyn Fn(&mut AppView, &mut Window, &mut Context<AppView>)| {
            act(cx, window, &app, |app, window, cx| f(app, window, cx));
        };
    // The Library reads the stacks, off the UI thread, as it is shown.
    let wait_unstacked = |cx: &mut TestAppContext, want: &[&str]| {
        in_window(cx, &|app, window, cx| app.set_screen(Screen::Stacking, window, cx));
        in_window(cx, &|app, window, cx| app.set_screen(Screen::Library, window, cx));
        wait_until(cx, "the Library's stacks", |cx| {
            app.read_with(cx, |app, cx| app.library.read(cx).unstacked_names(cx) == want)
        });
    };

    // bees #1, then ants in a new stack, edited and not stacked.
    stack(cx, window, &app);
    let bee = active(cx, &app).unwrap();
    in_window(cx, &|app, window, cx| app.new_stack(window, cx));
    crate::test_support::switched(cx, &app);
    in_window(cx, &|app, window, cx| app.add_paths(ants.clone(), window, cx));
    wait_until(cx, "the ants", |cx| summary(cx).as_ref() == "3 frames");
    exclude_first(cx, &app);
    // The open stack is listed too.
    wait_unstacked(cx, &["ants"]);

    // Opening bees keeps the ants, and the Library still lists them.
    open_render(cx, window, &app, &bee);
    assert_eq!(summary(cx).as_ref(), "2 frames");
    wait_unstacked(cx, &["ants"]);

    // Opened from there, they are back as they were left.
    app.update(cx, |app, cx| app.library.update(cx, |l, cx| l.open_unstacked("ants", cx)));
    wait_until(cx, "the ants again", |cx| summary(cx).as_ref() == "2 of 3 frames");
    app.read_with(cx, |app, cx| {
        assert_eq!(app.screen, Screen::Stacking);
        assert!(app.files.read(cx).can_stack());
    });
    wait_unstacked(cx, &["ants"]);
}

/// A failed stack says so in the inspector, with its number, while on
/// screen; the next stack clears it.
#[gpui::test]
fn a_failed_stack_says_so_until_the_next(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let mut paths = write_frames(dir.path(), 2);
    for name in ["frame_8.png", "frame_9.png"] {
        let broken = dir.path().join(name);
        std::fs::write(&broken, b"not an image").unwrap();
        paths.push(broken);
    }
    let root = dir.path().join("history");
    let (window, app) = launch(cx, &engine, paths, &root);
    let failure = |cx: &mut TestAppContext| {
        app.read_with(cx, |app, _| app.shown_failure().map(|f| (f.number, f.error.clone())))
    };
    let set_included = |cx: &mut TestAppContext, rows: [bool; 4]| {
        wait_until(cx, "the frames", |cx| {
            app.read_with(cx, |app, cx| app.files.read(cx).rows().count() == 4)
        });
        let ids: Vec<FileId> =
            app.read_with(cx, |app, cx| app.files.read(cx).rows().map(|r| r.0).collect());
        app.update(cx, |app, cx| {
            app.files.update(cx, |f, cx| {
                for (id, included) in ids.iter().zip(rows) {
                    f.set_included(*id, included, cx)
                }
            })
        });
    };

    // Only the broken frames: it fails.
    set_included(cx, [false, false, true, true]);
    stack(cx, window, &app);
    let (number, error) = failure(cx).expect("the failure is shown");
    assert_eq!(number, Some(1));
    // Unreadable frames are the user's to fix, in their terms; the technical
    // text is kept for Copy Details.
    assert_eq!(error.kind, deptheos_app::ExplanationKind::User, "{error:?}");
    assert!(error.reason.starts_with("Only 1 of the 2 frames could be stacked."), "{error:?}");
    assert!(error.reason.contains("couldn't be read as an image file."), "{error:?}");
    assert!(!error.details.is_empty());
    assert!(app.read_with(cx, |app, cx| app.renders.read(cx).renders().is_empty()));

    // Another stack on screen doesn't show it: Try Again would stack that one.
    let failed = app.update(cx, |app, _| std::mem::replace(&mut app.stack, StackId::new()));
    assert!(failure(cx).is_none());
    app.update(cx, |app, _| app.stack = failed);
    assert!(failure(cx).is_some());

    // The good frames stack, and the failure goes.
    set_included(cx, [true, true, false, false]);
    stack(cx, window, &app);
    assert!(failure(cx).is_none());
    assert_eq!(app.read_with(cx, |app, cx| app.renders.read(cx).renders().len()), 1);
}

/// In the Library, actions on Stacking's hidden stack (Revert, Star Render,
/// Remove All Frames, frame items) are dim and do nothing until Stacking is back.
#[gpui::test]
fn the_library_leaves_the_hidden_stack_alone(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let bee = dir.path().join("bee");
    std::fs::create_dir_all(&bee).unwrap();
    let paths = write_frames(&bee, 3);
    let (window, app) = launch(cx, &engine, paths, &dir.path().join("history"));
    stack(cx, window, &app);
    exclude_first(cx, &app);
    act(cx, window, &app, |app, window, cx| app.set_screen(Screen::Stacking, window, cx));
    let state = |cx: &mut TestAppContext| app.read_with(cx, |app, cx| app.menu_state(cx));
    let on = state(cx);
    assert!(matches!(on.revert, RevertItem::Revert { number: 1, enabled: true }));
    assert_eq!(on.picked, Some(false));
    assert!(on.frames);

    act(cx, window, &app, |app, window, cx| app.set_screen(Screen::Library, window, cx));
    let off = state(cx);
    assert!(matches!(off.revert, RevertItem::Revert { number: 1, enabled: false }));
    assert_eq!(off.picked, None);
    assert!(!off.frames && !off.stacked && !off.has_result);
    act(cx, window, &app, |app, window, cx| {
        app.revert_shown(window, cx);
        app.confirm_clear(window, cx);
    });
    cx.run_until_parked();
    assert!(!asking(cx, window), "nothing asks in the Library");
    app.read_with(cx, |app, cx| {
        assert_eq!(app.files.read(cx).summary().as_ref(), "2 of 3 frames");
    });

    act(cx, window, &app, |app, window, cx| app.set_screen(Screen::Stacking, window, cx));
    revert_and_confirm(cx, window, &app);
    app.read_with(cx, |app, cx| assert_eq!(app.files.read(cx).summary().as_ref(), "3 frames"));
}

/// A launch measures the renders, so the Library shows their size unopened.
#[gpui::test]
fn a_launch_measures_the_renders(cx: &mut TestAppContext) {
    let Some(Fixture { dir, engine, window, app }) = Fixture::launch(cx, 2) else { return };
    let root = dir.path().join("history");
    stack(cx, window, &app);
    crate::test_support::quit(cx, window, app);
    let (_, relaunched) = launch(cx, &engine, Vec::new(), &root);
    assert_eq!(relaunched.read_with(cx, |app, _| app.screen), Screen::Library);
    wait_until(cx, "the renders' size", |cx| {
        relaunched.read_with(cx, |app, cx| {
            let renders = app.renders.read(cx);
            !renders.renders().is_empty()
                && renders
                    .size_text(renders.renders().iter().map(|r| &r.id), cx.wording())
                    .is_some()
        })
    });
}

/// A long history reads only the newest thumbnails at launch, the rest as
/// the strip scrolls. Requests are checked, not reads, which finish at
/// their own pace.
#[gpui::test]
fn history_thumbnails_load_as_they_scroll_into_view(cx: &mut TestAppContext) {
    let Some(Fixture { dir, engine, window, app }) = Fixture::launch(cx, 2) else { return };
    let root = dir.path().join("history");
    stack(cx, window, &app);
    crate::test_support::quit(cx, window, app);
    // One render copied into a long history of one stack: the copies' ids
    // are older, so they are #1 to #79 and the original #80.
    let renders = root.join("Renders");
    let first = std::fs::read_dir(&renders).unwrap().next().unwrap().unwrap().path();
    const COUNT: usize = 80;
    let numbered = |dir: &Path, number: usize| {
        let manifest = dir.join("render.json");
        let mut json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
        json["number"] = number.into();
        std::fs::write(&manifest, serde_json::to_vec(&json).unwrap()).unwrap();
    };
    for n in 1..COUNT {
        let copy = renders.join(format!("{n:032x}"));
        std::fs::create_dir_all(&copy).unwrap();
        for file in std::fs::read_dir(&first).unwrap().flatten() {
            if file.path().is_file() {
                std::fs::copy(file.path(), copy.join(file.file_name())).unwrap();
            }
        }
        numbered(&copy, n);
    }
    numbered(&first, COUNT);
    // No stack selected at launch; selecting one makes the details pane
    // list and read all its renders.
    std::fs::remove_file(root.join("list.json")).unwrap();

    let (window, app) = launch(cx, &engine, Vec::new(), &root);
    let renders = app.read_with(cx, |app, _| app.renders.clone());
    wait_until(cx, "the newest thumbnails", |cx| {
        renders.read_with(cx, |r, _| !r.thumbnail_ids().is_empty())
    });
    cx.run_until_parked();
    let (oldest, newest) = renders
        .read_with(cx, |r, _| (r.renders()[0].id.clone(), r.renders()[COUNT - 1].id.clone()));
    let asked = |cx: &mut TestAppContext| renders.read_with(cx, |r, _| r.thumbnails_requested());
    let at_launch = asked(cx);
    assert!(at_launch.len() < COUNT, "not all at launch ({} of {COUNT})", at_launch.len());
    assert!(!at_launch.contains(&oldest));

    // In the stack its strip, newest first, asks for the tiles in view.
    open_render(cx, window, &app, &newest);
    act(cx, window, &app, |app, window, cx| app.set_screen(Screen::Stacking, window, cx));
    cx.run_until_parked();
    assert_eq!(renders.read_with(cx, |r, _| r.strip().last().cloned()), Some(oldest.clone()));
    let in_stack = asked(cx);
    assert!(in_stack.len() < COUNT, "not all in the stack ({} of {COUNT})", in_stack.len());
    assert!(!in_stack.contains(&oldest), "not before the strip scrolls to it");
    renders.update(cx, |r, cx| r.scroll_to_oldest(cx));
    wait_until(cx, "the oldest thumbnail", |cx| {
        renders.read_with(cx, |r, _| r.thumbnail_ids().contains(&oldest))
    });
}

/// Hundreds of stacks build only the tiles in view and ask for their covers
/// and the rows either side; scrolling to the end builds the last. Requests
/// are checked, not reads (one runs at a time).
#[gpui::test]
fn the_library_draws_only_the_rows_in_view(cx: &mut TestAppContext) {
    let Some(Fixture { dir, engine, window, app }) = Fixture::launch(cx, 2) else { return };
    let root = dir.path().join("history");
    stack(cx, window, &app);
    crate::test_support::quit(cx, window, app);
    // One render copied into stacks of their own, each a second older than
    // the next, so the grid shows the original then copies from COUNT - 1
    // down; each copy's folder names its tile.
    let renders = root.join("Renders");
    let first = std::fs::read_dir(&renders).unwrap().next().unwrap().unwrap().path();
    const COUNT: usize = 300;
    let manifest = first.join("render.json");
    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
    let made = json["created_unix_ms"].as_u64().unwrap();
    for n in 1..COUNT {
        let copy = renders.join(format!("{n:032x}"));
        std::fs::create_dir_all(&copy).unwrap();
        for file in std::fs::read_dir(&first).unwrap().flatten() {
            if file.path().is_file() {
                std::fs::copy(file.path(), copy.join(file.file_name())).unwrap();
            }
        }
        let mut json = json.clone();
        json["stack"] = format!("{:032x}", 0x10000 + n).into();
        json["number"] = 1.into();
        json["created_unix_ms"] = (made - (COUNT - n) as u64 * 1000).into();
        for frame in json["frames"].as_array_mut().unwrap() {
            let path = Path::new(frame["path"].as_str().unwrap());
            let file = path.file_name().unwrap().to_str().unwrap();
            frame["path"] = format!("s{}/{file}", COUNT - n).into();
        }
        std::fs::write(copy.join("render.json"), serde_json::to_vec(&json).unwrap()).unwrap();
    }
    // No stack selected at launch, so the details pane asks for nothing.
    std::fs::remove_file(root.join("list.json")).unwrap();

    let (window, app) = launch(cx, &engine, Vec::new(), &root);
    act(cx, window, &app, |app, window, cx| app.set_screen(Screen::Library, window, cx));
    let renders = app.read_with(cx, |app, _| app.renders.clone());
    let covers = app.read_with(cx, |app, cx| app.library.read(cx).shown_covers(cx));
    assert_eq!(covers.len(), COUNT);
    // A small window: two tiles to a row, a few rows in view.
    let vcx = gpui::VisualTestContext::from_window(window, cx);
    vcx.simulate_resize(gpui::size(gpui::px(900.0), gpui::px(500.0)));
    act(cx, window, &app, |_, window, _| window.refresh());
    cx.run_until_parked();
    // The copies' tiles laid out in the last frame, by their places.
    let selectors: Vec<&'static str> =
        (0..COUNT).map(|n| &*format!("library-tile-s{n}").leak()).collect();
    let built = |cx: &mut TestAppContext| {
        let mut vcx = gpui::VisualTestContext::from_window(window, cx);
        (1..COUNT).filter(|&n| vcx.debug_bounds(selectors[n]).is_some()).collect()
    };
    let shown: Vec<usize> = built(cx);
    assert!(shown.contains(&1) && shown.len() < 20, "the tiles built: {shown:?}");
    let asked = |cx: &mut TestAppContext| renders.read_with(cx, |r, _| r.thumbnails_requested());
    let in_view = asked(cx);
    // The newest PER_READ at launch plus the rows in view and either side.
    assert!(in_view.len() < 60, "{} asked for of {COUNT}", in_view.len());
    assert!(in_view.contains(&covers[0]), "the first tile's");
    assert!(!in_view.contains(&covers[COUNT - 1]), "not the last tile's before it is in view");

    app.update(cx, |app, cx| app.library.update(cx, |l, cx| l.scroll_to_end(cx)));
    act(cx, window, &app, |_, window, _| window.refresh());
    cx.run_until_parked();
    // Asked for once the read in flight (if one is) lands.
    wait_until(cx, "the last tile's, scrolled to", |cx| asked(cx).contains(&covers[COUNT - 1]));
    let shown: Vec<usize> = built(cx);
    assert!(shown.contains(&(COUNT - 1)) && shown.len() < 20, "the tiles built: {shown:?}");
}

/// While the Library is read the grid says so and the welcome waits: never
/// "No stacks yet" before it knows.
#[gpui::test]
fn the_library_says_it_is_reading(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let root = crate::test_support::opened(&dir.path().join("history"));
    let mut before = None;
    let (_, app) = open(cx, |window, cx| {
        let app = cx.new(|cx| AppView::new(engine, Vec::new(), root, window, cx));
        let view = app.read(cx);
        let library = view.library.read(cx);
        before = Some((view.screen, library.empty_text(cx), library.is_blank(cx)));
        app
    });
    let (screen, text, blank) = before.unwrap();
    assert_eq!(screen, Screen::Library, "the Library is home");
    assert_eq!(text, "Reading the Library…");
    assert!(!blank, "no welcome yet");
    wait_until(cx, "the history", |cx| {
        app.read_with(cx, |app, cx| !app.renders.read(cx).listing().is_listing())
    });
    app.read_with(cx, |app, cx| {
        let library = app.library.read(cx);
        assert!(matches!(app.renders.read(cx).listing(), LibraryRead::Listed));
        assert_eq!(
            library.empty_text(cx),
            "No stacks yet. New Stack starts one, and each is kept here."
        );
        assert!(library.is_blank(cx) && app.is_empty(cx), "the welcome");
    });
}

/// A rename the folder refuses puts the old name back, for a stack and a
/// render, and says why.
#[cfg(unix)]
#[gpui::test]
fn a_refused_rename_puts_the_old_name_back(cx: &mut TestAppContext) {
    use std::os::unix::fs::PermissionsExt as _;
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let paths = write_frames(dir.path(), 3);
    let history = dir.path().join("history");
    let (window, app) = launch(cx, &engine, paths, &history);
    stack(cx, window, &app);
    let render = active(cx, &app).unwrap();
    let (stack_id, render_dir) = app.read_with(cx, |app, cx| {
        let renders = app.renders.read(cx);
        let stack = renders.find(&render).unwrap().stack.clone();
        (stack, renders.store().render_path(&render))
    });
    let titles = |cx: &mut TestAppContext| {
        app.read_with(cx, |app, cx| app.library.read(cx).shown_titles(cx))
    };
    let before = titles(cx);
    let name = |cx: &mut TestAppContext| {
        app.read_with(cx, |app, cx| app.renders.read(cx).find(&render).unwrap().name.clone())
    };
    let notices = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| window.notifications(cx).len()).unwrap()
    };
    let base = notices(cx);
    let locked = [history.join("Stacks"), render_dir];
    for folder in &locked {
        std::fs::create_dir_all(folder).unwrap();
        std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o555)).unwrap();
    }
    let unlock = || {
        for folder in &locked {
            std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    };

    let refused = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        app.update(cx, |app, cx| {
            app.renders.update(cx, |r, cx| r.rename_stack(&stack_id, "Bee", cx))
        });
        assert_ne!(titles(cx), before, "the new name shows straight away");
        wait_until(cx, "the old stack name", |cx| titles(cx) == before);
        assert_eq!(notices(cx), base + 1, "the refusal is said");

        act(cx, window, &app, |app, window, cx| {
            app.renders.update(cx, |r, cx| r.start_rename(&render, window, cx))
        });
        cx.simulate_input(window, "Wasp");
        cx.simulate_keystrokes(window, "enter");
        wait_until(cx, "the old render name", |cx| name(cx).is_none() && notices(cx) == base + 2);
    }));
    unlock();
    assert!(refused.is_ok(), "both renames are undone");
}

/// Show Frames in Finder (stack menu and inspector) shows the first frame:
/// the cover's, or a never-stacked stack's.
#[gpui::test]
fn show_frames_in_finder_finds_a_stacks_frames(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (dir.path().join("ants"), dir.path().join("bees"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let (ants, bees) = (write_frames(&a, 2), write_frames(&b, 2));
    let (window, app) = launch(cx, &engine, bees.clone(), &dir.path().join("history"));
    stack(cx, window, &app);
    let bees_stack = app.read_with(cx, |app, _| app.stack.clone());
    act(cx, window, &app, |app, window, cx| app.new_stack(window, cx));
    crate::test_support::switched(cx, &app);
    let added = ants.clone();
    act(cx, window, &app, |app, window, cx| app.add_paths(added, window, cx));
    wait_until(cx, "the ants", |cx| {
        app.read_with(cx, |app, cx| app.files.read(cx).rows().count() == 2)
    });
    let ants_stack = app.read_with(cx, |app, _| app.stack.clone());
    act(cx, window, &app, |app, window, cx| app.set_screen(Screen::Library, window, cx));
    let first = |cx: &mut TestAppContext, stack: &deptheos_app::library::history::StackId| {
        app.read_with(cx, |app, cx| app.library.read(cx).first_frame(stack, cx))
    };
    assert_eq!(first(cx, &bees_stack), Some(bees[0].clone()));
    assert_eq!(first(cx, &ants_stack), Some(ants[0].clone()));
    // The inspector offers it for the stack selected.
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    assert!(vcx.debug_bounds("library-show-frames").is_some());
}

/// Delete Library works once it has content and nothing runs, leaving the
/// window as at first launch: blank Library, empty stack, nothing open, the
/// store empty including unlisted renders.
#[gpui::test]
fn deleting_the_library_leaves_a_first_launch(cx: &mut TestAppContext) {
    use crate::app::library_delete::{self, Deletable};
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let paths = write_frames(dir.path(), 3);
    let root = dir.path().join("history");
    let (window, app) = launch(cx, &engine, paths.clone(), &root);
    cx.update(|cx| library_delete::register(window, &app, cx));
    stack(cx, window, &app);
    let first = active(cx, &app).unwrap();
    exclude_first(cx, &app);
    stack(cx, window, &app);
    act(cx, window, &app, |app, window, cx| app.compare_render(first.clone(), window, cx));
    wait_until(cx, "the compare", |cx| {
        app.read_with(cx, |app, cx| !app.viewer.read(cx).is_loading())
    });
    // A render the listing can't read stays.
    let unreadable = root.join("Renders").join(format!("{:032x}", 7));
    std::fs::create_dir_all(&unreadable).unwrap();
    std::fs::write(unreadable.join("render.json"), "not json").unwrap();
    assert_eq!(
        cx.update(|cx| library_delete::deletable(cx)),
        Deletable::Ready { stacks: 1, renders: 2 }
    );
    // Not while Settings names another folder: that one opens next time.
    let other = dir.path().join("other");
    cx.update(|cx| crate::prefs::update_prefs(cx, |p| p.history_dir = Some(other)));
    assert_eq!(
        cx.update(|cx| library_delete::deletable(cx)),
        Deletable::Moved { open: root.clone() }
    );
    cx.update(library_delete::delete_library);
    cx.run_until_parked();
    assert!(active(cx, &app).is_some(), "nothing deleted");
    assert!(app.read_with(cx, |app, cx| !app.renders.read(cx).is_deleting()));
    cx.update(|cx| crate::prefs::update_prefs(cx, |p| p.history_dir = None));

    cx.update(library_delete::delete_library);
    wait_until(cx, "the delete", |cx| {
        app.read_with(cx, |app, cx| {
            app.renders.read(cx).renders().is_empty() && !app.renders.read(cx).is_deleting()
        })
    });
    app.read_with(cx, |app, cx| {
        assert_eq!(app.screen, Screen::Library);
        assert!(app.library.read(cx).is_blank(cx), "the welcome");
        assert!(app.is_empty(cx));
        assert_eq!(app.files.read(cx).rows().count(), 0);
        assert!(app.base.is_none());
        assert!(!app.has_result(cx));
        assert!(!app.viewer.read(cx).has_compare());
        assert!(!app.is_retouching());
    });
    assert_eq!(cx.update(|cx| library_delete::deletable(cx)), Deletable::Empty);
    let left: Vec<_> = std::fs::read_dir(root.join("Renders")).unwrap().flatten().collect();
    assert_eq!(left.len(), 1, "only the render the listing can't read");
    assert!(unreadable.join("render.json").exists());
    assert!(paths.iter().all(|p| p.exists()), "the frames stay");
}

/// Undo of a delete during a render's save restores it and the save lands
/// too: Library writes queue, none is dropped.
#[gpui::test]
fn undo_while_a_save_runs_restores_and_the_save_lands(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    stack(cx, window, &app);
    stack(cx, window, &app);
    let first = app.read_with(cx, |app, cx| app.renders.read(cx).renders()[0].id.clone());
    let gone = first.clone();
    act(cx, window, &app, |app, window, cx| {
        app.renders.update(cx, |r, cx| r.delete(vec![gone], window, cx))
    });
    wait_until(cx, "the delete", |cx| {
        app.read_with(cx, |app, cx| {
            let renders = app.renders.read(cx);
            !renders.is_deleting() && renders.find(&first).is_none()
        })
    });
    let ids: Vec<_> = app.read_with(cx, |app, cx| {
        app.renders.read(cx).store().deleted().unwrap().into_iter().map(|d| d.id).collect()
    });
    assert_eq!(ids.len(), 1);

    // A third stack, its save held at its encode.
    let held = crate::work::gate::hold("render-encode");
    act(cx, window, &app, |app, window, cx| app.run_stacking(window, cx));
    wait_until(cx, "the save, held", |_| held.waiting() == 1);
    assert!(app.read_with(cx, |app, cx| app.renders.read(cx).is_saving()));
    act(cx, window, &app, |app, window, cx| {
        app.renders.update(cx, |r, cx| r.restore(ids, window, cx))
    });
    held.open();
    wait_until(cx, "the save and the restore", |cx| {
        app.read_with(cx, |app, cx| {
            let renders = app.renders.read(cx);
            !app.is_processing(cx)
                && !renders.is_saving()
                && !renders.is_deleting()
                && renders.renders().len() == 3
                && renders.find(&first).is_some()
        })
    });
    let listed = app.read_with(cx, |app, cx| app.renders.read(cx).store().list().unwrap());
    assert_eq!(listed.len(), 3, "both on disk");
}
