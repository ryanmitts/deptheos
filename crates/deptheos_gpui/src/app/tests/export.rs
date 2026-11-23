//! Export: the sheet, from the Library, and grow sequences.

use super::*;
use deptheos_app::library::history::BatchLabel;

/// The Library's Export… reads the render, asks with the Export sheet and
/// writes it, reporting in the Library's bar, opening nothing in Stacking.
#[gpui::test]
fn export_from_the_library_stays_in_the_library(cx: &mut TestAppContext) {
    use crate::export_dialog::{ExportDialogEvent, ExportPlace};
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    stack(cx, window, &app);
    let first = active(cx, &app).unwrap();
    stack(cx, window, &app);
    let second = active(cx, &app).unwrap();
    act(cx, window, &app, |app, window, cx| app.set_screen(Screen::Library, window, cx));

    act(cx, window, &app, |app, window, cx| app.export_from_library(first.clone(), window, cx));
    let dialog = |cx: &mut TestAppContext| app.read_with(cx, |app, _| export_sheet(app));
    wait_until(cx, "the export sheet", |cx| dialog(cx).is_some());
    let dialog = dialog(cx).unwrap();
    app.read_with(cx, |app, cx| {
        assert_eq!(app.screen, Screen::Library, "still in the Library");
        assert!(!app.renders.read(cx).is_pending(), "nothing opens");
    });
    assert_eq!(active(cx, &app), Some(second), "the render shown stays");
    assert!(dialog.read_with(cx, |d, _| d.subtitle().to_string()).contains("Pyramid"));

    let out = tempfile::tempdir().unwrap();
    let mut choice = dialog.read_with(cx, |d, cx| d.choice(cx)).expect("it can export");
    assert!(choice.name.contains("-1."), "#1's name: {}", choice.name);
    choice.format = ExportFormat::Png;
    choice.name = "out.png".into();
    choice.dir = out.path().to_path_buf();
    choice.place = ExportPlace::Last;
    choice.reveal = false;
    dialog.update(cx, |_, cx| cx.emit(ExportDialogEvent::Export(choice)));
    // Once the pool has said the file isn't there.
    wait_until(cx, "the export to start", |cx| {
        app.read_with(cx, |app, cx| app.export_progress_text(cx).is_some())
    });
    app.read_with(cx, |app, cx| {
        assert_eq!(app.export_progress_text(cx).as_deref(), Some("Exporting out.png…"));
        assert!(app.files_being_written(cx), "quitting waits for it");
        assert_eq!(
            app.deletable(cx),
            crate::app::library_delete::Deletable::Writing,
            "and so does Delete Library"
        );
    });
    let written = out.path().join("out.png");
    wait_until(cx, "the export", |cx| {
        written.exists() && app.read_with(cx, |app, cx| app.export_progress_text(cx).is_none())
    });
    app.read_with(cx, |app, _| assert_eq!(app.screen, Screen::Library));
    let width = app.read_with(cx, |app, cx| app.renders.read(cx).find(&first).unwrap().width);
    assert_eq!(image::open(&written).unwrap().width(), width, "#1, whole");
    // Not shown when done, so the notice offers to.
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("notice-action-0").is_some(), "Show in Finder");
}

/// The Library's Export… says "retouched" for painted renders and shows the
/// thumbnail once read, even if not read as the sheet opened.
#[gpui::test]
fn export_from_the_library_says_what_it_read(cx: &mut TestAppContext) {
    let Some(Fixture { dir, engine, window, app }) = Fixture::launch(cx, 3) else { return };
    let root = dir.path().join("history");
    cx.update(|cx| cx.bind_keys(key_bindings()));
    stack(cx, window, &app);
    let id = active(cx, &app).unwrap();
    cx.simulate_keystrokes(window, "p");
    wait_until(cx, "the source", |cx| paint_state(cx, &app).source_ready);
    stroke(cx, &app, 40.0);
    wait_until(cx, "the stroke", |cx| paint_state(cx, &app).undo == 1);
    act(cx, window, &app, |app, window, cx| app.set_screen(Screen::Library, window, cx));
    let render_dir = crate::test_support::library(cx, &app).render_path(&id);
    wait_until(cx, "the paint saved", |_| {
        deptheos_app::paint::store::has_paint(&render_dir) == Ok(true)
    });
    crate::test_support::quit(cx, window, app);

    // Relaunched with its thumbnail unreadable, so the sheet opens without it.
    let thumb = render_dir.join("thumb.jpg");
    let aside = dir.path().join("thumb.jpg");
    std::fs::rename(&thumb, &aside).unwrap();
    let (window, app) = launch(cx, &engine, Vec::new(), &root);
    let renders = app.read_with(cx, |app, _| app.renders.clone());
    wait_until(cx, "the thumbnail's read", |cx| {
        renders.read_with(cx, |r, _| r.thumbnail_failed(&id))
    });
    act(cx, window, &app, |app, window, cx| app.export_from_library(id.clone(), window, cx));
    let dialog = |cx: &mut TestAppContext| app.read_with(cx, |app, _| export_sheet(app));
    wait_until(cx, "the export sheet", |cx| dialog(cx).is_some());
    let dialog = dialog(cx).unwrap();
    let subtitle = dialog.read_with(cx, |d, _| d.subtitle().to_string());
    assert!(subtitle.ends_with(" · retouched"), "{subtitle}");
    assert!(dialog.read_with(cx, |d, cx| d.thumbnail(cx)).is_none());

    std::fs::rename(&aside, &thumb).unwrap();
    renders.update(cx, |r, cx| {
        r.age_thumbnail_failures();
        r.request_thumbnails(vec![id.clone()], cx)
    });
    wait_until(cx, "the thumbnail", |cx| dialog.read_with(cx, |d, cx| d.thumbnail(cx)).is_some());
}

/// The Library's Export… during a batch does nothing, then or later.
#[gpui::test]
fn export_from_the_library_during_a_batch_does_nothing(cx: &mut TestAppContext) {
    let Some(Fixture { dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    stack(cx, window, &app);
    let first = active(cx, &app).unwrap();
    let sheet = |cx: &mut TestAppContext| app.read_with(cx, |app, _| export_sheet(app).is_some());

    let folders = batch_folders(&dir.path().join("batch"), &["A"], 2);
    let plan = per_folder_plan(&folders);
    act(cx, window, &app, |app, window, cx| {
        app.start_batch(plan, Batch::new(BatchLabel::default()), None, window, cx)
    });
    act(cx, window, &app, |app, window, cx| app.export_from_library(first.clone(), window, cx));
    app.read_with(cx, |app, cx| assert_eq!(app.export_progress_text(cx), None));
    wait_until(cx, "the batch", |cx| batch_over(cx, &app));
    assert!(!sheet(cx), "the click during the batch is forgotten");
}

/// One export at a time per window: while a Library render writes, the
/// result's Export… is disabled and inert, and vice versa.
#[gpui::test]
fn one_export_runs_at_a_time(cx: &mut TestAppContext) {
    use crate::export_dialog::{ExportDialogEvent, ExportPlace};
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    stack(cx, window, &app);
    let first = active(cx, &app).unwrap();
    let sheet = |cx: &mut TestAppContext| app.read_with(cx, |app, _| export_sheet(app));
    let out = tempfile::tempdir().unwrap();
    let export_as = |cx: &mut TestAppContext, name: &str| {
        let dialog = sheet(cx).expect("the export sheet");
        let mut choice = dialog.read_with(cx, |d, cx| d.choice(cx)).expect("it can export");
        choice.format = ExportFormat::Png;
        choice.name = name.into();
        choice.dir = out.path().to_path_buf();
        choice.place = ExportPlace::Last;
        choice.reveal = false;
        dialog.update(cx, |_, cx| cx.emit(ExportDialogEvent::Export(choice)));
        wait_until(cx, "the export to start", |cx| {
            app.read_with(cx, |app, cx| {
                app.export_progress_text(cx).is_some_and(|t| t.contains(name))
            })
        });
    };
    let over = |cx: &mut TestAppContext, name: &str| {
        let written = out.path().join(name);
        wait_until(cx, "the export", |cx| {
            written.exists() && !app.read_with(cx, |app, _| app.is_exporting())
        });
    };

    // A Library render being written: the result's Export… waits.
    act(cx, window, &app, |app, window, cx| app.set_screen(Screen::Library, window, cx));
    act(cx, window, &app, |app, window, cx| app.export_from_library(first.clone(), window, cx));
    wait_until(cx, "the export sheet", |cx| sheet(cx).is_some());
    export_as(cx, "library.png");
    act(cx, window, &app, |app, window, cx| app.set_screen(Screen::Stacking, window, cx));
    app.read_with(cx, |app, cx| assert!(!app.can_export(cx), "Export… is disabled"));
    act(cx, window, &app, |app, window, cx| app.save_result(window, cx));
    assert!(sheet(cx).is_none(), "and opens nothing");
    over(cx, "library.png");
    assert!(sheet(cx).is_none(), "then or later");

    // The result being written: the Library's Export… waits.
    act(cx, window, &app, |app, window, cx| app.save_result(window, cx));
    export_as(cx, "result.png");
    act(cx, window, &app, |app, window, cx| app.set_screen(Screen::Library, window, cx));
    act(cx, window, &app, |app, window, cx| app.export_from_library(first.clone(), window, cx));
    app.read_with(cx, |app, cx| {
        assert_eq!(app.export_progress_text(cx).as_deref(), Some("Exporting result.png…"));
    });
    over(cx, "result.png");
    cx.run_until_parked();
    assert!(sheet(cx).is_none(), "the click during the export is forgotten");
}

/// Return presses a sheet's primary button; with nothing to start the batch
/// dialog stays open, where the kit's own Return would close it.
#[gpui::test]
fn return_in_a_sheet_presses_its_primary_button(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    wait_until(cx, "the images", |cx| app.read_with(cx, |app, cx| app.files.read(cx).can_stack()));
    act(cx, window, &app, |app, window, cx| app.open_batch(window, cx));
    cx.run_until_parked();
    cx.simulate_keystrokes(window, "enter");
    cx.run_until_parked();
    app.read_with(cx, |app, _| assert!(batch_sheet(app).is_some(), "nothing to start: still open"));
}

/// Export Grow Sequence…: a frame per stacked frame, the last the render,
/// and a notice naming the folder.
#[gpui::test]
fn exports_a_grow_sequence_of_the_render_on_screen(cx: &mut TestAppContext) {
    use deptheos_app::runs::sequence::{FrameFormat, FrameOptions, GrowOptions};

    use crate::grow_dialog::GrowInfo;
    let Some(Fixture { dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    stack(cx, window, &app);
    let id = active(cx, &app).expect("the stack is saved");
    let source = app.read_with(cx, |app, cx| app.grow_source(None, cx));
    assert_eq!(source.as_ref(), Some(&id), "a pyramid render on screen");
    let (width, height) = app.read_with(cx, |app, cx| {
        let render = app.renders.read(cx).find(&id).unwrap();
        (render.width, render.height)
    });
    let info = GrowInfo {
        name: "bee".into(),
        frames: 3,
        width,
        height,
        sixteen_bit_source: false,
        has_paint: false,
    };
    let options = GrowOptions {
        frames: FrameOptions { format: FrameFormat::Png, ..FrameOptions::default() },
        ..GrowOptions::default()
    };
    let out = dir.path().join("out");
    std::fs::create_dir_all(&out).unwrap();
    act(cx, window, &app, |app, _, cx| app.start_grow(id, info, options, out.clone(), cx));
    app.read_with(cx, |app, cx| {
        assert!(app.is_processing(cx), "the app is locked");
        assert!(app.files_being_written(cx), "quitting, canceled too, waits for its frames");
        assert!(app.can_cancel(cx), "⌘. and the pill's Cancel stop a sequence");
        let menus = app.menu_state(cx);
        assert!(menus.running && !menus.can_retouch, "what waits for the run dims");
        assert_eq!(
            app.busy_reason(cx).as_deref(),
            Some("Not while a sequence is written: wait for it to finish, or cancel it")
        );
    });
    // Nothing ran on this thread since: it still runs.
    assert_eq!(cx.active_idle_sleep_preventions(), 1, "the Mac kept awake");
    wait_until(cx, "the sequence", |cx| app.read_with(cx, |app, cx| !app.is_processing(cx)));
    assert_eq!(cx.active_idle_sleep_preventions(), 0, "until it is written");
    assert_eq!(
        names_in(&out.join("bee grow")),
        ["bee grow_0001.png", "bee grow_0002.png", "bee grow_0003.png", "manifest.json"]
    );
}

/// ⌘E opens the sheet (the method named once); Export writes with its
/// choices and remembers them, saying "Exporting out.png…" and offering
/// Show in Finder when not showing it; ⇧⌘E repeats without a sheet,
/// numbering a taken name.
#[gpui::test]
fn exports_from_the_sheet_and_again(cx: &mut TestAppContext) {
    use crate::export_dialog::{ExportDialogEvent, ExportPlace};
    let Some(Fixture { dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    cx.update(|cx| cx.bind_keys(key_bindings()));
    stack(cx, window, &app);

    cx.simulate_keystrokes(window, "secondary-e");
    let dialog = app.read_with(cx, |app, _| export_sheet(app));
    let dialog = dialog.expect("the sheet is open");
    let subtitle = dialog.read_with(cx, |d, _| d.subtitle().to_string());
    assert_eq!(subtitle.matches("Pyramid").count(), 1, "{subtitle}");
    let out = tempfile::tempdir().unwrap();
    // The name's extension follows the format.
    cx.update_window(window, |_, window, cx| {
        dialog.update(cx, |d, cx| d.pick_format(ExportFormat::Jpeg, window, cx))
    })
    .unwrap();
    let mut choice = dialog.read_with(cx, |d, cx| d.choice(cx)).expect("it can export");
    assert!(choice.name.ends_with("-1.jpg"), "{}", choice.name);
    assert_eq!(choice.place, ExportPlace::Frames);
    assert_eq!(choice.dir, dir.path(), "next to the frames");
    choice.format = ExportFormat::Png;
    choice.name = "out.png".into();
    choice.dir = out.path().to_path_buf();
    choice.place = ExportPlace::Last;
    choice.reveal = false;
    dialog.update(cx, |_, cx| cx.emit(ExportDialogEvent::Export(choice)));
    let first = out.path().join("out.png");
    // Once the pool has said the file isn't there.
    wait_until(cx, "the export to start", |cx| {
        app.read_with(cx, |app, cx| app.export_progress_text(cx).is_some())
    });
    app.read_with(cx, |app, cx| {
        assert_eq!(app.export_progress_text(cx).as_deref(), Some("Exporting out.png…"));
    });
    wait_until(cx, "the export", |_| first.exists());
    app.read_with(cx, |app, cx| {
        assert!(export_sheet(app).is_none(), "the sheet closed");
        let p = crate::prefs::prefs(cx);
        assert_eq!(p.save_format, ExportFormat::Png);
        assert_eq!(p.export_place, ExportPlace::Last);
        assert_eq!(p.save_dir.as_deref(), Some(out.path()));
        assert!(!p.reveal_export);
    });
    wait_until(cx, "the save to end", |cx| !app.read_with(cx, |a, _| a.is_exporting()));
    app.read_with(cx, |app, cx| assert_eq!(app.export_progress_text(cx), None));
    // Not shown when done, so the notice offers to.
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("notice-action-0").is_some(), "Show in Finder");

    // Again, twice: the stack's name and number, then numbered.
    cx.simulate_keystrokes(window, "secondary-shift-e");
    wait_until(cx, "the export again", |_| png_count(out.path()) == 2);
    wait_until(cx, "the save to end", |cx| !app.read_with(cx, |a, _| a.is_exporting()));
    cx.simulate_keystrokes(window, "secondary-shift-e");
    wait_until(cx, "the third export", |_| png_count(out.path()) == 3);
    app.read_with(cx, |app, _| assert!(export_sheet(app).is_none(), "no sheet"));
    let names = names_in(out.path());
    assert!(names.iter().any(|n| n.ends_with("-1-2.png")), "{names:?}");
}

/// Export never replaces an existing file: it gets a number, as Export
/// Again's does, and the sheet closes. Export is busy meanwhile.
#[gpui::test]
fn the_export_sheet_numbers_a_name_taken(cx: &mut TestAppContext) {
    use crate::export_dialog::{ExportDialogEvent, ExportPlace};
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    cx.update(|cx| cx.bind_keys(key_bindings()));
    stack(cx, window, &app);

    cx.simulate_keystrokes(window, "secondary-e");
    let dialog = app.read_with(cx, |app, _| export_sheet(app));
    let dialog = dialog.expect("the sheet is open");
    let out = tempfile::tempdir().unwrap();
    let taken = out.path().join("out.png");
    std::fs::write(&taken, b"old").unwrap();
    let mut choice = dialog.read_with(cx, |d, cx| d.choice(cx)).expect("it can export");
    choice.format = ExportFormat::Png;
    choice.name = "out.png".into();
    choice.dir = out.path().to_path_buf();
    choice.place = ExportPlace::Last;
    choice.reveal = false;

    dialog.update(cx, |_, cx| cx.emit(ExportDialogEvent::Export(choice)));
    assert!(dialog.read_with(cx, |d, _| d.is_finding()), "Export busy while the name is found");
    let numbered = out.path().join("out-2.png");
    wait_until(cx, "the export", |_| numbered.exists());
    wait_until(cx, "the save to end", |cx| !app.read_with(cx, |a, _| a.is_exporting()));
    assert_eq!(std::fs::read(&taken).unwrap(), b"old", "the file there is kept");
    assert!(image::open(&numbered).is_ok(), "the export is beside it");
    app.read_with(cx, |app, _| assert!(export_sheet(app).is_none(), "the sheet closed"));
    assert!(!asking(cx, window), "nothing asked");
}

/// A folder too long for the Export sheet is cut, not run past its edge.
#[gpui::test]
fn the_export_sheet_cuts_a_long_folder(cx: &mut TestAppContext) {
    let Some(engine) = engine() else { return };
    let dir = tempfile::tempdir().unwrap();
    let long = (0..6).fold(dir.path().to_path_buf(), |p, i| {
        p.join(format!("a folder with a rather long name {i}"))
    });
    std::fs::create_dir_all(&long).unwrap();
    let paths = write_frames(&long, 3);
    let (window, app) = launch(cx, &engine, paths, &dir.path().join("history"));
    cx.update(|cx| cx.bind_keys(key_bindings()));
    stack(cx, window, &app);
    cx.simulate_keystrokes(window, "secondary-e");
    let mut vcx = gpui::VisualTestContext::from_window(window, cx);
    vcx.run_until_parked();
    let folder = vcx.debug_bounds("export-frames-folder").expect("the frames' folder is shown");
    let go = vcx.debug_bounds("export-go-button").expect("the Export button");
    assert!(folder.right() <= go.right(), "{folder:?} runs past {go:?}");
    assert!(folder.size.width > px(200.0), "and is still shown: {folder:?}");
}

fn png_count(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .unwrap()
        .filter(|e| e.as_ref().unwrap().path().extension().is_some_and(|x| x == "png"))
        .count()
}

/// 8-bit frames export 8 bits, leaving 16 chosen for the next 16-bit stack.
#[gpui::test]
fn an_eight_bit_stack_keeps_sixteen_for_the_next(cx: &mut TestAppContext) {
    use crate::export_dialog::{ExportDialogEvent, ExportPlace};
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    cx.update(|cx| cx.bind_keys(key_bindings()));
    cx.update(|cx| crate::prefs::update_prefs(cx, |p| p.export.sixteen_bit = true));
    stack(cx, window, &app);

    cx.simulate_keystrokes(window, "secondary-e");
    let dialog = app.read_with(cx, |app, _| export_sheet(app)).expect("the sheet is open");
    cx.update_window(window, |_, window, cx| {
        dialog.update(cx, |d, cx| d.pick_format(ExportFormat::Tiff, window, cx))
    })
    .unwrap();
    let out = tempfile::tempdir().unwrap();
    let mut choice = dialog.read_with(cx, |d, cx| d.choice(cx)).expect("it can export");
    assert!(!choice.sixteen_bit, "8-bit frames have no 16 to write");
    choice.name = "out.tif".into();
    choice.dir = out.path().to_path_buf();
    choice.place = ExportPlace::Last;
    choice.reveal = false;
    dialog.update(cx, |_, cx| cx.emit(ExportDialogEvent::Export(choice)));
    let written = out.path().join("out.tif");
    wait_until(cx, "the export", |cx| {
        written.exists() && !app.read_with(cx, |a, _| a.is_exporting())
    });
    assert!(matches!(image::open(&written).unwrap(), image::DynamicImage::ImageRgb8(_)));
    cx.update(|cx| assert!(crate::prefs::prefs(cx).export.sixteen_bit, "16 still chosen"));
}

/// An Export's name lookup belongs to its sheet: after Cancel and reopening,
/// a late first lookup neither writes nor closes the new sheet.
#[gpui::test]
fn a_canceled_sheets_name_lookup_goes_with_it(cx: &mut TestAppContext) {
    use crate::export_dialog::{ExportDialogEvent, ExportPlace};
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    cx.update(|cx| cx.bind_keys(key_bindings()));
    stack(cx, window, &app);

    let held = crate::work::gate::hold("export-preflight");
    cx.simulate_keystrokes(window, "secondary-e");
    let dialog = app.read_with(cx, |app, _| export_sheet(app)).expect("the sheet is open");
    let out = tempfile::tempdir().unwrap();
    let mut choice = dialog.read_with(cx, |d, cx| d.choice(cx)).expect("it can export");
    choice.format = ExportFormat::Png;
    choice.name = "out.png".into();
    choice.dir = out.path().to_path_buf();
    choice.place = ExportPlace::Last;
    choice.reveal = false;
    dialog.update(cx, |_, cx| cx.emit(ExportDialogEvent::Export(choice)));
    wait_until(cx, "the lookup, held", |_| held.waiting() == 1);
    dialog.update(cx, |_, cx| cx.emit(ExportDialogEvent::Cancel));
    cx.run_until_parked();
    app.read_with(cx, |app, _| assert!(export_sheet(app).is_none(), "canceled"));

    cx.simulate_keystrokes(window, "secondary-e");
    let again = app.read_with(cx, |app, _| export_sheet(app)).expect("open again");
    held.open();
    cx.run_until_parked();
    std::thread::sleep(std::time::Duration::from_millis(200));
    cx.run_until_parked();
    app.read_with(cx, |app, cx| {
        let open = export_sheet(app).map(|d| d.entity_id());
        assert_eq!(open, Some(again.entity_id()), "the new sheet stays");
        assert!(!app.is_exporting(), "nothing written");
        assert_ne!(crate::prefs::prefs(cx).save_dir.as_deref(), Some(out.path()), "nor kept");
    });
    assert!(!out.path().join("out.png").exists());
}
