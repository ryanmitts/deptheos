//! The menus and the shortcuts window.

use super::*;

/// The names of a menu's items, as the menu bar shows them, with whether
/// each is dimmed.
fn menu_items(state: &MenuState, menu: &str) -> Vec<(String, bool)> {
    let menus = menus(state, &crate::words::DesktopWords::english());
    let menu = menus.iter().find(|m| m.name.as_ref() == menu).expect("the menu");
    menu.items
        .iter()
        .filter_map(|item| match item {
            gpui::MenuItem::Action { name, disabled, .. } => Some((name.to_string(), *disabled)),
            _ => None,
        })
        .collect()
}

/// Help has Save Diagnostics… and Show Log Folder; the folder's item dims
/// when this launch has no log file.
#[test]
fn help_has_the_diagnostics_items() {
    let item = |log_folder: bool, name: &str| {
        let state = MenuState { log_folder, ..MenuState::default() };
        menu_items(&state, "Help").into_iter().find(|(n, _)| n == name).map(|(_, dim)| dim)
    };
    assert_eq!(item(false, "Save Diagnostics…"), Some(false));
    assert_eq!(item(false, "Show Log Folder"), Some(true));
    assert_eq!(item(true, "Show Log Folder"), Some(false));
}

/// Help's Try a Sample Stack: a submenu of both samples by name, dimmed
/// while it can't act; one item for one sample, and for none (opening the
/// website).
#[test]
fn help_has_the_samples() {
    use crate::app::sample::SampleStack;
    let help = |sample_stacks: &[SampleStack], can_try_sample: bool| {
        let state = MenuState {
            can_try_sample,
            sample_stacks: sample_stacks.to_vec(),
            ..MenuState::default()
        };
        let menus = menus(&state, &crate::words::DesktopWords::english());
        let help = menus.into_iter().find(|m| m.name.as_ref() == "Help").expect("Help");
        help.items
            .into_iter()
            .find_map(|item| match item {
                gpui::MenuItem::Action { name, action, disabled, .. }
                    if name.as_ref() == "Try a Sample Stack" =>
                {
                    let action = action.as_any().downcast_ref::<TrySample>().copied();
                    Some(Err((action.expect("TrySample"), disabled)))
                }
                gpui::MenuItem::Submenu(menu) if menu.name.as_ref() == "Try a Sample Stack" => {
                    Some(Ok(menu
                        .items
                        .into_iter()
                        .map(|item| match item {
                            gpui::MenuItem::Action { name, action, disabled, .. } => {
                                let action = action.as_any().downcast_ref::<TrySample>().copied();
                                (name.to_string(), action.expect("TrySample"), disabled)
                            }
                            _ => panic!("only the samples"),
                        })
                        .collect::<Vec<_>>()))
                }
                _ => None,
            })
            .expect("Try a Sample Stack")
    };
    let both = [SampleStack::Hornet, SampleStack::CircuitBoard];
    let hornet = ("Hornet".to_string(), TrySample(Some(SampleStack::Hornet)));
    let board = ("Circuit Board".to_string(), TrySample(Some(SampleStack::CircuitBoard)));
    let with = |(name, action): &(String, TrySample), dim| (name.clone(), *action, dim);
    assert_eq!(help(&both, true), Ok(vec![with(&hornet, false), with(&board, false)]));
    assert_eq!(help(&both, false), Ok(vec![with(&hornet, true), with(&board, true)]));
    assert_eq!(
        help(&[SampleStack::CircuitBoard], true),
        Err((TrySample(Some(SampleStack::CircuitBoard)), false))
    );
    assert_eq!(help(&[], true), Err((TrySample(None), false)));
}

/// The full screen item says which way it goes.
#[test]
fn full_screen_says_enter_or_exit() {
    let names = |fullscreen: bool| {
        let state = MenuState { fullscreen, ..MenuState::default() };
        menu_items(&state, "View").into_iter().map(|(n, _)| n).collect::<Vec<_>>()
    };
    assert!(names(false).iter().any(|n| n == "Enter Full Screen"));
    assert!(names(true).iter().any(|n| n == "Exit Full Screen"));
    assert!(!names(true).iter().any(|n| n == "Enter Full Screen"));
}

/// Items dim when they can't act: zoom with no image, Stack with nothing
/// to stack, Swap Sides with nothing compared.
#[test]
fn items_that_cannot_act_dim() {
    let dim = |state: &MenuState, menu: &str, name: &str| {
        menu_items(state, menu).into_iter().find(|(n, _)| n == name).map(|(_, dim)| dim)
    };
    let empty = MenuState::default();
    let ready = MenuState { has_image: true, can_stack: true, comparing: true, ..empty.clone() };
    for (menu, name) in [
        ("View", "Zoom In"),
        ("View", "Zoom Out"),
        ("View", "Zoom to Fit"),
        ("View", "Actual Size"),
        ("View", "Swap Sides"),
        ("Stack", "Stack"),
    ] {
        assert_eq!(dim(&empty, menu, name), Some(true), "{name} dims");
        assert_eq!(dim(&ready, menu, name), Some(false), "{name} acts");
    }
}

/// During a run, what can't start dims (New Stack, New Batch…, Add Frames…,
/// Add Folder…, Export Grow Sequence…); Export… and Retouch dim when inert.
#[test]
fn items_that_wait_for_a_run_dim_while_it_runs() {
    let dim = |state: &MenuState, menu: &str, name: &str| {
        menu_items(state, menu).into_iter().find(|(n, _)| n == name).map(|(_, dim)| dim)
    };
    let idle =
        MenuState { has_result: true, can_export: true, can_retouch: true, ..MenuState::default() };
    let running =
        MenuState { running: true, can_export: false, can_retouch: false, ..idle.clone() };
    for (menu, name) in [
        ("File", "New Stack"),
        ("File", "New Batch…"),
        ("File", "Add Frames…"),
        ("File", "Add Folder…"),
        ("File", "Export…"),
        ("File", "Export Again"),
        ("File", "Export Grow Sequence…"),
        ("Stack", "Retouch"),
    ] {
        assert_eq!(dim(&idle, menu, name), Some(false), "{name} acts");
        assert_eq!(dim(&running, menu, name), Some(true), "{name} dims during a run");
    }
}

/// The menu bar follows the window: Revert names its render and dims until
/// a change, the map item flips, Include All dims with nothing excluded,
/// Swap Sides needs a comparison and zoom an image; ⌘S is free.
#[gpui::test]
fn the_menus_follow_the_window(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    cx.update(|cx| cx.bind_keys(key_bindings()));
    stack(cx, window, &app);
    let state = |cx: &mut TestAppContext| app.read_with(cx, |app, cx| app.menu_state(cx));
    let item = |state: &MenuState, menu: &str, name: &str| {
        menu_items(state, menu).into_iter().find(|(n, _)| n == name)
    };

    let s = state(cx);
    assert_eq!(s.layout, Some(0), "Result is checked");
    assert_eq!(item(&s, "Stack", "Revert to #1"), Some(("Revert to #1".into(), true)));
    assert_eq!(item(&s, "View", "Show Depth Map").map(|i| i.1), Some(false));
    assert_eq!(item(&s, "Stack", "Include All Frames").map(|i| i.1), Some(true));
    assert_eq!(item(&s, "Edit", "Undo").map(|i| i.1), Some(true), "Retouch only");
    assert_eq!(item(&s, "Stack", "Stack").map(|i| i.1), Some(false));
    assert_eq!(item(&s, "View", "Actual Size").map(|i| i.1), Some(false));
    assert_eq!(item(&s, "View", "Swap Sides").map(|i| i.1), Some(true), "nothing compared");
    act(cx, window, &app, |app, window, cx| app.set_view(Look::SideBySide, window, cx));
    wait_until(cx, "the compare source", |cx| {
        app.read_with(cx, |app, cx| app.viewer.read(cx).has_compare())
    });
    let s = state(cx);
    assert_eq!(item(&s, "View", "Swap Sides").map(|i| i.1), Some(false), "comparing");
    act(cx, window, &app, |app, window, cx| app.set_view(Look::Result, window, cx));

    exclude_first(cx, &app);
    cx.simulate_keystrokes(window, "m");
    cx.run_until_parked();
    let s = state(cx);
    assert_eq!(item(&s, "Stack", "Revert to #1").map(|i| i.1), Some(false), "edited");
    assert!(item(&s, "View", "Hide Depth Map").is_some());
    assert_eq!(item(&s, "Stack", "Include All Frames").map(|i| i.1), Some(false));
    // The window keeps the state it set the menu bar from.
    app.read_with(cx, |app, cx| assert_eq!(app.menu_state, app.menu_state(cx)));

    // Include All, from the menu's action.
    cx.update_window(window, |_, window, cx| {
        window.dispatch_action(Box::new(IncludeAllFrames), cx)
    })
    .unwrap();
    app.read_with(cx, |app, cx| assert!(app.files.read(cx).all_included()));

    cx.simulate_keystrokes(window, "secondary-s");
    cx.run_until_parked();
    app.read_with(cx, |app, _| assert!(export_sheet(app).is_none(), "⌘S is free"));

    // The Library has no image to zoom and no stack to start.
    cx.simulate_keystrokes(window, "secondary-alt-2");
    cx.run_until_parked();
    let s = state(cx);
    assert!(s.library);
    assert_eq!(item(&s, "View", "Zoom In").map(|i| i.1), Some(true));
    assert_eq!(item(&s, "Stack", "Stack").map(|i| i.1), Some(true));
}

/// The ≡ menu (Windows and Linux) opens and closes; macOS shows no button.
#[gpui::test]
fn the_window_menu_opens(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 0) else { return };
    let toggle = |cx: &mut TestAppContext| {
        act(cx, window, &app, |app, window, cx| app.toggle_window_menu(window, cx));
        cx.run_until_parked();
    };
    toggle(cx);
    app.read_with(cx, |app, _| assert!(app.window_menu.is_some()));
    toggle(cx);
    app.read_with(cx, |app, _| assert!(app.window_menu.is_none()));
}

/// Keyboard Shortcuts stays inside the window: groups scroll under a fixed
/// title, the last row is reachable, no description overflows.
#[gpui::test]
fn the_shortcuts_window_fits_and_scrolls(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 0) else { return };
    // Shorter than the list.
    gpui::VisualTestContext::from_window(window, cx)
        .simulate_resize(gpui::size(px(1280.0), px(560.0)));
    act(cx, window, &app, |app, window, cx| app.show_shortcuts(window, cx));
    let rows = cx.update(|cx| crate::app::menu::shortcut_rows(cx.wording()));
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    vcx.run_until_parked();
    let dialog = vcx.debug_bounds("dialog-0").expect("the dialog");
    let height = vcx.update(|window, _| window.viewport_size().height);
    assert!(dialog.bottom() <= height, "the dialog ran off the window: {dialog:?}");
    for row in 0..rows {
        let what = vcx.debug_bounds(format!("shortcut-what-{row}").leak()).expect("a row");
        assert!(what.right() <= dialog.right(), "row {row} runs past the dialog: {what:?}");
    }
    // Selectors are static; a test may leak these few.
    let last: &str = format!("shortcut-what-{}", rows - 1).leak();
    let title = vcx.debug_bounds("shortcuts-title").expect("the title");
    assert!(vcx.debug_bounds(last).unwrap().bottom() > dialog.bottom(), "out of view at first");
    {
        let position = dialog.center();
        vcx.simulate_event(gpui::ScrollWheelEvent {
            position,
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.0), px(-5000.0))),
            ..Default::default()
        });
        vcx.run_until_parked();
    }
    let end = vcx.debug_bounds(last).unwrap();
    assert!(end.bottom() <= dialog.bottom(), "the last row can't be reached: {end:?}");
    assert_eq!(vcx.debug_bounds("shortcuts-title").unwrap(), title, "the title stays put");
}

/// Alt let go alone is a tap (the ≡ menu off macOS); a mouse press while
/// it is down, as Alt-click on the photo, makes it none.
#[gpui::test]
fn a_mouse_press_ends_an_alt_tap(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 2) else { return };
    stack(cx, window, &app);
    let bar = bar_part(cx, window, "view-bar").expect("the bar");
    let photo = gpui::point(bar.center().x, bar.top() - px(80.0));
    let alt = gpui::Modifiers { alt: true, ..Default::default() };
    let down = |cx: &mut TestAppContext| app.read_with(cx, |app, _| app.alt_down);
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    vcx.simulate_modifiers_change(alt);
    assert!(down(cx), "Alt alone");
    // Over the photo, and over a control that occludes it.
    for at in [photo, bar.center()] {
        let mut vcx = gpui::VisualTestContext::from_window(window, cx);
        vcx.simulate_modifiers_change(alt);
        vcx.simulate_mouse_down(at, gpui::MouseButton::Left, alt);
        assert!(!down(cx), "a press at {at:?} ends the tap");
        let mut vcx = gpui::VisualTestContext::from_window(window, cx);
        vcx.simulate_mouse_up(at, gpui::MouseButton::Left, alt);
        vcx.simulate_modifiers_change(Default::default());
    }
    if crate::app::window_menu::HAS_WINDOW_MENU {
        assert!(app.read_with(cx, |app, _| app.window_menu.is_none()), "no menu opened");
    }
}
