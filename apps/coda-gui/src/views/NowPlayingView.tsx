// The immersive Now Playing view (`f`): the only thing in the window
// while open (App hides the sidebar, browse area, queue column and bar).
// Large cover over a blurred, darkened copy of it, title/artist/album, one
// timeline with repeat beside it, one transport row with volume,
// visualizer bars and an "Up next" card (faded out, slot kept, while the
// queue is open). The queue opens as a side panel (`q` or the queue
// button) over the backdrop. Lyrics mode (`l` or the lyrics button)
// stacks the player into a column on the left and shows the lyrics in the
// middle (DESIGN.md). Visualizer readings are
// asked for only while this view is shown and the window is not hidden or
// minimised.
//
// Motion (DESIGN.md, all off with reduced motion; transforms and opacity
// only, nothing remounted): the view fades and scales in when it opens.
// The queue panel slides in and out (`usePresence`); when it or lyrics
// mode changes the layout, the cover and the info glide to their new
// place (`useFlip`), and the lyrics fade in after. A new track slides the
// cover and the title lines: the outgoing ones are drawn once more as an
// inert "ghost" at the place they were seen, sliding out to the left
// while the new ones come in from the right (the other way round for an
// earlier track in the queue); the backdrop crossfades.

import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
} from "react";
import { Cover } from "../components/Cover";
import { LyricsPanel } from "../components/LyricsPanel";
import { ActiveMixLine } from "../components/MixParts";
import { ArtistAlbumLine } from "../components/NameLink";
import { RepeatButton, SeekBar, VolumeControl } from "../components/NowPlayingBar";
import { Visualizer } from "../components/Visualizer";
import {
  CloseIcon,
  ExitFullscreenIcon,
  FullscreenIcon,
  LyricsIcon,
  NextIcon,
  PauseIcon,
  PlayIcon,
  PreviousIcon,
  QueueIcon,
} from "../components/icons";
import { useFadeOnChange } from "../hooks/useFadeOnChange";
import { useFlip } from "../hooks/useFlip";
import { useLyrics } from "../hooks/useLyrics";
import { usePresence } from "../hooks/usePresence";
import { coverSrc, intent, intentInOrder, onWindowMinimized } from "../lib/ipc";
import { animate, easing, motionMs, prefersReducedMotion } from "../lib/motion";
import { dragRegion } from "../lib/platform";
import type { ActiveMix, QueueItem, Snapshot, Track } from "../lib/types";

interface Props {
  snapshot: Snapshot;
  queue: QueueItem[];
  onClose: () => void;
  /** The queue side panel is shown (remembered apart from the main layout's). */
  queueOpen: boolean;
  onToggleQueue: () => void;
  /** Lyrics mode is on (sticky for the session, kept by App). */
  lyricsMode: boolean;
  onToggleLyrics: () => void;
  /** The `QueuePanel` to show in the side panel. */
  queuePanel: ReactNode;
  /** The window is full screen; the toolbar button enters or leaves it. */
  fullscreen: boolean;
  onToggleFullscreen: () => void;
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

/** How the queue panel looks out of view. */
const PANEL_HIDDEN: Keyframe = { opacity: 0, transform: "translateX(40px)" };
/** How far the cover and title lines slide on a track change. */
const SLIDE_PX = 64;
/** The longest wait for the new cover to decode before sliding anyway. */
const DECODE_WAIT_MS = 150;

/** A box in `.now-playing-content`'s coordinates (its padding box, scrolled). */
interface Box {
  left: number;
  top: number;
  width: number;
  height: number;
}

/** The outgoing track, drawn where it was seen while it slides out. */
interface Ghost {
  id: number;
  track: Track;
  mix: ActiveMix | null;
  /** 1: it leaves to the left (a later track); -1: to the right. */
  dir: 1 | -1;
  cover: Box | null;
  heading: Box | null;
  /** Its opacity when it was replaced (mid-slide, after a fast `n`). */
  opacity: number;
  /** The layout (`useFlip`'s key) its boxes were measured in. */
  layout: string;
}

let ghostIds = 0;

function boxIn(content: HTMLElement, origin: DOMRect, part: Element | null): Box | null {
  if (!part) return null;
  const rect = part.getBoundingClientRect();
  if (rect.width === 0 || rect.height === 0) return null;
  return {
    left: rect.left - origin.left - content.clientLeft + content.scrollLeft,
    top: rect.top - origin.top - content.clientTop + content.scrollTop,
    width: rect.width,
    height: rect.height,
  };
}

function boxStyle(box: Box): CSSProperties {
  return { left: box.left, top: box.top, width: box.width, height: box.height };
}

/** The cover itself (not the ghost's copy): a direct child of the content. */
function coverIn(content: HTMLElement | null): HTMLElement | null {
  return content?.querySelector<HTMLElement>(":scope > .now-playing-cover") ?? null;
}

/** Resolves once every `<img>` in `images` is decoded, or after `ms`. */
function decodedWithin(images: (Element | null | undefined)[], ms: number): Promise<void> {
  const decodes = images
    .filter((img): img is HTMLImageElement => img instanceof HTMLImageElement)
    .map((img) => img.decode().catch(() => {}));
  if (decodes.length === 0) return Promise.resolve();
  return Promise.race([
    Promise.all(decodes).then(() => {}),
    new Promise<void>((resolve) => window.setTimeout(resolve, ms)),
  ]);
}

export function NowPlayingView({
  snapshot,
  queue,
  onClose,
  queueOpen,
  onToggleQueue,
  lyricsMode,
  onToggleLyrics,
  queuePanel,
  fullscreen,
  onToggleFullscreen,
}: Props) {
  const { playing, status } = snapshot;
  const unavailable = status === "unavailable" || snapshot.phase !== "ready";
  const isPlaying = status === "playing" || status === "loading";
  useVisualizerReadings(snapshot.visualizerAvailable && playing != null);
  const panel = useRef<HTMLDivElement>(null);
  const view = useRef<HTMLElement>(null);
  const content = useRef<HTMLDivElement>(null);
  const info = useRef<HTMLDivElement>(null);
  const heading = useRef<HTMLDivElement>(null);
  const ghostEl = useRef<HTMLDivElement>(null);
  const lyricsShown = lyricsMode && playing != null;
  const lyrics = useLyrics(playing, lyricsMode, snapshot.browse.server);

  // The lyrics fade in after a switch made here, not when the view opens
  // already in lyrics mode. Decided while rendering, so the first frame
  // of the lyrics already has it.
  const [modeSeen, setModeSeen] = useState(lyricsMode);
  const [lyricsSwitched, setLyricsSwitched] = useState(false);
  if (lyricsMode !== modeSeen) {
    setModeSeen(lyricsMode);
    setLyricsSwitched(true);
  }

  // Entering lyrics mode, the cover glides into the column, which clips
  // its overflow (it scrolls): let it show while the glide runs.
  // (A data attribute, which React does not manage.) Before `useFlip`,
  // so its measurements see the same overflow.
  useLayoutEffect(() => {
    const element = content.current;
    if (!element || !lyricsShown || !lyricsSwitched) return;
    const ms = motionMs("--motion-slow");
    if (ms <= 0) return;
    element.dataset.gliding = "";
    const timer = window.setTimeout(() => delete element.dataset.gliding, ms);
    return () => {
      window.clearTimeout(timer);
      delete element.dataset.gliding;
    };
  }, [lyricsShown, lyricsSwitched]);

  // The queue panel or lyrics mode moved the cover and info: glide.
  const layout = `${queueOpen}|${lyricsShown}`;
  useFlip(layout, () => {
    const cover = coverIn(content.current);
    return [
      ...(cover ? [{ element: cover, scale: true }] : []),
      ...(info.current ? [{ element: info.current }] : []),
    ];
  });

  // ----- track change: slide -----
  const playingKey = playing ? snapshot.playingKey : null;
  /** The track shown in the last commit (the one that slides out). */
  const shown = useRef<{
    key: string | null;
    upNext: string | null;
    track: Track | null;
    mix: ActiveMix | null;
    layout: string;
  }>({
    key: playingKey,
    upNext: snapshot.upNextKey,
    track: playing,
    mix: snapshot.mix,
    layout,
  });
  const [slideKey, setSlideKey] = useState(playingKey);
  const [ghost, setGhost] = useState<Ghost | null>(null);
  /** The slide has begun (not still waiting for the new cover). */
  const slideStarted = useRef(true);
  if (playingKey !== slideKey) {
    // Measured while rendering: the DOM still shows the outgoing track
    // (and, after a fast `n`, where the previous slide had got to).
    setSlideKey(playingKey);
    setGhost(outgoingGhost());
  } else if (ghost && ghost.layout !== layout) {
    // The layout changed under a slide: the ghost's boxes are stale. Drop
    // it (the slide is cancelled, the new track is in place) and let the
    // glide run.
    setGhost(null);
  }

  function outgoingGhost(): Ghost | null {
    const before = shown.current;
    const element = content.current;
    if (!before.key || !before.track || !playingKey || !element || prefersReducedMotion()) return null;
    const origin = element.getBoundingClientRect();
    const cover = coverIn(element);
    const opacity = Number(getComputedStyle(cover ?? heading.current ?? element).opacity);
    // The track that was up next is a later one, also when repeat all
    // wraps to the start (core's `up_next` wraps; same keys as
    // `playingKey`). Otherwise compare queue places; a track not in the
    // queue counts as later. With two entries under repeat all, `p` to
    // the other entry is also its up next, so it slides as forward.
    const from = queue.findIndex((q) => q.key === before.key);
    const to = queue.findIndex((q) => q.key === playingKey);
    const forward = before.upNext != null && before.upNext === playingKey;
    const dir = forward ? 1 : from >= 0 && to >= 0 && to < from ? -1 : 1;
    // Still waiting for the last cover (fast `n`): the track on screen is
    // the last ghost's, the new one was never shown.
    if (ghost && !slideStarted.current && ghost.layout === layout) {
      return { ...ghost, id: ++ghostIds, dir };
    }
    if (before.layout !== layout) return null;
    return {
      id: ++ghostIds,
      track: before.track,
      mix: before.mix,
      dir,
      cover: boxIn(element, origin, cover),
      heading: boxIn(element, origin, heading.current),
      opacity: Number.isFinite(opacity) ? opacity : 1,
      layout,
    };
  }

  useLayoutEffect(() => {
    shown.current = {
      key: playingKey,
      upNext: snapshot.upNextKey,
      track: playing,
      mix: snapshot.mix,
      layout,
    };
  });

  useLayoutEffect(() => {
    const out = ghostEl.current;
    if (!ghost || !out) return;
    const done = () => setGhost((g) => (g?.id === ghost.id ? null : g));
    const d = SLIDE_PX * ghost.dir;
    const cover = coverIn(content.current);
    const animations: Animation[] = [];
    for (const part of [cover, heading.current]) {
      if (!part) continue;
      // The move adds onto a glide (`useFlip`) the cover may be running,
      // instead of replacing it; opacity on its own (adding would sum).
      const move = animate(
        part,
        [{ transform: `translateX(${d}px)` }, { transform: "translateX(0)" }],
        "--motion-slow",
        { fill: "both", composite: "add" },
      );
      const fade = animate(part, [{ opacity: 0 }, { opacity: 1 }], "--motion-slow", {
        fill: "both",
      });
      if (move) animations.push(move);
      if (fade) animations.push(fade);
    }
    const leave = animate(
      out,
      [
        { opacity: ghost.opacity, transform: "none" },
        { opacity: 0, transform: `translateX(${-d}px)` },
      ],
      "--motion-slow",
      { fill: "both", easing: easing("--ease-in-out") },
    );
    if (leave) animations.push(leave);
    if (animations.length === 0) {
      slideStarted.current = true;
      done();
      return;
    }
    // Held at their start (new track hidden, old one shown) until the new
    // cover is decoded, so it does not slide in blank.
    for (const a of animations) a.pause();
    slideStarted.current = false;
    let live = true;
    void decodedWithin([cover, out.querySelector("img")], DECODE_WAIT_MS).then(() => {
      if (!live) return;
      slideStarted.current = true;
      for (const a of animations) a.play();
      const end = () => {
        if (live) done();
      };
      Promise.all(animations.map((a) => a.finished)).then(end, end);
    });
    return () => {
      // Replaced by the next slide or finished: nothing stays mid-way.
      live = false;
      for (const a of animations) a.cancel();
    };
  }, [ghost]);

  // ----- queue panel -----
  const panelShown = usePresence(queueOpen, panel, {
    hidden: PANEL_HIDDEN,
    duration: "--motion-slow",
    // The leaving panel is inert: the focus moves to the view (a landing
    // spot for Tab; Escape still closes).
    onFocusLeaving: () => view.current?.focus(),
  });

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

  return (
    <section
      ref={view}
      tabIndex={-1}
      className={`now-playing ${playing ? "" : "is-empty"} ${queueOpen ? "has-queue" : ""} ${
        lyricsShown ? "is-lyrics" : ""
      }`}
      aria-label="Now Playing"
    >
      <Backdrop url={playing?.coverUrl ?? null} />
      <div className="now-playing-scrim" />
      <div className="now-playing-main">
        {playing ? (
          <div className="now-playing-content" ref={content}>
            <Cover
              url={playing.coverUrl}
              name={playing.album ?? playing.title}
              size="large"
              className="now-playing-cover"
            />
            <div className="now-playing-info" ref={info}>
              <div className="now-playing-heading" ref={heading}>
                <Heading track={playing} mix={snapshot.mix} onGo={onClose} />
              </div>
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
              <UpNext item={upNext} onOpen={onToggleQueue} hidden={queueOpen} />
            </div>
            {ghost && (
              <div key={ghost.id} ref={ghostEl} className="now-playing-ghost" aria-hidden="true" inert>
                {ghost.cover && (
                  <div className="now-playing-ghost-part" style={boxStyle(ghost.cover)}>
                    <Cover
                      url={ghost.track.coverUrl}
                      name={ghost.track.album ?? ghost.track.title}
                      size="large"
                      className="now-playing-ghost-cover"
                    />
                  </div>
                )}
                {ghost.heading && (
                  <div className="now-playing-ghost-part now-playing-heading" style={boxStyle(ghost.heading)}>
                    <Heading track={ghost.track} mix={ghost.mix} />
                  </div>
                )}
              </div>
            )}
          </div>
        ) : (
          <div className="now-playing-nothing">
            {snapshot.mix && <ActiveMixLine mix={snapshot.mix} className="now-playing-mix" />}
            <p className="secondary">Nothing playing</p>
          </div>
        )}
        {lyricsShown && (
          <div className={`now-playing-lyrics ${lyricsSwitched ? "is-entering" : ""}`}>
            <LyricsPanel state={lyrics} canSeek={!unavailable} />
          </div>
        )}
      </div>
      {panelShown && (
        <div className="now-playing-queue" ref={panel}>
          {queuePanel}
        </div>
      )}
      {/* macOS: the window buttons' row drags the window. */}
      <div className="now-playing-titlebar" {...dragRegion} />
      <div className="now-playing-toolbar">
        <button
          className={`icon-button ${lyricsMode ? "is-active" : ""}`}
          onClick={onToggleLyrics}
          aria-label="Lyrics"
          aria-pressed={lyricsMode}
          title="Lyrics (l)"
        >
          <LyricsIcon />
        </button>
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
          onClick={onToggleFullscreen}
          aria-label={fullscreen ? "Exit full screen" : "Full screen"}
          title={fullscreen ? "Exit full screen" : "Full screen"}
        >
          {fullscreen ? <ExitFullscreenIcon /> : <FullscreenIcon />}
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

/** The mix line, title and artist — album: the part that slides. */
function Heading({ track, mix, onGo }: { track: Track; mix: ActiveMix | null; onGo?: () => void }) {
  const subtitle = [track.artist, track.album].filter(Boolean).join(" — ");
  return (
    <>
      {mix && <ActiveMixLine mix={mix} className="now-playing-mix" />}
      <h1 className="now-playing-title" title={track.title}>
        {track.title}
      </h1>
      {subtitle && (
        <div className="now-playing-subtitle">
          <ArtistAlbumLine track={track} onGo={onGo} />
        </div>
      )}
    </>
  );
}

/**
 * The blurred cover behind the view. A new cover fades in over the old
 * one once loaded; the older ones go when its fade has run. At most
 * three layers, briefly: a settled one, one fading in, one loading.
 */
function Backdrop({ url }: { url: string | null }) {
  const [layers, setLayers] = useState(() => (url ? [{ id: 0, url, shown: true }] : []));
  const [seen, setSeen] = useState(url);
  const ids = useRef(1);
  if (url !== seen) {
    setSeen(url);
    const id = ids.current++;
    // The shown layers stay as the base (at most two: one settled, one
    // still fading in over it, so a third cover mid-fade does not drop to
    // a half-faded one); a cover still loading is replaced.
    setLayers((ls) => (url ? [...ls.filter((l) => l.shown).slice(-2), { id, url, shown: false }] : []));
  }
  const keepFrom = (id: number) => setLayers((ls) => ls.filter((l) => l.id >= id));
  return (
    <>
      {layers.map((layer) => (
        <img
          key={layer.id}
          className={`now-playing-backdrop ${layer.shown ? "is-shown" : ""}`}
          src={coverSrc(layer.url, "small")}
          alt=""
          draggable={false}
          onLoad={() => {
            setLayers((ls) => ls.map((l) => (l.id === layer.id ? { ...l, shown: true } : l)));
            // After the fade (a timer, not `transitionend`: a cached image
            // may load before the hidden state was ever drawn, and then
            // nothing transitions); at once with reduced motion.
            const ms = motionMs("--motion-slow");
            if (ms <= 0) keepFrom(layer.id);
            else window.setTimeout(() => keepFrom(layer.id), ms + 50);
          }}
          onError={() => setLayers((ls) => ls.filter((l) => l.id > layer.id))}
        />
      ))}
    </>
  );
}

/**
 * The next track as a small card; clicking it opens the queue panel.
 * `hidden` (the panel is open) keeps its slot, so the centred column does
 * not move, but fades it out of sight and takes it out of the tab order
 * and the accessibility tree. Another next track fades in.
 */
function UpNext({
  item,
  onOpen,
  hidden,
}: {
  item: QueueItem | undefined;
  onOpen: () => void;
  hidden: boolean;
}) {
  const card = useRef<HTMLElement | null>(null);
  const setCard = useCallback((element: HTMLElement | null) => {
    card.current = element;
  }, []);
  useFadeOnChange(item?.key ?? null, () => Array.from(card.current?.children ?? []), {
    duration: "--motion",
  });
  const hiddenClass = hidden ? "is-hidden" : "";
  if (!item) {
    return (
      <div
        ref={setCard}
        className={`now-playing-next ${hiddenClass}`}
        inert={hidden}
      >
        <span className="eyebrow">End of queue</span>
      </div>
    );
  }
  const { track } = item;
  return (
    <button
      ref={setCard}
      className={`now-playing-next is-card ${hiddenClass}`}
      inert={hidden}
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
