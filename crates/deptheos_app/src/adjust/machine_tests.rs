//! The Adjust machine's table tests: each state with the events that
//! matter in it, and the rule that a result counts only while current.

use super::*;
use deptheos_render::Adjustments;

fn key(n: u64) -> RenderId {
    RenderId::parse(&format!("{n:032x}")).unwrap()
}

/// Neutral but for its exposure.
fn at(exposure: f64) -> RenderAdjustments {
    RenderAdjustments {
        adjustments: Adjustments { exposure, ..Adjustments::NEUTRAL },
        ..RenderAdjustments::NONE
    }
}

fn uploads(actions: &[Action]) -> Vec<RequestId> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::UploadReduced(id) => Some(*id),
            _ => None,
        })
        .collect()
}

fn previews(actions: &[Action]) -> Vec<(RequestId, f64)> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::DrawPreview { id, value } => Some((*id, value.adjustments.exposure)),
            _ => None,
        })
        .collect()
}

fn fulls(actions: &[Action]) -> Vec<(RequestId, f64)> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::DrawFull { id, value } => Some((*id, value.adjustments.exposure)),
            _ => None,
        })
        .collect()
}

fn writes(actions: &[Action]) -> Vec<(RequestId, f64)> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::Write { id, value, .. } => Some((*id, value.adjustments.exposure)),
            _ => None,
        })
        .collect()
}

fn counts(actions: &[Action]) -> Vec<RequestId> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::CountHistogram { id, .. } => Some(*id),
            _ => None,
        })
        .collect()
}

fn shown(actions: &[Action]) -> Vec<RequestId> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::Show(id) => Some(*id),
            _ => None,
        })
        .collect()
}

fn closed(actions: &[Action]) -> Option<(RenderId, bool)> {
    actions.iter().find_map(|a| match a {
        Action::Report(Report::Closed { key, redraw }) => Some((key.clone(), *redraw)),
        _ => None,
    })
}

fn phase(m: &Machine) -> Option<Phase> {
    m.status().map(|s| s.phase)
}

/// Render `n` open at `value`, its reduced copy in, nothing drawing.
fn ready(n: u64, value: RenderAdjustments) -> Machine {
    let mut m = Machine::new();
    let a = m.on(Event::Open { key: key(n), value });
    let [upload] = uploads(&a)[..] else { panic!("{a:?}") };
    let a = m.on(Event::ReducedReady(upload));
    assert!(fulls(&a).is_empty(), "the viewer already shows it: {a:?}");
    m
}

/// Set `exposure` and commit it, with every draw and the write landing.
fn change(m: &mut Machine, n: u64, exposure: f64) {
    let a = m.on(Event::Set(at(exposure)));
    let a = [a, m.on(Event::Commit)].concat();
    let [(tick, _)] = previews(&a)[..] else { panic!("{a:?}") };
    let [(write, _)] = writes(&a)[..] else { panic!("{a:?}") };
    let a = m.on(Event::PreviewDrawn(tick));
    let [(full, _)] = fulls(&a)[..] else { panic!("{a:?}") };
    m.on(Event::FullDrawn(full));
    m.on(Event::Written { key: key(n), id: write });
}

#[test]
fn an_open_asks_for_the_reduced_copy_then_is_ready() {
    let mut m = Machine::new();
    let a = m.on(Event::Open { key: key(1), value: at(0.5) });
    assert_eq!(uploads(&a).len(), 1);
    assert_eq!(phase(&m), Some(Phase::Opening));
    assert!(m.closed(&[key(1)]).is_none(), "open: no delete");
    assert!(m.closed(&[key(2)]).is_some());
    m.on(Event::ReducedReady(uploads(&a)[0]));
    assert_eq!(phase(&m), Some(Phase::Ready));
    assert_eq!(m.status().unwrap().value, at(0.5));
}

#[test]
fn an_open_already_on_screen_counts_its_histogram_only() {
    let mut m = Machine::new();
    let upload = uploads(&m.on(Event::Open { key: key(1), value: at(0.5) }))[0];
    let a = m.on(Event::ReducedReady(upload));
    assert!(fulls(&a).is_empty() && previews(&a).is_empty(), "{a:?}");
    let [count] = counts(&a)[..] else { panic!("{a:?}") };
    assert_eq!(shown(&m.on(Event::Counted(count))), [count]);
    // A drag before it lands makes it stale; the tick brings its own.
    let mut m = Machine::new();
    let upload = uploads(&m.on(Event::Open { key: key(1), value: at(0.5) }))[0];
    let count = counts(&m.on(Event::ReducedReady(upload)))[0];
    let tick = previews(&m.on(Event::Set(at(1.0))))[0].0;
    assert!(shown(&m.on(Event::Counted(count))).is_empty());
    assert_eq!(shown(&m.on(Event::PreviewDrawn(tick))), [tick]);
    // A failed count leaves Adjust ready.
    let mut m = Machine::new();
    let upload = uploads(&m.on(Event::Open { key: key(1), value: at(0.5) }))[0];
    let count = counts(&m.on(Event::ReducedReady(upload)))[0];
    assert!(m.on(Event::CountFailed(count)).is_empty());
    assert_eq!(phase(&m), Some(Phase::Ready));
}

#[test]
fn a_drag_keeps_one_tick_in_flight_and_draws_the_newest_next() {
    let mut m = ready(1, RenderAdjustments::NONE);
    let a = m.on(Event::Set(at(0.1)));
    let [(first, e)] = previews(&a)[..] else { panic!("{a:?}") };
    assert_eq!(e, 0.1);
    // Two more while it draws: only the newest follows it.
    assert!(previews(&m.on(Event::Set(at(0.2)))).is_empty());
    assert!(previews(&m.on(Event::Set(at(0.3)))).is_empty());
    let a = m.on(Event::PreviewDrawn(first));
    assert_eq!(shown(&a), [first]);
    let [(second, e)] = previews(&a)[..] else { panic!("{a:?}") };
    assert_eq!(e, 0.3);
    // Held still: the preview stays, nothing in flight, nothing written.
    let a = m.on(Event::PreviewDrawn(second));
    assert_eq!(shown(&a), [second]);
    assert!(previews(&a).is_empty() && fulls(&a).is_empty() && writes(&a).is_empty());
    assert_eq!(m.status().unwrap().undo, 1, "a change in progress undoes");
}

#[test]
fn a_commit_writes_once_and_draws_the_full_picture_after_the_tick() {
    let mut m = ready(1, RenderAdjustments::NONE);
    let tick = previews(&m.on(Event::Set(at(1.0))))[0].0;
    let a = m.on(Event::Commit);
    assert_eq!(writes(&a).iter().map(|w| w.1).collect::<Vec<_>>(), [1.0]);
    assert!(fulls(&a).is_empty(), "waits for the tick in flight");
    let a = m.on(Event::PreviewDrawn(tick));
    let [(full, e)] = fulls(&a)[..] else { panic!("{a:?}") };
    assert_eq!(e, 1.0);
    assert_eq!(shown(&m.on(Event::FullDrawn(full))), [full]);
    // A commit with nothing moved writes nothing and draws nothing.
    let a = m.on(Event::Commit);
    assert!(writes(&a).is_empty() && fulls(&a).is_empty(), "{a:?}");
    assert_eq!(m.status().unwrap().undo, 1);
}

#[test]
fn a_change_from_rest_draws_in_full_when_committed_on_a_held_preview() {
    // A drag that pauses, then ends: the full picture follows the commit.
    let mut m = ready(1, RenderAdjustments::NONE);
    let tick = previews(&m.on(Event::Set(at(0.4))))[0].0;
    m.on(Event::PreviewDrawn(tick));
    let a = m.on(Event::Commit);
    assert_eq!(fulls(&a).iter().map(|f| f.1).collect::<Vec<_>>(), [0.4]);
    assert_eq!(writes(&a).len(), 1);
}

#[test]
fn a_held_arrow_key_keeps_showing_previews() {
    // Each press is a set and a commit; a full picture never holds them up.
    let mut m = ready(1, RenderAdjustments::NONE);
    let mut tick = previews(&m.on(Event::Set(at(0.01))))[0].0;
    m.on(Event::Commit);
    for step in 2..6 {
        let e = f64::from(step) / 100.0;
        assert!(previews(&m.on(Event::Set(at(e)))).is_empty(), "one in flight");
        m.on(Event::Commit);
        let a = m.on(Event::PreviewDrawn(tick));
        assert_eq!(shown(&a), [tick]);
        let [(full, _)] = fulls(&a)[..] else { panic!("{a:?}") };
        // The next press lands before the full picture: a tick again.
        let a = m.on(Event::Set(at(e + 0.001)));
        tick = previews(&a)[0].0;
        m.on(Event::Commit);
        assert!(shown(&m.on(Event::FullDrawn(full))).is_empty(), "stale");
    }
    assert_eq!(m.stale(), 4);
}

#[test]
fn a_stale_result_is_dropped_and_counted() {
    let mut m = ready(1, RenderAdjustments::NONE);
    let first = previews(&m.on(Event::Set(at(0.1))))[0].0;
    m.on(Event::Commit);
    let a = m.on(Event::PreviewDrawn(first));
    let full = fulls(&a)[0].0;
    // A new change starts while the full picture draws.
    let tick = previews(&m.on(Event::Set(at(0.2))))[0].0;
    assert!(shown(&m.on(Event::FullDrawn(full))).is_empty());
    assert!(shown(&m.on(Event::PreviewDrawn(first))).is_empty());
    let a = m.on(Event::DrawFailed { id: full, why: "gone".into() });
    assert_eq!(phase(&m), Some(Phase::Ready), "{a:?}");
    assert_eq!(m.stale(), 3);
    assert_eq!(shown(&m.on(Event::PreviewDrawn(tick))), [tick]);
}

#[test]
fn undo_and_redo_step_through_whole_values_and_each_writes() {
    let mut m = ready(1, RenderAdjustments::NONE);
    change(&mut m, 1, 1.0);
    change(&mut m, 1, 2.0);
    assert_eq!((m.status().unwrap().undo, m.status().unwrap().redo), (2, 0));
    let a = m.on(Event::Undo);
    assert_eq!(writes(&a).iter().map(|w| w.1).collect::<Vec<_>>(), [1.0]);
    assert_eq!(fulls(&a).iter().map(|f| f.1).collect::<Vec<_>>(), [1.0]);
    assert_eq!(m.status().unwrap().value, at(1.0));
    m.on(Event::Undo);
    assert_eq!(m.status().unwrap().value, at(0.0));
    assert!(writes(&m.on(Event::Undo)).is_empty(), "nothing left to undo");
    m.on(Event::Redo);
    assert_eq!(m.status().unwrap().value, at(1.0));
    assert_eq!((m.status().unwrap().undo, m.status().unwrap().redo), (1, 1));
    // A new change clears redo.
    change(&mut m, 1, 5.0);
    assert_eq!(m.status().unwrap().redo, 0);
}

#[test]
fn undo_in_a_change_commits_it_then_takes_it_back() {
    let mut m = ready(1, at(0.5));
    m.on(Event::Set(at(3.0)));
    let a = m.on(Event::Undo);
    assert_eq!(writes(&a).iter().map(|w| w.1).collect::<Vec<_>>(), [3.0, 0.5]);
    assert_eq!(m.status().unwrap().value, at(0.5));
    m.on(Event::Redo);
    assert_eq!(m.status().unwrap().value, at(3.0));
}

#[test]
fn undo_lasts_the_launch_per_render_across_leaving() {
    let mut m = ready(1, RenderAdjustments::NONE);
    change(&mut m, 1, 1.0);
    m.on(Event::Leave);
    m.on(Event::Open { key: key(2), value: RenderAdjustments::NONE });
    assert_eq!(m.status().unwrap().undo, 0, "render 2 has its own");
    m.on(Event::Open { key: key(1), value: at(1.0) });
    assert_eq!(m.status().unwrap().undo, 1);
    m.on(Event::Undo);
    assert_eq!(m.status().unwrap().value, at(0.0));
}

#[test]
fn undo_keeps_the_newest_steps() {
    let mut m = ready(1, RenderAdjustments::NONE);
    for n in 1..=KEPT_STEPS + 3 {
        m.on(Event::Set(at(n as f64 / 1000.0)));
        m.on(Event::Commit);
    }
    assert_eq!(m.status().unwrap().undo, KEPT_STEPS);
}

#[test]
fn leaving_waits_for_its_write_then_closes() {
    let mut m = ready(1, RenderAdjustments::NONE);
    let tick = previews(&m.on(Event::Set(at(1.0))))[0].0;
    let a = m.on(Event::Commit);
    let write = writes(&a)[0].0;
    let a = m.on(Event::PreviewDrawn(tick));
    let full = fulls(&a)[0].0;
    m.on(Event::FullDrawn(full));
    let a = m.on(Event::Leave);
    assert!(closed(&a).is_none(), "the write is still out");
    assert!(m.status().is_none() && m.adjusting().is_none());
    assert!(m.is_writing(&key(1)));
    assert!(m.closed(&[key(1)]).is_none());
    let a = m.on(Event::Written { key: key(1), id: write });
    assert_eq!(closed(&a), Some((key(1), false)));
    assert!(a.contains(&Action::FreeGpu));
    assert!(m.closed(&[key(1)]).is_some());
}

#[test]
fn leaving_mid_drag_commits_and_draws_the_full_picture() {
    let mut m = ready(1, RenderAdjustments::NONE);
    let tick = previews(&m.on(Event::Set(at(2.0))))[0].0;
    let a = m.on(Event::Leave);
    let [(write, 2.0)] = writes(&a)[..] else { panic!("{a:?}") };
    let [(full, 2.0)] = fulls(&a)[..] else { panic!("{a:?}") };
    assert!(shown(&m.on(Event::PreviewDrawn(tick))).is_empty(), "left: stale");
    m.on(Event::Written { key: key(1), id: write });
    let a = m.on(Event::FullDrawn(full));
    assert_eq!(shown(&a), [full]);
    assert_eq!(closed(&a), Some((key(1), false)));
}

#[test]
fn leaving_while_away_has_the_viewer_redraw() {
    let mut m = ready(1, RenderAdjustments::NONE);
    m.on(Event::Set(at(2.0)));
    m.on(Event::Away);
    let a = m.on(Event::Leave);
    assert!(fulls(&a).is_empty(), "no GPU copies to draw from");
    let write = writes(&a)[0].0;
    let a = m.on(Event::Written { key: key(1), id: write });
    assert_eq!(closed(&a), Some((key(1), true)));
}

#[test]
fn only_the_latest_write_of_a_render_counts() {
    let mut m = ready(1, RenderAdjustments::NONE);
    m.on(Event::Set(at(1.0)));
    let first = writes(&m.on(Event::Commit))[0].0;
    m.on(Event::Set(at(2.0)));
    let second = writes(&m.on(Event::Commit))[0].0;
    m.on(Event::Leave);
    m.on(Event::Written { key: key(1), id: first });
    assert!(m.is_writing(&key(1)));
    let a = m.on(Event::WriteFailed { key: key(1), id: second, why: "full disk".into() });
    assert!(
        a.contains(&Action::Report(Report::WriteFailed { key: key(1), why: "full disk".into() }))
    );
    assert!(!m.is_writing(&key(1)));
}

#[test]
fn opening_another_render_leaves_the_first() {
    let mut m = ready(1, RenderAdjustments::NONE);
    m.on(Event::Set(at(1.0)));
    let a = m.on(Event::Open { key: key(2), value: RenderAdjustments::NONE });
    assert_eq!(writes(&a).len(), 1, "render 1's change written");
    assert_eq!(uploads(&a).len(), 1);
    assert_eq!(m.adjusting(), Some(&key(2)));
    // Render 1 is no longer the state's, but its write still holds a delete.
    assert!(m.closed(&[key(1)]).is_none());
    assert!(m.on(Event::Open { key: key(2), value: at(9.0) }).is_empty(), "already open");
}

#[test]
fn away_frees_the_gpu_and_back_makes_the_reduced_copy_again() {
    let mut m = ready(1, RenderAdjustments::NONE);
    let a = m.on(Event::Away);
    assert_eq!(a.first(), Some(&Action::FreeGpu));
    assert_eq!(phase(&m), Some(Phase::Away));
    // A change while away is kept and written, drawn once back.
    m.on(Event::Set(at(1.5)));
    assert_eq!(writes(&m.on(Event::Commit)).len(), 1);
    let a = m.on(Event::Back);
    let [upload] = uploads(&a)[..] else { panic!("{a:?}") };
    let a = m.on(Event::ReducedReady(upload));
    assert_eq!(fulls(&a).iter().map(|f| f.1).collect::<Vec<_>>(), [1.5]);
    // A second away while away changes nothing.
    m.on(Event::Away);
    assert!(m.on(Event::Away).is_empty());
}

#[test]
fn away_mid_drag_draws_in_full_once_back() {
    let mut m = ready(1, RenderAdjustments::NONE);
    let tick = previews(&m.on(Event::Set(at(1.0))))[0].0;
    m.on(Event::Away);
    m.on(Event::Set(at(0.0)));
    let upload = uploads(&m.on(Event::Back))[0];
    assert!(shown(&m.on(Event::PreviewDrawn(tick))).is_empty());
    let a = m.on(Event::ReducedReady(upload));
    assert_eq!(fulls(&a).len(), 1, "a preview was on screen");
}

#[test]
fn a_failed_upload_or_draw_fails_and_retry_opens_again() {
    let mut m = Machine::new();
    let upload = uploads(&m.on(Event::Open { key: key(1), value: at(0.0) }))[0];
    m.on(Event::ReducedFailed { id: upload, why: "out of memory".into() });
    assert_eq!(phase(&m), Some(Phase::Failed("out of memory".into())));
    let upload = uploads(&m.on(Event::Retry))[0];
    m.on(Event::ReducedReady(upload));
    let tick = previews(&m.on(Event::Set(at(1.0))))[0].0;
    let a = m.on(Event::DrawFailed { id: tick, why: "device lost".into() });
    assert!(a.contains(&Action::FreeGpu));
    assert_eq!(phase(&m), Some(Phase::Failed("device lost".into())));
    // Failed keeps the value; a commit still writes it.
    assert_eq!(writes(&m.on(Event::Commit)).len(), 1);
    assert!(m.on(Event::Retry).iter().any(|a| matches!(a, Action::UploadReduced(_))));
}

#[test]
fn a_changed_stored_picture_makes_the_copy_and_the_picture_again() {
    let mut m = ready(1, at(1.0));
    let a = m.on(Event::StoredChanged);
    let [upload] = uploads(&a)[..] else { panic!("{a:?}") };
    let a = m.on(Event::ReducedReady(upload));
    assert_eq!(fulls(&a).iter().map(|f| f.1).collect::<Vec<_>>(), [1.0]);
    // An upload result from before the change is stale.
    let mut m = Machine::new();
    let old = uploads(&m.on(Event::Open { key: key(1), value: at(0.0) }))[0];
    let new = uploads(&m.on(Event::StoredChanged))[0];
    m.on(Event::ReducedReady(old));
    assert_eq!(phase(&m), Some(Phase::Opening));
    m.on(Event::ReducedReady(new));
    assert_eq!(phase(&m), Some(Phase::Ready));
}

#[test]
fn closed_events_do_nothing() {
    let mut m = Machine::new();
    for event in [
        Event::Set(at(1.0)),
        Event::Commit,
        Event::Undo,
        Event::Redo,
        Event::Retry,
        Event::Leave,
        Event::Away,
        Event::Back,
        Event::StoredChanged,
    ] {
        assert!(m.on(event.clone()).is_empty(), "{event:?}");
    }
    assert!(m.closed(&[key(1)]).is_some());
}

#[test]
fn status_is_reported_once_per_change() {
    let mut m = ready(1, RenderAdjustments::NONE);
    let statuses =
        |a: &[Action]| a.iter().filter(|a| matches!(a, Action::Report(Report::Status(_)))).count();
    assert_eq!(statuses(&m.on(Event::Set(at(1.0)))), 1);
    assert_eq!(statuses(&m.on(Event::Set(at(1.0)))), 0, "the same value");
}

#[test]
fn a_moderate_shed_trims_the_preview_and_a_critical_one_goes_away() {
    let mut m = ready(1, RenderAdjustments::NONE);
    assert_eq!(m.on(Event::Shed(Shed::Moderate)), [Action::TrimPreview]);
    assert_eq!(phase(&m), Some(Phase::Ready), "still drawing");
    let tick = previews(&m.on(Event::Set(at(1.0))))[0].0;
    let a = m.on(Event::Shed(Shed::Critical));
    assert_eq!(a.first(), Some(&Action::FreeGpu));
    assert_eq!(phase(&m), Some(Phase::Away));
    // Away, a moderate shed has nothing to trim; the drag's value is kept.
    assert!(m.on(Event::Shed(Shed::Moderate)).is_empty());
    let upload = uploads(&m.on(Event::Back))[0];
    assert!(shown(&m.on(Event::PreviewDrawn(tick))).is_empty(), "stale");
    let a = m.on(Event::ReducedReady(upload));
    assert_eq!(fulls(&a).iter().map(|f| f.1).collect::<Vec<_>>(), [1.0]);
}

#[test]
fn a_moderate_shed_while_opening_or_away_does_nothing() {
    let mut m = Machine::new();
    m.on(Event::Open { key: key(1), value: at(0.0) });
    assert!(m.on(Event::Shed(Shed::Moderate)).is_empty());
    assert_eq!(phase(&m), Some(Phase::Opening));
}
