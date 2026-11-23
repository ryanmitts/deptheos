//! Batches: running, canceling, their exports and turntables.

use super::*;
use deptheos_app::library::history::BatchLabel;

/// Cancel during a batch ends it: renders made are kept, nothing more starts.
#[gpui::test]
fn canceling_a_batch_stops_it(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let folders = batch_folders(dir.path(), &["A", "B", "C"], 2);
    let (window, app) = launch(cx, &engine, Vec::new(), &dir.path().join("history"));
    let plan = per_folder_plan(&folders);
    act(cx, window, &app, |app, window, cx| {
        app.start_batch(plan, Batch::new(BatchLabel::default()), None, window, cx);
        // Cancel lands during or just after the first stack: no other starts.
        app.cancel_run(window, cx);
    });
    // Every job is recorded as it starts, hidden while the run lasts.
    app.read_with(cx, |app, cx| assert!(app.renders.read(cx).failures().is_empty()));
    // Settings' converter and profile folders wait for it.
    assert!(cx.update(|cx| crate::prefs::environment_held(cx)));
    wait_until(cx, "the batch", |cx| batch_over(cx, &app));
    assert!(!cx.update(|cx| crate::prefs::environment_held(cx)));
    // What it didn't make is shown once it ends, to try again.
    app.read_with(cx, |app, cx| {
        let renders = app.renders.read(cx);
        let made = renders.renders().len();
        assert!(made <= 1);
        let failures = renders.failures();
        let mut left: Vec<&str> = failures.iter().map(|f| f.label.as_str()).collect();
        left.sort();
        assert_eq!(left, ["A", "B", "C"][made..], "the jobs not made");
        let why = "Not stacked yet: the batch ended before it got to this stack.";
        assert!(failures.iter().all(|f| f.error == why), "{failures:?}");
    });
    // And at the next launch, from the Library.
    assert_eq!(
        crate::test_support::library(cx, &app).failures().unwrap().len(),
        3 - { app.read_with(cx, |app, cx| app.renders.read(cx).renders().len()) }
    );
}

/// A batch started over an older render uses the working version's
/// settings, as Stack does.
#[gpui::test]
fn a_batch_runs_with_the_working_versions_settings(cx: &mut TestAppContext) {
    let Some(Fixture { dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    // #1 with Pyramid, then #2 with Depth: the working version's.
    stack(cx, window, &app);
    let first = active(cx, &app).unwrap();
    let depth = StackerConfig::Depth(DepthConfig { radius: 6.0, smoothing: 2.0, blend_range: 3.0 });
    act(cx, window, &app, |app, window, cx| {
        app.params.update(cx, |p, cx| p.restore(&depth, &SessionOptions::default(), window, cx))
    });
    stack(cx, window, &app);
    // #1 shown: the settings stay the working version's.
    open_render(cx, window, &app, &first);
    app.read_with(cx, |app, cx| {
        assert!(matches!(app.params.read(cx).stacker_config(cx), StackerConfig::Depth(_)));
    });
    let folders = batch_folders(&dir.path().join("batch"), &["A"], 2);
    let plan = per_folder_plan(&folders);
    act(cx, window, &app, |app, window, cx| {
        app.start_batch(plan, Batch::new(BatchLabel::default()), None, window, cx)
    });
    wait_until(cx, "the batch", |cx| batch_over(cx, &app));
    app.read_with(cx, |app, cx| {
        let renders = app.renders.read(cx).renders();
        let made = renders.iter().find(|r| r.batch.is_some()).expect("the batch's render");
        assert!(matches!(made.config, StackerConfig::Depth(_)), "the working version's settings");
    });
}

/// A batch: one render per folder under one batch; the list stays; a failed
/// stack is kept under the batch, and clicking it loads it to retry.
#[gpui::test]
fn a_batch_makes_one_render_per_folder(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let (bee, ant) = (dir.path().join("Bee"), dir.path().join("Ant"));
    std::fs::create_dir_all(&bee).unwrap();
    std::fs::create_dir_all(&ant).unwrap();
    write_frames(&bee, 2);
    write_frames(&ant, 3);
    let listed = write_frames(dir.path(), 2);
    let (window, app) = launch(cx, &engine, listed, &dir.path().join("history"));
    // The list kept, as a launch reads it: its folder is a root, keeping
    // a stack of files gone from it.
    assert!(deptheos_app::engine::store::store().flush(crate::test_support::STORE_DRAINS));

    let mut plan = per_folder_plan(&[bee.clone(), ant.clone()]);
    // A job whose files are gone fails without stopping the run.
    let mut gone = plan.jobs[0].clone();
    gone.label = "Gone".into();
    gone.frames = vec![dir.path().join("x/1.png"), dir.path().join("x/2.png")];
    plan.jobs.insert(1, gone);
    act(cx, window, &app, |app, window, cx| {
        app.start_batch(plan, Batch::new(BatchLabel::default()), None, window, cx)
    });
    app.read_with(cx, |app, cx| {
        assert_eq!(app.batch_progress().as_deref(), Some("Batch 1 of 3 · Bee"));
        // The pill: the batch's line, its stack's step, the whole run's bar.
        let status = app.run_status(cx).expect("a batch runs");
        assert!(status.text.starts_with("Batch 1 of 3 · Bee · "), "{}", status.text);
        match status.bar {
            crate::app::run::Bar::At(f) => assert!((0.0..=1.0 / 3.0).contains(&f), "{f}"),
            crate::app::run::Bar::Busy => panic!("a stack runs: its bar is measured"),
        }
    });
    wait_until(cx, "the batch", |cx| batch_over(cx, &app));
    // One notice, the batch's closing one; the failed stack has none.
    let notices = cx.update_window(window, |_, window, cx| window.notifications(cx).len()).unwrap();
    assert_eq!(notices, 1);
    // Show Failed: the Library with only the failed stack.
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    let show_failed = vcx.debug_bounds("notice-action-0").expect("Show Failed");
    vcx.simulate_click(show_failed.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    app.read_with(cx, |app, cx| {
        assert_eq!(app.screen, Screen::Library);
        assert_eq!(app.library.read(cx).shown_titles(cx), ["Gone"]);
    });
    app.read_with(cx, |app, cx| {
        let renders = app.renders.read(cx).renders();
        assert_eq!(renders.len(), 2);
        let batches: Vec<_> = renders.iter().map(|r| r.batch.clone().unwrap()).collect();
        assert_eq!(batches[0], batches[1], "one run, one batch");
        assert_eq!(batches[0].label, BatchLabel::default());
        assert_eq!(renders[0].display_name().as_deref(), Some("Bee"));
        assert_eq!(renders[1].display_name().as_deref(), Some("Ant"));
        assert_eq!(renders[1].included, [true; 3]);
        // The list on screen was never stacked: no batch render shows for it.
        assert_eq!(app.renders.read(cx).active(cx), None);
        assert_eq!(app.files.read(cx).summary().as_ref(), "2 frames");
        // Each stack of a batch is its own work.
        assert_ne!(renders[0].stack, renders[1].stack);
        assert_eq!((renders[0].number, renders[1].number), (1, 1));
        assert_ne!(renders[0].stack, app.stack, "nor is it the stack on screen");
        assert_eq!(app.files.read(cx).summary().as_ref(), "2 frames", "the list is untouched");
        let failures = app.renders.read(cx).failures();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].label, "Gone");
        assert_eq!(failures[0].batch, batches[0]);
    });
    // It is kept for the next launch, its frames where the folder is.
    let failure = crate::test_support::library(cx, &app).failures().unwrap().remove(0);
    // Named as the app named the gone folder: via the temp link or its target.
    let named = |at: &Path| vec![at.join("x/1.png"), at.join("x/2.png")];
    let at = deptheos_app::frames::sources::canonical(dir.path()).unwrap();
    assert!([named(&at), named(dir.path())].contains(&failure.frames), "{:?}", failure.frames);
    // Clicking it loads its images in place of the list.
    act(cx, window, &app, |app, window, cx| app.retry(&failure, window, cx));
    crate::test_support::switched(cx, &app);
    app.read_with(cx, |app, cx| {
        assert_eq!(app.files.read(cx).list().files, failure.frames);
        assert!(app.stacking.read(cx).result().is_none(), "nothing on screen");
    });
}

/// After stacking: each render exports as made (colliding names get "-2"),
/// a failing folder is skipped, and turntable sequences come from the kept
/// stacks, numbered without gaps, the gap in the manifest.
#[gpui::test]
fn a_turntable_batch_exports_and_writes_its_sequences(cx: &mut TestAppContext) {
    use deptheos_app::runs::sequence::{
        FrameFormat, FrameOptions, TurntableMode, TurntableOptions,
    };

    use crate::batch_dialog::{After, ExportEach, TurntableAfter};
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let shoot = dir.path().join("scan");
    let folders: Vec<PathBuf> = ["s001", "s002", "s003"].iter().map(|n| shoot.join(n)).collect();
    for folder in &folders {
        std::fs::create_dir_all(folder).unwrap();
        write_frames(folder, 3);
    }
    let (exports, sequences) = (dir.path().join("exports"), dir.path().join("sequences"));
    std::fs::create_dir_all(&exports).unwrap();
    std::fs::create_dir_all(&sequences).unwrap();
    // Taken already: s001's export becomes "s001-2.png".
    std::fs::write(exports.join("s001.png"), b"").unwrap();
    let (window, app) = launch(cx, &engine, Vec::new(), &dir.path().join("history"));

    let mut plan = per_folder_plan(&folders);
    let mut gone = plan.jobs[0].clone();
    gone.label = "s002b".into();
    gone.frames = vec![shoot.join("x/1.png"), shoot.join("x/2.png")];
    plan.jobs.insert(2, gone);
    let frames = FrameOptions { format: FrameFormat::Png, ..FrameOptions::default() };
    let after = After {
        export: Some(ExportEach {
            folder: exports.clone(),
            format: ExportFormat::Png,
            options: deptheos_app::SaveOptions::default(),
        }),
        turntable: Some(TurntableAfter {
            folder: sequences.clone(),
            options: TurntableOptions {
                mode: TurntableMode::Both,
                frames,
                ..TurntableOptions::default()
            },
        }),
    };
    act(cx, window, &app, |app, window, cx| {
        app.start_batch_with(plan, Batch::new(BatchLabel::default()), None, after, window, cx)
    });
    wait_until(cx, "the batch and its sequences", |cx| batch_over(cx, &app));

    app.read_with(cx, |app, cx| assert_eq!(app.renders.read(cx).renders().len(), 3));
    assert_eq!(names_in(&exports), ["s001-2.png", "s001.png", "s002.png", "s003.png"]);
    for (kind, count) in [("grow", 3), ("full", 3)] {
        let folder = sequences.join(format!("scan {kind}"));
        let names = names_in(&folder);
        let mut expected: Vec<String> =
            (1..=count).map(|i| format!("scan {kind}_{i:04}.png")).collect();
        expected.push("manifest.json".into());
        expected.sort();
        assert_eq!(names, expected, "{kind}");
        let manifest = std::fs::read_to_string(folder.join("manifest.json")).unwrap();
        assert!(manifest.contains("s002b"), "the failed stack is recorded: {manifest}");
    }
}

/// A turntable of PNG frames, grow and full, into `folder`.
fn turntable_after(folder: &Path) -> crate::batch_dialog::After {
    use deptheos_app::runs::sequence::{
        FrameFormat, FrameOptions, TurntableMode, TurntableOptions,
    };

    use crate::batch_dialog::{After, TurntableAfter};
    let frames = FrameOptions { format: FrameFormat::Png, ..FrameOptions::default() };
    After {
        export: None,
        turntable: Some(TurntableAfter {
            folder: folder.to_path_buf(),
            options: TurntableOptions {
                mode: TurntableMode::Both,
                frames,
                ..TurntableOptions::default()
            },
        }),
    }
}

/// The Library toolbar's Cancel stops a batch's turntable sequence, as the
/// sidebar's does.
#[gpui::test]
fn the_library_cancel_stops_a_batch_sequence(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let folders = batch_folders(&dir.path().join("scan"), &["s001", "s002", "s003", "s004"], 3);
    let sequences = dir.path().join("sequences");
    std::fs::create_dir_all(&sequences).unwrap();
    let (window, app) = launch(cx, &engine, Vec::new(), &dir.path().join("history"));
    let (plan, after) = (per_folder_plan(&folders), turntable_after(&sequences));
    act(cx, window, &app, |app, window, cx| {
        app.start_batch_with(plan, Batch::new(BatchLabel::default()), None, after, window, cx)
    });
    wait_until(cx, "the sequence", |cx| {
        app.read_with(cx, |app, cx| app.sequences.read(cx).is_running())
    });
    app.read_with(cx, |app, cx| {
        let (_, line) = app.sequences.read(cx).progress().unwrap();
        assert_eq!(app.run_status(cx).map(|s| s.text), Some(line.to_string()), "the pill says it");
    });
    app.read_with(cx, |app, cx| {
        assert!(app.can_cancel(cx));
        assert_eq!(
            app.busy_reason(cx).as_deref(),
            Some("Not while a batch runs: wait for it to finish, or cancel it"),
            "New Stack, New Batch and Open say why they are dimmed"
        );
    });
    act(cx, window, &app, |app, window, cx| app.cancel_run(window, cx));
    app.read_with(cx, |app, cx| {
        assert!(app.sequences.read(cx).cancel_requested(), "the sequence is asked to stop");
        assert!(!app.can_cancel(cx), "Cancel is asked once");
    });
    wait_until(cx, "the batch", |cx| batch_over(cx, &app));
}

/// A batch's stacks are its own: the stack on screen, its render and the
/// viewer's image stay as they were.
#[gpui::test]
fn a_batch_leaves_the_stack_on_screen(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let bee = dir.path().join("Bee");
    std::fs::create_dir_all(&bee).unwrap();
    write_frames(&bee, 2);
    let listed = write_frames(dir.path(), 2);
    let (window, app) = launch(cx, &engine, listed, &dir.path().join("history"));
    stack(cx, window, &app);
    let first = active(cx, &app).expect("the stack is kept and shown as its render");
    let shown = app.read_with(cx, |app, cx| app.stacking.read(cx).result().cloned().unwrap());
    act(cx, window, &app, |app, window, cx| {
        app.start_batch(
            per_folder_plan(std::slice::from_ref(&bee)),
            Batch::new(BatchLabel::default()),
            None,
            window,
            cx,
        )
    });
    app.read_with(cx, |app, cx| {
        assert!(app.stacking.read(cx).is_running());
        assert!(!app.stacking.read(cx).runs_on_screen(), "the list's rows aren't the batch's");
        assert!(app.viewer.read(cx).has_content(), "the viewer keeps the stack on screen");
        assert_eq!(app.renders.read(cx).active(cx), Some(&first));
    });
    wait_until(cx, "the batch", |cx| batch_over(cx, &app));
    assert_eq!(active(cx, &app), Some(first), "its render is still the one shown");
    app.read_with(cx, |app, cx| {
        let now = app.stacking.read(cx).result().cloned().unwrap();
        assert!(Arc::ptr_eq(&now, &shown), "and so is its result");
        assert_eq!(app.renders.read(cx).renders().len(), 2, "the batch made its own render");
    });
}

/// ⌘. on the Library cancels a batch, between stacks too (here its turntable
/// sequence); Esc there clears the selection and never cancels.
#[gpui::test]
fn command_period_on_the_library_cancels_a_batch(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let folders = batch_folders(&dir.path().join("scan"), &["s001", "s002", "s003", "s004"], 3);
    let sequences = dir.path().join("sequences");
    std::fs::create_dir_all(&sequences).unwrap();
    let (window, app) = launch(cx, &engine, Vec::new(), &dir.path().join("history"));
    cx.update(|cx| cx.bind_keys(key_bindings()));
    let (plan, after) = (per_folder_plan(&folders), turntable_after(&sequences));
    act(cx, window, &app, |app, window, cx| {
        app.start_batch_with(plan, Batch::new(BatchLabel::default()), None, after, window, cx)
    });
    wait_until(cx, "the sequence", |cx| {
        app.read_with(cx, |app, cx| app.sequences.read(cx).is_running())
    });
    app.read_with(cx, |app, cx| {
        assert_eq!(app.screen, Screen::Library);
        assert!(!app.stacking.read(cx).is_running(), "between stacks");
        assert!(app.can_cancel(cx));
    });
    let selected = |cx: &mut TestAppContext| {
        app.read_with(cx, |app, cx| app.library.read(cx).selected_len(cx))
    };
    // Esc clears the selection and the batch goes on.
    app.update(cx, |app, cx| app.library.update(cx, |l, cx| l.select_all(cx)));
    assert!(selected(cx) > 0);
    cx.simulate_keystrokes(window, "escape");
    assert_eq!(selected(cx), 0);
    app.read_with(cx, |app, cx| {
        assert!(!app.sequences.read(cx).cancel_requested(), "Esc doesn't cancel");
        assert!(app.can_cancel(cx));
    });
    cx.simulate_keystrokes(window, "secondary-.");
    app.read_with(cx, |app, cx| {
        // A sequence still queued on the pool ends at once.
        let sequences = app.sequences.read(cx);
        assert!(sequences.cancel_requested() || !sequences.is_running(), "⌘. cancels the batch");
        assert!(!app.can_cancel(cx), "once");
    });
    // While it cancels, Esc is still the Library's.
    app.update(cx, |app, cx| app.library.update(cx, |l, cx| l.select_all(cx)));
    cx.simulate_keystrokes(window, "escape");
    assert_eq!(selected(cx), 0, "Esc while canceling");
    wait_until(cx, "the batch", |cx| batch_over(cx, &app));
    // Nothing runs: Esc is the Library's still.
    app.update(cx, |app, cx| app.library.update(cx, |l, cx| l.select_all(cx)));
    assert!(selected(cx) > 0);
    cx.simulate_keystrokes(window, "escape");
    assert_eq!(selected(cx), 0);
}

/// ⌘. in Stacking cancels a batch between stacks too; Esc doesn't.
#[gpui::test]
fn command_period_in_stacking_cancels_a_batch_between_stacks(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let folders = batch_folders(&dir.path().join("scan"), &["s001", "s002", "s003", "s004"], 3);
    let sequences = dir.path().join("sequences");
    std::fs::create_dir_all(&sequences).unwrap();
    let (window, app) = launch(cx, &engine, Vec::new(), &dir.path().join("history"));
    cx.update(|cx| cx.bind_keys(key_bindings()));
    let (plan, after) = (per_folder_plan(&folders), turntable_after(&sequences));
    act(cx, window, &app, |app, window, cx| {
        app.set_screen(Screen::Stacking, window, cx);
        app.start_batch_with(plan, Batch::new(BatchLabel::default()), None, after, window, cx)
    });
    wait_until(cx, "the sequence", |cx| {
        app.read_with(cx, |app, cx| app.sequences.read(cx).is_running())
    });
    app.read_with(cx, |app, cx| {
        assert_eq!(app.screen, Screen::Stacking);
        assert!(!app.stacking.read(cx).is_running(), "between stacks");
    });
    cx.simulate_keystrokes(window, "escape");
    app.read_with(cx, |app, cx| {
        assert!(!app.sequences.read(cx).cancel_requested(), "Esc doesn't cancel");
        assert!(app.can_cancel(cx));
    });
    cx.simulate_keystrokes(window, "secondary-.");
    app.read_with(cx, |app, cx| {
        // A sequence still queued on the pool ends at once.
        let sequences = app.sequences.read(cx);
        assert!(sequences.cancel_requested() || !sequences.is_running(), "⌘. cancels the batch");
        assert!(!app.can_cancel(cx), "once");
    });
    wait_until(cx, "the batch", |cx| batch_over(cx, &app));
}

/// Esc during a batch with a render compared leaves Compare; the batch goes on.
#[gpui::test]
fn escape_during_a_batch_leaves_compare_without_canceling(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let listed = write_frames(dir.path(), 2);
    let folders = batch_folders(&dir.path().join("scan"), &["s001", "s002", "s003"], 3);
    let (window, app) = launch(cx, &engine, listed, &dir.path().join("history"));
    cx.update(|cx| cx.bind_keys(key_bindings()));
    stack(cx, window, &app);
    let first = active(cx, &app).unwrap();
    stack(cx, window, &app);
    act(cx, window, &app, |app, window, cx| app.compare_render(first.clone(), window, cx));
    let comparing =
        |cx: &mut TestAppContext| app.read_with(cx, |app, cx| app.viewer.read(cx).has_compare());
    wait_until(cx, "the compare", |cx| comparing(cx));
    act(cx, window, &app, |app, window, cx| {
        app.start_batch(
            per_folder_plan(&folders),
            Batch::new(BatchLabel::default()),
            None,
            window,
            cx,
        )
    });
    app.read_with(cx, |app, cx| {
        assert_eq!(app.screen, Screen::Stacking);
        assert!(app.can_cancel(cx), "the batch runs");
    });
    assert!(comparing(cx), "the compare stays on screen while the batch runs");
    cx.simulate_keystrokes(window, "escape");
    cx.run_until_parked();
    assert!(!comparing(cx), "Esc leaves Compare");
    app.read_with(cx, |app, cx| {
        let Running::Batch(run) = &app.running else { panic!("the batch goes on") };
        assert!(!run.machine.is_ending(), "Esc doesn't cancel");
        assert!(app.can_cancel(cx));
    });
    wait_until(cx, "the batch", |cx| batch_over(cx, &app));
    app.read_with(cx, |app, cx| assert_eq!(app.renders.read(cx).renders().len(), 5));
}

/// Exports being written keep a batch open after cancel: quit waits, and the
/// closing notice counts them.
#[gpui::test]
fn a_canceled_batch_waits_for_its_exports(cx: &mut TestAppContext) {
    use crate::batch_dialog::{After, ExportEach};
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let folders = batch_folders(dir.path(), &["A"], 2);
    let exports = dir.path().join("exports");
    std::fs::create_dir_all(&exports).unwrap();
    let (window, app) = launch(cx, &engine, Vec::new(), &dir.path().join("history"));
    let after = After {
        export: Some(ExportEach {
            folder: exports.clone(),
            format: ExportFormat::Png,
            options: deptheos_app::SaveOptions::default(),
        }),
        turntable: None,
    };
    let hold = app.read_with(cx, |app, _| app.export_probe.hold.clone());
    hold.store(true, std::sync::atomic::Ordering::Relaxed);
    let plan = per_folder_plan(&folders);
    act(cx, window, &app, |app, window, cx| {
        app.start_batch_with(plan, Batch::new(BatchLabel::default()), None, after, window, cx)
    });
    wait_until(cx, "the stack and its render", |cx| {
        app.read_with(cx, |app, cx| {
            !app.stacking.read(cx).is_running() && app.renders.read(cx).renders().len() == 1
        })
    });
    act(cx, window, &app, |app, window, cx| app.cancel_run(window, cx));
    app.read_with(cx, |app, cx| {
        assert!(app.batch_run().is_some(), "the run waits for its export");
        assert!(app.files_being_written(cx), "and quitting waits for it");
        assert!(app.files.read(cx).locked(), "locked until the run closes");
    });
    assert_eq!(cx.active_idle_sleep_preventions(), 1, "the Mac kept awake meanwhile");
    hold.store(false, std::sync::atomic::Ordering::Relaxed);
    wait_until(cx, "the batch", |cx| batch_over(cx, &app));
    assert_eq!(names_in(&exports), ["A.png"]);
    assert_eq!(cx.active_idle_sleep_preventions(), 0, "until the run is over");
}

/// The first failing export ends the batch: the running stack stops (or is
/// kept if finished), nothing more starts.
#[cfg(unix)]
#[gpui::test]
fn a_failed_export_ends_the_batch(cx: &mut TestAppContext) {
    use std::os::unix::fs::PermissionsExt as _;

    use crate::batch_dialog::{After, ExportEach};
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let folders = batch_folders(&dir.path().join("scan"), &["A", "B", "C", "D"], 2);
    let exports = dir.path().join("exports");
    std::fs::create_dir_all(&exports).unwrap();
    std::fs::set_permissions(&exports, std::fs::Permissions::from_mode(0o555)).unwrap();
    let (window, app) = launch(cx, &engine, Vec::new(), &dir.path().join("history"));
    let after = After {
        export: Some(ExportEach {
            folder: exports.clone(),
            format: ExportFormat::Png,
            options: deptheos_app::SaveOptions::default(),
        }),
        turntable: None,
    };
    let plan = per_folder_plan(&folders);
    act(cx, window, &app, |app, window, cx| {
        app.start_batch_with(plan, Batch::new(BatchLabel::default()), None, after, window, cx)
    });
    let ended = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        wait_until(cx, "the batch", |cx| batch_over(cx, &app))
    }));
    std::fs::set_permissions(&exports, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(ended.is_ok(), "the batch ends");
    let made = app.read_with(cx, |app, cx| app.renders.read(cx).renders().len());
    assert!((1..=2).contains(&made), "A, and B if it finished first: {made}");
    assert!(names_in(&exports).is_empty());
}

/// `zooms.len()` frames of the aligner's natural-image fixture, each
/// magnified by its zoom: a bracket the aligner can measure.
fn write_zoomed_frames(dir: &Path, zooms: &[f32]) {
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/../deptheos_aligner/fixtures/spider.jpg");
    let base = image::open(fixture).unwrap().resize_exact(
        1200,
        750,
        image::imageops::FilterType::Triangle,
    );
    std::fs::create_dir_all(dir).unwrap();
    for (i, zoom) in zooms.iter().enumerate() {
        let (w, h) = (800.0 / zoom, 500.0 / zoom);
        let window =
            base.crop_imm((600.0 - w / 2.0) as u32, (375.0 - h / 2.0) as u32, w as u32, h as u32);
        let frame = window.resize_exact(640, 400, image::imageops::FilterType::Lanczos3);
        frame.to_rgb8().save(dir.join(format!("frame_{i}.png"))).unwrap();
    }
}

/// One export at a time: nothing stacks while the first is written. A
/// turntable batch then restacks the stack with a stray frame, its export
/// overwriting the first's file.
#[gpui::test]
fn a_batch_stacks_on_once_its_export_lands(cx: &mut TestAppContext) {
    use crate::batch_dialog::ExportEach;
    use deptheos_app::runs::batch::Phase;
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let shoot = dir.path().join("scan");
    let folders: Vec<PathBuf> = ["s001", "s002", "s003"].iter().map(|n| shoot.join(n)).collect();
    write_zoomed_frames(&folders[0], &[1.04, 1.02, 1.0]);
    write_zoomed_frames(&folders[1], &[1.04, 1.02, 1.0]);
    // A frame left from the far end of the bracket before, then the bracket.
    write_zoomed_frames(&folders[2], &[1.0, 1.04, 1.02, 1.0]);
    let (exports, sequences) = (dir.path().join("exports"), dir.path().join("sequences"));
    std::fs::create_dir_all(&exports).unwrap();
    std::fs::create_dir_all(&sequences).unwrap();
    let (window, app) = launch(cx, &engine, Vec::new(), &dir.path().join("history"));
    let mut after = turntable_after(&sequences);
    after.export = Some(ExportEach {
        folder: exports.clone(),
        format: ExportFormat::Png,
        options: deptheos_app::SaveOptions::default(),
    });
    let hold = app.read_with(cx, |app, _| app.export_probe.hold.clone());
    hold.store(true, std::sync::atomic::Ordering::Relaxed);
    let plan = per_folder_plan(&folders);
    act(cx, window, &app, |app, window, cx| {
        app.start_batch_with(plan, Batch::new(BatchLabel::default()), None, after, window, cx)
    });
    wait_until(cx, "the first export", |cx| {
        app.read_with(cx, |app, _| {
            app.batch_run()
                .is_some_and(|run| matches!(run.machine.phase(), Phase::AwaitingExport { slot: 0 }))
        })
    });
    app.read_with(cx, |app, cx| {
        assert!(app.renders.read(cx).renders().len() <= 1, "only s001 is made");
        assert!(!app.stacking.read(cx).is_running(), "nothing stacks meanwhile");
    });
    hold.store(false, std::sync::atomic::Ordering::Relaxed);
    wait_until(cx, "the batch", |cx| batch_over(cx, &app));
    assert_eq!(names_in(&exports), ["s001.png", "s002.png", "s003.png"]);
    let written = app.read_with(cx, |app, _| app.export_probe.written.clone());
    assert_eq!(
        written.load(std::sync::atomic::Ordering::Relaxed),
        4,
        "each stack's export, and the restack's over s003's"
    );
    // The render the restack replaced goes as the run closes.
    wait_until(cx, "the replaced render's delete", |cx| {
        app.read_with(cx, |app, cx| app.renders.read(cx).renders().len() == 3)
    });
}

/// Names differing only in case are two files on a case-insensitive file
/// system: each export takes a free name.
#[gpui::test]
fn names_that_differ_in_case_are_two_exports(cx: &mut TestAppContext) {
    use crate::batch_dialog::{After, ExportEach};
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let folders = batch_folders(&dir.path().join("scan"), &["one", "two"], 2);
    let exports = dir.path().join("exports");
    std::fs::create_dir_all(&exports).unwrap();
    let (window, app) = launch(cx, &engine, Vec::new(), &dir.path().join("history"));
    let mut plan = per_folder_plan(&folders);
    plan.jobs[0].label = "Bee".into();
    plan.jobs[1].label = "bee".into();
    let after = After {
        export: Some(ExportEach {
            folder: exports.clone(),
            format: ExportFormat::Png,
            options: deptheos_app::SaveOptions::default(),
        }),
        turntable: None,
    };
    act(cx, window, &app, |app, window, cx| {
        app.start_batch_with(plan, Batch::new(BatchLabel::default()), None, after, window, cx)
    });
    wait_until(cx, "the batch", |cx| batch_over(cx, &app));
    let names = names_in(&exports);
    let ignores_case = exports.join("BEE.PNG").exists();
    match ignores_case {
        true => assert_eq!(names, ["Bee.png", "bee-2.png"]),
        false => assert_eq!(names, ["Bee.png", "bee.png"]),
    }
}

/// A turntable batch whose renders can't be kept still ends.
#[cfg(unix)]
#[gpui::test]
fn a_turntable_batch_ends_when_its_renders_cannot_be_saved(cx: &mut TestAppContext) {
    use std::os::unix::fs::PermissionsExt as _;
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let folders = batch_folders(&dir.path().join("scan"), &["s001", "s002"], 2);
    let sequences = dir.path().join("sequences");
    std::fs::create_dir_all(&sequences).unwrap();
    let history = dir.path().join("history");
    let (window, app) = launch(cx, &engine, Vec::new(), &history);
    // Saving a render makes a folder in here.
    let renders = history.join("Renders");
    std::fs::create_dir_all(&renders).unwrap();
    std::fs::set_permissions(&renders, std::fs::Permissions::from_mode(0o555)).unwrap();
    let (plan, after) = (per_folder_plan(&folders), turntable_after(&sequences));
    act(cx, window, &app, |app, window, cx| {
        app.start_batch_with(plan, Batch::new(BatchLabel::default()), None, after, window, cx)
    });
    let ended = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        wait_until(cx, "the batch", |cx| batch_over(cx, &app))
    }));
    std::fs::set_permissions(&renders, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(ended.is_ok(), "the batch ends");
    app.read_with(cx, |app, cx| assert!(app.renders.read(cx).renders().is_empty()));
}

/// Start, when the batch can't start, says why and keeps the dialog as chosen.
#[gpui::test]
fn the_batch_dialog_stays_open_when_its_batch_cant_start(cx: &mut TestAppContext) {
    use crate::batch_dialog::{After, BatchEvent};
    let Some(Fixture { dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    wait_until(cx, "the images", |cx| app.read_with(cx, |app, cx| app.files.read(cx).can_stack()));
    act(cx, window, &app, |app, window, cx| app.open_batch(window, cx));
    let dialog = app.read_with(cx, |app, _| batch_sheet(app).unwrap());
    let folders = batch_folders(&dir.path().join("batch"), &["A"], 2);
    let start = |cx: &mut TestAppContext| {
        let plan = per_folder_plan(&folders);
        let event = BatchEvent::Start {
            plan,
            batch: Batch::new(BatchLabel::default()),
            raw_backend: None,
            after: After::default(),
        };
        dialog.update(cx, |_, cx| cx.emit(event));
        cx.run_until_parked();
    };
    // A stack started meanwhile (from the menu, say).
    act(cx, window, &app, |app, window, cx| app.run_stacking(window, cx));
    start(cx);
    app.read_with(cx, |app, _| {
        assert!(batch_sheet(app).is_some(), "still open");
        assert!(!app.batch_run().is_some());
    });
    wait_until(cx, "the stack", |cx| {
        app.read_with(cx, |app, cx| !app.is_processing(cx) && !app.renders.read(cx).is_saving())
    });
    start(cx);
    app.read_with(cx, |app, _| {
        assert!(batch_sheet(app).is_none(), "closed once the batch starts");
        assert!(app.batch_run().is_some());
    });
    wait_until(cx, "the batch", |cx| batch_over(cx, &app));
}

/// Run a turntable batch of `folders` with `config`, asking `mode`, into
/// `out`; returns the renders.
#[allow(clippy::too_many_arguments)]
fn turntable_batch(
    cx: &mut TestAppContext,
    engine: &Arc<Engine>,
    folders: &[PathBuf],
    config: &StackerConfig,
    mode: deptheos_app::runs::sequence::TurntableMode,
    order: deptheos_app::FrameOrder,
    out: &Path,
    history: &Path,
) -> usize {
    use deptheos_app::runs::batch::{Input, Split, plan};
    use deptheos_app::runs::sequence::{FrameFormat, FrameOptions, TurntableOptions};

    use crate::batch_dialog::{After, TurntableAfter};
    let (window, app) = launch(cx, engine, Vec::new(), history);
    act(cx, window, &app, |app, window, cx| {
        app.params.update(cx, |p, cx| p.restore(config, &SessionOptions::default(), window, cx))
    });
    let inputs: Vec<Input> = folders.iter().cloned().map(Input::Folder).collect();
    let plan = plan(
        &inputs,
        Split::PerFolder,
        folder::Source::Raw,
        order,
        deptheos_app::RawBackend::LibRaw,
        deptheos_app::runs::batch::capture_info,
        &deptheos_app::Words::english(deptheos_app::Platform::Desktop),
    );
    let frames = FrameOptions {
        format: FrameFormat::Jpeg,
        long_edge: Some(1920),
        ..FrameOptions::default()
    };
    let after = After {
        export: None,
        turntable: Some(TurntableAfter {
            folder: out.to_path_buf(),
            options: TurntableOptions { mode, frames, ..TurntableOptions::default() },
        }),
    };
    act(cx, window, &app, |app, window, cx| {
        app.start_batch_with(plan, Batch::new(BatchLabel::default()), None, after, window, cx)
    });
    wait_until(cx, "the batch and its sequence", |cx| batch_over(cx, &app));
    app.read_with(cx, |app, cx| {
        let renders = app.renders.read(cx).renders();
        let same = |r: &StackerConfig| std::mem::discriminant(r) == std::mem::discriminant(config);
        assert!(renders.iter().all(|r| same(&r.config)), "the sidebar's algorithm, not Pyramid");
        renders.len()
    })
}

/// With Wavelet, a turntable asked for Both makes Full only (grow needs
/// Pyramid).
#[gpui::test]
fn a_wavelet_turntable_makes_the_full_sequence(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let shoot = dir.path().join("scan");
    let folders: Vec<PathBuf> = ["s001", "s002", "s003"].iter().map(|n| shoot.join(n)).collect();
    for folder in &folders {
        std::fs::create_dir_all(folder).unwrap();
        write_frames(folder, 3);
    }
    let out = dir.path().join("out");
    std::fs::create_dir_all(&out).unwrap();
    let wavelet = StackerConfig::Wavelet(deptheos_app::WaveletConfig::default());
    let both = deptheos_app::runs::sequence::TurntableMode::Both;
    let made = turntable_batch(
        cx,
        &engine,
        &folders,
        &wavelet,
        both,
        deptheos_app::FrameOrder::Name,
        &out,
        &dir.path().join("history"),
    );
    assert_eq!(made, 3);
    assert_eq!(names_in(&out), ["scan full"], "no grow sequence");
    assert_eq!(
        names_in(&out.join("scan full")),
        ["manifest.json", "scan full_0001.jpg", "scan full_0002.jpg", "scan full_0003.jpg"]
    );
}

/// The same on real stacks: DEPTHEOS_TURNTABLE_STACKS names folders to stack
/// (e.g. three of a turntable shoot), DEPTHEOS_TURNTABLE_OUT where the
/// sequence goes. Run with `--ignored`.
#[gpui::test]
#[ignore = "real shoots: set DEPTHEOS_TURNTABLE_STACKS and DEPTHEOS_TURNTABLE_OUT"]
fn a_wavelet_turntable_of_real_stacks(cx: &mut TestAppContext) {
    let (Ok(stacks), Ok(out)) =
        (std::env::var("DEPTHEOS_TURNTABLE_STACKS"), std::env::var("DEPTHEOS_TURNTABLE_OUT"))
    else {
        panic!("set DEPTHEOS_TURNTABLE_STACKS and DEPTHEOS_TURNTABLE_OUT");
    };
    let Some(engine) = engine() else { return };
    cx.executor().allow_parking();
    let folders: Vec<PathBuf> = std::env::split_paths(&stacks).collect();
    let out = PathBuf::from(out);
    std::fs::create_dir_all(&out).unwrap();
    let history = tempfile::tempdir().unwrap();
    let wavelet = StackerConfig::Wavelet(deptheos_app::WaveletConfig::default());
    let started = std::time::Instant::now();
    let made = turntable_batch(
        cx,
        &engine,
        &folders,
        &wavelet,
        deptheos_app::runs::sequence::TurntableMode::Full,
        // Camera names can sort out of shooting order (hex counters).
        deptheos_app::FrameOrder::CaptureTime,
        &out,
        history.path(),
    );
    eprintln!("{made} renders and the full sequence in {:.1?}", started.elapsed());
    assert_eq!(made, folders.len());
    let label = folders[0].parent().unwrap().file_name().unwrap().to_string_lossy().into_owned();
    let seq = out.join(format!("{label} full"));
    let names = names_in(&seq);
    assert_eq!(names.len(), folders.len() + 1, "{names:?}");
    eprintln!("{}", std::fs::read_to_string(seq.join("manifest.json")).unwrap());
}
