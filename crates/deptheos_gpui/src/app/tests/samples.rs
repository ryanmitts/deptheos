//! Try a Sample Stack.

use super::*;

/// A small hornet for Try a Sample Stack: two JPEG frames in `dir`.
fn write_sample(dir: &Path) -> Arc<crate::app::sample::Sample> {
    write_sample_of(crate::app::sample::SampleStack::Hornet, "hornet", dir)
}

/// A small `stack` for Try a Sample Stack: two JPEG frames named for
/// `prefix`, in a folder of its own in `dir`.
fn write_sample_of(
    stack: crate::app::sample::SampleStack,
    prefix: &str,
    dir: &Path,
) -> Arc<crate::app::sample::Sample> {
    let shipped = dir.join("shipped").join(prefix);
    std::fs::create_dir_all(&shipped).unwrap();
    for (n, png) in write_quarter_blurred_frames(dir).into_iter().enumerate() {
        image::open(&png).unwrap().save(shipped.join(format!("{prefix}-0{}.jpg", n + 1))).unwrap();
    }
    Arc::new(crate::app::sample::Sample::read(stack, &shipped).expect("a sample"))
}

/// Try a Sample Stack from the Library over a user stack: frames are copied
/// to Samples/Hornet and added to a new stack named Hornet, the user's left
/// as it was, nothing stacked. Tried again it makes another stack from the
/// same copy; during a run it does nothing.
#[gpui::test]
fn trying_the_sample_adds_a_copy_to_a_new_stack(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("history");
    let own_dir = dir.path().join("own");
    std::fs::create_dir_all(&own_dir).unwrap();
    let own = write_frames(&own_dir, 3);
    let (window, app) = launch(cx, &engine, own.clone(), &root);
    stack(cx, window, &app);
    let first = app.read_with(cx, |app, _| app.stack.clone());
    let sample = write_sample(dir.path());
    app.update(cx, |app, _| app.samples = vec![sample]);
    act(cx, window, &app, |app, window, cx| app.set_screen(Screen::Library, window, cx));

    act(cx, window, &app, |app, window, cx| {
        app.try_sample(Some(crate::app::sample::SampleStack::Hornet), window, cx)
    });
    crate::test_support::switched(cx, &app);
    app.read_with(cx, |app, cx| {
        assert_eq!(app.screen, Screen::Stacking);
        assert_ne!(app.stack, first, "a new stack");
        // Two small files may be copied by the time the switch is seen
        // through; while they are not, the welcome says so.
        if app.sample_open.copying().is_some() {
            assert!(app.is_empty(cx), "the welcome says it copies");
            assert!(!app.menu_state(cx).can_try_sample, "once at a time");
        }
    });
    let copy = root.join("Samples").join("Hornet");
    let copied = [copy.join("hornet-01.jpg"), copy.join("hornet-02.jpg")];
    wait_until(cx, "the sample's frames", |cx| {
        app.read_with(cx, |app, cx| {
            !app.sample_open.copying().is_some()
                && !app.files.read(cx).sorting()
                && app.files.read(cx).stack_frames() == copied
        })
    });
    let sampled = app.read_with(cx, |app, cx| {
        assert!(matches!(app.sample_open, crate::app::sample::SampleOpen::Idle));
        assert!(!app.is_processing(cx), "nothing stacks until Stack");
        assert!(app.can_stack(cx), "and Stack is ready");
        let renders = app.renders.read(cx);
        assert!(!renders.has_renders(&app.stack), "no render of it yet");
        assert!(renders.has_renders(&first), "the user's stack stays");
        app.stack.clone()
    });
    assert!(own.iter().all(|p| p.exists()));

    // Stacked by the user, it is named for its folder.
    stack(cx, window, &app);
    app.read_with(cx, |app, cx| {
        assert_eq!(app.stack, sampled);
        assert_eq!(app.renders.read(cx).stack_name(&app.stack, cx.wording()), "Hornet");
    });

    // Again: a new stack, and nothing copied again.
    let stamp = |p: &Path| std::fs::metadata(p).unwrap().modified().unwrap();
    let before: Vec<_> = copied.iter().map(|p| stamp(p)).collect();
    act(cx, window, &app, |app, window, cx| {
        app.try_sample(Some(crate::app::sample::SampleStack::Hornet), window, cx)
    });
    crate::test_support::switched(cx, &app);
    wait_until(cx, "the second stack's frames", |cx| {
        app.read_with(cx, |app, cx| {
            !app.sample_open.copying().is_some()
                && !app.files.read(cx).sorting()
                && app.files.read(cx).stack_frames() == copied
        })
    });
    app.read_with(cx, |app, cx| {
        assert!(app.stack != sampled && app.stack != first);
        assert!(!app.is_processing(cx));
    });
    assert_eq!(copied.iter().map(|p| stamp(p)).collect::<Vec<_>>(), before);

    // While a stack runs: nothing.
    let running = app.read_with(cx, |app, _| app.stack.clone());
    act(cx, window, &app, |app, window, cx| {
        app.run_stacking(window, cx);
        assert!(!app.menu_state(cx).can_try_sample);
        app.try_sample(Some(crate::app::sample::SampleStack::Hornet), window, cx);
    });
    crate::test_support::switched(cx, &app);
    app.read_with(cx, |app, _| {
        assert_eq!(app.stack, running);
        assert!(matches!(app.sample_open, crate::app::sample::SampleOpen::Idle));
    });
    wait_until(cx, "that stack", |cx| {
        app.read_with(cx, |app, cx| !app.is_processing(cx) && !app.renders.read(cx).is_saving())
    });
}

/// A sample that can't be copied (the Library folder's Samples is a file)
/// is said in a notice, and the new stack stays empty.
#[gpui::test]
fn a_sample_that_cannot_be_copied_is_said(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("history");
    let (window, app) = launch(cx, &engine, Vec::new(), &root);
    std::fs::write(root.join("Samples"), b"in the way").unwrap();
    let sample = write_sample(dir.path());
    app.update(cx, |app, _| app.samples = vec![sample]);
    let notices =
        |cx: &mut TestAppContext| cx.update_window(window, |_, w, cx| w.notifications(cx).len());
    let before = notices(cx).unwrap();
    act(cx, window, &app, |app, window, cx| {
        app.try_sample(Some(crate::app::sample::SampleStack::Hornet), window, cx)
    });
    crate::test_support::switched(cx, &app);
    wait_until(cx, "the copy", |cx| {
        app.read_with(cx, |app, _| matches!(app.sample_open, crate::app::sample::SampleOpen::Idle))
    });
    app.read_with(cx, |app, cx| {
        assert!(app.files.read(cx).list().is_empty());
        assert!(!app.is_processing(cx));
        assert!(app.can_try_sample(cx), "it can be tried again");
    });
    assert_eq!(notices(cx).unwrap(), before + 1);
}

/// The circuit board, both samples carried: copied to Samples/Circuit Board
/// beside the hornet's (unchanged), in a new stack named Circuit Board.
#[gpui::test]
fn trying_the_circuit_board_copies_it_beside_the_hornet(cx: &mut TestAppContext) {
    use crate::app::sample::SampleStack;
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("history");
    let (window, app) = launch(cx, &engine, Vec::new(), &root);
    let hornet = write_sample_of(SampleStack::Hornet, "hornet", dir.path());
    let board = write_sample_of(SampleStack::CircuitBoard, "pcb", dir.path());
    app.update(cx, |app, _| app.samples = vec![hornet, board]);
    let hornet_copy = root.join("Samples").join("Hornet");
    std::fs::create_dir_all(&hornet_copy).unwrap();
    std::fs::write(hornet_copy.join("hornet-01.jpg"), b"the user's").unwrap();
    let empty = app.read_with(cx, |app, _| app.stack.clone());

    act(cx, window, &app, |app, window, cx| {
        app.try_sample(Some(SampleStack::CircuitBoard), window, cx)
    });
    crate::test_support::switched(cx, &app);
    app.read_with(cx, |app, cx| {
        // Two small files may be copied by the time the switch is seen through.
        if let Some(copying) = app.sample_open.copying() {
            assert_eq!(copying, SampleStack::CircuitBoard);
            assert!(!app.menu_state(cx).can_try_sample, "neither while it copies");
        }
    });
    let copy = root.join("Samples").join("Circuit Board");
    let copied = [copy.join("pcb-01.jpg"), copy.join("pcb-02.jpg")];
    wait_until(cx, "the board's frames", |cx| {
        app.read_with(cx, |app, cx| {
            app.sample_open.copying().is_none()
                && !app.files.read(cx).sorting()
                && app.files.read(cx).stack_frames() == copied
        })
    });
    app.read_with(cx, |app, cx| {
        assert_eq!(app.stack, empty, "the empty stack on screen is the new one");
        assert!(!app.is_processing(cx), "nothing stacks until Stack");
    });
    let names: Vec<_> =
        std::fs::read_dir(&hornet_copy).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(names, ["hornet-01.jpg"]);
    assert_eq!(std::fs::read(hornet_copy.join("hornet-01.jpg")).unwrap(), b"the user's");

    stack(cx, window, &app);
    app.read_with(cx, |app, cx| {
        assert_eq!(app.renders.read(cx).stack_name(&app.stack, cx.wording()), "Circuit Board");
    });
}

/// The welcome's Try a sample stack menu lists both samples, the hornet
/// first; choosing the circuit board copies its 200 frames and adds them.
#[gpui::test]
fn the_welcome_offers_both_samples(cx: &mut TestAppContext) {
    use crate::app::sample::SampleStack;
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("history");
    let (window, app) = launch(cx, &engine, Vec::new(), &root);
    app.read_with(cx, |app, cx| assert!(app.is_empty(cx), "the welcome"));
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    let link = vcx.debug_bounds("welcome-sample").expect("Try a sample stack");
    vcx.simulate_click(link.center(), gpui::Modifiers::none());
    let hornet = vcx.debug_bounds("welcome-sample: Hornet").expect("the hornet in its menu");
    let board = vcx.debug_bounds("welcome-sample: Circuit Board").expect("the board in its menu");
    assert!(hornet.bottom() <= board.top(), "the hornet first: {hornet:?} {board:?}");
    vcx.simulate_click(board.center(), gpui::Modifiers::none());
    app.read_with(&vcx, |app, _| {
        assert_eq!(app.sample_open.copying(), Some(SampleStack::CircuitBoard));
    });
    let copied = root.join("Samples").join("Circuit Board").join("pcb-200.jpg");
    wait_until(cx, "the board's 200 frames", |cx| {
        app.read_with(cx, |app, cx| {
            app.sample_open.copying().is_none()
                && !app.files.read(cx).sorting()
                && app.files.read(cx).stack_frames().last() == Some(&copied)
        })
    });
    app.read_with(cx, |app, cx| assert_eq!(app.files.read(cx).stack_frames().len(), 200));
}
