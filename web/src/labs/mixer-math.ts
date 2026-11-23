// The level mixer's arithmetic. Collapsing is linear, so the result is frame
// B plus, for each level taken from A, A's level minus B's at full size:
// worked out once, so a switch costs one pass over the image.

import { expand, pyramid, sub, type Plane } from "./laplacian.ts";

/** Detail levels the mixer offers; the switch after them is "the rest", the coarsest level left. */
export const DETAIL_LEVELS = 4;

export interface Mixer {
  a: Plane;
  b: Plane;
  /** Per switch (the detail levels, finest first, then the rest): A's level minus B's, at full size. */
  differences: Plane[];
  /** Per switch: B's level, at full size. They add up to B. */
  levelsB: Plane[];
}

export function prepare(a: Plane, b: Plane): Mixer {
  const pa = pyramid(a, DETAIL_LEVELS);
  const pb = pyramid(b, DETAIL_LEVELS);
  const differences = pa.detail.map((d, l) => expand(sub(d, pb.detail[l]), l, a.w, a.h));
  differences.push(expand(sub(pa.rest, pb.rest), DETAIL_LEVELS, a.w, a.h));
  const levelsB = pb.detail.map((d, l) => expand(d, l, a.w, a.h));
  levelsB.push(expand(pb.rest, DETAIL_LEVELS, a.w, a.h));
  return { a, b, differences, levelsB };
}

/** The result, as RGBA bytes, with switch `i` taking its level from A where `fromA[i]`. */
export function mix(m: Pick<Mixer, "b" | "differences">, fromA: boolean[], out: Uint8ClampedArray): void {
  const { data } = m.b;
  const picked = m.differences.filter((_, i) => fromA[i]).map((d) => d.data);
  for (let p = 0, q = 0; p < data.length; p += 3, q += 4) {
    for (let c = 0; c < 3; c++) {
      let v = data[p + c];
      for (const d of picked) v += d[p + c];
      out[q + c] = v * 255;
    }
    out[q + 3] = 255;
  }
}

/** One switch's level of frame A or B at full size, cut to the rectangle: RGB, row-major (detail is signed, about zero). */
export function tile(m: Mixer, level: number, fromA: boolean, x0: number, y0: number, w: number, h: number): Float32Array {
  const out = new Float32Array(w * h * 3);
  const b = m.levelsB[level].data;
  const d = m.differences[level].data;
  for (let y = 0; y < h; y++)
    for (let x = 0; x < w; x++) {
      const p = ((y0 + y) * m.b.w + x0 + x) * 3;
      const q = (y * w + x) * 3;
      for (let c = 0; c < 3; c++) out[q + c] = b[p + c] + (fromA ? d[p + c] : 0);
    }
  return out;
}
