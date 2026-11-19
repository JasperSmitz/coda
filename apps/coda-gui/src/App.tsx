import { useCallback, useEffect, useRef, useState } from "react";
import { Banner } from "./components/Banner";
import { DialogHost } from "./components/Dialog";
import { MixDialogHost } from "./components/MixDialogs";
import { NowPlayingBar } from "./components/NowPlayingBar";
import { QueuePanel } from "./components/QueuePanel";
import { QueuePopover } from "./components/QueuePopover";
import { Toasts } from "./components/Toasts";
import { TopBar } from "./components/TopBar";
import { useBrowseKeys } from "./hooks/useBrowseKeys";
import { useFullscreen } from "./hooks/useFullscreen";
import { clearLyricsCache } from "./hooks/useLyrics";
import { useShortcuts } from "./hooks/useShortcuts";
import { clearBrowseCache } from "./lib/browse";
import { viewTransition } from "./lib/motion";
import { sourceReplaced, useNav } from "./state/nav";
import { setConfigured, startSources, useSources } from "./state/source";
import { connect, useAppState } from "./state/store";
import { startTheme } from "./state/theme";
import type { Snapshot } from "./lib/types";
import { BrowseView } from "./views/BrowseView";
import { NowPlayingView, queueToggleMovesContent } from "./views/NowPlayingView";
import { Onboarding } from "./views/Onboarding";

/** The queue panel over the page, open or not (1.x's column used `coda.queueOpen`). */
const QUEUE_OPEN_KEY = "coda.queuePanelOpen";
/** The Now Playing view's queue panel, remembered apart from the layout's. */
const NP_QUEUE_OPEN_KEY = "coda.nowPlayingQueueOpen";

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
  useEffect(() => startTheme(), []);
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
  const toggleNowPlaying = useCallback(() => setNowPlaying((open) => !open), []);
  const closeNowPlaying = useCallback(() => setNowPlaying(false), []);
  // Any navigation (a top bar entry, Go to album from Now Playing, search)
  // shows the browse view again.
  useEffect(() => setNowPlaying(false), [top]);
  // Now Playing hides the rest of the window; `q` toggles its own panel.
  const immersive = nowPlaying && snapshot !== null;
  const toggleQueue = immersive ? toggleNpQueue : toggleLayoutQueue;
  useFocusAfterNowPlaying(immersive);

  // `started`: the engine accepts intents (not starting or quitting);
  // `ready`: playback works too.
  const started = snapshot?.phase === "ready";
  const ready = started && snapshot.status !== "unavailable";
  const onboarding = started && snapshot.problems.some((p) => p.kind === "noConfig");
  // Leaving Now Playing (any route) leaves a full screen it entered.
  const fullscreen = useFullscreen(immersive && !onboarding);
  const enterFullscreen = fullscreen.enter;
  // `f` and the bar's Full screen button: Now Playing in full screen (the
  // bar's cover opens it windowed). The one place that decides it.
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
    <div className="app">
      <TopBar />
      <main className="main">
        {snapshot ? (
          <>
            <Banner problems={snapshot.problems} />
            {/* Sources are known once started (and the saved choice read). */}
            {snapshot.phase !== "starting" && sources.loaded && sources.configured && (
              <BrowseView entry={top} snapshot={snapshot} />
            )}
          </>
        ) : null}
        {snapshot?.phase === "starting" && <div className="starting secondary">Starting…</div>}
      </main>
      {snapshot && (
        <QueuePopover open={queueOpen} onClose={closeLayoutQueue}>
          {queuePanel(true)}
        </QueuePopover>
      )}
      {snapshot && (
        <NowPlayingBar
          snapshot={snapshot}
          queueOpen={queueOpen}
          onToggleQueue={toggleLayoutQueue}
          onToggleNowPlaying={toggleNowPlaying}
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
 * before it opened is gone: focus the bar's Now Playing button (or the
 * browse view's first tabbable element), unless something else (search,
 * a view) has taken the focus already.
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
      document.querySelector<HTMLElement>(".bar-cover-button:not(:disabled)") ??
      document.querySelector<HTMLElement>('.main [tabindex="0"], .main button:not(:disabled)');
    target?.focus();
  }, [immersive]);
}
