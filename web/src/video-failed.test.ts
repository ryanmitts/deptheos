import assert from "node:assert/strict";
import { test } from "node:test";
import { failedForGood, onVideoFailed } from "./video-failed.ts";

const NETWORK_LOADING = 2, NETWORK_NO_SOURCE = 3;
const av1 = { name: "av1 source" }, h264 = { name: "h264 source" };

/** A video with an AV1 source then an H.264 one. */
function video(networkState: number, error: MediaError | null = null) {
  return { error, networkState, querySelector: (s: string) => (s === "source:last-of-type" ? h264 : null) };
}

test("the first source failing while the next loads is not a failure", () => {
  assert.equal(failedForGood(video(NETWORK_LOADING), av1 as unknown as EventTarget), false);
});

test("the last source failing is", () => {
  assert.equal(failedForGood(video(NETWORK_LOADING), h264 as unknown as EventTarget), true);
});

test("no source left, or an error on the video itself, is", () => {
  assert.equal(failedForGood(video(NETWORK_NO_SOURCE), av1 as unknown as EventTarget), true);
  assert.equal(failedForGood(video(NETWORK_LOADING, { code: 3 } as MediaError), null), true);
});

test("onVideoFailed fires immediately if the video already failed before registration", () => {
  let called = 0;
  const alreadyDead = {
    error: null,
    networkState: NETWORK_NO_SOURCE,
    addEventListener: () => {
      throw new Error("should not attach listener when already failed");
    },
  } as unknown as HTMLVideoElement;
  onVideoFailed(alreadyDead, () => called++);
  assert.equal(called, 1);
});
