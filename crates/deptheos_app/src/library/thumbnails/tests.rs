use super::*;

fn id(n: u32) -> RenderId {
    RenderId::parse(&format!("{n:032x}")).unwrap()
}

fn ids(ns: &[u32]) -> Vec<RenderId> {
    ns.iter().map(|&n| id(n)).collect()
}

/// A run of events against one policy, each with the actions it gave,
/// written as "read 1 2", "keep 1", "drop 3" (the token left out).
struct Run {
    thumbs: Thumbnails,
    start: Instant,
    /// The read in flight, as the app would hold it.
    token: Option<ReadToken>,
}

impl Run {
    fn new(kept: usize) -> Self {
        Self { thumbs: Thumbnails::new(kept), start: Instant::now(), token: None }
    }

    fn at(&self, seconds: u64) -> Instant {
        self.start + Duration::from_secs(seconds)
    }

    fn on(&mut self, event: Event) -> Vec<String> {
        let number = |id: &RenderId| u32::from_str_radix(id.as_str(), 16).unwrap().to_string();
        self.thumbs
            .on(event)
            .into_iter()
            .map(|action| match action {
                Action::Read { token, ids } => {
                    self.token = Some(token);
                    format!("read {}", ids.iter().map(number).collect::<Vec<_>>().join(" "))
                }
                Action::Keep(id) => format!("keep {}", number(&id)),
                Action::Drop(id) => format!("drop {}", number(&id)),
            })
            .collect()
    }

    fn want(&mut self, ns: &[u32], seconds: u64) -> Vec<String> {
        let now = self.at(seconds);
        self.on(Event::Wanted { ids: ids(ns), now })
    }

    /// The read in flight lands: `ok` were read, the rest couldn't be.
    fn land(&mut self, asked: &[u32], ok: &[u32], seconds: u64) -> Vec<String> {
        let token = self.token.take().expect("a read in flight");
        let now = self.at(seconds);
        let mut done = Vec::new();
        for &n in asked {
            done.extend(self.on(Event::Read { token, id: id(n), ok: ok.contains(&n), now }));
        }
        done.extend(self.on(Event::Finished { token, now }));
        done
    }
}

#[test]
fn a_thumbnail_goes_through_its_states() {
    let mut r = Run::new(10);
    // Wanted: read once, and not again while it is read or shown.
    assert_eq!(r.want(&[1, 2], 0), ["read 1 2"]);
    assert_eq!(r.want(&[1, 2], 0), Vec::<String>::new(), "being read");
    assert_eq!(r.land(&[1, 2], &[1], 0), ["keep 1"]);
    assert_eq!(r.thumbs.get(&id(1)), Some(Thumb::Shown));
    assert!(matches!(r.thumbs.get(&id(2)), Some(Thumb::Failed { tries: 1, .. })));
    // One that failed: not again until RETRY_AFTER, and TRIES times in all.
    assert_eq!(r.want(&[1, 2], 1), Vec::<String>::new(), "failed just now");
    assert_eq!(r.want(&[1, 2], 5), ["read 2"], "try 2");
    assert_eq!(r.land(&[2], &[], 5), Vec::<String>::new());
    assert_eq!(r.want(&[2], 10), ["read 2"], "try 3");
    assert_eq!(r.land(&[2], &[], 10), Vec::<String>::new());
    assert_eq!(r.want(&[2], 60), Vec::<String>::new(), "no more tries");
    assert_eq!(r.want(&[2], 600), Vec::<String>::new(), "however long after");
}

/// A row coming and going reads a failed thumbnail no more often than
/// [`RETRY_AFTER`] and [`TRIES`] allow.
#[test]
fn a_row_shown_again_doesnt_read_a_failed_thumbnail_again() {
    let mut r = Run::new(10);
    assert_eq!(r.want(&[7], 0), ["read 7"]);
    r.land(&[7], &[], 0);
    let reads: usize = (0..20)
        .map(|i| {
            let shown = r.want(&[7], i);
            let gone = r.want(&[], i);
            let landed = if shown.is_empty() { Vec::new() } else { r.land(&[7], &[], i) };
            shown.len() + gone.len() + landed.len()
        })
        .sum();
    assert_eq!(reads, 2, "the two tries left, five seconds apart");
}

#[test]
fn one_read_at_a_time_and_what_is_wanted_meanwhile_comes_next() {
    let mut r = Run::new(10);
    assert_eq!(r.want(&[1, 2], 0), ["read 1 2"]);
    // Scrolled on while it reads: 3 and 4, then only 4.
    assert_eq!(r.want(&[3, 4], 0), Vec::<String>::new());
    assert_eq!(r.want(&[4], 0), Vec::<String>::new());
    // It lands, and the read after it is of what is wanted now.
    assert_eq!(r.land(&[1, 2], &[1, 2], 0), ["keep 1", "keep 2", "read 4"]);
}

#[test]
fn a_read_of_up_to_so_many_the_most_wanted_first() {
    let mut r = Run::new(100);
    let wanted: Vec<u32> = (1..=30).collect();
    let first = r.want(&wanted, 0);
    let expected: Vec<String> = (1..=PER_READ as u32).map(|n| n.to_string()).collect();
    assert_eq!(first, [format!("read {}", expected.join(" "))]);
    let read: Vec<u32> = (1..=PER_READ as u32).collect();
    let next = r.land(&read, &read, 0);
    assert_eq!(next.last().map(String::as_str), Some("read 25 26 27 28 29 30"));
}

#[test]
fn results_count_only_for_the_read_in_flight() {
    let mut r = Run::new(10);
    r.want(&[1], 0);
    let token = r.token.unwrap();
    let stale = ReadToken(token.0 + 7);
    assert_eq!(
        r.on(Event::Read { token: stale, id: id(1), ok: true, now: r.at(0) }),
        Vec::<String>::new()
    );
    // Deleted while it reads: what lands for it is not kept.
    assert_eq!(r.on(Event::Deleted { ids: ids(&[1]) }), Vec::<String>::new());
    assert_eq!(r.land(&[1], &[1], 0), Vec::<String>::new());
    assert_eq!(r.thumbs.get(&id(1)), None);
    // A read that ends without reporting one (it couldn't start) leaves
    // it to be asked for again, at the next want, not at once.
    r.want(&[2], 1);
    let token = r.token.take().unwrap();
    assert_eq!(r.on(Event::Finished { token, now: r.at(1) }), Vec::<String>::new());
    assert_eq!(r.want(&[2], 1), ["read 2"]);
}

#[test]
fn a_save_shows_at_once_and_a_read_after_it_doesnt_replace_it() {
    let mut r = Run::new(10);
    r.want(&[3], 0);
    assert_eq!(r.on(Event::Saved { id: id(3) }), ["keep 3"]);
    assert_eq!(r.land(&[3], &[3], 0), Vec::<String>::new(), "the save's stays");
    assert_eq!(r.thumbs.get(&id(3)), Some(Thumb::Shown));
}

#[test]
fn past_the_most_kept_the_one_wanted_longest_ago_goes_never_one_in_view() {
    let mut r = Run::new(2);
    r.want(&[1], 0);
    r.land(&[1], &[1], 0);
    r.want(&[2], 1);
    r.land(&[2], &[2], 1);
    // 3 is wanted with 2: 1, wanted longest ago, makes room.
    assert_eq!(r.want(&[2, 3], 2), ["read 3"]);
    assert_eq!(r.land(&[3], &[3], 2), ["drop 1", "keep 3"]);
    // All three wanted at once: none is dropped, the most kept or not.
    r.want(&[1, 2, 3], 3);
    assert_eq!(r.land(&[1], &[1], 3), ["keep 1"]);
    assert_eq!(r.thumbs.shown().len(), 3);
    // Then 2 and 3 are in view: the next want trims to two.
    assert_eq!(r.want(&[2, 3], 4), ["drop 1"]);
}

#[test]
fn shedding_keeps_only_what_is_wanted_and_clearing_drops_all() {
    let mut r = Run::new(10);
    r.want(&[1, 2, 3], 0);
    r.land(&[1, 2, 3], &[1, 2, 3], 0);
    r.want(&[2], 1);
    let mut shed = r.on(Event::Shed);
    shed.sort();
    assert_eq!(shed, ["drop 1", "drop 3"]);
    assert_eq!(r.on(Event::Cleared), ["drop 2"]);
    assert!(r.thumbs.shown().is_empty());
    // Nothing is in flight after: a want reads again.
    assert_eq!(r.want(&[2], 2), ["read 2"]);
}

/// A rewritten thumbnail is let go and read again while wanted; one out
/// of view is only forgotten, and one being read is read again after.
#[test]
fn a_rewritten_thumbnail_is_read_again() {
    let mut run = Run::new(10);
    assert_eq!(run.want(&[1, 2], 0), ["read 1 2"]);
    assert_eq!(run.land(&[1, 2], &[1, 2], 0), ["keep 1", "keep 2"]);
    let now = run.at(1);
    assert_eq!(run.on(Event::Rewritten { id: id(1), now }), ["drop 1", "read 1"]);
    // Rewritten again while that read is in flight: its pixels are old.
    assert!(run.on(Event::Rewritten { id: id(1), now }).is_empty());
    assert_eq!(run.land(&[1], &[1], 1), ["read 1"]);
    assert_eq!(run.land(&[1], &[1], 2), ["keep 1"]);
    // Out of view, it is only let go.
    assert!(run.want(&[1], 3).is_empty());
    let now = run.at(4);
    assert_eq!(run.on(Event::Rewritten { id: id(2), now }), ["drop 2"]);
    assert_eq!(run.thumbs.get(&id(2)), None);
    // A render with no thumbnail yet: nothing to do.
    assert!(run.on(Event::Rewritten { id: id(9), now }).is_empty());
}
