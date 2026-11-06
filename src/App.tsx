import { useCallback, useEffect, useRef, useState } from "react";
import { Banner } from "./components/Banner";
import { DialogHost } from "./components/Dialog";
import { NowPlayingBar } from "./components/NowPlayingBar";
import { QueuePanel } from "./components/QueuePanel";
import { Sidebar } from "./components/Sidebar";
import { Toasts } from "./components/Toasts";
import { useBrowseKeys } from "./hooks/useBrowseKeys";
import { useShortcuts } from "./hooks/useShortcuts";
import { clearBrowseCache } from "./lib/browse";
import { nav, routeFor, sourceReplaced, useNav, viewFor } from "./state/nav";
import { setConfigured, startSources, useSources } from "./state/source";
import { connect, useAppState } from "./state/store";
import { startTheme } from "./state/theme";
import type { ViewId } from "./views/viewList";
import type { Snapshot } from "./lib/types";
import { BrowseView } from "./views/BrowseView";
import { NowPlayingView } from "./views/NowPlayingView";
import { Onboarding } from "./views/Onboarding";

const QUEUE_OPEN_KEY = "coda.queueOpen";
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

/** A remembered open/closed flag. */
function useStoredFlag(key: string, fallback: boolean): [boolean, () => void] {
  const [open, setOpen] = useState(() => loadFlag(key, fallback));
  const toggle = useCallback(
    () =>
      setOpen((was) => {
        try {
          localStorage.setItem(key, String(!was));
        } catch {
          // Not saved; harmless.
        }
        return !was;
      }),
    [key],
  );
  return [open, toggle];
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
  const view = viewFor(stack[0].route);
  useConfiguredSources(snapshot);
  useBrowseGenerations(snapshot);
  const [layoutQueueOpen, toggleLayoutQueue] = useStoredFlag(QUEUE_OPEN_KEY, true);
  const [npQueueOpen, toggleNpQueue] = useStoredFlag(NP_QUEUE_OPEN_KEY, false);
  const [nowPlaying, setNowPlaying] = useState(false);
  const toggleNowPlaying = useCallback(() => setNowPlaying((open) => !open), []);
  const closeNowPlaying = useCallback(() => setNowPlaying(false), []);
  const showView = (next: ViewId) => {
    setNowPlaying(false);
    nav.root(routeFor(next, sources.active));
  };
  // Any navigation (a sidebar entry, Go to album from Now Playing, search)
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
  useBrowseKeys({
    enabled: ready && !onboarding,
    active: !onboarding,
    // Back closes Now Playing first; the stack stays.
    onBack: nowPlaying ? closeNowPlaying : undefined,
  });
  useShortcuts({
    volume: snapshot?.volume ?? 0,
    enabled: ready,
    started,
    onToggleQueue: toggleQueue,
    onToggleNowPlaying: onboarding ? undefined : toggleNowPlaying,
  });

  if (onboarding) {
    return (
      <>
        <Onboarding />
        <Toasts notices={snapshot.notices} />
      </>
    );
  }

  const queuePanel = snapshot && (
    <QueuePanel
      queue={queue}
      currentKey={snapshot.currentKey}
      playingKey={snapshot.playingKey}
      status={snapshot.status}
      disabled={!ready}
      locked={!started}
      serverConfigured={snapshot.navidrome.state !== "notConfigured"}
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
            queuePanel={queuePanel}
          />
          {snapshot.phase === "starting" && <div className="starting secondary">Starting…</div>}
        </main>
        <Toasts notices={snapshot.notices} />
        <DialogHost />
      </div>
    );
  }

  const queueOpen = layoutQueueOpen;
  return (
    <div className={`app ${queueOpen ? "queue-open" : ""}`}>
      <Sidebar view={view} onView={showView} snapshot={snapshot} />
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
      {queueOpen && queuePanel}
      {snapshot && (
        <NowPlayingBar
          snapshot={snapshot}
          queueOpen={queueOpen}
          onToggleQueue={toggleLayoutQueue}
          onToggleNowPlaying={toggleNowPlaying}
        />
      )}
      {snapshot && <Toasts notices={snapshot.notices} />}
      <DialogHost />
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
