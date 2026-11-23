//! Notices: what a stack, an export or a fault says.

use super::*;

#[test]
fn a_stack_not_kept_says_so() {
    assert_eq!(
        stacked_message(1.25, None, None, None, &crate::words::DesktopWords::english()),
        ("Stacked in 1.2 s".into(), false)
    );
    assert_eq!(
        stacked_message(12.3, Some(1), None, None, &crate::words::DesktopWords::english()),
        ("Stacked in 12.3 s · #1".into(), false),
        "a kept render's number"
    );
    let (message, warn) = stacked_message(
        2.0,
        None,
        Some("1 frame left out"),
        None,
        &crate::words::DesktopWords::english(),
    );
    assert_eq!(message, "Stacked in 2.0 s. 1 frame left out.");
    assert!(warn);
    let (message, warn) = stacked_message(
        2.0,
        None,
        None,
        Some(NotKept::NoThumbnail),
        &crate::words::DesktopWords::english(),
    );
    assert!(message.contains("wasn't kept in the Library, because its thumbnail"), "{message}");
    assert!(warn);
}

/// A fault notice offers Save Diagnostics… and Show Log Folder after its own
/// buttons; a user-error notice doesn't.
#[gpui::test]
fn a_fault_notice_offers_the_way_to_its_details(cx: &mut TestAppContext) {
    use crate::notices::{Kind, Subject};
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 0) else { return };
    let buttons = |cx: &mut TestAppContext| {
        let mut vcx = gpui::VisualTestContext::from_window(window, cx);
        ["notice-action-0", "notice-action-1", "notice-action-2", "notice-action-3"]
            .into_iter()
            .filter(|id| vcx.debug_bounds(id).is_some())
            .count()
    };
    act(cx, window, &app, |app, window, cx| {
        let text = crate::notices::fault(cx.wording(), "The render couldn't be exported.", "x");
        let own = app.action("Again", |_, _, _| {}, cx);
        app.post(Kind::Error, Subject::Export, text, vec![own], window, cx)
    });
    cx.run_until_parked();
    assert_eq!(buttons(cx), 3, "its own, then Save Diagnostics… and Show Log Folder");
    act(cx, window, &app, |app, window, cx| {
        let text = crate::notices::notice(
            cx.wording(),
            "The stack didn't finish.",
            &deptheos_app::Error::NoFrames,
        );
        app.post(Kind::Error, Subject::Export, text, Vec::new(), window, cx)
    });
    cx.run_until_parked();
    assert_eq!(buttons(cx), 0, "it replaced the fault's, with no buttons of its own");
}

/// A notice sits below the title bar, clear of its buttons, inset from the
/// right edge.
#[gpui::test]
fn a_notice_sits_below_the_title_bar(cx: &mut TestAppContext) {
    use crate::notices::{Kind, Subject};
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 0) else { return };
    act(cx, window, &app, |app, window, cx| {
        let own = app.action("Again", |_, _, _| {}, cx);
        app.post(
            Kind::Error,
            Subject::Export,
            "The render couldn't be exported.",
            vec![own],
            window,
            cx,
        )
    });
    // The notice slides in from above over 400 ms of real time.
    std::thread::sleep(std::time::Duration::from_millis(600));
    cx.update_window(window, |_, window, _| window.refresh()).unwrap();
    cx.run_until_parked();
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    let button = vcx.debug_bounds("notice-action-0").expect("the notice's button is drawn");
    let window_width = px(1280.0);
    assert!(button.top() > px(crate::app::toolbar::TITLE_BAR_HEIGHT + 12.0), "{button:?}");
    assert!(button.right() < window_width - px(12.0), "{button:?}");
}

/// A stack that went as asked ends in the inspector's footer, not a notice
/// over the Method picker. The line stays while its render matches the
/// settings; with the inspector hidden it is still a notice.
#[gpui::test]
fn a_stack_that_went_as_asked_ends_in_the_footer(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 2) else { return };
    let notices = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| window.notifications(cx).len()).unwrap()
    };
    let line = |cx: &mut TestAppContext| {
        app.read_with(cx, |app, cx| app.stacked_line(&app.snapshot(cx), cx))
    };
    assert_eq!(line(cx), None, "nothing stacked yet");
    stack(cx, window, &app);
    assert_eq!(notices(cx), 0, "no notice over the picker");
    let said = line(cx).expect("the footer says it");
    assert!(said.starts_with("Stacked in ") && said.ends_with(" s · #1"), "{said}");
    cx.run_until_parked();
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    assert!(vcx.debug_bounds("stacked-line").is_some(), "the line is drawn");
    let picker = vcx.debug_bounds("settings-method").expect("the inspector shows Method");
    let alignment = vcx.debug_bounds("settings-alignment").expect("and Alignment's row");
    assert!(picker.top() < alignment.top(), "Method leads: {picker:?} vs {alignment:?}");

    // A setting moved: the line was about the render, not about this.
    let smoother = StackerConfig::Pyramid(deptheos_app::PyramidConfig { smoothness: 6.5 });
    act(cx, window, &app, |app, window, cx| {
        app.params.update(cx, |p, cx| p.restore(&smoother, &SessionOptions::default(), window, cx))
    });
    assert_eq!(line(cx), None, "edited since");

    // With the inspector hidden the footer is out of sight: a notice.
    act(cx, window, &app, |app, window, cx| app.toggle_inspector(window, cx));
    stack(cx, window, &app);
    assert_eq!(notices(cx), 1, "said where it can be seen");
    assert!(line(cx).is_some_and(|l| l.ends_with("#2")), "and kept for the footer");

    // Shown again, the next stack's end takes the notice down.
    act(cx, window, &app, |app, window, cx| app.toggle_inspector(window, cx));
    let calmer = StackerConfig::Pyramid(deptheos_app::PyramidConfig { smoothness: 5.5 });
    act(cx, window, &app, |app, window, cx| {
        app.params.update(cx, |p, cx| p.restore(&calmer, &SessionOptions::default(), window, cx))
    });
    stack(cx, window, &app);
    // Closed, and gone once slid out (well before its five-second auto-hide).
    cx.executor().advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    assert_eq!(notices(cx), 0, "the footer says it again");
}

/// A failed export stays until closed, with Export… to retry; the next
/// export's notice replaces it.
#[gpui::test]
fn a_failed_export_stays_until_the_next(cx: &mut TestAppContext) {
    let Some(Fixture { dir, window, app, .. }) = Fixture::launch(cx, 2) else { return };
    stack(cx, window, &app);
    let notices = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| window.notifications(cx).len()).unwrap()
    };
    let export = |cx: &mut TestAppContext, path: PathBuf| {
        act(cx, window, &app, |app, window, cx| {
            let options = crate::prefs::prefs(cx).save_options();
            app.write_to(crate::app::exporting::Written::Shown, path, options, window, cx)
        });
        wait_until(cx, "the export", |cx| app.read_with(cx, |app, _| !app.is_exporting()));
    };
    // The test platform can't show a file in the Finder.
    cx.update(|cx| crate::prefs::update_prefs(cx, |p| p.reveal_export = false));
    let before = notices(cx);
    export(cx, dir.path().join("gone/bee.png"));
    assert_eq!(notices(cx), before + 1, "the failure is said");
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    assert!(vcx.debug_bounds("notice-action-0").is_some(), "with Export…");
    export(cx, dir.path().join("bee.png"));
    assert!(dir.path().join("bee.png").is_file());
    assert_eq!(notices(cx), before + 1, "the export's notice replaced the failure");
}

/// An unsavable render's error replaces its "Stacked in" notice and stays.
#[cfg(unix)]
#[gpui::test]
fn a_failed_save_replaces_the_stacked_notice(cx: &mut TestAppContext) {
    use std::os::unix::fs::PermissionsExt as _;
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let paths = write_frames(dir.path(), 2);
    let history = dir.path().join("history");
    let (window, app) = launch(cx, &engine, paths, &history);
    // Saving a render makes a folder in here.
    let renders = history.join("Renders");
    std::fs::create_dir_all(&renders).unwrap();
    std::fs::set_permissions(&renders, std::fs::Permissions::from_mode(0o555)).unwrap();
    let notices = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| window.notifications(cx).len()).unwrap()
    };
    let before = notices(cx);
    let stacked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        stack(cx, window, &app);
    }));
    std::fs::set_permissions(&renders, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(stacked.is_ok(), "the stack ends");
    app.read_with(cx, |app, cx| {
        assert!(app.has_result(cx));
        assert!(app.renders.read(cx).renders().is_empty(), "nothing was saved");
    });
    assert_eq!(notices(cx), before + 1, "the failure replaced the stack's notice");
    // A Done notice would hide by now; the error stays.
    cx.background_executor.advance_clock(std::time::Duration::from_secs(7));
    cx.run_until_parked();
    assert_eq!(notices(cx), before + 1, "the error stays until closed");
}

/// A stack ending with the window in front notifies in the window only;
/// behind another app, also by system notification.
#[gpui::test]
fn a_stack_that_ends_out_of_sight_posts_a_system_notification(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 2) else { return };
    // The test platform posts nothing without the app identity, as Windows.
    cx.update(|cx| cx.set_app_identity(crate::APP_ID, "Deptheos"));
    cx.update_window(window, |_, window, _| window.activate_window()).unwrap();
    cx.run_until_parked();
    let active = cx.update_window(window, |_, window, _| window.is_window_active()).unwrap();
    assert!(active, "the window is in front");
    stack(cx, window, &app);
    assert!(cx.shown_system_notifications().is_empty(), "the user saw it end");

    gpui::VisualTestContext::from_window(window, cx).deactivate_window();
    stack(cx, window, &app);
    let shown = cx.shown_system_notifications();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].title.as_ref(), "Stack finished");
    assert!(shown[0].body.starts_with("Stacked in "), "{}", shown[0].body);
}

/// Frames added to a stack under way say so with New Stack Instead: to a list
/// with frames, or handed over by the system to a stack with renders; never
/// to a new empty stack.
#[gpui::test]
fn the_added_notice_is_for_a_stack_under_way(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let folders: Vec<PathBuf> = ["ants", "bees", "wasps", "moths"]
        .iter()
        .map(|name| {
            let folder = dir.path().join(name);
            std::fs::create_dir_all(&folder).unwrap();
            write_frames(&folder, 2);
            folder
        })
        .collect();
    let (window, app) = launch(cx, &engine, Vec::new(), &dir.path().join("history"));
    let notices = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| window.notifications(cx).len()).unwrap()
    };
    let clear_notices = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| window.clear_notifications(cx)).unwrap();
        // They fade out before they are gone.
        cx.executor().advance_clock(std::time::Duration::from_secs(2));
        cx.run_until_parked();
    };
    let rows =
        |cx: &mut TestAppContext| app.read_with(cx, |app, cx| app.files.read(cx).rows().count());
    let add = |cx: &mut TestAppContext, folder: &Path, from: AddedFrom, want: usize| {
        let paths = vec![folder.to_path_buf()];
        act(cx, window, &app, |app, window, cx| app.add_paths_from(paths, from, window, cx));
        wait_until(cx, "the frames", |cx| rows(cx) == want);
    };
    clear_notices(cx);

    // Into a new, empty stack, even from outside: no notice.
    add(cx, &folders[0], AddedFrom::Outside, 2);
    assert_eq!(notices(cx), 0);
    // Into a list with frames, never stacked: said.
    add(cx, &folders[1], AddedFrom::Window, 4);
    assert_eq!(notices(cx), 1);
    clear_notices(cx);

    // A stack with a render, all frames removed: silent from the window,
    // said from outside.
    stack(cx, window, &app);
    act(cx, window, &app, |app, window, cx| app.files.update(cx, |f, cx| f.clear(window, cx)));
    clear_notices(cx);
    add(cx, &folders[2], AddedFrom::Window, 2);
    assert_eq!(notices(cx), 0);
    act(cx, window, &app, |app, window, cx| app.files.update(cx, |f, cx| f.clear(window, cx)));
    clear_notices(cx);
    add(cx, &folders[3], AddedFrom::Outside, 2);
    assert_eq!(notices(cx), 1);
}
