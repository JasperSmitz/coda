import type { ReactNode } from "react";
import type { Snapshot } from "../lib/types";
import type { ViewId } from "../views/viewList";
import { AlbumIcon, ArtistIcon, FolderIcon, PlaylistIcon, SearchIcon, SettingsIcon } from "./icons";

interface Props {
  /** The active entry (the navigation stack's root). */
  view: ViewId | null;
  onView: (view: ViewId) => void;
  snapshot: Snapshot | null;
}

function scanLine(snapshot: Snapshot | null): string | null {
  if (!snapshot) return null;
  const { scan } = snapshot;
  switch (scan.state) {
    case "noLibrary":
      return "Not set up";
    case "scanning":
      return `Scanning… ${scan.tracks.toLocaleString()}`;
    case "finished":
      return `${scan.tracks.toLocaleString()} tracks`;
    case "failed":
      return "Scan failed";
  }
}

export function Sidebar({ view, onView, snapshot }: Props) {
  const item = (id: ViewId, label: string, icon: ReactNode) => (
    <li>
      <button
        className={`nav-item ${view === id ? "is-active" : ""}`}
        aria-current={view === id ? "page" : undefined}
        onClick={() => onView(id)}
      >
        {icon}
        <span>{label}</span>
      </button>
    </li>
  );
  const navidrome = snapshot?.navidrome.state ?? "notConfigured";
  // Sources that are not configured are hidden (once known).
  const known = snapshot?.phase === "ready";
  const showLocal = !known || snapshot.scan.state !== "noLibrary";
  const showNavidrome = !known || navidrome !== "notConfigured";
  return (
    <nav className="sidebar" aria-label="Library">
      <ul>{item("search", "Search", <SearchIcon />)}</ul>
      {showLocal && (
        <div className="nav-group">
          <div className="nav-heading">
            <span>Local</span>
            <span className="nav-status">{scanLine(snapshot)}</span>
          </div>
          <ul>
            {item("local-artists", "Artists", <ArtistIcon />)}
            {item("local-albums", "Albums", <AlbumIcon />)}
            {item("local-folders", "Folders", <FolderIcon />)}
            {item("local-playlists", "Playlists", <PlaylistIcon />)}
          </ul>
        </div>
      )}
      {showNavidrome && (
        <div className="nav-group">
          <div className="nav-heading">
            <span>Navidrome</span>
            <span className={`connection connection-${navidrome}`} title={navidrome} />
          </div>
          <ul>
            {item("navidrome-artists", "Artists", <ArtistIcon />)}
            {item("navidrome-albums", "Albums", <AlbumIcon />)}
            {item("navidrome-playlists", "Playlists", <PlaylistIcon />)}
          </ul>
        </div>
      )}
      <ul className="nav-bottom">{item("settings", "Settings", <SettingsIcon />)}</ul>
    </nav>
  );
}
