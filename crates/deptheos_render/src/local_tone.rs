//! The local tone: the sliders that read neighbors, each a gain in stops
//! read from planes of the picture's log brightness. Highlights and
//! Shadows read a base, an edge-aware filter of it; Clarity its own guided
//! filter; Texture and Sharpening a blur each. The planes and the laws are
//! lightcraft's as they stand (MIT OR Apache-2.0): the planes are
//! craft-libs b2d2a1e94 `crates/develop/src/local.rs` (`tone_base`,
//! `detail_free`, `median_hv`, `guided_fast`, `texture_sigma`,
//! `sharpen_gain`) with its resampling and blurs
//! (`crates/develop/src/raster/resample.rs`,
//! `crates/develop/src/raster/blur.rs`); the laws are lightcraft 5bdb4e7a6
//! `crates/pipeline/src/finish.rs` (`local_tone_delta`, `tone_slider`,
//! `texture_slider`) and, for Sharpening's detail term, craft-libs
//! b2d2a1e94 `crates/develop/src/finish.rs` (`sharpen_term`,
//! `sharpen_limit`).
//!
//! The planes are computed for a rectangle at a time from the log
//! brightness around it and from planes of the whole frame far smaller than
//! it ([`Planes`]), each value from the same inputs in the same order as
//! the whole plane's, so a region, a band and the whole picture agree bit
//! for bit.

use std::collections::HashMap;
use std::ops::Range;

use rayon::prelude::*;

/// The base's reach, as a fraction of the long edge (at least 1 px).
pub const BASE_SIGMA: f32 = 0.015;
/// The radius of the median the base is taken of, as a fraction of the
/// long edge.
pub const BASE_DETAIL: f64 = 0.0015;
/// The largest median radius, in pixels.
pub const BASE_DETAIL_MAX: usize = 24;
/// The guided filter's epsilon, in stops squared.
pub const BASE_EPS: f32 = 0.35;
/// The coarse median's downscale.
pub const BASE_COARSE_STEP: usize = 3;
/// How much a full Shadows eases off: it acts at `1 / (1 + SHADOWS_EASE)`.
pub const SHADOWS_EASE: f64 = 1.33;
/// Clarity's reach, as a fraction of the long edge (at least 1 px), and
/// its guided filter's epsilon, in stops squared.
pub const CLARITY_SIGMA: f32 = 0.012;
pub const CLARITY_EPS: f32 = 0.8;
/// Clarity's law: its gain in stops per stop of detail, the detail's
/// limit, and the width (stops from middle gray) of the midtones where it
/// acts fully; outside them it keeps `CLARITY_FLOOR` of its strength.
pub const CLARITY_STRENGTH: f32 = 0.85;
pub const CLARITY_LIMIT: f32 = 2.5;
pub const CLARITY_MIDTONES: f32 = 3.2;
pub const CLARITY_FLOOR: f32 = 0.35;
/// Texture's reach, as a fraction of the long edge, and its least, in
/// pixels.
pub const TEXTURE_SIGMA: f32 = 0.0018;
pub const TEXTURE_SIGMA_MIN: f32 = 0.6;
/// How much of its strength a negative Texture keeps.
pub const NEGATIVE_TEXTURE: f64 = 0.67;
/// Texture's law: its gain in stops per stop of detail, the detail's
/// limit, and how far it is tamed between the edges (stops of detail).
pub const TEXTURE_STRENGTH: f32 = 1.1;
pub const TEXTURE_LIMIT: f32 = 1.0;
pub const TEXTURE_TAME: f32 = 0.6;
pub const TEXTURE_TAME_EDGES: [f32; 2] = [0.4, 1.6];
/// Sharpening's blur: the smallest and largest reach, in pixels.
pub const SHARPEN_SIGMA_MIN: f32 = 0.5;
pub const SHARPEN_SIGMA_MAX: f32 = 64.0;
/// Sharpening's law: its gain in stops per stop of detail at full
/// strength, and where Masking's edge weight rises (stops of detail, per
/// unit of Masking over 100, and its width).
pub const SHARPEN_GAIN: f32 = 7.0;
pub const SHARPEN_MASK_SCALE: f32 = 0.1;
pub const SHARPEN_MASK_WIDTH: f32 = 0.06;
/// The law's numbers: each slider's gain in stops at full weight, and
/// where Shadows' and Highlights' weights rise over the base (stops from
/// middle gray). The shaders take them from here.
pub const STRENGTH: f32 = 1.7;
pub const SHADOW_EDGES: [f32; 2] = [-4.8, 0.3];
pub const HIGHLIGHT_EDGES: [f32; 2] = [-1.0, 2.8];

/// Shadows (-100 to 100) as the strength the law takes: eased as the amount
/// grows, `a / (1 + 1.33·|a|)` for `a` the slider over 100.
pub fn shadows_strength(value: f64) -> f32 {
    let a = (value / 100.0).clamp(-1.0, 1.0);
    if !a.is_finite() {
        return 0.0;
    }
    (a / (1.0 + SHADOWS_EASE * a.abs())) as f32
}

/// Highlights (-100 to 100) as the strength the law takes: linear.
pub fn highlights_strength(value: f64) -> f32 {
    (value / 100.0) as f32
}

/// The base's reach and median radius on a picture whose long edge is
/// `long` pixels.
pub fn base_sizes(long: usize) -> (f32, usize) {
    let sigma = (BASE_SIGMA * long as f32).max(1.0);
    let detail = ((BASE_DETAIL * long as f64).round().max(0.0) as usize).min(BASE_DETAIL_MAX);
    (sigma, detail)
}

/// Clarity's reach on a picture whose long edge is `long` pixels.
pub fn clarity_sigma(long: usize) -> f32 {
    (CLARITY_SIGMA * long as f32).max(1.0)
}

/// Clarity (-100 to 100) as the strength the law takes: linear.
pub fn clarity_strength(value: f64) -> f32 {
    (value / 100.0) as f32
}

/// Texture's reach on a picture whose long edge is `long` pixels.
pub fn texture_sigma(long: usize) -> f32 {
    (TEXTURE_SIGMA * long as f32).max(TEXTURE_SIGMA_MIN)
}

/// Texture (-100 to 100) as the strength the law takes: linear, a negative
/// amount at [`NEGATIVE_TEXTURE`] of it.
pub fn texture_strength(value: f64) -> f32 {
    let t = value / 100.0;
    if t < 0.0 { (t * NEGATIVE_TEXTURE) as f32 } else { t as f32 }
}

/// Sharpening's numbers for the law: its strength (Amount over 150, times
/// the response's gain), the detail term's limit in stops and Masking over
/// 100; and its blur's reach. `None` when it changes nothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sharpen {
    pub strength: f32,
    pub limit: f32,
    pub masking: f32,
    pub sigma: f32,
}

impl Sharpen {
    /// Amount (0 to 150), Radius (0.5 to 3, in the picture's pixels),
    /// Detail and Masking (0 to 100), drawn at full size: one drawn pixel
    /// per pixel of the picture.
    pub fn new(amount: f64, radius: f64, detail: f64, masking: f64) -> Option<Self> {
        let raw = sharpen_sigma_raw(radius, 1.0, 1.0);
        let gain = sharpen_gain(raw);
        (amount != 0.0 && gain > 0.0).then(|| Self {
            strength: (amount / 150.0) as f32 * gain,
            limit: sharpen_limit(detail),
            masking: (masking / 100.0) as f32,
            sigma: sharpen_sigma(raw),
        })
    }
}

/// The blur's reach before it is clamped: Radius in the picture's pixels,
/// times the drawn pixels per pixel of the picture.
fn sharpen_sigma_raw(radius: f64, px_per_long: f64, native_long: f64) -> f32 {
    let radius = if radius.is_finite() { radius.clamp(0.1, 10.0) } else { 1.0 };
    (radius * px_per_long / native_long.max(1.0)) as f32
}

fn sharpen_sigma(raw: f32) -> f32 {
    raw.clamp(SHARPEN_SIGMA_MIN, SHARPEN_SIGMA_MAX)
}

/// Below one pixel, the gain that matches the continuous Gaussian's
/// response at Nyquist through the taps the blur can make, blended out
/// between half a pixel and one.
fn sharpen_gain(raw: f32) -> f32 {
    if !raw.is_finite() || raw <= 0.0 {
        return 0.0;
    }
    if raw >= 1.0 {
        return 1.0;
    }
    let taps = gauss_taps(sharpen_sigma(raw));
    let radius = taps.len() / 2;
    let response = taps
        .iter()
        .enumerate()
        .map(|(i, &t)| if (i + radius).is_multiple_of(2) { t } else { -t })
        .sum::<f32>();
    let desired = -(-0.5 * std::f32::consts::PI.powi(2) * raw * raw).exp_m1();
    let ratio = desired / (1.0 - response).max(1e-6);
    let blend = ((1.0 - raw) / 0.5).clamp(0.0, 1.0);
    1.0 + (ratio - 1.0) * blend
}

/// The detail term's soft limit, in stops, for Detail 0 to 100: 0.03
/// stops, which flattens large edges so halos stay small, up to 1.5.
fn sharpen_limit(detail: f64) -> f32 {
    let d = (detail / 100.0).clamp(0.0, 1.0) as f32;
    0.03 * 50f32.powf(d)
}

/// Normalized Gaussian taps for `sigma`, out to `ceil(3·sigma)`.
fn gauss_taps(sigma: f32) -> Vec<f32> {
    let sigma = if sigma.is_nan() { 0.05 } else { sigma.clamp(0.05, SHARPEN_SIGMA_MAX) };
    let r = (3.0 * sigma).ceil() as i32;
    let k = -0.5 / (sigma * sigma);
    let mut t: Vec<f32> = (-r..=r).map(|i| ((i * i) as f32 * k).exp()).collect();
    let sum: f32 = t.iter().sum();
    t.iter_mut().for_each(|v| *v /= sum);
    t
}

/// A plane of f32 values, row by row.
#[derive(Clone, Debug, PartialEq)]
pub struct Plane {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

impl Plane {
    pub fn new(width: usize, height: usize) -> Self {
        Self { width, height, data: vec![0.0; width * height] }
    }

    fn row(&self, y: usize) -> &[f32] {
        &self.data[y * self.width..(y + 1) * self.width]
    }
}

/// A rectangle of the frame, in its pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

impl Rect {
    pub fn rows(self) -> Range<usize> {
        self.y..self.y + self.height
    }

    pub fn columns(self) -> Range<usize> {
        self.x..self.x + self.width
    }

    /// The smallest rectangle holding both.
    pub fn union(self, other: Self) -> Self {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let x1 = (self.x + self.width).max(other.x + other.width);
        let y1 = (self.y + self.height).max(other.y + other.height);
        Self { x, y, width: x1 - x, height: y1 - y }
    }

    /// Grown by `margin` on every side, within a `frame`.
    pub fn grown(self, margin: usize, frame: [usize; 2]) -> Self {
        let x = self.x.saturating_sub(margin);
        let y = self.y.saturating_sub(margin);
        let x1 = (self.x + self.width + margin).min(frame[0]);
        let y1 = (self.y + self.height + margin).min(frame[1]);
        Self { x, y, width: x1 - x, height: y1 - y }
    }
}

/// Values of the frame over a rectangle.
pub struct Patch {
    pub rect: Rect,
    pub plane: Plane,
}

impl Patch {
    fn at(&self, x: usize, y: usize) -> f32 {
        self.plane.data[(y - self.rect.y) * self.plane.width + (x - self.rect.x)]
    }
}

/// A fast guided filter of a whole frame's plane, as craft-libs'
/// `guided_fast`: its blurred coefficients, at the frame's size or reduced.
/// With them and the plane over a rectangle, [`Guided::apply`] gives the
/// filtered values there.
struct Guided {
    frame: [usize; 2],
    mean_a: Plane,
    mean_b: Plane,
}

impl Guided {
    /// The coefficients of a `frame`-sized plane at reach `sigma` and
    /// `eps`, reading it a band of `band` rows at a time from `rows`, which
    /// gives its values over a rectangle of the frame.
    fn new(
        frame: [usize; 2],
        sigma: f32,
        eps: f32,
        band: usize,
        rows: &mut dyn FnMut(Rect) -> Plane,
    ) -> Self {
        let [w, h] = frame;
        let s = guided_fast_step(sigma);
        let (lw, lh) = (w.div_ceil(s).max(1), h.div_ceil(s).max(1));
        let small = shrink(frame, [lw, lh], band, &mut |r| {
            rows(Rect { x: 0, y: r.start, width: w, height: r.len() })
        });
        let (mean_a, mean_b) = guided_coeffs(&small, sigma / s as f32, eps);
        Self { frame, mean_a, mean_b }
    }

    /// The filtered values over `rect`, `p` the plane's values there.
    fn apply(&self, p: &Plane, rect: Rect) -> Plane {
        guided_apply(p, rect, self.frame, &self.mean_a, &self.mean_b)
    }
}

/// The planes of a whole frame the base needs, far smaller than it: the
/// coarse median and the guided filter's coefficients. With them and the
/// log brightness around a rectangle, [`Base::base`] gives the base there.
pub struct Base {
    detail: DetailFree,
    guided: Guided,
}

/// What takes a frame's detail out of its log brightness: the median's
/// radius and the coarse median.
struct DetailFree {
    frame: [usize; 2],
    detail: usize,
    /// The coarse median, its enlargement's weights to the frame
    /// (horizontal, then the vertical pass at its own height, then
    /// vertical to the frame).
    coarse: Option<Coarse>,
}

struct Coarse {
    plane: Plane,
    across: Vec<(usize, Vec<f32>)>,
    own_rows: Vec<(usize, Vec<f32>)>,
    down: Vec<(usize, Vec<f32>)>,
}

impl Base {
    /// The whole frame's planes for a `frame`-sized picture, reading its log
    /// brightness a band of `band` rows at a time from `log_brightness`,
    /// which gives the values over a rectangle of the frame.
    pub fn new(
        frame: [usize; 2],
        band: usize,
        mut log_brightness: impl FnMut(Rect) -> Plane,
    ) -> Self {
        let [w, h] = frame;
        let (sigma, detail) = base_sizes(w.max(h));
        let band = band.max(1);
        let coarse = (detail > 0).then(|| {
            let (lw, lh) =
                (w.div_ceil(BASE_COARSE_STEP).max(1), h.div_ceil(BASE_COARSE_STEP).max(1));
            ((lw, lh) != (w, h)).then(|| {
                let small = shrink(frame, [lw, lh], band, &mut |rows| {
                    log_brightness(Rect { x: 0, y: rows.start, width: w, height: rows.len() })
                });
                let whole = Rect { x: 0, y: 0, width: lw, height: lh };
                let patch = Patch { rect: whole, plane: small };
                Coarse {
                    plane: median_hv(&patch, [lw, lh], whole, detail),
                    across: weights(lw, w, Filter::Bilinear),
                    own_rows: weights(lh, lh, Filter::Bilinear),
                    down: weights(lh, h, Filter::Bilinear),
                }
            })
        });
        let free = DetailFree { frame, detail, coarse: coarse.flatten() };
        let guided = Guided::new(frame, sigma, BASE_EPS, band, &mut |rect| {
            let around = rect.grown(detail, frame);
            free.apply(&Patch { rect: around, plane: log_brightness(around) }, rect)
        });
        Self { detail: free, guided }
    }

    /// How far around a rectangle [`Self::base`] reads log brightness.
    pub fn margin(&self) -> usize {
        self.detail.detail
    }

    /// The base over `rect`, from the log brightness over `rect` grown by
    /// [`Self::margin`] within the frame.
    pub fn base(&self, around: &Patch, rect: Rect) -> Plane {
        self.guided.apply(&self.detail.apply(around, rect), rect)
    }
}

/// Which planes a draw's local tone reads: Highlights and Shadows' base,
/// Clarity's guided filter, Texture's blur, Sharpening's blur at its reach.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Needs {
    pub base: bool,
    pub clarity: bool,
    pub texture: bool,
    pub sharpen: Option<f32>,
}

/// The whole frame's planes a draw's local tone reads, far smaller than
/// the frame. With them and the log brightness around a rectangle,
/// [`Planes::at`] gives each pixel's values there.
pub struct Planes {
    frame: [usize; 2],
    base: Option<Base>,
    /// Clarity's guided filter of log brightness itself (craft-libs'
    /// `guided_fast` at [`CLARITY_EPS`]).
    clarity: Option<Guided>,
    /// Texture's blur of log brightness (craft-libs' `gaussian`).
    texture: Option<BoxBlur>,
    /// Sharpening's taps (craft-libs' `gaussian_taps`).
    sharpen: Option<Vec<f32>>,
}

impl Planes {
    /// The planes `needs` names, for a `frame`-sized picture whose log
    /// brightness `log_brightness` gives over a rectangle, read `band` rows
    /// at a time.
    pub fn new(
        frame: [usize; 2],
        band: usize,
        needs: Needs,
        mut log_brightness: impl FnMut(Rect) -> Plane,
    ) -> Self {
        let band = band.max(1);
        let base = needs.base.then(|| Base::new(frame, band, &mut log_brightness));
        let clarity = needs.clarity.then(|| {
            let sigma = clarity_sigma(frame[0].max(frame[1]));
            Guided::new(frame, sigma, CLARITY_EPS, band, &mut log_brightness)
        });
        let texture =
            needs.texture.then(|| BoxBlur::new(frame, texture_sigma(frame[0].max(frame[1]))));
        let sharpen = needs.sharpen.map(gauss_taps);
        Self { frame, base, clarity, texture, sharpen }
    }

    /// The rectangle around `rect` whose log brightness [`Self::at`] reads.
    pub fn around(&self, rect: Rect) -> Rect {
        let base = self.base.as_ref().map_or(0, Base::margin);
        let sharpen = self.sharpen.as_ref().map_or(0, |t| t.len() / 2);
        let near = rect.grown(base.max(sharpen), self.frame);
        self.texture.as_ref().map_or(near, |t| near.union(t.around(rect)))
    }

    /// Each pixel of `rect`, row by row: its base, or its log brightness
    /// without one; then its log brightness less Clarity's filter, less
    /// Texture's blur and less Sharpening's, each 0 without it. `around`
    /// covers [`Self::around`].
    pub fn at(&self, around: &Patch, rect: Rect) -> Vec<[f32; 4]> {
        let own = crop(around, rect);
        let base = self.base.as_ref().map(|b| b.base(around, rect));
        let clarity = self.clarity.as_ref().map(|c| c.apply(&own, rect));
        let texture = self.texture.as_ref().map(|t| t.apply(around, rect));
        let sharpen = self.sharpen.as_ref().map(|t| blur_taps(around, self.frame, rect, t));
        let mut out = vec![[0.0; 4]; rect.width * rect.height];
        for (i, (o, l)) in out.iter_mut().zip(&own.data).enumerate() {
            o[0] = base.as_ref().map_or(*l, |b| b.data[i]);
            if let Some(c) = &clarity {
                o[1] = l - c.data[i];
            }
            if let Some(t) = &texture {
                o[2] = l - t.data[i];
            }
            if let Some(b) = &sharpen {
                o[3] = l - b.data[i];
            }
        }
        out
    }
}

/// craft-libs' three-box blur of a whole frame's plane (`gaussian`), over
/// a rectangle at a time: each row's running sums span the frame's width
/// and each column's restart every [`BOX_BAND`] rows from the top, so a
/// rectangle reads whole rows from the start of a band of rows.
struct BoxBlur {
    frame: [usize; 2],
    /// `None`: no blur, as for a reach of 0.3 px or less.
    radii: Option<[usize; 3]>,
}

/// Rows of a frame-wide plane, from `first`.
struct Rows {
    first: usize,
    width: usize,
    data: Vec<f32>,
}

impl Rows {
    fn row(&self, y: usize) -> &[f32] {
        let i = y - self.first;
        &self.data[i * self.width..(i + 1) * self.width]
    }
}

impl BoxBlur {
    fn new(frame: [usize; 2], sigma: f32) -> Self {
        let [w, h] = frame;
        let radii = (!(sigma.is_nan() || sigma <= 0.3 || w == 0 || h == 0)).then(|| {
            let cap = (w.max(h) as f32 * 2.0).max(1.0);
            box_radii(sigma.min(cap))
        });
        Self { frame, radii }
    }

    /// The vertical passes' radii, with the rows each computes for `rect`
    /// and the rows the first reads.
    fn passes(&self, rect: Rect) -> (Vec<(usize, Range<usize>)>, Range<usize>) {
        let h = self.frame[1];
        let radii: Vec<usize> = self.radii.into_iter().flatten().filter(|&r| r > 0).collect();
        let mut passes = Vec::with_capacity(radii.len());
        let mut need = rect.rows();
        for &r in radii.iter().rev() {
            let start = need.start / BOX_BAND * BOX_BAND;
            passes.push((r, start..need.end));
            need = start.saturating_sub(r)..(need.end + r + 1).min(h);
        }
        passes.reverse();
        (passes, need)
    }

    /// The rectangle around `rect` whose values [`Self::apply`] reads:
    /// whole rows.
    fn around(&self, rect: Rect) -> Rect {
        let rows = self.passes(rect).1;
        Rect { x: 0, y: rows.start, width: self.frame[0], height: rows.len() }
    }

    /// The blur over `rect`, from the plane over [`Self::around`].
    fn apply(&self, around: &Patch, rect: Rect) -> Plane {
        let Some(radii) = self.radii else { return crop(around, rect) };
        let [w, h] = self.frame;
        let (passes, rows) = self.passes(rect);
        let mut data = vec![0.0f32; w * rows.len()];
        data.par_chunks_mut(w).zip(rows.clone()).for_each(|(row, y)| {
            for (o, x) in row.iter_mut().zip(0..w) {
                *o = around.at(x, y);
            }
            let mut tmp = Vec::with_capacity(w);
            for r in radii {
                box_row(row, &mut tmp, r);
            }
        });
        let mut src = Rows { first: rows.start, width: w, data };
        for (r, out) in passes {
            src = box_v_rows(&src, out, r, h);
        }
        let mut out = Plane::new(rect.width, rect.height);
        for (i, y) in rect.rows().enumerate() {
            out.data[i * rect.width..(i + 1) * rect.width]
                .copy_from_slice(&src.row(y)[rect.columns()]);
        }
        out
    }
}

/// craft-libs' separable blur with `taps` (`gaussian_taps`, edges clamped
/// to the `frame`) over `rect`, from values over `rect` grown by the taps'
/// radius within the frame.
fn blur_taps(around: &Patch, frame: [usize; 2], rect: Rect, taps: &[f32]) -> Plane {
    let [w, h] = frame;
    if taps.len() < 3 {
        return crop(around, rect);
    }
    let r = (taps.len() / 2) as isize;
    let clamp = |v: isize, n: usize| v.clamp(0, n as isize - 1) as usize;
    let rows = rect.grown(taps.len() / 2, frame).rows();
    let first = rows.start;
    let across: Vec<Vec<f32>> = rows
        .into_par_iter()
        .map(|y| {
            rect.columns()
                .map(|x| {
                    let mut acc = 0.0f32;
                    for (k, t) in taps.iter().enumerate() {
                        acc += around.at(clamp(x as isize + k as isize - r, w), y) * *t;
                    }
                    acc
                })
                .collect()
        })
        .collect();
    let mut out = Plane::new(rect.width, rect.height);
    out.data.par_chunks_mut(rect.width).zip(rect.rows()).for_each(|(row, y)| {
        for (k, t) in taps.iter().enumerate() {
            let src = &across[clamp(y as isize + k as isize - r, h) - first];
            for (o, v) in row.iter_mut().zip(src) {
                *o += *v * *t;
            }
        }
    });
    out
}

/// One box pass down the columns of a frame `h` rows high, over `out`'s
/// rows, as [`box_v`] makes them: each band of [`BOX_BAND`] rows starts its
/// running sums afresh. `out` starts at a band's first row.
fn box_v_rows(src: &Rows, out: Range<usize>, r: usize, h: usize) -> Rows {
    let w = src.width;
    let inv = 1.0 / (2 * r + 1) as f32;
    let last = h - 1;
    let mut data = vec![0.0f32; w * out.len()];
    data.par_chunks_mut(w * BOX_BAND).enumerate().for_each(|(band, rows)| {
        let y0 = out.start + band * BOX_BAND;
        let mut acc = vec![0.0f32; w];
        for i in 0..=2 * r {
            let yy = (y0 + i).saturating_sub(r).min(last);
            for (a, v) in acc.iter_mut().zip(src.row(yy)) {
                *a += *v * 1.0;
            }
        }
        for (k, o) in rows.chunks_mut(w).enumerate() {
            let y = y0 + k;
            for (d, a) in o.iter_mut().zip(&acc) {
                *d = 0.0 + *a * inv;
            }
            let (add, sub) = (src.row((y + r + 1).min(last)), src.row(y.saturating_sub(r)));
            for ((a, p), m) in acc.iter_mut().zip(add).zip(sub) {
                *a = *a + *p * 1.0 + *m * -1.0;
            }
        }
    });
    Rows { first: out.start, width: w, data }
}

impl DetailFree {
    /// The log brightness over `rect` without detail finer than the median
    /// radius: each value clamped between its median and the coarse
    /// median's.
    fn apply(&self, around: &Patch, rect: Rect) -> Plane {
        let r = self.detail;
        if r == 0 {
            return crop(around, rect);
        }
        let median = median_hv(around, self.frame, rect, r);
        let Some(coarse) = &self.coarse else { return median };
        let mut wide_rows = HashMap::new();
        let mut out = median;
        for (i, y) in rect.rows().enumerate() {
            let (y0, ws) = &coarse.down[y];
            let mut c = vec![0.0f32; rect.width];
            for (k, &wt) in ws.iter().enumerate() {
                let wide = wide_rows.entry(y0 + k).or_insert_with(|| coarse.wide_row(y0 + k, rect));
                for (c, &v) in c.iter_mut().zip(wide.iter()) {
                    *c += v * wt;
                }
            }
            let row = &mut out.data[i * rect.width..(i + 1) * rect.width];
            for ((o, x), c) in row.iter_mut().zip(rect.columns()).zip(c) {
                *o = clamp_between(around.at(x, y), *o, c);
            }
        }
        out
    }
}

impl Coarse {
    /// Row `y` of the coarse median enlarged across to the frame's width
    /// and passed through its own height, over `rect`'s columns.
    fn wide_row(&self, y: usize, rect: Rect) -> Vec<f32> {
        let (y0, ws) = &self.own_rows[y];
        let mut out = vec![0.0f32; rect.width];
        for (k, &wt) in ws.iter().enumerate() {
            let src = self.plane.row(y0 + k);
            for (o, x) in out.iter_mut().zip(rect.columns()) {
                let (left, xs) = &self.across[x];
                let mut acc = 0.0f32;
                for (j, &xw) in xs.iter().enumerate() {
                    acc += src[left + j] * xw;
                }
                *o += acc * wt;
            }
        }
        out
    }
}

/// `around`'s values over `rect`.
fn crop(around: &Patch, rect: Rect) -> Plane {
    let mut out = Plane::new(rect.width, rect.height);
    for (i, y) in rect.rows().enumerate() {
        for (j, x) in rect.columns().enumerate() {
            out.data[i * rect.width + j] = around.at(x, y);
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Filter {
    Box,
    Bilinear,
}

impl Filter {
    fn support(self) -> f32 {
        match self {
            Filter::Box => 0.5,
            Filter::Bilinear => 1.0,
        }
    }

    fn eval(self, x: f32) -> f32 {
        let x = x.abs();
        match self {
            Filter::Box => {
                if x <= 0.5 {
                    1.0
                } else {
                    0.0
                }
            }
            Filter::Bilinear => (1.0 - x).max(0.0),
        }
    }
}

/// Each output coordinate's first source index and weights.
fn weights(src: usize, dst: usize, filter: Filter) -> Vec<(usize, Vec<f32>)> {
    let scale = src as f32 / dst as f32;
    let fscale = scale.max(1.0);
    let support = filter.support() * fscale;
    (0..dst)
        .map(|i| {
            let center = (i as f32 + 0.5) * scale;
            let lo = ((center - support).floor() as isize).max(0) as usize;
            let hi = ((center + support).ceil() as usize).min(src);
            let mut w: Vec<f32> =
                (lo..hi).map(|j| filter.eval((j as f32 + 0.5 - center) / fscale)).collect();
            let sum: f32 = w.iter().sum();
            if sum.abs() > 1e-8 {
                w.iter_mut().for_each(|v| *v /= sum);
            } else if !w.is_empty() {
                let n = w.len() as f32;
                w.iter_mut().for_each(|v| *v = 1.0 / n);
            }
            (lo, w)
        })
        .collect()
}

/// A box-filtered copy of a `frame`-sized plane at `size`, made a band of
/// output rows at a time from the rows `rows` gives: across, then down.
fn shrink(
    frame: [usize; 2],
    size: [usize; 2],
    band: usize,
    rows: &mut dyn FnMut(Range<usize>) -> Plane,
) -> Plane {
    let ([w, h], [lw, lh]) = (frame, size);
    // A resize this small is horizontal first.
    assert!(lw * h <= w * lh * 4, "{frame:?} to {size:?}");
    let same = (w, h) == (lw, lh);
    let across = weights(w, lw, Filter::Box);
    let down = weights(h, lh, Filter::Box);
    let mut out = Plane::new(lw, lh);
    let mut top = 0;
    while top < lh {
        let end = (top + band).min(lh);
        let first = down[top].0;
        let last = (top..end).map(|y| down[y].0 + down[y].1.len()).max().unwrap_or(first);
        let source = rows(first..last);
        if same {
            out.data[top * lw..end * lw]
                .copy_from_slice(&source.data[(top - first) * w..(end - first) * w]);
            top = end;
            continue;
        }
        let narrow: Vec<Vec<f32>> = (0..source.height)
            .into_par_iter()
            .map(|y| {
                let src = source.row(y);
                across
                    .iter()
                    .map(|(lo, ws)| {
                        let mut acc = 0.0f32;
                        for (k, wt) in ws.iter().enumerate() {
                            acc += src[lo + k] * wt;
                        }
                        acc
                    })
                    .collect()
            })
            .collect();
        for (y, (lo, ws)) in down.iter().enumerate().take(end).skip(top) {
            let row = &mut out.data[y * lw..(y + 1) * lw];
            for (k, wt) in ws.iter().enumerate() {
                for (o, s) in row.iter_mut().zip(&narrow[lo + k - first]) {
                    *o += s * wt;
                }
            }
        }
        top = end;
    }
    out
}

/// `v` clamped between `a` and `b`, in either order; a NaN bound is
/// ignored.
fn clamp_between(v: f32, a: f32, b: f32) -> f32 {
    let (lo, hi) = (a.min(b), a.max(b));
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

/// The fast guided filter's subsampling for `sigma` (1: none).
fn guided_fast_step(sigma: f32) -> usize {
    (sigma / 3.0).floor().clamp(1.0, 16.0) as usize
}

/// `v`'s bits as an `i32` that orders as [`f32::total_cmp`] does.
fn total_key(v: f32) -> i32 {
    let b = v.to_bits() as i32;
    b ^ (((b >> 31) as u32) >> 1) as i32
}

fn total_key_inv(k: i32) -> f32 {
    f32::from_bits((k ^ (((k >> 31) as u32) >> 1) as i32) as u32)
}

/// `out[j]` the median of `src[j..=j + 2r]`: a sorted window that drops the
/// leaving value and inserts the entering one.
fn running_median(src: &[f32], r: usize, out: &mut [f32]) {
    let n = 2 * r + 1;
    debug_assert_eq!(src.len(), out.len() + 2 * r);
    let mut win: Vec<i32> = src[..n].iter().map(|&v| total_key(v)).collect();
    win.sort_unstable();
    for (j, o) in out.iter_mut().enumerate() {
        *o = total_key_inv(win[r]);
        let Some(&entering) = src.get(j + n) else { break };
        let (old, new) = (total_key(src[j]), total_key(entering));
        if old != new {
            let (mut i, mut k) = (0u32, 0u32);
            for &v in &win {
                i += u32::from(v < old);
                k += u32::from(v < new);
            }
            let (i, k) = (i as usize, k as usize);
            if k > i {
                win.copy_within(i + 1..k, i);
                win[k - 1] = new;
            } else {
                win.copy_within(k..i, k + 1);
                win[k] = new;
            }
        }
    }
}

/// The separable median (`2r + 1` along each row, then each column, edges
/// clamped to the `frame`) over `rect`, from values over `rect` grown by
/// `r` within the frame.
fn median_hv(around: &Patch, frame: [usize; 2], rect: Rect, r: usize) -> Plane {
    if r == 0 {
        return crop(around, rect);
    }
    let [w, h] = frame;
    let clamp = |v: isize, n: usize| v.clamp(0, n as isize - 1) as usize;
    // Rows first, over the rows the columns' pass reads.
    let rows = rect.grown(r, frame).rows();
    let first = rows.start;
    let across: Vec<Vec<f32>> = rows
        .into_par_iter()
        .map(|y| {
            let src: Vec<f32> = (0..rect.width + 2 * r)
                .map(|i| around.at(clamp(rect.x as isize + i as isize - r as isize, w), y))
                .collect();
            let mut out = vec![0.0; rect.width];
            running_median(&src, r, &mut out);
            out
        })
        .collect();
    let columns: Vec<Vec<f32>> = (0..rect.width)
        .into_par_iter()
        .map(|x| {
            let src: Vec<f32> = (0..rect.height + 2 * r)
                .map(|i| across[clamp(rect.y as isize + i as isize - r as isize, h) - first][x])
                .collect();
            let mut out = vec![0.0; rect.height];
            running_median(&src, r, &mut out);
            out
        })
        .collect();
    let mut out = Plane::new(rect.width, rect.height);
    for (x, column) in columns.iter().enumerate() {
        for (y, v) in column.iter().enumerate() {
            out.data[y * rect.width + x] = *v;
        }
    }
    out
}

/// Box radii approximating a Gaussian of `sigma` in three passes.
fn box_radii(sigma: f32) -> [usize; 3] {
    const MAX_SIGMA: f32 = 65_536.0;
    let sigma = if sigma.is_nan() { 0.0 } else { sigma.clamp(0.0, MAX_SIGMA) };
    let n = 3.0f32;
    let w_ideal = (12.0 * sigma * sigma / n + 1.0).sqrt();
    let mut wl = w_ideal.floor() as i32;
    if wl % 2 == 0 {
        wl -= 1;
    }
    let wu = wl + 2;
    let m_ideal = (12.0 * sigma * sigma
        - n * (i64::from(wl) * i64::from(wl)) as f32
        - 4.0 * n * wl as f32
        - 3.0 * n)
        / (-4.0 * wl as f32 - 4.0);
    let m = m_ideal.round() as i32;
    std::array::from_fn(|i| (((if (i as i32) < m { wl } else { wu }) - 1) / 2).max(0) as usize)
}

/// One box pass along `row`, edges clamped.
fn box_row(row: &mut [f32], tmp: &mut Vec<f32>, r: usize) {
    let w = row.len();
    if r == 0 || w == 0 {
        return;
    }
    tmp.clear();
    tmp.extend_from_slice(row);
    let s = &tmp[..];
    let last = w - 1;
    let inv = 1.0 / (2 * r + 1) as f32;
    let mut acc = 0.0f32;
    acc += s[0] * r as f32;
    for v in &s[..=r.min(last)] {
        acc += *v;
    }
    if r > last {
        acc += s[last] * (r - last) as f32;
    }
    for x in 0..w {
        row[x] = 0.0 + acc * inv;
        acc = acc + s[(x + r + 1).min(last)] - s[x.saturating_sub(r)];
    }
}

/// Rows per band of the vertical box pass, each band starting its running
/// sums afresh.
const BOX_BAND: usize = 32;

/// One box pass down the columns of `src` into `dst`, edges clamped.
fn box_v(src: &Plane, dst: &mut Plane, r: usize) {
    let (w, h) = (src.width, src.height);
    let inv = 1.0 / (2 * r + 1) as f32;
    let last = h - 1;
    dst.data.par_chunks_mut(w * BOX_BAND).enumerate().for_each(|(band, out)| {
        let y0 = band * BOX_BAND;
        let mut acc = vec![0.0f32; w];
        for i in 0..=2 * r {
            let yy = (y0 + i).saturating_sub(r).min(last);
            for (a, v) in acc.iter_mut().zip(src.row(yy)) {
                *a += *v * 1.0;
            }
        }
        for (k, o) in out.chunks_mut(w).enumerate() {
            let y = y0 + k;
            for (d, a) in o.iter_mut().zip(&acc) {
                *d = 0.0 + *a * inv;
            }
            let (add, sub) = (src.row((y + r + 1).min(last)), src.row(y.saturating_sub(r)));
            for ((a, p), m) in acc.iter_mut().zip(add).zip(sub) {
                *a = *a + *p * 1.0 + *m * -1.0;
            }
        }
    });
}

/// A Gaussian-like blur of `sigma` px by three box passes, rows then
/// columns.
fn gaussian(p: &Plane, sigma: f32) -> Plane {
    if sigma.is_nan() || sigma <= 0.3 || p.width == 0 || p.height == 0 {
        return p.clone();
    }
    let cap = (p.width.max(p.height) as f32 * 2.0).max(1.0);
    let radii = box_radii(sigma.min(cap));
    let mut a = p.clone();
    a.data.par_chunks_mut(p.width).for_each(|row| {
        let mut tmp = Vec::with_capacity(row.len());
        for r in radii {
            box_row(row, &mut tmp, r);
        }
    });
    let mut b = Plane::new(p.width, p.height);
    for r in radii {
        if r == 0 {
            continue;
        }
        box_v(&a, &mut b, r);
        std::mem::swap(&mut a, &mut b);
    }
    a
}

fn map(p: &Plane, f: impl Fn(f32) -> f32) -> Plane {
    Plane { width: p.width, height: p.height, data: p.data.iter().map(|&v| f(v)).collect() }
}

fn zip_map(a: &Plane, b: &Plane, f: impl Fn(f32, f32) -> f32) -> Plane {
    let data = a.data.iter().zip(&b.data).map(|(&a, &b)| f(a, b)).collect();
    Plane { width: a.width, height: a.height, data }
}

/// The guided filter's blurred coefficients (mean a, mean b) of `p`.
fn guided_coeffs(p: &Plane, sigma: f32, eps: f32) -> (Plane, Plane) {
    let mean = gaussian(p, sigma);
    let corr = gaussian(&map(p, |v| v * v), sigma);
    let a = zip_map(&corr, &mean, |c, m| {
        let var = (c - m * m).max(0.0);
        var / (var + eps)
    });
    let b = zip_map(&mean, &a, |m, a| m - a * m);
    (gaussian(&a, sigma), gaussian(&b, sigma))
}

/// `a·p + b` over `rect` of a `frame`, `p` its values there, the
/// coefficients sampled bilinearly when reduced.
fn guided_apply(p: &Plane, rect: Rect, frame: [usize; 2], a: &Plane, b: &Plane) -> Plane {
    let [w, h] = frame;
    let mut q = Plane::new(rect.width, rect.height);
    if (a.width, a.height) == (w, h) {
        for (i, y) in rect.rows().enumerate() {
            for (j, x) in rect.columns().enumerate() {
                let k = i * rect.width + j;
                q.data[k] = a.data[y * w + x] * p.data[k] + b.data[y * w + x];
            }
        }
        return q;
    }
    let wx = weights(a.width, w, Filter::Bilinear);
    let wy = weights(a.height, h, Filter::Bilinear);
    let wx = &wx[rect.columns()];
    q.data.par_chunks_mut(rect.width).enumerate().for_each(|(i, row)| {
        let y = rect.y + i;
        let (top, ys) = &wy[y];
        let mut bias = vec![0.0f32; rect.width];
        for (dy, yw) in ys.iter().enumerate() {
            let (ar, br) = (a.row(top + dy), b.row(top + dy));
            for ((v, bv), (left, xs)) in row.iter_mut().zip(bias.iter_mut()).zip(wx) {
                let (mut av, mut bx) = (0.0f32, 0.0f32);
                for (dx, xw) in xs.iter().enumerate() {
                    av += ar[left + dx] * xw;
                    bx += br[left + dx] * xw;
                }
                *v += av * yw;
                *bv += bx * yw;
            }
        }
        for ((v, pv), bv) in row.iter_mut().zip(p.row(i)).zip(&bias) {
            *v = *v * pv + bv;
        }
    });
    q
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Log brightness with edges, gradients and fine texture.
    fn scene(w: usize, h: usize) -> Plane {
        let mut p = Plane::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let hash = (x.wrapping_mul(73_856_093) ^ y.wrapping_mul(19_349_663)) % 1000;
                let step = if (x / 37 + y / 23) % 3 == 0 { -3.0 } else { 0.5 };
                p.data[y * w + x] = step + x as f32 / w as f32 * 2.0 + hash as f32 / 1000.0;
            }
        }
        p
    }

    fn reader(p: &Plane) -> impl FnMut(Rect) -> Plane + '_ {
        move |rect| {
            let mut out = Plane::new(rect.width, rect.height);
            for (i, y) in rect.rows().enumerate() {
                out.data[i * rect.width..(i + 1) * rect.width]
                    .copy_from_slice(&p.row(y)[rect.columns()]);
            }
            out
        }
    }

    fn base_of(p: &Plane, base: &Base, rect: Rect) -> Plane {
        let around = rect.grown(base.margin(), [p.width, p.height]);
        base.base(&Patch { rect: around, plane: reader(p)(around) }, rect)
    }

    /// The whole frame's planes and every region's base are the same
    /// whatever the band, and a region's base is that region of the whole.
    #[test]
    fn a_region_and_a_band_give_the_whole_planes_values() {
        for (w, h) in [(1300, 700), (200, 520), (61, 40)] {
            let p = scene(w, h);
            let whole = Rect { x: 0, y: 0, width: w, height: h };
            let one = Base::new([w, h], h, reader(&p));
            let full = base_of(&p, &one, whole);
            for band in [1, 7, 64] {
                let banded = Base::new([w, h], band, reader(&p));
                assert_eq!(banded.guided.mean_a, one.guided.mean_a, "{w}x{h} band {band}");
                assert_eq!(banded.guided.mean_b, one.guided.mean_b, "{w}x{h} band {band}");
            }
            for rect in [
                Rect { x: 0, y: 0, width: w.min(33), height: h.min(17) },
                Rect { x: w / 3, y: h / 4, width: w / 2, height: h / 3 },
                Rect { x: w - 9, y: h - 5, width: 9, height: 5 },
            ] {
                let part = base_of(&p, &one, rect);
                for (i, y) in rect.rows().enumerate() {
                    let row = &full.data[y * w + rect.x..y * w + rect.x + rect.width];
                    assert_eq!(&part.data[i * rect.width..(i + 1) * rect.width], row, "{rect:?}");
                }
            }
        }
    }

    /// The planes at a region are the whole frame's there, whatever the
    /// band, and Clarity's filter is the guided filter of log brightness.
    #[test]
    fn the_planes_at_a_region_are_the_whole_frames() {
        let sharpen = Some(1.4);
        let needs = Needs { base: true, clarity: true, texture: true, sharpen };
        for (w, h) in [(1300, 700), (61, 40)] {
            let p = scene(w, h);
            let at = |planes: &Planes, rect: Rect| {
                let around = planes.around(rect);
                planes.at(&Patch { rect: around, plane: reader(&p)(around) }, rect)
            };
            let whole = Rect { x: 0, y: 0, width: w, height: h };
            let one = Planes::new([w, h], h, needs, reader(&p));
            let full = at(&one, whole);
            let banded = Planes::new([w, h], 7, needs, reader(&p));
            for rect in [Rect { x: w / 3, y: h / 4, width: w / 2, height: h / 3 }, whole] {
                let part = at(&banded, rect);
                for (i, y) in rect.rows().enumerate() {
                    let row = &full[y * w + rect.x..y * w + rect.x + rect.width];
                    assert_eq!(&part[i * rect.width..(i + 1) * rect.width], row, "{rect:?}");
                }
            }
            assert!(full.iter().any(|v| v[1] != 0.0) && full.iter().any(|v| v[2] != 0.0));
            let blur = gaussian(&p, texture_sigma(w.max(h)));
            let detail: Vec<f32> = p.data.iter().zip(&blur.data).map(|(l, b)| l - b).collect();
            assert!(full.iter().map(|v| v[2]).eq(detail), "{w}x{h}: Texture's blur");
            assert!(full.iter().any(|v| v[3] != 0.0));
        }
    }

    #[test]
    fn the_running_median_is_the_windows_median() {
        let src: Vec<f32> = (0..50).map(|i| ((i * 37) % 23) as f32 - 7.5).collect();
        let r = 4;
        let mut out = vec![0.0; src.len() - 2 * r];
        running_median(&src, r, &mut out);
        for (j, o) in out.iter().enumerate() {
            let mut w = src[j..=j + 2 * r].to_vec();
            w.sort_by(f32::total_cmp);
            assert_eq!(*o, w[r]);
        }
    }

    #[test]
    fn shadows_ease_off() {
        assert_eq!(shadows_strength(0.0), 0.0);
        assert!((shadows_strength(100.0) - 1.0 / 2.33).abs() < 1e-6);
        assert!((shadows_strength(-100.0) + 1.0 / 2.33).abs() < 1e-6);
        assert_eq!(shadows_strength(f64::NAN), 0.0);
        assert_eq!(highlights_strength(-50.0), -0.5);
    }
}
