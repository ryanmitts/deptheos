import assert from "node:assert/strict";
import { test } from "node:test";
import { ALL_A, ALL_B, FINE_FROM_A, initialMixer, nextMixer, type MixerEvent, type MixerState } from "./mixer-state.ts";
import { mix, prepare, tile } from "./mixer-math.ts";

const run = (s: MixerState, ...events: MixerEvent[]) => events.reduce(nextMixer, s);
const ready = initialMixer;

test("it starts showing frame B", () => {
  assert.deepEqual(ready, { kind: "ready", fromA: ALL_B });
});

test("taking a level from A changes only that level", () => {
  assert.deepEqual(run(ready, { kind: "take", level: 0, fromA: true }), { kind: "ready", fromA: [true, false, false, false, false] });
});

test("a shortcut sets every level", () => {
  assert.deepEqual(run(ready, { kind: "set", fromA: FINE_FROM_A }), { kind: "ready", fromA: FINE_FROM_A });
});

test("a choice that changes nothing returns the same state", () => {
  assert.equal(nextMixer(ready, { kind: "take", level: 2, fromA: false }), ready);
  assert.equal(nextMixer(ready, { kind: "set", fromA: ALL_B }), ready);
});

// The arithmetic: every level from A gives A back, none gives B, whatever the pictures.
test("the mix of all A is A, of none is B", () => {
  const w = 40;
  const h = 28;
  const plane = (f: (x: number, y: number, c: number) => number) => {
    const data = new Float32Array(w * h * 3);
    for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) for (let c = 0; c < 3; c++) data[(y * w + x) * 3 + c] = f(x, y, c);
    return { w, h, data };
  };
  const a = plane((x, y, c) => (Math.sin(x * 0.7 + c) * Math.cos(y * 0.3) + 1) / 2);
  const b = plane((x, y) => ((x * 7 + y * 3) % 17) / 17);
  const m = prepare(a, b);
  const out = new Uint8ClampedArray(w * h * 4);
  for (const [fromA, want] of [[ALL_A, a], [ALL_A.map(() => false), b]] as const) {
    mix(m, fromA, out);
    let worst = 0;
    for (let i = 0; i < w * h; i++) for (let c = 0; c < 3; c++) worst = Math.max(worst, Math.abs(out[i * 4 + c] - want.data[i * 3 + c] * 255));
    assert.ok(worst <= 0.51, `off by ${worst}`);
  }
  // A frame's levels, as the tiles show them, add back up to the frame.
  for (const [fromA, want] of [[true, a], [false, b]] as const) {
    let worst = 0;
    const levels = m.differences.map((_, i) => tile(m, i, fromA, 0, 0, w, h));
    for (let i = 0; i < w * h * 3; i++) worst = Math.max(worst, Math.abs(levels.reduce((sum, l) => sum + l[i], 0) - want.data[i]));
    assert.ok(worst < 1e-4, `levels off by ${worst}`);
  }
});
