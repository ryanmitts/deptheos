import assert from "node:assert/strict";
import { test } from "node:test";
import { nextStops, type StopsEvent, type StopsState } from "./stops-state.ts";

const run = (s: StopsState, ...events: StopsEvent[]) => events.reduce(nextStops, s);
const ready: StopsState = { kind: "ready", stop: 1, place: 0 };

test("choosing a stop or a place changes only that", () => {
  assert.deepEqual(run(ready, { kind: "stop", stop: 2 }), { kind: "ready", stop: 2, place: 0 });
  assert.deepEqual(run(ready, { kind: "place", place: 3 }, { kind: "stop", stop: 0 }), { kind: "ready", stop: 0, place: 3 });
});

test("choosing what is already chosen changes nothing", () => {
  assert.equal(nextStops(ready, { kind: "stop", stop: 1 }), ready);
  assert.equal(nextStops(ready, { kind: "place", place: 0 }), ready);
});
