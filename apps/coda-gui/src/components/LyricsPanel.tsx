// Lyrics mode's middle area. Timed lyrics follow the song: the current
// line (from the `progress` store, so only this component re-renders on
// progress) is in the text colour and kept about a third of the way down;
// any scroll the view did not start itself (wheel, touch, keys, a
// scrollbar drag) pauses that for a few seconds, then it eases back; a
// scrollbar held still is never pulled away. Clicking a timed line seeks there. Untimed lyrics
// are plain text to scroll. Lines are not focusable (the timeline seeks
// from the keyboard); the list itself is, to scroll it with the keys.

import { memo, useCallback, useEffect, useLayoutEffect, useRef } from "react";
import type { LyricsState } from "../hooks/useLyrics";
import { intent } from "../lib/ipc";
import {
  FOLLOW_PAUSE_MS,
  FOLLOW_SCROLL_MAX_MS,
  type FollowScroll,
  classifyScroll,
  currentLineIndex,
  followPausedUntil,
  followScrollTop,
  isFollowing,
} from "../lib/lyrics";
import type { Lyrics } from "../lib/types";
import { useProgress } from "../state/store";

function prefersReducedMotion(): boolean {
  return window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
}

interface Props {
  state: LyricsState;
  /** Clicking a line seeks (playback available). */
  canSeek: boolean;
}

export function LyricsPanel({ state, canSeek }: Props) {
  let body;
  switch (state.status) {
    case "ready":
      body = <LyricsText lyrics={state.lyrics} canSeek={canSeek} />;
      break;
    case "none":
      body = <p className="lyrics-message">No lyrics for this track</p>;
      break;
    case "error":
      body = <p className="lyrics-message">Couldn't load lyrics</p>;
      break;
    case "loading":
      body = state.slow ? <p className="lyrics-message">Loading lyrics…</p> : null;
      break;
    default:
      body = null;
  }
  return (
    <div className="lyrics">
      {body}
    </div>
  );
}

function LyricsText({ lyrics, canSeek }: { lyrics: Lyrics; canSeek: boolean }) {
  const { positionMs } = useProgress();
  // Without a position (loading, stopped) the highlight stays where it was.
  const last = useRef({ lyrics, index: -1 });
  if (last.current.lyrics !== lyrics) last.current = { lyrics, index: -1 };
  if (lyrics.timed && positionMs != null) {
    last.current.index = currentLineIndex(lyrics.lines, positionMs);
  }
  const current = lyrics.timed ? last.current.index : -1;

  const scroller = useRef<HTMLDivElement>(null);
  const pausedUntil = useRef(0);
  const resume = useRef(0);
  /** The view's own scroll in flight; any other scroll is the user's. */
  const inFlight = useRef<FollowScroll | null>(null);
  /** A pointer is held on the scroller itself (its scrollbar). */
  const held = useRef(false);
  const currentRef = useRef(current);
  currentRef.current = current;

  const follow = useCallback((smooth: boolean) => {
    const element = scroller.current;
    if (!element) return;
    const index = currentRef.current;
    const line = index >= 0 ? element.querySelector<HTMLElement>(`[data-line="${index}"]`) : null;
    const wanted = line
      ? followScrollTop(line.offsetTop, line.offsetHeight, element.clientHeight)
      : 0;
    const target = Math.min(wanted, Math.max(0, element.scrollHeight - element.clientHeight));
    if (Math.abs(element.scrollTop - target) < 2) return;
    inFlight.current = { target, until: performance.now() + FOLLOW_SCROLL_MAX_MS };
    element.scrollTo({ top: target, behavior: smooth && !prefersReducedMotion() ? "smooth" : "auto" });
  }, []);

  // Eases back after the pause, unless the scrollbar is still held.
  const scheduleResume = useCallback(() => {
    window.clearTimeout(resume.current);
    resume.current = window.setTimeout(function again() {
      if (held.current) {
        pausedUntil.current = followPausedUntil(performance.now());
        resume.current = window.setTimeout(again, FOLLOW_PAUSE_MS);
      } else {
        follow(true);
      }
    }, FOLLOW_PAUSE_MS);
  }, [follow]);

  // New lyrics: start from the top (or the current line), following.
  useLayoutEffect(() => {
    pausedUntil.current = 0;
    window.clearTimeout(resume.current);
    if (lyrics.timed) follow(false);
    else scroller.current?.scrollTo({ top: 0 });
  }, [lyrics, follow]);

  useEffect(() => {
    if (lyrics.timed && isFollowing(pausedUntil.current, performance.now())) follow(true);
  }, [current, lyrics, follow]);

  // The list was resized (window, queue panel): keep the line in place.
  useEffect(() => {
    const element = scroller.current;
    if (!element || !lyrics.timed || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => {
      if (isFollowing(pausedUntil.current, performance.now())) follow(false);
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, [lyrics, follow]);

  // A press on the scrollbar may end outside the view.
  useEffect(() => {
    const release = () => {
      held.current = false;
    };
    window.addEventListener("pointerup", release);
    window.addEventListener("pointercancel", release);
    return () => {
      window.removeEventListener("pointerup", release);
      window.removeEventListener("pointercancel", release);
      window.clearTimeout(resume.current);
    };
  }, []);

  const manual = useCallback(() => {
    if (!lyrics.timed) return;
    inFlight.current = null;
    pausedUntil.current = followPausedUntil(performance.now());
    scheduleResume();
  }, [lyrics, scheduleResume]);

  const onScroll = useCallback(() => {
    const element = scroller.current;
    if (!element) return;
    const verdict = classifyScroll(inFlight.current, element.scrollTop, performance.now());
    inFlight.current = verdict.inFlight;
    if (verdict.manual) manual();
  }, [manual]);

  const seek = useCallback(
    (startMs: number) => {
      if (!canSeek) return;
      pausedUntil.current = 0;
      window.clearTimeout(resume.current);
      intent({ type: "seekTo", seconds: startMs / 1000 });
    },
    [canSeek],
  );

  return (
    <div
      ref={scroller}
      className={`lyrics-scroll ${lyrics.timed ? "is-timed" : "is-plain"}`}
      role="region"
      aria-label="Lyrics"
      tabIndex={0}
      onScroll={onScroll}
      // Input that scrolls, counted at once (its scroll events would also
      // be, unless they land inside a follow scroll).
      onWheel={manual}
      onTouchMove={manual}
      onPointerDown={(e) => {
        // The scrollbar (a drag), not a line.
        if (e.target === e.currentTarget) held.current = true;
      }}
    >
      <LyricsLines lyrics={lyrics} current={current} onSeek={canSeek ? seek : undefined} />
    </div>
  );
}

/** The lines; re-rendered only when the current line changes. */
const LyricsLines = memo(function LyricsLines({
  lyrics,
  current,
  onSeek,
}: {
  lyrics: Lyrics;
  current: number;
  onSeek?: (startMs: number) => void;
}) {
  return (
    <div className="lyrics-lines">
      {lyrics.lines.map((line, i) => {
        const start = line.startMs;
        const seekable = lyrics.timed && start != null && onSeek != null;
        return (
          <p
            key={i}
            data-line={i}
            className={`lyrics-line ${i === current ? "is-current" : ""} ${
              line.text ? "" : "is-pause"
            } ${seekable ? "is-seekable" : ""}`}
            aria-current={i === current ? "true" : undefined}
            onClick={seekable ? () => onSeek(start) : undefined}
          >
            {line.text || " "}
          </p>
        );
      })}
    </div>
  );
});
