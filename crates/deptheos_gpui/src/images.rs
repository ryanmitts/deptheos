//! CPU display images (`deptheos_app::engine::present`), wrapped for GPUI's renderer.
//!
//! GPUI keeps a `RenderImage` in its sprite atlas from first paint until
//! `drop_image` (dropping the `Arc` leaks the GPU copy), so every
//! replacement goes through [`Atlas`].
//!
//! A display image is a grid of tiles per level ([`TILE_CORE`]). One shared
//! with its maker (paint's source) is copied into GPUI images a tile at a
//! time as painted ([`TileImage`]). Each visible tile is painted whole and
//! clipped to its core: a partial rect's texture coordinates would round to
//! whole texels, shifting the image at high zoom.
//!
//! GPUI samples linearly, blurring pixels zoomed far in. Above [`CRISP_FROM`]
//! a visible tile is painted from a pixel-repeated copy ([`Crisp`]), made off
//! the UI thread, kept only while on screen and under [`CRISP_BUDGET`]. Off
//! while retouching ([`Atlas::set_crisp`]): each patch would drop its copy,
//! leaving soft tiles under the brush beside sharp ones.
//!
//! A picture shows in the window's space ([`crate::color::shown`], read by
//! [`Atlas::begin_frame`]). A tile in another space is converted as it is
//! painted, so a space change converts only what is seen; in the same
//! space nothing is converted.

use std::sync::Arc;
use std::time::Duration;

use deptheos_app::engine::present::convert_display;
use deptheos_app::paint::DisplayPatch;
use deptheos_app::{CpuImage, PresentedImage, ScreenSpace, TILE_CORE, TiledLevel, level_index};
use gpui::{App, Bounds, Pixels, RenderImage, Window, point, px, size};
use image::{Frame, RgbaImage};

/// Atlas bytes above which tiles not painted lately are dropped; their CPU
/// bytes stay for re-upload.
const ATLAS_BUDGET: usize = deptheos_app::engine::memory::DISPLAY_ATLAS;
/// Frames a dropped image stays in the atlas: the GPU may still sample it,
/// and an upload reusing the region would race it.
const DROP_DELAY: u64 = 2;
/// How long a drop waits when the viewer draws no frames (off screen).
pub const DROP_AFTER: Duration = Duration::from_millis(250);
/// Device pixels per stored pixel above which a tile is drawn enlarged.
const CRISP_FROM: f32 = 2.0;
/// The largest enlargement: a 512 × 512 tile becomes 2048 × 2048 (16 MiB).
const CRISP_MAX: u32 = 4;
/// Enlarged copies' bytes at most, made or making; past it tiles draw as is.
const CRISP_BUDGET: usize = 192 << 20;

/// One displayable image: full size plus half-size levels, possibly
/// starting below full size.
pub struct TiledImage {
    pub width: u32,
    pub height: u32,
    first_level: u32,
    /// The pixels [`TileImage::Shared`] tiles copy from; moved into the tiles
    /// for an image handed over whole.
    pixels: Arc<PresentedImage>,
    levels: Vec<Level>,
    /// The space its pixels are in.
    source: ScreenSpace,
    /// The space its copied tiles and enlarged copies were made in.
    shown: ScreenSpace,
}

struct Level {
    width: u32,
    height: u32,
    cols: u32,
    rows: u32,
    tiles: Vec<Tile>,
}

struct Tile {
    image: TileImage,
    bytes: usize,
    /// The frame this tile was last painted in, while it is in the atlas.
    painted: Option<u64>,
    crisp: Crisp,
}

/// A tile's pixels as GPUI draws them.
///
/// Lifecycle: `Shared`, `Copied` or `Own`, changed only by [`Atlas`] and
/// [`TiledImage::apply`]. `Shared` → `Copied` (painted or enlarged, in the
/// space shown) → `Shared` (dropped from the atlas, or the space shown
/// changed). Any → `Own` (paint's new pixels). `Own` → `Own` (painted in
/// another space than its own: converted).
enum TileImage {
    /// Only in the image's shared pixels.
    Shared,
    /// Copied from the shared pixels, in the image's space shown; goes with
    /// its atlas copy.
    Copied(Arc<RenderImage>),
    /// Held only here (handed over whole, or a paint patch), in the space
    /// given: kept when its atlas copy goes.
    Own(Arc<RenderImage>, ScreenSpace),
}

impl Tile {
    fn own(image: CpuImage, space: ScreenSpace) -> Self {
        let bytes = image.pixels.len();
        Self::new(TileImage::Own(render_image(image), space), bytes)
    }

    fn new(image: TileImage, bytes: usize) -> Self {
        Self { image, bytes, painted: None, crisp: Crisp::None }
    }

    /// Its GPUI image, if it has one.
    fn made(&self) -> Option<&Arc<RenderImage>> {
        match &self.image {
            TileImage::Shared => None,
            TileImage::Copied(image) | TileImage::Own(image, _) => Some(image),
        }
    }

    /// Its GPUI image, copied out of `shared` (in `spaces.0`) into
    /// `spaces.1` if it has none.
    fn image(&mut self, shared: &CpuImage, spaces: (ScreenSpace, ScreenSpace)) -> Arc<RenderImage> {
        if let TileImage::Shared = self.image {
            tracing::debug!("display tile: shared -> copied");
            self.image = TileImage::Copied(render_in(shared.clone(), spaces.0, spaces.1));
        }
        self.made().expect("made above").clone()
    }
}

/// A tile's enlarged copy, for painting it zoomed far in.
///
/// Lifecycle: `None`, `Making` or `Made`, changed only by [`Atlas`] and
/// [`TiledImage::take_crisp`]. `None` → `Making` (on screen above
/// [`CRISP_FROM`], within [`CRISP_BUDGET`]) → `Made` (back at the factor
/// still wanted) → `None` (off screen a frame, another factor, painted
/// over, or image replaced). `Making` → `None` the same ways.
enum Crisp {
    None,
    /// Being made, `factor` times as large.
    Making {
        factor: u32,
    },
    /// Made; `painted` as for [`Tile::painted`].
    Made {
        factor: u32,
        image: Arc<RenderImage>,
        painted: Option<u64>,
    },
}

/// An enlarged copy to make: `source` `factor` times as large.
pub struct CrispJob {
    pub source: Arc<RenderImage>,
    pub factor: u32,
}

impl CrispJob {
    /// The copy. Runs off the UI thread.
    pub fn make(&self) -> Arc<RenderImage> {
        enlarge(&self.source, self.factor)
    }
}

/// One tile painted into one pane.
pub struct TileDraw {
    /// The tile's core on screen, within the pane: the content mask.
    pub mask: Bounds<Pixels>,
    /// Where the whole stored tile (core and gutter) lands.
    pub image: Bounds<Pixels>,
    pub data: Arc<RenderImage>,
    /// The coarser level's tiles under `mask`, painted if this one fails.
    pub fallback: Vec<TileDraw>,
}

impl TiledImage {
    /// Moved into the tiles when handed over whole, otherwise shared and
    /// copied a tile at a time as painted.
    pub fn from_presented(presented: impl Into<Arc<PresentedImage>>) -> Self {
        let mut pixels = presented.into();
        let (width, height, first_level) = (pixels.width(), pixels.height(), pixels.first_level);
        let source = pixels.space();
        let levels = match Arc::get_mut(&mut pixels) {
            Some(whole) => whole.levels.iter_mut().map(|l| Level::own(l, source)).collect(),
            None => pixels.levels.iter().map(Level::shared).collect(),
        };
        Self { width, height, first_level, pixels, levels, source, shown: source }
    }

    /// Show in the atlas's space: copies and enlarged copies made in
    /// another go (its own tiles convert as painted).
    fn follow(&mut self, atlas: &mut Atlas) {
        if self.shown == atlas.shown {
            return;
        }
        tracing::debug!("display image: shown in {:?} -> {:?}", self.shown, atlas.shown);
        self.shown = atlas.shown;
        for tile in self.tiles_mut() {
            atlas.drop_crisp(tile);
            if let TileImage::Copied(_) = tile.image {
                tracing::debug!("display tile: copied -> shared");
                atlas.unload(tile);
            }
        }
    }

    /// The tiles to paint for the image drawn at `rect`, clipped to `clip`,
    /// at `scale` device pixels per full-size pixel. Tiles outside `clip` are
    /// left out, since GPUI uploads an image before culling it.
    pub fn draws(
        &mut self,
        atlas: &mut Atlas,
        rect: Bounds<Pixels>,
        clip: Bounds<Pixels>,
        scale: f32,
    ) -> Vec<TileDraw> {
        self.follow(atlas);
        let spaces = (self.source, self.shown);
        let k = level_index(scale, self.first_level, self.levels.len());
        // Screen pixels per stored pixel of level `k`.
        let texel = scale * (1u32 << (self.first_level as usize + k).min(31)) as f32;
        let factor = if texel > CRISP_FROM && !atlas.crisp_off {
            (texel.floor() as u32).min(CRISP_MAX)
        } else {
            1
        };
        let pixels = &self.pixels.levels;
        let mut draws = self.levels[k].draws(atlas, &pixels[k], spaces, rect, clip, factor);
        // Counted as painted though drawn only if a finer tile fails: the
        // budget runs slightly early, but a fallback in the atlas is counted.
        if let Some(coarser) = self.levels.get_mut(k + 1) {
            for draw in &mut draws {
                draw.fallback = coarser.draws(atlas, &pixels[k + 1], spaces, rect, draw.mask, 1);
            }
        }
        draws
    }

    /// Swap in one tile's new pixels (paint), in the image's space,
    /// freeing the old atlas copy after the delay; a patch for another
    /// layout is ignored.
    pub fn apply(&mut self, atlas: &mut Atlas, patch: DisplayPatch) {
        let Some(level) = self.levels.get_mut(patch.level) else { return };
        if patch.tx >= level.cols || patch.ty >= level.rows {
            return;
        }
        let tile = &mut level.tiles[(patch.ty * level.cols + patch.tx) as usize];
        // Its enlarged copy goes with it, and one being made is not taken.
        atlas.retire(std::mem::replace(tile, Tile::own(patch.tile, self.source)));
    }

    /// Take `job`'s enlarged copy if its tile still wants it; `true` if taken.
    pub fn take_crisp(&mut self, job: &CrispJob, image: Arc<RenderImage>) -> bool {
        let made = |t: &&mut Tile| t.made().is_some_and(|i| Arc::ptr_eq(i, &job.source));
        let Some(tile) = self.tiles_mut().find(made) else {
            return false;
        };
        match tile.crisp {
            Crisp::Making { factor } if factor == job.factor => {
                tracing::debug!("enlarged tile: making -> made (×{factor})");
                tile.crisp = Crisp::Made { factor, image, painted: None };
                true
            }
            _ => false,
        }
    }

    /// Which picture it is: its pixels' address (tests).
    #[cfg(test)]
    pub fn identity(&self) -> usize {
        Arc::as_ptr(&self.pixels) as usize
    }

    /// Every tile's pixels, full size level first (tests).
    #[cfg(test)]
    pub fn pixels(&self) -> Vec<u8> {
        let shared = self.pixels.levels.iter().flat_map(|l| &l.tiles);
        let bytes = |(t, s): (&Tile, &CpuImage)| match t.made() {
            Some(image) => image.as_bytes(0).unwrap_or_default().to_vec(),
            None => s.pixels.clone(),
        };
        self.tiles().zip(shared).flat_map(bytes).collect()
    }

    #[cfg(test)]
    fn tiles(&self) -> impl Iterator<Item = &Tile> {
        self.levels.iter().flat_map(|l| &l.tiles)
    }

    fn tiles_mut(&mut self) -> impl Iterator<Item = &mut Tile> {
        self.levels.iter_mut().flat_map(|l| &mut l.tiles)
    }
}

impl Level {
    /// `level`'s tiles, their pixels in `space` moved out of it.
    fn own(level: &mut TiledLevel, space: ScreenSpace) -> Self {
        let moved = |t: &mut CpuImage| CpuImage { pixels: std::mem::take(&mut t.pixels), ..*t };
        let tiles = level.tiles.iter_mut().map(|t| Tile::own(moved(t), space)).collect();
        Self::with(level, tiles)
    }

    /// `level`'s tiles, their pixels left in it to be copied when painted.
    fn shared(level: &TiledLevel) -> Self {
        let tile = |t: &CpuImage| Tile::new(TileImage::Shared, t.pixels.len());
        Self::with(level, level.tiles.iter().map(tile).collect())
    }

    fn with(level: &TiledLevel, tiles: Vec<Tile>) -> Self {
        Self { width: level.width, height: level.height, cols: level.cols, rows: level.rows, tiles }
    }

    /// Tile `(tx, ty)`'s core and stored rect on screen; all edges come
    /// from [`edge`], so neighboring cores meet exactly.
    fn tile_rects(
        &self,
        rect: Bounds<Pixels>,
        tx: u32,
        ty: u32,
    ) -> (Bounds<Pixels>, Bounds<Pixels>) {
        let (x0, x1) = (self.edge_x(rect, tx), self.edge_x(rect, tx + 1));
        let (y0, y1) = (self.edge_y(rect, ty), self.edge_y(rect, ty + 1));
        let core = Bounds::from_corners(point(x0, y0), point(x1, y1));
        // One level pixel of gutter each side.
        let (gx, gy) = (rect.size.width / self.width as f32, rect.size.height / self.height as f32);
        let stored = Bounds {
            origin: point(x0 - gx, y0 - gy),
            size: size(x1 - x0 + gx * 2.0, y1 - y0 + gy * 2.0),
        };
        (core, stored)
    }

    fn edge_x(&self, rect: Bounds<Pixels>, i: u32) -> Pixels {
        edge(rect.origin.x, rect.size.width, self.width, i)
    }

    fn edge_y(&self, rect: Bounds<Pixels>, i: u32) -> Pixels {
        edge(rect.origin.y, rect.size.height, self.height, i)
    }

    /// The tile columns and rows that meet `clip`, as half-open ranges.
    fn visible(&self, rect: Bounds<Pixels>, clip: Bounds<Pixels>) -> (u32, u32, u32, u32) {
        let span = |from: Pixels, len: Pixels, level: u32, count: u32, lo: Pixels, hi: Pixels| {
            let tile = f32::from(len) / level as f32 * TILE_CORE as f32;
            let at =
                |p: Pixels| (f32::from(p - from) / tile).floor().clamp(0.0, count as f32) as u32;
            (at(lo), (at(hi) + 1).min(count))
        };
        let (x0, x1) =
            span(rect.origin.x, rect.size.width, self.width, self.cols, clip.left(), clip.right());
        let (y0, y1) = span(
            rect.origin.y,
            rect.size.height,
            self.height,
            self.rows,
            clip.top(),
            clip.bottom(),
        );
        (x0, x1, y0, y1)
    }

    /// The tiles meeting `clip`, from `factor`× copies where made, shown
    /// in `spaces.1` from pixels in `spaces.0`.
    fn draws(
        &mut self,
        atlas: &mut Atlas,
        pixels: &TiledLevel,
        spaces: (ScreenSpace, ScreenSpace),
        rect: Bounds<Pixels>,
        clip: Bounds<Pixels>,
        factor: u32,
    ) -> Vec<TileDraw> {
        let (x0, x1, y0, y1) = self.visible(rect, clip);
        let mut draws = Vec::new();
        for ty in y0..y1 {
            for tx in x0..x1 {
                let (core, stored) = self.tile_rects(rect, tx, ty);
                let mask = core.intersect(&clip);
                if mask.is_empty() {
                    continue;
                }
                let i = (ty * self.cols + tx) as usize;
                let (tile, shared) = (&mut self.tiles[i], &pixels.tiles[i]);
                atlas.convert_own(tile, spaces.1);
                let data = match atlas.crisp(tile, shared, spaces, factor) {
                    Some(crisp) => crisp,
                    None => {
                        atlas.touch(tile);
                        tile.image(shared, spaces)
                    }
                };
                draws.push(TileDraw { mask, image: stored, data, fallback: Vec::new() });
            }
        }
        draws
    }
}

/// Screen position of tile edge `i` of a level `count` pixels long over
/// `len` screen pixels; the last edge is the image's.
fn edge(origin: Pixels, len: Pixels, count: u32, i: u32) -> Pixels {
    let at = (i * TILE_CORE).min(count);
    origin + px(f32::from(len) * at as f32 / count as f32)
}

/// The viewer's share of GPUI's atlas: an LRU of painted tiles under
/// [`ATLAS_BUDGET`], with drops delayed by [`DROP_DELAY`].
#[derive(Default)]
pub struct Atlas {
    frame: u64,
    /// Bytes of the tiles in the atlas: painted, and not evicted since.
    uploaded: usize,
    /// Images to drop: the frame each was let go in, and its sequence number.
    drops: Vec<(u64, u64, Arc<RenderImage>)>,
    /// The number the next drop gets.
    dropped: u64,
    /// A picture was let go since memory was last given back
    /// ([`crate::give_back_memory`]).
    let_go: bool,
    /// Bytes of the enlarged copies made or being made.
    crisp: usize,
    /// Enlarged copies asked for this frame, for the viewer to have made.
    jobs: Vec<CrispJob>,
    /// No enlarged copies are drawn (retouching).
    crisp_off: bool,
    /// The space pictures show in, read each frame.
    shown: ScreenSpace,
}

impl Atlas {
    /// Start a frame: drop what has waited long enough, asking for frames
    /// while drops are pending.
    pub fn begin_frame(&mut self, window: &mut Window, cx: &mut App) {
        self.frame += 1;
        self.shown = crate::color::shown();
        let frame = self.frame;
        let (due, waiting) = std::mem::take(&mut self.drops)
            .into_iter()
            .partition(|(at, _, _)| frame >= at + DROP_DELAY);
        self.drops = waiting;
        for (_, _, image) in due {
            cx.drop_image(image, Some(window));
        }
        if !self.drops.is_empty() {
            window.request_animation_frame();
        }
    }

    /// A mark for [`Self::flush_before`] after [`DROP_AFTER`].
    pub fn drop_mark(&self) -> u64 {
        self.dropped
    }

    /// Drop what waited at `mark`, frames drawn or not: off screen no frame
    /// comes to drop it. Returns whether any still wait.
    pub fn flush_before(&mut self, mark: u64, cx: &mut App) -> bool {
        let (due, waiting) =
            std::mem::take(&mut self.drops).into_iter().partition(|(_, n, _)| *n < mark);
        self.drops = waiting;
        for (_, _, image) in due {
            cx.drop_image(image, None);
        }
        if self.drops.is_empty() && std::mem::take(&mut self.let_go) {
            crate::give_back_memory();
        }
        !self.drops.is_empty()
    }

    /// Drops wait, or memory waits to be given back.
    pub fn has_drops(&self) -> bool {
        !self.drops.is_empty() || self.let_go
    }

    /// Drop `image` from GPUI's atlas once it is safe to.
    fn queue(&mut self, image: Arc<RenderImage>) {
        self.drops.push((self.frame, self.dropped, image));
        self.dropped += 1;
    }

    /// Drop a whole image painted outside the tiles (Adjust's flat
    /// picture) once it is safe to.
    pub fn let_go(&mut self, image: Arc<RenderImage>) {
        self.let_go = true;
        self.queue(image);
    }

    /// `tile` is painted this frame.
    fn touch(&mut self, tile: &mut Tile) {
        if tile.painted.is_none() {
            self.uploaded += tile.bytes;
            // Evicted but not dropped yet: keep it, it is in use again.
            if let Some(made) = tile.made()
                && !self.drops.is_empty()
            {
                self.drops.retain(|(_, _, image)| !Arc::ptr_eq(image, made));
            }
        }
        tile.painted = Some(self.frame);
    }

    /// The space pictures show in this frame.
    pub fn shown(&self) -> ScreenSpace {
        self.shown
    }

    /// Convert `tile`'s own pixels to `shown` if they are in another space,
    /// its old copies let go.
    fn convert_own(&mut self, tile: &mut Tile, shown: ScreenSpace) {
        let TileImage::Own(image, space) = &tile.image else { return };
        let Some(converted) = reshown(image, *space, shown) else { return };
        tracing::debug!("display tile: own in {space:?} -> own in {shown:?}");
        self.drop_crisp(tile);
        let old = std::mem::replace(&mut tile.image, TileImage::Own(converted, shown));
        if let TileImage::Own(old, _) = old {
            self.queue(old);
        }
    }

    /// `tile`'s `factor`× copy if made; else request it (within
    /// [`CRISP_BUDGET`]), dropping one at another factor. `None` for 1.
    fn crisp(
        &mut self,
        tile: &mut Tile,
        shared: &CpuImage,
        spaces: (ScreenSpace, ScreenSpace),
        factor: u32,
    ) -> Option<Arc<RenderImage>> {
        if factor <= 1 {
            return None;
        }
        match &mut tile.crisp {
            Crisp::Made { factor: made, image, painted } if *made == factor => {
                *painted = Some(self.frame);
                return Some(image.clone());
            }
            Crisp::Making { factor: making } if *making == factor => return None,
            Crisp::None | Crisp::Making { .. } | Crisp::Made { .. } => {}
        }
        self.drop_crisp(tile);
        let bytes = tile.bytes * (factor * factor) as usize;
        if self.crisp + bytes <= CRISP_BUDGET {
            self.crisp += bytes;
            tracing::debug!("enlarged tile: none -> making (×{factor})");
            tile.crisp = Crisp::Making { factor };
            self.jobs.push(CrispJob { source: tile.image(shared, spaces), factor });
        }
        None
    }

    /// Forget `tile`'s enlarged copy, freeing its atlas copy (after
    /// [`DROP_DELAY`]) if painted.
    fn drop_crisp(&mut self, tile: &mut Tile) {
        let factor = match std::mem::replace(&mut tile.crisp, Crisp::None) {
            Crisp::None => return,
            Crisp::Making { factor } => {
                tracing::debug!("enlarged tile: making -> none (×{factor})");
                factor
            }
            Crisp::Made { factor, image, painted } => {
                tracing::debug!("enlarged tile: made -> none (×{factor})");
                if painted.is_some() {
                    self.queue(image);
                }
                factor
            }
        };
        self.crisp -= tile.bytes * (factor * factor) as usize;
    }

    /// Whether far-zoomed tiles use enlarged copies; off, they go at the
    /// next [`Self::evict`].
    pub fn set_crisp(&mut self, on: bool) {
        self.crisp_off = !on;
    }

    /// The enlarged copies asked for since the last call.
    pub fn take_jobs(&mut self) -> Vec<CrispJob> {
        std::mem::take(&mut self.jobs)
    }

    /// Drop enlarged copies not painted this frame or the last, and those
    /// being made for tiles not painted then; then, over budget, the tiles
    /// painted longest ago (never those two frames).
    pub fn evict<'a>(&mut self, images: impl IntoIterator<Item = &'a mut TiledImage>) {
        let recent = self.frame.saturating_sub(1);
        let mut tiles: Vec<&mut Tile> = images.into_iter().flat_map(|i| i.tiles_mut()).collect();
        for tile in tiles.iter_mut() {
            let stale = match tile.crisp {
                Crisp::None => false,
                // Its job finds the tile by its copy, which the pass below
                // may let go: the job then can't be taken.
                Crisp::Making { .. } => tile.painted.is_none_or(|f| f < recent),
                Crisp::Made { painted, .. } => painted.is_none_or(|f| f < recent),
            };
            if stale {
                self.drop_crisp(tile);
            }
        }
        if self.uploaded <= ATLAS_BUDGET {
            return;
        }
        let mut old: Vec<&mut Tile> =
            tiles.into_iter().filter(|t| t.painted.is_some_and(|f| f < recent)).collect();
        old.sort_by_key(|t| t.painted);
        for tile in old {
            if self.uploaded <= ATLAS_BUDGET {
                break;
            }
            self.unload(tile);
        }
    }

    /// Take `tile` out of the atlas: a copy goes back to the shared pixels,
    /// its own pixels are kept for upload again.
    fn unload(&mut self, tile: &mut Tile) {
        if tile.painted.take().is_some() {
            self.uploaded -= tile.bytes;
        }
        match std::mem::replace(&mut tile.image, TileImage::Shared) {
            TileImage::Copied(image) => {
                tracing::debug!("display tile: copied -> shared");
                self.queue(image);
            }
            TileImage::Own(image, space) => {
                self.queue(image.clone());
                tile.image = TileImage::Own(image, space);
            }
            TileImage::Shared => {}
        }
    }

    /// Free `image`'s atlas copies (after [`DROP_DELAY`]).
    fn release(&mut self, image: TiledImage) {
        self.let_go = true;
        for tile in image.levels.into_iter().flat_map(|l| l.tiles) {
            self.retire(tile);
        }
    }

    /// Free a tile's atlas copies (after [`DROP_DELAY`]); a copy still being
    /// made is not taken.
    fn retire(&mut self, mut tile: Tile) {
        self.drop_crisp(&mut tile);
        if tile.painted.is_some() {
            self.uploaded -= tile.bytes;
        }
        if let TileImage::Copied(image) | TileImage::Own(image, _) = tile.image {
            self.queue(image);
        }
    }

    /// Swap in a new image, releasing the old one.
    pub fn replace(&mut self, slot: &mut Option<TiledImage>, new: Option<TiledImage>) {
        if let Some(old) = slot.take() {
            self.release(old);
        }
        *slot = new;
    }

    #[cfg(test)]
    fn uploaded(&self) -> usize {
        self.uploaded
    }
}

/// `source` (BGRA) `factor` times as large, each pixel repeated.
fn enlarge(source: &RenderImage, factor: u32) -> Arc<RenderImage> {
    let size = source.size(0);
    let (w, h) = (size.width.0.max(0) as usize, size.height.0.max(0) as usize);
    let f = factor.max(1) as usize;
    let bytes = source.as_bytes(0).unwrap_or_default();
    let row = w * f * 4;
    let mut out = vec![0u8; row * h * f];
    for y in 0..h.min(bytes.len() / (w * 4).max(1)) {
        let from = &bytes[y * w * 4..(y + 1) * w * 4];
        let first = y * f * row;
        let dest = &mut out[first..first + row];
        for (x, pixel) in from.as_chunks::<4>().0.iter().enumerate() {
            for k in 0..f {
                dest[(x * f + k) * 4..(x * f + k + 1) * 4].copy_from_slice(pixel);
            }
        }
        for r in 1..f {
            out.copy_within(first..first + row, first + r * row);
        }
    }
    let buffer = RgbaImage::from_raw((w * f) as u32, (h * f) as u32, out)
        .expect("an enlarged tile is its size times 4 bytes");
    Arc::new(RenderImage::new(vec![Frame::new(buffer)]))
}

/// Wraps the BGRA8 straight-alpha bytes `present` wrote, which is GPUI's
/// `RenderImage` layout, without conversion.
fn render_image(image: CpuImage) -> Arc<RenderImage> {
    let buffer = RgbaImage::from_raw(image.width, image.height, image.pixels)
        .expect("display images are width*height*4 bytes");
    Arc::new(RenderImage::new(vec![Frame::new(buffer)]))
}

/// `image`, in `from`, as GPUI draws it in `to`; converted only if they
/// differ.
pub fn render_in(mut image: CpuImage, from: ScreenSpace, to: ScreenSpace) -> Arc<RenderImage> {
    convert_display(&mut image.pixels, from, to);
    render_image(image)
}

/// An sRGB picture the app made or read (a thumbnail, the Library's
/// picture) as GPUI draws it in `to`.
pub fn render_srgb(image: CpuImage, to: ScreenSpace) -> Arc<RenderImage> {
    render_in(image, ScreenSpace::Srgb, to)
}

/// `image`, in `from`, made again in `to`; `None` when they are the same.
pub fn reshown(
    image: &RenderImage,
    from: ScreenSpace,
    to: ScreenSpace,
) -> Option<Arc<RenderImage>> {
    if from == to {
        return None;
    }
    let size = image.size(0);
    let (width, height) = (size.width.0.max(0) as u32, size.height.0.max(0) as u32);
    let pixels = image.as_bytes(0).unwrap_or_default().to_vec();
    Some(render_in(CpuImage { width, height, pixels }, from, to))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(w: u32, h: u32) -> TiledImage {
        let pixels = vec![0; (w * h * 4) as usize];
        TiledImage::from_presented(PresentedImage::single(CpuImage { width: w, height: h, pixels }))
    }

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds { origin: point(px(x), px(y)), size: size(px(w), px(h)) }
    }

    #[test]
    fn cores_tile_the_image_without_seams() {
        // 3×2 tiles of 1100×600 at 2.5× from a fractional origin: cores share
        // edges and cover the image rect exactly.
        let mut img = image(1100, 600);
        let mut atlas = Atlas::default();
        let at = rect(10.3, -7.7, 2750.0, 1500.0);
        let draws = img.draws(&mut atlas, at, at, 2.5);
        assert_eq!(draws.len(), 6);
        let level = &img.levels[0];
        for ty in 0..level.rows {
            for tx in 0..level.cols {
                let (core, stored) = level.tile_rects(at, tx, ty);
                if tx + 1 < level.cols {
                    assert_eq!(core.right(), level.tile_rects(at, tx + 1, ty).0.left());
                }
                if ty + 1 < level.rows {
                    assert_eq!(core.bottom(), level.tile_rects(at, tx, ty + 1).0.top());
                }
                // The stored tile is the core plus one level pixel (2.5 px) each side.
                assert!((f32::from(core.left() - stored.left()) - 2.5).abs() < 1e-3);
                assert!((f32::from(stored.bottom() - core.bottom()) - 2.5).abs() < 1e-3);
            }
        }
        let (first, last) = (level.tile_rects(at, 0, 0).0, level.tile_rects(at, 2, 1).0);
        assert_eq!(Bounds::from_corners(first.origin, last.bottom_right()), at);
    }

    #[test]
    fn only_tiles_meeting_the_clip_are_drawn() {
        // 1600 % over 4×4 tiles: a 100 px pane sees one tile, or two across
        // an edge (510 × 16 = 8160 px).
        let mut img = image(2040, 2040);
        let mut atlas = Atlas::default();
        let at = rect(0.0, 0.0, 2040.0 * 16.0, 2040.0 * 16.0);
        assert_eq!(img.draws(&mut atlas, at, rect(9000.0, 9000.0, 100.0, 100.0), 16.0).len(), 1);
        let straddle = img.draws(&mut atlas, at, rect(8100.0, 9000.0, 100.0, 100.0), 16.0);
        assert_eq!(straddle.len(), 2);
        assert!(straddle.iter().all(|d| d.mask.size.width <= px(100.0)));
        // Outside the image entirely: nothing.
        assert!(img.draws(&mut atlas, at, rect(-500.0, 0.0, 100.0, 100.0), 16.0).is_empty());
    }

    #[test]
    fn eviction_keeps_recent_tiles_and_releases_account() {
        // 9 × 9 tiles of 1 MiB (512² BGRA): 81 MiB per full paint.
        let side = TILE_CORE * 9;
        let mut images =
            [image(side, side), image(side, side), image(side, side), image(side, side)];
        let mut atlas = Atlas::default();
        let at = rect(0.0, 0.0, side as f32, side as f32);
        for img in &mut images {
            atlas.frame += 1;
            img.draws(&mut atlas, at, at, 1.0);
        }
        let per_image = images[0].tiles().map(|t| t.bytes).sum::<usize>();
        assert_eq!(atlas.uploaded(), 4 * per_image);
        atlas.evict(images.iter_mut());
        assert!(atlas.uploaded() <= ATLAS_BUDGET);
        // Only the oldest image loses tiles, as many as needed:
        // 4 × 81 MiB − 256 MiB is 68 of its 81.
        assert!(images[1..].iter().flat_map(|i| i.tiles()).all(|t| t.painted.is_some()));
        assert_eq!(images[0].tiles().filter(|t| t.painted.is_none()).count(), 68);
        // Painting an evicted tile again cancels its pending drop.
        let pending = atlas.drops.len();
        atlas.frame += 1;
        images[0].draws(&mut atlas, at, rect(0.0, 0.0, 1.0, 1.0), 1.0);
        assert_eq!(atlas.drops.len(), pending - 1);
        let [a, b, c, d] = images;
        for img in [a, b, c, d] {
            atlas.release(img);
        }
        assert_eq!(atlas.uploaded(), 0);
    }

    fn copied(img: &TiledImage) -> usize {
        img.tiles().filter(|t| matches!(t.image, TileImage::Copied(_))).count()
    }

    /// An image its maker keeps is shared: only painted tiles are copied,
    /// each going with its atlas copy. One handed over whole is moved in.
    #[test]
    fn a_shared_image_copies_only_the_tiles_painted() {
        let side = TILE_CORE * 9;
        let pixels = (0..side * side * 4).map(|i| i as u8).collect();
        let shared =
            Arc::new(PresentedImage::single(CpuImage { width: side, height: side, pixels }));
        let mut images = [
            TiledImage::from_presented(shared.clone()),
            image(side, side),
            image(side, side),
            image(side, side),
        ];
        assert!(Arc::ptr_eq(&images[0].pixels, &shared), "the maker's pixels");
        assert!(images[1].tiles().all(|t| matches!(t.image, TileImage::Own(..))));
        assert_eq!(copied(&images[0]), 0);
        let mut atlas = Atlas::default();
        let at = rect(0.0, 0.0, side as f32, side as f32);
        images[0].draws(&mut atlas, at, rect(0.0, 0.0, 10.0, 10.0), 1.0);
        assert_eq!(copied(&images[0]), 1, "the one tile painted");
        assert_eq!(
            images[0].pixels(),
            shared.levels[0].tiles.iter().flat_map(|t| t.pixels.clone()).collect::<Vec<_>>()
        );

        // All painted, then the oldest evicted: 68 of 81 copies go, its
        // pixels stay shared.
        for img in &mut images {
            atlas.frame += 1;
            img.draws(&mut atlas, at, at, 1.0);
        }
        assert_eq!(copied(&images[0]), 81);
        atlas.evict(images.iter_mut());
        assert_eq!(copied(&images[0]), 81 - 68);
        assert!(images[1].tiles().all(|t| matches!(t.image, TileImage::Own(..))), "kept");
        assert_eq!(Arc::strong_count(&shared), 2);
        for img in images {
            atlas.release(img);
        }
        assert_eq!(Arc::strong_count(&shared), 1);
    }

    /// Make every enlarged copy asked for and give it to `img`.
    fn make_all(atlas: &mut Atlas, img: &mut TiledImage) -> usize {
        let jobs = atlas.take_jobs();
        for job in &jobs {
            assert!(img.take_crisp(job, job.make()));
        }
        jobs.len()
    }

    fn crisp_count(img: &TiledImage) -> usize {
        img.tiles().filter(|t| matches!(t.crisp, Crisp::Made { .. })).count()
    }

    #[test]
    fn an_enlarged_tile_repeats_each_pixel() {
        let pixels: Vec<u8> = (0..16).collect();
        let source = render_image(CpuImage { width: 2, height: 2, pixels });
        let big = enlarge(&source, 3);
        let size = big.size(0);
        assert_eq!((size.width.0, size.height.0), (6, 6));
        let bytes = big.as_bytes(0).unwrap();
        let at = |x: usize, y: usize| &bytes[(y * 6 + x) * 4..(y * 6 + x + 1) * 4];
        assert_eq!(at(0, 0), &[0, 1, 2, 3]);
        assert_eq!(at(2, 2), &[0, 1, 2, 3]);
        assert_eq!(at(3, 0), &[4, 5, 6, 7]);
        assert_eq!(at(5, 5), &[12, 13, 14, 15]);
        assert_eq!(at(0, 3), &[8, 9, 10, 11]);
    }

    /// Up to 2× nothing is enlarged; above it visible tiles use whole-factor
    /// copies (at most 4) once made, and off-screen tiles' copies go.
    #[test]
    fn enlarged_copies_follow_what_is_on_screen() {
        // 3 × 2 tiles.
        let mut img = image(1100, 600);
        let mut atlas = Atlas::default();
        let at = |scale: f32| rect(0.0, 0.0, 1100.0 * scale, 600.0 * scale);
        img.draws(&mut atlas, at(2.0), at(2.0), 2.0);
        assert!(atlas.take_jobs().is_empty(), "200%: as it is");

        atlas.frame += 1;
        let first = img.draws(&mut atlas, at(3.5), at(3.5), 3.5);
        assert!(
            first
                .iter()
                .all(|d| img.tiles().any(|t| t.made().is_some_and(|i| Arc::ptr_eq(i, &d.data))))
        );
        assert!(atlas.jobs.iter().all(|j| j.factor == 3));
        assert_eq!(make_all(&mut atlas, &mut img), 6);
        atlas.frame += 1;
        let drawn = img.draws(&mut atlas, at(3.5), at(3.5), 3.5);
        assert!(
            drawn
                .iter()
                .all(|d| !img.tiles().any(|t| t.made().is_some_and(|i| Arc::ptr_eq(i, &d.data)))),
            "drawn from the copies"
        );
        assert!(atlas.take_jobs().is_empty(), "made once");
        let tile_bytes: usize = img.tiles().map(|t| t.bytes).sum();
        assert_eq!(atlas.crisp, tile_bytes * 9);

        // Panned to the first tile: the others' copies go after a frame off screen.
        atlas.frame += 1;
        img.draws(&mut atlas, at(3.5), rect(0.0, 0.0, 100.0, 100.0), 3.5);
        atlas.evict([&mut img]);
        assert_eq!(crisp_count(&img), 6, "seen last frame: kept");
        atlas.frame += 1;
        img.draws(&mut atlas, at(3.5), rect(0.0, 0.0, 100.0, 100.0), 3.5);
        atlas.evict([&mut img]);
        assert_eq!(crisp_count(&img), 1);
        assert_eq!(atlas.crisp, img.tiles().next().unwrap().bytes * 9);

        // At 1600 % the factor is 4: the copy at 3 makes way for one at 4.
        atlas.frame += 1;
        img.draws(&mut atlas, at(16.0), rect(0.0, 0.0, 100.0, 100.0), 16.0);
        let jobs = atlas.take_jobs();
        assert_eq!(jobs.iter().map(|j| j.factor).collect::<Vec<_>>(), [4]);
        assert_eq!(crisp_count(&img), 0);

        atlas.release(img);
        assert_eq!(atlas.crisp, 0, "the image's copies, made or being made, go with it");
    }

    /// Retouching, nothing is enlarged and the copies made go.
    #[test]
    fn retouching_draws_tiles_as_they_are() {
        let mut img = image(1100, 600);
        let mut atlas = Atlas::default();
        let at = rect(0.0, 0.0, 1100.0 * 3.0, 600.0 * 3.0);
        img.draws(&mut atlas, at, at, 3.0);
        make_all(&mut atlas, &mut img);
        atlas.set_crisp(false);
        for _ in 0..2 {
            atlas.frame += 1;
            img.draws(&mut atlas, at, at, 3.0);
            atlas.evict([&mut img]);
        }
        assert!(atlas.take_jobs().is_empty());
        assert_eq!((crisp_count(&img), atlas.crisp), (0, 0));
    }

    /// Copies made or making stay within budget; the rest draw as is.
    #[test]
    fn enlarged_copies_stay_within_the_budget() {
        // 9 × 9 tiles of 1 MiB, at 4×: 16 MiB a copy, 12 within 192 MiB.
        let side = TILE_CORE * 9;
        let mut img = image(side, side);
        let mut atlas = Atlas::default();
        let at = rect(0.0, 0.0, side as f32 * 4.0, side as f32 * 4.0);
        let drawn = img.draws(&mut atlas, at, at, 4.0);
        assert_eq!(drawn.len(), 81);
        assert_eq!(atlas.jobs.len(), CRISP_BUDGET / (16 << 20));
        assert!(atlas.crisp <= CRISP_BUDGET);
        make_all(&mut atlas, &mut img);
        atlas.frame += 1;
        img.draws(&mut atlas, at, at, 4.0);
        assert!(atlas.take_jobs().is_empty(), "no room for more");
        assert!(atlas.crisp <= CRISP_BUDGET);
    }

    /// A tile painted over loses its copy at once; a copy of the old pixels
    /// being made is not taken.
    #[test]
    fn a_painted_tile_never_shows_a_stale_copy() {
        let mut img = image(1100, 600);
        let mut atlas = Atlas::default();
        let at = rect(0.0, 0.0, 1100.0 * 3.0, 600.0 * 3.0);
        img.draws(&mut atlas, at, at, 3.0);
        make_all(&mut atlas, &mut img);
        atlas.frame += 1;
        img.draws(&mut atlas, at, at, 3.0);
        let patch = || DisplayPatch {
            level: 0,
            tx: 0,
            ty: 0,
            tile: CpuImage { width: 512, height: 512, pixels: vec![9; 512 * 512 * 4] },
        };
        img.apply(&mut atlas, patch());
        assert!(matches!(img.levels[0].tiles[0].crisp, Crisp::None));
        assert_eq!(crisp_count(&img), 5);

        // Asked for again, then painted over before it is made.
        atlas.frame += 1;
        img.draws(&mut atlas, at, at, 3.0);
        let jobs = atlas.take_jobs();
        assert_eq!(jobs.len(), 1);
        img.apply(&mut atlas, patch());
        assert!(!img.take_crisp(&jobs[0], jobs[0].make()), "the old pixels' copy");
        let others: usize = img.levels[0].tiles[1..].iter().map(|t| t.bytes).sum();
        assert_eq!(atlas.crisp, others * 9, "the five other tiles' copies");
    }

    /// A shared tile whose copy is being made, evicted over budget, drops the
    /// copy asked for: it neither stays counted nor waits for a job that
    /// can't be taken, and is asked for again once back on screen.
    #[test]
    fn an_evicted_tile_drops_the_copy_being_made() {
        let side = TILE_CORE * 9;
        let pixels = vec![0; (side * side * 4) as usize];
        let shared =
            Arc::new(PresentedImage::single(CpuImage { width: side, height: side, pixels }));
        let mut images = [
            // Kept by its maker, so shared.
            TiledImage::from_presented(shared.clone()),
            image(side, side),
            image(side, side),
            image(side, side),
            image(side, side),
        ];
        let mut atlas = Atlas::default();
        let whole = rect(0.0, 0.0, side as f32, side as f32);
        let zoomed = rect(0.0, 0.0, side as f32 * 4.0, side as f32 * 4.0);
        // The first tile asked for at 4×, then every image painted at 1×.
        atlas.frame += 1;
        images[0].draws(&mut atlas, zoomed, rect(0.0, 0.0, 10.0, 10.0), 4.0);
        let jobs = atlas.take_jobs();
        assert_eq!(jobs.len(), 1);
        for img in &mut images[1..] {
            atlas.frame += 1;
            img.draws(&mut atlas, whole, whole, 1.0);
        }
        atlas.frame += 1;
        atlas.evict(images.iter_mut());
        let first = &images[0].levels[0].tiles[0];
        assert!(matches!(first.image, TileImage::Shared), "let go over budget");
        assert!(matches!(first.crisp, Crisp::None));
        assert_eq!(atlas.crisp, 0);
        assert!(!images[0].take_crisp(&jobs[0], jobs[0].make()));

        // Back on screen: asked for again, and taken.
        atlas.frame += 1;
        images[0].draws(&mut atlas, zoomed, rect(0.0, 0.0, 10.0, 10.0), 4.0);
        assert_eq!(make_all(&mut atlas, &mut images[0]), 1);
        assert_eq!(crisp_count(&images[0]), 1);
        for img in images {
            atlas.release(img);
        }
        assert_eq!(atlas.crisp, 0);
    }

    /// A picture all one BGRA `pixel`, in `space`; `shared` keeps a
    /// reference so its tiles are copied as painted.
    fn plain(pixel: [u8; 4], space: ScreenSpace, shared: bool) -> TiledImage {
        let (w, h) = (1100, 600);
        let pixels = pixel.repeat((w * h) as usize);
        let image = PresentedImage::single(CpuImage { width: w, height: h, pixels });
        let image = Arc::new(image.in_space(space));
        let held = shared.then(|| image.clone());
        let tiled = TiledImage::from_presented(image);
        drop(held);
        tiled
    }

    /// Every pixel of every tile, made or shared, as BGRA.
    fn colors(img: &TiledImage) -> Vec<[u8; 4]> {
        let mut colors: Vec<[u8; 4]> = img.pixels().as_chunks::<4>().0.to_vec();
        colors.dedup();
        colors
    }

    fn near(got: &[[u8; 4]], want: [u8; 4]) -> bool {
        got.iter().all(|g| g.iter().zip(want).all(|(a, b)| a.abs_diff(b) <= 1))
    }

    /// Display P3's red, (234, 51, 35), is sRGB's pure red.
    const P3_RED: [u8; 4] = [35, 51, 234, 255];
    const SRGB_RED: [u8; 4] = [0, 0, 255, 255];

    /// In the space shown, a picture's bytes are its own and no tile is made
    /// again; in another, each tile painted is converted.
    #[test]
    fn a_picture_in_another_space_converts_as_painted() {
        use ScreenSpace::{DisplayP3, Srgb};
        let at = rect(0.0, 0.0, 1100.0, 600.0);
        for shared in [false, true] {
            let mut same = plain(SRGB_RED, Srgb, shared);
            let mut atlas = Atlas::default();
            let first = same.draws(&mut atlas, at, at, 1.0);
            let again = same.draws(&mut atlas, at, at, 1.0);
            assert!(first.iter().zip(&again).all(|(a, b)| Arc::ptr_eq(&a.data, &b.data)));
            assert_eq!(colors(&same), [SRGB_RED]);
            assert!(atlas.drops.is_empty());

            let mut wide = plain(P3_RED, DisplayP3, shared);
            let drawn = wide.draws(&mut atlas, at, at, 1.0);
            let bytes = drawn[0].data.as_bytes(0).unwrap();
            assert!(near(bytes.as_chunks::<4>().0, SRGB_RED), "{:?}", &bytes[..4]);
            assert!(near(&colors(&wide), SRGB_RED), "{shared}: {:?}", colors(&wide));
        }
    }

    /// When the space shown changes, the tiles painted are converted, their
    /// old copies let go, and back again they return to their colors.
    #[test]
    fn a_space_change_converts_the_tiles_held() {
        use ScreenSpace::{DisplayP3, Srgb};
        let at = rect(0.0, 0.0, 1100.0 * 3.0, 600.0 * 3.0);
        for shared in [false, true] {
            let mut img = plain(SRGB_RED, Srgb, shared);
            let mut atlas = Atlas::default();
            img.draws(&mut atlas, at, at, 3.0);
            make_all(&mut atlas, &mut img);
            atlas.frame += 1;
            img.draws(&mut atlas, at, at, 3.0);
            assert_eq!(crisp_count(&img), 6);
            let uploaded = atlas.uploaded();

            atlas.frame += 1;
            atlas.shown = DisplayP3;
            img.draws(&mut atlas, at, at, 3.0);
            assert_eq!(crisp_count(&img), 0, "enlarged in the old space");
            assert!(near(&colors(&img), P3_RED), "{shared}: {:?}", colors(&img));
            assert_eq!(atlas.uploaded(), uploaded);
            assert!(atlas.drops.len() >= 6 + 6, "the old tiles and their copies");
            make_all(&mut atlas, &mut img);
            assert_eq!(crisp_count(&img), 6, "made again from the converted tiles");

            atlas.frame += 1;
            atlas.shown = Srgb;
            img.draws(&mut atlas, at, at, 3.0);
            assert!(near(&colors(&img), SRGB_RED), "{shared}: {:?}", colors(&img));
            atlas.release(img);
            assert_eq!((atlas.uploaded(), atlas.crisp), (0, 0));
        }
    }

    /// Paint's new pixels are in the picture's space, and show in the
    /// window's.
    #[test]
    fn a_patch_shows_in_the_space_shown() {
        let mut img = plain(SRGB_RED, ScreenSpace::Srgb, false);
        let mut atlas = Atlas { shown: ScreenSpace::DisplayP3, ..Atlas::default() };
        let at = rect(0.0, 0.0, 1100.0, 600.0);
        img.draws(&mut atlas, at, at, 1.0);
        let pixels = SRGB_RED.repeat(512 * 512);
        let tile = CpuImage { width: 512, height: 512, pixels };
        img.apply(&mut atlas, DisplayPatch { level: 0, tx: 0, ty: 0, tile });
        img.draws(&mut atlas, at, at, 1.0);
        assert!(near(&colors(&img), P3_RED), "{:?}", colors(&img));
    }
}
