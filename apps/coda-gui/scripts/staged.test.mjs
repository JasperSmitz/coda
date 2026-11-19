// `node --test scripts/`: the two-step state behind page transitions
// (src/lib/staged.ts).

import assert from "node:assert/strict";
import { test } from "node:test";
import { createStaged } from "../src/lib/staged.ts";

/** A staged number whose changes run as transitions when `on`; started ones wait in `started`. */
function setup(on = true) {
  const events = [];
  const started = [];
  const staged = createStaged(0, {
    transitions: () => on,
    start: (from, to, publish) => {
      events.push(`start ${from}->${to}`);
      started.push(publish);
    },
    notify: () => events.push(`notify ${staged.shown}`),
  });
  return { staged, events, started };
}

test("without transitions every change is shown at once", () => {
  const { staged, events } = setup(false);
  staged.set(1);
  assert.equal(staged.shown, 1);
  assert.deepEqual(events, ["notify 1"]);
  let ran = false;
  staged.whenShown(() => (ran = true));
  assert.ok(ran);
});

test("a transition shows its change only when it publishes", () => {
  const { staged, events, started } = setup();
  staged.set(1);
  assert.equal(staged.state, 1);
  assert.equal(staged.shown, 0);
  assert.ok(staged.pending);
  assert.equal(started[0](), 1);
  assert.equal(staged.shown, 1);
  assert.ok(!staged.pending);
  assert.deepEqual(events, ["start 0->1", "notify 1"]);
});

test("changes while a publish is pending go with it: one transition", () => {
  const { staged, events, started } = setup();
  staged.set(1);
  staged.set(2);
  staged.set(3);
  assert.equal(started.length, 1);
  assert.equal(staged.state, 3);
  started[0]();
  assert.equal(staged.shown, 3);
  assert.deepEqual(events, ["start 0->1", "notify 3"]);
  // After the publish, a new change starts a new transition from what is shown.
  staged.set(4);
  assert.deepEqual(events.at(-1), "start 3->4");
});

test("what waits for the change runs after the views were told, in order", () => {
  const { staged, events, started } = setup();
  staged.set(1);
  staged.whenShown(() => events.push("after a"));
  staged.whenShown(() => events.push("after b"));
  assert.deepEqual(events, ["start 0->1"]);
  started[0]();
  assert.deepEqual(events, ["start 0->1", "notify 1", "after a", "after b"]);
});

test("setting the same state again does nothing", () => {
  const { staged, events } = setup();
  staged.set(0);
  assert.deepEqual(events, []);
  assert.ok(!staged.pending);
});
