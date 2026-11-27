//! A render's thumbnail drawn again under its look, when an Adjust or
//! Retouch session on it closes (ADR-140): its adjustments read on the
//! store thread, in order with their writes, the picture drawn on the
//! pool, the file written on the store thread. A rewrite sent later
//! overtakes one not yet written, which is skipped. The app then tells its
//! thumbnails ([`super::Event::Rewritten`]), which read it again.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use deptheos_render::reframe::{Rect, Size};
use deptheos_render::{Adjustments, Draw, OutputSpace, Reframe};
use image::RgbImage;

use crate::adjust::GpuCopies;
use crate::engine::Engine;
use crate::library::adjustments::RenderAdjustments;
use crate::library::history::{History, RenderId};
use crate::library::render_file::{THUMBNAIL_FILE, THUMBNAIL_SIDE, encode_thumbnail};
use crate::paint::Flattened;
use crate::runs::session::CompletedStack;
use crate::{Error, Look, Result};

/// What a rewrite did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rewrote {
    Written,
    /// A later rewrite was sent, which writes in its place.
    Overtaken,
}

/// Each render's count of rewrites sent, shared by every caller, so a
/// rewrite from Adjust overtakes one from Retouch queued before it.
#[derive(Clone, Debug, Default)]
pub(crate) struct Rewrites(Arc<Mutex<HashMap<RenderId, Arc<AtomicU64>>>>);

impl Rewrites {
    /// A new rewrite of `id`: its number, and the count it is checked
    /// against.
    fn send(&self, id: &RenderId) -> (u64, Arc<AtomicU64>) {
        let sent = crate::lock(&self.0).entry(id.clone()).or_default().clone();
        (sent.fetch_add(1, Ordering::AcqRel) + 1, sent)
    }
}

impl History {
    /// Draw render `id`'s thumbnail again under its adjustments and
    /// reframe, from `stack` with the paint `flattened` answers (none: as
    /// stored). Never canceled; quit waits for it.
    pub fn rewrite_thumbnail(
        self: &Arc<Self>,
        engine: Arc<Engine>,
        id: RenderId,
        stack: Arc<CompletedStack>,
        flattened: Option<Receiver<Flattened>>,
    ) -> impl std::future::Future<Output = Result<Rewrote>> + Send + 'static {
        let (tx, rx) = futures::channel::oneshot::channel();
        self.rewrite_thumbnail_then(engine, id, stack, flattened, move |done| {
            let _ = tx.send(done);
        });
        async move {
            rx.await.unwrap_or_else(|_| {
                Err(Error::InvalidState("the thumbnail rewrite panicked".into()))
            })
        }
    }

    /// [`Self::rewrite_thumbnail`] with the paint this Library's paint
    /// worker holds for `id`, if any: for an app that keeps none.
    pub fn rewrite_thumbnail_painted(
        self: &Arc<Self>,
        engine: Arc<Engine>,
        id: RenderId,
        stack: Arc<CompletedStack>,
    ) -> impl std::future::Future<Output = Result<Rewrote>> + Send + 'static {
        let flattened = crate::paint::worker::flatten(self.root(), id.clone(), stack.clone());
        self.rewrite_thumbnail(engine, id, stack, flattened)
    }

    /// [`Self::rewrite_thumbnail`], calling `done` with what it did, on the
    /// pool's or the store's thread.
    pub fn rewrite_thumbnail_then(
        self: &Arc<Self>,
        engine: Arc<Engine>,
        id: RenderId,
        stack: Arc<CompletedStack>,
        flattened: Option<Receiver<Flattened>>,
        done: impl FnOnce(Result<Rewrote>) + Send + 'static,
    ) {
        let (number, sent) = self.rewrites.send(&id);
        let overtaken = move || number < sent.load(Ordering::Acquire);
        let history = self.clone();
        let read = move || {
            let value = match history.render(&id).map_err(Error::from).and_then(|render| {
                history.adjustments(&render).map_err(|e| {
                    Error::InvalidState(format!("render {}'s adjustments: {e}", id.as_str()))
                })
            }) {
                Ok(value) => value,
                Err(e) => {
                    done(Err(e));
                    return;
                }
            };
            let draw = move || {
                if overtaken() {
                    done(Ok(Rewrote::Overtaken));
                    return;
                }
                let pixels = match flattened.map(|rx| rx.recv()) {
                    None | Some(Ok(Ok(None))) => None,
                    Some(Ok(Ok(Some(pixels)))) => Some(pixels),
                    Some(Ok(Err(e))) => {
                        done(Err(e));
                        return;
                    }
                    Some(Err(_)) => {
                        let why = "the paint worker stopped before the paint was flattened";
                        done(Err(Error::InvalidState(why.into())));
                        return;
                    }
                };
                let pixels = pixels.as_deref().unwrap_or(&stack.pixels);
                let generation = engine.generations().of(&id);
                let drawn = draw(&engine, &stack, pixels, &value, generation)
                    .and_then(|image| encode_thumbnail(&image).map_err(Error::from));
                let bytes = match drawn {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        done(Err(e));
                        return;
                    }
                };
                let _written = crate::engine::store::store().write(move || {
                    if overtaken() {
                        done(Ok(Rewrote::Overtaken));
                        return;
                    }
                    let path = history.render_path(&id).join(THUMBNAIL_FILE);
                    let written = deptheos_core::io::write_replacing(&path, |tmp| {
                        std::fs::write(tmp, &bytes).map_err(Into::into)
                    });
                    tracing::debug!("render {}: thumbnail rewritten", id.as_str());
                    done(written.map(|()| Rewrote::Written).map_err(|e| {
                        Error::from(crate::StoreError::Io {
                            path,
                            source: std::io::Error::other(e.to_string()),
                        })
                    }));
                });
            };
            crate::engine::work::pool()
                .run(
                    crate::engine::work::Lane::Background,
                    crate::engine::work::Key::kind("thumbnail-rewrite"),
                    &crate::engine::work::must_finish(),
                    draw,
                )
                .detach();
        };
        let _read = crate::engine::store::store().write(read);
    }
}

/// `stack`'s thumbnail with `pixels` under `value`, in sRGB: the
/// reframe's window, flipped and turned, [`THUMBNAIL_SIDE`] on its long
/// side. With every slider at neutral it is the unadjusted display's, as
/// a save takes it; else drawn on the GPU from copies made at
/// `generation` ([`GpuCopies::thumbnail`]), with the vignette centered on
/// the window.
pub fn draw(
    engine: &Engine,
    stack: &CompletedStack,
    pixels: &[u16],
    value: &RenderAdjustments,
    generation: crate::engine::generation::Generation,
) -> Result<RgbImage> {
    let RenderAdjustments { adjustments, reframe } = value;
    let picture = Size { width: stack.width, height: stack.height };
    let window = reframe
        .window(picture)
        .map_err(|o| Error::InvalidState(format!("crop {:?} outside the picture", o.crop)))?;
    if adjustments.is_neutral() {
        let shown = engine.present_pixels(Look::shown(stack, &Adjustments::NEUTRAL), pixels)?;
        return Ok(unadjusted(&shown, picture, window, *reframe));
    }
    let look = Look::redrawn(stack, adjustments, *reframe, OutputSpace::Srgb);
    let Draw::Rendered(constants) = look.resolve() else {
        return Err(Error::InvalidState("a redrawn look draws on the GPU".into()));
    };
    let mut copies = GpuCopies::make(engine, stack, pixels, generation)?;
    copies.thumbnail(engine, &constants, *reframe)
}

/// The unadjusted display `shown` reframed: `window` cut from its
/// smallest level that holds it at [`THUMBNAIL_SIDE`], reduced, flipped
/// and turned.
fn unadjusted(
    shown: &crate::PresentedImage,
    picture: Size,
    window: Rect,
    reframe: Reframe,
) -> RgbImage {
    let long = window.width.max(window.height);
    let level = shown
        .levels
        .iter()
        .rev()
        .find(|l| u64::from(long) * u64::from(l.width) >= u64::from(THUMBNAIL_SIDE * picture.width))
        .or(shown.levels.first())
        .expect("a display has a level");
    let image = level.assemble().to_rgb();
    let (sx, sy) = (
        image.width() as f64 / picture.width as f64,
        image.height() as f64 / picture.height as f64,
    );
    let x = ((window.x as f64 * sx).round() as u32).min(image.width() - 1);
    let y = ((window.y as f64 * sy).round() as u32).min(image.height() - 1);
    let w = ((window.width as f64 * sx).round() as u32).clamp(1, image.width() - x);
    let h = ((window.height as f64 * sy).round() as u32).clamp(1, image.height() - y);
    let cut = image::imageops::crop_imm(&image, x, y, w, h).to_image();
    let scale = (THUMBNAIL_SIDE as f64 / long as f64).min(1.0);
    let target = |v: u32| ((v as f64 * scale).round() as u32).max(1);
    let (tw, th) = (target(window.width), target(window.height));
    let cut = if (tw, th) == (w, h) { cut } else { image::imageops::thumbnail(&cut, tw, th) };
    let turned = Reframe { crop: None, ..reframe };
    let (pixels, out) = turned
        .apply(cut.as_raw(), Size { width: tw, height: th }, 3)
        .expect("the whole picture is inside it");
    RgbImage::from_raw(out.width, out.height, pixels).expect("rows of its size")
}
