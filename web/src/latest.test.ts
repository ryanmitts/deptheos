import assert from "node:assert/strict";
import { test } from "node:test";
import { latest } from "./latest.ts";

/** A promise with its resolve and reject, for ordering work by hand. */
function deferred<T>() {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

test("a later run supersedes an earlier one that finishes after it", async () => {
  const l = latest();
  const applied: string[] = [];
  const slow = deferred<string>();
  const fast = deferred<string>();
  let slowSignal: AbortSignal | undefined;
  const a = l.run((s) => ((slowSignal = s), slow.promise), (v) => applied.push(v));
  const b = l.run(() => fast.promise, (v) => applied.push(v));
  assert.equal(slowSignal?.aborted, true);
  fast.resolve("fast");
  slow.resolve("slow");
  await Promise.all([a, b]);
  assert.deepEqual(applied, ["fast"]);
});

test("cancel drops the work in flight, and later runs still apply", async () => {
  const l = latest();
  const applied: number[] = [];
  const first = deferred<number>();
  const a = l.run(() => first.promise, (v) => applied.push(v));
  l.cancel();
  first.resolve(1);
  await a;
  await l.run(async () => 2, (v) => applied.push(v));
  assert.deepEqual(applied, [2]);
});

test("stop drops the work in flight, and later runs never start", async () => {
  const l = latest();
  const applied: number[] = [];
  const first = deferred<number>();
  const a = l.run(() => first.promise, (v) => applied.push(v));
  l.stop();
  first.resolve(1);
  await a;
  let started = false;
  await l.run(async () => ((started = true), 2), (v) => applied.push(v));
  assert.equal(l.stopped, true);
  assert.equal(started, false);
  assert.deepEqual(applied, []);
});

test("an aborted run's rejection is dropped, a current run's passes through", async () => {
  const l = latest();
  const first = deferred<number>();
  const a = l.run(() => first.promise, () => assert.fail("applied"));
  l.cancel();
  first.reject(new Error("aborted readback"));
  await a;
  await assert.rejects(
    l.run(async () => {
      throw new Error("real failure");
    }, () => {}),
    /real failure/,
  );
});
