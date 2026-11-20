import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Banner } from "./components/Banner";
import { GroundField } from "./components/GroundField";
import { PageGlow } from "./components/PageGlow";
import { DialogHost } from "./components/Dialog";
import { forgetGenreCovers } from "./components/GenreTiles";
import { MixDialogHost } from "./components/MixDialogs";
import { Player } from "./components/Player";
import { QueuePanel } from "./components/QueuePanel";
import { QueuePopover } from "./components/QueuePopover";
import { Toasts } from "./components/Toasts";
import { TopBar } from "./components/TopBar";
import { useBrowseKeys } from "./hooks/useBrowseKeys";
import { PageScroller } from "./hooks/useLayout";
import { useFullscreen } from "./hooks/useFullscreen";
import { useRootSwipe } from "./hooks/useRootSwipe";
import { clearLyricsCache } from "./hooks/useLyrics";
import { useShortcuts } from "./hooks/useShortcuts";
import { clearBrowseCache } from "./lib/browse";
import { decodedWithin, viewTransition } from "./lib/motion";
import { sourceReplaced, useNav } from "./state/nav";
import { setConfigured, startSources, useSources } from "./state/source";
import { connect, useAppState } from "./state/store";
import { startAccent } from "./state/accent";
import { NowPlayingOpener } from "./state/nowPlaying";
import { usePageCover, usePageGlow } from "./state/pageSubject";
import { useColourScope, usePageGround } from "./state/palettes";
import type { Snapshot } from "./lib/types";
import { BrowseView } from "./views/BrowseView";
import { NowPlayingView, queueToggleMovesContent } from "./views/NowPlayingView";
import { Onboarding } from "./views/Onboarding";

/** The queue panel over the page, open or not (1.x's column used `coda.queueOpen`). */
const QUEUE_OPEN_KEY = "coda.queuePanelOpen";
/** The Now Playing view's queue panel, remembered apart from the layout's. */
const NP_QUEUE_OPEN_KEY = "coda.nowPlayingQueueOpen";
/** The longest wait for the morph's end cover to decode (the screen is frozen meanwhile). */
const MORPH_DECODE_WAIT_MS = 150;

function loadFlag(key: string, fallback: boolean): boolean {
  try {
    const value = localStorage.getItem(key);
    return value === null ? fallback : value === "true";
  } catch {
    return fallback;
  }
}

/** A remembered open/closed flag: the flag, a toggle and a close. */
function useStoredFlag(key: string, fallback: boolean): [boolean, () => void, () => void] {
  const [open, setOpen] = useState(() => loadFlag(key, fallback));
  const update = useCallback(
    (next: (was: boolean) => boolean) =>
      setOpen((was) => {
        const value = next(was);
        try {
          localStorage.setItem(key, String(value));
        } catch {
          // Not saved; harmless.
        }
        return value;
      }),
    [key],
  );
  const toggle = useCallback(() => update((was) => !was), [update]);
  const close = useCallback(() => update(() => false), [update]);
  return [open, toggle, close];
}

/**
 * Settings replaced the library or the Navidrome server: cached answers
 * and the views of that source are stale.
 */
function useBrowseGenerations(snapshot: Snapshot | null) {
  const seen = useRef<Snapshot["browse"] | null>(null);
  // Only once started: startup itself sets the first generations.
  const ready = snapshot?.phase === "ready";
  const library = ready ? snapshot.browse.library : undefined;
  const server = ready ? snapshot.browse.server : undefined;
  useEffect(() => {
    if (library === undefined || server === undefined) return;
    const last = seen.current;
    seen.current = { library, server };
    if (!last) return;
    if (server !== last.server) {
      clearBrowseCache();
      clearLyricsCache();
      forgetGenreCovers();
      sourceReplaced("navidrome");
    }
    if (library !== last.library) sourceReplaced("local");
  }, [library, server]);
}

/**
 * Tells the source store which sources are configured, so the shown one
 * is always set up: when the chosen source is not (no music folder, or no
 * server; e.g. at startup or after settings or onboarding changed the
 * sources), the other one is shown.
 */
function useConfiguredSources(snapshot: Snapshot | null) {
  const ready = snapshot?.phase === "ready";
  const local = ready && snapshot.scan.state !== "noLibrary";
  const server = ready && snapshot.navidrome.state !== "notConfigured";
  useEffect(() => {
    if (ready) setConfigured(local, server);
  }, [ready, local, server]);
}

export default function App() {
  useEffect(() => connect(), []);
  useEffect(() => startAccent(), []);
  useEffect(() => startSources(), []);
  const sources = useSources();
  const { snapshot, queue } = useAppState();
  const stack = useNav();
  const top = stack[stack.length - 1];
  useConfiguredSources(snapshot);
  useBrowseGenerations(snapshot);
  const [layoutQueueOpen, toggleLayoutQueue, closeLayoutQueue] = useStoredFlag(
    QUEUE_OPEN_KEY,
    false,
  );
  const [npQueueOpen, toggleNpQueueNow] = useStoredFlag(NP_QUEUE_OPEN_KEY, false);
  const [nowPlaying, setNowPlaying] = useState(false);
  // Lyrics mode in Now Playing: sticky for the session, off at launch.
  const [npLyrics, setNpLyrics] = useState(false);
  // Now Playing's layout switches (`q`, `l`, their buttons, the Up next
  // card) run as view transitions (experience.css). `q`: the cover morphs
  // and the text crossfades, only where the content moves with the panel;
  // elsewhere the panel's own slide is all there is. `l`: the text fades
  // out, the cover glides, then the parts and the lyrics rise in.
  const toggleNpQueue = useCallback(() => {
    if (queueToggleMovesContent(npLyrics)) viewTransition(toggleNpQueueNow, { kind: "queue" });
    else toggleNpQueueNow();
  }, [toggleNpQueueNow, npLyrics]);
  // The state each request asks for, so a quick second `l` (whose update
  // may run before the first one's has) gets the right direction.
  const npLyricsTarget = useRef(npLyrics);
  const toggleNpLyrics = useCallback(() => {
    const target = !npLyricsTarget.current;
    npLyricsTarget.current = target;
    viewTransition(() => setNpLyrics(target), { kind: "lyrics", dir: target ? "enter" : "leave" });
  }, []);
  // Any navigation (a top bar entry, Go to album from Now Playing, search)
  // shows the browse view again.
  useEffect(() => setNowPlaying(false), [top]);
  // Now Playing hides the rest of the window; `q` toggles its own panel.
  const immersive = nowPlaying && snapshot !== null;
  const toggleQueue = immersive ? toggleNpQueue : toggleLayoutQueue;
  useFocusAfterNowPlaying(immersive);
  // The page takes the colours of its subject (an album page its cover…);
  // Now Playing takes the playing cover's itself. `.main.page` is also
  // the page's one scroll container (its glow, header and lists scroll in
  // it, under the top bar and the player), shared with the views.
  //
  // The views get the page as a ref through `PageScroller`, but that ref
  // must change when the element appears: React attaches a host's ref
  // only after its children's layout effects have run, so when `<main>`
  // mounts in the same commit as the view (closing Now Playing,
  // navigating from it, finishing onboarding) every child effect would
  // see `null` and, with unchanged deps, never run again (docs/LESSONS.md).
  // So the element is state, set by a callback ref, and the context value
  // is a new ref object whenever it changes: every `[scroller]` effect
  // runs again once the page exists.
  const mainRef = useRef<HTMLElement | null>(null);
  const [pageEl, setPageEl] = useState<HTMLElement | null>(null);
  const setMain = useCallback((el: HTMLElement | null) => {
    mainRef.current = el;
    setPageEl(el);
  }, []);
  const pageScroller = useMemo(() => ({ current: pageEl }), [pageEl]);
  const pageCover = usePageCover(immersive ? null : top.id) ?? null;
  useColourScope(mainRef, pageCover);
  // The whole page takes its subject's ground field (pages without one:
  // the base), kept behind Now Playing so closing it shows the page as it
  // was. Pages with a bloom of their cover draw no full-width glow.
  const groundStops = usePageGround(usePageCover(top.id), !immersive);
  const pageGlow = usePageGlow(top.id) ? pageCover : null;

  // `started`: the engine accepts intents (not starting or quitting);
  // `ready`: playback works too.
  const started = snapshot?.phase === "ready";
  const ready = started && snapshot.status !== "unavailable";
  const onboarding = started && snapshot.problems.some((p) => p.kind === "noConfig");
  // Leaving Now Playing (any route) leaves a full screen it entered.
  const fullscreen = useFullscreen(immersive && !onboarding);
  // A sideways two-finger swipe switches Home, Search and Library (not
  // in Now Playing or onboarding).
  useRootSwipe(snapshot !== null && !immersive && !onboarding);
  const { enter: enterFullscreen, leavesOnClose } = fullscreen;
  // Opening and closing Now Playing with a track playing morphs the
  // player's cover into Now Playing's and back (a View Transition; the
  // rest crossfades; experience.css `np-morph`). Not when the window
  // changes size meanwhile (entering or leaving full screen), which would
  // cut the transition short: then Now Playing fades in by itself.
  const hasTrack = snapshot?.playing != null;
  const morphNowPlaying = useCallback((open: boolean) => {
    viewTransition(() => setNowPlaying(open), {
      kind: "np-morph",
      dir: open ? "open" : "close",
      // The cover it grows into (or back into) drawn before the new screen is taken.
      settle: () =>
        decodedWithin(
          [document.querySelector(open ? ".now-playing-content > .now-playing-cover" : ".player-cover")],
          MORPH_DECODE_WAIT_MS,
        ),
    });
  }, []);
  // The player's cover: Now Playing windowed.
  const openNowPlaying = useCallback(() => {
    if (hasTrack) morphNowPlaying(true);
    else setNowPlaying(true);
  }, [hasTrack, morphNowPlaying]);
  const closeNowPlaying = useCallback(() => {
    if (hasTrack && !leavesOnClose()) morphNowPlaying(false);
    else setNowPlaying(false);
  }, [hasTrack, leavesOnClose, morphNowPlaying]);
  // `f` and the player's Full screen button: Now Playing in full screen
  // (the player's cover opens it windowed). The one place that decides
  // it. No morph: the window resizes as it enters full screen.
  const connected = snapshot !== null;
  const openNowPlayingFullscreen = useCallback(() => {
    setNowPlaying(true);
    // Not before the view can show (no snapshot yet).
    if (connected) void enterFullscreen();
  }, [connected, enterFullscreen]);
  useBrowseKeys({
    enabled: ready && !onboarding,
    active: !onboarding,
    // Back closes Now Playing first; the stack stays.
    onBack: nowPlaying ? closeNowPlaying : undefined,
    // Esc closes the queue panel first.
    onEscape: !immersive && layoutQueueOpen ? closeLayoutQueue : undefined,
  });
  useShortcuts({
    volume: snapshot?.volume ?? 0,
    enabled: ready,
    started,
    onToggleQueue: toggleQueue,
    onToggleNowPlaying: onboarding
      ? undefined
      : nowPlaying
        ? closeNowPlaying
        : openNowPlayingFullscreen,
    onToggleLyrics: immersive && !onboarding ? toggleNpLyrics : undefined,
  });

  if (onboarding) {
    return (
      <>
        <Onboarding />
        <Toasts notices={snapshot.notices} />
      </>
    );
  }

  // The floating panel's header does not drag the window (Now Playing's does).
  const queuePanel = (floating: boolean) =>
    snapshot && (
      <QueuePanel
        dragHeader={!floating}
        queue={queue}
        currentKey={snapshot.currentKey}
        playingKey={snapshot.playingKey}
        status={snapshot.status}
        disabled={!ready}
        locked={!started}
        serverConfigured={snapshot.navidrome.state !== "notConfigured"}
        mix={snapshot.mix}
      />
    );

  if (immersive && snapshot) {
    return (
      <div className="app is-immersive">
        <main className="main">
          <NowPlayingView
            snapshot={snapshot}
            queue={queue}
            onClose={closeNowPlaying}
            queueOpen={npQueueOpen}
            onToggleQueue={toggleNpQueue}
            lyricsMode={npLyrics}
            onToggleLyrics={toggleNpLyrics}
            queuePanel={queuePanel(false)}
            fullscreen={fullscreen.fullscreen}
            onToggleFullscreen={fullscreen.toggle}
          />
          {snapshot.phase === "starting" && <div className="starting secondary">Starting…</div>}
        </main>
        <Toasts notices={snapshot.notices} />
        <DialogHost />
        <MixDialogHost />
      </div>
    );
  }

  const queueOpen = layoutQueueOpen;
  return (
    <div className={`app ${queueOpen ? "has-queue-panel" : ""}`}>
      <GroundField stops={groundStops} />
      <TopBar />
      <main className="main page" ref={setMain}>
        <PageScroller.Provider value={pageScroller}>
          <PageGlow url={pageGlow} />
          {/* The clearance under the top bar is on this unpositioned
              wrapper, not on the scroller: a sticky element's offset counts
              from the scroller's padding edge (app.css `.page-content`). */}
          <div className="page-content">
            {snapshot ? (
              <>
                <Banner problems={snapshot.problems} />
                {/* Sources are known once started (and the saved choice read). */}
                {snapshot.phase !== "starting" && sources.loaded && sources.configured && (
                  <NowPlayingOpener.Provider value={openNowPlaying}>
                    <BrowseView entry={top} snapshot={snapshot} />
                  </NowPlayingOpener.Provider>
                )}
              </>
            ) : null}
          </div>
        </PageScroller.Provider>
      </main>
      {snapshot?.phase === "starting" && <div className="starting secondary">Starting…</div>}
      {snapshot && (
        <QueuePopover
          open={queueOpen}
          onClose={closeLayoutQueue}
          coverUrl={snapshot.playing?.coverUrl ?? null}
        >
          {queuePanel(true)}
        </QueuePopover>
      )}
      {snapshot && (
        <Player
          snapshot={snapshot}
          queueOpen={queueOpen}
          onToggleQueue={toggleLayoutQueue}
          onOpenNowPlaying={openNowPlaying}
          onFullscreen={openNowPlayingFullscreen}
        />
      )}
      {snapshot && <Toasts notices={snapshot.notices} />}
      <DialogHost />
      <MixDialogHost />
    </div>
  );
}

/**
 * Closing Now Playing remounts the browse view, so the element focused
 * before it opened is gone: focus the player's cover (its Now Playing
 * button; or the browse view's first tabbable element), unless something
 * else (search, a view) has taken the focus already.
 */
function useFocusAfterNowPlaying(immersive: boolean) {
  const was = useRef(immersive);
  useEffect(() => {
    const closed = was.current && !immersive;
    was.current = immersive;
    if (!closed) return;
    const lost = !document.activeElement || document.activeElement === document.body;
    if (!lost) return;
    const target =
      document.querySelector<HTMLElement>(".player-cover-button:not(:disabled)") ??
      document.querySelector<HTMLElement>('.main [tabindex="0"], .main button:not(:disabled)');
    target?.focus();
  }, [immersive]);
}
