// `node --test scripts/`: lookups a few at a time (src/lib/lookupQueue.ts),
// as the Navidrome genre tiles look up their covers.

import assert from "node:assert/strict";
import { test } from "node:test";
import { createLookupQueue } from "../src/lib/lookupQueue.ts";

/** A queue whose lookups wait until `answer(key, value)` (or `fail(key)`). */
function setup(concurrency = 1) {
  const started = [];
  const pending = new Map();
  const queue = createLookupQueue(
    (key) =>
      new Promise((resolve, reject) => {
        started.push(key);
        pending.set(key, { resolve, reject });
      }),
    concurrency,
  );
  const answer = async (key, value) => {
    pending.get(key).resolve(value);
    await new Promise((r) => setTimeout(r, 0));
  };
  const fail = async (key) => {
    pending.get(key).reject(new Error("no"));
    await new Promise((r) => setTimeout(r, 0));
  };
  return { queue, started, answer, fail };
}

test("one at a time, in order; answers are remembered", async () => {
  const { queue, started, answer } = setup();
  const got = [];
  queue.request("a", (v) => got.push(["a", v]));
  queue.request("b", (v) => got.push(["b", v]));
  assert.deepEqual(started, ["a"]);
  assert.deepEqual(queue.queued(), ["b"]);
  await answer("a", "cover-a");
  assert.deepEqual(started, ["a", "b"]);
  await answer("b", null);
  assert.deepEqual(got, [["a", "cover-a"], ["b", null]]);
  // Asked again: answered from memory, no lookup.
  queue.request("a", (v) => got.push(["a again", v]));
  assert.deepEqual(started, ["a", "b"]);
  assert.deepEqual(got.at(-1), ["a again", "cover-a"]);
});

test("unmounting (every asker cancels) leaves nothing queued", async () => {
  const { queue, started, answer } = setup();
  const cancels = ["a", "b", "c", "d"].map((k) => queue.request(k, () => assert.fail(`told ${k}`)));
  assert.deepEqual(queue.queued(), ["b", "c", "d"]);
  cancels.forEach((cancel) => cancel());
  assert.deepEqual(queue.queued(), []);
  // The one already running finishes and is remembered; nothing else starts.
  await answer("a", "cover-a");
  assert.deepEqual(started, ["a"]);
  assert.equal(queue.known("a"), "cover-a");
});

test("a tile leaving the view before its turn leaves the queue; another asker keeps it", () => {
  const { queue } = setup();
  queue.request("a", () => {});
  const leave = queue.request("b", () => {});
  queue.request("c", () => {});
  const leaveC = queue.request("c", () => {});
  leave();
  leaveC();
  assert.deepEqual(queue.queued(), ["c"]);
});

test("clear drops what is queued and remembered, and ignores running answers", async () => {
  const { queue, started, answer } = setup();
  queue.request("a", () => assert.fail("told after clear"));
  queue.request("b", () => assert.fail("told after clear"));
  queue.clear();
  assert.deepEqual(queue.queued(), []);
  await answer("a", "stale");
  assert.equal(queue.known("a"), undefined);
  assert.deepEqual(started, ["a"]);
  // Works afterwards.
  let got;
  queue.request("b", (v) => (got = v));
  assert.deepEqual(started, ["a", "b"]);
  await answer("b", "cover-b");
  assert.equal(got, "cover-b");
});

test("a failed lookup is not remembered", async () => {
  const { queue, started, fail, answer } = setup();
  let got = "unset";
  queue.request("a", (v) => (got = v));
  await fail("a");
  assert.equal(got, null);
  assert.equal(queue.known("a"), undefined);
  queue.request("a", () => {});
  assert.deepEqual(started, ["a", "a"]);
  await answer("a", "x");
});

test("drop (Genres unmounting) empties the queue and keeps the answers", async () => {
  const { queue, started, answer } = setup();
  queue.request("a", () => {});
  queue.request("b", () => assert.fail("told after drop"));
  queue.request("c", () => assert.fail("told after drop"));
  await answer("a", "cover-a");
  queue.drop();
  assert.deepEqual(queue.queued(), []);
  await answer("b", "cover-b");
  assert.deepEqual(started, ["a", "b"]);
  assert.equal(queue.known("a"), "cover-a");
});
