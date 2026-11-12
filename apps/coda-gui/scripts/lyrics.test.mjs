// `node --test scripts/`: lyrics mode's pure logic (src/lib/lyrics.ts,
// src/lib/progress.ts; loaded with Node's built-in type stripping).

import assert from "node:assert/strict";
import { test } from "node:test";
import { playingEntryChanged } from "../src/lib/progress.ts";
import {
  FOLLOW_PAUSE_MS,
  FOLLOW_SCROLL_MAX_MS,
  LyricsCache,
  classifyScroll,
  currentLineIndex,
  followPausedUntil,
  followScrollTop,
  isFollowing,
  lyricsKey,
} from "../src/lib/lyrics.ts";

const timed = (...starts) => starts.map((startMs) => ({ startMs, text: "x" }));

test("finds the line playing at a position", () => {
  const lines = timed(1000, 2000, 2000, 5000);
  assert.equal(currentLineIndex(lines, 0), -1);
  assert.equal(currentLineIndex(lines, 999), -1);
  assert.equal(currentLineIndex(lines, 1000), 0);
  assert.equal(currentLineIndex(lines, 1999), 0);
  // Equal starts: the last of them.
  assert.equal(currentLineIndex(lines, 2000), 2);
  assert.equal(currentLineIndex(lines, 4999), 2);
  assert.equal(currentLineIndex(lines, 5000), 3);
  assert.equal(currentLineIndex(lines, 10 ** 9), 3);
});

test("has no current line without a position, lines or times", () => {
  assert.equal(currentLineIndex(timed(0, 10), null), -1);
  assert.equal(currentLineIndex(timed(0, 10), Number.NaN), -1);
  assert.equal(currentLineIndex([], 500), -1);
  assert.equal(currentLineIndex([{ startMs: null, text: "plain" }], 500), -1);
});

test("binary search agrees with a scan on a long song", () => {
  const lines = timed(...Array.from({ length: 997 }, (_, i) => i * 37));
  for (let pos = -5; pos < 997 * 37 + 50; pos += 13) {
    let expected = -1;
    lines.forEach((l, i) => {
      if (l.startMs <= pos) expected = i;
    });
    assert.equal(currentLineIndex(lines, pos), expected, `at ${pos}`);
  }
});

test("a manual scroll pauses auto-follow for a while", () => {
  assert.equal(isFollowing(0, 0), true);
  const paused = followPausedUntil(10_000);
  assert.equal(paused, 10_000 + FOLLOW_PAUSE_MS);
  assert.equal(isFollowing(paused, 10_000), false);
  assert.equal(isFollowing(paused, paused - 1), false);
  assert.equal(isFollowing(paused, paused), true);
  // Another scroll extends the pause.
  assert.equal(isFollowing(followPausedUntil(12_000), paused), false);
});

test("keeps the line a third of the way down", () => {
  assert.equal(followScrollTop(1000, 30, 600), 815);
  assert.equal(followScrollTop(10, 30, 600), 0);
});

test("cache keys name the source, and the server for Navidrome", () => {
  assert.equal(lyricsKey({ kind: "local", id: "3:17" }, 1), "local:3:17");
  assert.equal(lyricsKey({ kind: "local", id: "3:17" }, 2), "local:3:17");
  assert.equal(lyricsKey({ kind: "local", id: null }, 1), null);
  const song = { kind: "navidrome", songId: "s1", coverArt: null };
  assert.equal(lyricsKey(song, 1), "navidrome:1:s1");
  // The same song id on another server is another key.
  assert.notEqual(lyricsKey(song, 2), lyricsKey(song, 1));
});

test("scrolls during a follow are the view's; any other is manual", () => {
  // No follow in flight: a scrollbar drag, keys, anything.
  assert.deepEqual(classifyScroll(null, 300, 0), { manual: true, inFlight: null });
  const follow = { target: 500, until: 1000 + FOLLOW_SCROLL_MAX_MS };
  // On the way to the target: not manual, still in flight.
  assert.deepEqual(classifyScroll(follow, 320, 1100), { manual: false, inFlight: follow });
  // Arrived: not manual, and the follow is over, so the next scroll is.
  assert.deepEqual(classifyScroll(follow, 499, 1200), { manual: false, inFlight: null });
  // A follow that never arrives ends with its time.
  assert.deepEqual(classifyScroll(follow, 320, follow.until + 1), {
    manual: true,
    inFlight: null,
  });
});

test("a position from before the track change is dropped", () => {
  assert.equal(playingEntryChanged(null, { playingKey: "q1" }), false);
  assert.equal(playingEntryChanged({ playingKey: "q1" }, { playingKey: "q1" }), false);
  assert.equal(playingEntryChanged({ playingKey: "q1" }, { playingKey: "q2" }), true);
  assert.equal(playingEntryChanged({ playingKey: "q1" }, { playingKey: null }), true);
  assert.equal(playingEntryChanged({ playingKey: null }, { playingKey: "q1" }), true);
});

test("the cache keeps the most recently used results", () => {
  const cache = new LyricsCache(2);
  cache.set("a", null);
  cache.set("b", { timed: false, lines: [] });
  assert.equal(cache.get("a"), null);
  cache.set("c", null);
  assert.equal(cache.has("b"), false);
  assert.equal(cache.has("a"), true);
  assert.equal(cache.get("missing"), undefined);
  assert.equal(cache.size, 2);
  cache.clear();
  assert.equal(cache.size, 0);
  assert.equal(cache.get("a"), undefined);
});
