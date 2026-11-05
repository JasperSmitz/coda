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
import { nav, routeFor, routeSource, sourceReplaced, useNav, viewFor } from "./state/nav";
import { connect, useAppState } from "./state/store";
import type { ViewId } from "./views/viewList";
import type { Snapshot } from "./lib/types";
import { BrowseView } from "./views/BrowseView";
import { NowPlayingView } from "./views/NowPlayingView";
import { Onboarding } from "./views/Onboarding";

const QUEUE_OPEN_KEY = "coda.queueOpen";

function loadQueueOpen(): boolean {
  try {
    return localStorage.getItem(QUEUE_OPEN_KEY) !== "false";
  } catch {
    return true;
  }
}

/**
 * Settings replaced the library or the Navidrome server: cached answers
 * and the views of that source are stale.
 */
function useBrowseGenerations(snapshot: Snapshot | null) {
  const seen = useRef<Snapshot["browse"] | null>(null);
  const library = snapshot?.browse.library;
  const server = snapshot?.browse.server;
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
 * Keeps the stack on a configured source: when the root's source is not
 * set up (no music folder, or no server; both hidden in the sidebar), the
 * other source's artists are shown instead, e.g. at startup or after
 * settings or onboarding changed the sources.
 */
function useConfiguredRoot(snapshot: Snapshot | null) {
  const stack = useNav();
  const root = routeSource(stack[0].route);
  const ready = snapshot?.phase === "ready";
  const local = ready && snapshot.scan.state !== "noLibrary";
  const server = ready && snapshot.navidrome.state !== "notConfigured";
  useEffect(() => {
    if (!ready) return;
    if (root === "local" && !local && server) nav.root(routeFor("navidrome-artists"));
    else if (root === "navidrome" && !server && local) nav.root(routeFor("local-artists"));
  }, [ready, root, local, server]);
}

export default function App() {
  useEffect(() => connect(), []);
  const { snapshot, queue } = useAppState();
  const stack = useNav();
  const top = stack[stack.length - 1];
  const view = viewFor(stack[0].route);
  useConfiguredRoot(snapshot);
  useBrowseGenerations(snapshot);
  const [queueOpen, setQueueOpen] = useState(loadQueueOpen);
  const [nowPlaying, setNowPlaying] = useState(false);
  const toggleNowPlaying = useCallback(() => setNowPlaying((open) => !open), []);
  const closeNowPlaying = useCallback(() => setNowPlaying(false), []);
  const showView = (next: ViewId) => {
    setNowPlaying(false);
    nav.root(routeFor(next));
  };
  // Any navigation (a sidebar entry, Go to album from Now Playing, search)
  // shows the browse view again.
  useEffect(() => setNowPlaying(false), [top]);
  const toggleQueue = () =>
    setQueueOpen((open) => {
      try {
        localStorage.setItem(QUEUE_OPEN_KEY, String(!open));
      } catch {
        // Not saved; harmless.
      }
      return !open;
    });

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

  return (
    <div className={`app ${queueOpen ? "queue-open" : ""}`}>
      <Sidebar view={view} onView={showView} snapshot={snapshot} />
      <main className="main">
        {snapshot && nowPlaying ? (
          <NowPlayingView snapshot={snapshot} queue={queue} onClose={closeNowPlaying} />
        ) : snapshot ? (
          <>
            <Banner problems={snapshot.problems} />
            {/* Sources are known once started. */}
            {snapshot.phase !== "starting" && <BrowseView entry={top} snapshot={snapshot} />}
          </>
        ) : null}
        {snapshot?.phase === "starting" && <div className="starting secondary">Starting…</div>}
      </main>
      {queueOpen && snapshot && (
        <QueuePanel
          queue={queue}
          currentKey={snapshot.currentKey}
          playingKey={snapshot.playingKey}
          status={snapshot.status}
          disabled={!ready}
          locked={!started}
          serverConfigured={snapshot.navidrome.state !== "notConfigured"}
        />
      )}
      {snapshot && (
        <NowPlayingBar
          snapshot={snapshot}
          queueOpen={queueOpen}
          onToggleQueue={toggleQueue}
          nowPlayingOpen={nowPlaying}
          onToggleNowPlaying={toggleNowPlaying}
        />
      )}
      {snapshot && <Toasts notices={snapshot.notices} />}
      <DialogHost />
    </div>
  );
}
