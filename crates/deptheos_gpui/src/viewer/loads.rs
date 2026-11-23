//! The viewer's loads: one at a time, a newer request dropping (and so
//! canceling) the one before, a result shown only while current. Frames and
//! renders load through one; the lit depth map through another, so lighting
//! never drops a frame loading under the map.

use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;

use deptheos_app::{CompletedStack, PresentedImage};
use gpui::{Context, SharedString, Task};

use super::{CompareLabels, Loader, MapPicture, Shown, Source, ViewEvent, Viewer, ViewerEvent};
use crate::files::FileId;
use crate::images::TiledImage;
use crate::words::{Wording as _, say};
use crate::work;

/// How long the held image stays undimmed while the next loads, so a quick
/// load never flickers.
const DIM_AFTER: Duration = Duration::from_millis(150);

/// What a finished load is shown as.
enum Loaded {
    /// A frame in place of the result.
    Plain,
    /// A compare source, with the labels over its panes when it has them.
    Compare(Option<CompareLabels>),
}

/// One load at a time: a newer request replaces the one loading, dropping
/// its task cancels its decode (`crate::work`). `P` is the request
/// ([`FrameLoad`], [`LitLoad`]); `T` its task.
pub(super) struct Loads<P, T = Task<()>> {
    /// Names the loads in the log.
    name: &'static str,
    /// Request numbers; a result shows only if [`Load::Loading`] holds its.
    generation: u64,
    state: Load<T>,
    _asks: PhantomData<fn() -> P>,
}

/// A viewer load.
///
/// Lifecycle: `Idle` or `Loading` a request, held by a [`Loads`] and
/// changed only through its methods.
///
/// - `Idle` → `Loading` (a request; its task starts).
/// - `Loading` → `Loading` (a newer request replaces it, its decode
///   canceled; or [`DIM_AFTER`] passes and the held image dims).
/// - `Loading` → `Idle` (shown, failed, or canceled).
enum Load<T> {
    Idle,
    /// `task` loads request `generation`, the one to be shown.
    Loading {
        generation: u64,
        /// "Loading bee_013.jpg…".
        label: SharedString,
        held: Held,
        _task: T,
    },
}

/// How the image already on screen is drawn while the next one loads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Held {
    /// Undimmed for [`DIM_AFTER`] after request `since`.
    Shown { since: u64 },
    /// Dimmed, with the pane saying what loads.
    Dimmed,
}

/// A frame, a file or a render to compare, as the frames' loads load it.
#[derive(Clone)]
pub(super) struct FrameLoad {
    file: Option<FileId>,
    source: Source,
    labels: Option<CompareLabels>,
}

/// The depth map with `stack`'s `frame` lit, asked with its token, or whole
/// with none ([`CompletedStack::present_map`], `MapPicture::Off`).
#[derive(Clone)]
pub(super) struct LitLoad {
    pub(super) frame: Option<(usize, u64)>,
    pub(super) stack: Arc<CompletedStack>,
}

/// A request: how its picture is made, which [`Loads`] runs it, and how it
/// is shown once wanted.
pub(super) trait Payload: Clone + Send + 'static {
    /// Its kind on the engine's pool: one of a kind runs at a time.
    const KIND: &'static str;
    /// The picture; runs on the engine's pool.
    fn make(&self, loader: &Loader) -> Result<PresentedImage, deptheos_app::Error>;
    /// The loads that run it.
    fn loads(viewer: &mut Viewer) -> &mut Loads<Self>;
    /// Show it: it is the result wanted.
    fn finish(
        self,
        viewer: &mut Viewer,
        result: Result<PresentedImage, deptheos_app::Error>,
        cx: &mut Context<Viewer>,
    );
}

impl Payload for FrameLoad {
    const KIND: &'static str = "viewer-load";

    fn make(&self, loader: &Loader) -> Result<PresentedImage, deptheos_app::Error> {
        loader(&self.source)
    }

    fn loads(viewer: &mut Viewer) -> &mut Loads<Self> {
        &mut viewer.loads
    }

    fn finish(
        self,
        viewer: &mut Viewer,
        result: Result<PresentedImage, deptheos_app::Error>,
        cx: &mut Context<Viewer>,
    ) {
        let loaded = match self.source {
            Source::File(_) => Loaded::Plain,
            _ => Loaded::Compare(self.labels),
        };
        viewer.finish_load(self.file, loaded, result, cx);
    }
}

impl Payload for LitLoad {
    const KIND: &'static str = "lit-map";

    fn make(&self, _: &Loader) -> Result<PresentedImage, deptheos_app::Error> {
        self.stack
            .present_map(self.frame.map(|(index, _)| index))
            .ok_or_else(|| deptheos_app::Error::InvalidState("no depth map to show".into()))
    }

    fn loads(viewer: &mut Viewer) -> &mut Loads<Self> {
        &mut viewer.lighting
    }

    fn finish(
        self,
        viewer: &mut Viewer,
        result: Result<PresentedImage, deptheos_app::Error>,
        cx: &mut Context<Viewer>,
    ) {
        viewer.finish_light(self.frame, &self.stack, result, cx);
    }
}

impl<P, T> Loads<P, T> {
    pub(super) fn new(name: &'static str) -> Self {
        Self { name, generation: 0, state: Load::Idle, _asks: PhantomData }
    }

    fn state_name(&self) -> &'static str {
        match self.state {
            Load::Idle => "idle",
            Load::Loading { held: Held::Shown { .. }, .. } => "loading",
            Load::Loading { held: Held::Dimmed, .. } => "loading (dimmed)",
        }
    }

    fn set(&mut self, state: Load<T>) {
        let from = self.state_name();
        // The task before, if any, is dropped here: its work is canceled.
        self.state = state;
        tracing::debug!("viewer {}: {from} -> {}", self.name, self.state_name());
    }

    /// The label of what is loading.
    pub(super) fn loading(&self) -> Option<&SharedString> {
        match &self.state {
            Load::Loading { label, .. } => Some(label),
            Load::Idle => None,
        }
    }

    /// Whether the image on screen is dimmed while another loads.
    pub(super) fn dimmed(&self) -> bool {
        matches!(self.state, Load::Loading { held: Held::Dimmed, .. })
    }

    /// Start request `label` with the task `start` makes for its number,
    /// replacing any before. Returns the number.
    fn request(&mut self, label: SharedString, start: impl FnOnce(u64) -> T) -> u64 {
        self.generation += 1;
        let generation = self.generation;
        let held = match &self.state {
            Load::Loading { held, .. } => *held,
            Load::Idle => Held::Shown { since: generation },
        };
        let task = start(generation);
        self.set(Load::Loading { generation, label, held, _task: task });
        generation
    }

    /// Request `generation`'s result is wanted; the load is then over.
    fn arrived(&mut self, generation: u64) -> bool {
        match &self.state {
            Load::Loading { generation: loading, .. } if *loading == generation => {
                self.set(Load::Idle);
                true
            }
            _ => false,
        }
    }

    /// [`DIM_AFTER`] passed since `generation`: dim if still loading. `true`
    /// when that changed what is drawn.
    fn dim(&mut self, generation: u64) -> bool {
        match &mut self.state {
            Load::Loading { held: held @ Held::Shown { .. }, .. }
                if *held == (Held::Shown { since: generation }) =>
            {
                *held = Held::Dimmed;
                tracing::debug!("viewer {}: loading -> loading (dimmed)", self.name);
                true
            }
            _ => false,
        }
    }

    /// Forget the load: its task is dropped, its decode canceled.
    pub(super) fn cancel(&mut self) {
        if let Load::Loading { .. } = self.state {
            self.set(Load::Idle);
        }
    }
}

impl Viewer {
    pub fn is_loading(&self) -> bool {
        self.loads.loading().is_some()
    }

    /// The depth map's lit picture is wanted and not yet shown.
    #[cfg(test)]
    pub fn lighting_busy(&self) -> bool {
        self.lighting.loading().is_some()
    }

    /// What loads while the picture on screen is dimmed, if anything is.
    pub(super) fn dimmed_loading(&self) -> Option<&SharedString> {
        let frames = self.loads.loading().filter(|_| self.loads.dimmed());
        frames.or_else(|| self.lighting.loading().filter(|_| self.lighting.dimmed()))
    }

    /// Show `source` (for `file`) once loaded, a compare source with
    /// `labels`; replaces the load before.
    pub fn load(
        &mut self,
        file: Option<FileId>,
        name: &str,
        source: Source,
        labels: Option<CompareLabels>,
        cx: &mut Context<Self>,
    ) {
        // A render shows no overlay, so a changed overlay need not reload it.
        let shown = match &source {
            Source::Render { .. } => None,
            Source::File(_) | Source::Compare { .. } => Some(Shown {
                file,
                name: name.to_string(),
                source: source.clone(),
                labels: labels.clone(),
            }),
        };
        self.set_shown(shown);
        let label = say!(cx, "viewer-loading", name = name).into();
        self.request(label, FrameLoad { file, source, labels }, cx);
    }

    /// Ask `P`'s loads for `payload`, labeled `label`, replacing the load
    /// before; the held picture dims if it takes a while.
    pub(super) fn request<P: Payload>(
        &mut self,
        label: SharedString,
        payload: P,
        cx: &mut Context<Self>,
    ) {
        let loader = self.loader.clone();
        let generation = P::loads(self).request(label, |generation| {
            cx.spawn(async move |this, cx| {
                let job = payload.clone();
                let result =
                    work::interactive(P::KIND, move || job.make(&loader)).await.unwrap_or_else(
                        |e| Err(deptheos_app::Error::InvalidState(format!("load {e}"))),
                    );
                this.update(cx, |this, cx| {
                    // Shown only while it is the load held.
                    if P::loads(this).arrived(generation) {
                        payload.finish(this, result, cx);
                    }
                })
                .ok();
            })
        });
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DIM_AFTER).await;
            this.update(cx, |this, cx| {
                if P::loads(this).dim(generation) {
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Forget the frames' load: its task is dropped, its decode canceled.
    pub fn cancel_loads(&mut self, cx: &mut Context<Self>) {
        self.loads.cancel();
        cx.notify();
    }

    /// The map lit for `frame` made: shown if its token is still awaited,
    /// else dropped. A failure puts the light out, showing the whole map
    /// (ADR-149).
    pub(super) fn finish_light(
        &mut self,
        frame: Option<(usize, u64)>,
        stack: &Arc<CompletedStack>,
        result: Result<PresentedImage, deptheos_app::Error>,
        cx: &mut Context<Self>,
    ) {
        let image = match result {
            Ok(image) => Some(TiledImage::from_presented(image)),
            Err(e) => {
                tracing::warn!("the depth map couldn't be shown for frame {frame:?}: {e}");
                None
            }
        };
        let now = self.state.now();
        match frame {
            Some((_, token)) if image.is_none() => {
                self.on(ViewEvent::LightFailed { token }, cx);
            }
            Some((_, token)) if now.lit_token() == Some(token) => {
                self.atlas.replace(&mut self.lit_map, image)
            }
            // Made again whole, for the map still shown whole of its stack.
            None if now.is_map()
                && now.lit_token().is_none()
                && matches!(&self.map, MapPicture::Off(off) if Arc::ptr_eq(off, stack)) =>
            {
                if let Some(image) = image {
                    self.set_map(MapPicture::Held(image, Some(stack.clone())));
                }
            }
            _ => return,
        }
        cx.notify();
    }

    /// Show what loaded as `loaded` says: it is the load wanted.
    fn finish_load(
        &mut self,
        file: Option<FileId>,
        loaded: Loaded,
        result: Result<PresentedImage, deptheos_app::Error>,
        cx: &mut Context<Self>,
    ) {
        match (result, loaded) {
            (Ok(image), Loaded::Compare(labels)) => {
                self.compare_labels = labels;
                self.atlas.replace(&mut self.secondary, Some(TiledImage::from_presented(image)));
            }
            (Ok(image), Loaded::Plain) => {
                self.atlas.replace(&mut self.primary, Some(TiledImage::from_presented(image)));
                self.set_map(MapPicture::None);
                self.drop_compare();
            }
            (Err(e), _) => {
                tracing::warn!("load failed: {e}");
                // Not retried by an overlay change either.
                self.set_shown(None);
                let explained = crate::notices::explain(&e, crate::words::of(cx));
                let message = crate::notices::sentences(
                    cx.wording(),
                    None,
                    &explained,
                    crate::notices::Details::Diagnostics,
                );
                cx.emit(ViewerEvent::LoadFailed { file, message });
            }
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    /// A load's task in these tests: notes its request's number when dropped.
    struct Probe(u64, Rc<RefCell<Vec<u64>>>);

    impl Drop for Probe {
        fn drop(&mut self) {
            self.1.borrow_mut().push(self.0);
        }
    }

    struct Fixture {
        loads: Loads<FrameLoad, Probe>,
        dropped: Rc<RefCell<Vec<u64>>>,
    }

    fn loads() -> Fixture {
        Fixture { loads: Loads::new("load"), dropped: Rc::default() }
    }

    fn request(f: &mut Fixture, name: &str) -> u64 {
        let dropped = f.dropped.clone();
        f.loads.request(name.to_string().into(), |generation| Probe(generation, dropped))
    }

    /// The newest request is the one loading; earlier results are never shown.
    #[test]
    fn only_the_newest_result_is_shown() {
        let mut f = loads();
        let a = request(&mut f, "a");
        let b = request(&mut f, "b");
        assert_eq!(*f.dropped.borrow(), [a], "a's task goes as b comes");
        let c = request(&mut f, "c");
        assert_eq!(*f.dropped.borrow(), [a, b]);
        assert_eq!(f.loads.loading().map(|s| s.as_ref()), Some("c"));
        assert!(!f.loads.arrived(a) && !f.loads.arrived(b));
        assert!(f.loads.arrived(c));
        assert_eq!(*f.dropped.borrow(), [a, b, c]);
        assert_eq!(f.loads.loading(), None);
        assert!(matches!(f.loads.state, Load::Idle));
    }

    /// A cancel drops the load's task, and its result is never shown.
    #[test]
    fn a_cancel_drops_the_load() {
        let mut f = loads();
        let a = request(&mut f, "a");
        f.loads.cancel();
        assert_eq!(*f.dropped.borrow(), [a]);
        assert_eq!(f.loads.loading(), None);
        assert!(!f.loads.arrived(a));
        assert!(matches!(f.loads.state, Load::Idle));
    }

    /// The held image dims once loading has gone on a while, across later
    /// requests, but not for a load that already ended.
    #[test]
    fn the_held_image_dims_while_loading() {
        let mut f = loads();
        let a = request(&mut f, "a");
        assert!(!f.loads.dimmed());
        let b = request(&mut f, "b");
        assert!(!f.loads.dim(b), "b's timer: the loading started with a");
        assert!(f.loads.dim(a));
        assert!(f.loads.dimmed());
        assert!(!f.loads.arrived(a));
        assert!(f.loads.dimmed(), "still loading: still dimmed");
        assert!(f.loads.arrived(b));
        assert!(!f.loads.dimmed());

        let c = request(&mut f, "c");
        assert!(!f.loads.dim(a) && !f.loads.dim(b), "an earlier loading's timers");
        assert!(!f.loads.dimmed());
        assert!(f.loads.dim(c));
    }
}
