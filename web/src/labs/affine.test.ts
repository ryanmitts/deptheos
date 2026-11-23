import assert from "node:assert/strict";
import { test } from "node:test";
import { apply, drawMoved, invert, motionAt, rescale, type Warp } from "./affine.ts";

const close = (a: number, b: number, eps = 1e-9) => assert.ok(Math.abs(a - b) < eps, `${a} vs ${b}`);

test("a transform and its inverse undo each other", () => {
  const w: Warp = [0.99, -0.004, -9, 0.003, 0.988, -27];
  const [x, y] = apply(invert(w), ...apply(w, 300, 200));
  close(x, 300);
  close(y, 200);
});

test("rescaling keeps pixel centers: halving a shift of 2 px gives 1 px", () => {
  const w = rescale([1, 0, 2, 0, 1, -4], 0.5);
  close(w[2], 1);
  close(w[5], -2);
});

test("rescaling a pure scale moves the origin as pixel centers require", () => {
  // At full size x' = 2x; at half size x = 2X + 0.5, so X' = (2x + 0.5)/2 - 0.5 = 2X + 0.25.
  const w = rescale([2, 0, 0, 0, 2, 0], 0.5);
  close(w[2], 0.25);
  close(apply(w, 10, 0)[0], 20.25);
});

test("the canvas draw of a shifted frame moves it the other way", () => {
  // The moved frame's pixel x' = x - 5 sits over reference pixel x: its content moved 5 px right.
  const [a, b, c, d, e, f] = drawMoved([1, 0, -5, 0, 1, 0]);
  [a, b, c, d].forEach((v, i) => close(v, [1, 0, 0, 1][i]));
  close(e, 5);
  close(f, 0);
});

test("motion at a point: the moved frame shows the picture shifted and smaller", () => {
  // The moved frame's pixel x' = x/2 sits over reference pixel x: the picture is half size, and at
  // the reference's (100, 100) it sits at (50, 50).
  const m = motionAt([0.5, 0, 0, 0, 0.5, 0], 100, 100);
  close(m.dx, -50);
  close(m.size, 0.5);
});
