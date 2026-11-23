//! The cache's and a stack's conversions, through a stand-in converter.

use std::os::unix::fs::PermissionsExt as _;

use super::*;

/// A stand-in converter (`-l -c -p0 -d DIR -o NAME INPUT`): logs the input
/// and writes DIR/NAME; fails on `bad.cr3`, hangs on `slow.cr3`.
fn fake_converter(dir: &Path) -> (PathBuf, PathBuf) {
    let log = dir.join("calls.log");
    let exe = dir.join("converter.sh");
    let script = format!(
        "#!/bin/sh\necho \"$8\" >> '{}'\ncase \"$8\" in *bad.cr3) exit 3;; *slow.cr3) exec sleep 60;; esac\n\
         echo dng > \"$5/$7\"\n",
        log.display()
    );
    std::fs::write(&exe, script).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    (exe, log)
}

fn calls(log: &Path) -> usize {
    std::fs::read_to_string(log).map_or(0, |s| s.lines().count())
}

#[test]
fn converts_each_source_once_until_it_changes() {
    let dir = tempfile::tempdir().unwrap();
    let (exe, log) = fake_converter(dir.path());
    let cache = Arc::new(DngCache::new(dir.path().join("cache"), Some(exe)).unwrap());
    let raw = dir.path().join("frame.cr3");
    std::fs::write(&raw, "raw").unwrap();

    let first = cache.convert(&raw).unwrap();
    assert!(first.is_file());
    assert_eq!(cache.convert(&raw).unwrap(), first);
    assert_eq!(calls(&log), 1, "the second request reuses the conversion");

    std::fs::write(&raw, "edited in place").unwrap();
    let second = cache.convert(&raw).unwrap();
    assert_ne!(second, first, "a changed source gets a new key");
    assert_eq!(calls(&log), 2);
}

#[test]
fn a_failed_conversion_leaves_nothing_behind() {
    let dir = tempfile::tempdir().unwrap();
    let (exe, log) = fake_converter(dir.path());
    let cache_dir = dir.path().join("cache");
    let cache = Arc::new(DngCache::new(cache_dir.clone(), Some(exe)).unwrap());
    let bad = dir.path().join("bad.cr3");
    std::fs::write(&bad, "raw").unwrap();

    assert!(cache.convert(&bad).is_err());
    assert!(cache.convert(&bad).is_err());
    assert_eq!(calls(&log), 2, "a failure is not cached");
    assert_eq!(std::fs::read_dir(&cache_dir).unwrap().count(), 0);
}

#[test]
fn the_directory_goes_with_the_cache() {
    let dir = tempfile::tempdir().unwrap();
    let (exe, _) = fake_converter(dir.path());
    let raw = dir.path().join("frame.cr3");
    std::fs::write(&raw, "raw").unwrap();

    let cache_dir = dir.path().join("dropped");
    let cache = Arc::new(DngCache::new(cache_dir.clone(), Some(exe.clone())).unwrap());
    cache.convert(&raw).unwrap();
    drop(cache);
    assert!(!cache_dir.exists());

    let cache_dir = dir.path().join("cleared");
    let cache = Arc::new(DngCache::new(cache_dir.clone(), Some(exe.clone())).unwrap());
    cache.convert(&raw).unwrap();
    cache.clear();
    crate::engine::store::store().write(|| ()).wait();
    assert!(!cache_dir.exists());
}

fn sources(dir: &Path, names: &[&str]) -> Vec<PathBuf> {
    names
        .iter()
        .map(|n| {
            let path = dir.join(n);
            std::fs::write(&path, n).unwrap();
            path
        })
        .collect()
}

/// Wait up to 5 s for `log` to reach `count` conversions.
fn wait_for_calls(log: &Path, count: usize) {
    let start = Instant::now();
    while calls(log) < count && start.elapsed() < std::time::Duration::from_secs(5) {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(calls(log), count);
}

#[test]
fn the_limit_deletes_least_recently_used_files_nobody_holds() {
    let dir = tempfile::tempdir().unwrap();
    let (exe, _) = fake_converter(dir.path());
    // The stand-in writes 4 bytes a conversion: room for two.
    let cache = Arc::new(DngCache::new(dir.path().join("cache"), Some(exe)).unwrap().with_limit(8));
    let raw = sources(dir.path(), &["a.cr3", "b.cr3", "c.cr3", "d.cr3"]);

    let a = cache.open(&raw[0]).unwrap();
    let b = cache.convert(&raw[1]).unwrap();
    let c = cache.convert(&raw[2]).unwrap();
    assert!(a.path().is_file(), "a held file stays");
    assert!(!b.is_file(), "the oldest free file goes");
    assert!(c.is_file());

    let a_path = a.path().to_path_buf();
    drop(a);
    assert!(a_path.is_file(), "letting go deletes nothing while it fits");
    let d = cache.convert(&raw[3]).unwrap();
    assert!(!a_path.is_file(), "once free, the least recently used goes first");
    assert!(c.is_file() && d.is_file());
}

#[test]
fn a_stack_converts_ahead_of_what_it_reads() {
    let dir = tempfile::tempdir().unwrap();
    let (exe, log) = fake_converter(dir.path());
    let cache = Arc::new(DngCache::new(dir.path().join("cache"), Some(exe)).unwrap().with_limit(0));
    let paths =
        sources(dir.path(), &["0.cr3", "1.cr3", "2.cr3", "3.cr3", "4.cr3", "5.jpg", "6.cr3"]);
    let stack = StackConversions::start(cache, paths.clone(), RawBackend::AdobeDng, 1);

    // One job runs two frames ahead of the furthest asked for.
    let first = stack.file(&paths[0]).unwrap();
    assert!(first.is_file());
    wait_for_calls(&log, 3);
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert_eq!(calls(&log), 3, "no further than two ahead");

    assert_eq!(stack.file(&paths[5]).unwrap(), paths[5], "a JPEG is read as is");
    assert!(stack.file(&paths[4]).unwrap().is_file());
    wait_for_calls(&log, 6);

    // Released, a frame is evicted (limit 0); asked again, reconverted.
    stack.release(&paths[0]);
    assert!(!first.is_file());
    assert!(stack.file(&paths[0]).unwrap().is_file());
    assert_eq!(calls(&log), 7);
}

/// Space needed is the first size times the rest when all are held, else
/// capped by the limit and the frames within reach; nothing once done.
#[test]
fn a_stack_says_what_space_its_conversions_need() {
    let dir = tempfile::tempdir().unwrap();
    let (exe, log) = fake_converter(dir.path());
    let cache = Arc::new(DngCache::new(dir.path().join("cache"), Some(exe)).unwrap().with_limit(0));
    let names = ["0.cr3", "1.cr3", "2.cr3", "3.cr3", "4.cr3", "5.cr3", "6.cr3", "7.cr3", "j.jpg"];
    let paths = sources(dir.path(), &names);
    let stack = StackConversions::start(cache.clone(), paths.clone(), RawBackend::AdobeDng, 1);

    // The first asked for, one job makes it and the two after it.
    let each = std::fs::metadata(stack.file(&paths[0]).unwrap()).unwrap().len();
    wait_for_calls(&log, 3);
    std::thread::sleep(std::time::Duration::from_millis(100));
    let held = stack.space_needed(true).unwrap();
    assert_eq!((held.folder.as_path(), held.bytes), (dir.path().join("cache").as_path(), 5 * each));
    // Two ahead and the one job, over a limit of 0.
    assert_eq!(stack.space_needed(false).unwrap().bytes, 3 * each);

    assert!(stack.file(&paths[7]).unwrap().is_file());
    assert!(stack.space_needed(true).is_none(), "all made");
}

#[test]
fn a_stack_follows_its_read_order() {
    let dir = tempfile::tempdir().unwrap();
    let (exe, log) = fake_converter(dir.path());
    let cache = Arc::new(DngCache::new(dir.path().join("cache"), Some(exe)).unwrap());
    let paths = sources(dir.path(), &["0.cr3", "1.cr3", "2.cr3", "3.cr3", "4.cr3", "5.cr3"]);
    let stack = StackConversions::start(cache, paths.clone(), RawBackend::AdobeDng, 1);

    stack.file(&paths[0]).unwrap();
    wait_for_calls(&log, 3);
    // Restarted reversed: the last frames come next, not frame 3.
    let reversed: Vec<PathBuf> = paths.iter().rev().cloned().collect();
    stack.plan(&reversed);
    stack.file(&paths[5]).unwrap();
    wait_for_calls(&log, 5);
    let order = std::fs::read_to_string(&log).unwrap();
    let names: Vec<&str> =
        order.lines().map(|l| Path::new(l).file_name().unwrap().to_str().unwrap()).collect();
    assert_eq!(names, ["0.cr3", "1.cr3", "2.cr3", "5.cr3", "4.cr3"]);
}

#[test]
fn a_failed_conversion_fails_only_its_frame() {
    let dir = tempfile::tempdir().unwrap();
    let (exe, log) = fake_converter(dir.path());
    let cache = Arc::new(DngCache::new(dir.path().join("cache"), Some(exe)).unwrap());
    let paths = sources(dir.path(), &["0.cr3", "bad.cr3", "2.cr3", "3.cr3"]);
    let stack = StackConversions::start(cache, paths.clone(), RawBackend::AdobeDng, 1);

    assert!(stack.file(&paths[1]).is_err());
    assert!(stack.file(&paths[3]).unwrap().is_file(), "the frames after it still convert");
    assert_eq!(calls(&log), 4);
}

#[test]
fn stopping_kills_the_conversions_running() {
    let dir = tempfile::tempdir().unwrap();
    let (exe, log) = fake_converter(dir.path());
    let cache = Arc::new(DngCache::new(dir.path().join("cache"), Some(exe)).unwrap());
    let paths = sources(dir.path(), &["slow.cr3", "1.cr3"]);
    let stack = StackConversions::start(cache, paths.clone(), RawBackend::AdobeDng, 1);
    wait_for_calls(&log, 1);

    let start = Instant::now();
    stack.stop();
    assert!(stack.file(&paths[0]).is_err());
    drop(stack);
    assert!(start.elapsed() < std::time::Duration::from_secs(5), "took {:?}", start.elapsed());
}

#[test]
fn every_asker_of_a_failed_frame_learns_why() {
    let dir = tempfile::tempdir().unwrap();
    let (exe, _log) = fake_converter(dir.path());
    let cache = Arc::new(DngCache::new(dir.path().join("cache"), Some(exe)).unwrap());
    let paths = sources(dir.path(), &["bad.cr3", "1.cr3"]);
    let stack = StackConversions::start(cache, paths.clone(), RawBackend::AdobeDng, 1);

    // The session probes the first frame before it reads it.
    let first = stack.file(&paths[0]).unwrap_err().to_string();
    let again = stack.file(&paths[0]).unwrap_err().to_string();
    assert!(again.contains(&first), "{again:?} keeps {first:?}");
}

#[test]
fn only_camera_raw_is_converted() {
    let dir = tempfile::tempdir().unwrap();
    let (exe, log) = fake_converter(dir.path());
    let cache = Arc::new(DngCache::new(dir.path().join("cache"), Some(exe)).unwrap());
    let inputs = sources(dir.path(), &["a.cr3", "b.dng", "c.jpg", "a.cr3"]);
    let never = Cancel::root();

    let libraw = cache.hold_all(&inputs, RawBackend::LibRaw, 2, &never, |_, _| {}).unwrap();
    let libraw = libraw.unwrap();
    assert!(libraw.is_empty());
    assert_eq!(libraw.file(&inputs[0]), inputs[0]);
    assert_eq!(calls(&log), 0);

    let adobe = cache.hold_all(&inputs, RawBackend::AdobeDng, 2, &never, |_, _| {}).unwrap();
    let adobe = adobe.unwrap();
    assert_eq!(adobe.len(), 1, "a repeated frame converts once");
    assert_eq!(adobe.file(&inputs[0]).extension().unwrap(), "dng");
    assert_eq!(adobe.file(&inputs[1]), inputs[1], "a DNG is read as is");
    assert_eq!(adobe.file(&inputs[2]), inputs[2]);
    assert_eq!(calls(&log), 1);
}

#[test]
fn a_stack_reads_what_was_converted_first() {
    let dir = tempfile::tempdir().unwrap();
    let (exe, log) = fake_converter(dir.path());
    // No room at all: only holding keeps the files.
    let cache = Arc::new(DngCache::new(dir.path().join("cache"), Some(exe)).unwrap().with_limit(0));
    let paths = sources(dir.path(), &["0.cr3", "1.cr3", "2.cr3"]);
    let never = Cancel::root();
    let mut progress = Vec::new();
    let held = cache
        .hold_all(&paths, RawBackend::AdobeDng, 2, &never, |c, t| progress.push((c, t)))
        .unwrap()
        .unwrap();
    assert_eq!(calls(&log), 3);
    assert_eq!(progress.first(), Some(&(1, 3)));
    let files: Vec<PathBuf> = paths.iter().map(|p| held.file(p).to_path_buf()).collect();
    assert!(files.iter().all(|f| f.is_file()), "held files outlive the limit");

    for _ in 0..2 {
        let stack = StackConversions::start(cache.clone(), paths.clone(), RawBackend::AdobeDng, 2);
        for path in &paths {
            assert!(stack.file(path).unwrap().is_file());
            stack.release(path);
        }
    }
    assert_eq!(calls(&log), 3, "every stack reuses the conversions");
    drop(held);
    assert!(files.iter().all(|f| !f.exists()), "let go, they fall to the limit");
}

/// Clearing the cache (at quit) kills a running conversion without
/// waiting for its thread.
#[test]
fn clearing_during_a_conversion_does_not_wait_for_it() {
    let dir = tempfile::tempdir().unwrap();
    let (exe, log) = fake_converter(dir.path());
    let cache = Arc::new(DngCache::new(dir.path().join("cache"), Some(exe)).unwrap());
    let slow = sources(dir.path(), &["slow.cr3"]).remove(0);
    let converting = std::thread::spawn({
        let cache = cache.clone();
        move || cache.convert(&slow)
    });
    wait_for_calls(&log, 1);
    let start = Instant::now();
    cache.clear();
    assert!(start.elapsed() < std::time::Duration::from_secs(1), "took {:?}", start.elapsed());
    assert!(converting.join().unwrap().is_err());
    assert!(start.elapsed() < std::time::Duration::from_secs(5), "took {:?}", start.elapsed());
    let after = sources(dir.path(), &["after.cr3"]).remove(0);
    assert!(matches!(cache.convert(&after), Err(CoreError::Canceled)), "a cleared cache");
}

/// A wait on another thread's conversion ends at its own cancel; the
/// conversion goes on for its owner.
#[test]
fn a_wait_for_another_threads_conversion_watches_its_token() {
    let dir = tempfile::tempdir().unwrap();
    let (exe, log) = fake_converter(dir.path());
    let cache = Arc::new(DngCache::new(dir.path().join("cache"), Some(exe)).unwrap());
    let slow = sources(dir.path(), &["slow.cr3"]).remove(0);
    let owner = std::thread::spawn({
        let (cache, slow) = (cache.clone(), slow.clone());
        move || cache.convert(&slow)
    });
    wait_for_calls(&log, 1);
    let cancel = Cancel::root();
    let waiter = std::thread::spawn({
        let (cache, cancel) = (cache.clone(), cancel.clone());
        move || cache.acquire(&slow, false, &cancel).map(|_| ())
    });
    std::thread::sleep(std::time::Duration::from_millis(50));
    let start = Instant::now();
    cancel.cancel();
    assert!(matches!(waiter.join().unwrap(), Err(CoreError::Canceled)));
    assert!(start.elapsed() < std::time::Duration::from_secs(1), "took {:?}", start.elapsed());
    assert!(!owner.is_finished(), "the owner's conversion goes on");
    assert_eq!(calls(&log), 1, "the waiter converted nothing");
    cache.clear();
    assert!(owner.join().unwrap().is_err());
}

/// A conversion opened within some work dies when that work is canceled.
#[test]
fn an_open_conversion_watches_its_work() {
    let dir = tempfile::tempdir().unwrap();
    let (exe, log) = fake_converter(dir.path());
    let cache = Arc::new(DngCache::new(dir.path().join("cache"), Some(exe)).unwrap());
    let slow = sources(dir.path(), &["slow.cr3"]).remove(0);
    let cancel = Cancel::root();
    let opening = std::thread::spawn({
        let (cache, cancel) = (cache.clone(), cancel.clone());
        move || crate::engine::work::within(&cancel, 1, || cache.open(&slow).map(drop))
    });
    wait_for_calls(&log, 1);
    let start = Instant::now();
    cancel.cancel();
    let opened = opening.join().unwrap();
    let stopped = matches!(
        opened,
        Err(CoreError::Converter(deptheos_core::converter::ConverterError::Stopped(_)))
    );
    assert!(stopped, "{opened:?}");
    assert!(start.elapsed() < std::time::Duration::from_secs(5), "took {:?}", start.elapsed());
}

#[test]
fn a_cancel_kills_converting_first() {
    let dir = tempfile::tempdir().unwrap();
    let (exe, log) = fake_converter(dir.path());
    let cache = Arc::new(DngCache::new(dir.path().join("cache"), Some(exe)).unwrap());
    let paths = sources(dir.path(), &["slow.cr3", "1.cr3"]);
    let cancel = Cancel::root();
    let start = Instant::now();
    let held = std::thread::scope(|scope| {
        scope.spawn(|| {
            wait_for_calls(&log, 1);
            cancel.cancel();
        });
        cache.hold_all(&paths, RawBackend::AdobeDng, 1, &cancel, |_, _| {})
    });
    assert!(matches!(held, Ok(None)));
    assert!(start.elapsed() < std::time::Duration::from_secs(5), "took {:?}", start.elapsed());
}
