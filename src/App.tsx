import { useEffect, useState } from "react";
import { Banner } from "./components/Banner";
import { NowPlayingBar } from "./components/NowPlayingBar";
import { QueuePanel } from "./components/QueuePanel";
import { Sidebar } from "./components/Sidebar";
import { Toasts } from "./components/Toasts";
import { useShortcuts } from "./hooks/useShortcuts";
import { connect, useAppState } from "./state/store";
import { PlaceholderView } from "./views/PlaceholderView";
import type { ViewId } from "./views/viewList";

const QUEUE_OPEN_KEY = "coda.queueOpen";

function loadQueueOpen(): boolean {
  try {
    return localStorage.getItem(QUEUE_OPEN_KEY) !== "false";
  } catch {
    return true;
  }
}

export default function App() {
  useEffect(() => connect(), []);
  const { snapshot, queue } = useAppState();
  const [view, setView] = useState<ViewId>("local-artists");
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

  return (
    <div className={`app ${queueOpen ? "queue-open" : ""}`}>
      <Sidebar view={view} onView={setView} snapshot={snapshot} />
      <main className="main">
        {snapshot ? (
          <>
            <Banner problems={snapshot.problems} />
            <PlaceholderView view={view} snapshot={snapshot} />
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
