//! A picture the viewer shows, whole at any zoom: the engine's tiled pyramid
//! ([`PresentedImage`]) handed over as one object with its base
//! ([`Pictured`]). The app draws the base first, and asks for
//! [`Picture::tiles`] of a finer level only where zoomed past it. Each is
//! made on the pool for a [`Request`] (ADR-143).
//!
//! A picture handed out holds its pyramid alone: the result may release its
//! own ([`StackResult::release_displays`]), and the pixels live as long as
//! the app holds the picture. Each picture logs at debug as it is made and
//! dropped, with its size and levels, to read beside a memory measurement.

use std::path::PathBuf;
use std::sync::Arc;

use deptheos_app::engine::work::Lane;
use deptheos_app::{PresentedImage, RawBackend};

use crate::request::{Ask, on_pool};
use crate::{DisplayImage, Engine, Request, Result, StackResult, Told, level_image};

/// A picture's full-size width and height, in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct PictureSize {
    pub width: u32,
    pub height: u32,
}

/// One level of a [`Picture`], in `cols` × `rows` tiles (a tile's side:
/// [`crate::paint_tile_core`]): full size halved `halvings` times, rounding
/// up. Every level is drawn stretched over the full-size rect
/// ([`Picture::size`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct PictureLevel {
    pub width: u32,
    pub height: u32,
    pub cols: u32,
    pub rows: u32,
    pub halvings: u32,
}

/// A picture with its base, at most the side asked on its long side, made
/// together from one pyramid.
#[derive(uniffi::Record)]
pub struct Pictured {
    pub picture: Arc<Picture>,
    /// `None` for a picture with no pixels, or a base side of 0 (remade for
    /// a base still on screen).
    pub base: Option<DisplayImage>,
}

impl Pictured {
    pub(crate) fn of(picture: Arc<Picture>, base_side: u32) -> Self {
        let base = if base_side == 0 { None } else { picture.base(base_side) };
        Self { picture, base }
    }
}

/// A result's pictures as it opens ([`StackResult::pictures`]).
#[derive(uniffi::Record)]
pub struct ResultPictures {
    pub display: Option<Pictured>,
    pub map: Option<Pictured>,
}

/// Which picture of a result ([`StackResult::picture`]).
#[derive(uniffi::Enum)]
pub enum PictureOf {
    /// The result.
    Display,
    /// The depth map, its finest level half size: each part in the legend
    /// color ([`StackResult::frame_colors`]) of the frame it came from,
    /// whatever the method (`CompletedStack::present_map`). None for a
    /// pyramid or wavelet render saved without a frame map.
    Map,
    /// The depth map lit for `frame`: what it supplied keeps its color, the
    /// rest dims to gray. None without a frame map, or for a frame it didn't
    /// stack.
    LitMap { frame: PathBuf },
    /// The frame at `path` in the result's geometry and color, to compare
    /// pixel for pixel. With `contribution`, the pixels the result took stay
    /// and the rest goes dim gray (an unstacked frame shows whole). A frame
    /// not in the stack is only cropped. Uses the GPU.
    ComparedFrame { path: PathBuf, contribution: bool },
    /// Another render in the result's geometry, to compare pixel for pixel.
    /// Fails when they share no frame, or only one is camera RAW. Uses the
    /// GPU.
    ComparedRender { other: Arc<StackResult> },
}

/// A tile asked for ([`Picture::tiles`]): with `stand_in`, the one a level
/// coarser when this one isn't there yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TileAsk {
    pub level: u32,
    pub tx: u32,
    pub ty: u32,
    pub stand_in: bool,
}

/// What came of a [`TileAsk`].
#[derive(uniffi::Enum)]
pub enum TileFetched {
    Tile {
        image: DisplayImage,
    },
    /// Not there yet: the one a level coarser, to show a quarter of.
    Coarser {
        image: DisplayImage,
    },
    Nothing,
}

/// Each of `asked` from `tile`, of a picture with `levels` levels.
pub(crate) fn fetch_tiles(
    asked: &[TileAsk],
    levels: u32,
    tile: impl Fn(u32, u32, u32) -> Option<DisplayImage>,
) -> Vec<TileFetched> {
    let fetch = |a: &TileAsk| {
        if let Some(image) = tile(a.level, a.tx, a.ty) {
            return TileFetched::Tile { image };
        }
        let coarser = (a.stand_in && a.level + 1 < levels)
            .then(|| tile(a.level + 1, a.tx / 2, a.ty / 2))
            .flatten();
        coarser.map_or(TileFetched::Nothing, |image| TileFetched::Coarser { image })
    };
    asked.iter().map(fetch).collect()
}

/// A displayable picture with every level the engine made, finest (full
/// size, or half for the depth map) down: the result, depth map, lit map,
/// a compared frame or render, a frame alone. Immutable; any thread may
/// ask it for pixels.
#[derive(uniffi::Object)]
pub struct Picture {
    image: Arc<PresentedImage>,
}

impl Picture {
    /// Sharing `image`.
    pub(crate) fn shared(image: Arc<PresentedImage>) -> Arc<Self> {
        let picture = Self { image };
        tracing::debug!(
            "picture made: {}, {}",
            picture.describe(),
            if Arc::strong_count(&picture.image) > 1 { "shared" } else { "its own" }
        );
        Arc::new(picture)
    }

    /// "8256×5504, 4 levels, 243 MB": for the log.
    fn describe(&self) -> String {
        let mb = self.image.bytes() as f64 / 1_000_000.0;
        format!(
            "{}×{}, {} levels, {mb:.0} MB",
            self.image.width(),
            self.image.height(),
            self.image.levels.len()
        )
    }

    /// Of `image`, which it alone holds.
    pub(crate) fn of(image: PresentedImage) -> Arc<Self> {
        Self::shared(Arc::new(image))
    }
}

impl Drop for Picture {
    fn drop(&mut self) {
        // The last holder frees the pyramid; else the result still holds it.
        let freed = Arc::strong_count(&self.image) == 1;
        tracing::debug!(
            "picture let go of: {}, {}",
            self.describe(),
            if freed { "freed" } else { "still held by its result" }
        );
    }
}

#[uniffi::export]
impl Picture {
    /// Full size, whatever its finest level's: the depth map's is half.
    pub fn size(&self) -> PictureSize {
        PictureSize { width: self.image.width(), height: self.image.height() }
    }

    /// Finest first; `level` in [`Picture::tiles`] indexes this.
    pub fn levels(&self) -> Vec<PictureLevel> {
        (self.image.first_level..)
            .zip(&self.image.levels)
            .map(|(halvings, l)| PictureLevel {
                width: l.width,
                height: l.height,
                cols: l.cols,
                rows: l.rows,
                halvings,
            })
            .collect()
    }

    /// The tiles `asked`, each `(tx, ty)` of `level` (into
    /// [`Picture::levels`]) with its one-pixel gutter, as
    /// [`crate::PaintSession::tiles`]: `core + 2` pixels a side, the core
    /// short of [`crate::paint_tile_core`] at the right and bottom. Copied
    /// on the pool for `request`.
    pub async fn tiles(
        &self,
        request: Arc<Request>,
        asked: Vec<TileAsk>,
    ) -> Result<Vec<TileFetched>> {
        let picture = Self { image: self.image.clone() };
        let levels = self.image.levels.len() as u32;
        let words = deptheos_app::Words::english(deptheos_app::Platform::Ios);
        let fetch = move || Ok(fetch_tiles(&asked, levels, |l, x, y| picture.tile(l, x, y)));
        on_pool(&words, &request, Lane::Interactive, Ask::Own("picture-tiles"), fetch).await
    }
}

impl Picture {
    /// The flat picture shown first: the smallest level at least `max_side`
    /// long, scaled down to it. `None` only for a picture with no pixels.
    pub(crate) fn base(&self, max_side: u32) -> Option<DisplayImage> {
        level_image(&self.image, max_side)
    }

    /// One of [`Picture::tiles`]; `None` when there is no such tile.
    pub(crate) fn tile(&self, level: u32, tx: u32, ty: u32) -> Option<DisplayImage> {
        let tiled = self.image.levels.get(level as usize)?;
        if tx >= tiled.cols || ty >= tiled.rows {
            return None;
        }
        let tile = tiled.tiles.get((ty * tiled.cols + tx) as usize)?;
        let (width, height, bgra) = (tile.width, tile.height, tile.pixels.clone());
        Some(DisplayImage { width, height, bgra, space: self.image.space().into() })
    }
}

#[uniffi::export]
impl StackResult {
    /// The picture `of`, with its base at most `base_side`, made on the pool
    /// for `request`. The display and map share what this result holds; once
    /// released ([`Self::pictures`], or a save) they are remade from the
    /// stack (the display on the GPU for a linear stack, the map on the
    /// CPU), for an app that dropped its picture in the background. The lit
    /// map is made from the stack each time. `None` when the result has none
    /// ([`PictureOf`]).
    pub async fn picture(
        self: Arc<Self>,
        request: Arc<Request>,
        of: PictureOf,
        base_side: u32,
    ) -> Result<Option<Pictured>> {
        let this = self.clone();
        let make = move || Ok(this.picture_of(&of)?.map(|p| Pictured::of(p, base_side)));
        on_pool(self.engine.words(), &request, Lane::Interactive, Ask::Own("picture"), make).await
    }

    /// The display's and map's pictures with bases at most `base_side`, on
    /// the pool for `request`; then it releases its own displays (each
    /// picture keeps its pyramid). All the app shows as a render opens.
    pub async fn pictures(
        self: Arc<Self>,
        request: Arc<Request>,
        base_side: u32,
    ) -> Result<ResultPictures> {
        let this = self.clone();
        let make = move || {
            let display = this.picture_of(&PictureOf::Display)?;
            let map = this.picture_of(&PictureOf::Map)?;
            this.release_displays();
            Ok(ResultPictures {
                display: display.map(|p| Pictured::of(p, base_side)),
                map: map.map(|p| Pictured::of(p, base_side)),
            })
        };
        on_pool(self.engine.words(), &request, Lane::Interactive, Ask::Own("pictures"), make).await
    }
}

impl StackResult {
    /// [`Self::picture`], here.
    pub(crate) fn picture_of(&self, of: &PictureOf) -> Result<Option<Arc<Picture>>> {
        let made = match of {
            PictureOf::Display => {
                if let Some(held) = self.held(false) {
                    return Ok(Some(Picture::shared(held)));
                }
                // Under the render's adjustments, read after its pending
                // writes; a result not saved has none.
                let words = self.engine.words();
                let saved = self.saved_adjustments()?;
                Some(self.engine.present_display(&self.stack, &saved).told(words)?)
            }
            PictureOf::Map => {
                if let Some(held) = self.held(true) {
                    return Ok(Some(Picture::shared(held)));
                }
                self.stack.present_map(None)
            }
            PictureOf::LitMap { frame } => self.lit_map(frame),
            PictureOf::ComparedFrame { path, contribution } => {
                Some(self.compared_frame(path, *contribution)?)
            }
            PictureOf::ComparedRender { other } => Some(self.compared_render(other)?),
        };
        Ok(made.map(Picture::of))
    }
}

#[uniffi::export]
impl Engine {
    /// The frame at `path` as the engine decodes it, base at most
    /// `base_side`: camera RAW and DNG developed on the GPU in their own
    /// color (RAW by `raw_decoder`, the stack's), others as stored, to sRGB.
    /// Not lined up with any result. Decoded on the pool for `request`.
    pub async fn frame_picture(
        self: Arc<Self>,
        request: Arc<Request>,
        path: PathBuf,
        raw_decoder: RawBackend,
        base_side: u32,
    ) -> Result<Pictured> {
        self.pool(&request, Lane::Interactive, Ask::Own("frame-picture"), move |e| {
            let words = e.engine.words();
            let presented = e.engine.load_preview(&path, raw_decoder).told(words)?;
            Ok(Pictured::of(Picture::of(presented), base_side))
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use deptheos_app::engine::present::{TILE_CORE, present_frame_map, present_rgb};
    use deptheos_app::{CompletedStack, Gray16Image};

    use super::*;
    use crate::tests::{now, req};

    /// `width`×`height` RGB with every pixel its own: no two tiles alike.
    fn patterned(width: u32, height: u32) -> PresentedImage {
        let pixels: Vec<u8> = (0..width * height)
            .flat_map(|i| [(i % 251) as u8, (i / 251 % 241) as u8, (i % 239) as u8])
            .collect();
        present_rgb(&pixels, width, height, |v| v)
    }

    /// From its tiles' cores, as the app draws them.
    fn from_tiles(picture: &Picture, level: u32) -> DisplayImage {
        let l = picture.levels()[level as usize];
        let (w, h) = (l.width as usize, l.height as usize);
        let mut bgra = vec![0u8; w * h * 4];
        for ty in 0..l.rows {
            for tx in 0..l.cols {
                let tile = picture.tile(level, tx, ty).unwrap();
                let (x0, y0) = ((tx * TILE_CORE) as usize, (ty * TILE_CORE) as usize);
                let core_w = (l.width - tx * TILE_CORE).min(TILE_CORE) as usize;
                let core_h = (l.height - ty * TILE_CORE).min(TILE_CORE) as usize;
                assert_eq!((tile.width as usize, tile.height as usize), (core_w + 2, core_h + 2));
                for y in 0..core_h {
                    let src = &tile.bgra[((y + 1) * tile.width as usize + 1) * 4..][..core_w * 4];
                    bgra[((y0 + y) * w + x0) * 4..][..core_w * 4].copy_from_slice(src);
                }
            }
        }
        DisplayImage { width: l.width, height: l.height, bgra, space: picture.image.space().into() }
    }

    /// Every level's tiles together are that level, odd sizes and short
    /// edge tiles too; the gutter repeats the edge pixel at the border and
    /// the neighbor's inside.
    #[test]
    fn a_pictures_tiles_make_up_its_levels() {
        let (width, height) = (2047, 1029);
        let image = patterned(width, height);
        let picture = Picture::of(image.clone());
        assert_eq!(picture.size(), PictureSize { width, height });
        let levels = picture.levels();
        assert_eq!(levels.len(), image.levels.len());
        assert_eq!(levels[0], PictureLevel { width, height, cols: 5, rows: 3, halvings: 0 });
        let half = PictureLevel { width: 1024, height: 515, cols: 3, rows: 2, halvings: 1 };
        assert_eq!(levels[1], half);
        for (k, level) in image.levels.iter().enumerate() {
            let tiled = from_tiles(&picture, k as u32);
            let whole = level.assemble();
            assert_eq!((tiled.width, tiled.height), (whole.width, whole.height), "level {k}");
            assert!(tiled.bgra == whole.pixels, "level {k}");
        }

        let full = image.levels[0].assemble();
        let px = |x: u32, y: u32| &full.pixels[((y * width + x) * 4) as usize..][..4];
        let corner = picture.tile(0, 4, 2).unwrap();
        assert_eq!((corner.width, corner.height), (7 + 2, 9 + 2));
        let at = |t: &DisplayImage, x: u32, y: u32| {
            t.bgra[((y * t.width + x) * 4) as usize..][..4].to_vec()
        };
        assert_eq!(at(&corner, corner.width - 1, 1), px(width - 1, 1020), "edge repeated");
        assert_eq!(at(&corner, 0, 1), px(2039, 1020), "neighbor's pixel in the gutter");
        assert_eq!(at(&corner, 1, 0), px(2040, 1019), "the row above in the gutter");

        assert!(picture.tile(0, 5, 0).is_none() && picture.tile(0, 0, 3).is_none());
        assert!(picture.tile(levels.len() as u32, 0, 0).is_none());
    }

    /// The depth map's picture is the stack's full size, its finest level
    /// half that, rounded up.
    #[test]
    fn the_depth_maps_picture_starts_at_half_size() {
        let (width, height) = (2051, 1201);
        let map = Gray16Image::from_fn(width, height, |x, y| image::Luma([(x * 31 + y) as u16]));
        let image = present_frame_map(&map, None);
        let picture = Picture::of(image.clone());
        assert_eq!(picture.size(), PictureSize { width, height });
        let levels = picture.levels();
        assert_eq!(
            levels[0],
            PictureLevel { width: 1026, height: 601, cols: 3, rows: 2, halvings: 1 }
        );
        assert_eq!((levels[1].width, levels[1].height, levels[1].halvings), (513, 301, 2));
        let tiled = from_tiles(&picture, 0);
        assert!(tiled.bgra == image.levels[0].assemble().pixels);
        let base = picture.base(256).unwrap();
        assert_eq!(base.width, 256);
    }

    /// The base is what the app has always been given, and a result's
    /// pictures (display and map) still serve after the result releases
    /// them.
    #[test]
    fn a_results_pictures_outlive_its_displays() {
        let dir = tempfile::tempdir().unwrap();
        let engine = crate::tests::engine(dir.path());
        let (width, height) = (1100, 530);
        let frame_map = Gray16Image::from_fn(width, height, |x, _| {
            image::Luma([if x < width / 2 { 0 } else { 65535 }])
        });
        let stack = CompletedStack {
            frame_map: Some(frame_map),
            ..CompletedStack::srgb(
                width,
                height,
                (0..width * height * 3).map(|i| (i * 7) as u16).collect(),
                vec!["a.png".into(), "b.png".into()],
            )
        };
        let result = StackResult::new(
            engine.engine.clone(),
            engine.engine.present_stack(stack, &deptheos_app::Adjustments::NEUTRAL).unwrap(),
        );
        let display = result.picture_of(&PictureOf::Display).unwrap().unwrap();
        let map = result.picture_of(&PictureOf::Map).unwrap().unwrap();
        for side in [64, 600, 4096] {
            let held = result.held(false).unwrap();
            let (old, new) = (level_image(&held, side).unwrap(), display.base(side).unwrap());
            assert_eq!((old.width, old.height), (new.width, new.height), "{side}");
            assert!(old.bgra == new.bgra, "{side}");
            let held = result.held(true).unwrap();
            let (old, new) = (level_image(&held, side).unwrap(), map.base(side).unwrap());
            assert!((old.width, old.height, &old.bgra) == (new.width, new.height, &new.bgra));
        }
        assert_eq!(map.levels()[0].halvings, 1);
        let lit = result.picture_of(&PictureOf::LitMap { frame: "a.png".into() }).unwrap().unwrap();
        assert_eq!(lit.size(), map.size());
        assert!(
            result
                .picture_of(&PictureOf::LitMap { frame: "elsewhere.png".into() })
                .unwrap()
                .is_none()
        );

        let before = display.tile(0, 2, 1).unwrap();
        result.release_displays();
        // Remade from the stack, the same pixels as the one let go of.
        let (again, map_again) = (
            result.picture_of(&PictureOf::Display).unwrap().unwrap(),
            result.picture_of(&PictureOf::Map).unwrap().unwrap(),
        );
        assert!(!Arc::ptr_eq(&again.image, &display.image), "made again, not held");
        assert_eq!((again.size(), again.levels()), (display.size(), display.levels()));
        assert!(again.tile(0, 2, 1).unwrap().bgra == before.bgra);
        assert!(again.base(4096).unwrap().bgra == display.base(4096).unwrap().bgra);
        assert_eq!((map_again.size(), map_again.levels()), (map.size(), map.levels()));
        assert!(map_again.tile(0, 1, 0).unwrap().bgra == map.tile(0, 1, 0).unwrap().bgra);
        let after = display.tile(0, 2, 1).unwrap();
        assert!(before.bgra == after.bgra);
        assert_eq!(display.size(), PictureSize { width, height });
        assert!(map.tile(0, 1, 0).is_some() && map.base(4096).is_some());
        drop(result);
        assert!(display.base(512).is_some(), "the picture alone holds its pyramid");
    }

    /// A frame alone is the engine's own decode of it, at its full size.
    #[test]
    fn a_frame_alone_is_the_engines_decode() {
        let dir = tempfile::tempdir().unwrap();
        let engine = crate::tests::engine(dir.path());
        let path = dir.path().join("frame.png");
        image::RgbImage::from_fn(700, 300, |x, y| image::Rgb([x as u8, y as u8, 9]))
            .save(&path)
            .unwrap();
        let decode = |path: &std::path::Path| {
            now(engine.clone().frame_picture(req(), PathBuf::from(path), RawBackend::Apple, 512))
        };
        let Pictured { picture, base } = decode(&path).unwrap();
        assert_eq!(picture.size(), PictureSize { width: 700, height: 300 });
        assert_eq!(base.map(|b| (b.width, b.height)), Some((512, 219)), "its base, made with it");
        let tile = picture.tile(0, 1, 0).unwrap();
        // Level pixel (510, 0) at gutter offset (1, 1): BGRA of (510 % 256, 0, 9).
        let i = ((tile.width + 1) * 4) as usize;
        assert_eq!(&tile.bgra[i..i + 4], &[9, 0, (510 % 256) as u8, 255]);
        assert!(decode(&dir.path().join("none.png")).is_err());
    }
}
