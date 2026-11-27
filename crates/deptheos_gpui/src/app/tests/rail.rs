//! The focus rail.

use super::*;

/// The hover label stays over the rail at both ends, even for a name longer
/// than half the rail.
#[gpui::test]
fn the_rail_label_stays_over_the_rail(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let paths: Vec<PathBuf> = write_frames(dir.path(), 4)
        .into_iter()
        .enumerate()
        .map(|(i, path)| {
            let long = dir.path().join(format!("a_long_name_from_the_camera_card_{i}.png"));
            std::fs::rename(&path, &long).unwrap();
            long
        })
        .collect();
    let (window, app) = launch(cx, &engine, paths, &dir.path().join("history"));
    stack(cx, window, &app);
    let rail = bar_part(cx, window, "focus-rail").expect("the rail");
    for i in [0, 1, 2, 3] {
        let mut vcx = gpui::VisualTestContext::from_window(window, cx);
        vcx.simulate_mouse_move(tick(rail, i, 4), None, Default::default());
        let label = bar_part(cx, window, "rail-label").expect("the label");
        assert!(label.left() >= rail.left() - px(1.0), "tick {i}: {label:?} {rail:?}");
        assert!(label.right() <= rail.right() + px(1.0), "tick {i}: {label:?} {rail:?}");
        assert!(label.size.width > px(100.0), "tick {i}: room for the name ({label:?})");
    }
}

/// The rail: hover names the frame, a press shows it alone, a drag scrubs,
/// wheel and ← / → step; comparing, a scrubbed frame is compared; the map
/// returns to the frame; Contribution needs a frame in view; home returns
/// to the result.
#[gpui::test]
fn the_focus_rail_scrubs_the_frames(cx: &mut TestAppContext) {
    use crate::app::scrubber::Scrub;
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 4) else {
        return;
    };
    cx.update(|cx| cx.bind_keys(key_bindings()));
    stack(cx, window, &app);
    let settled =
        |cx: &mut TestAppContext| !app.read_with(cx, |app, cx| app.viewer.read(cx).is_loading());
    let current = |cx: &mut TestAppContext| {
        app.read_with(cx, |app, cx| app.rail_model(cx).and_then(|m| m.current))
    };
    let mode = |cx: &mut TestAppContext| app.read_with(cx, |app, cx| app.viewer.read(cx).mode());
    let gesture = |cx: &mut TestAppContext| app.read_with(cx, |app, _| app.scrub_gesture());
    let stacked = app.read_with(cx, |app, cx| app.stacking.read(cx).result().unwrap().clone());
    let lit = |cx: &mut TestAppContext| {
        let mode = mode(cx);
        mode.lit().and_then(|lit| stacked.paths.iter().position(|p| p == lit))
    };
    let whole = |cx: &mut TestAppContext| {
        matches!(mode(cx), ViewMode::Map { back: Back::Frame { .. }, light: Light::Whole })
    };
    assert_eq!(stacked.paths.len(), 4, "a tick per frame");
    assert_eq!(current(cx), None, "the result alone marks no frame");
    assert!(bar_part(cx, window, "rail-readout").is_some(), "the readout has room");
    app.read_with(cx, |app, cx| assert!(!app.has_contribution(cx), "no frame in view"));

    // A hover names the frame under the pointer.
    let rail = bar_part(cx, window, "focus-rail").expect("the rail is in the view bar");
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    vcx.simulate_mouse_move(tick(rail, 2, 4), None, Default::default());
    assert_eq!(gesture(cx), Scrub::Hovering(2));
    assert!(bar_part(cx, window, "rail-label").is_some(), "the label over its tick");

    // A press shows that frame alone; dragging scrubs on.
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    vcx.simulate_mouse_down(tick(rail, 0, 4), gpui::MouseButton::Left, Default::default());
    assert_eq!(gesture(cx), Scrub::Dragging(0));
    assert!(matches!(mode(cx), ViewMode::Frame { .. }));
    assert_eq!(current(cx), Some(0));
    // A few pixels on start the drag, as a real one does.
    let start = tick(rail, 0, 4) + gpui::point(px(3.0), px(0.0));
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    vcx.simulate_mouse_move(start, gpui::MouseButton::Left, Default::default());
    // Past the rail's start, the drag keeps to the first frame.
    let before = gpui::point(rail.left() - px(40.0), rail.top() - px(30.0));
    for (at, i) in [(tick(rail, 1, 4), 1), (before, 0), (tick(rail, 3, 4), 3)] {
        let mut vcx = gpui::VisualTestContext::from_window(window, cx);
        vcx.simulate_mouse_move(at, gpui::MouseButton::Left, Default::default());
        assert_eq!(gesture(cx), Scrub::Dragging(i));
        assert_eq!(current(cx), Some(i));
    }
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    vcx.simulate_mouse_up(tick(rail, 3, 4), gpui::MouseButton::Left, Default::default());
    assert_ne!(gesture(cx), Scrub::Dragging(3), "let go");
    wait_until(cx, "the frame", settled);
    app.read_with(cx, |app, cx| {
        let viewer = app.viewer.read(cx);
        assert!(viewer.has_compare() && matches!(viewer.mode(), ViewMode::Frame { .. }), "alone");
        let files = app.files.read(cx);
        assert_eq!(files.selected(cx), files.row_of(&stacked.paths[3]), "its row");
    });

    // The wheel steps: a notch up goes back, trackpad scroll a frame per stretch.
    let wheel = |cx: &mut TestAppContext, delta: gpui::ScrollDelta| {
        let mut vcx = gpui::VisualTestContext::from_window(window, cx);
        vcx.simulate_event(gpui::ScrollWheelEvent {
            position: tick(rail, 1, 4),
            delta,
            modifiers: Default::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
    };
    wheel(cx, gpui::ScrollDelta::Lines(gpui::point(0.0, 1.0)));
    assert_eq!(current(cx), Some(2));
    wheel(cx, gpui::ScrollDelta::Pixels(gpui::point(px(0.0), px(10.0))));
    assert_eq!(current(cx), Some(2), "not yet a step");
    wheel(cx, gpui::ScrollDelta::Pixels(gpui::point(px(0.0), px(20.0))));
    assert_eq!(current(cx), Some(1));
    // ← and → step along the rail too, still alone.
    cx.simulate_keystrokes(window, "left");
    assert_eq!(current(cx), Some(0));
    cx.simulate_keystrokes(window, "left");
    assert_eq!(current(cx), Some(0), "the first stays");
    cx.simulate_keystrokes(window, "right");
    assert_eq!(current(cx), Some(1));
    assert!(matches!(mode(cx), ViewMode::Frame { .. }));
    wait_until(cx, "the frame", settled);
    let tints = stacked.frame_map.is_some();
    app.read_with(cx, |app, cx| assert_eq!(app.has_contribution(cx), tints, "a frame in view"));

    // The depth map goes back to the frame.
    cx.simulate_keystrokes(window, "m");
    assert!(mode(cx).is_map());
    app.read_with(cx, |app, cx| assert!(!app.has_contribution(cx), "under the map"));
    cx.simulate_keystrokes(window, "m");
    assert!(matches!(mode(cx), ViewMode::Frame { .. }));

    // On the map the rail is its key in the map's colors. A press lights a
    // frame (map stays, readout follows); ← and → step the light; the lit
    // tick again puts it out. M leaves for the lit frame with Contribution,
    // and reopens lit; unlit it returns to the frame before. An unlightable
    // map is left for the frame pressed.
    cx.simulate_keystrokes(window, "m");
    let legend = app.read_with(cx, |app, cx| app.rail_model(cx).and_then(|m| m.legend));
    let colors: Vec<_> = (0..4).map(|i| stacked.frame_color(i)).collect();
    assert_eq!(legend, Some(colors), "the ticks in the map's colors");
    let press = |cx: &mut TestAppContext, i: usize| {
        let rail = bar_part(cx, window, "focus-rail").expect("the rail");
        let mut vcx = gpui::VisualTestContext::from_window(window, cx);
        vcx.simulate_mouse_down(tick(rail, i, 4), gpui::MouseButton::Left, Default::default());
        let mut vcx = gpui::VisualTestContext::from_window(window, cx);
        vcx.simulate_mouse_up(tick(rail, i, 4), gpui::MouseButton::Left, Default::default());
    };
    press(cx, 2);
    if tints {
        assert!(matches!(mode(cx), ViewMode::Map { back: Back::Frame { .. }, .. }));
        assert_eq!(lit(cx), Some(2));
        assert_eq!(current(cx), Some(2), "marked, and the readout says so");
        cx.simulate_keystrokes(window, "right");
        assert_eq!(lit(cx), Some(3), "→ steps the light");
        press(cx, 3);
        assert!(whole(cx), "put out");
        assert_eq!(current(cx), None);
        // The pointer over the map marks its frame on the rail; a click lights it.
        bar_part(cx, window, "view-bar");
        let photo = app.read_with(cx, |app, cx| app.viewer.read(cx).pane_clips()[0].center());
        let mut vcx = gpui::VisualTestContext::from_window(window, cx);
        vcx.simulate_mouse_move(photo, None, Default::default());
        let Scrub::OverMap(under) = gesture(cx) else {
            panic!("over the map, not {:?}", gesture(cx));
        };
        let label = bar_part(cx, window, "rail-label");
        assert!(label.is_some(), "the label names the frame under the pointer");
        let mut vcx = gpui::VisualTestContext::from_window(window, cx);
        vcx.simulate_mouse_down(photo, gpui::MouseButton::Left, Default::default());
        vcx.simulate_mouse_up(photo, gpui::MouseButton::Left, Default::default());
        assert_eq!(lit(cx), Some(under), "a click lights it");
        // A press that pans lights nothing.
        let mut vcx = gpui::VisualTestContext::from_window(window, cx);
        let past = photo + gpui::point(px(20.0), px(0.0));
        vcx.simulate_mouse_down(photo, gpui::MouseButton::Left, Default::default());
        vcx.simulate_mouse_move(past, gpui::MouseButton::Left, Default::default());
        vcx.simulate_mouse_up(past, gpui::MouseButton::Left, Default::default());
        assert_eq!(lit(cx), Some(under), "a pan");
        act(cx, window, &app, |app, _, cx| app.point_at_map(None, cx));
        assert_eq!(gesture(cx), Scrub::Idle, "off the picture");
        let overlay =
            |cx: &mut TestAppContext| app.read_with(cx, |app, cx| app.viewer.read(cx).overlay());
        assert_eq!(overlay(cx), deptheos_app::Overlay::None, "the frame under the map, plain");
        // Left lit: the frame lit, alone, with Contribution on.
        cx.simulate_keystrokes(window, "m");
        assert!(matches!(mode(cx), ViewMode::Frame { .. }));
        assert_eq!(current(cx), Some(under), "the frame lit");
        assert_eq!(overlay(cx), deptheos_app::Overlay::Contribution);
        wait_until(cx, "the frame", settled);
        // With Contribution on, the map opens lit for that frame.
        cx.simulate_keystrokes(window, "m");
        assert!(mode(cx).is_map());
        assert_eq!(lit(cx), Some(under), "lit again");
        // Put out, M goes back to the frame as it was.
        press(cx, under);
        assert_eq!(lit(cx), None);
        cx.simulate_keystrokes(window, "m");
        assert!(matches!(mode(cx), ViewMode::Frame { .. }));
        assert_eq!(current(cx), Some(under));
        // Contribution off: the map opens whole.
        cx.simulate_keystrokes(window, "c");
        assert_eq!(overlay(cx), deptheos_app::Overlay::None);
        cx.simulate_keystrokes(window, "m");
        assert!(whole(cx), "whole");
        cx.simulate_keystrokes(window, "m");
    } else {
        assert!(matches!(mode(cx), ViewMode::Frame { .. }), "left for the frame");
        wait_until(cx, "the frame", settled);
        cx.simulate_keystrokes(window, "left");
    }
    wait_until(cx, "the frame", settled);

    // Comparing, a frame scrubbed to is compared in the layout on screen.
    act(cx, window, &app, |app, window, cx| app.set_view(Look::TopAndBottom, window, cx));
    act(cx, window, &app, |app, window, cx| app.scrub_to(3, window, cx));
    wait_until(cx, "the frame", settled);
    assert_eq!(mode(cx).look(), Some(Look::TopAndBottom));
    assert_eq!(current(cx), Some(3));

    // Home: back to the result alone, unlit (the resting view, not a mode).
    let home = bar_part(cx, window, "view-home-body").expect("the home control");
    gpui::VisualTestContext::from_window(window, cx)
        .simulate_click(home.center(), Default::default());
    assert_eq!(mode(cx), ViewMode::Result);
    assert_eq!(current(cx), None);
}

/// In Retouch the rail picks the frame painted from (marker follows, painted
/// frames dotted); Paint From is titled Frames, a render's name or the
/// original. Off a frame the rail marks none ("–/3"), and Frames returns to
/// the frame last painted from.
#[gpui::test]
fn the_rail_picks_the_frame_painted_from(cx: &mut TestAppContext) {
    use crate::app::view_bar::PaintFrom;
    use crate::paint::SourcesTab;
    use deptheos_app::paint::SourceKind;
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 4) else {
        return;
    };
    cx.update(|cx| cx.bind_keys(key_bindings()));
    stack(cx, window, &app);
    let first = active(cx, &app).unwrap();
    // Three frames in the render retouched.
    exclude_first(cx, &app);
    stack(cx, window, &app);
    let stacked = app.read_with(cx, |app, cx| app.stacking.read(cx).result().unwrap().clone());
    let from = |cx: &mut TestAppContext| app.read_with(cx, |app, cx| app.paint_from(cx));
    assert_eq!(from(cx), None, "only in Retouch");
    act(cx, window, &app, |app, _, cx| app.toggle_paint(cx));
    wait_until(cx, "the source", |cx| paint_state(cx, &app).source_ready);
    assert!(bar_part(cx, window, "view-home").is_none(), "Paint From in its place");
    assert!(bar_part(cx, window, "view-paint-from").is_some());
    let joined = bar_part(cx, window, "paint-from-rail").expect("Paint From and the rail");
    let rail = bar_part(cx, window, "focus-rail").expect("the rail");
    assert!(joined.left() <= rail.left() && rail.right() <= joined.right(), "one control");
    let model = |cx: &mut TestAppContext| app.read_with(cx, |app, cx| app.rail_model(cx).unwrap());
    let frame = |i: usize| Some(SourceKind::Frame { path: stacked.paths[i].clone() });
    let tab = |cx: &mut TestAppContext| app.read_with(cx, |app, _| app.sources_tab());

    act(cx, window, &app, |app, window, cx| app.scrub_to(2, window, cx));
    assert_eq!(paint_state(cx, &app).source, frame(2), "picked at once");
    assert_eq!(model(cx).current, Some(2));
    assert_eq!(from(cx), Some(PaintFrom::Frames));
    app.read_with(cx, |app, cx| {
        let files = app.files.read(cx);
        assert_eq!(files.selected(cx), files.row_of(&stacked.paths[2]), "its row");
    });
    cx.simulate_keystrokes(window, "left");
    assert_eq!(paint_state(cx, &app).source, frame(1));
    assert_eq!(model(cx).current, Some(1));
    // Scrubbed: the full frame once it settles.
    cx.executor().advance_clock(std::time::Duration::from_millis(500));
    wait_until(cx, "the frame", |cx| {
        let state = paint_state(cx, &app);
        state.source_ready && state.source == frame(1)
    });
    stroke(cx, &app, 40.0);
    wait_until(cx, "the stroke", |cx| !paint_state(cx, &app).painted_from.is_empty());
    assert_eq!(model(cx).painted, vec![false, true, false], "a dot over the frame painted from");

    // E, the menu's last item: the original, no frame marked.
    cx.simulate_keystrokes(window, "e");
    wait_until(cx, "the original", |cx| paint_state(cx, &app).source == Some(SourceKind::Original));
    assert_eq!(from(cx), Some(PaintFrom::Original));
    assert_eq!(model(cx).current, None, "no frame marked");
    assert!(bar_part(cx, window, "rail-readout").is_some(), "the readout says –/3");

    // Another render, by its name; the left panel lists the renders.
    assert_eq!(tab(cx), Some(SourcesTab::Frames));
    act(cx, window, &app, |app, window, cx| app.compare_with_render(first.clone(), window, cx));
    let from_first = SourceKind::Render { render: first.as_str().to_string() };
    wait_until(cx, "#1 painted from", |cx| {
        paint_state(cx, &app).source == Some(from_first.clone())
    });
    let named = matches!(from(cx), Some(PaintFrom::Render(name)) if name.starts_with("#1"));
    assert!(named, "named as the bar names renders: {:?}", from(cx));
    assert_eq!(tab(cx), Some(SourcesTab::Renders), "the panel follows");
    assert_eq!(model(cx).current, None, "no frame marked");

    // Frames: back to the frame painted from last, the frames listed.
    act(cx, window, &app, |app, window, cx| app.paint_from_frames(window, cx));
    assert_eq!(paint_state(cx, &app).source, frame(1), "the frame painted from last");
    assert_eq!(model(cx).current, Some(1));
    assert_eq!(from(cx), Some(PaintFrom::Frames));
    assert_eq!(tab(cx), Some(SourcesTab::Frames));
    wait_until(cx, "the frame", |cx| paint_state(cx, &app).source_ready);
}

/// An Option drag steps a frame per 8 points from where Option took effect
/// (a press, or mid-drag); released mid-drag, the frame under the pointer
/// is scrubbed to again.
#[gpui::test]
fn option_drag_steps_the_rail_a_frame_at_a_time(cx: &mut TestAppContext) {
    use crate::app::scrubber::{FINE_STEP, Scrub};
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 4) else { return };
    stack(cx, window, &app);
    let current = |cx: &mut TestAppContext| {
        app.read_with(cx, |app, cx| app.rail_model(cx).and_then(|m| m.current))
    };
    let gesture = |cx: &mut TestAppContext| app.read_with(cx, |app, _| app.scrub_gesture());
    let alt = gpui::Modifiers { alt: true, ..Default::default() };
    let rail = bar_part(cx, window, "focus-rail").expect("the rail");
    // A frame's room is far wider than a step, so only the step moves.
    assert!(rail.size.width / 4.0 > px(FINE_STEP * 4.0), "{rail:?}");
    let left = tick(rail, 0, 4);
    let step = |n: f32| left + gpui::point(px(FINE_STEP * n), px(0.0));
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    vcx.simulate_mouse_down(left, gpui::MouseButton::Left, alt);
    assert!(matches!(gesture(cx), Scrub::Fine { at: 0, from: 0, .. }), "{:?}", gesture(cx));
    assert_eq!(current(cx), Some(0));
    for (n, frame) in [(0.4, 0), (1.0, 1), (1.9, 1), (2.0, 2), (3.5, 3), (9.0, 3), (1.2, 1)] {
        let mut vcx = gpui::VisualTestContext::from_window(window, cx);
        vcx.simulate_mouse_move(step(n), gpui::MouseButton::Left, alt);
        assert_eq!(current(cx), Some(frame), "{n} steps");
        assert!(matches!(gesture(cx), Scrub::Fine { .. }), "{:?}", gesture(cx));
    }
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    vcx.simulate_mouse_up(step(1.2), gpui::MouseButton::Left, alt);
    assert!(!matches!(gesture(cx), Scrub::Fine { .. } | Scrub::Dragging(_)), "let go");

    // Without Option a drag follows the pointer; Option then steps from that
    // frame, and releasing returns to the frame under the pointer.
    let third = tick(rail, 2, 4);
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    vcx.simulate_mouse_down(third, gpui::MouseButton::Left, Default::default());
    let at = third + gpui::point(px(3.0), px(0.0));
    vcx.simulate_mouse_move(at, gpui::MouseButton::Left, Default::default());
    assert_eq!((gesture(cx), current(cx)), (Scrub::Dragging(2), Some(2)));
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    vcx.simulate_modifiers_change(alt);
    assert!(matches!(gesture(cx), Scrub::Fine { at: 2, from: 2, .. }), "{:?}", gesture(cx));
    let back = at - gpui::point(px(FINE_STEP), px(0.0));
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    vcx.simulate_mouse_move(back, gpui::MouseButton::Left, alt);
    assert_eq!(current(cx), Some(1), "a step back");
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    vcx.simulate_modifiers_change(Default::default());
    assert_eq!((gesture(cx), current(cx)), (Scrub::Dragging(2), Some(2)), "under the pointer");
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    vcx.simulate_mouse_up(back, gpui::MouseButton::Left, Default::default());
    if crate::app::window_menu::HAS_WINDOW_MENU {
        assert!(app.read_with(cx, |app, _| app.window_menu.is_none()), "no menu opened");
    }
}

/// The frames a [`Fixture`] wrote, in their order.
fn frames_in(dir: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "png"))
        .collect();
    paths.sort();
    paths
}

/// While the stack on screen runs the rail is the live key: ticks color as
/// frames are taken in, restart from the last frame on reversal, and all
/// color from the blend on; one height, none marked, the readout counting.
/// Pointer, wheel and keys do nothing; result actions aren't in the bar. A
/// run ending without a result leaves no rail.
#[gpui::test]
fn the_rail_is_the_key_to_a_running_stack(cx: &mut TestAppContext) {
    use crate::app::scrubber::{RailOf, Scrub};
    use deptheos_app::SessionEvent;
    use deptheos_app::engine::present::{legend_color, running_legend};
    let Some(Fixture { dir, window, app, .. }) = Fixture::launch(cx, 4) else { return };
    cx.update(|cx| cx.bind_keys(key_bindings()));
    let paths = frames_in(dir.path());
    assert_eq!(paths.len(), 4);
    act(cx, window, &app, |app, _, cx| {
        app.stacking.update(cx, |s, cx| s.hold_run(paths.clone(), cx))
    });
    let feed = |cx: &mut TestAppContext, event: SessionEvent| {
        act(cx, window, &app, |app, _, cx| app.stacking.update(cx, |s, cx| s.feed(event, cx)));
    };
    let model = |cx: &mut TestAppContext| {
        app.read_with(cx, |app, cx| {
            app.rail_model(cx).map(|m| {
                let heights: Vec<f32> = (0..m.count()).map(|i| m.height(i)).collect();
                (m.of, m.legend, m.current, heights)
            })
        })
    };
    let readout = |cx: &mut TestAppContext| {
        bar_part(cx, window, "rail-readout")?;
        app.read_with(cx, |app, cx| app.rail_model(cx)).map(|m| m.of)
    };

    let (of, legend, current, heights) = model(cx).expect("the run's rail");
    assert_eq!(of, RailOf::Run { taken: 0 });
    assert_eq!(legend, Some(vec![None; 4]), "nothing taken in yet");
    assert_eq!(current, None, "no frame marked");
    assert!(heights.iter().all(|&h| h == heights[0]), "one height: no shares yet");
    assert!(bar_part(cx, window, "focus-rail").is_some(), "the rail is up during the run");
    assert_eq!(readout(cx), Some(RailOf::Run { taken: 0 }), "the readout counts");
    for absent in ["view-home", "view-compare", "contribution", "view-paint-from"] {
        assert!(bar_part(cx, window, absent).is_none(), "{absent} waits for a result");
    }
    let progress = app.read_with(cx, |app, cx| app.render_progress_card(cx).is_some());
    assert!(progress, "the card stays");

    // Taken in, frame by frame.
    feed(cx, SessionEvent::Started { total: 4 });
    feed(cx, SessionEvent::Accumulated { current: 2, total: 4 });
    let (of, legend, ..) = model(cx).unwrap();
    assert_eq!(of, RailOf::Run { taken: 2 });
    assert_eq!(legend, Some(running_legend(4, 2, false)));
    let legend = legend.unwrap();
    assert_eq!(legend[0], Some(legend_color(0, 4)));
    assert_eq!(legend[1], Some(legend_color(1, 4)));
    assert_eq!(&legend[2..], &[None, None]);

    // Reversed: the count starts over from the last frame.
    feed(cx, SessionEvent::Reversed);
    assert_eq!(model(cx).unwrap().1, Some(vec![None; 4]), "recolored from nothing");
    feed(cx, SessionEvent::Accumulated { current: 1, total: 4 });
    let legend = model(cx).unwrap().1.unwrap();
    assert_eq!(legend, running_legend(4, 1, true));
    assert_eq!(legend[3], Some(legend_color(0, 4)), "the last frame taken first");
    assert_eq!(&legend[..3], &[None, None, None]);

    // Blending: every frame is taken in.
    feed(cx, SessionEvent::Finalized);
    let (of, legend, ..) = model(cx).unwrap();
    assert_eq!(of, RailOf::Run { taken: 4 });
    assert!(legend.unwrap().iter().all(Option::is_some), "all colored while blending");
    feed(cx, SessionEvent::Blended { current: 1, total: 2 });
    assert_eq!(model(cx).unwrap().0, RailOf::Run { taken: 4 });

    // Read-only: no hover, press, drag, wheel or key does anything.
    let mode = |cx: &mut TestAppContext| app.read_with(cx, |app, cx| app.viewer.read(cx).mode());
    let before = mode(cx);
    let shown =
        |cx: &mut TestAppContext| app.read_with(cx, |app, cx| app.viewer.read(cx).shown_file());
    let shown_before = shown(cx);
    let gesture = |cx: &mut TestAppContext| app.read_with(cx, |app, _| app.scrub_gesture());
    let rail = bar_part(cx, window, "focus-rail").expect("the rail");
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    vcx.simulate_mouse_move(tick(rail, 1, 4), None, Default::default());
    assert_eq!(gesture(cx), Scrub::Idle, "no hover");
    assert!(bar_part(cx, window, "rail-label").is_none(), "no label");
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    vcx.simulate_mouse_down(tick(rail, 1, 4), gpui::MouseButton::Left, Default::default());
    vcx.simulate_mouse_move(tick(rail, 3, 4), gpui::MouseButton::Left, Default::default());
    vcx.simulate_mouse_up(tick(rail, 3, 4), gpui::MouseButton::Left, Default::default());
    assert_eq!(gesture(cx), Scrub::Idle, "no drag");
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    vcx.simulate_event(gpui::ScrollWheelEvent {
        position: tick(rail, 1, 4),
        delta: gpui::ScrollDelta::Lines(gpui::point(0.0, -1.0)),
        modifiers: Default::default(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    cx.simulate_keystrokes(window, "right");
    cx.simulate_keystrokes(window, "left");
    act(cx, window, &app, |app, window, cx| app.scrub_by(1, window, cx));
    assert_eq!(model(cx).unwrap().2, None, "no frame marked");
    assert_eq!(mode(cx), before, "the view is as it was");
    assert_eq!(shown(cx), shown_before, "no frame shown alone");

    // Ended with no result: the rail goes with the run.
    act(cx, window, &app, |app, _, cx| app.stacking.update(cx, |s, cx| s.end_held(cx)));
    cx.run_until_parked();
    assert!(model(cx).is_none(), "no run, no result: no rail");
    assert!(bar_part(cx, window, "focus-rail").is_none());
}

/// A real run shows its rail from the start; done, the result's rail has
/// the colors the run gave each frame. Canceled over a render, the render's
/// rail returns with no run colors.
#[gpui::test]
fn the_rail_goes_back_to_a_result_after_a_run(cx: &mut TestAppContext) {
    use crate::app::scrubber::RailOf;
    use deptheos_app::engine::present::running_legend;
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    cx.update(|cx| cx.bind_keys(key_bindings()));
    wait_until(cx, "the images", |cx| app.read_with(cx, |app, cx| app.files.read(cx).can_stack()));
    let of =
        |cx: &mut TestAppContext| app.read_with(cx, |app, cx| app.rail_model(cx).map(|m| m.of));
    let started = act(cx, window, &app, |app, window, cx| {
        app.run_stacking(window, cx);
        app.rail_model(cx).map(|m| (m.of, m.legend))
    });
    assert_eq!(started, Some((RailOf::Run { taken: 0 }, Some(vec![None; 3]))));
    wait_until(cx, "the stack and its save", |cx| {
        app.read_with(cx, |app, cx| !app.is_processing(cx) && !app.renders.read(cx).is_saving())
    });
    assert_eq!(of(cx), Some(RailOf::Result), "completed: the result's rail");
    let first = app.read_with(cx, |app, cx| app.stacking.read(cx).result().unwrap().clone());
    cx.simulate_keystrokes(window, "m");
    let legend = app.read_with(cx, |app, cx| app.rail_model(cx).and_then(|m| m.legend)).unwrap();
    let running = running_legend(3, 3, false);
    for (i, color) in legend.iter().enumerate() {
        if !first.geometry.is_left_out(i) {
            assert_eq!(*color, running[i], "frame {i} keeps the run's color");
        }
    }
    cx.simulate_keystrokes(window, "m");
    let rendered = active(cx, &app).expect("saved as a render");

    // Again, canceled at once: back to the render's rail.
    let started = act(cx, window, &app, |app, window, cx| {
        app.run_stacking(window, cx);
        let of = app.rail_model(cx).map(|m| m.of);
        app.cancel_run(window, cx);
        of
    });
    assert_eq!(started, Some(RailOf::Run { taken: 0 }));
    wait_until(cx, "the render again", |cx| {
        app.read_with(cx, |app, cx| {
            !app.is_processing(cx)
                && !app.renders.read(cx).is_saving()
                && app.stacking.read(cx).result().is_some()
        })
    });
    assert_eq!(active(cx, &app), Some(rendered), "the render shown before");
    let model = app.read_with(cx, |app, cx| app.rail_model(cx)).expect("its rail");
    assert_eq!(model.of, RailOf::Result);
    assert_eq!(model.paths, first.paths, "the render's frames");
    assert_eq!(model.legend, None, "no color left from the run");
    assert!(app.read_with(cx, |app, cx| app.stacking.read(cx).taken().is_none()));
}
