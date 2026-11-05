import { useCallback, useEffect, useState } from "react";
import { Banner } from "./components/Banner";
import { NowPlayingBar } from "./components/NowPlayingBar";
import { QueuePanel } from "./components/QueuePanel";
import { Sidebar } from "./components/Sidebar";
import { Toasts } from "./components/Toasts";
import { useShortcuts } from "./hooks/useShortcuts";
import { connect, useAppState } from "./state/store";
import { NowPlayingView } from "./views/NowPlayingView";
import { Onboarding } from "./views/Onboarding";
import { PlaceholderView } from "./views/PlaceholderView";
import { SettingsView } from "./views/SettingsView";
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
  const [nowPlaying, setNowPlaying] = useState(false);
  const toggleNowPlaying = useCallback(() => setNowPlaying((open) => !open), []);
  const closeNowPlaying = useCallback(() => setNowPlaying(false), []);
  const showView = (next: ViewId) => {
    setNowPlaying(false);
    setView(next);
  };
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
            {view === "settings" ? (
              <SettingsView snapshot={snapshot} />
            ) : (
              <PlaceholderView view={view} snapshot={snapshot} />
            )}
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
    </div>
  );
}
