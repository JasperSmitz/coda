// The immersive Now Playing view (`f`): the only thing in the window
// while open (App hides the sidebar, browse area, queue column and bar).
// Large cover over a blurred, darkened copy of it, title/artist/album, one
// timeline with repeat beside it, one transport row with volume,
// visualizer bars and an "Up next" card. The queue opens as a side panel
// (`q` or the queue button) over the backdrop. Visualizer readings are
// asked for only while this view is shown and the window is not hidden or
// minimised.

import { useEffect, useRef, type ReactNode } from "react";
import { Cover } from "../components/Cover";
import { RepeatButton, SeekBar, VolumeControl } from "../components/NowPlayingBar";
import { Visualizer } from "../components/Visualizer";
import {
  CloseIcon,
  NextIcon,
  PauseIcon,
  PlayIcon,
  PreviousIcon,
  QueueIcon,
} from "../components/icons";
import { coverSrc, intent, intentInOrder, onWindowMinimized } from "../lib/ipc";
import type { QueueItem, Snapshot } from "../lib/types";

interface Props {
  snapshot: Snapshot;
  queue: QueueItem[];
  onClose: () => void;
  /** The queue side panel is shown (remembered apart from the main layout's). */
  queueOpen: boolean;
  onToggleQueue: () => void;
  /** The `QueuePanel` to show in the side panel. */
  queuePanel: ReactNode;
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

export function NowPlayingView({
  snapshot,
  queue,
  onClose,
  queueOpen,
  onToggleQueue,
  queuePanel,
}: Props) {
  const { playing, status } = snapshot;
  const unavailable = status === "unavailable" || snapshot.phase !== "ready";
  const isPlaying = status === "playing" || status === "loading";
  useVisualizerReadings(snapshot.visualizerAvailable && playing != null);
  const panel = useRef<HTMLDivElement>(null);
  const view = useRef<HTMLElement>(null);

  // The browse view (and the element focused in it) went away: give the
  // view the focus, so Tab continues from here (and Escape still closes).
  useEffect(() => {
    const focused = document.activeElement;
    if (!focused || focused === document.body || !focused.isConnected) view.current?.focus();
  }, []);

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

  // Opening the panel (not showing it because it was left open) moves the
  // focus into the queue list, so the keyboard can work in it at once.
  const wasOpen = useRef(queueOpen);
  useEffect(() => {
    if (queueOpen && !wasOpen.current) {
      panel.current?.querySelector<HTMLElement>(".queue-list")?.focus();
    }
    wasOpen.current = queueOpen;
  }, [queueOpen]);

  const upNext = snapshot.upNextKey ? queue.find((q) => q.key === snapshot.upNextKey) : undefined;
  const subtitle = playing ? [playing.artist, playing.album].filter(Boolean).join(" — ") : "";

  return (
    <section
      ref={view}
      tabIndex={-1}
      className={`now-playing ${playing ? "" : "is-empty"} ${queueOpen ? "has-queue" : ""}`}
      aria-label="Now Playing"
    >
      {playing?.coverUrl && (
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
      <div className="now-playing-main">
        {playing ? (
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
                <SeekBar
                  disabled={unavailable}
                  trailing={
                    <RepeatButton repeat={snapshot.repeat} disabled={snapshot.phase !== "ready"} />
                  }
                />
              </div>
              <div className="now-playing-transport">
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
                <VolumeControl volume={snapshot.volume} disabled={unavailable} />
              </div>
              {snapshot.visualizerAvailable && <Visualizer active={status === "playing"} />}
              {!queueOpen && <UpNext item={upNext} onOpen={onToggleQueue} />}
            </div>
          </div>
        ) : (
          <p className="now-playing-nothing secondary">Nothing playing</p>
        )}
      </div>
      {queueOpen && (
        <div className="now-playing-queue" ref={panel}>
          {queuePanel}
        </div>
      )}
      <div className="now-playing-toolbar">
        <button
          className={`icon-button ${queueOpen ? "is-active" : ""}`}
          onClick={onToggleQueue}
          aria-label="Queue"
          aria-pressed={queueOpen}
          title="Queue (q)"
        >
          <QueueIcon />
        </button>
        <button
          className="icon-button"
          onClick={onClose}
          aria-label="Close Now Playing"
          title="Close (Esc or f)"
        >
          <CloseIcon />
        </button>
      </div>
    </section>
  );
}

/** The next track as a small card; clicking it opens the queue panel. */
function UpNext({ item, onOpen }: { item: QueueItem | undefined; onOpen: () => void }) {
  if (!item) {
    return (
      <div className="now-playing-next">
        <span className="eyebrow">End of queue</span>
      </div>
    );
  }
  const { track } = item;
  return (
    <button
      className="now-playing-next is-card"
      onClick={onOpen}
      title="Show the queue (q)"
      aria-label={`Up next: ${[track.title, track.artist].filter(Boolean).join(" — ")}. Show the queue`}
    >
      <Cover
        url={track.coverUrl}
        name={track.album ?? track.title}
        size="small"
        className="now-playing-next-cover"
      />
      <span className="now-playing-next-text">
        <span className="eyebrow">Up next</span>
        <span className="now-playing-next-title">{track.title}</span>
        <span className="now-playing-next-artist">{track.artist ?? "Unknown artist"}</span>
      </span>
    </button>
  );
}
