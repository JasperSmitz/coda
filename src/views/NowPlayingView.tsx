// The full Now Playing view (`f`): large cover over a blurred, darkened
// copy of it, title/artist/album, the timeline, transport, visualizer
// bars and "Up next". Readings are asked for only while this view is
// shown and the window is not hidden or minimised.

import { useEffect } from "react";
import { Cover } from "../components/Cover";
import { SeekBar } from "../components/NowPlayingBar";
import { Visualizer } from "../components/Visualizer";
import { NextIcon, PauseIcon, PlayIcon, PreviousIcon } from "../components/icons";
import { coverSrc, intent, intentInOrder, onWindowMinimized } from "../lib/ipc";
import type { QueueItem, Snapshot } from "../lib/types";

interface Props {
  snapshot: Snapshot;
  queue: QueueItem[];
  onClose: () => void;
}

// Visualizer requests go out one at a time, each carrying the latest
// wish, so a quick off/on (StrictMode, fast `f`) can never reach the
// engine reordered.
let visualizerChain: Promise<void> = Promise.resolve();
let visualizerWanted = false;
let visualizerSent: boolean | null = null;

function requestVisualizer(visible: boolean) {
  visualizerWanted = visible;
  visualizerChain = visualizerChain.then(async () => {
    const wanted = visualizerWanted;
    if (wanted === visualizerSent) return;
    visualizerSent = wanted;
    await intentInOrder({ type: "setVisualizerVisible", visible: wanted });
  });
}

/**
 * Asks for visualizer readings while `wanted`, the page is visible and the
 * window is not minimised.
 */
function useVisualizerReadings(wanted: boolean) {
  useEffect(() => {
    if (!wanted) return;
    let minimized = false;
    let disposed = false;
    const sync = () => {
      if (!disposed) requestVisualizer(document.visibilityState === "visible" && !minimized);
    };
    sync();
    document.addEventListener("visibilitychange", sync);
    const unlisten = onWindowMinimized((m) => {
      minimized = m;
      sync();
    });
    return () => {
      disposed = true;
      document.removeEventListener("visibilitychange", sync);
      unlisten.then((f) => f()).catch(() => {});
      requestVisualizer(false);
    };
  }, [wanted]);
}

export function NowPlayingView({ snapshot, queue, onClose }: Props) {
  const { playing, status } = snapshot;
  const unavailable = status === "unavailable" || snapshot.phase !== "ready";
  const isPlaying = status === "playing" || status === "loading";
  useVisualizerReadings(snapshot.visualizerAvailable && playing != null);

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !e.defaultPrevented) {
        e.preventDefault();
        onClose();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onClose]);

  if (!playing) {
    return (
      <section className="now-playing is-empty" aria-label="Now Playing">
        <p className="secondary">Nothing playing</p>
      </section>
    );
  }

  const upNext = snapshot.upNextKey ? queue.find((q) => q.key === snapshot.upNextKey) : undefined;
  const subtitle = [playing.artist, playing.album].filter(Boolean).join(" — ");

  return (
    <section className="now-playing" aria-label="Now Playing">
      {playing.coverUrl && (
        <img
          key={playing.coverUrl}
          className="now-playing-backdrop"
          src={coverSrc(playing.coverUrl, "small")}
          alt=""
          draggable={false}
          onError={(e) => (e.currentTarget.style.display = "none")}
        />
      )}
      <div className="now-playing-scrim" />
      <div className="now-playing-content">
        <Cover
          url={playing.coverUrl}
          name={playing.album ?? playing.title}
          size="large"
          className="now-playing-cover"
        />
        <div className="now-playing-info">
          <h1 className="now-playing-title" title={playing.title}>
            {playing.title}
          </h1>
          {subtitle && <div className="now-playing-subtitle">{subtitle}</div>}
          <div className="now-playing-timeline">
            <SeekBar disabled={unavailable} />
          </div>
          <div className="now-playing-controls">
            <button
              className="icon-button"
              onClick={() => intent({ type: "previous" })}
              disabled={unavailable}
              aria-label="Previous"
              title="Previous (p)"
            >
              <PreviousIcon size={22} />
            </button>
            <button
              className="icon-button play-button now-playing-play"
              onClick={() => intent({ type: "togglePause" })}
              disabled={unavailable}
              aria-label={isPlaying ? "Pause" : "Play"}
              title={`${isPlaying ? "Pause" : "Play"} (Space)`}
            >
              {isPlaying ? <PauseIcon size={24} /> : <PlayIcon size={24} />}
            </button>
            <button
              className="icon-button"
              onClick={() => intent({ type: "next" })}
              disabled={unavailable}
              aria-label="Next"
              title="Next (n)"
            >
              <NextIcon size={22} />
            </button>
          </div>
          {snapshot.visualizerAvailable && <Visualizer active={status === "playing"} />}
          <div className="now-playing-next secondary">
            {upNext ? (
              <>
                <span className="eyebrow">Up next</span>{" "}
                <span>
                  {[upNext.track.title, upNext.track.artist].filter(Boolean).join(" — ")}
                </span>
              </>
            ) : (
              <span className="eyebrow">End of queue</span>
            )}
          </div>
        </div>
      </div>
      <button className="text-button now-playing-close" onClick={onClose} title="Close (Esc)">
        Close
      </button>
    </section>
  );
}
