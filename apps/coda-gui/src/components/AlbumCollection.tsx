// Albums as Library's Covers (the grid) or List (the rows), with the
// same scrolling, infinite-scroll and menu behaviour either way.

import type { RefObject } from "react";
import type { Album, LibraryView, SourceKind } from "../lib/types";
import { AlbumGrid } from "./AlbumGrid";
import { AlbumList, type ListColumn } from "./AlbumList";

export interface ListSorts<S extends string> {
  /** The current sort; Most played shows plays instead of years. */
  sort: S;
  plays: boolean;
  sorts: Partial<Record<ListColumn, S>>;
  onSort: (sort: S) => void;
}

export function AlbumCollection<S extends string>({
  view,
  albums,
  source,
  scroller,
  label,
  onEnd,
  endKey,
  list,
}: {
  view: LibraryView;
  albums: Album[];
  source: SourceKind;
  scroller: RefObject<HTMLElement | null>;
  label: string;
  onEnd?: () => void;
  endKey?: unknown;
  /** The list's sortable columns (none: a plain list). */
  list?: ListSorts<S>;
}) {
  return view === "list" ? (
    <AlbumList
      albums={albums}
      source={source}
      scroller={scroller}
      label={label}
      onEnd={onEnd}
      endKey={endKey}
      plays={list?.plays}
      sort={list?.sort}
      sorts={list?.sorts}
      onSort={list?.onSort}
    />
  ) : (
    <AlbumGrid
      albums={albums}
      source={source}
      scroller={scroller}
      label={label}
      onEnd={onEnd}
      endKey={endKey}
    />
  );
}
