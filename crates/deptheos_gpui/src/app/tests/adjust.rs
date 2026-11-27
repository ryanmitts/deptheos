//! Adjust: its mode, the slider panel's changes, Before, the eyedropper,
//! the flat preview and Copy and Paste Adjustments.

use deptheos_app::adjust::Slider;
use deptheos_app::adjust::machine::Phase;
use gpui::{MouseButton, VisualTestContext, px};

use super::*;
use crate::adjust::AdjustTool;

/// What Adjust says: the render, ready, and its undo and redo steps.
fn seen(cx: &mut TestAppContext, app: &Entity<AppView>) -> Option<(RenderId, bool, [usize; 2])> {
    app.read_with(cx, |app, cx| {
        let adjust = app.adjust.read(cx);
        let status = adjust.status()?;
        Some((status.key.clone(), status.phase == Phase::Ready, adjust.steps()))
    })
}

fn value(cx: &mut TestAppContext, app: &Entity<AppView>, slider: Slider) -> Option<f64> {
    app.read_with(cx, |app, cx| app.adjust.read(cx).value(slider))
}

fn steps(cx: &mut TestAppContext, app: &Entity<AppView>) -> [usize; 2] {
    seen(cx, app).map_or([0, 0], |s| s.2)
}

/// A stack, then Adjust on its render by its key, ready.
fn adjusting(cx: &mut TestAppContext) -> Option<(Fixture, RenderId)> {
    let fixture = Fixture::launch(cx, 3)?;
    let (window, app) = (fixture.window, fixture.app.clone());
    cx.update(|cx| cx.bind_keys(key_bindings()));
    stack(cx, window, &app);
    let render = active(cx, &app).unwrap();
    cx.simulate_keystrokes(window, "a");
    wait_until(cx, "Adjust ready", |cx| seen(cx, &app).is_some_and(|s| s.1));
    // The first settled picture, before any change.
    wait_until(cx, "the first settle", |cx| {
        app.read_with(cx, |app, cx| {
            app.viewer
                .read(cx)
                .adjusting()
                .is_some_and(|a| a.picture == deptheos_app::view::Picture::Settled)
        })
    });
    Some((fixture, render))
}

/// Draw the window, so the panel's bounds are known.
fn drawn(cx: &mut TestAppContext, window: gpui::AnyWindowHandle) -> VisualTestContext {
    cx.update_window(window, |_, window, _| window.refresh()).unwrap();
    cx.run_until_parked();
    VisualTestContext::from_window(window, cx)
}

/// A by A, Esc leaves; the menu, the key context and both sessions follow.
#[gpui::test]
fn a_opens_adjust_and_escape_leaves(cx: &mut TestAppContext) {
    let Some((Fixture { dir: _dir, window, app, .. }, render)) = adjusting(cx) else { return };
    assert_eq!(seen(cx, &app).map(|s| s.0), Some(render));
    app.read_with(cx, |app, cx| {
        assert!(app.adjusting().is_some());
        let menu = app.menu_state(cx);
        assert!(menu.adjusting && !menu.can_undo);
    });
    assert!(drawn(cx, window).debug_bounds("adjust-histogram").is_some(), "the panel is shown");
    cx.simulate_keystrokes(window, "escape");
    wait_until(cx, "Adjust left", |cx| {
        app.read_with(cx, |app, cx| app.adjust.read(cx).session().adjusting().is_none())
    });
    app.read_with(cx, |app, cx| {
        assert!(app.adjusting().is_none());
        assert!(app.viewer.read(cx).adjusting().is_none());
    });
}

/// A drag of Exposure's track sets it on every move and commits once.
#[gpui::test]
fn a_slider_drag_commits_once(cx: &mut TestAppContext) {
    let Some((Fixture { dir: _dir, window, app, .. }, _)) = adjusting(cx) else { return };
    let mut vcx = drawn(cx, window);
    let row = vcx.debug_bounds("adjust-exposure").expect("Exposure's row");
    // The track: the row's bottom, from its middle rightward.
    let y = row.bottom() - px(6.0);
    let at = |f: f32| gpui::point(row.left() + row.size.width * f, y);
    vcx.simulate_mouse_down(at(0.5), MouseButton::Left, gpui::Modifiers::none());
    for f in [0.55, 0.6, 0.65, 0.7] {
        vcx.simulate_mouse_move(at(f), Some(MouseButton::Left), gpui::Modifiers::none());
    }
    vcx.simulate_mouse_up(at(0.7), MouseButton::Left, gpui::Modifiers::none());
    vcx.run_until_parked();
    wait_until(cx, "the drag committed", |cx| steps(cx, &app) == [1, 0]);
    let exposure = value(cx, &app, Slider::Exposure).unwrap();
    assert!(exposure > 0.5, "dragged right: {exposure}");
}

/// → nudges the slider picked by a step, ⇧→ by ten, each one change; a
/// typed value is one change too.
#[gpui::test]
fn arrow_keys_and_a_typed_value_commit(cx: &mut TestAppContext) {
    let Some((Fixture { dir: _dir, window, app, .. }, _)) = adjusting(cx) else { return };
    act(cx, window, &app, |app, _, cx| {
        app.adjust.update(cx, |a, cx| a.select(Slider::Exposure, cx))
    });
    cx.simulate_keystrokes(window, "right");
    wait_until(cx, "a step", |cx| steps(cx, &app)[0] == 1);
    assert!((value(cx, &app, Slider::Exposure).unwrap() - 0.01).abs() < 1e-9);
    cx.simulate_keystrokes(window, "shift-right");
    wait_until(cx, "ten steps", |cx| steps(cx, &app)[0] == 2);
    assert!((value(cx, &app, Slider::Exposure).unwrap() - 0.11).abs() < 1e-9);

    // ↓ picks the next slider: Contrast after Exposure.
    cx.simulate_keystrokes(window, "down");
    let picked = app.read_with(cx, |app, cx| app.adjust.read(cx).selected());
    assert_eq!(picked, Some(Slider::Contrast));

    // Click Exposure's value, type, Enter.
    click(cx, window, "adjust-exposure-value");
    let input = app.read_with(cx, |app, cx| app.adjust.read(cx).typing(Slider::Exposure).cloned());
    let input = input.expect("typing Exposure");
    act(cx, window, &app, |_, window, cx| input.update(cx, |i, cx| i.set_value("1.5", window, cx)));
    cx.simulate_keystrokes(window, "enter");
    wait_until(cx, "the typed value", |cx| steps(cx, &app)[0] == 3);
    assert_eq!(value(cx, &app, Slider::Exposure), Some(1.5));
    let typing =
        app.read_with(cx, |app, cx| app.adjust.read(cx).typing(Slider::Exposure).is_some());
    assert!(!typing, "done typing");

    // Esc drops a value being typed, and Adjust stays open.
    click(cx, window, "adjust-exposure-value");
    cx.simulate_keystrokes(window, "escape");
    cx.run_until_parked();
    assert_eq!(steps(cx, &app)[0], 3, "nothing set");
    assert!(app.read_with(cx, |app, _| app.adjusting().is_some()));
}

/// A double-click on a slider's name resets it; Reset All resets every one;
/// ⌘Z and ⇧⌘Z go through Adjust's own steps.
#[gpui::test]
fn reset_reset_all_undo_and_redo(cx: &mut TestAppContext) {
    let Some((Fixture { dir: _dir, window, app, .. }, _)) = adjusting(cx) else { return };
    let set = |cx: &mut TestAppContext, slider: Slider, v: f64| {
        act(cx, window, &app, |app, _, cx| {
            app.adjust.update(cx, |a, cx| {
                a.set(slider, v, cx);
                a.commit();
            })
        });
    };
    set(cx, Slider::Exposure, 1.0);
    set(cx, Slider::Contrast, 20.0);
    wait_until(cx, "two changes", |cx| steps(cx, &app)[0] == 2);

    let mut vcx = drawn(cx, window);
    let label = vcx.debug_bounds("adjust-exposure-label").expect("Exposure's name");
    vcx.simulate_event(gpui::MouseDownEvent {
        button: MouseButton::Left,
        position: label.center(),
        modifiers: gpui::Modifiers::none(),
        click_count: 2,
        first_mouse: false,
    });
    vcx.simulate_event(gpui::MouseUpEvent {
        button: MouseButton::Left,
        position: label.center(),
        modifiers: gpui::Modifiers::none(),
        click_count: 2,
    });
    wait_until(cx, "Exposure reset", |cx| value(cx, &app, Slider::Exposure) == Some(0.0));
    assert_eq!(value(cx, &app, Slider::Contrast), Some(20.0), "only Exposure");

    click(cx, window, "adjust-reset-all");
    wait_until(cx, "all reset", |cx| value(cx, &app, Slider::Contrast) == Some(0.0));
    let [undo, _] = steps(cx, &app);

    cx.simulate_keystrokes(window, "cmd-z");
    wait_until(cx, "undone", |cx| value(cx, &app, Slider::Contrast) == Some(20.0));
    assert_eq!(steps(cx, &app), [undo - 1, 1]);
    cx.simulate_keystrokes(window, "cmd-shift-z");
    wait_until(cx, "redone", |cx| value(cx, &app, Slider::Contrast) == Some(0.0));
    let menu = app.read_with(cx, |app, cx| app.menu_state(cx));
    assert_eq!(menu.undo.as_deref(), Some("Adjustment"));
    assert!(menu.can_undo && !menu.can_redo);
}

/// \ held, or the Before button held, shows the render without its
/// adjustments; let go, the adjusted one again.
#[gpui::test]
fn before_shows_while_held(cx: &mut TestAppContext) {
    let Some((Fixture { dir: _dir, window, app, .. }, _)) = adjusting(cx) else { return };
    wait_until(cx, "the picture before", |cx| {
        app.read_with(cx, |app, cx| app.viewer.read(cx).has_before())
    });
    let holding =
        |cx: &mut TestAppContext| app.read_with(cx, |app, cx| app.viewer.read(cx).holding_before());
    cx.dispatch_action(window, HoldBefore);
    assert!(holding(cx), "held");
    // The key's release.
    act(cx, window, &app, |app, _, cx| app.hold_before(false, cx));
    assert!(!holding(cx), "let go");

    let mut vcx = drawn(cx, window);
    let button = vcx.debug_bounds("adjust-before").expect("Before");
    vcx.simulate_mouse_down(button.center(), MouseButton::Left, gpui::Modifiers::none());
    assert!(holding(cx), "the button held");
    let mut vcx = VisualTestContext::from_window(window, cx);
    vcx.simulate_mouse_up(button.center(), MouseButton::Left, gpui::Modifiers::none());
    assert!(!holding(cx), "the button let go");
}

/// W picks the eyedropper; a click on the photo reaches the session, which
/// sets the white balance or says why not, and the tool goes.
#[gpui::test]
fn the_eyedropper_click_reaches_the_session(cx: &mut TestAppContext) {
    let Some((Fixture { dir: _dir, window, app, .. }, _)) = adjusting(cx) else { return };
    cx.simulate_keystrokes(window, "w");
    let tool = |cx: &mut TestAppContext| app.read_with(cx, |app, cx| app.adjust.read(cx).tool());
    assert_eq!(tool(cx), AdjustTool::Eyedropper);
    // Esc drops the tool first, Adjust stays.
    cx.simulate_keystrokes(window, "escape");
    assert_eq!(tool(cx), AdjustTool::Sliders);
    assert!(app.read_with(cx, |app, _| app.adjusting().is_some()));

    let notices = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| window.notifications(cx).len()).unwrap()
    };
    let before = notices(cx);
    cx.simulate_keystrokes(window, "w");
    let mut vcx = drawn(cx, window);
    let pane = app.read_with(&vcx, |app, cx| app.viewer.read(cx).pane_clips()[0]);
    vcx.simulate_click(pane.center(), gpui::Modifiers::none());
    assert_eq!(tool(cx), AdjustTool::Sliders, "one click, then back to the sliders");
    wait_until(cx, "the white balance set, or a notice why not", |cx| {
        steps(cx, &app)[0] == 1 || notices(cx) > before
    });
}

/// A tick's preview lands as one flat picture over the tiles, which stay
/// the same picture; the settled one replaces the tiles and the preview.
#[gpui::test]
fn the_preview_is_flat_and_the_settle_replaces_the_tiles(cx: &mut TestAppContext) {
    let Some((Fixture { dir: _dir, window, app, .. }, _)) = adjusting(cx) else { return };
    let viewer = app.read_with(cx, |app, _| app.viewer.clone());
    let tiles = |cx: &mut TestAppContext| viewer.read_with(cx, |v, _| v.primary_identity());
    let flat = |cx: &mut TestAppContext| viewer.read_with(cx, |v, _| v.flat().is_some());
    let settled = tiles(cx);
    act(cx, window, &app, |app, _, cx| {
        app.adjust.update(cx, |a, cx| a.set(Slider::Exposure, 0.5, cx))
    });
    wait_until(cx, "a preview", flat);
    assert_eq!(tiles(cx), settled, "the tiles aren't rebuilt for a tick");
    act(cx, window, &app, |app, _, cx| {
        app.adjust.update(cx, |a, cx| a.set(Slider::Exposure, 0.8, cx))
    });
    cx.run_until_parked();
    assert_eq!(tiles(cx), settled, "nor for the next");
    act(cx, window, &app, |app, _, cx| app.adjust.update(cx, |a, _| a.commit()));
    wait_until(cx, "the settle", |cx| !flat(cx) && tiles(cx) != settled);
}

/// Copy Adjustments on one render and Paste Adjustments on another, in
/// Adjust and outside it.
#[gpui::test]
fn copy_and_paste_between_renders(cx: &mut TestAppContext) {
    let Some((Fixture { dir: _dir, window, app, .. }, first)) = adjusting(cx) else { return };
    act(cx, window, &app, |app, _, cx| {
        app.adjust.update(cx, |a, cx| {
            a.set(Slider::Exposure, 1.25, cx);
            a.commit();
        })
    });
    wait_until(cx, "set", |cx| steps(cx, &app)[0] == 1);
    cx.simulate_keystrokes(window, "cmd-shift-c");
    assert!(cx.update(|cx| crate::adjust::copied(cx)).is_some_and(|a| a.exposure == 1.25));
    cx.simulate_keystrokes(window, "escape");
    wait_until(cx, "Adjust left", |cx| {
        app.read_with(cx, |app, cx| app.adjust.read(cx).session().adjusting().is_none())
    });

    // A second render, pasted onto outside Adjust.
    exclude_first(cx, &app);
    stack(cx, window, &app);
    let second = active(cx, &app).unwrap();
    assert_ne!(second, first);
    cx.simulate_keystrokes(window, "cmd-shift-v");
    let store = crate::test_support::library(cx, &app);
    wait_until(cx, "pasted", |_| {
        store.adjustments_to_export(&second).is_ok_and(|a| a.adjustments.exposure == 1.25)
    });

    // In Adjust: onto the render open, as one change.
    cx.simulate_keystrokes(window, "a");
    wait_until(cx, "Adjust ready", |cx| seen(cx, &app).is_some_and(|s| s.0 == second && s.1));
    assert_eq!(value(cx, &app, Slider::Exposure), Some(1.25), "opened on what was pasted");
    act(cx, window, &app, |app, _, cx| {
        app.adjust.update(cx, |a, cx| {
            a.set(Slider::Exposure, 0.0, cx);
            a.commit();
        })
    });
    wait_until(cx, "set", |cx| value(cx, &app, Slider::Exposure) == Some(0.0));
    cx.simulate_keystrokes(window, "cmd-shift-v");
    wait_until(cx, "pasted in Adjust", |cx| value(cx, &app, Slider::Exposure) == Some(1.25));
}
