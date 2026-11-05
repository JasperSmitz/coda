// The main view: the top of the navigation stack.

import type { Snapshot } from "../lib/types";
import type { Entry } from "../state/nav";
import { AlbumView } from "./AlbumView";
import { AlbumsView } from "./AlbumsView";
import { ArtistView } from "./ArtistView";
import { ArtistsView } from "./ArtistsView";
import { FolderView } from "./FolderView";
import { PlaceholderView } from "./PlaceholderView";
import { PlaylistView, PlaylistsView } from "./PlaylistsView";
import { SearchView } from "./SearchView";

export function BrowseView({ entry, snapshot }: { entry: Entry; snapshot: Snapshot }) {
  const { route } = entry;
  // Keyed by entry: a pushed view starts fresh, Back restores the old one.
  switch (route.kind) {
    case "search":
      return <SearchView key={entry.id} entry={entry} query={route.query} />;
    case "artists":
      return <ArtistsView key={entry.id} entry={entry} source={route.source} />;
    case "albums":
      return <AlbumsView key={entry.id} entry={entry} route={route} />;
    case "playlists":
      return <PlaylistsView key={entry.id} entry={entry} source={route.source} />;
    case "folder":
      return <FolderView key={entry.id} entry={entry} path={route.path} />;
    case "artist":
      return (
        <ArtistView
          key={entry.id}
          entry={entry}
          source={route.source}
          id={route.id}
          name={route.name}
        />
      );
    case "album":
      return (
        <AlbumView
          key={entry.id}
          entry={entry}
          source={route.source}
          id={route.id}
          name={route.name}
          hint={route.hint}
        />
      );
    case "playlist":
      return (
        <PlaylistView
          key={entry.id}
          entry={entry}
          source={route.source}
          id={route.id}
          name={route.name}
          hint={route.hint}
        />
      );
    case "settings":
      return <PlaceholderView view="settings" snapshot={snapshot} />;
  }
}
