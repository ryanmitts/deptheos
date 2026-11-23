//! A commit killed at random: a child process commits generations of a
//! render's paint until killed; the parent checks the file holds one whole
//! generation, no older than the last committed, with nothing set aside.
//!
//! Each generation changes every tile it keeps, deletes a third and
//! replaces the source table, so a torn commit would show.
//! `DEPTHEOS_PAINT_KILLS` sets how many kills (5 by default).

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

use super::canvas::Canvas;
use super::doc::PaintDoc;
use super::source::{Source, SourceTable};
use super::store::{self, Writers};
use super::tile::{TILE_PIXELS, TileData, TileId};

/// The render: 12 × 8 tiles.
const WIDTH: u32 = 1536;
const HEIGHT: u32 = 1024;
/// Set in the child, to the render's folder.
const CHILD: &str = "DEPTHEOS_PAINT_KILL_CHILD";
const TEST: &str = "paint::store_kill::a_commit_killed_at_random_leaves_one_whole";

/// Whether generation `n` paints tile `id`: two thirds of the tiles, a
/// different third left out each time.
fn painted(n: u64, id: TileId) -> bool {
    !(u64::from(id.x) + u64::from(id.y) + n).is_multiple_of(3)
}

/// Tile `id` of generation `n`, incompressible so it spans pages.
fn tile(n: u64, id: TileId) -> TileData {
    let seed = n.wrapping_mul(0x9e37_79b9) ^ u64::from(id.x << 16 | id.y);
    let mut tile = TileData::blank();
    for (i, v) in tile.rgb.iter_mut().enumerate() {
        *v = (seed.wrapping_add(i as u64).wrapping_mul(2_654_435_761) >> 13) as u16;
    }
    tile.coverage.iter_mut().for_each(|c| *c = 255);
    tile.source.iter_mut().for_each(|s| *s = 1);
    debug_assert_eq!(tile.coverage.len(), TILE_PIXELS);
    tile
}

/// The source table of generation `n`: its number, as a render source.
fn sources(n: u64) -> SourceTable {
    SourceTable::of(vec![Source::Original, Source::Render(format!("generation-{n}"))])
}

/// The generation the doc holds, from its source table.
fn generation(doc: &PaintDoc) -> u64 {
    match doc.sources.sources() {
        [Source::Original, Source::Render(name)] => {
            name.strip_prefix("generation-").and_then(|n| n.parse().ok()).expect("a generation")
        }
        other => panic!("not a generation's sources: {other:?}"),
    }
}

/// The child: commit and announce generations until killed.
fn child(dir: &Path) -> ! {
    let base = super::stroke::tests::stack(WIDTH, HEIGHT, 20000);
    let mut doc = store::open_doc(dir, WIDTH, HEIGHT);
    let first = if doc.is_painted() { generation(&doc) + 1 } else { 1 };
    doc.open(base);
    let mut writers = Writers::default();
    for n in first..u64::MAX {
        let canvas: &mut Canvas = doc.canvas_mut().expect("open");
        let [cols, rows] = canvas.grid();
        for y in 0..rows {
            for x in 0..cols {
                let id = TileId { x, y };
                canvas.replace(id, painted(n, id).then(|| Arc::new(tile(n, id))));
            }
        }
        doc.sources = sources(n);
        doc.mark_changed();
        writers.commit(&doc.snapshot().expect("saved")).expect("committed");
        println!("committed {n}");
    }
    unreachable!("generations run out")
}

/// A random number below `below`.
fn random(below: u64) -> u64 {
    use std::hash::{BuildHasher, RandomState};
    RandomState::new().hash_one(Instant::now()) % below
}

#[test]
fn a_commit_killed_at_random_leaves_one_whole() {
    if let Ok(dir) = std::env::var(CHILD) {
        child(Path::new(&dir));
    }
    let kills: usize =
        std::env::var("DEPTHEOS_PAINT_KILLS").ok().and_then(|n| n.parse().ok()).unwrap_or(5);
    let dir = super::store::tests::render_dir();
    let exe = std::env::current_exe().unwrap();
    let mut last = 0;
    let started = Instant::now();
    for kill in 0..kills {
        let mut child = Command::new(&exe)
            .args([TEST, "--exact", "--nocapture", "--test-threads=1"])
            .env(CHILD, dir.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let (said, heard) = channel();
        let out = BufReader::new(child.stdout.take().unwrap());
        let reader = std::thread::spawn(move || {
            for line in out.lines().map_while(Result::ok) {
                if let Some(n) = line.strip_prefix("committed ") {
                    let _ = said.send(n.parse::<u64>().unwrap());
                }
            }
        });
        // Once it commits, killed within a few commits' time.
        let first = heard.recv_timeout(Duration::from_secs(60)).expect("the child commits");
        std::thread::sleep(Duration::from_millis(random(400)));
        child.kill().unwrap();
        child.wait().unwrap();
        reader.join().unwrap();
        let reported = heard.try_iter().last().unwrap_or(first);

        let doc = store::load_doc(dir.path(), WIDTH, HEIGHT)
            .unwrap_or_else(|e| panic!("kill {kill}: the paint can't be read: {e}"))
            .expect("committed");
        let n = generation(&doc);
        assert!(n >= reported, "kill {kill}: generation {n}, but {reported} was committed");
        assert!(n <= reported + 1, "kill {kill}: generation {n} after {reported}");
        let layer = doc.layer().unwrap();
        for (id, at) in (0..layer.grid()[1])
            .flat_map(|y| (0..layer.grid()[0]).map(move |x| TileId { x, y }))
            .zip(&layer.tiles)
        {
            match at {
                Some(t) => {
                    assert!(painted(n, id) && **t == tile(n, id), "kill {kill}: tile {id:?}")
                }
                None => assert!(!painted(n, id), "kill {kill}: tile {id:?} missing"),
            }
        }
        let aside = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with("paint.unreadable-"));
        assert!(!aside, "kill {kill}: nothing set aside");
        assert!(n >= last, "kill {kill}: generations go forward");
        last = n;
    }
    eprintln!(
        "paint kills: {kills} in {:.1} s, the last at generation {last}",
        started.elapsed().as_secs_f64()
    );
}
