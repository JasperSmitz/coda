import { useEffect, useRef, useState } from "react";
import { Banner } from "./components/Banner";
import { NowPlayingBar } from "./components/NowPlayingBar";
import { QueuePanel } from "./components/QueuePanel";
import { Sidebar } from "./components/Sidebar";
import { Toasts } from "./components/Toasts";
import { useBrowseKeys } from "./hooks/useBrowseKeys";
import { useShortcuts } from "./hooks/useShortcuts";
import { nav, routeFor, useNav, viewFor } from "./state/nav";
import { connect, useAppState } from "./state/store";
import type { Snapshot } from "./lib/types";
import { BrowseView } from "./views/BrowseView";

const QUEUE_OPEN_KEY = "coda.queueOpen";

function loadQueueOpen(): boolean {
  try {
    return localStorage.getItem(QUEUE_OPEN_KEY) !== "false";
  } catch {
    return true;
  }
}

/**
 * Starts on Navidrome's artists when there is no music folder but a
 * server (once, after startup; the user's own navigation wins).
 */
function useStartView(snapshot: Snapshot | null) {
  const decided = useRef(false);
  useEffect(() => {
    if (decided.current || snapshot?.phase !== "ready") return;
    decided.current = true;
    const untouched = nav.top().route.kind === "artists" && nav.top().id === 1;
    if (
      untouched &&
      snapshot.scan.state === "noLibrary" &&
      snapshot.navidrome.state !== "notConfigured"
    ) {
      nav.root(routeFor("navidrome-artists"));
    }
  }, [snapshot]);
}

export default function App() {
  useEffect(() => connect(), []);
  const { snapshot, queue } = useAppState();
  const stack = useNav();
  const top = stack[stack.length - 1];
  const view = viewFor(stack[0].route);
  useStartView(snapshot);
  const [queueOpen, setQueueOpen] = useState(loadQueueOpen);
  const toggleQueue = () =>
    setQueueOpen((open) => {
      try {
        localStorage.setItem(QUEUE_OPEN_KEY, String(!open));
      } catch {
        // Not saved; harmless.
      }
      return !open;
    });

  const ready = snapshot?.phase === "ready" && snapshot.status !== "unavailable";
  useShortcuts({ volume: snapshot?.volume ?? 0, enabled: ready, onToggleQueue: toggleQueue });
  useBrowseKeys(ready);

  return (
    <div className={`app ${queueOpen ? "queue-open" : ""}`}>
      <Sidebar view={view} onView={(id) => nav.root(routeFor(id))} snapshot={snapshot} />
      <main className="main">
        {snapshot ? (
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
        />
      )}
      {snapshot && (
        <NowPlayingBar snapshot={snapshot} queueOpen={queueOpen} onToggleQueue={toggleQueue} />
      )}
      {snapshot && <Toasts notices={snapshot.notices} />}
    </div>
  );
}
