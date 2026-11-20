// `node --test scripts/`: the swipe between Home, Search and Library (src/lib/swipe.ts).

import assert from "node:assert/strict";
import { test } from "node:test";
import {
  blocksSwipe,
  initialSwipe,
  swipe,
  swipeTarget,
  SWIPE_COOLDOWN_MS,
  SWIPE_IDLE_MS,
  SWIPE_PX,
} from "../src/lib/swipe.ts";

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

/** A firm swipe from `t0`: fingers (8 × 40px), then momentum decaying for `ms`. */
function firm(t0, ms, direction = 1) {
  const events = gesture(40 * direction, 8, t0);
  let t = t0 + 8 * 16;
  for (let i = 0; t < t0 + 8 * 16 + ms; i++, t += 16) events.push([40 * direction * Math.exp(-i / 30), 0, t]);
  return events;
}

test("a second deliberate swipe 400ms after the first switches again, inside the momentum", () => {
  // The first swipe's momentum is still coming when the fingers land again.
  const first = firm(1000, 400 - 8 * 16);
  const t1 = first[first.length - 1][2] + 16;
  const second = [2, 5, 12, 25, 40, 40, 40, 40].map((dx, i) => [dx, 0, t1 + i * 16]);
  assert.ok(t1 - 1000 <= 400);
  assert.deepEqual(run([...first, ...second]).steps, [1, 1]);
  // Back the other way too.
  const back = [-2, -5, -12, -25, -40, -40, -40, -40].map((dx, i) => [dx, 0, t1 + i * 16]);
  assert.deepEqual(run([...first, ...back]).steps, [1, -1]);
});

test("momentum alone never looks like a new swipe (noisy, repeating decay)", () => {
  const events = gesture(40, 8);
  let t = 1000 + 8 * 16;
  // Steps down with repeats and a single uptick now and then.
  const tail = [30, 30, 28, 29, 27, 27, 25, 26, 24, 22, 22, 20, 21, 19, 17, 17, 15, 14, 14, 12, 13, 11, 10, 9, 9, 8, 7, 7, 6, 5, 5, 4, 3, 3, 2, 2, 1, 1, 1];
  for (const dx of [...tail, ...tail.slice(-10), ...Array(40).fill(1)]) {
    events.push([dx, 0, t]);
    t += 16;
  }
  assert.deepEqual(run(events).steps, [1]);
});

test("the cooldown is short", () => {
  assert.ok(SWIPE_COOLDOWN_MS <= 400);
});

// ----- where a swipe may start -----

/** A node chain, innermost first: each `[tag, props]`. */
function chain(...nodes) {
  let parent = null;
  for (const [tag, props = {}] of nodes.reverse()) {
    parent = {
      tag,
      classes: [],
      overflowX: "visible",
      scrollWidth: 100,
      clientWidth: 100,
      scrollLeft: 0,
      parent,
      ...props,
    };
  }
  return parent;
}

test("a swipe over the Search field switches; over a field whose text scrolls, not", () => {
  const field = chain(["input", { inputType: "search", classes: ["search-input"] }], ["div"], ["main", { overflowX: "hidden" }]);
  assert.equal(blocksSwipe(field, 1), false);
  assert.equal(blocksSwipe(field, -1), false);
  const long = chain(["input", { inputType: "text", scrollWidth: 400, clientWidth: 300 }], ["div"]);
  assert.equal(blocksSwipe(long, 1), true);
  assert.equal(blocksSwipe(chain(["input", { inputType: "range" }]), 1), true);
  assert.equal(blocksSwipe(chain(["textarea"]), 1), true);
});

test("a page scroller with overflow-x hidden (or a pixel too wide) does not block", () => {
  const page = chain(["div"], ["main", { overflowX: "hidden", scrollWidth: 1402, clientWidth: 1400 }]);
  assert.equal(blocksSwipe(page, 1), false);
  // Scrollable sideways but only a pixel left: not enough.
  const nearly = chain(["div", { overflowX: "auto", scrollWidth: 1001, clientWidth: 1000 }]);
  assert.equal(blocksSwipe(nearly, 1), false);
  // A real sideways scroller blocks the way it can still go, not the other.
  const row = chain(["button"], ["div", { overflowX: "auto", scrollWidth: 2000, clientWidth: 1000, scrollLeft: 0 }]);
  assert.equal(blocksSwipe(row, 1), true);
  assert.equal(blocksSwipe(row, -1), false);
});

test("the queue panel, Now Playing, dialogs, menus and sliders block", () => {
  for (const props of [
    { classes: ["queue-popover"] },
    { classes: ["now-playing"] },
    { role: "dialog" },
    { role: "menu" },
    { role: "slider" },
    { editable: true },
  ]) {
    assert.equal(blocksSwipe(chain(["span"], ["div", props], ["main"]), 1), true, JSON.stringify(props));
  }
});

/** Runs input objects ({dx, dy, t, busy}); returns [step, t] pairs. */
function runInputs(events) {
  let state = initialSwipe;
  const steps = [];
  for (const e of events) {
    const r = swipe(state, e, yes);
    state = r.state;
    if (r.step) steps.push([r.step, e.t]);
  }
  return steps;
}

test("a second swipe whose start falls inside the page change switches once it is over", () => {
  // Switch at the 3rd event (t=1032); the page change runs ~450ms after it.
  const ev = [];
  let t = 1000;
  for (let i = 0; i < 8; i++, t += 16) ev.push({ dx: 40, dy: 0, t });
  for (let i = 0; t < 1400; i++, t += 16) ev.push({ dx: 40 * Math.exp(-i / 30), dy: 0, t });
  for (const dx of [2, 5, 12, 25, 40, 40, 40, 40, 40, 40]) {
    ev.push({ dx, dy: 0, t });
    t += 16;
  }
  for (let i = 0; i < 60; i++, t += 16) ev.push({ dx: 40 * Math.exp(-i / 30), dy: 0, t });
  for (const e of ev) e.busy = e.t > 1032 && e.t <= 1032 + 450;
  const steps = runInputs(ev);
  assert.equal(steps.length, 2, JSON.stringify(steps));
  assert.ok(steps[1][1] > 1032 + 450, "only after the page change");
});

test("a gesture that starts during a page change waits, then switches", () => {
  const ev = gesture(30, 10).map(([dx, dy, t]) => ({ dx, dy, t, busy: t < 1000 + 5 * 16 }));
  assert.deepEqual(runInputs(ev).map(([s]) => s), [1]);
});

test("momentum delivered in batches (deltas summed while the webview was busy) switches once", () => {
  for (const groups of [[2, 3, 4], [1, 2, 4, 8], [1, 2, 3]]) {
    for (const start of [25, 40, 60]) {
      const ev = gesture(40, 8).map(([dx, dy, t]) => ({ dx, dy, t }));
      let t = 1000 + 7 * 16;
      const tail = Array.from({ length: 150 }, (_, i) => 40 * Math.exp(-i / 30));
      const raw = [];
      for (let i = 0, g = 0; i < tail.length; ) {
        const n = i >= start && g < groups.length ? groups[g++] : 1;
        const dx = tail.slice(i, i + n).reduce((a, b) => a + b, 0);
        t += 16 * n;
        ev.push({ dx, dy: 0, t });
        raw.push(dx);
        i += n;
      }
      // The raw deltas do rise, three in a row: a delta-based rule would see fingers.
      const at = raw.findIndex((_, i) => i >= 3 && raw[i] > raw[i - 1] && raw[i - 1] > raw[i - 2] && raw[i - 2] > raw[i - 3]);
      if (groups[0] !== 1 || groups.length === 4) assert.ok(at >= 0, `raw deltas rise (${groups})`);
      assert.equal(runInputs(ev).length, 1, `${groups} at ${start}`);
    }
  }
});
