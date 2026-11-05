import type { Snapshot } from "../lib/types";
import { VIEW_TITLES, viewSource, type ViewId } from "./viewList";

/** A browse view that is not built yet (M2), with what the engine knows. */
export function PlaceholderView({ view, snapshot }: { view: ViewId; snapshot: Snapshot }) {
  const source = viewSource(view);
  let detail: string;
  if (view.startsWith("local-")) {
    const { scan } = snapshot;
    detail =
      scan.state === "noLibrary"
        ? "No music folder is configured."
        : `${scan.tracks.toLocaleString()} tracks and ${scan.playlists.toLocaleString()} playlists scanned${scan.state === "scanning" ? " so far" : ""}.`;
  } else if (view.startsWith("navidrome-")) {
    detail = {
      notConfigured: "No Navidrome server is configured.",
      connecting: "Connecting to the server…",
      connected: "Connected to the server.",
      failed: `Not connected: ${snapshot.navidrome.message ?? "unknown error"}`,
    }[snapshot.navidrome.state];
  } else {
    detail = "";
  }
  return (
    <section className="view">
      <header className="view-header">
        {source && <div className="eyebrow">{source}</div>}
        <h1>{VIEW_TITLES[view]}</h1>
      </header>
      <div className="view-body placeholder">
        <p className="coming-soon">Coming soon</p>
        {detail && <p className="secondary">{detail}</p>}
      </div>
    </section>
  );
}
