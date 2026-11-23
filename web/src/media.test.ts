import assert from "node:assert/strict";
import { test } from "node:test";
import { resolveMedia, setOf, type MediaTarget } from "./media.ts";

const target: MediaTarget = {
  base: "https://images.example/site",
  sets: { "gallery/hornet": "aaaa", "labs/pick": "bbbb", home: "cccc", turntable: "dddd" },
};

test("a set is the first two folders, or the first for a file directly in it", () => {
  assert.equal(setOf("gallery/hornet/1200.jpg"), "gallery/hornet");
  assert.equal(setOf("gallery/hornet/tiles/0-0.jpg"), "gallery/hornet");
  assert.equal(setOf("gallery/hornet/tiles"), "gallery/hornet");
  assert.equal(setOf("turntable/pcb.av1.mp4"), "turntable");
  assert.equal(setOf("home/hornet-frame-2400.jpg"), "home");
});

test("with no target a path is its own address", () => {
  assert.equal(resolveMedia("/media/labs/pick/w1.jpg", null), "/media/labs/pick/w1.jpg");
});

test("with a target the set's hash follows the set", () => {
  assert.equal(resolveMedia("/media/labs/pick/w1.jpg", target), "https://images.example/site/labs/pick/bbbb/w1.jpg");
  assert.equal(resolveMedia("/media/gallery/hornet/tiles/0-0.jpg", target), "https://images.example/site/gallery/hornet/aaaa/tiles/0-0.jpg");
  assert.equal(resolveMedia("/media/gallery/hornet/tiles", target), "https://images.example/site/gallery/hornet/aaaa/tiles");
  assert.equal(resolveMedia("/media/home/hornet-frame-2400.jpg", target), "https://images.example/site/home/cccc/hornet-frame-2400.jpg");
  assert.equal(resolveMedia("/media/turntable/pcb-poster.jpg", target), "https://images.example/site/turntable/dddd/pcb-poster.jpg");
});

test("a path outside /media/, a root-level media file, and a set the target lacks are errors", () => {
  assert.throws(() => resolveMedia("/gallery/hornet/1200.jpg", target), /not a media path/);
  assert.throws(() => setOf("orphan.jpg"), /must live inside a set folder/);
  assert.throws(() => resolveMedia("/media/gallery/wasp/1200.jpg", target), /no media set "gallery\/wasp"/);
});
