import type { ReactNode } from "react";
import type { Snapshot, SourceKind } from "../lib/types";
import { bothConfigured, switchSource, useSources } from "../state/source";
import type { ViewId } from "../views/viewList";
import {
  AlbumIcon,
  ArtistIcon,
  FolderIcon,
  HomeIcon,
  LocalIcon,
  PlaylistIcon,
  SearchIcon,
  ServerIcon,
  SettingsIcon,
} from "./icons";

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

const SOURCE_NAME: Record<SourceKind, string> = { local: "Local", navidrome: "Navidrome" };
/** Ctrl on Linux, Cmd on macOS (both work). */
const MOD = /Mac/.test(navigator.platform) ? "⌘" : "Ctrl+";

/** The active source's status: the scan line or the connection dot. */
function SourceStatus({ source, snapshot }: { source: SourceKind; snapshot: Snapshot | null }) {
  if (source === "local") return <span className="nav-status">{scanLine(snapshot)}</span>;
  const state = snapshot?.navidrome.state ?? "notConfigured";
  return <span className={`connection connection-${state}`} title={state} />;
}

export function Sidebar({ view, onView, snapshot }: Props) {
  const sources = useSources();
  const source = sources.active;
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
  const sourceButton = (kind: SourceKind, index: number, icon: ReactNode) => (
    <button
      role="radio"
      aria-checked={source === kind}
      className={`source-option ${source === kind ? "is-active" : ""}`}
      onClick={() => switchSource(kind)}
      title={`Browse ${kind === "local" ? "the local library" : "Navidrome"} (${MOD}${index})`}
    >
      {icon}
      <span>{SOURCE_NAME[kind]}</span>
    </button>
  );
  return (
    <nav className="sidebar" aria-label="Library">
      {bothConfigured(sources) ? (
        <div className="source-switch-group">
          <div className="source-switch" role="radiogroup" aria-label="Source">
            {sourceButton("local", 1, <LocalIcon size={15} />)}
            {sourceButton("navidrome", 2, <ServerIcon size={15} />)}
          </div>
          <div className="nav-heading source-status">
            <span>{source === "local" ? "Local library" : "Server"}</span>
            <SourceStatus source={source} snapshot={snapshot} />
          </div>
        </div>
      ) : (
        <div className="nav-heading">
          <span>{SOURCE_NAME[source]}</span>
          <SourceStatus source={source} snapshot={snapshot} />
        </div>
      )}
      <ul>
        {item("home", "Home", <HomeIcon />)}
        {item("search", "Search", <SearchIcon />)}
      </ul>
      <ul>
        {item("albums", "Albums", <AlbumIcon />)}
        {item("artists", "Artists", <ArtistIcon />)}
        {item("playlists", "Playlists", <PlaylistIcon />)}
        {source === "local" && item("folders", "Folders", <FolderIcon />)}
      </ul>
      <ul className="nav-bottom">{item("settings", "Settings", <SettingsIcon />)}</ul>
    </nav>
  );
}
