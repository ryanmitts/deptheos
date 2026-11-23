import assert from "node:assert/strict";
import { test } from "node:test";
import { down, expand, pyramid, up, type Plane } from "./laplacian.ts";

function makePlane(w: number, h: number): Plane {
  const data = new Float32Array(w * h * 3);
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const i = (y * w + x) * 3;
      data[i] = (x + 1) / (w + 1);
      data[i + 1] = (y + 1) / (h + 1);
      data[i + 2] = ((x * 3 + y * 5) % 11) / 10;
    }
  }
  return { w, h, data };
}

test("pyramid collapses back to the original plane for odd and even dimensions", () => {
  for (const [w, h, levels] of [
    [13, 9, 3],
    [5, 7, 3],
    [1, 1, 2],
    [16, 12, 4],
  ] as const) {
    const img = makePlane(w, h);
    const { detail, rest } = pyramid(img, levels);
    const rebuilt = expand(rest, levels, w, h).data;
    for (let l = 0; l < levels; l++) {
      const d = expand(detail[l], l, w, h).data;
      for (let i = 0; i < rebuilt.length; i++) rebuilt[i] += d[i];
    }
    let worst = 0;
    for (let i = 0; i < img.data.length; i++) worst = Math.max(worst, Math.abs(rebuilt[i] - img.data[i]));
    assert.ok(worst < 1e-5, `${w}x${h} reconstruction off by ${worst}`);
  }
});

test("down and up preserve constant planes exactly", () => {
  const w = 9;
  const h = 7;
  const data = new Float32Array(w * h * 3).fill(0.42);
  const half = down({ w, h, data });
  assert.equal(half.w, 5);
  assert.equal(half.h, 4);
  for (const v of half.data) assert.ok(Math.abs(v - 0.42) < 1e-6);
  const back = up(half, w, h);
  assert.equal(back.w, w);
  assert.equal(back.h, h);
  for (const v of back.data) assert.ok(Math.abs(v - 0.42) < 1e-6);
});
