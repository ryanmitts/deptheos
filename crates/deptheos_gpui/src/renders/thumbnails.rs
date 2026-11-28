//! Render thumbnails for the strip, the Library and Render Info: the engine's
//! policy ([`deptheos_app::library::thumbnails`], ADR-144) decides what to
//! read, keep and drop; this holds the GPUI images, in the window's space
//! ([`Thumbnails::follow`]), and the read in flight.

use std::collections::HashMap;
#[cfg(test)]
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use deptheos_app::library::history::RenderId;
#[cfg(test)]
use deptheos_app::library::thumbnails::Thumb;
use deptheos_app::library::thumbnails::{Action, Event, ReadToken};
use deptheos_app::{CpuImage, ScreenSpace};
use gpui::{App, RenderImage, Task};

use crate::images::{render_srgb, reshown};

/// The most thumbnails kept (about 170 KB each, plus an atlas copy), more
/// than a Library screen shows; the least wanted go first and reload in view.
const THUMBS_KEPT: usize = 400;

/// A read to start: its token and what it reads.
pub(super) type Read = (ReadToken, Vec<RenderId>);

/// The policy's thumbnails, the pixels of those shown, and the read in flight.
pub(super) struct Thumbnails {
    policy: deptheos_app::library::thumbnails::Thumbnails,
    /// Added only by the policy's [`Action::Keep`], removed by [`Action::Drop`].
    images: HashMap<RenderId, Arc<RenderImage>>,
    /// The read in flight, while it runs.
    loading: Option<Task<()>>,
    /// Added to the clock (a test's wait for a retry).
    skew: Duration,
    /// The space the images are shown in.
    shown: ScreenSpace,
}

impl Default for Thumbnails {
    fn default() -> Self {
        Self {
            policy: deptheos_app::library::thumbnails::Thumbnails::new(THUMBS_KEPT),
            images: HashMap::new(),
            loading: None,
            skew: Duration::ZERO,
            shown: crate::color::shown(),
        }
    }
}

impl Thumbnails {
    fn now(&self) -> Instant {
        Instant::now() + self.skew
    }

    /// Feed the policy `event` and apply it: keep what `landed` holds that it
    /// keeps, drop what it drops (from every atlas). Returns the read it starts.
    fn on(
        &mut self,
        event: Event,
        mut landed: HashMap<RenderId, Arc<RenderImage>>,
        cx: &mut App,
    ) -> Option<Read> {
        let mut read = None;
        for action in self.policy.on(event) {
            match action {
                Action::Read { token, ids } => read = Some((token, ids)),
                Action::Keep(id) => {
                    let Some(image) = landed.remove(&id) else { continue };
                    if let Some(old) = self.images.insert(id, image) {
                        cx.defer(move |cx| cx.drop_image(old, None));
                    }
                }
                Action::Drop(id) => {
                    if let Some(old) = self.images.remove(&id) {
                        cx.defer(move |cx| cx.drop_image(old, None));
                    }
                }
            }
        }
        read
    }

    /// `id`'s thumbnail, once read.
    pub(super) fn get(&self, id: &RenderId) -> Option<Arc<RenderImage>> {
        self.images.get(id).cloned()
    }

    /// `ids` wanted in view, most wanted first: the read to start, if any.
    pub(super) fn want(&mut self, ids: Vec<RenderId>, cx: &mut App) -> Option<Read> {
        let now = self.now();
        self.on(Event::Wanted { ids, now }, HashMap::new(), cx)
    }

    /// The read in flight, or none once it has landed.
    pub(super) fn set_loading(&mut self, loading: Option<Task<()>>) {
        self.loading = loading;
    }

    /// Read `token` of `asked` landed with `read`: the next read, if any.
    pub(super) fn read(
        &mut self,
        token: ReadToken,
        asked: Vec<RenderId>,
        read: Vec<(RenderId, CpuImage)>,
        cx: &mut App,
    ) -> Option<Read> {
        let now = self.now();
        let mut read: HashMap<RenderId, CpuImage> = read.into_iter().collect();
        for id in asked {
            let image = read.remove(&id).map(|image| render_srgb(image, self.shown));
            let ok = image.is_some();
            let landed =
                image.map(|image| HashMap::from([(id.clone(), image)])).unwrap_or_default();
            self.on(Event::Read { token, id, ok, now }, landed, cx);
        }
        self.on(Event::Finished { token, now }, HashMap::new(), cx)
    }

    /// `id` was saved with `image`: shown at once.
    pub(super) fn saved(&mut self, id: &RenderId, image: CpuImage, cx: &mut App) {
        let landed = HashMap::from([(id.clone(), render_srgb(image, self.shown))]);
        self.on(Event::Saved { id: id.clone() }, landed, cx);
    }

    /// Show the thumbnails in `shown`, the old images leaving the atlas.
    pub(super) fn follow(&mut self, shown: ScreenSpace, cx: &mut App) {
        let from = std::mem::replace(&mut self.shown, shown);
        for image in self.images.values_mut() {
            if let Some(new) = reshown(image, from, shown) {
                let old = std::mem::replace(image, new);
                cx.defer(move |cx| cx.drop_image(old, None));
            }
        }
    }

    /// `id`'s thumbnail file was drawn again: the one held goes, and the
    /// read to start, if it is in view.
    pub(super) fn rewritten(&mut self, id: &RenderId, cx: &mut App) -> Option<Read> {
        let now = self.now();
        self.on(Event::Rewritten { id: id.clone(), now }, HashMap::new(), cx)
    }

    /// `ids` were deleted: their shown thumbnails, for the caller to drop
    /// from the atlas.
    pub(super) fn forget(&mut self, ids: &[RenderId]) -> Vec<Arc<RenderImage>> {
        let going: Vec<Arc<RenderImage>> =
            ids.iter().filter_map(|id| self.images.remove(id)).collect();
        // Nothing left for its drops to let go of.
        let actions = self.policy.on(Event::Deleted { ids: ids.to_vec() });
        debug_assert!(actions.iter().all(|a| matches!(a, Action::Drop(_))));
        going
    }

    /// The thumbnails read so far.
    #[cfg(test)]
    pub(super) fn shown_ids(&self) -> HashSet<RenderId> {
        self.images.keys().cloned().collect()
    }

    /// The thumbnails asked for: read, or being read.
    #[cfg(test)]
    pub(super) fn requested_ids(&self) -> HashSet<RenderId> {
        let asked = self.policy.thumbs().filter(|(_, t)| !matches!(t, Thumb::Failed { .. }));
        asked.map(|(id, _)| id.clone()).collect()
    }

    /// Whether `id`'s thumbnail failed to read, last time it was asked for.
    #[cfg(test)]
    pub(super) fn failed(&self, id: &RenderId) -> bool {
        matches!(self.policy.get(id), Some(Thumb::Failed { .. }))
    }

    /// Keep at most `kept` thumbnails.
    #[cfg(test)]
    pub(super) fn set_kept(&mut self, kept: usize) {
        self.policy.set_kept(kept);
    }

    /// Let failed thumbnails be read again, as if
    /// [`deptheos_app::library::thumbnails::RETRY_AFTER`] had passed.
    #[cfg(test)]
    pub(super) fn age_failures(&mut self) {
        self.skew += deptheos_app::library::thumbnails::RETRY_AFTER;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u32) -> RenderId {
        RenderId::parse(&format!("{n:032x}")).unwrap()
    }

    fn picture() -> CpuImage {
        CpuImage::from_rgb(&image::RgbImage::new(4, 4))
    }

    /// A replaced thumbnail goes to `drop_image`: GPUI keeps a drawn image in
    /// the atlas until then.
    #[gpui::test]
    fn a_replaced_thumbnail_leaves_the_atlas(cx: &mut gpui::TestAppContext) {
        let a = id(1);
        let mut t = Thumbnails::default();
        let held = cx.update(|cx| {
            t.saved(&a, picture(), cx);
            Arc::downgrade(&t.get(&a).unwrap())
        });
        cx.update(|cx| {
            t.saved(&a, picture(), cx);
            assert!(held.upgrade().is_some(), "for drop_image, once no window draws");
        });
        assert!(held.upgrade().is_none(), "then dropped");
        assert!(t.get(&a).is_some(), "the new one shown");
    }
}
