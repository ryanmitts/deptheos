//! The bundled sample stacks (`deptheos_app::frames::sample`, a
//! [`SampleStack`] each): Try a Sample Stack (welcome menu, Help submenu)
//! copies one into the Library folder's Samples by the engine's rule, as on
//! iOS, and adds its frames to a new stack, unstacked, so the newcomer sees
//! a frame and presses Stack.
//!
//! Release bundles carry the frames (script/bundle-mac.sh into
//! Contents/Resources, packaging/windows/deptheos.iss and the portable zip
//! beside the exe, script/bundle-linux.sh and packaging/linux/nfpm.yaml in
//! share/deptheos); a repository build finds them there. A build with none
//! links to the website's samples.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

pub use deptheos_app::frames::sample::{Sample, SampleStack};
use deptheos_app::library::history::StackId;
use gpui::{App, Context, Task, Window};

use super::AppView;
use super::list::AddedFrom;
use crate::notices::{Kind, Subject};
use crate::words::{DesktopWords, Wording as _, say};

/// The website's sample page ("A stack to try"), opened by a build without
/// samples.
pub(super) const SAMPLE_PAGE: &str = "/docs/getting-started/#samples";

/// The Library folder's subfolder for the copies, one per sample by name.
const COPIES_FOLDER: &str = "Samples";

/// The welcome menu's line under `stack`'s name: its frames and how shot.
pub(crate) fn line(stack: SampleStack, w: &DesktopWords, frames: usize) -> String {
    match stack.handheld() {
        true => say!(w, "welcome-sample-handheld", count = frames),
        false => say!(w, "welcome-sample-tripod", count = frames),
    }
}

/// The build's samples in [`SampleStack::ALL`]'s order, looked for once.
pub(crate) fn bundled() -> Vec<Arc<Sample>> {
    static FOUND: OnceLock<Vec<Arc<Sample>>> = OnceLock::new();
    FOUND
        .get_or_init(|| {
            let exe = std::env::current_exe().ok();
            let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
            let exe_dir = exe.as_deref().and_then(Path::parent);
            let found = find_all(exe_dir, &repo);
            for sample in &found {
                tracing::info!(
                    "the sample stack {} is at {} ({} frames)",
                    sample.stack().name(),
                    sample.dir().display(),
                    sample.frame_count()
                );
            }
            if found.is_empty() {
                tracing::info!("this build carries no sample stack");
            }
            found.into_iter().map(Arc::new).collect()
        })
        .clone()
}

/// Each sample stack found from the exe's folder or the repository.
fn find_all(exe_dir: Option<&Path>, repo: &Path) -> Vec<Sample> {
    SampleStack::ALL
        .into_iter()
        .filter_map(|stack| Sample::find(stack, &places(exe_dir, repo, stack)))
        .collect()
}

/// Where `stack` may be, in order: a Mac app's Contents/Resources/samples,
/// Linux's share/deptheos/samples beside bin (also inside AppImage and
/// Flatpak trees), beside the exe on Windows, then the repository's.
fn places(exe_dir: Option<&Path>, repo: &Path, stack: SampleStack) -> Vec<PathBuf> {
    let folder = stack.bundled_folder();
    let mut places = Vec::new();
    if let Some(dir) = exe_dir {
        places.push(dir.join("../Resources/samples").join(folder));
        places.push(dir.join("../share/deptheos/samples").join(folder));
        places.push(dir.join("samples").join(folder));
    }
    places.push(repo.join("samples").join(folder));
    places
}

/// Trying a sample stack, until its frames are in their stack.
///
/// Lifecycle: `Idle`, or `Copying` a [`SampleStack`] into Samples by the
/// engine's `frames::sample` rule; changed only by
/// [`AppView::set_sample_open`]. `Idle` → `Copying` when a sample is tried,
/// a new empty stack on screen. `Copying` → `Idle` when the copy lands (its
/// frames added if that stack is still on screen, empty and idle, else
/// dropped, the copy kept) or fails (a notice says why). One at a time.
pub(super) enum SampleOpen {
    Idle,
    /// `sample`'s frames are being copied for `stack`.
    Copying {
        stack: StackId,
        sample: SampleStack,
        _task: Task<()>,
    },
}

impl SampleOpen {
    /// The sample being copied, if one is.
    pub(super) fn copying(&self) -> Option<SampleStack> {
        match self {
            Self::Idle => None,
            Self::Copying { sample, .. } => Some(*sample),
        }
    }
}

impl std::fmt::Display for SampleOpen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Idle => f.write_str("idle"),
            Self::Copying { sample, .. } => write!(f, "copying {}", sample.name()),
        }
    }
}

impl AppView {
    pub(super) fn set_sample_open(&mut self, open: SampleOpen, cx: &mut Context<Self>) {
        tracing::debug!("sample stack: {} -> {open}", self.sample_open);
        self.sample_open = open;
        cx.notify();
    }

    /// The sample stacks this build carries, in the order offered.
    pub(super) fn sample_stacks(&self) -> Vec<SampleStack> {
        self.samples.iter().map(|s| s.stack()).collect()
    }

    /// Try a Sample Stack would act: it opens the website without samples,
    /// else waits for any run, as New Stack does, and for a sample in flight.
    pub(super) fn can_try_sample(&self, cx: &App) -> bool {
        self.samples.is_empty()
            || (!self.is_processing(cx) && matches!(self.sample_open, SampleOpen::Idle))
    }

    /// Try a Sample Stack: `which` sample's frames copied into the Library
    /// folder, in a new stack (New Stack first, keeping the current one's
    /// working version). `None`: the website's samples.
    pub(crate) fn try_sample(
        &mut self,
        which: Option<SampleStack>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let carried = which.and_then(|w| self.samples.iter().find(|s| s.stack() == w).cloned());
        let Some(sample) = carried else {
            if self.samples.is_empty() {
                cx.open_url(&crate::updates::site_page(SAMPLE_PAGE));
            } else {
                tracing::debug!("sample stack: {which:?} isn't in this build");
            }
            return;
        };
        if !self.can_try_sample(cx) {
            tracing::debug!("sample stack: not now ({})", self.sample_open);
            return;
        }
        self.new_stack_then(window, cx, move |this, window, cx| {
            this.copy_sample(sample, window, cx)
        });
    }

    /// Try a Sample Stack, its new stack on screen: copy `sample` for it.
    fn copy_sample(&mut self, sample: Arc<Sample>, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_try_sample(cx) {
            tracing::debug!("sample stack: not now ({})", self.sample_open);
            return;
        }
        if self.screen != super::Screen::Stacking || !self.files.read(cx).list().is_empty() {
            tracing::debug!("sample stack: no new stack");
            return;
        }
        let stack = self.stack.clone();
        let library = self.renders.read(cx).store().root().to_path_buf();
        let which = sample.stack();
        let task = cx.spawn_in(window, {
            let stack = stack.clone();
            async move |this, cx| {
                let into = library.join(COPIES_FOLDER);
                let ran = crate::work::write("sample-copy", move || {
                    sample.copy_into(&into, None).map(|copied| copied.frames)
                })
                .await;
                this.update_in(cx, |this, window, cx| this.sample_copied(&stack, ran, window, cx))
                    .ok();
            }
        });
        self.set_sample_open(SampleOpen::Copying { stack, sample: which, _task: task }, cx);
    }

    /// The copy landed: its frames join the stack's list if it is still on
    /// screen, empty, with nothing running.
    fn sample_copied(
        &mut self,
        stack: &StackId,
        ran: Result<deptheos_app::Result<Vec<PathBuf>>, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let SampleOpen::Copying { stack: copying, .. } = &self.sample_open else { return };
        if copying != stack {
            return;
        }
        let lead = say!(cx, "sample-copy-failed");
        let paths = match crate::notices::worked(cx.wording(), &lead, ran) {
            Ok(paths) => paths,
            Err(said) => {
                self.set_sample_open(SampleOpen::Idle, cx);
                self.post(Kind::Error, Subject::Alone, said, Vec::new(), window, cx);
                return;
            }
        };
        if self.stack != *stack || self.is_processing(cx) || !self.files.read(cx).list().is_empty()
        {
            // Another stack, a run, or user frames: the copy waits for next time.
            tracing::debug!("sample stack: copied too late for its stack");
            self.set_sample_open(SampleOpen::Idle, cx);
            return;
        }
        self.set_sample_open(SampleOpen::Idle, cx);
        self.add_files(paths, AddedFrom::Window, window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(dir: &Path, names: &[&str]) {
        std::fs::create_dir_all(dir).unwrap();
        for name in names {
            std::fs::write(dir.join(name), name.as_bytes()).unwrap();
        }
    }

    /// Each platform's bundle is found from its exe's folder, both samples
    /// in it; a repository build finds the repository's.
    #[test]
    fn finds_the_samples_in_each_layout() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        let layouts = [
            ("Deptheos.app/Contents/MacOS", "Deptheos.app/Contents/Resources/samples"),
            ("usr/bin", "usr/share/deptheos/samples"),
            ("Deptheos", "Deptheos/samples"),
        ];
        for (exe, samples) in layouts {
            let exe = root.path().join(exe);
            std::fs::create_dir_all(&exe).unwrap();
            assert_eq!(find_all(Some(&exe), &repo), [], "{exe:?}: none yet");
            let samples = root.path().join(samples);
            frames(&samples.join("hornet"), &["hornet-01.jpg", "hornet-02.jpg"]);
            frames(&samples.join("pcb"), &["pcb-001.jpg", "pcb-002.jpg", "pcb-003.jpg"]);
            let found = find_all(Some(&exe), &repo);
            let stacks: Vec<_> = found.iter().map(Sample::stack).collect();
            assert_eq!(stacks, SampleStack::ALL, "{exe:?}");
            for (sample, (folder, count)) in found.iter().zip([("hornet", 2), ("pcb", 3)]) {
                let want = samples.join(folder).canonicalize().unwrap();
                assert_eq!(sample.dir().canonicalize().unwrap(), want);
                assert_eq!(sample.frame_count(), count);
            }
        }
        frames(&repo.join("samples/hornet"), &["a.jpg", "b.JPG"]);
        let found = find_all(Some(&root.path().join("elsewhere")), &repo);
        assert_eq!(found.len(), 1, "only the hornet is there");
        assert!(found[0].dir().ends_with("repo/samples/hornet"));
        // The real build: the repository's samples, all of them.
        let built = bundled();
        let counts: Vec<_> = built.iter().map(|s| (s.stack(), s.frame_count())).collect();
        assert_eq!(counts, [(SampleStack::Hornet, 66), (SampleStack::CircuitBoard, 200)]);
    }

    /// A build with only one sample offers that one, whichever it is.
    #[test]
    fn a_build_with_one_sample_has_that_one() {
        let root = tempfile::tempdir().unwrap();
        let exe = root.path().join("Deptheos");
        frames(&exe.join("samples/pcb"), &["pcb-001.jpg", "pcb-002.jpg"]);
        let found = find_all(Some(&exe), &root.path().join("repo"));
        assert_eq!(
            found.iter().map(Sample::stack).collect::<Vec<_>>(),
            [SampleStack::CircuitBoard]
        );
    }
}
