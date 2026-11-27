//! The window: its title, bars, the welcome and narrow sizes.

use super::*;

/// Digits typed in the zoom field are text, not the view keys (0 is
/// Zoom to Fit), and Enter zooms to them and hands the keys back.
#[gpui::test]
fn the_zoom_field_takes_a_percentage(cx: &mut TestAppContext) {
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
    cx.update(|cx| cx.bind_keys(key_bindings()));
    wait_until(cx, "the preview", |cx| {
        app.read_with(cx, |app, cx| app.viewer.read(cx).has_content())
    });
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        // Clicked into, the field selects its value ("Fit"): typing
        // replaces it.
        let input = app.read(cx).zoom_input.clone();
        input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.simulate_keystrokes(window, "3 0 0");
    app.read_with(cx, |app, cx| {
        assert_eq!(app.zoom_input.read(cx).value().as_ref(), "300");
    });
    cx.simulate_keystrokes(window, "enter");
    cx.run_until_parked();
    app.read_with(cx, |app, cx| {
        assert_eq!(app.viewer.read(cx).zoom_percent(), 300);
        assert_eq!(app.zoom_input.read(cx).value().as_ref(), "300%");
    });
    // The view keys work again: 0 is Zoom to Fit, ⌘1 Actual Size and ⌘0
    // Zoom to Fit again.
    cx.simulate_keystrokes(window, "0");
    cx.run_until_parked();
    app.read_with(cx, |app, cx| assert!(app.viewer.read(cx).is_fitted()));
    cx.simulate_keystrokes(window, "secondary-1");
    cx.run_until_parked();
    app.read_with(cx, |app, cx| assert_eq!(app.viewer.read(cx).zoom_percent(), 100));
    cx.simulate_keystrokes(window, "secondary-0");
    cx.run_until_parked();
    app.read_with(cx, |app, cx| assert!(app.viewer.read(cx).is_fitted()));
}

/// The welcome's steps name the keys bound to Stack and Export, as this
/// platform writes them.
#[gpui::test]
fn the_welcome_names_the_bound_keys(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let (window, _app) = launch(cx, &engine, Vec::new(), &dir.path().join("history"));
    cx.update(|cx| cx.bind_keys(key_bindings()));
    cx.run_until_parked();
    let (stack, export) = cx
        .update_window(window, |_, window, _| {
            (
                bound_keys(&RunStack, KEY_CONTEXT, window),
                bound_keys(&SaveResult, KEY_CONTEXT, window),
            )
        })
        .unwrap();
    let (want_stack, want_export) =
        if cfg!(target_os = "macos") { ("⌘⏎", "⌘E") } else { ("Ctrl+Enter", "Ctrl+E") };
    assert_eq!((stack.as_str(), export.as_str()), (want_stack, want_export));
    let steps =
        crate::app::welcome::step_texts(&crate::words::DesktopWords::english(), &stack, &export);
    assert!(steps[1].1.ends_with(&format!("Click Stack ({want_stack}).")), "{}", steps[1].1);
    assert!(steps[2].1.ends_with(&format!("Export ({want_export}).")), "{}", steps[2].1);
}

/// A blank Library's welcome leads with the app's icon, and all of it fits
/// the smallest window; an empty stack's welcome has no icon.
#[gpui::test]
fn the_blank_librarys_welcome_shows_the_icon_and_fits(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 0) else { return };
    act(cx, window, &app, |app, window, cx| app.set_screen(Screen::Library, window, cx));
    cx.simulate_window_resize(window, gpui::size(px(900.0), px(600.0)));
    cx.run_until_parked();
    app.read_with(cx, |app, cx| assert!(app.is_empty(cx), "the welcome"));
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    let icon = vcx.debug_bounds("welcome-icon").expect("the icon leads the welcome");
    let last = vcx.debug_bounds("welcome-batch-line").expect("the batch line ends it");
    let title_bar = px(crate::app::toolbar::TITLE_BAR_CONTROL_HEIGHT);
    assert_eq!(icon.size.height, px(crate::app::welcome::WELCOME_ICON_SIZE));
    assert!(icon.top() > title_bar, "below the title bar: {icon:?}");
    assert!(last.bottom() < px(600.0), "nothing clipped: {last:?}");
    // Try a sample stack offers the repository's two, by name and line.
    let words = crate::words::DesktopWords::english();
    let choices = app.read_with(&vcx, |app, _| app.sample_choices(&words));
    use crate::app::sample::SampleStack::{CircuitBoard, Hornet};
    assert_eq!(
        choices,
        [
            (Hornet, "Hornet", "66 frames, handheld".to_string()),
            (CircuitBoard, "Circuit Board", "200 frames, on a tripod".to_string()),
        ]
    );
    // Centered over the welcome's own middle.
    let mid = |b: gpui::Bounds<gpui::Pixels>| b.left() + b.size.width / 2.0;
    assert!((mid(icon) - mid(last)).abs() < px(1.0), "{icon:?} {last:?}");

    act(cx, window, &app, |app, window, cx| app.set_screen(Screen::Stacking, window, cx));
    app.read_with(cx, |app, cx| assert!(app.is_empty(cx), "an empty stack's welcome"));
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    assert!(vcx.debug_bounds("welcome-batch-line").is_some());
    assert!(vcx.debug_bounds("welcome-icon").is_none(), "no icon for an empty stack");
}

/// Every control in the title bar is one height. An empty Library keeps
/// them all in place, and they stay put when the first stack appears.
#[gpui::test]
fn the_title_bar_controls_are_one_height_and_stay_put(cx: &mut TestAppContext) {
    let Some(Fixture { dir, window, app, .. }) = Fixture::launch(cx, 0) else { return };
    let height = px(crate::app::toolbar::TITLE_BAR_CONTROL_HEIGHT);
    let library = ["library-search", "new-stack", "new-batch", "library-open", "toggle-details"];
    let bounds = |cx: &mut TestAppContext, names: &[&'static str]| {
        let mut vcx = gpui::VisualTestContext::from_window(window, cx);
        names
            .iter()
            .map(|name| vcx.debug_bounds(name).unwrap_or_else(|| panic!("{name} is in the bar")))
            .collect::<Vec<_>>()
    };
    act(cx, window, &app, |app, window, cx| app.set_screen(Screen::Library, window, cx));
    app.read_with(cx, |app, cx| assert!(app.library.read(cx).is_blank(cx)));
    let blank = bounds(cx, &library);
    for (name, b) in library.iter().zip(&blank) {
        assert_eq!(b.size.height, height, "{name}");
    }
    // Nothing to search, open or inspect yet: the inspector toggle, its
    // menu item and its key leave the saved choice alone.
    cx.update(|cx| cx.bind_keys(key_bindings()));
    let inspector_keys = if cfg!(target_os = "macos") { "ctrl-cmd-i" } else { "ctrl-alt-i" };
    let shown = |cx: &mut TestAppContext| app.read_with(cx, |app, _| app.chrome.inspector_chosen());
    let before = shown(cx);
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    vcx.simulate_click(blank[4].center(), gpui::Modifiers::none());
    cx.simulate_keystrokes(window, inspector_keys);
    act(cx, window, &app, |app, window, cx| app.toggle_inspector(window, cx));
    assert_eq!(shown(cx), before, "a blank Library's inspector toggle does nothing");
    app.read_with(cx, |app, cx| assert_eq!(app.menu_state(cx).inspector, None, "dimmed"));

    // The first stack.
    let frames = dir.path().join("frames");
    std::fs::create_dir(&frames).unwrap();
    write_frames(&frames, 3);
    let dropped = ExternalPaths([frames].into_iter().collect());
    act(cx, window, &app, |app, window, cx| app.drop_files(&dropped, window, cx));
    stack(cx, window, &app);
    let stacking =
        ["library-back", "toggle-sidebar", "retouch", "export", "stack", "toggle-inspector"];
    for (name, b) in stacking.iter().zip(bounds(cx, &stacking)) {
        assert_eq!(b.size.height, height, "{name}");
    }
    act(cx, window, &app, |app, window, cx| app.set_screen(Screen::Library, window, cx));
    app.read_with(cx, |app, cx| assert!(!app.library.read(cx).is_blank(cx)));
    assert_eq!(bounds(cx, &library), blank, "the bar doesn't move");
    // With a stack, the Library's inspector has its key, as Stacking's does.
    app.read_with(cx, |app, cx| assert_eq!(app.menu_state(cx).inspector, Some(before)));
    cx.simulate_keystrokes(window, inspector_keys);
    assert_eq!(shown(cx), !before, "the key toggles the Library's inspector");
    cx.simulate_keystrokes(window, inspector_keys);
    assert_eq!(shown(cx), before);
}

/// In a window as narrow as it goes, the Library's buttons keep their
/// width, side by side in order, and only the search field narrows.
#[gpui::test]
fn the_library_bar_keeps_its_buttons_in_a_narrow_window(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 0) else { return };
    act(cx, window, &app, |app, window, cx| app.set_screen(Screen::Library, window, cx));
    let names = ["library-search", "new-stack", "new-batch", "library-open", "toggle-details"];
    let bounds = |cx: &mut TestAppContext, width: f32| {
        cx.simulate_window_resize(window, gpui::size(px(width), px(800.0)));
        cx.run_until_parked();
        let mut vcx = gpui::VisualTestContext::from_window(window, cx);
        names
            .iter()
            .map(|name| vcx.debug_bounds(name).unwrap_or_else(|| panic!("{name} is in the bar")))
            .collect::<Vec<_>>()
    };
    let wide = bounds(cx, 1440.0);
    for width in [1100.0, 1000.0, 900.0] {
        let narrow = bounds(cx, width);
        for (i, name) in names.iter().enumerate().skip(1) {
            assert_eq!(narrow[i].size.width, wide[i].size.width, "{name} at {width}");
        }
        for (pair, b) in names.windows(2).zip(narrow.windows(2)) {
            assert!(b[0].right() <= b[1].left(), "{} overlaps {} at {width}", pair[0], pair[1]);
        }
        assert!(narrow[0].size.width >= px(120.0), "the search at {width}");
        assert!(narrow[4].right() <= px(width), "the bar fits at {width}");
    }
}

#[gpui::test]
fn the_empty_window_and_narrow_windows(cx: &mut TestAppContext) {
    let Some(Fixture { dir, window, app, .. }) = Fixture::launch(cx, 0) else { return };
    cx.run_until_parked();
    app.read_with(cx, |app, cx| {
        assert!(app.is_empty(cx), "first run");
        assert_eq!(app.title_parts(cx).name, None, "titled New stack");
    });

    // Frames: no longer empty.
    let frames = dir.path().join("frames");
    std::fs::create_dir(&frames).unwrap();
    write_frames(&frames, 3);
    let dropped = ExternalPaths([frames].into_iter().collect());
    act(cx, window, &app, |app, window, cx| app.drop_files(&dropped, window, cx));
    wait_until(cx, "the frames", |cx| app.read_with(cx, |app, cx| !app.is_empty(cx)));

    let resize = |cx: &mut TestAppContext, width: f32| {
        cx.simulate_window_resize(window, gpui::size(px(width), px(800.0)));
        cx.run_until_parked();
    };
    resize(cx, 1440.0);
    app.read_with(cx, |app, cx| {
        assert!(!app.narrow() && !app.cramped());
        assert!(!app.renders.read(cx).compact);
    });
    resize(cx, 1280.0);
    app.read_with(cx, |app, cx| {
        assert!(app.narrow() && !app.cramped(), "1280 is narrow");
        assert!(app.renders.read(cx).compact);
        assert!(app.inspector_visible(), "still docked");
    });

    // Below 1100 the inspector floats, hidden until ⌃⌘I.
    resize(cx, 1000.0);
    app.read_with(cx, |app, _| assert!(app.cramped() && !app.inspector_visible()));
    cx.update(|cx| cx.bind_keys(key_bindings()));
    cx.simulate_keystrokes(window, crate::app::actions::PANEL_KEYS[1]);
    cx.run_until_parked();
    app.read_with(cx, |app, cx| {
        assert!(app.inspector_visible());
        assert_eq!(app.menu_state(cx).inspector, Some(true));
        assert!(app.chrome.inspector_chosen(), "the docked choice is kept");
    });
    resize(cx, 1440.0);
    app.read_with(cx, |app, _| assert!(app.inspector_visible()));
}

/// The window is titled after what is open, in the title bar's words, for
/// the Window menu and the window switcher.
#[gpui::test]
fn the_window_is_titled_after_what_is_open(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let history = dir.path().join("history");
    let (window, app) = launch(cx, &engine, Vec::new(), &history);
    let title = |cx: &mut TestAppContext| {
        cx.run_until_parked();
        app.read_with(cx, |app, _| app.window_title.to_string())
    };
    assert_eq!(title(cx), "Library — Deptheos", "the Library is home");
    act(cx, window, &app, |app, window, cx| app.new_stack(window, cx));
    crate::test_support::switched(cx, &app);
    assert_eq!(title(cx), "New stack — Deptheos");
    let bee = dir.path().join("Bee");
    std::fs::create_dir(&bee).unwrap();
    let frames = write_frames(&bee, 2);
    act(cx, window, &app, |app, window, cx| app.add_paths(frames, window, cx));
    wait_until(cx, "the frames", |cx| app.read_with(cx, |app, cx| app.files.read(cx).can_stack()));
    assert_eq!(title(cx), "Bee — Deptheos");
    act(cx, window, &app, |app, window, cx| app.set_screen(Screen::Library, window, cx));
    assert_eq!(title(cx), "Library — Deptheos");
}

/// The About window opens and draws, its icon read from the binary.
#[gpui::test]
fn the_about_window_draws(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 0) else { return };
    act(cx, window, &app, |app, window, cx| app.show_about(window, cx));
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("dialog-0").is_some(), "the About window is open");
}

/// The view bar fits the photo at every width: the rail takes the bar's
/// room, Compare loses its word when narrow, the readout goes when cramped
/// and Paint From shows Frames as a glyph, and the rail never shrinks below
/// its least.
#[gpui::test]
fn the_view_bar_fits_narrow_windows(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    stack(cx, window, &app);
    let fits = |cx: &mut TestAppContext, width: f32, painting: bool| {
        assert_eq!(app.read_with(cx, |app, _| app.is_retouching()), painting);
        cx.simulate_window_resize(window, gpui::size(px(width), px(800.0)));
        cx.run_until_parked();
        let bar = bar_part(cx, window, "view-bar").expect("the bar");
        let rail = bar_part(cx, window, "focus-rail").expect("the rail");
        assert!(bar.right() <= px(width), "{width}: the bar fits ({bar:?})");
        assert!(rail.size.width >= px(crate::app::scrubber::RAIL_MIN), "{width}: {rail:?}");
        // Retouch's layouts are in Compare's place.
        let compare = bar_part(cx, window, "view-compare").unwrap();
        let layouts = bar_part(cx, window, "view-layouts").unwrap();
        assert!(compare.right() <= layouts.left() && rail.right() <= compare.left(), "{painting}");
        (rail, bar_part(cx, window, "rail-readout").is_some())
    };
    let paint_from = |cx: &mut TestAppContext| {
        let label = bar_part(cx, window, "view-paint-from-label").is_some();
        (bar_part(cx, window, "view-paint-from").expect("Paint From"), label)
    };
    for painting in [false, true] {
        if painting {
            wait_until(cx, "the source", |cx| paint_state(cx, &app).source_ready);
        }
        let (wide, readout) = fits(cx, 1600.0, painting);
        assert!(readout && wide.size.width > px(300.0), "the rail takes the room: {wide:?}");
        // It stands within the bar's height.
        let bar = bar_part(cx, window, "view-bar").unwrap();
        assert!(wide.top() >= bar.top() && wide.bottom() <= bar.bottom(), "{wide:?} {bar:?}");
        let (wider, _) = fits(cx, 2400.0, painting);
        assert!(wider.size.width > wide.size.width, "{wider:?} {wide:?}");
        let full = painting.then(|| paint_from(cx));
        if let Some((_, label)) = full {
            assert!(label, "Frames by its word with room");
        }
        let (_, readout) = fits(cx, 1280.0, painting);
        assert!(readout, "narrow keeps the readout");
        let (_, readout) = fits(cx, 1000.0, painting);
        assert!(!readout, "cramped drops it");
        if let Some((full, _)) = full {
            let (compact, label) = paint_from(cx);
            assert!(!label, "Frames by its glyph alone when cramped");
            assert!(compact.size.width < full.size.width, "{compact:?} {full:?}");
        }
        let (_, _) = fits(cx, 760.0, painting);
        act(cx, window, &app, |app, _, cx| app.toggle_paint(cx));
    }
}
