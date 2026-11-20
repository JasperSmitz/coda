// The floating player (DESIGN.md "Player"): a rounded glass strip above
// the bottom edge of the window, the page scrolling under it. Left the
// playing cover (a click opens Now Playing windowed, App morphing this
// cover into Now Playing's) and title / "artist · album"; in the middle
// previous / play-pause / next over the timeline (repeat after the total
// time); right a small visualizer, the volume, Queue and Full screen (Now
// Playing in full screen, `f`). Its background is the playing cover,
// stretched across the strip and blurred under a wash of its tint as
// much as that cover needs (`CoverGlow`), and it
// takes that cover's colours (a colour scope). Narrow windows drop the
// visualizer and the volume (app.css; the visualizer is then not
// rendered, so no readings flow for it), then the timeline and
// previous / next. Nothing playing: "Nothing playing", the controls as
// the snapshot allows (disabled while starting or without mpv).
//
// Also exports the timeline (`SeekBar`), `RepeatButton` and
// `VolumeControl`, which Now Playing uses too.

import { useRef, useState, type ReactNode } from "react";
import { intent } from "../lib/ipc";
import { formatTime } from "../lib/format";
import type { Progress, Repeat, Snapshot } from "../lib/types";
import { useMediaQuery, useVisualizerReadings } from "../hooks/useVisualizerReadings";
import { useProgress } from "../state/store";
import { useColourScope } from "../state/palettes";
import { Cover } from "./Cover";
import { CoverGlow } from "./PageGlow";
import { ArtistAlbumLine } from "./NameLink";
import { Slider } from "./Slider";
import { Visualizer } from "./Visualizer";
import {
  FullscreenIcon,
  NextIcon,
  PauseIcon,
  PlayIcon,
  PreviousIcon,
  QueueIcon,
  RepeatIcon,
  RepeatOneIcon,
  VolumeIcon,
  VolumeMutedIcon,
} from "./icons";

/** Where the visualizer and the volume fit (keep in step with app.css). */
const ROOMY = "(min-width: 860px)";

interface Props {
  snapshot: Snapshot;
  queueOpen: boolean;
  onToggleQueue: () => void;
  /** Opens Now Playing windowed (the cover). */
  onOpenNowPlaying: () => void;
  /** Opens Now Playing in full screen (what `f` does). */
  onFullscreen: () => void;
}

export function Player({ snapshot, queueOpen, onToggleQueue, onOpenNowPlaying, onFullscreen }: Props) {
  const { playing, status } = snapshot;
  const unavailable = status === "unavailable" || snapshot.phase !== "ready";
  const isPlaying = status === "playing" || status === "loading";
  // The playing cover's colours, and the cover itself, stretched across
  // the strip and blurred under a wash of its tint, as the background.
  const ref = useRef<HTMLElement>(null);
  const coverUrl = playing?.coverUrl ?? null;
  useColourScope(ref, coverUrl);
  // The small visualizer: only in a roomy window, while the engine
  // measures and a track is loaded; readings only while it is drawn.
  const roomy = useMediaQuery(ROOMY);
  const showVisualizer = roomy && snapshot.visualizerAvailable && playing != null;
  useVisualizerReadings(showVisualizer);

  return (
    <footer className="player" ref={ref}>
      <CoverGlow url={coverUrl} className="player-backdrop" surface="player" />
      <div className="player-track">
        {playing ? (
          <>
            <button
              className="player-cover-button"
              onClick={onOpenNowPlaying}
              aria-label="Open Now Playing"
              title="Now Playing"
            >
              <Cover
                url={playing.coverUrl}
                name={playing.album ?? playing.title}
                size="small"
                className="player-cover"
              />
            </button>
            <div className="player-text">
              <div className="player-title" title={playing.title}>
                {playing.title}
              </div>
              <div className="player-artist secondary">
                <ArtistAlbumLine track={playing} />
              </div>
            </div>
          </>
        ) : (
          <div className="player-text player-nothing secondary">Nothing playing</div>
        )}
      </div>

      <div className="player-center">
        <div className="player-controls">
          <button
            className="icon-button player-skip"
            onClick={() => intent({ type: "previous" })}
            disabled={unavailable}
            aria-label="Previous"
            title="Previous (p)"
          >
            <PreviousIcon />
          </button>
          <button
            className="icon-button play-button"
            onClick={() => intent({ type: "togglePause" })}
            disabled={unavailable}
            aria-label={isPlaying ? "Pause" : "Play"}
            title={`${isPlaying ? "Pause" : "Play"} (Space)`}
          >
            {isPlaying ? <PauseIcon size={18} /> : <PlayIcon size={18} />}
          </button>
          <button
            className="icon-button player-skip"
            onClick={() => intent({ type: "next" })}
            disabled={unavailable}
            aria-label="Next"
            title="Next (n)"
          >
            <NextIcon />
          </button>
        </div>
        <SeekBar
          disabled={unavailable || !playing}
          trailing={<RepeatButton repeat={snapshot.repeat} disabled={snapshot.phase !== "ready"} />}
        />
      </div>

      <div className="player-right">
        {showVisualizer && (
          <Visualizer active={status === "playing"} bars={5} className="player-visualizer" />
        )}
        <VolumeControl volume={snapshot.volume} disabled={unavailable} />
        <button
          className={`icon-button queue-toggle ${queueOpen ? "is-active" : ""}`}
          onClick={onToggleQueue}
          aria-label="Queue"
          aria-pressed={queueOpen}
          title="Queue (q)"
        >
          <QueueIcon />
        </button>
        <button
          className="icon-button"
          onClick={onFullscreen}
          aria-label="Full screen"
          title="Full screen (f)"
        >
          <FullscreenIcon />
        </button>
      </div>
    </footer>
  );
}

const REPEAT_LABEL: Record<Repeat, string> = {
  off: "Repeat off",
  all: "Repeat all",
  one: "Repeat one",
};
const REPEAT_NEXT: Record<Repeat, Repeat> = { off: "all", all: "one", one: "off" };

/** The repeat toggle beside a timeline: off (muted), all (accent), one
 * (accent with a "1"). Clicking (or `r`) cycles off → all → one. */
export function RepeatButton({ repeat, disabled }: { repeat: Repeat; disabled: boolean }) {
  const label = `${REPEAT_LABEL[repeat]}; click for ${REPEAT_LABEL[REPEAT_NEXT[repeat]].toLowerCase()}`;
  return (
    <button
      className={`icon-button repeat-button ${repeat !== "off" ? "is-active" : ""}`}
      onClick={() => intent({ type: "cycleRepeat" })}
      disabled={disabled}
      aria-label={label}
      title={`${label} (r)`}
    >
      {repeat === "one" ? <RepeatOneIcon /> : <RepeatIcon />}
    </button>
  );
}

/** Elapsed · seek slider · total (· `trailing`, e.g. the repeat button).
 * Shows the drag (then the target) until the backend reports a position
 * again. */
export function SeekBar({ disabled, trailing }: { disabled: boolean; trailing?: ReactNode }) {
  const progress = useProgress();
  const [pending, setPending] = useState<{ ms: number; from: Progress } | null>(null);
  // A seek target stays shown until a newer progress report arrives.
  const shownPending = pending && pending.from === progress ? pending.ms : null;
  const duration = progress.durationMs ?? 0;
  const position = shownPending ?? progress.positionMs ?? 0;
  const [dragMs, setDragMs] = useState<number | null>(null);
  const shown = dragMs ?? position;

  return (
    <div className={`seek ${trailing ? "has-trailing" : ""}`}>
      <span className="time">{formatTime(progress.positionMs == null ? null : shown)}</span>
      <Slider
        className="seek-slider"
        label="Seek"
        value={position}
        max={duration}
        step={5000}
        valueText={formatTime(shown)}
        disabled={disabled || duration <= 0}
        onInput={setDragMs}
        onCommit={(ms) => {
          setDragMs(null);
          setPending({ ms, from: progress });
          intent({ type: "seekTo", seconds: ms / 1000 });
        }}
      />
      <span className="time">{formatTime(progress.durationMs)}</span>
      {trailing}
    </div>
  );
}

/** Volume slider; requests are sent at most once per animation frame. */
export function VolumeControl({ volume, disabled }: { volume: number; disabled: boolean }) {
  const [drag, setDrag] = useState<number | null>(null);
  const frame = useRef<number | null>(null);
  const latest = useRef(volume);
  const send = (v: number) => {
    latest.current = Math.round(v);
    if (frame.current !== null) return;
    frame.current = requestAnimationFrame(() => {
      frame.current = null;
      intent({ type: "setVolume", volume: latest.current });
    });
  };
  const shown = drag ?? volume;
  return (
    <div className="volume" title={`Volume ${Math.round(shown)}% (+/−)`}>
      {shown === 0 ? <VolumeMutedIcon /> : <VolumeIcon />}
      <Slider
        className="volume-slider"
        label="Volume"
        value={shown}
        max={100}
        step={5}
        valueText={`${Math.round(shown)}%`}
        disabled={disabled}
        onInput={(v) => {
          setDrag(v);
          send(v);
        }}
        onCommit={(v) => {
          setDrag(null);
          send(v);
        }}
      />
    </div>
  );
}
