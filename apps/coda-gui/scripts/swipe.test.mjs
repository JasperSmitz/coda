// `node --test scripts/`: the swipe between Home, Search and Library (src/lib/swipe.ts).

import assert from "node:assert/strict";
import { test } from "node:test";
import { initialSwipe, swipe, swipeTarget, SWIPE_COOLDOWN_MS, SWIPE_IDLE_MS, SWIPE_PX } from "../src/lib/swipe.ts";

const yes = () => true;

/** Runs `events` ([dx, dy, t]) and returns the steps taken. */
function run(events, allowed = yes, state = initialSwipe) {
  const steps = [];
  for (const [dx, dy, t] of events) {
    const result = swipe(state, { dx, dy, t }, allowed);
    state = result.state;
    if (result.step !== 0) steps.push(result.step);
  }
  return { steps, state };
}

/** A gesture of `n` events of `dx` each, 16ms apart, from `t0`. */
const gesture = (dx, n, t0 = 1000, dy = 0) =>
  Array.from({ length: n }, (_, i) => [dx, dy, t0 + i * 16]);

test("a swipe switches once its travel passes the threshold", () => {
  assert.deepEqual(run(gesture(10, 11)).steps, []); // 110px
  assert.deepEqual(run(gesture(10, 12)).steps, [1]); // 120px
  assert.deepEqual(run(gesture(-30, 4)).steps, [-1]);
  assert.equal(SWIPE_PX, 120);
});

test("one gesture never switches twice, momentum included", () => {
  // 40 events of 30px (1200px) without a pause: one step.
  assert.deepEqual(run(gesture(30, 40)).steps, [1]);
  // Momentum after a short pause is the same gesture.
  const events = [...gesture(30, 5), ...gesture(20, 20, 1000 + 4 * 16 + SWIPE_IDLE_MS - 10)];
  assert.deepEqual(run(events).steps, [1]);
});

test("a pause (after the cooldown) ends the gesture; the next one switches again", () => {
  const second = 1000 + 4 * 16 + Math.max(SWIPE_IDLE_MS, SWIPE_COOLDOWN_MS);
  const events = [...gesture(30, 5), ...gesture(30, 5, second)];
  assert.deepEqual(run(events).steps, [1, 1]);
  // Travel short of the threshold is forgotten after a pause.
  const short = [...gesture(10, 10), ...gesture(10, 5, 1000 + 9 * 16 + SWIPE_IDLE_MS)];
  assert.deepEqual(run(short).steps, []);
});

test("a gesture that starts vertical is ignored to its end", () => {
  const events = [[2, 20, 1000], ...gesture(40, 10, 1016)];
  assert.deepEqual(run(events).steps, []);
  // Vertical events within a sideways gesture add nothing.
  const mixed = [...gesture(30, 3), [0, 50, 1048], [5, 60, 1064], ...gesture(30, 1, 1080)];
  assert.deepEqual(run(mixed).steps, [1]);
  // Events that do not move do not decide.
  assert.deepEqual(run([[0, 0, 1000], ...gesture(40, 3, 1016)]).steps, [1]);
});

test("where a swipe may not switch is asked once, with its direction", () => {
  const asked = [];
  const no = (direction) => {
    asked.push(direction);
    return false;
  };
  assert.deepEqual(run(gesture(-40, 10), no).steps, []);
  assert.deepEqual(asked, [-1]);
});

test("the roots in order, nothing past the ends or from Preferences", () => {
  assert.equal(swipeTarget("home", 1), "search");
  assert.equal(swipeTarget("search", 1), "library");
  assert.equal(swipeTarget("library", 1), null);
  assert.equal(swipeTarget("library", -1), "search");
  assert.equal(swipeTarget("home", -1), null);
  assert.equal(swipeTarget("prefs", 1), null);
});

test("only travel in the gesture's own direction switches", () => {
  // Starts left (-1), then the fingers go right 300px: no switch.
  const events = [[-10, 0, 1000], ...gesture(30, 10, 1016)];
  assert.deepEqual(run(events).steps, []);
  // Back and forth that ends past the threshold in its direction does.
  assert.deepEqual(run([...gesture(-60, 1), ...gesture(40, 1, 1016), ...gesture(-60, 3, 1032)]).steps, [-1]);
});

test("a long momentum tail after a switch switches exactly once", () => {
  // A firm swipe: 8 frames of 40px, then 1.5s of momentum every 16ms, decaying.
  const events = gesture(40, 8);
  let t = 1000 + 8 * 16;
  for (let i = 0; t < 1000 + 8 * 16 + 1500; i++, t += 16) events.push([40 * Math.exp(-i / 30), 0, t]);
  assert.deepEqual(run(events).steps, [1]);
});

test("a page change mid-gesture never resets the lock", () => {
  // Switch, then the page change runs (`busy`) and the webview holds
  // events back for 400ms, then delivers the momentum.
  const events = gesture(40, 4).map(([dx, dy, t]) => ({ dx, dy, t }));
  let t = 1000 + 4 * 16;
  for (let i = 0; i < 5; i++, t += 16) events.push({ dx: 30, dy: 0, t, busy: true });
  t += 400;
  for (let i = 0; i < 10; i++, t += 16) events.push({ dx: 30, dy: 0, t, busy: true });
  for (let i = 0; i < 40; i++, t += 16) events.push({ dx: 20, dy: 0, t });
  let state = initialSwipe;
  const steps = [];
  for (const e of events) {
    const r = swipe(state, e, yes);
    state = r.state;
    if (r.step) steps.push(r.step);
  }
  assert.deepEqual(steps, [1]);
  // A pause within the cooldown keeps it locked too.
  const quick = [...gesture(40, 4), ...gesture(40, 10, 1000 + 3 * 16 + SWIPE_IDLE_MS + 20)];
  assert.ok(1000 + 3 * 16 + SWIPE_IDLE_MS + 20 < 1000 + 3 * 16 + SWIPE_COOLDOWN_MS);
  assert.deepEqual(run(quick).steps, [1]);
  // After the cooldown and a pause, the next swipe switches.
  const later = [...gesture(40, 4), ...gesture(40, 4, 1000 + 3 * 16 + SWIPE_COOLDOWN_MS + SWIPE_IDLE_MS)];
  assert.deepEqual(run(later).steps, [1, 1]);
});

test("nothing starts while a page change runs", () => {
  const events = gesture(40, 6).map(([dx, dy, t]) => ({ dx, dy, t, busy: true }));
  let state = initialSwipe;
  let steps = 0;
  for (const e of events) {
    const r = swipe(state, e, yes);
    state = r.state;
    steps += Math.abs(r.step);
  }
  assert.equal(steps, 0);
});
