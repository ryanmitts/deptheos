import assert from "node:assert/strict";
import { beforeEach, test } from "node:test";
import { autoplayWhenVisible, Timeline, type PauseCause } from "./timeline.ts";

// Animation frames by hand: `frame(t)` runs the callbacks pending, at time t.
let pending = new Map<number, (now: number) => void>();
let nextId = 1;
const g = globalThis as Record<string, unknown>;
g.requestAnimationFrame = (fn: (now: number) => void) => {
  pending.set(nextId, fn);
  return nextId++;
};
g.cancelAnimationFrame = (id: number) => pending.delete(id);
const frame = (now: number) => {
  const due = [...pending.values()];
  pending = new Map();
  for (const fn of due) fn(now);
};

// One element's visibility, for autoplayWhenVisible.
let setVisible: (visible: boolean) => void = () => {};
g.window = { matchMedia: () => ({ matches: false }) };
g.IntersectionObserver = class {
  constructor(private fn: (entries: { isIntersecting: boolean }[]) => void) {
    setVisible = (visible) => this.fn([{ isIntersecting: visible }]);
  }
  observe() {}
  disconnect() {
    setVisible = () => {};
  }
};

beforeEach(() => {
  pending = new Map();
});

test("pause then play within one frame keeps one loop", () => {
  const t = new Timeline(10, { fps: 1000 });
  t.play();
  frame(performance.now() + 100);
  t.pause();
  t.play();
  assert.equal(pending.size, 1);
  frame(performance.now() + 200);
  assert.equal(pending.size, 1);
  t.pause();
  assert.equal(pending.size, 0);
});

test("it holds the last frame, then starts again from the first", () => {
  const t = new Timeline(3, { fps: 1000, holdMs: 500 });
  t.play();
  let now = performance.now();
  frame((now += 100));
  frame((now += 100));
  assert.equal(t.frame, 2);
  frame((now += 100));
  assert.equal(t.frame, 2);
  frame((now += 500));
  assert.equal(t.frame, 0);
  assert.equal(t.playing, true);
  t.pause();
});

test("autoplay stops for good only when the reader pauses", () => {
  const t = new Timeline(2, { fps: 1000 });
  const causes: (PauseCause | undefined)[] = [];
  t.onPlayState((playing, cause) => playing || causes.push(cause));
  autoplayWhenVisible({} as HTMLElement, t);
  setVisible(true);
  assert.equal(t.playing, true);
  // Scrolled away, it pauses and plays again when back.
  setVisible(false);
  assert.equal(t.playing, false);
  setVisible(true);
  assert.equal(t.playing, true);
  // The reader's pause sticks.
  t.pause();
  setVisible(false);
  setVisible(true);
  assert.equal(t.playing, false);
  assert.deepEqual(causes, ["page", "reader"]);
});

test("after the reader pauses, a Play they press still pauses when the figure leaves", () => {
  const t = new Timeline(2, { fps: 1000 });
  autoplayWhenVisible({} as HTMLElement, t);
  setVisible(true);
  t.pause();
  t.play();
  setVisible(false);
  assert.equal(t.playing, false);
  setVisible(true);
  assert.equal(t.playing, false);
});
