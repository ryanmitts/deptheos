// The aligner's transforms for the docs figures: row-major [a, b, tx, c, d, ty]
// on pixel centers, reference to moved frame, so the moved frame's pixel
// x' = A x + t sits over reference pixel x.

export type Warp = [number, number, number, number, number, number];

export const IDENTITY: Warp = [1, 0, 0, 0, 1, 0];

/** `w` for frames scaled by `k` (a level of the aligner's pyramid, or a smaller copy). */
export function rescale(w: Warp, k: number): Warp {
  const [a, b, tx, c, d, ty] = w;
  return [a, b, k * tx + ((k - 1) * (1 - a - b)) / 2, c, d, k * ty + ((k - 1) * (1 - c - d)) / 2];
}

/** The inverse transform: moved frame → reference. */
export function invert(w: Warp): Warp {
  const [a, b, tx, c, d, ty] = w;
  const det = a * d - b * c;
  if (det === 0) throw new Error("singular affine transform");
  const [ia, ib, ic, id] = [d / det, -b / det, -c / det, a / det];
  return [ia, ib, -(ia * tx + ib * ty), ic, id, -(ic * tx + id * ty)];
}

/** Where `w` takes the point (x, y). */
export function apply(w: Warp, x: number, y: number): [number, number] {
  return [w[0] * x + w[1] * y + w[2], w[3] * x + w[4] * y + w[5]];
}

/** The `setTransform` that draws the moved frame over the reference; the canvas counts from pixel corners. */
export function drawMoved(w: Warp): [number, number, number, number, number, number] {
  // In corner coordinates the moved frame's u' = A u + t + (1 - row sums) / 2; draw with its inverse.
  const [a, b, tx, c, d, ty] = w;
  const corner: Warp = [a, b, tx + (1 - a - b) / 2, c, d, ty + (1 - c - d) / 2];
  const [ia, ib, itx, ic, id, ity] = invert(corner);
  return [ia, ic, ib, id, itx, ity];
}

/** How far the moved frame's picture moved at (cx, cy), in pixels, and its scale (1 = the same). */
export function motionAt(w: Warp, cx: number, cy: number): { dx: number; dy: number; size: number } {
  // The scene at the reference's (cx, cy) is at (x, y) in the moved frame.
  const [x, y] = apply(w, cx, cy);
  return { dx: x - cx, dy: y - cy, size: Math.sqrt(Math.abs(w[0] * w[4] - w[1] * w[3])) };
}
