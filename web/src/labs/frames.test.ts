import assert from "node:assert/strict";
import { test } from "node:test";
import { frameColor, frameLut } from "./frames.ts";

test("frameColor returns valid 0..255 RGB for single-frame and multi-frame stacks", () => {
  for (const [frame, frames] of [
    [0, 1],
    [0, 66],
    [33, 66],
    [65, 66],
  ] as const) {
    const [r, g, b] = frameColor(frame, frames);
    for (const c of [r, g, b]) {
      assert.ok(Number.isInteger(c) && c >= 0 && c <= 255, `invalid channel ${c}`);
    }
  }
});

test("frameLut clamps indices past frames - 1 to the last frame color", () => {
  const lut = frameLut(10);
  assert.equal(lut.length, 256);
  assert.deepEqual(lut[9], lut[10]);
  assert.deepEqual(lut[9], lut[255]);
  assert.notDeepEqual(lut[0], lut[9]);
});
