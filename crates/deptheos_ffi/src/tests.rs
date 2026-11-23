use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use deptheos_app::engine::work::{self, Lane};
use deptheos_app::frames::folder::Source;
use deptheos_app::frames::list::List;
use deptheos_app::library::export::ExportFormat;
use deptheos_app::library::history::{Batch, StackId};
use deptheos_app::library::stacks::SetupPart;
use deptheos_app::preferences::{AlignmentPreferences, Preference};
use deptheos_app::runs::batch::Split;
use deptheos_app::view::Facts;
use deptheos_app::words::ios::Line;
use deptheos_app::{
    AlignMode, CompletedStack, Overlay, Platform, PyramidConfig, RawBackend, SessionOptions,
    StackColor, StackGeometry, StackerConfig, TiffCompression, Words as AppWords,
};
use deptheos_core::CoreError;
use deptheos_core::io::{self, HostDecoder};

use super::*;

/// A decoder whose header says what it is told and whose decode must never
/// be reached.
struct Claims(FrameHeader);

impl FrameDecoder for Claims {
    fn open_frame(&self, _path: PathBuf) -> std::result::Result<Arc<dyn OpenedFrame>, EngineError> {
        Ok(Arc::new(Claims(self.0.clone())))
    }
}

impl OpenedFrame for Claims {
    fn header(&self) -> FrameHeader {
        self.0.clone()
    }

    fn decode_into(&self, _destination: u64, _length: u64) -> std::result::Result<(), EngineError> {
        panic!("decoded a frame that should have been refused");
    }
}

fn header(width: u32, height: u32, color: FrameColor) -> FrameHeader {
    FrameHeader {
        width,
        height,
        depth: SampleDepth::Sixteen,
        layout: FrameLayout::Rgbx,
        color,
        orientation: 1,
    }
}

/// A header claiming more pixels than any frame has (or none) is refused
/// before anything is allocated.
#[test]
fn a_huge_or_empty_header_is_refused() {
    let refused = |width, height| {
        let header = header(width, height, FrameColor::Untagged);
        let open = HostAdapter(Arc::new(Claims(header))).open(Path::new("/x.heic")).unwrap();
        open.decode().err().unwrap()
    };
    for (width, height) in [(u32::MAX, u32::MAX), (1 << 15, 1 << 15)] {
        let e = refused(width, height);
        assert!(
            matches!(
                e,
                deptheos_core::CoreError::TooLarge { width: w, height: h, max_pixels, .. }
                    if (w, h) == (width, height) && max_pixels == MAX_DECODE_PIXELS
            ),
            "{e}"
        );
    }
    let e = refused(0, 100);
    assert!(matches!(e, deptheos_core::CoreError::Decode { .. }), "{e}");
    assert_eq!(frame_samples(1 << 14, 1 << 15, 4), Ok(1 << 31));
}

/// CMYK, Lab and HDR frames are refused by name when opened, before any
/// buffer is made.
#[test]
fn colors_the_engine_cannot_stack_are_refused_at_open() {
    use deptheos_core::io::Refused;
    for (color, refused) in [
        (FrameColor::Cmyk, Refused::Cmyk),
        (FrameColor::Lab, Refused::Lab),
        (FrameColor::Hdr, Refused::Hdr),
    ] {
        let adapter = HostAdapter(Arc::new(Claims(header(4, 4, color))));
        let e = adapter.open(Path::new("/x.heic")).err().unwrap();
        assert!(matches!(&e, CoreError::Refused { what, .. } if *what == refused), "{e}");
    }
}

/// A frame drawn as stored reaches the input stage with its layout,
/// premultiplied alpha, profile and orientation: a half-transparent
/// premultiplied pixel comes out over white, and a 2 × 1 frame turned a
/// quarter is 1 × 2.
#[test]
fn a_drawn_frame_goes_through_the_input_stage() {
    struct Drawn;
    impl FrameDecoder for Drawn {
        fn open_frame(
            &self,
            _path: PathBuf,
        ) -> std::result::Result<Arc<dyn OpenedFrame>, EngineError> {
            Ok(Arc::new(Drawn))
        }
    }
    impl OpenedFrame for Drawn {
        fn header(&self) -> FrameHeader {
            FrameHeader {
                width: 2,
                height: 1,
                depth: SampleDepth::Eight,
                layout: FrameLayout::Rgba,
                color: FrameColor::Untagged,
                orientation: 6,
            }
        }
        fn decode_into(
            &self,
            destination: u64,
            length: u64,
        ) -> std::result::Result<(), EngineError> {
            let pixels: [u8; 8] = [0, 0, 0, 255, 50, 50, 50, 128];
            assert_eq!(length, 8);
            // SAFETY: the engine passes a live buffer of `length` bytes.
            unsafe { std::ptr::copy_nonoverlapping(pixels.as_ptr(), destination as *mut u8, 8) };
            Ok(())
        }
    }
    let open = HostAdapter(Arc::new(Drawn)).open(Path::new("/x.png")).unwrap();
    let frame =
        io::input_frame(Path::new("/x.png"), open.decode().unwrap(), io::Target::Own).unwrap();
    assert_eq!((frame.image.width(), frame.image.height()), (1, 2));
    let rgb = frame.image.into_rgb8();
    assert_eq!(rgb.get_pixel(0, 0).0, [0, 0, 0]);
    // 50 + (255 − 128) = 177.
    assert_eq!(rgb.get_pixel(0, 1).0, [177, 177, 177]);
}

fn message(e: EngineError) -> String {
    match e {
        EngineError::Failed { message } => message,
        EngineError::Canceled => panic!("canceled, not failed"),
    }
}

/// An engine error reaches the app as the user's sentences, a fault as the
/// fixed one with where to report it, a cancel as a cancel.
#[test]
fn errors_are_told_in_the_users_terms() {
    let words = AppWords::english(Platform::Ios);
    let told = |e: deptheos_app::Error| EngineError::explained(&e, &words);
    assert_eq!(
        message(told(deptheos_app::Error::NotRaw)),
        "Only a stack of RAW or DNG frames can be exported as DNG. Export it as TIFF instead."
    );
    let fault = message(told(deptheos_app::Error::InvalidState("x".into())));
    assert_eq!(
        fault,
        "Deptheos ran into an unexpected problem and stopped. Send Diagnostics… in the \
         Library's Settings sends the details."
    );
    assert_eq!(message(EngineError::fault(&words, Some(Line::SaveFailed), "x")), {
        format!("The render couldn't be saved. {fault}")
    });
    assert!(matches!(told(deptheos_app::Error::Canceled), EngineError::Canceled));
}

/// The Library's tiles across the bridge: one per stack, a batch's stacks
/// together, the counts and the filter's line.
#[test]
fn library_tiles_are_stacks() {
    let id = |n: u64| format!("{n:032x}");
    let label = deptheos_app::library::history::BatchLabel {
        split: deptheos_app::runs::batch::Split::AutoSort,
        skipped: 0,
    };
    let batch = Batch { id: "b".into(), label };
    let day_ms = 86_400_000;
    let render = |n: u64, stack: u64, days: i64, batch: Option<&Batch>| LibraryRender {
        id: id(n),
        stack: id(stack),
        stack_name: None,
        name: None,
        picked: false,
        created_ms: 1_800_000_000_000 + days * day_ms,
        batch: batch.cloned(),
    };
    // Stack 100: made by the batch (1), stacked again later (4). Stack 200:
    // renders 2 and 3.
    let renders = vec![
        render(1, 100, 0, Some(&batch)),
        render(2, 200, 2, None),
        render(3, 200, 2, None),
        render(4, 100, 4, None),
    ];
    let unstacked = vec![UnstackedInfo { stack: id(300), name: None, frames: 3 }];
    let words = Words::new("en".into());
    let tiles = |filter, query: &str| {
        let history = LibraryHistory {
            renders: renders.clone(),
            failures: Vec::new(),
            unstacked: unstacked.clone(),
        };
        library_tiles(
            history,
            filter,
            query.into(),
            "Untitled stack".into(),
            Some("1 GB".into()),
            words.clone(),
        )
    };
    let all = tiles(LibraryFilter::All, "");
    let tile = |stack: u64, ids: &[u64]| LibraryTile {
        stack: id(stack),
        kind: LibraryTileKind::Renders {
            cover: id(ids[0]),
            shown: ids.iter().map(|n| id(*n)).collect(),
            total: ids.len() as u32,
        },
        badge: Some(ids.len().to_string()),
    };
    let runs: Vec<Vec<LibraryRun>> = all.days.iter().map(|d| d.runs.clone()).collect();
    assert_eq!(
        runs,
        [
            vec![LibraryRun::Loose { tiles: vec![tile(200, &[3, 2])] }],
            vec![LibraryRun::Batch {
                batch,
                tiles: vec![tile(100, &[4, 1])],
                started_ms: 1_800_000_000_000,
                failed: 0
            }],
        ]
    );
    assert!(all.days[0].day > all.days[1].day);
    assert_eq!(all.unstacked, unstacked);
    assert_eq!((all.all, all.starred, all.failed_stacks), (3, 0, 0));
    assert_eq!(all.count_line, "3 stacks · 4 renders · 1 GB");
    // A stack with no name is searched by what it is called.
    assert_eq!(tiles(LibraryFilter::All, "untitled").days.len(), 2);
    assert_eq!(tiles(LibraryFilter::Starred, "").count_line, "No starred renders");
}

/// The thumbnail policy across the bridge: a read for what is wanted, a
/// keep for what landed, nothing more for one that just failed.
#[test]
fn the_thumbnail_policy_reads_keeps_and_drops() {
    let id = |n: u64| format!("{n:032x}");
    let policy = ThumbnailPolicy::new(1);
    let [ThumbnailAction::Read { token, ids }] = &policy.wanted(vec![id(1), id(2)])[..] else {
        panic!("a read")
    };
    assert_eq!(ids, &[id(1), id(2)]);
    assert_eq!(policy.read(*token, id(1), true), [ThumbnailAction::Keep { id: id(1) }]);
    assert!(policy.read(*token, id(2), false).is_empty());
    assert!(policy.finished(*token).is_empty(), "2 failed just now");
    assert!(policy.wanted(vec![id(2)]).is_empty());
    assert_eq!(policy.shed(), [ThumbnailAction::Drop { id: id(1) }]);
}

/// The view across the bridge: Depth Map over a frame with Contribution
/// lights it, a failed picture puts the light out, and a left-out frame
/// comes back plain, Contribution kept for the next (ADR-149).
#[test]
fn the_view_lights_the_map_and_comes_back_plain() {
    let view = ViewControl::new();
    let pick = |path: &str, supplied| FramePick { path: path.into(), supplied };
    let facts = Facts { map: true, lights: true };
    view.on(ViewEvent::Shown { result: Some(facts), keep: false, supplied: true });
    let shown = view.on(ViewEvent::ShowFrame { frame: pick("a", true), put_out: false });
    assert_eq!(shown.actions, [ViewAction::Show { path: "a".into() }]);
    let on = view.on(ViewEvent::ToggleContribution);
    assert_eq!(on.contribution, Some(ContributionToggle { on: true, can_change: true }));
    let map = view.on(ViewEvent::ToggleMap { on_screen: Some(pick("a", true)) });
    let [ViewAction::Light { frame, token }] = &map.actions[..] else { panic!("lit: {map:?}") };
    assert_eq!(frame, "a");
    let failed = view.on(ViewEvent::LightFailed { token: *token });
    let whole = ViewState::Map {
        back: MapBack::Frame { overlay: Overlay::Contribution },
        light: ViewLight::Whole,
    };
    assert_eq!(failed.now, whole);
    view.on(ViewEvent::ShowFrame { frame: pick("b", false), put_out: false });
    let off = view.on(ViewEvent::ToggleMap { on_screen: None });
    assert_eq!(off.actions, [ViewAction::Show { path: "b".into() }]);
    assert_eq!(off.now, ViewState::Frame { overlay: Overlay::None }, "left out: plain");
    let kept = Some(ContributionToggle { on: true, can_change: false });
    assert_eq!(off.contribution, kept, "Contribution kept for the next frame, not changed here");
}

/// A render is named and spoken from the engine's words, as on the desktop.
#[test]
fn a_render_is_named_by_the_engine() {
    let words = Words::new("en".into());
    assert_eq!(words.render_name(4, None, false), "#4");
    assert_eq!(words.render_name(4, Some("sharper".into()), true), "#4 ★ · sharper");
    let pyramid = Algorithm::Pyramid { smoothness: 4.0 };
    assert!(words.render_menu_title(4, None, true, pyramid).starts_with("#4 ★ · Pyramid"));
    assert_eq!(words.render_spoken(4, None, Some(pyramid), true), "Render 4, Pyramid, starred");
    assert_eq!(words.render_spoken(3, None, None, false), "Render 3");
}

fn job(label: &str) -> BatchJob {
    BatchJob {
        label: label.into(),
        frames: vec![format!("/{label}/1.png").into(), format!("/{label}/2.png").into()],
    }
}

/// The iOS loop: stack, save, the app goes away mid-stack and comes back,
/// and the stopped stack runs again.
#[test]
fn a_batch_queue_drives_the_app_one_stack_at_a_time() {
    let queue = BatchQueue::new(vec![job("a"), job("b")], Words::new("en".into()));
    let stack = |actions: &[BatchAction]| {
        actions.iter().find_map(|a| match a {
            BatchAction::Stack { stack, frames, .. } => Some((*stack, frames.clone())),
            _ => None,
        })
    };
    let started = queue.start();
    let BatchAction::RecordUnstacked { jobs, .. } = &started[0] else { panic!("{started:?}") };
    assert_eq!(jobs, &[job("a"), job("b")], "every job recorded first");
    let (a, frames) = stack(&started).unwrap();
    assert_eq!(frames, job("a").frames);
    let made = queue.stacked();
    let save = BatchSave { stack: a, job: 0, listed: job("a").frames };
    assert_eq!(made.save, Some(save), "the stack just made, recording its frames");
    assert_eq!(stack(&made.then).unwrap().1, job("b").frames);
    assert!(queue.saved(a).is_empty());
    // The background stops b with a GPU error: it isn't a failure.
    assert!(queue.pause().is_empty());
    assert!(queue.failed("device lost".into()).is_empty());
    assert_eq!(queue.status().progress, "Batch 1 of 2 · paused");
    let (b, frames) = stack(&queue.resume()).unwrap();
    assert_eq!(frames, job("b").frames);
    assert!(queue.resume().is_empty(), "no second stack");
    let made = queue.stacked();
    assert_eq!(made.save.map(|s| (s.stack, s.job)), Some((b, 1)));
    assert!(made.then.is_empty());
    let actions = queue.saved(b);
    assert_eq!(actions, [BatchAction::Finish { summary: "Batch: 2 of 2 made".into() }]);
}

/// An async call, waited for, as a test makes the bridge's calls.
pub(crate) fn now<F: std::future::Future>(call: F) -> F::Output {
    futures::executor::block_on(call)
}

pub(crate) fn req() -> Arc<Request> {
    Request::new()
}

pub(crate) fn engine(dir: &Path) -> Arc<Engine> {
    now(Engine::new(
        Arc::new(crate::paint::tests::ImageCrate),
        dir.join("History"),
        None,
        "en".into(),
    ))
    .unwrap()
}

/// An engine whose Library won't open says so first, with the shared
/// layer's reason: one a newer version made names it, one already open is
/// in use.
#[test]
fn an_engine_whose_library_wont_open_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let history = dir.path().join("History");
    let start = || {
        let decoder = Arc::new(crate::paint::tests::ImageCrate);
        match now(Engine::new(decoder, history.clone(), None, "en".into())) {
            Err(EngineError::Failed { message }) => message,
            Err(EngineError::Canceled) => panic!("canceled"),
            Ok(_) => panic!("started"),
        }
    };
    let held = deptheos_app::library::history::History::open(&history).unwrap();
    let said = start();
    assert!(said.starts_with("Deptheos can't open its Library. "), "{said}");
    assert!(said.contains("in another Deptheos on this computer"), "{said}");
    drop(held);

    let sources = history.join("sources.json");
    std::fs::write(&sources, r#"{"schema": 99, "written_by": "0.9.0", "roots": {}}"#).unwrap();
    let said = start();
    assert!(said.starts_with("Deptheos can't open its Library. "), "{said}");
    assert!(said.contains("last used by Deptheos 0.9.0"), "{said}");
    assert!(said.contains("Get the latest version"), "{said}");
}

/// A frame in the app's container is found after the container moves (a
/// reinstall): the Library names it from the container, with no relink.
#[test]
fn a_frame_in_the_app_follows_its_container() {
    let dir = tempfile::tempdir().unwrap();
    let open = |app: &Path| {
        // The app makes its Library's folder before it starts the engine.
        let history = app.join("Library/History");
        std::fs::create_dir_all(&history).unwrap();
        let decoder = Arc::new(crate::paint::tests::ImageCrate);
        now(Engine::new(decoder, history, Some(PathBuf::from(app)), "en".into())).unwrap()
    };
    let app = dir.path().join("Container-1");
    let imports = app.join("Documents/Imports/Bee");
    std::fs::create_dir_all(&imports).unwrap();
    let listed: Vec<PathBuf> = ["a.jpg", "b.jpg"]
        .iter()
        .map(|n| {
            std::fs::write(imports.join(n), n).unwrap();
            imports.join(n)
        })
        .collect();
    let engine = open(&app);
    save(&engine, &listed, &listed, &new_stack_id());
    drop(engine);

    let moved = dir.path().join("Container-2");
    std::fs::rename(&app, &moved).unwrap();
    let renders = now(open(&moved).list_renders(req())).unwrap();
    let at = moved.join("Documents/Imports/Bee/a.jpg");
    assert_eq!(renders[0].frames[0], at);
}

/// `names` in a folder `shoot` under `dir`, as the app lists them, since
/// the Library names a frame by its folder.
fn frames(dir: &Path, names: &[&str]) -> Vec<PathBuf> {
    let folder = dir.join("shoot");
    std::fs::create_dir_all(&folder).unwrap();
    for name in names {
        let file = folder.join(name);
        if !file.exists() {
            std::fs::write(&file, name).unwrap();
        }
    }
    names.iter().map(|n| folder.join(n)).collect()
}

/// A finished stack of `stacked`, made by hand with `options`, as
/// [`Engine::stack`] returns one.
fn result(engine: &Engine, stacked: &[PathBuf], options: SessionOptions) -> Arc<StackResult> {
    let (width, height) = (600, 400);
    let stack = CompletedStack {
        width,
        height,
        pixels: (0..width * height * 3).map(|i| (i * 7) as u16).collect(),
        frame_map: None,
        color: StackColor::Srgb { sixteen_bit: true, profile: None },
        paths: stacked.iter().map(PathBuf::from).collect(),
        geometry: StackGeometry {
            transforms: vec![None; stacked.len()],
            crop: None,
            reversed: false,
            left_out: Vec::new(),
        },
        config: StackerConfig::Pyramid(PyramidConfig { smoothness: 4.0 }),
        options,
        notes: Vec::new(),
        contributions: Default::default(),
    };
    StackResult::new(engine.engine.clone(), engine.engine.present_stack(stack).unwrap())
}

/// Stack `stacked` (from `listed`) as iOS does, into `stack`.
fn save(engine: &Arc<Engine>, listed: &[PathBuf], stacked: &[PathBuf], stack: &str) -> RenderInfo {
    let options = StackSettings::of(
        &StackerConfig::Pyramid(PyramidConfig { smoothness: 4.0 }),
        &SessionOptions::default(),
    )
    .options();
    let result = result(engine, stacked, options);
    now(engine.clone().save_render(listed.to_vec(), result, stack.to_string(), None)).unwrap()
}

/// Stacking twice in one stack gives #1 and #2, named for their folder
/// until renamed; deleting #1 leaves #2; a new stack id starts at #1.
#[test]
fn renders_are_numbered_in_the_working_stack() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let listed = frames(dir.path(), &["a.jpg", "b.jpg", "c.jpg"]);
    let stack = new_stack_id();
    let first = save(&engine, &listed, &listed, &stack);
    let second = save(&engine, &listed, &listed[..2], &stack);
    assert_eq!((first.number, second.number), (1, 2));
    assert_eq!((first.stack.as_str(), second.stack.as_str()), (stack.as_str(), stack.as_str()));
    assert_eq!(second.stack_name.as_deref(), Some("shoot"));

    now(engine.clone().rename_stack(stack.clone(), Some("Bee".into()))).unwrap();
    let names: Vec<_> = now(engine.clone().list_renders(req()))
        .unwrap()
        .into_iter()
        .map(|r| (r.stack_name, r.number))
        .collect();
    assert_eq!(names, [(Some("Bee".to_string()), 1), (Some("Bee".to_string()), 2)]);
    assert_eq!(
        save(&engine, &listed, &listed, &stack).stack_name.as_deref(),
        Some("Bee"),
        "saved into it"
    );

    now(engine.clone().delete_renders(vec![first.id], None)).unwrap();
    let numbers: Vec<_> =
        now(engine.clone().list_renders(req())).unwrap().iter().map(|r| r.number).collect();
    assert_eq!(numbers, [2, 3], "numbers never shift");
    now(engine.clone().rename_stack(stack.clone(), Some("Ant".into()))).unwrap();
    let renamed = save(&engine, &listed, &listed, &stack);
    assert_eq!(renamed.stack_name.as_deref(), Some("Ant"), "renamed since the last listing");
    now(engine.clone().rename_stack(stack.clone(), None)).unwrap();
    let unnamed = save(&engine, &listed, &listed, &stack);
    assert_eq!(unnamed.stack_name.as_deref(), Some("shoot"), "its folder's name again");
    assert_eq!(save(&engine, &listed, &listed, &new_stack_id()).number, 1);
}

/// Deleting the base makes the stack's newest render left the base, none
/// once none is left; deleting another render leaves the base.
#[test]
fn a_deleted_base_is_the_newest_left() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let listed = frames(dir.path(), &["a.jpg", "b.jpg", "c.jpg"]);
    let stack = new_stack_id();
    let first = save(&engine, &listed, &listed, &stack);
    let second = save(&engine, &listed, &listed[..2], &stack);
    let third = save(&engine, &listed, &listed, &stack);
    save(&engine, &listed, &listed, &new_stack_id());
    let delete = |id: &str, base: &str| {
        now(engine.clone().delete_renders(vec![id.to_string()], Some(base.to_string())))
            .unwrap()
            .base
    };
    assert_eq!(delete(&first.id, &third.id), Some(third.id.clone()), "not the base");
    assert_eq!(delete(&third.id, &third.id), Some(second.id.clone()), "the newest left");
    assert_eq!(delete(&second.id, &second.id), None, "none left in its stack");
    assert_eq!(now(engine.clone().delete_renders(vec![], None)).unwrap().base, None);
    assert_eq!(
        unstarred_of(stack.clone(), now(engine.clone().list_renders(req())).unwrap()),
        Vec::<String>::new(),
        "nothing of the stack left"
    );
}

/// Delete Library: every stack's renders go, the frames stay; the question
/// says what goes.
#[test]
fn the_library_is_deleted_through_the_bindings() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let listed = frames(dir.path(), &["a.jpg", "b.jpg", "c.jpg"]);
    std::fs::write(&listed[0], b"a frame").unwrap();
    let shown = save(&engine, &listed, &listed, &new_stack_id());
    save(&engine, &listed, &listed[..2], &new_stack_id());

    now(engine.clone().delete_library()).unwrap();
    assert!(now(engine.clone().list_renders(req())).unwrap().is_empty());
    assert!(now(engine.clone().list_failures(req())).unwrap().is_empty());
    assert!(now(engine.clone().render_sizes(req())).unwrap().is_empty());
    assert!(Path::new(&listed[0]).exists(), "the frames stay");
    assert!(now(engine.clone().load_render(req(), shown.id)).is_err(), "a render deleted is gone");

    let words = Words::new("en".into());
    let question = words.delete_library_question(2, 2, Some("4 MB".into()));
    assert_eq!(question.title, "Delete the Library?");
    let message = question.message;
    assert!(message.starts_with("2 stacks and 2 renders are deleted"), "{message}");
    assert!(message.ends_with("This frees 4 MB."), "{message}");
}

/// Recently Deleted: a deleted render returns its record, worded for the
/// notice; `deleted` lists it with size and thumbnail; `restore_deleted`
/// brings it back; deleted again and emptied, nothing is left.
#[test]
fn recently_deleted_through_the_bindings() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let listed = frames(dir.path(), &["a.jpg", "b.jpg", "c.jpg"]);
    let stack = new_stack_id();
    let first = save(&engine, &listed, &listed, &stack);
    let second = save(&engine, &listed, &listed[..2], &stack);
    now(engine.clone().rename_stack(stack.clone(), Some("Bee".into()))).unwrap();

    let records =
        now(engine.clone().delete_renders(vec![second.id.clone()], None)).unwrap().records;
    assert_eq!(records.len(), 1, "one record for its stack");
    let record = &records[0];
    assert_eq!((record.stack.as_str(), record.whole), (stack.as_str(), false));
    assert_eq!((record.numbers.as_slice(), &record.renders), (&[2][..], &vec![second.id.clone()]));
    assert!(record.size > 0, "sized");
    let words = Words::new("en".into());
    assert_eq!(
        words.deleted_notice(records.clone(), "Untitled stack".into()),
        "Bee #2 moved to Recently Deleted."
    );
    assert_eq!(words.deleted_title(record.clone(), "Untitled stack".into()), "Bee #2");
    assert_eq!(words.deleted_days_left(record.clone()), "30 days left");

    let ids = |engine: &Arc<Engine>| -> Vec<String> {
        now(engine.clone().list_renders(req())).unwrap().into_iter().map(|r| r.id).collect()
    };
    assert_eq!(ids(&engine), std::slice::from_ref(&first.id), "gone from the history");
    assert_eq!(now(engine.clone().deleted(req())).unwrap(), records, "listed");
    let thumbnail = now(engine.clone().deleted_thumbnail(req(), record.id.clone())).unwrap();
    assert!(thumbnail.width > 0, "its render's thumbnail");

    now(engine.clone().restore_deleted(vec![record.id.clone()])).unwrap();
    assert_eq!(ids(&engine), [first.id.clone(), second.id.clone()], "back in the history");
    assert!(now(engine.clone().deleted(req())).unwrap().is_empty(), "no longer listed");

    let stack_records =
        now(engine.clone().delete_stack(stack.clone(), ids(&engine), Vec::new())).unwrap();
    assert!(stack_records[0].whole, "the whole stack is one record");
    assert_eq!(
        words.deleted_notice(stack_records, "Untitled stack".into()),
        "Bee moved to Recently Deleted."
    );
    now(engine.clone().empty_deleted()).unwrap();
    assert!(now(engine.clone().deleted(req())).unwrap().is_empty(), "emptied");
    assert!(ids(&engine).is_empty());
}

/// The working version round-trips through list.json; Edited counts
/// against its base; switching stacks and back restores it exactly.
#[test]
fn the_working_version_through_the_bindings() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let listed = frames(dir.path(), &["a.jpg", "b.jpg", "c.jpg"]);
    let empty = now(engine.clone().read_working(req())).unwrap();
    assert!(empty.base.is_none() && empty.list.files.is_empty());
    assert!(!showing(&engine, empty, None).edited, "no base");

    let stack = new_stack_id();
    let render = save(&engine, &listed, &listed, &stack);
    let working = now(engine.clone().working_of_render(req(), render.id.clone())).unwrap();
    assert_eq!(working.stack, stack);
    assert_eq!(working.base.as_deref(), Some(render.id.as_str()));
    // Against its base, as the app asks with the base shown.
    let since = |working: &WorkingInfo| showing(&engine, working.clone(), Some(render.id.clone()));
    assert!(!since(&working).edited);

    // One frame out and the smoothness changed: two changes since #1.
    let mut edited = WorkingInfo {
        list: list_set_included(working.list.clone(), vec![listed[1].clone()], false),
        ..working.clone()
    };
    if let Some(settings) = &mut edited.settings {
        settings.algorithm = Algorithm::Pyramid { smoothness: 6.0 };
    }
    let changes = since(&edited).change_line;
    assert_eq!(changes.as_deref(), Some("1 change to the settings and 1 to the frames since #1"));

    engine.keep_working(edited.clone()).unwrap();
    assert!(now(engine.clone().written(5000)).unwrap());
    let read = now(engine.clone().read_working(req())).unwrap();
    assert_eq!((&read.stack, &read.base, &read.list), (&edited.stack, &edited.base, &edited.list));
    assert_eq!(since(&read).change_line, changes, "settings kept too");

    // Away to a stack of its own and back: the edits are there.
    let other = save(&engine, &listed, &listed, &new_stack_id());
    let away = now(engine.clone().switch_stack(edited.clone(), other.stack.clone(), None))
        .unwrap()
        .unwrap();
    assert_eq!(away.base.as_deref(), Some(other.id.as_str()), "its newest render");
    let back = now(engine.clone().switch_stack(away, stack.clone(), None)).unwrap().unwrap();
    assert_eq!((&back.base, &back.list), (&edited.base, &edited.list));
    assert_eq!(since(&back).change_line, changes);
    assert_eq!(now(engine.clone().read_working(req())).unwrap().list, back.list, "on screen");

    // A stack with nothing to switch to: nothing changes.
    assert!(
        now(engine.clone().switch_stack(back.clone(), new_stack_id(), None)).unwrap().is_none()
    );

    // Its base deleted: no longer edited "since" anything.
    now(engine.clone().delete_renders(vec![render.id.clone()], None)).unwrap();
    assert!(!since(&back).edited);
}

/// Frames listed and never stacked are kept when another stack goes on
/// screen, listed as unstacked until opened again or discarded.
#[test]
fn a_stack_never_stacked_is_listed_through_the_bindings() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    // In a folder named "shoot".
    let listed = frames(dir.path(), &["a.jpg", "b.jpg", "c.jpg"]);
    let other = save(&engine, &listed, &listed, &new_stack_id());
    let fly = WorkingInfo {
        stack: new_stack_id(),
        base: None,
        list: now(list_add(
            req(),
            now(engine.clone().read_working(req())).unwrap().list,
            listed.clone(),
        ))
        .unwrap(),
        settings: None,
    };
    assert!(now(engine.clone().list_unstacked(req())).unwrap().is_empty());

    let away =
        now(engine.clone().switch_stack(fly.clone(), other.stack.clone(), None)).unwrap().unwrap();
    let unstacked = now(engine.clone().list_unstacked(req())).unwrap();
    let want = UnstackedInfo { stack: fly.stack.clone(), name: Some("shoot".into()), frames: 3 };
    assert_eq!(unstacked, [want]);

    // Its frames still need their folder: Settings counts it beside the
    // stack on screen, whose render is of the same folder.
    let folders = now(engine.clone().read_folders(req())).unwrap();
    let stacks: Vec<_> = folders.iter().map(|f| f.stacks).collect();
    assert_eq!(stacks, [2], "{folders:?}");

    // Opened again it is on screen, no longer kept.
    let back =
        now(engine.clone().switch_stack(away.clone(), fly.stack.clone(), None)).unwrap().unwrap();
    assert_eq!(back.list, fly.list);
    assert!(now(engine.clone().list_unstacked(req())).unwrap().is_empty());
}

/// A desktop render with options iOS doesn't show isn't edited by opening
/// it on iOS.
#[test]
fn a_desktop_render_opens_unedited() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let listed = frames(dir.path(), &["a.jpg", "b.jpg"]);
    let options = SessionOptions {
        alignment: deptheos_app::Alignment::On(deptheos_app::AlignSettings {
            mode: AlignMode::Grid,
            ..Default::default()
        }),
        order: deptheos_app::FrameOrder::CaptureTime,
        ..SessionOptions::default()
    };
    let result = result(&engine, &listed, options);
    let render = now(engine.clone().save_render(listed, result, new_stack_id(), None)).unwrap();
    let working = now(engine.clone().working_of_render(req(), render.id.clone())).unwrap();
    assert!(!showing(&engine, working, Some(render.id)).edited);
}

/// A render loaded to show has its display until let go; one loaded for
/// its pixels has none and is the same render.
#[test]
fn a_render_loads_with_or_without_its_display() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let listed = frames(dir.path(), &["a.jpg", "b.jpg"]);
    let made = result(&engine, &listed, SessionOptions::default());
    let render = now(engine.clone().save_render(listed, made, new_stack_id(), None)).unwrap();

    let shown = now(engine.clone().load_render(req(), render.id.clone())).unwrap();
    assert!(shown.held(false).is_some());
    shown.release_displays();
    assert!(shown.held(false).is_none() && shown.held(true).is_none());

    let pixels = now(engine.clone().load_render_pixels(req(), render.id)).unwrap();
    assert!(pixels.held(false).is_none() && pixels.held(true).is_none());
    assert_eq!((pixels.width(), pixels.height()), (shown.width(), shown.height()));
    assert_eq!(pixels.frames(), shown.frames());
    assert_eq!(pixels.export_formats(), shown.export_formats());
    // Nothing to thumbnail: it can't be kept as a new render.
    let again = now(engine.clone().save_render(Vec::new(), pixels, new_stack_id(), None));
    assert!(again.is_err());
}

/// A frame and another render come back lined up with the result, at its
/// size, in their own pixels.
#[test]
fn frames_and_renders_compare_with_the_result() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let (width, height) = (300, 200);
    let frames: Vec<PathBuf> = [255u8, 0]
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            let path = dir.path().join(format!("f{i}.png"));
            image::RgbImage::from_pixel(width, height, image::Rgb([v, v, v])).save(&path).unwrap();
            PathBuf::from(&path)
        })
        .collect();
    let paths: Vec<PathBuf> = frames.iter().map(PathBuf::from).collect();
    let result = |gray: u16| {
        let pixels = vec![gray; (3 * width * height) as usize];
        let stack = CompletedStack {
            color: StackColor::Srgb { sixteen_bit: false, profile: None },
            ..CompletedStack::srgb(width, height, pixels, paths.clone())
        };
        StackResult::new(engine.engine.clone(), engine.engine.present_stack(stack).unwrap())
    };
    let (gray, black) = (result(128 * 257), result(0));

    let compared = |result: &StackResult, of| result.picture_of(&of).map(Option::unwrap);
    let white =
        compared(&gray, PictureOf::ComparedFrame { path: frames[0].clone(), contribution: false })
            .unwrap();
    assert_eq!(white.size(), PictureSize { width, height });
    let white = white.base(4096).unwrap();
    assert!(white.bgra[..3].iter().all(|&b| b > 250), "the white frame");
    assert!(
        !gray.has_contribution()
            || compared(
                &gray,
                PictureOf::ComparedFrame { path: frames[1].clone(), contribution: true }
            )
            .is_ok()
    );

    let other = compared(&gray, PictureOf::ComparedRender { other: black }).unwrap();
    assert_eq!(other.size(), PictureSize { width, height });
    let other = other.base(4096).unwrap();
    assert!(other.bgra[..3].iter().all(|&b| b < 5), "the black render");
}

/// Trying a failed batch stack again joins its stack, with no base.
#[test]
fn a_failure_is_tried_again_in_its_stack() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let listed = frames(dir.path(), &["a.jpg", "b.jpg"]);
    let settings = StackSettings {
        algorithm: Algorithm::Depth { radius: 8.0, smoothing: 5.0, blend_range: 3.0 },
        align: true,
        align_method: AlignMode::Grid,
        crop: false,
        raw_decoder: RawBackend::Apple,
    };
    let failure = now(engine.clone().save_failure(
        "Bee".into(),
        listed.clone(),
        settings,
        crate::stacks::new_stack_id(),
        Batch::new(deptheos_app::library::history::BatchLabel::default()),
        "boom".into(),
    ))
    .unwrap();
    let working = failure_working(failure.clone());
    assert_eq!((working.stack.as_str(), working.base.as_ref()), (failure.stack.as_str(), None));
    assert_eq!(working.list.files, listed);
    assert!(matches!(working.settings.map(|s| s.algorithm), Some(Algorithm::Depth { .. })));
}

/// Camera RAW pairs with its JPEG whoever decodes it.
#[test]
fn camera_raw_pairs_with_its_jpeg() {
    let paths = ["d/P2.JPG", "d/P1.ORF", "d/p1.jpg", "d/P2.ORF", "d/P3.heic"];
    let list = ListInfo {
        files: paths.iter().map(PathBuf::from).collect(),
        ..ListInfo::from(&List::default())
    };
    let rows = list_rows(list);
    let got: Vec<_> =
        rows.iter().map(|r| (r.stem.as_str(), r.file.to_str().unwrap(), r.is_pair)).collect();
    assert_eq!(
        got,
        [("P1", "d/P1.ORF", true), ("P2", "d/P2.ORF", true), ("P3", "d/P3.heic", false)]
    );
}

/// The list round-trips: rows by shot, a pair's exclusion, the RAW / JPEG
/// choice, removal and following a relink.
#[test]
fn the_list_through_the_bindings() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("shoot");
    std::fs::create_dir_all(&folder).unwrap();
    for name in ["P1.dng", "P1.jpg", "P2.dng", "P3.dng"] {
        std::fs::write(folder.join(name), b"").unwrap();
    }
    let list = now(list_add(req(), ListInfo::default(), vec![PathBuf::from(&folder)])).unwrap();
    let rows = list_rows(list.clone());
    assert_eq!(rows.len(), 3);
    assert!(rows[0].is_pair && rows[0].file.ends_with("P1.dng"));
    let list = list_set_included(list, vec![rows[0].files[1].clone()], false);
    let rows = list_rows(list_set_source(list.clone(), Source::Standard));
    assert!(!rows[0].included && rows[0].file.ends_with("P1.jpg"));
    let list = list_remove(list, vec![rows[2].file.clone()]);
    assert_eq!(list_rows(list.clone()).len(), 2);
    let elsewhere = dir.path().join("elsewhere");
    std::fs::rename(&folder, &elsewhere).unwrap();
    let moves = vec![FolderMove { old: PathBuf::from(&folder), new: PathBuf::from(&elsewhere) }];
    let moved = list_follow(list, moves);
    assert!(moved.files.iter().all(|f| f.starts_with(&elsewhere)));
    assert_eq!(moved.excluded.len(), 2, "both files of the pair");
}

/// A listing through the add plan: new ones added, New Stack Instead
/// offered once the list had frames, a missing folder said, and nothing
/// added once another stack is on screen or a stack runs.
#[test]
fn the_add_plan_through_the_bindings() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("shoot");
    std::fs::create_dir_all(&folder).unwrap();
    for name in ["P1.jpg", "P2.jpg"] {
        std::fs::write(folder.join(name), b"").unwrap();
    }
    let words = Words::new("en".into());
    // A folder that can't be listed.
    use std::os::unix::fs::PermissionsExt;
    let locked = dir.path().join("locked");
    std::fs::create_dir_all(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let paths = vec![PathBuf::from(&folder), PathBuf::from(&locked)];
    let listed = now(list_expand(req(), paths, words.clone())).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(listed.unreadable.len(), 1, "{listed:?}");
    assert!(listed.unreadable[0].contains("\"locked\""), "{listed:?}");
    assert_eq!(listed.files.len(), 2);
    let (a, b) = (StackId::new().as_str().to_owned(), StackId::new().as_str().to_owned());
    let plan = |files: &[PathBuf], on: &str, running, list| {
        add_plan(files.to_vec(), a.clone(), on.into(), running, list, words.clone())
    };
    let AddPlan::Add { list, offer: false } =
        plan(&listed.files[..1], &a, false, ListInfo::default())
    else {
        panic!("added to an empty stack, nothing offered")
    };
    let AddPlan::Add { list, offer: true } = plan(&listed.files, &a, false, list) else {
        panic!("added to a stack with frames, New Stack Instead offered")
    };
    assert_eq!(list.files.len(), 2);
    assert!(matches!(plan(&listed.files, &a, false, list.clone()), AddPlan::Nothing { .. }));
    let AddPlan::Refused { message } = plan(&listed.files, &b, false, list.clone()) else {
        panic!("another stack on screen")
    };
    assert!(message.contains("another stack was opened"), "{message}");
    assert!(matches!(plan(&listed.files, &a, true, list), AddPlan::Refused { .. }));
    assert_eq!(words.added_frames_text(2, "shoot".into()), "Added 2 frames to shoot.");
}

/// DNG never stacks with standard images; under LibRaw camera RAW goes
/// with DNG (under Apple's decoder with standard images, tested by
/// `deptheos_core`'s `host_decoder`). Only non-DNG camera RAW has a decoder
/// to choose.
#[test]
fn mixed_kinds_as_the_engine_sees_them() {
    let paths = |names: &[&str]| names.iter().map(PathBuf::from).collect::<Vec<_>>();
    let words = Words::new("en".into());
    let mixes_kinds = |paths: Vec<PathBuf>, decoder| {
        let problem = stack_problem(req(), paths.len() as u32, paths, 0, decoder, words.clone());
        now(problem).unwrap().is_some()
    };
    for decoder in [RawBackend::LibRaw, RawBackend::Apple] {
        assert!(!mixes_kinds(paths(&["a.dng", "b.DNG"]), decoder));
        assert!(!mixes_kinds(paths(&["a.jpg", "b.heic"]), decoder));
        assert!(mixes_kinds(paths(&["a.jpg", "b.dng"]), decoder));
    }
    assert!(mixes_kinds(paths(&["a.ORF", "b.jpg"]), RawBackend::LibRaw));
    assert!(!mixes_kinds(paths(&["a.ORF", "b.dng"]), RawBackend::LibRaw));
    assert!(now(chooses_raw_decoder(req(), paths(&["a.dng", "b.ORF"]))).unwrap());
    assert!(!now(chooses_raw_decoder(req(), paths(&["a.dng", "b.jpg"]))).unwrap());
    let two = paths(&["a.jpg", "b.jpg"]);
    let missing = now(stack_problem(req(), 3, two, 2, RawBackend::LibRaw, words)).unwrap();
    assert_eq!(missing.as_deref(), Some("2 included frames can't be found."));
}

/// The frames the app adds are those the engine reads: by extension, any
/// case, not every image (GIF, WEBP, BMP, PSD).
#[test]
fn frames_are_what_the_engine_reads() {
    for name in ["a.jpg", "a.JPEG", "a.png", "a.tif", "a.TIFF", "a.dng", "/x/y/a.DNG"] {
        assert!(is_frame(name.into()), "{name}");
    }
    for name in ["a.gif", "a.webp", "a.bmp", "a.psd", "a.avif", "a.txt", "a", "jpg", ".jpg"] {
        assert!(!is_frame(name.into()), "{name}");
    }
    let raw = deptheos_core::io::supported_extensions().contains(&"orf");
    assert_eq!(is_frame("a.ORF".into()), raw);
}

#[test]
fn a_batch_plans_each_folder() {
    let dir = tempfile::tempdir().unwrap();
    for (name, count) in [("a", 3), ("b", 1)] {
        let folder = dir.path().join("card").join(name);
        std::fs::create_dir_all(&folder).unwrap();
        for i in 0..count {
            std::fs::write(folder.join(format!("{i}.jpg")), b"").unwrap();
        }
    }
    let parent = dir.path().join("card");
    let inputs: Vec<BatchInput> = now(folders_inside(req(), parent, false))
        .unwrap()
        .into_iter()
        .map(|path| BatchInput::Folder { path })
        .collect();
    let plan = now(plan_batch(
        req(),
        inputs,
        Split::PerFolder,
        Source::Raw,
        RawBackend::LibRaw,
        Words::new("en".into()),
    ))
    .unwrap();
    assert_eq!(plan.jobs.len(), 1);
    assert_eq!(plan.label, "One stack per folder · 1 frame skipped");
    assert_eq!(plan.summary, "2 folders → 1 render, 1 frame skipped, 1 folder with too few frames");
}

#[test]
fn left_out_frames_and_summary_line_are_reported() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let listed = frames(dir.path(), &["a.jpg", "b.jpg", "c.jpg"]);
    let (width, height) = (60, 40);
    let paths: Vec<PathBuf> = listed.iter().map(PathBuf::from).collect();
    let mut stack =
        CompletedStack::srgb(width, height, vec![0; 3 * width as usize * height as usize], paths);
    stack.geometry.left_out =
        vec![deptheos_app::LeftOut { index: 1, reason: "couldn't be aligned".into() }];
    let res = StackResult::new(engine.engine.clone(), engine.engine.present_stack(stack).unwrap());
    let want = vec![LeftOutInfo {
        path: listed[1].clone(),
        reason: "Left out b.jpg (couldn't be aligned)".into(),
    }];
    assert_eq!(res.left_out(), want);
    assert_eq!(engine.words().left_out_line(1), "1 frame left out");
    assert_eq!(engine.words().left_out_line(2), "2 frames left out");

    let saved = now(engine.clone().save_render(listed, res, new_stack_id(), None)).unwrap();
    let reopened = now(engine.clone().load_render(req(), saved.id)).unwrap();
    assert_eq!(reopened.left_out(), want);
}

/// The working version against the render on screen: the base unedited and
/// edited, another render, nothing on screen, and Revert's guard.
#[test]
fn the_render_shown_is_worded_by_the_engine() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let listed = frames(dir.path(), &["a.jpg", "b.jpg", "c.jpg"]);
    let stack = new_stack_id();
    let first = save(&engine, &listed, &listed, &stack);
    let second = save(&engine, &listed, &listed[..2], &stack);
    let working = now(engine.clone().working_of_render(req(), second.id.clone())).unwrap();
    let show = |working: &WorkingInfo, shown: Option<&RenderInfo>| {
        showing(&engine, working.clone(), shown.map(|r| r.id.clone()))
    };

    // #2, the base, unedited: nothing to say, no Revert.
    let base = show(&working, Some(&second));
    assert_eq!((base.kind, base.shown, base.base), (ShowingKind::Base, Some(2), Some(2)));
    assert!(!base.edited);
    assert_eq!((base.change_line, base.base_tile_line, base.revert), (None, None, None));
    assert_eq!(base.log, "#2 (base)");

    // #1 shown: how the working version differs, and a Revert that loses
    // nothing.
    let other = show(&working, Some(&first));
    assert_eq!((other.kind, other.shown, other.base), (ShowingKind::Other, Some(1), Some(2)));
    assert_eq!(other.change_line.as_deref(), Some("1 difference from #1 in the frames"));
    let revert = other.revert.unwrap();
    assert_eq!((revert.render.as_str(), revert.number), (first.id.as_str(), 1));
    assert_eq!(revert.alert, None);

    // Edited since #2: the base's tile and line say so, and Revert asks.
    let edited = WorkingInfo {
        list: list_set_included(working.list.clone(), vec![listed[0].clone()], false),
        ..working.clone()
    };
    let base = show(&edited, Some(&second));
    assert!(base.edited);
    assert_eq!(base.change_line.as_deref(), Some("1 change to the frames since #2"));
    assert_eq!(base.base_tile_line.as_deref(), Some("1 change"));
    let alert = base.revert.unwrap().alert.unwrap();
    assert_eq!(alert.title, "Revert to #2?");
    assert_eq!(
        alert.body,
        "Your 1 change since #2 hasn't been stacked, and will be lost. Every render is kept."
    );
    let other = show(&edited, Some(&first));
    assert_eq!(
        other.base_tile_line.as_deref(),
        Some("1 change"),
        "on the base's tile in every state"
    );
    assert_eq!(other.revert.unwrap().alert.unwrap().title, "Revert to #1?");

    // All at once, as the title and Renders row ask: each as if shown.
    let ids = vec![first.id.clone(), second.id.clone()];
    let asked =
        engine.clone().showings(req(), edited.clone(), Some(second.id.clone()), ids.clone());
    let all = now(asked).unwrap();
    assert_eq!(all.shown, show(&edited, Some(&second)));
    assert_eq!(all.renders[&first.id], show(&edited, Some(&first)));
    assert_eq!(all.renders[&second.id], all.shown);

    // Nothing on screen, or a render deleted since: nothing to say.
    let nothing = show(&edited, None);
    assert_eq!(
        (nothing.kind, nothing.change_line, nothing.revert),
        (ShowingKind::Nothing, None, None)
    );
    now(engine.clone().delete_renders(vec![first.id.clone()], None)).unwrap();
    assert_eq!(show(&edited, Some(&first)).kind, ShowingKind::Nothing);
    let all = now(engine.clone().showings(req(), edited.clone(), None, ids)).unwrap();
    assert_eq!(all.renders.keys().collect::<Vec<_>>(), [&second.id], "a deleted render left out");
    // Asked again, still none: what is known missing isn't reread, and a
    // listing forgets it.
    assert_eq!(show(&edited, Some(&first)).kind, ShowingKind::Nothing);
    now(engine.clone().list_renders(req())).unwrap();
    assert_eq!(show(&edited, Some(&first)).kind, ShowingKind::Nothing);
    assert_eq!(showing(&engine, edited, Some("not an id".into())).kind, ShowingKind::Nothing);
}

/// Render Info: a render's record by section, with the working version's
/// value under differing rows, and the line under its thumbnail.
#[test]
fn render_info_is_the_engines_record() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let listed = frames(dir.path(), &["a.jpg", "b.jpg", "c.jpg"]);
    let render = save(&engine, &listed, &listed[..2], &new_stack_id());
    let mut working = now(engine.clone().working_of_render(req(), render.id.clone())).unwrap();
    if let Some(settings) = &mut working.settings {
        settings.algorithm = Algorithm::Pyramid { smoothness: 6.0 };
    }
    let sections = now(engine.clone().render_setup(req(), render.id.clone(), working)).unwrap();
    let parts: Vec<_> = sections.iter().map(|s| (s.part, s.title.as_str())).collect();
    assert_eq!(
        parts,
        [
            (SetupPart::Method, "Method"),
            (SetupPart::Alignment, "Alignment"),
            (SetupPart::Frames, "Frames")
        ]
    );
    let smoothness = &sections[0].rows[1];
    assert_eq!((smoothness.label.as_str(), smoothness.value.as_str()), ("Smoothness", "4"));
    assert!(smoothness.differs);
    assert_eq!(smoothness.working.as_deref(), Some("Working version: 6"));
    let frames = &sections[2].rows;
    assert_eq!((frames[0].label.as_str(), frames[0].value.as_str()), ("Included", "2 of 3"));
    let c = frames.iter().find(|r| r.label == "c.jpg").unwrap();
    assert_eq!((c.value.as_str(), c.differs), ("Excluded", false));
    assert_eq!(c.file.as_deref(), Some(listed[2].as_path()));
    let working = now(engine.clone().read_working(req())).unwrap();
    assert!(now(engine.clone().render_setup(req(), "not an id".into(), working)).is_err());

    let words = Words::new("en".into());
    assert_eq!(
        words.stacked_at_line("Nov 14, 2026 at 10:42".into()),
        "Stacked Nov 14, 2026 at 10:42"
    );
}

/// The shared preferences: the desktop's defaults, ranges and words, and
/// values read back clamped.
#[test]
fn the_shared_preferences_through_the_bindings() {
    let words = Words::new("en".into());
    let defaults = default_preferences();
    assert_eq!(defaults.export.file_name_pattern, "{stack}-{render}");
    assert_eq!(defaults.export.jpeg_quality, deptheos_app::JPEG_QUALITY);
    assert_eq!(defaults.export.tiff_compression, TiffCompression::Zip);
    assert_eq!(defaults.alignment.grid_horizontal_pct, 5.0);
    assert!(!defaults.alignment.exposure_compensation);
    assert_eq!(sanitized_preferences(defaults.clone()), defaults);

    let quality = preference_range(Preference::JpegQuality).unwrap();
    assert_eq!((quality.min, quality.max, quality.step), (1.0, 100.0, 1.0));
    assert_eq!(quality.default_value, f32::from(defaults.export.jpeg_quality));
    let rotation = preference_range(Preference::GridRotation).unwrap();
    assert_eq!(rotation.default_value, defaults.alignment.grid_rotation_deg);
    assert!(preference_range(Preference::TiffCompression).is_none());
    assert!(preference_range(Preference::ExposureCompensation).is_none());

    assert_eq!(words.preference_label(Preference::GridHorizontal), "Horizontal shift (%)");
    assert_eq!(words.preference_help(Preference::GridVertical), "Of the frame's longer side.");
    assert_eq!(words.grid_limits_label(), "Grid search limits");
    assert!(words.grid_limits_help().starts_with("How far Grid search looks"));
    assert_eq!(words.tiff_compression_label(TiffCompression::None), "None");
    assert_eq!(
        words.export_file_name_example(
            defaults.export.clone(),
            ExportFormat::Tiff,
            "2026-11-14".into()
        ),
        "For example: bee-4.tif"
    );

    let mut odd = defaults.clone();
    odd.export.jpeg_quality = 0;
    odd.alignment.grid_rotation_deg = f32::NAN;
    odd.alignment.grid_magnification_pct = 99.0;
    let clamped = sanitized_preferences(odd);
    assert_eq!(clamped.export.jpeg_quality, 1);
    assert_eq!(clamped.alignment.grid_rotation_deg, 0.0);
    let magnification = preference_range(Preference::GridMagnification).unwrap();
    assert_eq!(clamped.alignment.grid_magnification_pct, magnification.max);
}

/// A stack takes Grid search's limits from the preferences, recorded with
/// its options; they never make a working version edited.
#[test]
fn a_stack_takes_the_alignment_preferences() {
    let settings = StackSettings {
        algorithm: Algorithm::Pyramid { smoothness: 4.0 },
        align: true,
        align_method: AlignMode::Grid,
        crop: true,
        raw_decoder: RawBackend::LibRaw,
    };
    let alignment = AlignmentPreferences {
        grid_horizontal_pct: 12.0,
        grid_rotation_deg: 40.0,
        exposure_compensation: true,
        ..default_preferences().alignment
    };
    let options = stack_options(&settings, alignment);
    let deptheos_app::Alignment::On(deptheos_app::AlignSettings {
        mode: AlignMode::Grid,
        grid,
        ..
    }) = options.alignment
    else {
        panic!("{:?}", options.alignment);
    };
    assert_eq!(grid.horizontal_pct, 12.0);
    assert_eq!(grid.rotation_deg, 10.0, "clamped into its range");
    assert!(grid.apply_brightness);
    // Against the defaults' options only what the Settings window sets
    // differs: the aligner, crop and decoder a working version counts are
    // the same.
    let plain = stack_options(&settings, default_preferences().alignment);
    assert_eq!(options.alignment.mode(), plain.alignment.mode());
    assert_eq!(options.alignment.crops(), plain.alignment.crops());
    assert_eq!(options.raw_backend, RawBackend::LibRaw);
    assert_eq!(options.raw_backend, plain.raw_backend);
}

/// Align Frames off keeps the aligner, crop and reverse check, so turning
/// it on after an engine round trip gives them back.
#[test]
fn alignment_off_keeps_what_it_resumes_with() {
    let settings = StackSettings {
        algorithm: Algorithm::Pyramid { smoothness: 4.0 },
        align: false,
        align_method: AlignMode::Grid,
        crop: false,
        raw_decoder: RawBackend::Apple,
    };
    let options = settings.options();
    assert!(!options.alignment.is_on() && !options.alignment.crops());
    let back = StackSettings::of(&settings.algorithm.config(), &options);
    assert_eq!((back.align, back.align_method, back.crop), (false, AlignMode::Grid, false));
}

/// An export names its file by the pattern with the result's method, and
/// writes TIFF with the compression chosen.
#[test]
fn an_export_follows_the_export_preferences() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let stacked = frames(dir.path(), &["a.jpg", "b.jpg"]);
    let result = result(&engine, &stacked, SessionOptions::default());
    let stem = result.export_file_stem(
        "{date} {stack} {method} {render}".into(),
        "spider".into(),
        Some(3),
        "2026-11-14".into(),
    );
    assert_eq!(stem, "2026-11-14 spider Pyramid 3");
    let out = PathBuf::from(dir.path());
    let mut preferences = default_preferences().export;
    let size = |preferences: &ExportPreferences, name: &str| {
        let path = now(result.clone().export(
            req(),
            out.clone(),
            name.into(),
            ExportFormat::Tiff,
            preferences.clone(),
        ))
        .unwrap();
        assert!(path.ends_with(format!("{name}.tif")), "{}", path.display());
        std::fs::metadata(path).unwrap().len()
    };
    preferences.tiff_compression = TiffCompression::None;
    let none = size(&preferences, "none");
    preferences.tiff_compression = TiffCompression::Zip;
    let zip = size(&preferences, "zip");
    assert!(zip < none, "ZIP {zip} bytes, uncompressed {none}");
}

/// Each frame's share and the frame at a point, from the frame map, a
/// left-out frame in its place; none without a map.
#[test]
fn frame_contributions_and_the_frame_at_a_point() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let stacked: Vec<PathBuf> = ["a.png", "b.png", "c.png"].map(PathBuf::from).to_vec();
    let bare = result(&engine, &stacked, SessionOptions::default());
    assert_eq!(bare.frame_contributions(), None, "no frame map, nothing to count");
    assert_eq!(bare.frame_at(1.5, 1.5), None);

    // Four pixels: three from a.png (plane 0), one from c.png (plane 1),
    // b.png left out.
    let (width, height) = (2, 2);
    let mut stack = CompletedStack {
        frame_map: Some(deptheos_app::Gray16Image::from_raw(2, 2, vec![0, 0, 0, 65535]).unwrap()),
        ..CompletedStack::srgb(
            width,
            height,
            vec![0; 12],
            stacked.iter().map(PathBuf::from).collect(),
        )
    };
    stack.geometry.left_out = vec![deptheos_app::LeftOut { index: 1, reason: "blurry".into() }];
    let result = StackResult::pixels_only(engine.engine.clone(), stack);
    let shares = result.frame_contributions().unwrap();
    let row = |path: &str, share: f32, left_out: bool| FrameContribution {
        path: path.into(),
        share,
        left_out,
    };
    assert_eq!(
        shares,
        [row("a.png", 0.75, false), row("b.png", 0.0, true), row("c.png", 0.25, false)]
    );
    assert_eq!(result.frame_at(0.5, 0.5).as_deref(), Some(Path::new("a.png")));
    assert_eq!(result.frame_at(1.5, 1.5).as_deref(), Some(Path::new("c.png")));
    assert_eq!(result.frame_at(2.0, 0.0), None);
    assert_eq!(result.frame_at(-0.5, 0.0), None);
}

/// The depth map in legend colors for every method, lit for one frame, and
/// each frame's color; none without a frame map.
#[test]
fn the_depth_map_is_lit_frame_by_frame() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let stacked = frames(dir.path(), &["a.png", "b.png", "c.png"]);
    let [a_png, b_png, c_png] = [0, 1, 2].map(|i| stacked[i].clone());
    let bare = result(&engine, &stacked, SessionOptions::default());
    assert!(
        bare.picture_of(&PictureOf::Map).unwrap().is_none(),
        "a pyramid stack without a frame map has no map"
    );
    assert!(bare.picture_of(&PictureOf::LitMap { frame: a_png.clone() }).unwrap().is_none());

    // The left half from a.png (plane 0), the right from c.png (plane 1),
    // b.png left out.
    let (width, height) = (8, 8);
    let mut stack = CompletedStack {
        frame_map: Some(deptheos_app::Gray16Image::from_fn(width, height, |x, _| {
            image::Luma([if x < width / 2 { 0 } else { 65535 }])
        })),
        ..CompletedStack::srgb(
            width,
            height,
            vec![0; (width * height * 3) as usize],
            stacked.iter().map(PathBuf::from).collect(),
        )
    };
    stack.geometry.left_out = vec![deptheos_app::LeftOut { index: 1, reason: "blurry".into() }];
    let result =
        StackResult::new(engine.engine.clone(), engine.engine.present_stack(stack).unwrap());

    let colors = result.frame_colors();
    assert_eq!(colors.len(), 3, "one per frame, in the order given");
    assert_eq!(colors[1], None, "a frame left out has no color");
    let (a, c) = (colors[0].unwrap(), colors[2].unwrap());
    assert_ne!(a, c, "each stacked frame its own color");

    // A pixel of the map (BGRA) as an sRGB color.
    let at = |image: &DisplayImage, x: u32| {
        let i = (x * 4) as usize;
        let p = &image.bgra[i..i + 4];
        LegendColor { red: p[2], green: p[1], blue: p[0] }
    };
    let whole = result.picture_of(&PictureOf::Map).unwrap().unwrap().base(64).unwrap();
    let right = whole.width - 1;
    assert_eq!((at(&whole, 0), at(&whole, right)), (a, c), "the map is in the legend's colors");

    let lit = result
        .picture_of(&PictureOf::LitMap { frame: a_png.clone() })
        .unwrap()
        .unwrap()
        .base(64)
        .unwrap();
    assert_eq!((lit.width, lit.height), (whole.width, whole.height));
    assert_eq!(at(&lit, 0), a, "what a.png supplied keeps its color");
    let dim = at(&lit, right);
    assert!(dim.red == dim.green && dim.green == dim.blue, "the rest is gray: {dim:?}");
    assert!(dim.red < c.red.max(c.green).max(c.blue), "and dimmed");

    assert!(
        result.picture_of(&PictureOf::LitMap { frame: b_png.clone() }).unwrap().is_none(),
        "a frame left out lights nothing"
    );
    assert!(
        result.picture_of(&PictureOf::LitMap { frame: "elsewhere.png".into() }).unwrap().is_none()
    );

    // Saved, it keeps the display it thumbnailed and lets go of the map
    // before the encode; the map is remade, the same, when asked.
    now(engine.clone().save_render(stacked.clone(), result.clone(), new_stack_id(), None)).unwrap();
    assert!(result.held(false).is_some() && result.held(true).is_none());
    assert!(
        result.picture_of(&PictureOf::Map).unwrap().unwrap().base(64).unwrap().bgra == whole.bgra
    );

    // The lit map is made from the stack, so it outlives the displays.
    result.release_displays();
    assert!(result.held(true).is_none());
    assert!(result.picture_of(&PictureOf::LitMap { frame: c_png.clone() }).unwrap().is_some());
}

/// A result is shown at the size asked whatever its width: a level just
/// over twice that is scaled down to it, not halved past it.
#[test]
fn a_wide_result_is_shown_at_the_size_asked_for() {
    for (width, height) in [(8192, 64), (8256, 64), (9504, 64), (3000, 64)] {
        let pixels = vec![128u8; width as usize * height as usize * 3];
        let presented = deptheos_app::engine::present::present_rgb(&pixels, width, height, |v| v);
        let shown = level_image(&presented, 4096).unwrap();
        let long = width.min(4096);
        assert_eq!(shown.width, long, "{width} px wide");
        let expected = (height * long + width / 2) / width;
        assert_eq!(shown.height, expected.max(1), "{width} px wide");
        assert_eq!(shown.bgra.len(), (shown.width * shown.height * 4) as usize);
        assert!(shown.bgra.chunks(4).all(|p| p == [128, 128, 128, 255]), "{width} px wide");
    }
}

/// The depth map, whose levels start at half size, is shown at the size
/// asked too.
#[test]
fn a_wide_depth_map_is_shown_at_the_size_asked_for() {
    for width in [8192, 8256, 9504] {
        let map = image::ImageBuffer::from_pixel(width, 64, image::Luma([7u16]));
        let presented = deptheos_app::engine::present::present_frame_map(&map, None);
        let shown = level_image(&presented, 2048).unwrap();
        assert_eq!(shown.width, 2048, "{width} px wide");
    }
}

/// A picked folder is a root with Swift's bookmark; Locate… says how many
/// frames the new pick has, keeps its bookmark, and renames every path to
/// where it is now; Settings lists the folder with its stacks.
#[test]
fn a_folder_picked_is_a_root_with_its_bookmark() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let listed = frames(dir.path(), &["a.jpg", "b.jpg"]);
    let shoot = dir.path().join("shoot");
    let kept = now(engine.clone().keep_folder(shoot.clone(), b"one".to_vec())).unwrap();
    assert!(kept.is_empty());
    let root = now(engine.clone().roots_of(req(), listed.clone())).unwrap().remove(0).unwrap();
    assert_eq!(root.folder, shoot);
    assert_eq!(root.bookmark.as_deref(), Some(&b"one"[..]));
    assert_eq!(now(engine.clone().bookmarked_roots(req())).unwrap(), [root]);
    assert!(
        now(engine.clone().read_folders(req())).unwrap().is_empty(),
        "a folder no stack uses isn't listed"
    );
    assert!(now(engine.clone().read_folder_roots(req())).unwrap().is_empty());
    let stack = new_stack_id();
    save(&engine, &listed, &listed, &stack);
    assert_eq!(
        now(engine.clone().reach(req(), listed.clone())).unwrap(),
        [Reach::There, Reach::There]
    );
    let folders = now(engine.clone().read_folders(req())).unwrap();
    assert_eq!(folders.len(), 1);
    assert_eq!((folders[0].reach, folders[0].stacks), (Reach::There, 1), "its render's");
    let roots = now(engine.clone().read_folder_roots(req())).unwrap();
    assert_eq!(roots, [folders[0].root.clone()], "what is leased first");

    let bee = dir.path().join("Bee");
    std::fs::rename(dir.path().join("shoot"), &bee).unwrap();
    std::fs::remove_file(bee.join("b.jpg")).unwrap();
    assert_eq!(
        now(engine.clone().reach(req(), listed.clone())).unwrap(),
        [Reach::Missing, Reach::Missing]
    );
    let located =
        now(engine.clone().locate(shoot.clone(), PathBuf::from(&bee), Some(b"two".to_vec())))
            .unwrap();
    assert_eq!((located.found, located.of), (1, 2));
    assert_eq!(located.moved, [FolderMove { old: shoot, new: PathBuf::from(&bee) }]);
    assert_eq!(engine.relinks(), 1);
    let moved = engine.paths_now(listed.clone());
    assert_eq!(moved, [bee.join("a.jpg"), bee.join("b.jpg")]);
    assert_eq!(now(engine.clone().list_renders(req())).unwrap()[0].frames, moved);
    let roots = now(engine.clone().roots_of(req(), moved.clone())).unwrap();
    assert_eq!(roots[0].as_ref().unwrap().bookmark.as_deref(), Some(&b"two"[..]));
    assert_eq!(now(engine.clone().reach(req(), moved)).unwrap(), [Reach::There, Reach::Missing]);
}

/// At launch a moved root is found where its bookmark led (Swift's
/// resolving, handed back), and the renders follow.
#[test]
fn a_root_is_found_again_where_its_bookmark_led() {
    let dir = tempfile::tempdir().unwrap();
    let listed = frames(dir.path(), &["a.jpg"]);
    let engine = engine(dir.path());
    now(engine.clone().keep_folder(dir.path().join("shoot"), b"m".to_vec())).unwrap();
    let root = now(engine.clone().bookmarked_roots(req())).unwrap().remove(0).root;
    save(&engine, &listed, &listed, &new_stack_id());
    drop(engine);
    let bee = dir.path().join("Bee");
    std::fs::rename(dir.path().join("shoot"), &bee).unwrap();

    let engine = self::engine(dir.path());
    now(engine.clone().list_renders(req())).unwrap();
    assert_eq!(now(engine.clone().reach(req(), listed.clone())).unwrap(), [Reach::Missing]);
    let led = BookmarkResolution { root, path: PathBuf::from(&bee), fresh: Some(b"n".to_vec()) };
    let found = now(engine.clone().find_again(vec![led])).unwrap();
    assert_eq!(found.len(), 1);
    let canon = std::fs::canonicalize(&bee).unwrap();
    assert_eq!(now(engine.clone().list_renders(req())).unwrap()[0].frames, [canon.join("a.jpg")]);
    assert_eq!(
        now(engine.clone().reach(req(), vec![canon.join("a.jpg")])).unwrap(),
        [Reach::There]
    );
}

/// Copy into Deptheos points the folder at its copy only when the copy
/// holds every frame used from it unchanged; else nothing moves and the
/// missing frames are named.
#[test]
fn a_copy_into_the_app_is_taken_only_whole() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let listed = frames(dir.path(), &["a.jpg", "b.jpg"]);
    save(&engine, &listed, &listed, &new_stack_id());
    let shoot = dir.path().join("shoot");
    assert_eq!(
        now(engine.clone().frames_to_copy(req(), shoot.clone())).unwrap(),
        ["a.jpg", "b.jpg"]
    );

    let copy = dir.path().join("Imports/shoot");
    std::fs::create_dir_all(&copy).unwrap();
    std::fs::copy(&listed[0], copy.join("a.jpg")).unwrap();
    let lacking =
        message(now(engine.clone().take_copy(shoot.clone(), PathBuf::from(&copy))).unwrap_err());
    assert!(lacking.contains("\"b.jpg\"") && !lacking.contains("a.jpg"), "{lacking}");
    assert_eq!(now(engine.clone().list_renders(req())).unwrap()[0].frames, listed);

    std::fs::copy(&listed[1], copy.join("b.jpg")).unwrap();
    let moved = now(engine.clone().take_copy(shoot.clone(), PathBuf::from(&copy))).unwrap();
    assert_eq!(moved, [FolderMove { old: shoot, new: PathBuf::from(&copy) }]);
    let copies = [copy.join("a.jpg"), copy.join("b.jpg")];
    assert_eq!(now(engine.clone().list_renders(req())).unwrap()[0].frames, copies);
}

// ---------------------------------------------------------------- async calls

/// A stack's progress and live maps as the observer hears them, in order.
#[derive(Default)]
struct Heard {
    progress: Mutex<Vec<StackProgress>>,
    maps: std::sync::atomic::AtomicUsize,
    /// Told once the first frame is accumulated.
    aligning: Mutex<Option<std::sync::mpsc::Sender<()>>>,
}

impl StackObserver for Heard {
    fn progress(&self, progress: StackProgress) {
        let phase = progress.phase;
        self.progress.lock().unwrap().push(progress);
        if phase == StackPhase::Aligning
            && let Some(tx) = self.aligning.lock().unwrap().take()
        {
            let _ = tx.send(());
        }
    }

    fn live_map(&self, _image: DisplayImage) {
        self.maps.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

/// `count` textured frames of the same noise, so alignment has something
/// to hold.
fn textured(dir: &Path, count: usize) -> Vec<PathBuf> {
    (0..count)
        .map(|i| {
            let path = dir.join(format!("t{i}.png"));
            image::RgbImage::from_fn(320, 240, |x, y| {
                let v = ((x * 7919 + y * 104_729) % 251) as u8;
                image::Rgb([v, v.wrapping_add(i as u8 * 9), 255 - v])
            })
            .save(&path)
            .unwrap();
            PathBuf::from(&path)
        })
        .collect()
}

fn stack_settings() -> StackSettings {
    StackSettings {
        algorithm: Algorithm::Pyramid { smoothness: 4.0 },
        align: false,
        align_method: AlignMode::Ecc,
        crop: false,
        raw_decoder: RawBackend::Apple,
    }
}

/// A stack through the bridge runs on its own thread and returns to the
/// awaiting call; its observer heard every phase in order, Completed before
/// the call returned.
#[test]
fn a_stack_is_awaited_and_its_final_progress_is_heard() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let paths = textured(dir.path(), 4);
    let heard = Arc::new(Heard::default());
    let alignment = default_preferences().alignment;
    let stacking =
        engine.stack(paths.clone(), stack_settings(), alignment, 64, req(), heard.clone());
    let result = now(stacking).unwrap();
    assert_eq!(result.frames(), paths);
    let phases: Vec<_> = heard.progress.lock().unwrap().iter().map(|p| p.phase).collect();
    assert_eq!(phases.first(), Some(&StackPhase::Started));
    assert_eq!(phases.last(), Some(&StackPhase::Completed), "{phases:?}");
}

/// A Depth stack's progress fills once: its blend continues from where
/// taking frames in left off, never back to 0, and every frame is taken in
/// from finalizing on.
#[test]
fn a_two_pass_stacks_progress_never_goes_back() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let paths = textured(dir.path(), 4);
    let heard = Arc::new(Heard::default());
    let settings = StackSettings {
        algorithm: Algorithm::Depth { radius: 8.0, smoothing: 2.5, blend_range: 3.0 },
        ..stack_settings()
    };
    let alignment = default_preferences().alignment;
    let stacking = engine.stack(paths, settings, alignment, 64, req(), heard.clone());
    now(stacking).unwrap();
    let progress = heard.progress.lock().unwrap().clone();
    assert!(progress.iter().any(|p| p.phase == StackPhase::Blending), "{progress:?}");
    for pair in progress.windows(2) {
        assert!(pair[1].fraction >= pair[0].fraction, "back: {pair:?}");
    }
    for p in progress.iter().filter(|p| p.phase == StackPhase::Blending) {
        assert!(p.fraction >= 0.5 && p.taken == 4, "{p:?}");
    }
    assert_eq!(progress.last().map(|p| p.fraction), Some(1.0));
}

/// A stack's request canceled mid-run ends it at its next frame: the call
/// says Canceled promptly, and the engine is free.
#[test]
fn a_canceled_request_ends_its_stack_promptly() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let paths = textured(dir.path(), 24);
    let (tx, aligning) = std::sync::mpsc::channel();
    let heard = Arc::new(Heard { aligning: Mutex::new(Some(tx)), ..Heard::default() });
    let request = req();
    let canceler = {
        let request = request.clone();
        std::thread::spawn(move || {
            aligning.recv().unwrap();
            request.cancel();
            std::time::Instant::now()
        })
    };
    let alignment = default_preferences().alignment;
    let stacking = engine.stack(paths, stack_settings(), alignment, 64, request, heard);
    let ended = now(stacking);
    let canceled_at = canceler.join().unwrap();
    assert!(matches!(ended, Err(EngineError::Canceled)), "{:?}", ended.err());
    assert!(canceled_at.elapsed() < std::time::Duration::from_secs(5), "ended promptly");
}

/// Work held until its token is canceled: says it started, then waits.
fn held(started: std::sync::mpsc::Sender<()>) -> impl FnOnce() -> Result<()> + Send + 'static {
    move || {
        started.send(()).unwrap();
        let token = work::current();
        let words = AppWords::english(Platform::Ios);
        while !token.is_canceled() {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        token.check().told(&words)
    }
}

/// Canceled before its work starts, the work never runs; canceled as it
/// runs, the work sees it at once. Either way the call says Canceled.
#[test]
fn a_canceled_request_never_runs_or_stops_its_work() {
    let words = AppWords::english(Platform::Ios);
    let ask = || request::Ask::Own("test-held");
    let early = req();
    early.cancel();
    let ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let call = request::on_pool(&words, &early, Lane::Interactive, ask(), {
        let ran = ran.clone();
        move || {
            ran.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    });
    assert!(matches!(now(call), Err(EngineError::Canceled)));
    assert!(!ran.load(std::sync::atomic::Ordering::SeqCst), "canceled before it ran");

    let request = req();
    let (tx, started) = std::sync::mpsc::channel();
    let call = request::on_pool(&words, &request, Lane::Interactive, ask(), held(tx));
    let canceler = {
        let request = request.clone();
        std::thread::spawn(move || {
            started.recv().unwrap();
            request.cancel();
        })
    };
    assert!(matches!(now(call), Err(EngineError::Canceled)));
    canceler.join().unwrap();
}

/// A call let go of before its work ends (UniFFI frees a future whose Swift
/// task went away) cancels the work.
#[test]
fn a_dropped_call_cancels_its_work() {
    let words = AppWords::english(Platform::Ios);
    let request = req();
    let (tx, started) = std::sync::mpsc::channel();
    let (done_tx, done) = std::sync::mpsc::channel();
    let work = held(tx);
    let mut call = Box::pin(request::on_pool(
        &words,
        &request,
        Lane::Interactive,
        request::Ask::Own("test-dropped"),
        move || {
            let ended = work();
            done_tx.send(ended.is_err()).unwrap();
            ended
        },
    ));
    // Polled once, so the work is queued, then dropped while it runs.
    let waker = futures::task::noop_waker();
    let _ = call.as_mut().poll(&mut std::task::Context::from_waker(&waker));
    started.recv().unwrap();
    drop(call);
    let canceled = done.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    assert!(canceled, "the work saw its token canceled");
}

/// A Library write is never canceled: it lands in the order sent, and the
/// awaited store says everything sent is on disk.
#[test]
fn writes_land_in_order_and_are_awaited() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let listed = frames(dir.path(), &["a.jpg", "b.jpg"]);
    let stack = new_stack_id();
    let render = save(&engine, &listed, &listed, &stack);
    let first = engine.clone().rename_render(render.id.clone(), Some("One".into()));
    let second = engine.clone().rename_render(render.id.clone(), Some("Two".into()));
    let (first, second) = now(async { futures::join!(first, second) });
    first.unwrap();
    second.unwrap();
    assert!(now(engine.clone().written(5_000)).unwrap());
    let renders = now(engine.clone().list_renders(req())).unwrap();
    assert_eq!(renders[0].name.as_deref(), Some("Two"));
}

/// How `working` stands against `shown` alone.
fn showing(engine: &Arc<Engine>, working: WorkingInfo, shown: Option<String>) -> ShowingInfo {
    now(engine.clone().showings(req(), working, shown, Vec::new())).unwrap().shown
}
