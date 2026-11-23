//! A set of frames, and what the checks need of them, measured once.

use std::path::{Path, PathBuf};

use deptheos_app::library::export::ExportFormat;
use deptheos_dng::RenderParams;
use rayon::prelude::*;

use crate::metadata::FrameTags;
use crate::read::{Small, is_raw, read, read_by};
use crate::space::Space;

/// A folder of frames: one test set.
pub struct Set {
    pub name: String,
    pub dir: PathBuf,
    /// In name order.
    pub frames: Vec<PathBuf>,
}

impl Set {
    /// The frames in `dir`: JPEG, PNG and TIFF, camera RAW and DNG.
    pub fn open(dir: &Path) -> Result<Self, String> {
        let mut frames: Vec<PathBuf> = std::fs::read_dir(dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_file() && (ExportFormat::of(p).is_some() || is_raw(p)))
            .filter(|p| !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')))
            .collect();
        frames.sort();
        if frames.is_empty() {
            return Err(format!("{}: no frames", dir.display()));
        }
        let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        Ok(Self { name, dir: dir.to_path_buf(), frames })
    }

    /// The set in `dir`, byte for byte as its manifest ([`MANIFEST`]) says,
    /// so a reference is held to the frames it was made from.
    pub fn open_checked(dir: &Path) -> Result<Self, String> {
        let set = Self::open(dir)?;
        let path = dir.join(MANIFEST);
        let text = std::fs::read_to_string(&path).map_err(|e| {
            format!(
                "{}: {e}; a set needs its manifest, written once with `e2e-check manifest {}`",
                path.display(),
                dir.display()
            )
        })?;
        let wanted: Manifest =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let found = set.manifest()?;
        if found != wanted {
            let names =
                |m: &Manifest| -> Vec<String> { m.frames.iter().map(|f| f.name.clone()).collect() };
            let changed: Vec<String> = wanted
                .frames
                .iter()
                .filter(|w| !found.frames.contains(w))
                .map(|w| w.name.clone())
                .chain(names(&found).into_iter().filter(|n| !names(&wanted).contains(n)))
                .collect();
            return Err(format!(
                "{}: the frames aren't the ones its manifest lists (changed, added or gone: {})",
                dir.display(),
                changed.join(", ")
            ));
        }
        Ok(set)
    }

    /// Write the set's manifest; an existing one is never replaced.
    pub fn write_manifest(&self) -> Result<PathBuf, String> {
        let path = self.dir.join(MANIFEST);
        if path.exists() {
            return Err(format!("{} is there already", path.display()));
        }
        let text = serde_json::to_string_pretty(&self.manifest()?).expect("plain data");
        std::fs::write(&path, text + "\n").map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(path)
    }

    /// Each frame's name, size and SHA-256.
    fn manifest(&self) -> Result<Manifest, String> {
        use sha2::{Digest, Sha256};
        use std::io::Read as _;
        let frames = self
            .frames
            .par_iter()
            .map(|path| {
                let failed = |e: std::io::Error| format!("{}: {e}", path.display());
                let mut file = std::fs::File::open(path).map_err(failed)?;
                let mut hash = Sha256::new();
                let mut buf = vec![0u8; 1 << 20];
                let mut bytes = 0u64;
                loop {
                    let n = file.read(&mut buf).map_err(failed)?;
                    if n == 0 {
                        break;
                    }
                    hash.update(&buf[..n]);
                    bytes += n as u64;
                }
                let sha256 = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
                let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                Ok(ManifestFrame { name, bytes, sha256 })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Manifest { frames })
    }

    /// Whether `other` has the same frame names but for the extension (the
    /// same frames from another decoder).
    pub fn same_frames(&self, other: &Set) -> bool {
        let stems = |s: &Set| -> Vec<Option<std::ffi::OsString>> {
            s.frames.iter().map(|f| f.file_stem().map(|n| n.to_ascii_lowercase())).collect()
        };
        stems(self) == stems(other)
    }

    /// The frame named `name`, as an app's paint source names it.
    pub fn frame_named(&self, name: &str) -> Option<&Path> {
        self.frames.iter().find(|f| f.file_name().is_some_and(|n| n == name)).map(PathBuf::as_path)
    }
}

/// A set's manifest, in its folder beside the frames.
pub const MANIFEST: &str = "manifest.json";

/// What a set's frames were when its manifest was written.
#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Manifest {
    frames: Vec<ManifestFrame>,
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct ManifestFrame {
    name: String,
    bytes: u64,
    sha256: String,
}

/// The rough long side a set is measured at ([`scale_for`]).
const MEASURED_SIDE: u32 = 2048;
/// A tile at the measuring scale (256 pixels of a 45 MP frame).
pub(crate) const TILE: usize = 64;
/// The whole factor frames are shrunk by to be measured: the long side
/// over [`MEASURED_SIDE`], rounded, at least 1.
pub(crate) fn scale_for(width: u32, height: u32) -> usize {
    ((width.max(height) + MEASURED_SIDE / 2) / MEASURED_SIDE).max(1) as usize
}

/// What the checks need of a set's frames, read once.
pub struct Frames {
    pub width: u32,
    pub height: u32,
    /// The factor everything is shrunk by to be measured ([`scale_for`]).
    pub(crate) k: usize,
    pub(crate) mean: Small,
    /// Per frame, Sobel energy per tile on the frame's own grid.
    pub(crate) tiles: Vec<Vec<f32>>,
    pub(crate) cols: usize,
    pub(crate) rows: usize,
    /// What the frames' metadata holds, for an export's to be checked against.
    pub(crate) tags: FrameTags,
    /// Per frame, the color science a RAW or DNG frame renders with.
    pub(crate) colors: Vec<Option<RenderParams>>,
    /// The color space the frames are in, as their files say.
    pub(crate) space: Space,
}

impl Frames {
    pub fn read(set: &Set) -> Result<Self, String> {
        let mut k = 1;
        let first = read_by(&set.frames[0], |w, h| {
            k = scale_for(w, h);
            k
        })?;
        let (w, h) = (first.small.w, first.small.h);
        let (cols, rows) = (w / TILE, h / TILE);
        type One = (Small, Vec<f32>, Option<RenderParams>);
        let read_one = |path: &PathBuf| -> Result<One, String> {
            let r = read(path, k)?;
            if (r.width, r.height) != (first.width, first.height) {
                return Err(format!("{}: another size from the first frame's", path.display()));
            }
            let energy = sobel(&r.small.gray(), w, h);
            let tiles = tile_means(&energy, w, cols, rows, (0, 0));
            Ok((r.small, tiles, r.color))
        };
        let mut mean = vec![[0f32; 3]; w * h];
        let mut tiles = Vec::with_capacity(set.frames.len());
        let mut colors = Vec::with_capacity(set.frames.len());
        // A few at a time: 48 frames of 45 MP don't all fit in memory.
        let at_once = if set.frames.iter().any(|f| is_raw(f)) { 3 } else { 8 };
        for chunk in set.frames.chunks(at_once) {
            let read: Vec<_> = chunk.par_iter().map(read_one).collect();
            for r in read {
                let (small, t, color) = r?;
                for (m, p) in mean.iter_mut().zip(&small.px) {
                    for c in 0..3 {
                        m[c] += p[c];
                    }
                }
                tiles.push(t);
                colors.push(color);
            }
        }
        let n = set.frames.len() as f32;
        for m in &mut mean {
            for c in m.iter_mut() {
                *c /= n;
            }
        }
        Ok(Self {
            width: first.width,
            height: first.height,
            k,
            mean: Small { w, h, px: mean },
            tiles,
            tags: FrameTags::read(&set.frames),
            colors,
            space: Space::of_frame(&set.frames[0])?,
            cols,
            rows,
        })
    }

    /// Where the stack in `file` sits on the frames, in their pixels.
    pub fn place_file(&self, file: &Path) -> Result<(usize, usize), String> {
        Ok(self.place(&read(file, self.k)?.small))
    }

    /// The frame with the least detail under `region` (frame pixels: left,
    /// top, right, bottom), so a stroke from it visibly changes the stack.
    pub fn softest_in(&self, region: (usize, usize, usize, usize)) -> usize {
        let side = self.k * TILE;
        let tiles: Vec<usize> = (region.1 / side..=region.3 / side)
            .flat_map(|ty| (region.0 / side..=region.2 / side).map(move |tx| (tx, ty)))
            .filter(|&(tx, ty)| tx < self.cols && ty < self.rows)
            .map(|(tx, ty)| ty * self.cols + tx)
            .collect();
        let detail = |t: &Vec<f32>| tiles.iter().map(|&i| t[i]).sum::<f32>();
        (0..self.tiles.len())
            .min_by(|&a, &b| detail(&self.tiles[a]).total_cmp(&detail(&self.tiles[b])))
            .unwrap_or(0)
    }

    /// Where a stack (shrunk by [`Frames::k`]) sits on the frames, which the
    /// aligner cropped it from: matched to their mean, coarse then fine.
    pub fn place(&self, stack: &Small) -> (usize, usize) {
        let coarse = 4;
        let (s, m) = (stack.shrink(coarse), self.mean.shrink(coarse));
        let (sg, mg) = (s.gray(), m.gray());
        let all = |w: usize, mw: usize| (0..=mw.saturating_sub(w)).collect::<Vec<_>>();
        let (cx, cy) = best_offset(&sg, s.w, s.h, &mg, m.w, all(s.w, m.w), all(s.h, m.h));
        let near = |c: usize, max: usize| {
            let lo = (c * coarse).saturating_sub(coarse * 2);
            (lo..=(c * coarse + coarse * 2).min(max)).collect::<Vec<_>>()
        };
        let (sg, mg) = (stack.gray(), self.mean.gray());
        let (fx, fy) = best_offset(
            &sg,
            stack.w,
            stack.h,
            &mg,
            self.mean.w,
            near(cx, self.mean.w.saturating_sub(stack.w)),
            near(cy, self.mean.h.saturating_sub(stack.h)),
        );
        (fx * self.k, fy * self.k)
    }
}

/// The offset of `a` in `b` where they differ least, over the offsets
/// given, on every other pixel.
fn best_offset(
    a: &[f32],
    aw: usize,
    ah: usize,
    b: &[f32],
    bw: usize,
    xs: Vec<usize>,
    ys: Vec<usize>,
) -> (usize, usize) {
    let offsets: Vec<(usize, usize)> =
        ys.iter().flat_map(|&y| xs.iter().map(move |&x| (x, y))).collect();
    offsets
        .into_par_iter()
        .map(|(ox, oy)| {
            let mut sum = 0f32;
            for y in (0..ah).step_by(2) {
                for x in (0..aw).step_by(2) {
                    sum += (a[y * aw + x] - b[(y + oy) * bw + x + ox]).abs();
                }
            }
            (sum, (ox, oy))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, o)| o)
        .unwrap_or((0, 0))
}

/// Squared Sobel gradient magnitude, zero on the edge.
pub(crate) fn sobel(g: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        if y == 0 || y + 1 >= h {
            return;
        }
        for x in 1..w - 1 {
            let p = |dx: isize, dy: isize| {
                g[(y as isize + dy) as usize * w + (x as isize + dx) as usize]
            };
            let gx = p(1, -1) + 2.0 * p(1, 0) + p(1, 1) - p(-1, -1) - 2.0 * p(-1, 0) - p(-1, 1);
            let gy = p(-1, 1) + 2.0 * p(0, 1) + p(1, 1) - p(-1, -1) - 2.0 * p(0, -1) - p(1, -1);
            row[x] = gx * gx + gy * gy;
        }
    });
    out
}

/// The mean of `values` (`w` wide) in each tile of the frames' grid,
/// with `values` placed at `at` on it; NaN for a tile it doesn't cover.
pub(crate) fn tile_means(
    values: &[f32],
    w: usize,
    cols: usize,
    rows: usize,
    at: (usize, usize),
) -> Vec<f32> {
    let h = values.len() / w;
    (0..rows * cols)
        .map(|i| {
            let (tx, ty) = (i % cols, i / cols);
            let (x0, y0) = (tx * TILE, ty * TILE);
            // A pixel's gradient needs its neighbors: one in from each edge.
            if x0 < at.0 + 1
                || y0 < at.1 + 1
                || x0 + TILE + 1 > at.0 + w
                || y0 + TILE + 1 > at.1 + h
            {
                return f32::NAN;
            }
            let mut sum = 0f32;
            for y in y0..y0 + TILE {
                let row = (y - at.1) * w;
                for x in x0..x0 + TILE {
                    sum += values[row + x - at.0];
                }
            }
            sum / (TILE * TILE) as f32
        })
        .collect()
}
