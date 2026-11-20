// `node --test scripts/`: Home's words and numbers (src/lib/home.ts).

import assert from "node:assert/strict";
import { test } from "node:test";
import {
  moodEyebrow,
  moodFacts,
  moodSwatches,
  partOfDay,
  quietLine,
  relativeTime,
  resumeStart,
  slot,
} from "../src/lib/home.ts";

const at = (y, mo, d, h = 12, mi = 0) => new Date(y, mo - 1, d, h, mi);
const secs = (date) => Math.floor(date.getTime() / 1000);

test("the part of the day, local time; after midnight still the night before", () => {
  // 2026-11-19 is a Thursday.
  assert.equal(partOfDay(at(2026, 11, 19, 8)), "Thursday morning");
  assert.equal(partOfDay(at(2026, 11, 19, 12)), "Thursday afternoon");
  assert.equal(partOfDay(at(2026, 11, 19, 20)), "Thursday evening");
  assert.equal(partOfDay(at(2026, 11, 19, 23, 30)), "Thursday night");
  assert.equal(partOfDay(at(2026, 11, 20, 1)), "Thursday night");
  assert.equal(partOfDay(at(2026, 11, 20, 5)), "Friday morning");
});

test("relative times in plain words", () => {
  const now = at(2026, 11, 19, 20);
  assert.equal(relativeTime(secs(new Date(2026, 10, 19, 19, 59, 40)), now), "just now");
  assert.equal(relativeTime(secs(at(2026, 11, 19, 19, 59)), now), "a minute ago");
  assert.equal(relativeTime(secs(at(2026, 11, 19, 19, 55)), now), "five minutes ago");
  assert.equal(relativeTime(secs(at(2026, 11, 19, 18)), now), "two hours ago");
  assert.equal(relativeTime(secs(at(2026, 11, 18, 23)), now), "yesterday");
  assert.equal(relativeTime(secs(at(2026, 11, 16, 9)), now), "three days ago");
  assert.equal(relativeTime(secs(at(2026, 11, 5)), now), "two weeks ago");
  assert.equal(relativeTime(secs(at(2026, 8, 3)), now), "in August");
  assert.equal(relativeTime(secs(at(2025, 8, 3)), now), "in August 2025");
});

test("the quiet line says where it stopped", () => {
  const now = at(2026, 11, 19, 20);
  const item = {
    playedAt: secs(at(2026, 11, 19, 18)),
    stop: { index: 3, total: 9, trackId: "s4", positionMs: 61000 },
  };
  assert.equal(quietLine(item, false, now), "Played two hours ago · stopped in track 4 of 9");
  assert.equal(quietLine(item, true, now), "Playing now");
  assert.equal(quietLine({ ...item, stop: null }, false, now), "Played two hours ago");
  const single = { ...item, stop: { index: 0, total: 1, trackId: "s", positionMs: 130500 } };
  assert.equal(quietLine(single, false, now), "Played two hours ago · stopped at 2:10");
});

test("Resume starts at the stopped track if it is still there", () => {
  const stop = { index: 1, total: 3, trackId: "b", positionMs: 5000 };
  // The seek names its track: the engine seeks only if that one starts.
  assert.deepEqual(resumeStart(stop, ["a", "x", "b"]), {
    start: 2,
    resume: { seekMs: 5000, trackId: "b" },
  });
  // Gone: the recorded place, from its start (within the tracks there are).
  assert.deepEqual(resumeStart(stop, ["a", "x"]), { start: 1, resume: null });
  assert.deepEqual(resumeStart({ ...stop, index: 7 }, ["a"]), { start: 0, resume: null });
  assert.deepEqual(resumeStart({ ...stop, trackId: null }, ["a", "b"]), { start: 1, resume: null });
  assert.deepEqual(resumeStart(null, ["a"]), { start: 0, resume: null });
  assert.deepEqual(resumeStart({ ...stop, positionMs: 0 }, ["b"]), { start: 0, resume: null });
});

test("carousel slots shrink, dim and fade out by the fourth, mirrored", () => {
  assert.deepEqual(slot(0), { x: 0, scale: 1, opacity: 1 });
  assert.equal(slot(1).scale, 0.72);
  assert.equal(slot(-2).scale, 0.6);
  assert.equal(slot(-1).x, -slot(1).x);
  assert.ok(slot(1).opacity < 1 && slot(2).opacity < slot(1).opacity);
  assert.equal(slot(3).opacity, 0);
  assert.equal(slot(5).opacity, 0);
  assert.ok(slot(5).x > slot(3).x);
});

test("a mood's facts and eyebrow", () => {
  const now = at(2026, 11, 19, 20);
  const album = (durationMs, lastPlayed) => ({ durationMs, lastPlayed });
  const mood = (albums, albumCount = 46) => ({ genre: "Jazz", albumCount, albums });
  const aug = secs(at(2026, 8, 12));
  assert.equal(
    moodFacts(mood([album(3600000, aug), album(2400000, aug), album(2040000, secs(at(2026, 7, 1)))]), now),
    "2 h 14 min · not played since August",
  );
  assert.equal(
    moodFacts(mood([album(null, null), album(1, aug), album(1, aug)]), now),
    "one you've never played",
  );
  assert.equal(moodFacts(mood([album(60000, null), album(60000, null)]), now), "2 min · new to you");
  assert.equal(moodFacts(mood([album(null, secs(at(2026, 11, 7)))]), now), "not played for 12 days");
  assert.equal(moodFacts(mood([album(null, secs(at(2025, 3, 7)))]), now), "not played since March 2025");
  assert.equal(moodEyebrow(mood([]), "local"), "46 albums in your library");
  assert.equal(moodEyebrow(mood([], 1), "navidrome"), "1 album on your server");
  assert.equal(moodEyebrow(mood([], null), "local"), null);
});

test("a mood card's two colours come from its albums' swatches", () => {
  assert.equal(moodSwatches([[], [], []]), null);
  assert.equal(moodSwatches([]), null);
  // The first album's first, then the next album's first.
  assert.deepEqual(moodSwatches([["#a"], ["#b"], ["#c"]]), ["#a", "#b"]);
  // A greyscale first album: the next album leads.
  assert.deepEqual(moodSwatches([[], ["#b", "#b2"], ["#c"]]), ["#b", "#c"]);
  // The same first colour everywhere: the first album's second.
  assert.deepEqual(moodSwatches([["#a", "#a2"], ["#a"], []]), ["#a", "#a2"]);
  // Only one colour: twice.
  assert.deepEqual(moodSwatches([["#a"], [], []]), ["#a", "#a"]);
});
