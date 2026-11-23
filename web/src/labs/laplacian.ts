// The pyramid method's arithmetic in plain JavaScript, as in
// `laplacian.wgsl` and `pyramid.wgsl`, for the pyramid diagram and the
// level mixer.

export const TAPS = [0.0625, 0.25, 0.375, 0.25, 0.0625];

export interface Plane {
  w: number;
  h: number;
  /** RGB, 0–1, row-major. */
  data: Float32Array;
}

export const clampi = (v: number, lo: number, hi: number) => (v < lo ? lo : v > hi ? hi : v);

/** 5×5 Gaussian blur, then every second pixel (the stacker's downsample), separable 1D passes. */
export function down(p: Plane): Plane {
  const w = Math.max(1, Math.ceil(p.w / 2));
  const h = Math.max(1, Math.ceil(p.h / 2));
  const tmp = new Float32Array(w * p.h * 3);
  for (let sy = 0; sy < p.h; sy++) {
    const rowIn = sy * p.w;
    const rowOut = sy * w;
    for (let x = 0; x < w; x++) {
      const o = (rowOut + x) * 3;
      let r = 0, g = 0, b = 0;
      for (let i = -2; i <= 2; i++) {
        const t = TAPS[i + 2];
        const q = (rowIn + clampi(2 * x + i, 0, p.w - 1)) * 3;
        r += t * p.data[q];
        g += t * p.data[q + 1];
        b += t * p.data[q + 2];
      }
      tmp[o] = r;
      tmp[o + 1] = g;
      tmp[o + 2] = b;
    }
  }
  const out = new Float32Array(w * h * 3);
  for (let y = 0; y < h; y++) {
    const rowOut = y * w;
    for (let x = 0; x < w; x++) {
      const o = (rowOut + x) * 3;
      let r = 0, g = 0, b = 0;
      for (let j = -2; j <= 2; j++) {
        const t = TAPS[j + 2];
        const q = (clampi(2 * y + j, 0, p.h - 1) * w + x) * 3;
        r += t * tmp[q];
        g += t * tmp[q + 1];
        b += t * tmp[q + 2];
      }
      out[o] = r;
      out[o + 1] = g;
      out[o + 2] = b;
    }
  }
  return { w, h, data: out };
}

/** Scale a level back up to `w`×`h` with the same weights (the stacker's upsample), separable 1D passes. */
export function up(p: Plane, w: number, h: number): Plane {
  const tmp = new Float32Array(w * p.h * 3);
  for (let sy = 0; sy < p.h; sy++) {
    const rowIn = sy * p.w;
    const rowOut = sy * w;
    for (let x = 0; x < w; x++) {
      const o = (rowOut + x) * 3;
      let r = 0, g = 0, b = 0;
      for (let i = -2; i <= 2; i++) {
        if ((x + i) & 1) continue;
        const t = 2 * TAPS[i + 2];
        const q = (rowIn + clampi((x + i) >> 1, 0, p.w - 1)) * 3;
        r += t * p.data[q];
        g += t * p.data[q + 1];
        b += t * p.data[q + 2];
      }
      tmp[o] = r;
      tmp[o + 1] = g;
      tmp[o + 2] = b;
    }
  }
  const out = new Float32Array(w * h * 3);
  for (let y = 0; y < h; y++) {
    const rowOut = y * w;
    for (let x = 0; x < w; x++) {
      const o = (rowOut + x) * 3;
      let r = 0, g = 0, b = 0;
      for (let j = -2; j <= 2; j++) {
        if ((y + j) & 1) continue;
        const t = 2 * TAPS[j + 2];
        const q = (clampi((y + j) >> 1, 0, p.h - 1) * w + x) * 3;
        r += t * tmp[q];
        g += t * tmp[q + 1];
        b += t * tmp[q + 2];
      }
      out[o] = r;
      out[o + 1] = g;
      out[o + 2] = b;
    }
  }
  return { w, h, data: out };
}


/** a − b, pixel by pixel (same size). */
export function sub(a: Plane, b: Plane): Plane {
  const data = new Float32Array(a.data.length);
  for (let i = 0; i < data.length; i++) data[i] = a.data[i] - b.data[i];
  return { w: a.w, h: a.h, data };
}

/** A Laplacian pyramid of `n` detail levels (`detail[0]` finest) and `rest`; collapsing it gives the image back exactly. */
export function pyramid(image: Plane, n: number): { detail: Plane[]; rest: Plane } {
  const detail: Plane[] = [];
  let g = image;
  for (let l = 0; l < n; l++) {
    const next = down(g);
    detail.push(sub(g, up(next, g.w, g.h)));
    g = next;
  }
  return { detail, rest: g };
}

/** A plane of a pyramid's level `level` (0 = full size) scaled back up to full size `w`×`h`, as collapsing does. */
export function expand(p: Plane, level: number, w: number, h: number): Plane {
  // The sizes of the levels above it, so each scale-up lands exactly where collapsing would.
  const sizes: [number, number][] = [[w, h]];
  for (let l = 1; l < level; l++) sizes.push([Math.max(1, Math.ceil(sizes[l - 1][0] / 2)), Math.max(1, Math.ceil(sizes[l - 1][1] / 2))]);
  let q = p;
  for (let l = level - 1; l >= 0; l--) q = up(q, sizes[l][0], sizes[l][1]);
  return q;
}
