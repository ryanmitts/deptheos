import assert from "node:assert/strict";
import { test } from "node:test";
import { initialDepthMap, lit, nextDepthMap } from "./depthmap-state.ts";

const ready = nextDepthMap(initialDepthMap, { kind: "frame", frame: 5 });

test("it starts on the first frame", () => {
  assert.deepEqual(initialDepthMap, { kind: "ready", frame: 0 });
});

test("the slider or a tap moves to another frame", () => {
  assert.deepEqual(nextDepthMap(ready, { kind: "frame", frame: 24 }), { kind: "ready", frame: 24 });
});

test("events that change nothing return the same state", () => {
  assert.equal(nextDepthMap(ready, { kind: "frame", frame: 5 }), ready);
});

test("a frame lights its own depth and its neighbors'", () => {
  assert.ok(lit(10, 10) && lit(9, 10) && lit(11, 10));
  assert.ok(!lit(8, 10) && !lit(12, 10));
});
