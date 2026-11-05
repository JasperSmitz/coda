// The sidebar's destinations (routes: `routeFor` in state/nav.ts).

export type ViewId =
  | "search"
  | "local-artists"
  | "local-albums"
  | "local-folders"
  | "local-playlists"
  | "navidrome-artists"
  | "navidrome-albums"
  | "navidrome-playlists"
  | "settings";

export const VIEW_TITLES: Record<ViewId, string> = {
  search: "Search",
  "local-artists": "Artists",
  "local-albums": "Albums",
  "local-folders": "Folders",
  "local-playlists": "Playlists",
  "navidrome-artists": "Artists",
  "navidrome-albums": "Albums",
  "navidrome-playlists": "Playlists",
  settings: "Settings",
};

export function viewSource(view: ViewId): string | null {
  if (view.startsWith("local-")) return "Local library";
  if (view.startsWith("navidrome-")) return "Navidrome";
  return null;
}
