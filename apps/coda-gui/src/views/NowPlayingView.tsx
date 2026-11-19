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
// Motion (DESIGN.md, all off with reduced motion; nothing remounted):
// the view fades and scales in when it opens; the queue panel slides in
// and out (`usePresence`). Switching the layout (`q`, `l`) and a new track
// are View Transitions (`lib/motion.ts` `viewTransition`; App wraps the
// toggles, this view the track change): the browser takes the old and the
// new screen. `q` (wide windows): the cover morphs to its new place and
// size, the text crossfades. `l`: the text fades out where it is, the
// cover glides, then the parts rise in staggered and the lyrics (the live
// element, `useStaggeredEntrance`) rise in last. A track change: the
// cover and the title lines slide (the old ones out to the left, the new
// ones in from the right; the other way round for an earlier track in the
// queue). The names (`view-transition-name`) and the animations are in
// experience.css, keyed by `data-vt` on <html>. Without View Transitions:
// a short crossfade for `q` and track changes, the same staggered
// entrance (no glide) for `l`.

import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
  type RefObject,
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
import { useLyrics } from "../hooks/useLyrics";
import { usePresence } from "../hooks/usePresence";
import { coverSrc, intent, intentInOrder, onWindowMinimized } from "../lib/ipc";
import {
  type TransitionHandle,
  cssMs,
  decodedWithin,
  easing,
  motionMs,
  viewTransition,
  viewTransitionsOn,
} from "../lib/motion";
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
/** The longest wait for the new cover to decode (the screen is frozen meanwhile). */
const DECODE_WAIT_MS = 150;

/**
 * Whether opening or closing the queue panel moves the cover and info:
 * only in a wide window (the panel takes room beside them) and not in
 * lyrics mode (the panel lies over the lyrics). Keep in step with the
 * `min-width: 1180px` rules in experience.css (`.has-queue`). Elsewhere
 * the panel slides over the content, which stays put, so App toggles it
 * without a view transition (named parts would be drawn above the panel).
 */
export function queueToggleMovesContent(lyricsMode: boolean): boolean {
  return !lyricsMode && (window.matchMedia?.("(min-width: 1180px)").matches ?? false);
}

/**
 * Without View Transitions: after a lyrics switch the new layout's parts
 * rise in, staggered like the view transition's (experience.css
 * `--np-lx-*`): heading; timeline and transport; visualizer and Up next;
 * the lyrics. The cover does not move. With or without View Transitions,
 * entering lyrics mode lets the lyrics element itself rise in from below
 * once the cover has (nearly) moved away: the live element, so it stays
 * under the queue panel, and lyrics loading meanwhile appear inside it.
 * A transform does not change `offsetTop`, so the follow scroll is not
 * affected. Not when the view opens already in lyrics mode.
 */
function useStaggeredEntrance(
  lyricsShown: boolean,
  content: RefObject<HTMLDivElement | null>,
  heading: RefObject<HTMLDivElement | null>,
  lyricsArea: RefObject<HTMLDivElement | null>,
) {
  const seen = useRef(lyricsShown);
  const running = useRef<Animation[]>([]);
  useLayoutEffect(() => {
    if (seen.current === lyricsShown) return;
    seen.current = lyricsShown;
    for (const a of running.current) a.cancel();
    running.current = [];
    const root = content.current;
    const duration = cssMs("--np-lx-in", 400);
    if (!root || duration <= 0) return;
    const step = cssMs("--np-lx-step", 70);
    const style = getComputedStyle(document.documentElement);
    const value = (name: string, fallback: string) => style.getPropertyValue(name).trim() || fallback;
    const transitions = viewTransitionsOn();

    const lyricsEl = lyricsArea.current;
    const lyricsIn = cssMs("--np-lx-lyrics-in", 500);
    if (lyricsShown && lyricsEl && typeof lyricsEl.animate === "function" && lyricsIn > 0) {
      const at = Number(value("--np-lx-lyrics-at", "0.85"));
      // After the cover's glide only when a lyrics switch is really
      // running (not when playback starting shows them in lyrics mode).
      const switching = transitions && document.documentElement.dataset.vt === "lyrics";
      const delay = switching
        ? cssMs("--np-lx-out", 160) + cssMs("--np-lx-glide", 380) * (Number.isFinite(at) ? at : 0.85)
        : 3 * step;
      running.current.push(
        lyricsEl.animate(
          [
            { opacity: 0, transform: `translateY(${value("--np-lx-lyrics-rise", "40px")})` },
            { opacity: 1, transform: "none" },
          ],
          { duration: lyricsIn, delay, easing: easing("--ease-out"), fill: "backwards" },
        ),
      );
    }

    // The rest: the view transition staggers its snapshots itself.
    if (transitions) return;
    const rise = value("--np-lx-rise", "10px");
    const stages: (Element | null)[][] = [
      [heading.current],
      [root.querySelector(".now-playing-timeline"), root.querySelector(".now-playing-transport")],
      [root.querySelector(".now-playing-info > .visualizer"), root.querySelector(".now-playing-next")],
    ];
    stages.forEach((elements, i) => {
      for (const element of elements) {
        if (!element || typeof element.animate !== "function") continue;
        running.current.push(
          element.animate(
            [
              { opacity: 0, transform: `translateY(${rise})` },
              { opacity: 1, transform: "none" },
            ],
            { duration, delay: i * step, easing: easing("--ease-out"), fill: "backwards" },
          ),
        );
      }
    });
  }, [lyricsShown, content, heading, lyricsArea]);
  useLayoutEffect(
    () => () => {
      for (const a of running.current) a.cancel();
    },
    [],
  );
}

/** The track whose cover and title lines are shown. */
interface Shown {
  key: string | null;
  track: Track | null;
  mix: ActiveMix | null;
}

/** The cover itself: a direct child of the content. */
function coverIn(content: HTMLElement | null): HTMLElement | null {
  return content?.querySelector<HTMLElement>(":scope > .now-playing-cover") ?? null;
}

/** Slides started; an older one's update does nothing (the newest wins). */
let slides = 0;

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
  const heading = useRef<HTMLDivElement>(null);
  const lyricsArea = useRef<HTMLDivElement>(null);
  const lyricsShown = lyricsMode && playing != null;
  const lyrics = useLyrics(playing, lyricsMode, snapshot.browse.server);

  // Without View Transitions (App wraps `q` and `l` in one): a short
  // crossfade of the player after the queue panel moved it, and the
  // lyrics switch's staggered entrance (no cover glide).
  useFadeOnChange(
    queueOpen,
    () => (viewTransitionsOn() || !queueToggleMovesContent(lyricsShown) ? [] : [content.current]),
    { duration: "--motion" },
  );
  useStaggeredEntrance(lyricsShown, content, heading, lyricsArea);

  // ----- track change: slide -----
  // The cover, the title lines and the backdrop show `shown`, which
  // follows `playingKey` through a view transition (CSS: the old ones
  // slide out, the new ones in). Everything else follows the snapshot.
  const playingKey = playing ? snapshot.playingKey : null;
  const [shown, setShown] = useState<Shown>({ key: playingKey, track: playing, mix: snapshot.mix });
  const current = shown.key === playingKey;
  const shownTrack = (current ? playing : shown.track) ?? playing;
  const shownMix = current ? snapshot.mix : shown.mix;
  /** The up-next key of the last commit (before this snapshot). */
  const lastUpNext = useRef(snapshot.upNextKey);
  const latest = useRef<Shown>({ key: playingKey, track: playing, mix: snapshot.mix });
  /** The slide still running or waiting, if any. */
  const slide = useRef<TransitionHandle | null>(null);
  latest.current = { key: playingKey, track: playing, mix: snapshot.mix };

  useLayoutEffect(() => {
    const from = shown.key;
    // Any change makes a slide still waiting to apply stale (e.g. `p`
    // back to the shown track before the slide to the next one ran).
    const id = ++slides;
    // Ended before anything newer starts (skipping later would end that).
    slide.current?.skip();
    slide.current = null;
    if (from === playingKey) return;
    const next = latest.current;
    // Only the newest slide's update counts: a skipped one running late
    // must not bring back a track that is no longer playing.
    const apply = () => {
      if (id === slides) setShown(next);
    };
    // Nothing to slide from or to; or the lyrics column is scrolled, so
    // the cover or title is partly out of its view, and a transition
    // would draw it whole, unclipped.
    if (!from || !playingKey || (lyricsShown && (content.current?.scrollTop ?? 0) > 0)) {
      apply();
      return;
    }
    // The track that was up next is a later one, also when repeat all
    // wraps to the start (core's `up_next` wraps; same keys as
    // `playingKey`). Otherwise compare queue places; a track not in the
    // queue counts as later. With two entries under repeat all, `p` to
    // the other entry is also its up next, so it slides as forward.
    const fromIndex = queue.findIndex((q) => q.key === from);
    const toIndex = queue.findIndex((q) => q.key === playingKey);
    const forward = lastUpNext.current != null && lastUpNext.current === playingKey;
    const back = !forward && fromIndex >= 0 && toIndex >= 0 && toIndex < fromIndex;
    slide.current = viewTransition(apply, {
      kind: "slide",
      dir: back ? "back" : "forward",
      // The new cover (and backdrop) drawn before the new screen is taken.
      settle: () =>
        decodedWithin(
          [coverIn(content.current), view.current?.querySelector(".now-playing-backdrop:last-of-type")],
          DECODE_WAIT_MS,
        ),
    });
    // `shown` is read for the key it had when `playingKey` changed.
  }, [playingKey]);

  useLayoutEffect(() => {
    lastUpNext.current = snapshot.upNextKey;
  });

  // Without View Transitions: the new cover and title lines fade in.
  useFadeOnChange(
    shown.key,
    () => (viewTransitionsOn() ? [] : [coverIn(content.current), heading.current]),
    { duration: "--motion" },
  );

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
      <Backdrop url={shownTrack?.coverUrl ?? null} />
      <div className="now-playing-scrim" />
      <div className="now-playing-main">
        {playing && shownTrack ? (
          <div className="now-playing-content" ref={content}>
            <Cover
              url={shownTrack.coverUrl}
              name={shownTrack.album ?? shownTrack.title}
              size="large"
              className="now-playing-cover"
            />
            <div className="now-playing-info">
              <div className="now-playing-heading" ref={heading}>
                <Heading track={shownTrack} mix={shownMix} onGo={onClose} />
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
          </div>
        ) : (
          <div className="now-playing-nothing">
            {snapshot.mix && <ActiveMixLine mix={snapshot.mix} className="now-playing-mix" />}
            <p className="secondary">Nothing playing</p>
          </div>
        )}
        {lyricsShown && (
          <div className="now-playing-lyrics" ref={lyricsArea}>
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
        <span className="now-playing-title-text">{track.title}</span>
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
