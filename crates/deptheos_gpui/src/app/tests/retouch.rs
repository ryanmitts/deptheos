//! Retouch: painting, its sources and its history.

use super::*;
use crate::app::mode::Mode;

/// While retouching, Revert is off everywhere (Stack ▸ Revert to #n, the
/// change line's button, Revert itself); closing Retouch restores it.
#[gpui::test]
fn revert_is_off_while_retouching(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    cx.update(|cx| cx.bind_keys(key_bindings()));
    stack(cx, window, &app);
    let first = active(cx, &app).unwrap();
    exclude_first(cx, &app);
    let revert = |cx: &mut TestAppContext| app.read_with(cx, |app, cx| app.menu_state(cx).revert);
    assert!(matches!(revert(cx), RevertItem::Revert { number: 1, enabled: true }));

    act(cx, window, &app, |app, _, cx| app.toggle_paint(cx));
    app.read_with(cx, |app, _| assert!(app.is_retouching()));
    assert!(matches!(revert(cx), RevertItem::Revert { number: 1, enabled: false }));
    let line = app.read_with(cx, |app, cx| app.compared(cx)).expect("edited since #1");
    assert!(line.retouching && line.revert == Some(1), "{line:?}");
    act(cx, window, &app, |app, window, cx| app.revert_to(&first, window, cx));
    let summary =
        |cx: &mut TestAppContext| app.read_with(cx, |app, cx| app.files.read(cx).summary());
    assert_eq!(summary(cx).as_ref(), "2 of 3 frames", "nothing reverted");

    act(cx, window, &app, |app, _, cx| app.toggle_paint(cx));
    assert!(matches!(revert(cx), RevertItem::Revert { number: 1, enabled: true }));
    let line = app.read_with(cx, |app, cx| app.compared(cx)).expect("edited since #1");
    assert!(!line.retouching, "{line:?}");
}

/// Retouch's left panel switches between frames and the stack's other
/// renders: a render's row paints from it, and each opening starts on
/// frames. The strip is put away, so a render click always means paint from.
#[gpui::test]
fn retouch_lists_frames_or_renders(cx: &mut TestAppContext) {
    use crate::paint::SourcesTab;
    use deptheos_app::paint::SourceKind;
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    let bounds = |cx: &mut TestAppContext, selector: &'static str| {
        cx.update_window(window, |_, window, _| window.refresh()).unwrap();
        cx.run_until_parked();
        gpui::VisualTestContext::from_window(window, cx).debug_bounds(selector)
    };
    let tab = |cx: &mut TestAppContext| app.read_with(cx, |app, _| app.sources_tab());
    stack(cx, window, &app);
    let first = active(cx, &app).unwrap();
    exclude_first(cx, &app);
    let strip = |cx: &mut TestAppContext| app.read_with(cx, |app, cx| app.strip_shown(cx));
    stack(cx, window, &app);
    assert_eq!(tab(cx), None, "only in Retouch");
    assert!(strip(cx), "the strip outside Retouch");

    act(cx, window, &app, |app, _, cx| app.toggle_paint(cx));
    wait_until(cx, "the source", |cx| paint_state(cx, &app).source_ready);
    assert_eq!(tab(cx), Some(SourcesTab::Frames));
    assert!(!strip(cx), "no strip in Retouch");
    let switch = bounds(cx, "paint-sources").expect("the switch is drawn");
    assert!(bounds(cx, "paint-renders").is_none());

    // Renders: the stack's other render, a click paints from it.
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    let right = gpui::point(switch.right() - switch.size.width / 4.0, switch.center().y);
    vcx.simulate_click(right, gpui::Modifiers::none());
    assert_eq!(tab(cx), Some(SourcesTab::Renders));
    assert!(bounds(cx, "paint-renders").is_some(), "the renders are listed");
    let row: &'static str = Box::leak(format!("paint-render-{}", first.as_str()).into_boxed_str());
    let row = bounds(cx, row).expect("#1's row");
    gpui::VisualTestContext::from_window(window, cx)
        .simulate_click(row.center(), Default::default());
    let from_first = SourceKind::Render { render: first.as_str().to_string() };
    wait_until(cx, "#1 painted from", |cx| {
        paint_state(cx, &app).source == Some(from_first.clone())
    });

    // The view bar's menu of renders picks too.
    app.update(cx, |app, cx| app.paint.update(cx, |p, cx| p.pick_original(cx)));
    wait_until(cx, "the original", |cx| paint_state(cx, &app).source == Some(SourceKind::Original));
    act(cx, window, &app, |app, window, cx| app.compare_with_render(first.clone(), window, cx));
    wait_until(cx, "#1 again", |cx| paint_state(cx, &app).source == Some(from_first.clone()));
    assert_eq!(tab(cx), Some(SourcesTab::Renders), "the list stays as chosen");

    // Done brings the strip back; Retouch again starts on the frames.
    act(cx, window, &app, |app, _, cx| app.toggle_paint(cx));
    assert_eq!(tab(cx), None);
    assert!(strip(cx));
    act(cx, window, &app, |app, _, cx| app.toggle_paint(cx));
    assert_eq!(tab(cx), Some(SourcesTab::Frames));
}

/// Retouch end to end: entering shows the painted result alone (source
/// loaded for \\ and compare); a stroke is one undo entry marking its frame;
/// ⌥-click picks the frame behind that pixel; paint keys override view
/// keys; a render's history survives painting another and coming back.
#[gpui::test]
fn paints_and_keeps_history_across_renders(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else {
        return;
    };
    cx.update(|cx| cx.bind_keys(key_bindings()));
    stack(cx, window, &app);
    let first = active(cx, &app).unwrap();

    cx.simulate_keystrokes(window, "p");
    wait_until(cx, "the source", |cx| {
        let state = paint_state(cx, &app);
        state.open.is_some() && state.source_ready
    });
    app.read_with(cx, |app, cx| {
        assert!(app.is_retouching());
        let viewer = app.viewer.read(cx);
        assert!(viewer.has_compare(), "the source is loaded");
        assert_eq!(viewer.mode(), ViewMode::Result, "one large view");
        assert_eq!(app.painting().unwrap().frames.len(), 3, "it steps through the list");
    });
    assert!(paint_state(cx, &app).open.is_some());

    stroke(cx, &app, 40.0);
    wait_until(cx, "the stroke", |cx| paint_state(cx, &app).undo == 1);
    let state = paint_state(cx, &app);
    assert!(state.painted);
    assert_eq!(state.painted_from.len(), 1, "one frame painted from");
    assert_eq!(state.painted_from.first(), state.source.as_ref());
    cx.run_until_parked();
    app.read_with(cx, |app, cx| {
        let (title, what) = app.retouch_title(cx);
        assert_eq!(title.as_ref(), "Retouching #1");
        assert!(what.ends_with(" · 1 stroke"), "{what}");
    });

    // Held \\ shows the source; Done lets go of it.
    cx.update_window(window, |_, window, cx| {
        window.dispatch_action(Box::new(PeekSource), cx);
    })
    .unwrap();
    app.read_with(cx, |app, cx| assert!(app.viewer.read(cx).peeking()));
    app.update(cx, |app, cx| app.peek_source(false, cx));
    app.read_with(cx, |app, cx| assert!(!app.viewer.read(cx).peeking()));
    // No key-up comes when the window loses focus with \\ down: the hold
    // ends with the focus.
    cx.update_window(window, |_, window, cx| {
        window.dispatch_action(Box::new(PeekSource), cx);
    })
    .unwrap();
    app.read_with(cx, |app, cx| assert!(app.viewer.read(cx).peeking()));
    cx.update_window(window, |_, window, _| window.activate_window()).unwrap();
    cx.run_until_parked();
    app.read_with(cx, |app, cx| assert!(app.viewer.read(cx).peeking(), "still held"));
    gpui::VisualTestContext::from_window(window, cx).deactivate_window();
    app.read_with(cx, |app, cx| assert!(!app.viewer.read(cx).peeking()));
    cx.update_window(window, |_, window, _| window.activate_window()).unwrap();
    cx.run_until_parked();

    // ⌥-click: the frame the result took that pixel from.
    let painted_frame = paint_state(cx, &app).source;
    act(cx, window, &app, |app, window, cx| app.pick_sharpest(100, 60, window, cx));
    let want = app.read_with(cx, |app, cx| {
        let (stack, _) = app.result_doc(cx).unwrap();
        stack.paths[stack.frame_at(100, 60).expect("a frame map")].clone()
    });
    let picked = deptheos_app::paint::SourceKind::Frame { path: want.clone() };
    wait_until(cx, "the picked frame", |cx| paint_state(cx, &app).source.as_ref() == Some(&picked));
    app.read_with(cx, |app, cx| {
        let files = app.files.read(cx);
        assert_eq!(files.selected(cx), files.row_of(&want), "its row is highlighted");
    });
    // Back to the frame painted from, for what follows.
    if let Some(deptheos_app::paint::SourceKind::Frame { path }) = painted_frame {
        app.update(cx, |app, cx| app.paint.update(cx, |p, cx| p.pick_frame(&path, cx)));
    }

    // "5" is opacity in paint mode, not a view key.
    cx.simulate_keystrokes(window, "5");
    app.read_with(cx, |app, cx| {
        let paint = app.paint.read(cx);
        assert!((paint.current_brush(cx).opacity - 0.5).abs() < 1e-6);
        assert_eq!(app.viewer.read(cx).mode(), ViewMode::Result);
    });
    cx.simulate_keystrokes(window, "secondary-z");
    wait_until(cx, "the undo", |cx| paint_state(cx, &app).redo == 1);
    cx.simulate_keystrokes(window, "secondary-shift-z");
    wait_until(cx, "the redo", |cx| paint_state(cx, &app).undo == 1);

    // A second render, painted twice.
    cx.simulate_keystrokes(window, "escape");
    app.read_with(cx, |app, _| assert!(!app.is_retouching()));
    exclude_first(cx, &app);
    stack(cx, window, &app);
    cx.simulate_keystrokes(window, "p");
    wait_until(cx, "the second canvas", |cx| {
        let state = paint_state(cx, &app);
        state.source_ready && state.undo == 0 && state.open.is_some()
    });
    stroke(cx, &app, 60.0);
    stroke(cx, &app, 80.0);
    wait_until(cx, "two strokes", |cx| paint_state(cx, &app).undo == 2);

    // Back to the first render while painting: its stroke is there.
    act(cx, window, &app, |app, window, cx| {
        app.renders.update(cx, |r, cx| r.open(&first, window, cx))
    });
    wait_until(cx, "the first render's doc", |cx| {
        let state = paint_state(cx, &app);
        state.open.as_ref() == Some(&first) && state.undo == 1
    });
    cx.simulate_keystrokes(window, "secondary-z");
    wait_until(cx, "the first render undone", |cx| {
        let state = paint_state(cx, &app);
        state.undo == 0 && !state.painted
    });
}

/// Leaving Retouch keeps the canvas with the last stroke's end: the
/// worker's last tiles still reach it.
#[gpui::test]
fn leaving_retouch_keeps_the_last_strokes_tiles(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    cx.update(|cx| cx.bind_keys(key_bindings()));
    stack(cx, window, &app);
    let shown = |cx: &mut TestAppContext| {
        app.read_with(cx, |app, cx| app.viewer.read(cx).primary_pixels().unwrap())
    };
    cx.simulate_keystrokes(window, "p");
    wait_until(cx, "the canvas and its source", |cx| {
        let opening = app.read_with(cx, |app, cx| app.paint.read(cx).is_opening());
        !opening && paint_state(cx, &app).source_ready
    });
    let plain = shown(cx);
    // A stroke and Done at once: none of its tiles is in yet.
    stroke(cx, &app, 40.0);
    act(cx, window, &app, |app, _, cx| app.leave_paint(cx));
    assert_eq!(shown(cx), plain);
    wait_until(cx, "the stroke on the canvas left", |cx| shown(cx) != plain);
}

/// Tiles of a render other than the canvas's (sent late in a switch) aren't applied.
#[gpui::test]
fn another_renders_tiles_leave_the_canvas_shown(cx: &mut TestAppContext) {
    use deptheos_app::engine::present::CpuImage;
    use deptheos_app::paint::{DisplayPatch, Report};
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    cx.update(|cx| cx.bind_keys(key_bindings()));
    stack(cx, window, &app);
    let shown = |cx: &mut TestAppContext| {
        app.read_with(cx, |app, cx| app.viewer.read(cx).primary_pixels().unwrap())
    };
    cx.simulate_keystrokes(window, "p");
    wait_until(cx, "the canvas and its source", |cx| {
        let opening = app.read_with(cx, |app, cx| app.paint.read(cx).is_opening());
        !opening && paint_state(cx, &app).source_ready
    });
    let plain = shown(cx);
    let open = paint_state(cx, &app).open.expect("open");
    let other = deptheos_app::library::history::RenderId::parse(&format!("{:032x}", 9)).unwrap();
    assert_ne!(open, other);
    let patch = |key| Report::Patches {
        key,
        patches: vec![DisplayPatch {
            level: 0,
            tx: 0,
            ty: 0,
            tile: CpuImage { width: 4, height: 4, pixels: vec![255; 64] },
        }],
    };
    app.update(cx, |app, cx| app.paint.update(cx, |p, cx| p.hear(patch(other), cx)));
    assert_eq!(shown(cx), plain, "another render's tile isn't applied");
    app.update(cx, |app, cx| app.paint.update(cx, |p, cx| p.hear(patch(open), cx)));
    assert_ne!(shown(cx), plain, "the shown render's is");
}

/// Paint of another format is damaged: Retouch sets it aside and opens the
/// render unpainted, ready for a stroke.
#[gpui::test]
fn retouch_opens_over_damaged_paint_with_none(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    cx.update(|cx| cx.bind_keys(key_bindings()));
    stack(cx, window, &app);
    let id = active(cx, &app).unwrap();
    cx.simulate_keystrokes(window, "p");
    wait_until(cx, "the source", |cx| paint_state(cx, &app).source_ready);
    stroke(cx, &app, 40.0);
    wait_until(cx, "the stroke", |cx| paint_state(cx, &app).undo == 1);
    act(cx, window, &app, |app, _, cx| app.leave_paint(cx));
    assert!(app.read_with(cx, |app, cx| app.paint.read(cx).save()));
    // Another format's: its user_version, at byte 60 of the file's header.
    let dir = crate::test_support::library(cx, &app).render_path(&id);
    let file = dir.join("paint.sqlite");
    let mut bytes = std::fs::read(&file).unwrap();
    bytes[60..64].copy_from_slice(&99u32.to_be_bytes());
    std::fs::write(&file, bytes).unwrap();

    cx.simulate_keystrokes(window, "p");
    wait_until(cx, "the source", |cx| paint_state(cx, &app).source_ready);
    assert!(app.read_with(cx, |app, cx| app.paint.read(cx).open_failure().is_none()));
    assert!(!paint_state(cx, &app).painted, "opened with nothing painted");
    let aside = std::fs::read_dir(&dir).unwrap().flatten();
    let aside = aside.filter(|e| {
        let name = e.file_name().to_string_lossy().into_owned();
        name.contains("unreadable") && name.ends_with(".sqlite")
    });
    assert_eq!(aside.count(), 1, "the damaged paint set aside");
    stroke(cx, &app, 40.0);
    wait_until(cx, "a stroke on it", |cx| paint_state(cx, &app).undo == 1);
    act(cx, window, &app, |app, _, cx| app.leave_paint(cx));
}

/// Paint is part of the result outside paint mode too: each render shows its
/// own, and its pixels for saving carry it.
#[gpui::test]
fn paint_shows_and_saves_outside_paint_mode(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    cx.update(|cx| cx.bind_keys(key_bindings()));
    stack(cx, window, &app);
    let first = active(cx, &app).unwrap();
    exclude_first(cx, &app);
    stack(cx, window, &app);
    let second = active(cx, &app).unwrap();

    let shown = |cx: &mut TestAppContext| {
        app.read_with(cx, |app, cx| app.viewer.read(cx).primary_pixels().unwrap())
    };
    let plain_second = shown(cx);
    open_render(cx, window, &app, &first);
    let plain_first = shown(cx);

    // Paint the first render, and back to stack mode.
    cx.simulate_keystrokes(window, "p");
    wait_until(cx, "the first doc", |cx| {
        let state = paint_state(cx, &app);
        state.open.is_some() && state.source_ready
    });
    stroke(cx, &app, 40.0);
    wait_until(cx, "the stroke", |cx| paint_state(cx, &app).undo == 1);
    wait_until(cx, "the stroke shown", |cx| shown(cx) != plain_first);
    let painted_first = shown(cx);
    cx.simulate_keystrokes(window, "escape");

    open_render(cx, window, &app, &second);
    wait_until(cx, "the second, unpainted", |cx| shown(cx) == plain_second);
    open_render(cx, window, &app, &first);
    wait_until(cx, "the first, painted", |cx| shown(cx) == painted_first);

    let (stack, key) = app.read_with(cx, |app, cx| app.result_doc(cx).unwrap());
    assert_eq!(key, first);
    let rx = app.update(cx, |app, cx| app.paint.update(cx, |p, _| p.flatten(key, stack.clone())));
    let pixels = rx.recv().unwrap().unwrap().expect("the first render is painted");
    assert_eq!(pixels.len(), stack.pixels.len());
    assert_ne!(pixels, stack.pixels);
}

/// Retouch's Keys are one hint per row, stacked within the inspector's width.
#[gpui::test]
fn retouchs_keys_fit_the_inspector(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 2) else { return };
    cx.update(|cx| cx.bind_keys(key_bindings()));
    stack(cx, window, &app);
    cx.simulate_keystrokes(window, "p");
    wait_until(cx, "Retouch", |cx| paint_state(cx, &app).open.is_some());
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    vcx.run_until_parked();
    let panel = vcx.debug_bounds("inspector").expect("the inspector");
    let mut above: Option<gpui::Bounds<gpui::Pixels>> = None;
    for row in 0..8 {
        let hint = vcx.debug_bounds(format!("paint-key-{row}").leak()).expect("a hint");
        assert!(hint.left() >= panel.left(), "hint {row} starts left of the panel: {hint:?}");
        assert!(hint.right() <= panel.right(), "hint {row} runs past the panel: {hint:?}");
        if let Some(above) = above {
            assert!(hint.top() >= above.bottom(), "hint {row} overlaps the one above: {hint:?}");
        }
        above = Some(hint);
    }
}

/// Retouch's bar is Stacking's: Paint From first, the rail, then Compare in
/// the same place, laying the source beside the canvas; its body toggles the
/// last reference layout (Split first) and the canvas alone; Top and Bottom
/// is offered, Flicker isn't.
#[gpui::test]
fn retouch_lays_its_source_out_from_compares_place(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    cx.update(|cx| cx.bind_keys(key_bindings()));
    stack(cx, window, &app);
    let order = ["view-home", "focus-rail", "view-compare", "view-layouts", "contribution"];
    let in_order = |cx: &mut TestAppContext| {
        let bar = bar_part(cx, window, "view-bar").expect("the bar");
        let painting = app.read_with(cx, |app, _| app.is_retouching());
        let lead = if painting { "view-paint-from" } else { "view-home" };
        let parts: Vec<_> = std::iter::once(&lead)
            .chain(&order[1..])
            .map(|part| bar_part(cx, window, part).unwrap_or_else(|| panic!("{part}")))
            .collect();
        assert!(parts[0].left() - bar.left() < px(12.0), "the home control leads: {parts:?}");
        for pair in parts.windows(2) {
            assert!(pair[0].right() <= pair[1].left(), "{pair:?}");
        }
        parts
    };
    let stacking = in_order(cx);
    act(cx, window, &app, |app, _, cx| app.toggle_paint(cx));
    wait_until(cx, "the source", |cx| paint_state(cx, &app).source_ready);
    let retouch = in_order(cx);
    // Compare keeps its place after the rail and readout (the rail is wider
    // in Retouch; Paint From's outline is 1px).
    let after_rail = |parts: &[gpui::Bounds<Pixels>]| f32::from(parts[2].left() - parts[1].right());
    let moved = after_rail(&retouch) - after_rail(&stacking);
    assert!(moved.abs() <= 1.0, "Compare where it was: {retouch:?} {stacking:?}");

    let mode = |cx: &mut TestAppContext| app.read_with(cx, |app, cx| app.viewer.read(cx).mode());
    let click = |cx: &mut TestAppContext, part: &'static str| {
        let at = bar_part(cx, window, part).expect(part).center();
        gpui::VisualTestContext::from_window(window, cx).simulate_click(at, Default::default());
    };
    assert_eq!(mode(cx), ViewMode::Result, "the canvas alone");
    click(cx, "view-compare");
    assert_eq!(mode(cx).look(), Some(Look::Split), "Split to start");
    click(cx, "view-compare");
    assert_eq!(mode(cx), ViewMode::Result);
    // Top and Bottom, from the bar's menu or the menu bar, is remembered.
    cx.dispatch_action(window, RetouchLayout(Look::TopAndBottom));
    assert_eq!(mode(cx).look(), Some(Look::TopAndBottom));
    bar_part(cx, window, "view-bar");
    let panes = app.read_with(cx, |app, cx| app.viewer.read(cx).pane_clips());
    assert!(panes.len() == 2 && panes[0].bottom() <= panes[1].top(), "source over canvas");
    click(cx, "view-compare");
    click(cx, "view-compare");
    assert_eq!(mode(cx).look(), Some(Look::TopAndBottom), "the layout used last");
    cx.dispatch_action(window, Flicker);
    assert_eq!(mode(cx).look(), Some(Look::TopAndBottom), "no Flicker in Retouch");
    // 1–0 set the opacity here, not the layout.
    cx.simulate_keystrokes(window, "1");
    assert_eq!(mode(cx).look(), Some(Look::TopAndBottom));

    // Leaving keeps Stacking's own memory: its Compare turns on Split.
    act(cx, window, &app, |app, _, cx| app.toggle_paint(cx));
    act(cx, window, &app, |app, window, cx| app.toggle_compare(window, cx));
    assert_eq!(mode(cx).look(), Some(Look::Split));
}

/// Retouch reopens on what it last showed in its own layout; the viewer
/// keeps its own.
#[gpui::test]
fn retouch_opens_again_on_what_it_was_left_showing(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    cx.update(|cx| cx.bind_keys(key_bindings()));
    stack(cx, window, &app);
    let mode = |cx: &mut TestAppContext| app.read_with(cx, |app, cx| app.viewer.read(cx).mode());
    act(cx, window, &app, |app, _, cx| app.toggle_paint(cx));
    wait_until(cx, "the source", |cx| paint_state(cx, &app).source_ready);
    cx.dispatch_action(window, RetouchLayout(Look::SideBySide));
    assert_eq!(mode(cx).look(), Some(Look::SideBySide));
    act(cx, window, &app, |app, _, cx| app.toggle_paint(cx));
    assert_eq!(mode(cx), ViewMode::Result, "Retouch closes on the result");
    act(cx, window, &app, |app, window, cx| app.toggle_compare(window, cx));
    assert_eq!(mode(cx).look(), Some(Look::Split), "the viewer's own layout");
    act(cx, window, &app, |app, window, cx| app.toggle_compare(window, cx));
    act(cx, window, &app, |app, _, cx| app.toggle_paint(cx));
    wait_until(cx, "the source", |cx| paint_state(cx, &app).source_ready);
    assert_eq!(mode(cx).look(), Some(Look::SideBySide), "as Retouch was left");
    act(cx, window, &app, |app, window, cx| app.toggle_compare(window, cx));
    assert_eq!(mode(cx), ViewMode::Result);
    act(cx, window, &app, |app, window, cx| app.toggle_compare(window, cx));
    assert_eq!(mode(cx).look(), Some(Look::SideBySide), "its layout used last");
}

/// The View menu in Retouch: Painted first, no Flicker, and no layout keys
/// (1–0 set opacity, \\ shows the source).
#[test]
fn retouchs_view_menu_names_no_keys() {
    let state = |painting: bool| MenuState {
        has_result: true,
        painting,
        layout: Some(0),
        ..MenuState::default()
    };
    let view = |painting: bool| {
        let words = crate::words::DesktopWords::english();
        let menus = menus(&state(painting), &words);
        let menu = menus.into_iter().find(|m| m.name.as_ref() == "View").expect("View");
        menu.items
            .into_iter()
            .filter_map(|item| match item {
                gpui::MenuItem::Action { name, action, disabled, .. } => {
                    Some((name.to_string(), action, disabled))
                }
                _ => None,
            })
            .take(5)
            .collect::<Vec<_>>()
    };
    let names = |items: &[(String, Box<dyn gpui::Action>, bool)]| {
        items.iter().map(|(n, _, _)| n.clone()).collect::<Vec<_>>()
    };
    let stacking = view(false);
    assert_eq!(names(&stacking)[0], "Result");
    assert!(stacking.iter().all(|(_, _, dim)| !dim));
    let retouch = view(true);
    assert_eq!(names(&retouch), ["Painted", "Split", "Side by Side", "Top and Bottom", "Flicker"]);
    let dim: Vec<bool> = retouch.iter().map(|(_, _, dim)| *dim).collect();
    assert_eq!(dim, [false, false, false, false, true], "Flicker is off");
    let bindings = key_bindings();
    for (name, action, _) in &retouch {
        assert!(
            !bindings.iter().any(|b| b.action().partial_eq(action.as_ref())),
            "{name} names no key"
        );
    }
}

/// Retouch has no Contribution (C, bar and menu off; the source pane shows
/// the frame as is). The map over the canvas is lit for the frame painted
/// from and follows a scrub, the lit tick pressed again (kept), the original
/// (whole map), a map click (picks the frame) and ⌥-click. A drag on the map
/// pans without painting; off it a press paints.
#[gpui::test]
fn retouch_lights_the_depth_map_for_the_frame_painted_from(cx: &mut TestAppContext) {
    use deptheos_app::paint::SourceKind;
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let paths = write_quarter_blurred_frames(dir.path());
    let (window, app) = launch(cx, &engine, paths, &dir.path().join("history"));
    cx.update(|cx| cx.bind_keys(key_bindings()));
    stack(cx, window, &app);
    let stacked = app.read_with(cx, |app, cx| app.stacking.read(cx).result().unwrap().clone());
    assert!(stacked.frame_map.is_some(), "the stack records where its detail came from");
    act(cx, window, &app, |app, _, cx| app.toggle_paint(cx));
    let settled = |cx: &mut TestAppContext| {
        let loading = app.read_with(cx, |app, cx| app.viewer.read(cx).is_loading());
        paint_state(cx, &app).source_ready && !loading
    };
    wait_until(cx, "the source", settled);
    let viewer = |cx: &mut TestAppContext| app.read_with(cx, |app, cx| app.viewer.read(cx).mode());
    let lit = |cx: &mut TestAppContext| {
        let mode = viewer(cx);
        mode.lit().and_then(|lit| stacked.paths.iter().position(|p| p == lit))
    };
    let source = |cx: &mut TestAppContext| match paint_state(cx, &app).source {
        Some(SourceKind::Frame { path }) => stacked.paths.iter().position(|p| *p == path),
        _ => None,
    };
    assert_eq!(viewer(cx), ViewMode::Result, "Retouch opens on the canvas alone");

    // No Contribution: C does nothing, and nothing offers it.
    let plain = app.read_with(cx, |app, cx| app.viewer.read(cx).compare_pixels());
    assert!(plain.is_some(), "the source beside the canvas");
    cx.simulate_keystrokes(window, "c");
    cx.run_until_parked();
    app.read_with(cx, |app, cx| {
        assert!(!app.has_contribution(cx) && !app.contribution_on(cx));
        assert_eq!(app.menu_state(cx).contribution, None, "the menu's item is off");
        assert_eq!(app.viewer.read(cx).overlay(), deptheos_app::Overlay::None);
        assert_eq!(app.viewer.read(cx).compare_pixels(), plain, "the source as it is");
        assert!(!app.viewer.read(cx).is_loading(), "nothing loads");
    });
    assert_eq!(viewer(cx), ViewMode::Result, "and the view stays");

    // M: the depth map over the canvas, lit for the frame painted from.
    let first = source(cx).expect("a frame first");
    cx.simulate_keystrokes(window, "m");
    assert!(viewer(cx).is_map());
    assert_eq!(lit(cx), Some(first), "lit for the frame painted from");
    let legend = app.read_with(cx, |app, cx| app.rail_model(cx).and_then(|m| m.legend));
    assert!(legend.is_some(), "the rail is its key");
    // A scrub picks the frame painted from, and the light follows it.
    let other = 1 - first;
    act(cx, window, &app, |app, window, cx| app.scrub_to(other, window, cx));
    assert_eq!(lit(cx), Some(other), "at once");
    cx.executor().advance_clock(std::time::Duration::from_millis(500));
    wait_until(cx, "the frame", |cx| settled(cx) && source(cx) == Some(other));
    assert_eq!(lit(cx), Some(other));
    // Pressing the lit tick keeps the light: Retouch paints from a source.
    act(cx, window, &app, |app, window, cx| app.press_tick(other, window, cx));
    assert_eq!(lit(cx), Some(other), "still lit");
    // The original paints from no frame: the map whole.
    act(cx, window, &app, |app, _, cx| app.with_paint(cx, |p, cx| p.pick_original(cx)));
    wait_until(cx, "the original", |cx| {
        paint_state(cx, &app).source == Some(SourceKind::Original) && settled(cx)
    });
    assert!(viewer(cx).is_map() && lit(cx).is_none(), "whole");

    // A click on the map picks the frame under it to paint from, lit.
    bar_part(cx, window, "view-bar");
    let photo = app.read_with(cx, |app, cx| app.viewer.read(cx).pane_clips()[0].center());
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    vcx.simulate_mouse_move(photo, None, Default::default());
    let gesture = app.read_with(cx, |app, _| app.scrub_gesture());
    let crate::app::scrubber::Scrub::OverMap(under) = gesture else {
        panic!("over the map, not {gesture:?}");
    };
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    vcx.simulate_mouse_down(photo, gpui::MouseButton::Left, Default::default());
    vcx.simulate_mouse_up(photo, gpui::MouseButton::Left, Default::default());
    assert_eq!(source(cx), Some(under), "painted from");
    assert_eq!(lit(cx), Some(under), "and lit");
    cx.executor().advance_clock(std::time::Duration::from_millis(500));
    wait_until(cx, "the frame", |cx| settled(cx) && source(cx) == Some(under));

    // A drag on the map pans: no stroke, and the light stays.
    let past = photo + gpui::point(px(60.0), px(0.0));
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    vcx.simulate_mouse_down(photo, gpui::MouseButton::Left, Default::default());
    vcx.simulate_mouse_move(past, gpui::MouseButton::Left, Default::default());
    vcx.simulate_mouse_up(past, gpui::MouseButton::Left, Default::default());
    cx.run_until_parked();
    assert_eq!(paint_state(cx, &app).undo, 0, "no stroke on the map");
    assert_eq!(lit(cx), Some(under));

    // ⌥-click picks the frame sharpest there: marked and lit.
    let (x, y) = (stacked.width * 7 / 8, stacked.height / 2);
    let sharpest = stacked.frame_at(x, y).expect("a frame recorded there");
    act(cx, window, &app, |app, window, cx| app.pick_sharpest(x, y, window, cx));
    app.read_with(cx, |app, cx| {
        assert_eq!(app.rail_model(cx).unwrap().current, Some(sharpest), "the marker moved")
    });
    assert_eq!(lit(cx), Some(sharpest), "the light too");
    wait_until(cx, "the frame", |cx| settled(cx) && source(cx) == Some(sharpest));

    // Off the map, back to the canvas, where the same drag paints.
    cx.simulate_keystrokes(window, "m");
    assert_eq!(viewer(cx), ViewMode::Result, "back to the canvas");
    let photo = app.read_with(cx, |app, cx| {
        let canvas = *app.viewer.read(cx).pane_clips().last().expect("the canvas is drawn");
        canvas.center() - gpui::point(px(30.0), px(0.0))
    });
    bar_part(cx, window, "view-bar");
    let past = photo + gpui::point(px(60.0), px(0.0));
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    vcx.simulate_mouse_down(photo, gpui::MouseButton::Left, Default::default());
    vcx.simulate_mouse_move(past, gpui::MouseButton::Left, Default::default());
    vcx.simulate_mouse_up(past, gpui::MouseButton::Left, Default::default());
    wait_until(cx, "the stroke", |cx| paint_state(cx, &app).undo == 1);
}

/// Quit keeps order: the stroke and list change are on disk before the
/// last step (`exit`, standing in for `work::exit`) runs, once.
#[gpui::test]
fn quit_saves_the_paint_and_the_list_before_it_exits(cx: &mut TestAppContext) {
    use std::cell::Cell;
    use std::rc::Rc;

    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    cx.update(|cx| cx.bind_keys(key_bindings()));
    stack(cx, window, &app);
    let id = active(cx, &app).unwrap();
    cx.simulate_keystrokes(window, "p");
    wait_until(cx, "the source", |cx| paint_state(cx, &app).source_ready);
    stroke(cx, &app, 40.0);
    wait_until(cx, "the stroke", |cx| paint_state(cx, &app).undo == 1);
    exclude_first(cx, &app);
    let excluded = app.read_with(cx, |app, cx| app.working(cx).list.excluded);
    assert_eq!(excluded.len(), 1);

    let exits = Rc::new(Cell::new(0));
    let exited = exits.clone();
    let library = crate::test_support::library(cx, &app);
    let paint = library.render_path(&id).join("paint.sqlite");
    cx.update(|cx| {
        crate::app::quit::set_main_view(&app, cx);
        crate::app::quit::on_quit(cx, move || {
            // `work::exit`'s first step: the store thread's last writes (all
            // tests share the thread).
            assert!(deptheos_app::engine::store::store().flush(crate::test_support::STORE_DRAINS));
            assert!(paint.exists(), "the stroke is saved before the exit");
            let list = library.read_working().unwrap().list;
            // The file stores paths under its root, which may resolve the temp
            // folder's link: compare by name.
            let names = |set: &std::collections::BTreeSet<PathBuf>| -> Vec<_> {
                set.iter().map(|p| p.file_name().unwrap().to_owned()).collect()
            };
            assert_eq!(names(&list.excluded), names(&excluded), "the list as it was at quit");
            exited.set(exited.get() + 1);
        });
    });
    cx.quit();
    assert_eq!(exits.get(), 1);
}

/// Retouch's keys work at once after clicking a control Retouch hides: focus
/// returns to the app, not to nothing.
#[gpui::test]
fn retouchs_keys_work_after_the_focused_control_goes(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else {
        return;
    };
    cx.update(|cx| cx.bind_keys(key_bindings()));
    stack(cx, window, &app);
    click(cx, window, "method-0");
    click(cx, window, "retouch");
    wait_until(cx, "Retouch open", |cx| {
        let state = paint_state(cx, &app);
        state.open.is_some() && state.source_ready
    });
    let focused = cx.update_window(window, |_, window, cx| window.focused(cx).is_some()).unwrap();
    assert!(focused, "the window's keys go somewhere");

    let radius = |cx: &mut TestAppContext| {
        app.read_with(cx, |app, cx| app.paint.read(cx).current_brush(cx).radius)
    };
    let before = radius(cx);
    cx.simulate_keystrokes(window, "]");
    assert!(radius(cx) > before, "] makes the brush larger");
    cx.simulate_keystrokes(window, "escape");
    app.read_with(cx, |app, _| assert!(!app.is_retouching(), "Escape leaves Retouch"));
}

/// Where the main window's quit is, by name.
fn quitting(cx: &mut TestAppContext, app: &Entity<AppView>) -> &'static str {
    use crate::app::quit::Quitting;
    app.read_with(cx, |app, _| match app.quitting {
        Quitting::Staying => "staying",
        Quitting::Waiting { .. } => "waiting",
        Quitting::Kept => "kept",
    })
}

/// Waits for the quit to leave `Waiting`, its polls' timers run.
fn wait_for_the_quit(cx: &mut TestAppContext, app: &Entity<AppView>) {
    wait_until(cx, "the quit's save", |cx| {
        cx.executor().advance_clock(std::time::Duration::from_millis(20));
        quitting(cx, app) != "waiting"
    });
}

/// Quitting with retouching that couldn't be written (a folder where the
/// paint file goes, so RAM only) waits on the save without blocking, then
/// asks instead of quitting.
#[gpui::test]
fn quit_asks_when_the_retouching_is_not_saved(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    cx.update(|cx| cx.bind_keys(key_bindings()));
    stack(cx, window, &app);
    let id = active(cx, &app).unwrap();
    let file = crate::test_support::library(cx, &app).render_path(&id).join("paint.sqlite");
    std::fs::create_dir(&file).unwrap();
    cx.simulate_keystrokes(window, "p");
    wait_until(cx, "the source", |cx| paint_state(cx, &app).source_ready);
    stroke(cx, &app, 40.0);
    wait_until(cx, "the stroke", |cx| paint_state(cx, &app).undo == 1);
    let now = act(cx, window, &app, |app, window, cx| app.confirm_quit(window, cx));
    assert!(!now, "quit waits on the save");
    assert_eq!(quitting(cx, &app), "waiting");
    wait_for_the_quit(cx, &app);
    assert_eq!(quitting(cx, &app), "staying", "the quit is off until answered");
    assert!(asking(cx, window), "asked first: the retouching isn't saved");
}

/// Quit saves retouching from a task, the window free meanwhile, and the
/// quit hook doesn't save it again.
#[gpui::test]
fn quit_saves_the_retouching_once_then_quits(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    cx.update(|cx| cx.bind_keys(key_bindings()));
    stack(cx, window, &app);
    let id = active(cx, &app).unwrap();
    cx.simulate_keystrokes(window, "p");
    wait_until(cx, "the source", |cx| paint_state(cx, &app).source_ready);
    stroke(cx, &app, 40.0);
    wait_until(cx, "the stroke", |cx| paint_state(cx, &app).undo == 1);
    let now = act(cx, window, &app, |app, window, cx| app.confirm_quit(window, cx));
    assert!(!now, "quit waits on the save");
    wait_for_the_quit(cx, &app);
    assert_eq!(quitting(cx, &app), "kept", "saved: the quit hook leaves the paint be");
    let paint = crate::test_support::library(cx, &app).render_path(&id).join("paint.sqlite");
    assert!(paint.is_file(), "the stroke is saved");
    assert!(!asking(cx, window), "nothing to ask");
}

/// Viewing, Retouch and Adjust are one mode: opening one closes the other,
/// the viewer's place and both sessions following.
#[gpui::test]
fn retouch_and_adjust_are_one_mode(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    stack(cx, window, &app);
    let render = active(cx, &app).unwrap();
    let seen = |cx: &mut TestAppContext| {
        app.read_with(cx, |app, cx| {
            let viewer = app.viewer.read(cx);
            let place = match (viewer.retouched().is_some(), viewer.adjusting().is_some()) {
                (false, false) => "viewer",
                (true, false) => "retouch",
                (false, true) => "adjust",
                (true, true) => "both",
            };
            let mode = match (&app.mode, app.painting(), app.adjusting()) {
                (Mode::Viewing, None, None) => "viewing",
                (Mode::Retouch(_), Some(_), None) => "retouch",
                (Mode::Adjust(_), None, Some(_)) => "adjust",
                _ => "mixed",
            };
            let painting = app.paint.read(cx).state().is_some();
            let adjusting = app.adjust.read(cx).session().adjusting();
            (mode, place, painting, adjusting)
        })
    };
    assert_eq!(seen(cx), ("viewing", "viewer", false, None));

    act(cx, window, &app, |app, _, cx| app.toggle_paint(cx));
    assert_eq!(seen(cx), ("retouch", "retouch", true, None));

    // Retouch closes as Adjust opens.
    act(cx, window, &app, |app, _, cx| app.toggle_adjust(cx));
    wait_until(cx, "the render adjusted", |cx| seen(cx).3.is_some());
    assert_eq!(seen(cx), ("adjust", "adjust", false, Some(render.clone())));

    // And Adjust as Retouch opens.
    act(cx, window, &app, |app, _, cx| app.toggle_paint(cx));
    wait_until(cx, "Adjust left", |cx| seen(cx).3.is_none());
    assert_eq!(seen(cx), ("retouch", "retouch", true, None));

    act(cx, window, &app, |app, _, cx| app.toggle_paint(cx));
    assert_eq!(seen(cx), ("viewing", "viewer", false, None));

    act(cx, window, &app, |app, _, cx| app.toggle_adjust(cx));
    wait_until(cx, "the render adjusted", |cx| seen(cx).3.is_some());
    act(cx, window, &app, |app, _, cx| app.toggle_adjust(cx));
    wait_until(cx, "Adjust left", |cx| seen(cx).3.is_none());
    assert_eq!(seen(cx), ("viewing", "viewer", false, None));
}
