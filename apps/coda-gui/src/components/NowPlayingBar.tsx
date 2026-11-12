import { useRef, useState, type ReactNode } from "react";
import { intent } from "../lib/ipc";
import { formatTime } from "../lib/format";
import type { Progress, Repeat, Snapshot } from "../lib/types";
import { useProgress } from "../state/store";
import { Cover } from "./Cover";
import { ArtistAlbumLine } from "./NameLink";
import { Slider } from "./Slider";
import {
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

interface Props {
  snapshot: Snapshot;
  queueOpen: boolean;
  onToggleQueue: () => void;
  /** Opens or closes the Now Playing view (`f`). */
  nowPlayingOpen?: boolean;
  onToggleNowPlaying?: () => void;
}

export function NowPlayingBar({
  snapshot,
  queueOpen,
  onToggleQueue,
  nowPlayingOpen = false,
  onToggleNowPlaying,
}: Props) {
  const { playing, status } = snapshot;
  const unavailable = status === "unavailable" || snapshot.phase !== "ready";
  const isPlaying = status === "playing" || status === "loading";

  return (
    <footer className="bar">
      <div className="bar-track">
        {playing ? (
          <>
            <button
              className={`bar-cover-button ${nowPlayingOpen ? "is-active" : ""}`}
              onClick={onToggleNowPlaying}
              disabled={!onToggleNowPlaying}
              aria-label={nowPlayingOpen ? "Close Now Playing" : "Open Now Playing"}
              aria-pressed={nowPlayingOpen}
              title="Now Playing (f)"
            >
              <Cover
                url={playing.coverUrl}
                name={playing.album ?? playing.title}
                size="small"
                className="bar-cover"
              />
            </button>
            <div className="bar-text">
              <div className="bar-title" title={playing.title}>
                {playing.title}
              </div>
              <div className="bar-artist secondary">
                <ArtistAlbumLine track={playing} />
              </div>
            </div>
          </>
        ) : (
          <div className="bar-text secondary">Nothing playing</div>
        )}
      </div>

      <div className="bar-center">
        <div className="bar-controls">
          <button
            className="icon-button"
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
            {isPlaying ? <PauseIcon size={20} /> : <PlayIcon size={20} />}
          </button>
          <button
            className="icon-button"
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

      <div className="bar-right">
        <VolumeControl volume={snapshot.volume} disabled={unavailable} />
        <button
          className={`icon-button ${queueOpen ? "is-active" : ""}`}
          onClick={onToggleQueue}
          aria-label="Queue"
          aria-pressed={queueOpen}
          title="Queue (q)"
        >
          <QueueIcon />
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
