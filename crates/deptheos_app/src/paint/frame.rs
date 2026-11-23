//! The sources besides the original, loaded the same on every front end:
//! a frame (placed through a sibling render when the render left it out),
//! and another render of its stack, lined up through a frame both stacked.
//! Each is warped into the render's geometry and exposure.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use super::worker::{Failure, SourceLoader};
use crate::library::history::{History, Render, RenderId};
use crate::words::{Words, say};
use crate::{CompletedStack, Engine, Explain, SourceInput};

/// How a front end words a source's load error, after the worker's lead.
pub type Told = Arc<dyn Fn(&dyn Explain) -> Failure + Send + Sync>;

/// "The two renders share no frame, so they can't be lined up."
pub fn no_shared_frame(words: &Words) -> String {
    say!(words, "paint-no-shared-frame")
}

/// The other renders a frame the render didn't stack can be placed through.
#[derive(Clone)]
pub struct Siblings {
    pub history: Arc<History>,
    /// The render painted, never its own sibling.
    pub render: RenderId,
    /// The renders to try, in order. Called on the worker, only for a frame
    /// the render didn't stack.
    pub list: Arc<dyn Fn() -> Vec<Render> + Send + Sync>,
}

impl Siblings {
    /// Every render of `history`, listed once per open render, the first
    /// time a frame needs them. One deleted since is skipped.
    pub fn listed_once(history: Arc<History>, render: RenderId) -> Self {
        let listed = OnceLock::new();
        let lister = history.clone();
        let list = Arc::new(move || {
            listed
                .get_or_init(|| {
                    lister.list().unwrap_or_else(|e| {
                        tracing::warn!("paint: the Library couldn't be listed: {e}");
                        Vec::new()
                    })
                })
                .clone()
        });
        Self { history, render, list }
    }
}

/// Builds the frame at `path` as a source for `stack`, whose frames are
/// `paths` as they are now. A stacked frame is placed by its own
/// alignment; any other through a sibling that stacked it, shares a frame
/// with it and used the same RAW backend.
pub fn frame_loader(
    engine: Arc<Engine>,
    stack: Arc<CompletedStack>,
    paths: Vec<PathBuf>,
    path: PathBuf,
    siblings: Siblings,
    told: Told,
) -> SourceLoader {
    Arc::new(move |with_pixels| {
        let stacked =
            paths.iter().position(|p| *p == path).is_some_and(|i| !stack.geometry.is_left_out(i));
        let geometries: Vec<_> = if stacked {
            Vec::new()
        } else {
            (siblings.list)()
                .into_iter()
                .filter(|r| {
                    r.id != siblings.render
                        && r.options.raw_backend == stack.options.raw_backend
                        && r.frames.iter().any(|f| paths.contains(f))
                })
                .filter_map(|r| siblings.history.load_geometry(&r.id).ok())
                .collect()
        };
        let alignment = stack
            .geometry
            .frame_alignment(&paths, &path, geometries.iter().map(|(p, g)| (p.as_slice(), g)))
            .ok_or_else(|| say!(engine.words(), "paint-frame-not-lined-up"))?;
        let input = SourceInput::Frame { path: &path, alignment };
        engine.paint_source(&stack, input, with_pixels).map_err(|e| told(&e))
    })
}

/// Builds render `other` (loaded with the source) as a source for `stack`,
/// lined up through a frame both stacked.
pub fn render_loader(
    engine: Arc<Engine>,
    stack: Arc<CompletedStack>,
    other: impl Fn() -> Result<Arc<CompletedStack>, Failure> + Send + Sync + 'static,
    told: Told,
) -> SourceLoader {
    Arc::new(move |with_pixels| {
        let other = other()?;
        let alignment =
            stack.alignment_to(&other).ok_or_else(|| no_shared_frame(engine.words()))?;
        let input = SourceInput::Render { other: &other, pixels: &other.pixels, alignment };
        engine.paint_source(&stack, input, with_pixels).map_err(|e| told(&e))
    })
}
