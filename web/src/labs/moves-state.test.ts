import assert from "node:assert/strict";
import { test } from "node:test";
import { initialMoves, nextMoves } from "./moves-state.ts";

test("it starts without the outlines", () => {
  assert.deepEqual(initialMoves, { kind: "ready", keeps: false });
});

test("the toggle turns the outlines on and off; a repeat changes nothing", () => {
  const on = nextMoves(initialMoves, { kind: "keeps", on: true });
  assert.deepEqual(on, { kind: "ready", keeps: true });
  assert.equal(nextMoves(on, { kind: "keeps", on: true }), on);
  assert.deepEqual(nextMoves(on, { kind: "keeps", on: false }), { kind: "ready", keeps: false });
});
