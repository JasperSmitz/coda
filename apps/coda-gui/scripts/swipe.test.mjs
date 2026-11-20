// `node --test scripts/`: the swipe between Home, Search and Library (src/lib/swipe.ts).

import assert from "node:assert/strict";
import { test } from "node:test";
import {
  blocksSwipe,
  initialSwipe,
  swipe,
  swipeTarget,
  SWIPE_GAP_MS,
  SWIPE_PX,
  SWIPE_SETTLE_MS,
} from "../src/lib/swipe.ts";

const yes = () => true;

/** Runs events ({dx, dy, t, busy, changeEnded} or [dx, dy, t]); returns [step, t] pairs. */
function run(events, allowed = yes, state = initialSwipe) {
  const steps = [];
  for (const e of events) {
    const input = Array.isArray(e) ? { dx: e[0], dy: e[1], t: e[2] } : e;
    const r = swipe(state, input, allowed);
    state = r.state;
    if (r.step !== 0) steps.push([r.step, input.t]);
  }
  return { steps, state, dirs: steps.map(([s]) => s) };
}

/** `n` events of `dx` each, 16ms apart, from `t0`. */
const gesture = (dx, n, t0 = 1000, dy = 0) => Array.from({ length: n }, (_, i) => [dx, dy, t0 + i * 16]);

/** A firm swipe from `t0`: 8 frames of 40px, then momentum decaying every 16ms for `ms`. */
function firm(t0, ms, direction = 1) {
  const events = gesture(40 * direction, 8, t0);
  let t = t0 + 8 * 16;
  for (let i = 0; t < t0 + 8 * 16 + ms; i++, t += 16) events.push([40 * direction * Math.exp(-i / 30), 0, t]);
  return events;
}

test("a swipe switches once its travel passes the threshold", () => {
  assert.deepEqual(run(gesture(10, 11)).dirs, []); // 110px
  assert.deepEqual(run(gesture(10, 12)).dirs, [1]); // 120px
  assert.deepEqual(run(gesture(-30, 4)).dirs, [-1]);
  assert.equal(SWIPE_PX, 120);
});

test("a long momentum tail switches once", () => {
  assert.deepEqual(run(firm(1000, 1500)).dirs, [1]);
  assert.deepEqual(run(gesture(30, 200)).dirs, [1]);
});

test("a momentum tail delivered in batches switches once", () => {
  for (const groups of [[2, 3, 4], [1, 2, 4, 8], [1, 2, 3]]) {
    for (const start of [25, 40, 60]) {
      const ev = gesture(40, 8);
      let t = 1000 + 7 * 16;
      const tail = Array.from({ length: 150 }, (_, i) => 40 * Math.exp(-i / 30));
      for (let i = 0, g = 0; i < tail.length; ) {
        const n = i >= start && g < groups.length ? groups[g++] : 1;
        t += 16 * n;
        ev.push([tail.slice(i, i + n).reduce((a, b) => a + b, 0), 0, t]);
        i += n;
      }
      assert.deepEqual(run(ev).dirs, [1], `${groups} at ${start}`);
    }
  }
});

test("events delivered late after a page change (a 300ms gap, then a burst) do not switch", () => {
  // Switch at t=1032; the page change runs until 1032 + 350; the webview
  // held events back and delivers them as a burst 300ms after the last one.
  const ev = gesture(40, 8).map(([dx, dy, t]) => ({ dx, dy, t, busy: t > 1032 }));
  const changeEnded = 1032 + 350;
  let t = 1000 + 7 * 16 + 300;
  for (let i = 0; i < 12; i++, t += 4) ev.push({ dx: 60, dy: 0, t, busy: t <= changeEnded, changeEnded: t > changeEnded ? changeEnded : undefined });
  // And momentum on after it, every frame.
  for (let i = 0; i < 60; i++, t += 16) ev.push({ dx: 20 * Math.exp(-i / 20), dy: 0, t, changeEnded });
  assert.deepEqual(run(ev).dirs, [1]);
});

test("a stream that starts before the settle time is ignored for its whole length", () => {
  const changeEnded = 1400;
  // The first swipe, then a second starting 100ms after the change ended (too early).
  const ev = [...gesture(40, 4)];
  for (let i = 0, t = changeEnded + 100; i < 30; i++, t += 16) ev.push({ dx: 40, dy: 0, t, changeEnded });
  assert.deepEqual(run(ev).dirs, [1]);
});

test("a second swipe after the quiet gap and the settle time switches", () => {
  const changeEnded = 1032 + 350;
  const first = gesture(40, 4).map(([dx, dy, t]) => ({ dx, dy, t, busy: t > 1032 }));
  const start = changeEnded + SWIPE_SETTLE_MS + 10;
  assert.ok(start - first[first.length - 1].t >= SWIPE_GAP_MS);
  const second = gesture(-40, 5, start).map(([dx, dy, t]) => ({ dx, dy, t, changeEnded }));
  assert.deepEqual(run([...first, ...second]).dirs, [1, -1]);
  // Without a page change (no View Transitions): settle counts from the switch.
  const plain = [...gesture(40, 4), ...gesture(40, 5, 1032 + SWIPE_SETTLE_MS + 10)];
  assert.deepEqual(run(plain).dirs, [1, 1]);
});

test("one stream never switches twice, however long, until a quiet gap", () => {
  // 3 seconds of continuous events: one switch.
  assert.deepEqual(run(gesture(30, 190)).dirs, [1]);
  // A gap just under the threshold keeps it one stream.
  const ev = [...gesture(40, 4), ...gesture(40, 10, 1000 + 3 * 16 + SWIPE_GAP_MS - 10)];
  assert.deepEqual(run(ev).dirs, [1]);
});

test("a stream that starts vertical is ignored to its end; vertical events add nothing", () => {
  assert.deepEqual(run([[2, 20, 1000], ...gesture(40, 10, 1016)]).dirs, []);
  const mixed = [...gesture(30, 3), [0, 50, 1048], [5, 60, 1064], ...gesture(30, 1, 1080)];
  assert.deepEqual(run(mixed).dirs, [1]);
  assert.deepEqual(run([[0, 0, 1000], ...gesture(40, 3, 1016)]).dirs, [1]);
});

test("only travel in the stream's own direction switches", () => {
  assert.deepEqual(run([[-10, 0, 1000], ...gesture(30, 10, 1016)]).dirs, []);
});

test("where a swipe may not start is asked once, with its direction; then ignored to the end", () => {
  const asked = [];
  const no = (direction) => {
    asked.push(direction);
    return false;
  };
  assert.deepEqual(run(gesture(-40, 10), no).dirs, []);
  assert.deepEqual(asked, [-1]);
});

test("nothing starts while a page change runs", () => {
  const ev = gesture(40, 6).map(([dx, dy, t]) => ({ dx, dy, t, busy: true }));
  assert.deepEqual(run(ev).dirs, []);
});

test("the roots in order, nothing past the ends or from Preferences", () => {
  assert.equal(swipeTarget("home", 1), "search");
  assert.equal(swipeTarget("search", 1), "library");
  assert.equal(swipeTarget("library", 1), null);
  assert.equal(swipeTarget("library", -1), "search");
  assert.equal(swipeTarget("home", -1), null);
  assert.equal(swipeTarget("prefs", 1), null);
});

test("each event says what it did (for the trace)", () => {
  let state = initialSwipe;
  const notes = [];
  for (const [dx, dy, t] of [...gesture(50, 4), [10, 0, 1064], [0, 30, 2000]]) {
    const r = swipe(state, { dx, dy, t }, yes);
    state = r.state;
    notes.push(r.note);
  }
  assert.deepEqual(notes, ["start", "accumulate", "switch", "locked", "locked", "ignored:vertical"]);
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
