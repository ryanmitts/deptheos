//! The paint machine's table tests: each state with the events that
//! matter in it and the token rules.

use super::*;
use crate::Gray16Image;
use crate::paint::worker::PickedSource;
use crate::words::explain::Platform;

fn words() -> Words {
    Words::english(Platform::Desktop)
}

fn key(n: u64) -> RenderId {
    RenderId::parse(&format!("{n:032x}")).unwrap()
}

fn frame(n: usize) -> PathBuf {
    PathBuf::from(format!("/frames/{n}.png"))
}

fn kind(n: usize) -> SourceKind {
    SourceKind::Frame { path: frame(n) }
}

/// Four frames over 4 × 2 pixels, each stacked; with `map` a frame map
/// that took the left half from the first frame and the right half from
/// the last.
fn stack(map: bool) -> Arc<CompletedStack> {
    let paths = (0..4).map(frame).collect();
    let mut stack = CompletedStack::srgb(4, 2, vec![0; 24], paths);
    if map {
        stack.frame_map =
            Some(Gray16Image::from_fn(4, 2, |x, _| image::Luma([if x < 2 { 0 } else { 65535 }])));
    }
    Arc::new(stack)
}

fn render(n: u64, first: Option<usize>) -> OpenRender {
    OpenRender {
        key: key(n),
        stack: stack(true),
        frames: (0..4).map(frame).collect(),
        first: first.map(frame),
    }
}

fn machine() -> Machine {
    Machine::new(words())
}

/// A machine with render 1 open on frame 1, its canvas in, the worker
/// caught up and nothing scheduled.
fn ready() -> Machine {
    let mut m = machine();
    m.on(Event::Open(render(1, Some(1))));
    m.on(Event::Worker(canvas(1)));
    m.on(state(1, 1, Some(kind(1)), true));
    let token = scheduled(&m.on(Event::PickFrame(frame(1))));
    m.on(state(1, 2, Some(kind(1)), true));
    m.on(Event::Fired(token.unwrap()));
    m
}

fn display() -> PresentedImage {
    PresentedImage::single(crate::CpuImage::from_rgb(&image::RgbImage::new(1, 1)))
}

fn canvas(n: u64) -> Output {
    Output::Canvas { key: key(n), display: display() }
}

/// The worker's state of render `n`, having answered pick `pick`, on
/// `source` (`ready`: its pixels in).
fn state(n: u64, pick: u64, source: Option<SourceKind>, ready: bool) -> Event {
    let mut s = PaintState::empty(key(n));
    s.pick = pick;
    s.source = source.map(|kind| PickedSource { kind, ready });
    Event::Worker(Output::State(Some(s)))
}

fn orders(actions: &[Action]) -> Vec<&Order> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::Send(o) => Some(o),
            _ => None,
        })
        .collect()
}

fn scheduled(actions: &[Action]) -> Option<Token> {
    actions.iter().find_map(|a| match a {
        Action::Schedule { token, .. } => Some(*token),
        _ => None,
    })
}

fn delays(actions: &[Action]) -> Vec<Duration> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::Schedule { after, .. } => Some(*after),
            _ => None,
        })
        .collect()
}

fn reports(actions: &[Action]) -> Vec<&Report> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::Report(r) => Some(r),
            _ => None,
        })
        .collect()
}

fn pick(kind: SourceKind, scrub: bool, pick: u64) -> Order {
    Order::Pick { kind, scrub, pick }
}

fn selection(m: &Machine) -> SourceKind {
    m.status().unwrap().selection
}

// ---------------------------------------------------------------- opening

#[test]
fn an_open_sends_the_render_and_one_pick_then_fetches_ahead() {
    let mut m = machine();
    let out = m.on(Event::Open(render(1, Some(2))));
    assert_eq!(orders(&out), [&Order::Open(key(1)), &pick(kind(2), false, 1)]);
    assert_eq!(delays(&out), [PREFETCH_AFTER]);
    let status = m.status().unwrap();
    assert_eq!(
        (status.canvas, status.selection, status.ready),
        (CanvasState::Opening, kind(2), false)
    );
    assert_eq!(status.last_frame, Some(frame(2)));
    assert_eq!(status.lit, Some(2), "the map is lit for the frame painted from");
    assert!(matches!(reports(&out)[..], [Report::Status(_)]));
}

#[test]
fn an_open_starts_on_a_frame_the_render_stacked_else_the_original() {
    let mut m = machine();
    // A frame outside the render: its first stacked frame.
    let mut r = render(1, None);
    r.first = Some(PathBuf::from("/elsewhere.png"));
    m.on(Event::Open(r));
    assert_eq!(selection(&m), kind(0));
    // No frames at all: the original, nothing fetched ahead.
    let r = OpenRender { frames: Vec::new(), ..render(2, None) };
    let out = m.on(Event::Open(r));
    assert_eq!(selection(&m), SourceKind::Original);
    assert!(delays(&out).is_empty());
    assert_eq!(m.status().unwrap().lit, None, "the map whole for the original");
}

#[test]
fn the_canvas_lands_only_for_the_render_open() {
    let mut m = machine();
    m.on(Event::Open(render(1, None)));
    // A canvas for a render opened before: dropped.
    let out = m.on(Event::Worker(canvas(9)));
    assert!(out.is_empty());
    assert_eq!(m.status().unwrap().canvas, CanvasState::Opening);
    let out = m.on(Event::Worker(canvas(1)));
    assert!(matches!(reports(&out)[..], [Report::Canvas { .. }, Report::Status(s)]
        if s.canvas == CanvasState::Ready));
}

#[test]
fn a_failed_open_shows_on_the_canvas_and_retry_opens_it_again() {
    let mut m = machine();
    m.on(Event::Open(render(1, Some(3))));
    // Another render's failure says nothing of this one.
    m.on(Event::Worker(Output::OpenFailed { key: key(2), message: "no".into() }));
    assert_eq!(m.status().unwrap().canvas, CanvasState::Opening);
    m.on(Event::Worker(Output::OpenFailed { key: key(1), message: "not opened".into() }));
    assert_eq!(m.status().unwrap().canvas, CanvasState::Failed("not opened".into()));
    // No stroke on a failed canvas, nor while opening.
    let brush = Event::BeginStroke { brush: Brush::default(), at: None };
    assert!(orders(&m.on(brush)).is_empty());
    let out = m.on(Event::Retry);
    assert_eq!(orders(&out), [&Order::Open(key(1)), &pick(kind(3), false, 2)]);
    assert_eq!(m.status().unwrap().canvas, CanvasState::Opening);
    assert!(orders(&m.on(Event::BeginStroke { brush: Brush::default(), at: None })).is_empty());
    // Retry only after a failure.
    assert!(m.on(Event::Retry).is_empty());
    m.on(Event::Worker(canvas(1)));
    let dab = Dab { x: 1.0, y: 1.0, pressure: 1.0 };
    let out = m.on(Event::BeginStroke { brush: Brush::default(), at: Some(dab) });
    assert_eq!(orders(&out), [&Order::BeginStroke(Brush::default()), &Order::Dabs(vec![dab])]);
}

// ---------------------------------------------------------------- closing

#[test]
fn a_render_left_still_gets_its_last_tiles_until_the_worker_closes_it() {
    let mut m = ready();
    let out = m.on(Event::Close);
    assert_eq!(orders(&out), [&Order::Close]);
    assert!(matches!(reports(&out)[..], [Report::Closed { key: k }] if *k == key(1)));
    assert!(m.status().is_none());
    let patches = || Event::Worker(Output::Patches { key: key(1), patches: Vec::new() });
    assert!(matches!(reports(&m.on(patches()))[..], [Report::Patches { .. }]));
    // Nothing else of it: no source, no state, no strokes.
    let source = Output::Source { key: key(1), kind: kind(1), display: Arc::new(display()) };
    assert!(m.on(Event::Worker(source)).is_empty());
    assert!(m.on(Event::Undo).is_empty());
    // The worker's state with none open: closed, its tiles no longer.
    m.on(Event::Worker(Output::State(None)));
    assert!(m.on(patches()).is_empty());
    // Closing again does nothing.
    assert!(m.on(Event::Close).is_empty());
}

/// A source is shown only for the render open.
#[test]
fn a_closed_renders_source_is_not_shown_in_the_next() {
    let mut m = ready();
    m.on(Event::Close);
    m.on(Event::Open(render(2, None)));
    let late = Output::Source { key: key(1), kind: kind(1), display: Arc::new(display()) };
    assert!(m.on(Event::Worker(late)).is_empty());
    // The tiles of the render left no longer come either, another open.
    let tiles = Output::Patches { key: key(1), patches: Vec::new() };
    assert!(m.on(Event::Worker(tiles)).is_empty());
    let own = Output::Source { key: key(2), kind: kind(0), display: Arc::new(display()) };
    assert!(matches!(reports(&m.on(Event::Worker(own)))[..], [Report::Source { .. }]));
}

// ---------------------------------------------------------------- picks

#[test]
fn a_state_from_before_a_pick_never_moves_the_selection_back() {
    let mut m = ready();
    m.on(Event::PickFrame(frame(3)));
    // The worker's state from before it heard of pick 3.
    m.on(state(1, 2, Some(kind(1)), true));
    assert_eq!(selection(&m), kind(3));
    assert!(!m.status().unwrap().ready);
    m.on(state(1, 3, Some(kind(3)), true));
    assert_eq!(selection(&m), kind(3));
    assert!(m.status().unwrap().ready);
}

/// A pick the worker couldn't load falls back to what it kept, once it
/// has answered every pick.
#[test]
fn a_pick_that_failed_falls_back_to_what_the_worker_kept() {
    let mut m = ready();
    m.on(Event::PickRender("other".into()));
    let failed = Output::SourceFailed { kind: kind(9), message: "couldn't load".into() };
    let out = m.on(Event::Worker(failed));
    assert!(matches!(reports(&out)[..], [Report::Failed { key: Some(k), .. }] if *k == key(1)));
    m.on(state(1, 3, Some(kind(1)), true));
    assert_eq!(selection(&m), kind(1));
}

/// A failure moves no selection.
#[test]
fn a_failed_stroke_is_not_a_failed_pick() {
    let mut m = ready();
    m.on(Event::ScrubTo(frame(2)));
    let out = m.on(Event::Worker(Output::Failed("the stroke was refused".into())));
    assert!(matches!(reports(&out)[..], [Report::Failed { .. }]));
    assert_eq!(selection(&m), kind(2));
    // The scrub goes on as it was.
    m.on(Event::ScrubTo(frame(3)));
    let out = m.on(state(1, 3, Some(kind(2)), false));
    assert_eq!(orders(&out), [&pick(kind(3), true, 4)]);
}

#[test]
fn the_eraser_the_brush_and_swap() {
    let mut m = ready();
    m.on(Event::PickOriginal);
    assert_eq!(selection(&m), SourceKind::Original);
    assert_eq!(m.status().unwrap().lit, None);
    // The brush: back to what was painted from before the eraser.
    let out = m.on(Event::UseBrush);
    assert_eq!(selection(&m), kind(1));
    assert_eq!(orders(&out)[0], &pick(kind(1), false, 4));
    // The brush on a frame does nothing.
    assert!(orders(&m.on(Event::UseBrush)).is_empty());
    // Swap goes back and forth.
    m.on(Event::PickRender("r".into()));
    m.on(Event::Swap);
    assert_eq!(selection(&m), kind(1));
    m.on(Event::Swap);
    assert_eq!(selection(&m), SourceKind::Render { render: "r".into() });
    // From the eraser with nothing before it, the brush is the first frame.
    let mut fresh = machine();
    fresh.on(Event::Open(OpenRender { frames: Vec::new(), ..render(3, None) }));
    fresh.on(Event::UseBrush);
    assert_eq!(selection(&fresh), SourceKind::Original, "no frame to go to");
    let mut fresh = machine();
    fresh.on(Event::Open(render(3, Some(2))));
    fresh.on(Event::PickOriginal);
    fresh.on(Event::Swap);
    fresh.on(Event::PickOriginal);
    fresh.on(Event::UseBrush);
    assert_eq!(selection(&fresh), kind(2));
}

#[test]
fn frames_goes_back_to_the_frame_painted_from_last() {
    let mut m = ready();
    // On a frame already: nothing.
    assert!(m.on(Event::UseFrames).is_empty());
    m.on(Event::PickFrame(frame(2)));
    m.on(Event::PickRender("r".into()));
    assert_eq!(m.status().unwrap().last_frame, Some(frame(2)));
    m.on(Event::UseFrames);
    assert_eq!(selection(&m), kind(2));
}

#[test]
fn a_pick_at_a_point_is_the_sharpest_frame_there() {
    let mut m = ready();
    m.on(Event::PickAt { x: 3.5, y: 0.5 });
    assert_eq!(selection(&m), kind(3));
    m.on(Event::PickAt { x: 0.5, y: 0.5 });
    assert_eq!(selection(&m), kind(0));
    // Off the image: nothing.
    assert!(m.on(Event::PickAt { x: 4.0, y: 0.5 }).is_empty());
    assert!(m.on(Event::PickAt { x: f32::NAN, y: 0.5 }).is_empty());
    // A render that keeps no frame map says so.
    let mut m = machine();
    m.on(Event::Open(OpenRender { stack: stack(false), ..render(2, None) }));
    let out = m.on(Event::PickAt { x: 1.0, y: 1.0 });
    assert!(matches!(reports(&out)[..], [Report::Notice { key: k, .. }] if *k == key(2)));
}

// ---------------------------------------------------------------- scrubbing

#[test]
fn a_scrub_keeps_one_frame_in_flight_then_settles_and_fetches_ahead() {
    let mut m = ready();
    let out = m.on(Event::ScrubTo(frame(2)));
    assert_eq!(orders(&out), [&pick(kind(2), true, 3)]);
    // Scrubbed on while it loads: shown at once, not sent.
    let out = m.on(Event::ScrubTo(frame(3)));
    assert!(orders(&out).is_empty());
    assert_eq!(selection(&m), kind(3));
    // The worker answers frame 2: frame 3 goes.
    let out = m.on(state(1, 3, Some(kind(2)), false));
    assert_eq!(orders(&out), [&pick(kind(3), true, 4)]);
    assert_eq!(selection(&m), kind(3), "the worker's frame 2 doesn't take the selection back");
    // Answered with nothing scrubbed since: it settles.
    let out = m.on(state(1, 4, Some(kind(3)), false));
    assert!(orders(&out).is_empty());
    assert_eq!(delays(&out), [SCRUB_SETTLE]);
    let settle = scheduled(&out).unwrap();
    let out = m.on(Event::Fired(settle));
    assert_eq!(orders(&out), [&pick(kind(3), false, 5)]);
    assert_eq!(delays(&out), [PREFETCH_AFTER]);
    // Settled, the frame after it in the direction scrubbed is fetched;
    // past the last frame there is none.
    let out = m.on(Event::Fired(scheduled(&out).unwrap()));
    assert!(orders(&out).is_empty(), "frame 3 is the last");
    // Going back, the frame before.
    m.on(Event::ScrubTo(frame(2)));
    m.on(state(1, 6, Some(kind(2)), false));
    let settle = m.on(Event::Fired(Token(0)));
    assert!(settle.is_empty(), "a token never given does nothing");
    let tokens = m.tokens;
    let out = m.on(Event::Fired(Token(tokens)));
    let out = m.on(Event::Fired(scheduled(&out).unwrap()));
    assert_eq!(orders(&out), [&Order::Prefetch(kind(1))]);
}

#[test]
fn a_timer_counts_only_while_the_state_holds_its_token() {
    let mut m = ready();
    m.on(Event::ScrubTo(frame(2)));
    let settle = scheduled(&m.on(state(1, 3, Some(kind(2)), false))).unwrap();
    // A tap since: the settle is outdated, and so is the tap's prefetch
    // once a stroke begins.
    let prefetch = scheduled(&m.on(Event::PickFrame(frame(0)))).unwrap();
    assert!(m.on(Event::Fired(settle)).is_empty());
    m.on(Event::BeginStroke { brush: Brush::default(), at: None });
    assert!(m.on(Event::Fired(prefetch)).is_empty());
    // And a close outdates everything.
    m.on(Event::EndStroke);
    let prefetch = scheduled(&m.on(Event::PickFrame(frame(1)))).unwrap();
    m.on(Event::Close);
    assert!(m.on(Event::Fired(prefetch)).is_empty());
}

/// A stroke begun while the rail's newest frame waits behind the one in
/// flight paints from the newest: it is sent in full first.
#[test]
fn a_stroke_paints_from_the_frame_on_screen() {
    let mut m = ready();
    m.on(Event::ScrubTo(frame(2)));
    m.on(Event::ScrubTo(frame(3)));
    let out = m.on(Event::BeginStroke { brush: Brush::default(), at: None });
    assert_eq!(orders(&out), [&pick(kind(3), false, 4), &Order::BeginStroke(Brush::default())]);
    // The scrub is over: the answer to frame 2 sends nothing more.
    assert!(orders(&m.on(state(1, 3, Some(kind(2)), false))).is_empty());
    assert_eq!(selection(&m), kind(3));
}

#[test]
fn a_step_fetches_ahead_in_its_own_direction() {
    let mut m = ready();
    let out = m.on(Event::PickFrame(frame(0)));
    let out = m.on(Event::Fired(scheduled(&out).unwrap()));
    assert!(orders(&out).is_empty(), "before the first there is none");
    m.on(Event::PickFrame(frame(2)));
    let out = m.on(Event::Step(-1));
    assert_eq!(selection(&m), kind(1));
    let out2 = m.on(Event::Fired(scheduled(&out).unwrap()));
    assert_eq!(orders(&out2), [&Order::Prefetch(kind(0))]);
    let out = m.on(Event::Step(1));
    assert_eq!(selection(&m), kind(2));
    let out = m.on(Event::Fired(scheduled(&out).unwrap()));
    assert_eq!(orders(&out), [&Order::Prefetch(kind(3))]);
    // From the original, the first frame or the last.
    m.on(Event::PickOriginal);
    m.on(Event::Step(-1));
    assert_eq!(selection(&m), kind(3));
    m.on(Event::PickOriginal);
    m.on(Event::Step(1));
    assert_eq!(selection(&m), kind(0));
}

// ---------------------------------------------------------------- reports

#[test]
fn the_status_is_reported_once_per_change_with_the_workers_counts() {
    let mut m = ready();
    let mut s = PaintState::empty(key(1));
    s.pick = 2;
    s.source = Some(PickedSource { kind: kind(1), ready: true });
    s.undo = 1;
    s.painted = true;
    s.painted_from = vec![kind(1)];
    let out = m.on(Event::Worker(Output::State(Some(s.clone()))));
    assert!(matches!(reports(&out)[..], [Report::Status(st)]
        if st.undo == 1 && st.painted && st.painted_from == [kind(1)]));
    // The same again: nothing new.
    assert!(m.on(Event::Worker(Output::State(Some(s)))).is_empty());
    // A state of another render says nothing of this one.
    assert!(m.on(state(7, 99, Some(kind(0)), true)).is_empty());
    // A present's answer comes whatever is open.
    let out = m.on(Event::Worker(Output::Presented { key: key(5), display: None }));
    assert!(
        matches!(reports(&out)[..], [Report::Presented { key: k, display: None }] if *k == key(5))
    );
}

#[test]
fn nothing_but_an_open_does_anything_while_closed() {
    let mut m = machine();
    for event in [
        Event::Retry,
        Event::Close,
        Event::PickFrame(frame(1)),
        Event::ScrubTo(frame(1)),
        Event::Step(1),
        Event::PickOriginal,
        Event::PickRender("r".into()),
        Event::Swap,
        Event::UseBrush,
        Event::UseFrames,
        Event::PickAt { x: 0.0, y: 0.0 },
        Event::BeginStroke { brush: Brush::default(), at: None },
        Event::Dabs(Vec::new()),
        Event::EndStroke,
        Event::Undo,
        Event::Redo,
        Event::Reset,
        Event::Fired(Token(1)),
        Event::Worker(canvas(1)),
        state(1, 1, None, false),
    ] {
        assert!(m.on(event).is_empty());
    }
}
