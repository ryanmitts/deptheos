//! Comparisons: with a frame, with another render, and the depth map's light.

use super::*;

/// C turns on Contribution with a result: the compare source reloads with
/// the result's share as is and the rest dimmed; the first frame keeps its
/// sharp right quarter and dims its blurred left.
#[gpui::test]
fn contribution_lights_the_compare_source(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let paths = write_quarter_blurred_frames(dir.path());
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
    wait_until(cx, "the preview", |cx| {
        app.read_with(cx, |app, cx| app.viewer.read(cx).has_content())
    });
    cx.update(|cx| cx.bind_keys(key_bindings()));

    // Nothing to contribute to before a stack: C does nothing.
    cx.simulate_keystrokes(window, "c");
    app.read_with(cx, |app, cx| {
        assert_eq!(app.viewer.read(cx).overlay(), deptheos_app::Overlay::None)
    });

    stack(cx, window, &app);
    act(cx, window, &app, |app, window, cx| app.set_view(Look::SideBySide, window, cx));
    let settled =
        |cx: &mut TestAppContext| !app.read_with(cx, |app, cx| app.viewer.read(cx).is_loading());
    wait_until(cx, "the compare source", settled);
    app.read_with(cx, |app, cx| assert!(app.has_result(cx), "the stack ran"));
    let compare = |cx: &mut TestAppContext| {
        app.read_with(cx, |app, cx| app.viewer.read(cx).compare_pixels().unwrap())
    };
    let plain = compare(cx);

    cx.simulate_keystrokes(window, "c");
    app.read_with(cx, |app, cx| {
        assert_eq!(app.viewer.read(cx).overlay(), deptheos_app::Overlay::Contribution);
    });
    wait_until(cx, "the tinted compare source", settled);
    let tinted = compare(cx);
    assert_eq!(tinted.len(), plain.len());
    // One tile at this size (the crop trims a little), with its 1 px gutter.
    let (w, h) = app.read_with(cx, |app, cx| app.viewer.read(cx).compare_size().unwrap());
    let (w, h) = (w as usize + 2, h as usize + 2);
    assert_eq!(tinted.len(), w * h * 4);
    let changed = |x_from: usize, x_to: usize| {
        (0..h)
            .flat_map(|y| (x_from..x_to).map(move |x| (y * w + x) * 4))
            .filter(|&i| plain[i..i + 4] != tinted[i..i + 4])
            .count()
    };
    // Clear of the blur edge, the border and the pyramid's odd checkerboard
    // corner picks (see the engine's test).
    let (margin, border) = (16, 8);
    let own = changed(w * 3 / 4 + margin, w - border);
    assert!(own < (w / 4 - margin - border) * h / 10, "the sharp quarter is as it was ({own})");
    let other = changed(border, w / 4 - margin);
    assert!(other > (w / 4 - margin - border) * h * 9 / 10, "the blurred quarter is dim ({other})");

    // C again turns it off.
    cx.simulate_keystrokes(window, "c");
    app.read_with(cx, |app, cx| {
        assert_eq!(app.viewer.read(cx).overlay(), deptheos_app::Overlay::None)
    });
}

/// Deleting the render pinned as A ends the compare.
#[gpui::test]
fn deleting_the_compared_render_ends_the_compare(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    stack(cx, window, &app);
    let first = active(cx, &app).unwrap();
    exclude_first(cx, &app);
    stack(cx, window, &app);
    act(cx, window, &app, |app, window, cx| app.compare_render(first.clone(), window, cx));
    wait_until(cx, "the compare", |cx| {
        app.read_with(cx, |app, cx| !app.viewer.read(cx).is_loading())
    });
    app.read_with(cx, |app, cx| assert!(app.viewer.read(cx).has_compare()));
    act(cx, window, &app, |app, window, cx| {
        app.renders.update(cx, |r, cx| r.delete(vec![first], window, cx))
    });
    wait_until(cx, "the delete", |cx| {
        app.read_with(cx, |app, cx| app.renders.read(cx).renders().len() == 1)
    });
    app.read_with(cx, |app, cx| assert!(!app.viewer.read(cx).has_compare()));
}

/// Two renders compared: A beside the result, both labeled; Swap makes A the
/// result with B pinned; Esc returns to the result alone; a source image
/// takes the panes over.
#[gpui::test]
fn compares_two_renders(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    let on_window = |cx: &mut TestAppContext,
                     f: fn(&mut AppView, &mut Window, &mut Context<AppView>)| {
        act(cx, window, &app, f);
    };
    let settled = |cx: &mut TestAppContext| {
        wait_until(cx, "the viewer", |cx| {
            app.read_with(cx, |app, cx| {
                !app.viewer.read(cx).is_loading() && !app.renders.read(cx).is_saving()
            })
        })
    };
    let state = |cx: &mut TestAppContext| {
        app.read_with(cx, |app, cx| {
            let renders = app.renders.read(cx);
            (renders.active(cx).cloned(), renders.compare().cloned())
        })
    };

    stack(cx, window, &app);
    let first = state(cx).0.expect("the stack is shown as its render");
    exclude_first(cx, &app);
    stack(cx, window, &app);
    let second = state(cx).0.unwrap();
    assert_ne!(first, second);
    // The picker offers the stack's other render, labeled by number.
    let choices = app.read_with(cx, |app, cx| app.stack_renders(cx));
    assert_eq!(choices.iter().map(|c| &c.0).collect::<Vec<_>>(), [&first]);
    assert_eq!(choices[0].1.as_ref(), "#1 · Pyramid · S4");

    act(cx, window, &app, |app, window, cx| app.compare_render(first.clone(), window, cx));
    settled(cx);
    app.read_with(cx, |app, cx| {
        let viewer = app.viewer.read(cx);
        assert!(viewer.has_compare());
        assert_eq!(viewer.mode().look(), Some(Look::Split), "the layout used last");
        let (a, b) = viewer.shown_labels().expect("both panes labeled");
        assert!(b.starts_with("B · ") && b.ends_with(" · Pyramid · S4"), "{b}");
        assert!(a.starts_with("A · ") && a.ends_with(" · Pyramid · S4"), "{a}");
    });
    assert_eq!(state(cx), (Some(second.clone()), Some(first.clone())));

    // Flicker flips with each press; S swaps the sides.
    on_window(cx, |app, window, cx| app.set_view(Look::Flicker, window, cx));
    app.read_with(cx, |app, cx| assert!(app.viewer.read(cx).flicker_shows_source()));
    on_window(cx, |app, window, cx| app.set_view(Look::Flicker, window, cx));
    app.read_with(cx, |app, cx| assert!(!app.viewer.read(cx).flicker_shows_source()));
    on_window(cx, |app, window, cx| app.set_view(Look::SideBySide, window, cx));
    app.update(cx, |app, cx| app.viewer.update(cx, |v, cx| v.on(ViewEvent::SwapSides, cx)));
    app.read_with(cx, |app, cx| assert!(app.viewer.read(cx).sides_swapped()));

    // Swapped: the first render is the result, the second pinned as A; the
    // list stays the working version's.
    on_window(cx, |app, window, cx| app.swap_render_compare(window, cx));
    wait_until(cx, "the swap", |cx| {
        app.read_with(cx, |app, cx| app.renders.read(cx).active(cx) == Some(&first))
    });
    settled(cx);
    assert_eq!(state(cx), (Some(first.clone()), Some(second.clone())));
    app.read_with(cx, |app, cx| {
        assert_eq!(app.files.read(cx).summary().as_ref(), "2 of 3 frames");
        assert!(app.viewer.read(cx).has_compare());
    });

    // A click on the compared render's tile swaps them back, comparing on.
    act(cx, window, &app, |app, window, cx| {
        app.renders.update(cx, |r, cx| r.click_tile(&second, window, cx))
    });
    wait_until(cx, "the click", |cx| {
        app.read_with(cx, |app, cx| app.renders.read(cx).active(cx) == Some(&second))
    });
    settled(cx);
    assert_eq!(state(cx), (Some(second.clone()), Some(first.clone())));
    assert!(app.read_with(cx, |app, cx| app.viewer.read(cx).has_compare()));

    // Esc: the result alone, the sides back as they were.
    cx.dispatch_action(window, StopCompare);
    cx.run_until_parked();
    app.read_with(cx, |app, cx| {
        let viewer = app.viewer.read(cx);
        assert!(!viewer.has_compare() && viewer.mode() == ViewMode::Result);
        assert!(!viewer.sides_swapped());
    });
    assert_eq!(state(cx).1, None);

    // Pinned again, then a source image is picked: it takes the panes.
    act(cx, window, &app, |app, window, cx| app.compare_render(second.clone(), window, cx));
    settled(cx);
    let id = app.read_with(cx, |app, cx| app.files.read(cx).rows().nth(1).unwrap().0);
    act(cx, window, &app, |app, window, cx| app.show_file(id, window, cx));
    settled(cx);
    assert_eq!(state(cx).1, None);
    app.read_with(cx, |app, cx| {
        let (source, result) = app.viewer.read(cx).shown_labels().unwrap();
        assert_eq!((source.as_ref(), result.as_ref()), ("frame_1.png", "Result"));
    });

    // Another tile clicked becomes the result, the frame still compared.
    act(cx, window, &app, |app, window, cx| {
        app.renders.update(cx, |r, cx| r.click_tile(&first, window, cx))
    });
    wait_until(cx, "the click", |cx| {
        app.read_with(cx, |app, cx| app.renders.read(cx).active(cx) == Some(&first))
    });
    settled(cx);
    app.read_with(cx, |app, cx| {
        let viewer = app.viewer.read(cx);
        assert!(viewer.has_compare() && viewer.mode().comparing().is_some());
        assert_eq!(viewer.shown_file(), Some(id), "the same frame");
    });

    // The home menu names renders by number; picking one while a frame is
    // compared shows it, the frame still compared.
    let rows = app.read_with(cx, |app, cx| app.stack_render_rows(cx));
    assert_eq!(rows.iter().map(|r| r.1.as_ref()).collect::<Vec<_>>(), ["#2"]);
    act(cx, window, &app, |app, window, cx| app.pick_home_render(second.clone(), window, cx));
    wait_until(cx, "the pick", |cx| {
        app.read_with(cx, |app, cx| app.renders.read(cx).active(cx) == Some(&second))
    });
    settled(cx);
    assert_eq!(state(cx).1, None, "no render compared");
    app.read_with(cx, |app, cx| {
        assert_eq!(app.viewer.read(cx).shown_file(), Some(id), "the same frame");
    });
}

/// The map's light carries across in Stacking: from a comparison with
/// Contribution, the map opens lit for its frame; a Frames list click lights
/// its frame as its tick does; leaving lands on the lit frame as source with
/// Contribution. Without Contribution the map opens whole and returns to the
/// comparison as it was.
#[gpui::test]
fn the_light_carries_across_the_map(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 4) else {
        return;
    };
    cx.update(|cx| cx.bind_keys(key_bindings()));
    stack(cx, window, &app);
    let stacked = app.read_with(cx, |app, cx| app.stacking.read(cx).result().unwrap().clone());
    assert!(stacked.frame_map.is_some(), "the stack records where its detail came from");
    let settled =
        |cx: &mut TestAppContext| !app.read_with(cx, |app, cx| app.viewer.read(cx).is_loading());
    let mode = |cx: &mut TestAppContext| app.read_with(cx, |app, cx| app.viewer.read(cx).mode());
    let overlay =
        |cx: &mut TestAppContext| app.read_with(cx, |app, cx| app.viewer.read(cx).overlay());
    let current = |cx: &mut TestAppContext| {
        app.read_with(cx, |app, cx| app.rail_model(cx).and_then(|m| m.current))
    };

    // Frame 1 compared Side by Side, with Contribution on.
    act(cx, window, &app, |app, window, cx| app.set_view(Look::SideBySide, window, cx));
    act(cx, window, &app, |app, window, cx| app.scrub_to(1, window, cx));
    wait_until(cx, "the frame", settled);
    cx.simulate_keystrokes(window, "c");
    assert_eq!(overlay(cx), deptheos_app::Overlay::Contribution);
    wait_until(cx, "the frame", settled);
    let side = mode(cx);
    let comparing = side.comparing().expect("Side by Side").clone();
    let lit = |cx: &mut TestAppContext| match mode(cx) {
        ViewMode::Map { back, light } => {
            let lit = match light {
                Light::Lit { frame, .. } => stacked.paths.iter().position(|p| *p == frame),
                Light::Whole | Light::Marked { .. } => None,
            };
            Some((back, lit))
        }
        _ => None,
    };

    // The map opens lit for it.
    cx.simulate_keystrokes(window, "m");
    let back = Back::Compare(comparing);
    assert_eq!(lit(cx), Some((back.clone(), Some(1))));

    // A click in the Frames list lights its frame; the map stays.
    let row = app.read_with(cx, |app, cx| app.files.read(cx).row_of(&stacked.paths[2]).unwrap());
    act(cx, window, &app, |app, window, cx| app.show_file(row, window, cx));
    assert_eq!(lit(cx), Some((back.clone(), Some(2))), "lit by the list");
    assert_eq!(current(cx), Some(2));
    app.read_with(cx, |app, cx| assert_eq!(app.files.read(cx).selected(cx), Some(row)));
    assert!(settled(cx), "no frame loads under the map");

    // Leaving lands on frame 2, as the comparison's source, Contribution on.
    cx.simulate_keystrokes(window, "m");
    assert_eq!(mode(cx), side, "the comparison it came from");
    assert_eq!(current(cx), Some(2), "with the frame lit");
    assert_eq!(overlay(cx), deptheos_app::Overlay::Contribution);
    wait_until(cx, "the frame", settled);
    app.read_with(cx, |app, cx| assert_eq!(app.viewer.read(cx).shown_frame_index(), Some(2)));

    // Contribution off: the map opens whole, and goes back as it was.
    cx.simulate_keystrokes(window, "c");
    assert_eq!(overlay(cx), deptheos_app::Overlay::None);
    cx.simulate_keystrokes(window, "m");
    let plain = Back::Compare(Compare {
        target: Target::Frame {
            overlay: deptheos_app::view::Overlaid {
                chosen: deptheos_app::Overlay::None,
                supplied: true,
            },
        },
        ..side.comparing().unwrap().clone()
    });
    assert_eq!(lit(cx), Some((plain, None)), "whole");
    cx.simulate_keystrokes(window, "m");
    assert_eq!(mode(cx).comparing().map(|c| c.layout), Some(Layout::SideBySide));
    assert_eq!(current(cx), Some(2));
    assert_eq!(overlay(cx), deptheos_app::Overlay::None);
}
