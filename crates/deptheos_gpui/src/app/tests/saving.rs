//! A render's save while the app moves on (a stack delete, a switch, quit),
//! each asked while its pictures are still encoding (held by
//! [`crate::work::gate`]).

use super::*;

/// Runs a stack whose save is held at its encode, and waits until it is.
fn stack_held(
    cx: &mut TestAppContext,
    window: gpui::AnyWindowHandle,
    app: &Entity<AppView>,
) -> crate::work::gate::Held {
    let held = crate::work::gate::hold("render-encode");
    wait_until(cx, "the images", |cx| app.read_with(cx, |app, cx| app.files.read(cx).can_stack()));
    act(cx, window, app, |app, window, cx| app.run_stacking(window, cx));
    wait_until(cx, "the save, held at its encode", |_| held.waiting() == 1);
    assert!(app.read_with(cx, |app, cx| app.renders.read(cx).is_saving()));
    held
}

fn saved(cx: &mut TestAppContext, app: &Entity<AppView>) -> bool {
    app.read_with(cx, |app, cx| !app.is_processing(cx) && !app.renders.read(cx).is_saving())
}

/// A delete of a stack whose render is being saved is refused, saying so;
/// once the render is saved the stack deletes, its render with it.
#[gpui::test]
fn a_stack_deletes_only_once_its_save_lands(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    stack(cx, window, &app);
    let ours = app.read_with(cx, |app, _| app.stack.clone());
    let held = stack_held(cx, window, &app);
    let delete = |cx: &mut TestAppContext| {
        let stacks = vec![ours.clone()];
        act(cx, window, &app, |app, window, cx| {
            app.renders.update(cx, |r, cx| r.delete_stacks(stacks, window, cx))
        });
    };
    delete(cx);
    cx.run_until_parked();
    let said = cx.update_window(window, |_, window, cx| crate::notices::last_said(window, cx));
    assert_eq!(
        said.unwrap().as_deref(),
        Some(
            "Nothing was deleted, because a render is still being saved. Delete again when it is saved."
        )
    );
    assert!(!app.read_with(cx, |app, cx| app.renders.read(cx).is_deleting()));
    held.open();
    wait_until(cx, "the save", |cx| saved(cx, &app));
    let renders = |cx: &mut TestAppContext| {
        app.read_with(cx, |app, cx| {
            app.renders.read(cx).renders().iter().filter(|r| r.stack == ours).count()
        })
    };
    assert_eq!(renders(cx), 2, "both saved into it");
    delete(cx);
    wait_until(cx, "the delete", |cx| {
        renders(cx) == 0 && !app.read_with(cx, |app, cx| app.renders.read(cx).is_deleting())
    });
    let listed = app.read_with(cx, |app, cx| app.renders.read(cx).store().list().unwrap());
    assert!(listed.iter().all(|r| r.stack != ours), "none left on disk");
}

/// A save landing after a switch goes into its own stack, numbered there,
/// leaving the stack on screen alone.
#[gpui::test]
fn a_save_landing_after_a_switch_keeps_to_its_stack(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    stack(cx, window, &app);
    let first = app.read_with(cx, |app, _| app.stack.clone());
    let held = stack_held(cx, window, &app);
    act(cx, window, &app, |app, window, cx| app.new_stack(window, cx));
    crate::test_support::switched(cx, &app);
    let fresh = app.read_with(cx, |app, _| app.stack.clone());
    assert_ne!(fresh, first);
    held.open();
    wait_until(cx, "the save", |cx| saved(cx, &app));
    let numbers: Vec<u32> = app.read_with(cx, |app, cx| {
        let renders = app.renders.read(cx);
        renders.renders().iter().filter(|r| r.stack == first).map(|r| r.number).collect()
    });
    assert_eq!(numbers, [1, 2], "its stack's next number");
    app.read_with(cx, |app, cx| {
        assert_eq!((&app.stack, app.base.as_ref()), (&fresh, None), "the stack on screen stays");
        assert!(app.renders.read(cx).renders().iter().all(|r| r.stack != fresh));
    });
}

/// Quit while a render's pictures are being encoded waits for its save:
/// the files are still being written until the render is in the Library.
#[gpui::test]
fn quit_waits_for_a_save_being_encoded(cx: &mut TestAppContext) {
    let Some(Fixture { dir: _dir, window, app, .. }) = Fixture::launch(cx, 3) else { return };
    let held = stack_held(cx, window, &app);
    let writing = |cx: &mut TestAppContext| app.read_with(cx, |app, cx| app.must_finish(cx));
    assert!(writing(cx), "quit waits");
    held.open();
    wait_until(cx, "the save", |cx| saved(cx, &app));
    wait_until(cx, "nothing left to write", |cx| !writing(cx));
    let listed = app.read_with(cx, |app, cx| app.renders.read(cx).store().list().unwrap());
    assert_eq!(listed.len(), 1, "on disk before quit goes on");
}
