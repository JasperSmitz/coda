// Home's words and numbers (DESIGN.md "Home"), pure: no React, no DOM, so
// `node --test scripts/` can test it (`scripts/home.test.mjs`). The part
// of the day above the heading, the carousel's quiet line and where its
// covers sit, Resume's start, and a mood card's facts.

import type { HomeItem, HomeStop, Mood } from "./types";

const DAY_MS = 24 * 60 * 60 * 1000;
const MONTHS = [
  "January",
  "February",
  "March",
  "April",
  "May",
  "June",
  "July",
  "August",
  "September",
  "October",
  "November",
  "December",
];
const WEEKDAYS = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const NUMBERS = ["no", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine"];

/** "Thursday evening" (local time): morning 5–12, afternoon 12–18, evening 18–23, night. */
export function partOfDay(now: Date): string {
  const hour = now.getHours();
  const part =
    hour >= 5 && hour < 12
      ? "morning"
      : hour >= 12 && hour < 18
        ? "afternoon"
        : hour >= 18 && hour < 23
          ? "evening"
          : "night";
  // After midnight it is still the evening's night, not the next day's.
  const day = hour < 5 ? new Date(now.getTime() - DAY_MS) : now;
  return `${WEEKDAYS[day.getDay()]} ${part}`;
}

function count(n: number, one: string, many: string): string {
  if (n === 1) return one;
  return `${n < NUMBERS.length ? NUMBERS[n] : n} ${many}`;
}

/** Midnight before `date` (local time). */
function startOfDay(date: Date): number {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
}

/**
 * When `atSec` (unix seconds) was, in plain words, to follow "Played":
 * "just now", "five minutes ago", "an hour ago", "yesterday", "three
 * days ago", "two weeks ago", "in August", "in August 2025".
 */
export function relativeTime(atSec: number, now: Date): string {
  const at = new Date(atSec * 1000);
  const ms = now.getTime() - at.getTime();
  const minutes = Math.floor(ms / 60000);
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${count(minutes, "a minute", "minutes")} ago`;
  const days = Math.round((startOfDay(now) - startOfDay(at)) / DAY_MS);
  if (days === 0) return `${count(Math.floor(minutes / 60), "an hour", "hours")} ago`;
  if (days === 1) return "yesterday";
  if (days < 7) return `${count(days, "a day", "days")} ago`;
  if (days < 28) return `${count(Math.floor(days / 7), "a week", "weeks")} ago`;
  return at.getFullYear() === now.getFullYear()
    ? `in ${MONTHS[at.getMonth()]}`
    : `in ${MONTHS[at.getMonth()]} ${at.getFullYear()}`;
}

/** `m:ss` (or `h:mm:ss`). */
function clock(ms: number): string {
  const total = Math.floor(ms / 1000);
  const s = total % 60;
  const m = Math.floor(total / 60) % 60;
  const h = Math.floor(total / 3600);
  const ss = String(s).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${ss}` : `${m}:${ss}`;
}

/** Where it stopped: "stopped in track 4 of 9", or for one track "stopped at 2:10". */
export function stoppedText(stop: HomeStop): string | null {
  if (stop.total > 1) return `stopped in track ${stop.index + 1} of ${stop.total}`;
  return stop.positionMs >= 1000 ? `stopped at ${clock(stop.positionMs)}` : null;
}

/** The carousel's quiet line: "Playing now", or "Played 2 hours ago · stopped in track 4 of 9". */
export function quietLine(item: HomeItem, playing: boolean, now: Date): string {
  if (playing) return "Playing now";
  const parts = [`Played ${relativeTime(item.playedAt, now)}`];
  const stopped = item.stop ? stoppedText(item.stop) : null;
  if (stopped) parts.push(stopped);
  return parts.join(" · ");
}

/**
 * Where Resume starts in the context's tracks now (`ids`: each track's id
 * as `HomeStop.trackId` names it): the stopped track and its position if
 * it is still there; else the recorded place from its start; without a
 * stop, the first track.
 */
export function resumeStart(
  stop: HomeStop | null,
  ids: (string | null)[],
): { start: number; resume: { seekMs: number; trackId: string } | null } {
  if (!stop || ids.length === 0) return { start: 0, resume: null };
  const found = stop.trackId === null ? -1 : ids.indexOf(stop.trackId);
  if (found >= 0 && stop.trackId !== null) {
    return {
      start: found,
      resume: stop.positionMs > 0 ? { seekMs: stop.positionMs, trackId: stop.trackId } : null,
    };
  }
  return { start: Math.min(Math.max(0, stop.index), ids.length - 1), resume: null };
}

/** A cover's place in the carousel, `offset` places from the focused one. */
export interface Slot {
  /** Its centre's distance from the carousel's centre, in focused-cover widths. */
  x: number;
  scale: number;
  opacity: number;
}

/**
 * The focused cover at full size; its neighbours ×0.72, then ×0.6,
 * dimmed, a little behind each other; the fourth faded out (DESIGN.md).
 */
export function slot(offset: number): Slot {
  const d = Math.abs(offset);
  const sign = Math.sign(offset);
  const steps: Slot[] = [
    { x: 0, scale: 1, opacity: 1 },
    { x: 0.8, scale: 0.72, opacity: 0.6 },
    { x: 1.38, scale: 0.6, opacity: 0.32 },
    { x: 1.85, scale: 0.5, opacity: 0 },
  ];
  const step = steps[Math.min(d, steps.length - 1)];
  // Beyond the fourth: further out, still hidden.
  const x = d < steps.length ? step.x : step.x + (d - steps.length + 1) * 0.4;
  return { ...step, x: sign * x };
}

/** A length in words: `48 min`, `1 h 12 min`, `2 h`. */
export function lengthText(ms: number): string {
  const minutes = Math.max(1, Math.round(ms / 60000));
  const h = Math.floor(minutes / 60);
  const m = minutes % 60;
  if (h === 0) return `${m} min`;
  return m === 0 ? `${h} h` : `${h} h ${m} min`;
}

/**
 * A mood card's facts from what is known: the three albums' length when
 * every one is known, then "one you've never played", "new to you" (none
 * heard), or when the last of them was heard: "not played for 12 days",
 * "not played since August".
 */
export function moodFacts(mood: Mood, now: Date): string {
  const parts: string[] = [];
  const albums = mood.albums;
  if (albums.length > 0 && albums.every((a) => a.durationMs !== null && a.durationMs > 0)) {
    parts.push(lengthText(albums.reduce((sum, a) => sum + (a.durationMs ?? 0), 0)));
  }
  const never = albums.filter((a) => a.lastPlayed === null).length;
  if (albums.length > 0 && never === albums.length) {
    parts.push("new to you");
  } else if (never > 0) {
    parts.push(`${never < NUMBERS.length ? NUMBERS[never] : never} you've never played`);
  } else if (albums.length > 0) {
    const last = Math.max(...albums.map((a) => a.lastPlayed ?? 0));
    const at = new Date(last * 1000);
    const days = Math.round((startOfDay(now) - startOfDay(at)) / DAY_MS);
    if (days < 28) parts.push(`not played for ${days} days`);
    else
      parts.push(
        at.getFullYear() === now.getFullYear()
          ? `not played since ${MONTHS[at.getMonth()]}`
          : `not played since ${MONTHS[at.getMonth()]} ${at.getFullYear()}`,
      );
  }
  return parts.join(" · ");
}

/** A mood card's eyebrow: "46 albums in your library". */
export function moodEyebrow(mood: Mood, source: "local" | "navidrome"): string | null {
  const n = mood.albumCount;
  if (n === null || n <= 0) return null;
  const where = source === "local" ? "in your library" : "on your server";
  return `${n.toLocaleString("en")} ${n === 1 ? "album" : "albums"} ${where}`;
}

/** The plain label of a mood's quick mix: "Jazz mood". */
export function moodMixName(mood: Mood): string {
  return `${mood.genre} mood`;
}
