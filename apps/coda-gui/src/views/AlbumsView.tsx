// All albums of a source as a grid. Local: A–Z or by artist. Navidrome:
// the six server orders, paged with infinite scroll.

import { useRef } from "react";
import { AlbumGrid } from "../components/AlbumGrid";
import { Empty, ErrorState, SkeletonGrid } from "../components/LoadState";
import { PagedAlbumsBody, usePagedAlbums } from "../components/PagedAlbums";
import { useLibraryVersion, useScroll } from "../hooks/useLayout";
import { useLoad } from "../hooks/useLoad";
import { browse } from "../lib/browse";
import { shuffleAll, useShuffleContext } from "../lib/play";
import type { AlbumSort, LocalAlbumOrder } from "../lib/types";
import { nav, type Entry, type Route } from "../state/nav";
import { PlayButtons, ScanLine, SOURCE_LABEL, ViewHeader, plural } from "./common";

const SORTS: [AlbumSort, string][] = [
  ["name", "A–Z"],
  ["artist", "By artist"],
  ["newest", "Recently added"],
  ["frequent", "Most played"],
  ["recent", "Recently played"],
  ["random", "Random"],
];

const ORDERS: [LocalAlbumOrder, string][] = [
  ["name", "A–Z"],
  ["artist", "By artist"],
];

type AlbumsRoute = Extract<Route, { kind: "albums" }>;

export function AlbumsView({ entry, route }: { entry: Entry; route: AlbumsRoute }) {
  return route.source === "local" ? (
    <LocalAlbums entry={entry} order={route.order} />
  ) : (
    <NavidromeAlbums entry={entry} sort={route.sort} />
  );
}

function LocalAlbums({ entry, order }: { entry: Entry; order: LocalAlbumOrder }) {
  const version = useLibraryVersion();
  const load = useLoad(() => browse.localAlbums(order), order, version);
  const scroller = useRef<HTMLDivElement>(null);
  const albums = load.data ?? [];
  useScroll(scroller, entry, albums.length > 0);
  useShuffleContext(() => shuffleAll("local"));
  return (
    <section className="view">
      <ViewHeader
        eyebrow={SOURCE_LABEL.local}
        title="Albums"
        actions={
          <>
            <SortSelect
              value={order}
              options={ORDERS}
              onChange={(order) => nav.replace({ kind: "albums", source: "local", order })}
            />
            <PlayButtons onShuffle={() => shuffleAll("local")} />
          </>
        }
      >
        {load.state === "ready" && (
          <div className="secondary">{plural(albums.length, "album")}</div>
        )}
      </ViewHeader>
      <ScanLine />
      <div className="view-body" ref={scroller}>
        {load.state === "loading" && <SkeletonGrid />}
        {load.state === "error" && <ErrorState error={load.error} onRetry={load.retry} />}
        {load.state === "ready" && albums.length === 0 && (
          <Empty title="No albums">Nothing has been scanned yet.</Empty>
        )}
        {albums.length > 0 && (
          <AlbumGrid albums={albums} source="local" scroller={scroller} label="Albums" />
        )}
      </div>
    </section>
  );
}

function NavidromeAlbums({ entry, sort }: { entry: Entry; sort: AlbumSort }) {
  const scroller = useRef<HTMLDivElement>(null);
  const paged = usePagedAlbums((offset) => browse.navidromeAlbums(sort, offset), sort);
  const { pages } = paged;
  useScroll(scroller, entry, pages.albums.length > 0);
  useShuffleContext(() => shuffleAll("navidrome"));

  return (
    <section className="view">
      <ViewHeader
        eyebrow={SOURCE_LABEL.navidrome}
        title="Albums"
        actions={
          <>
            <SortSelect
              value={sort}
              options={SORTS}
              onChange={(sort) => nav.replace({ kind: "albums", source: "navidrome", sort })}
            />
            <PlayButtons onShuffle={() => shuffleAll("navidrome")} />
          </>
        }
      >
        {pages.albums.length > 0 && (
          <div className="secondary">
            {plural(pages.albums.length, "album")}
            {pages.next !== null ? "+" : ""}
          </div>
        )}
      </ViewHeader>
      <div className="view-body" ref={scroller}>
        <PagedAlbumsBody
          paged={paged}
          scroller={scroller}
          label="Albums"
          empty="The server lists no albums."
        />
      </div>
    </section>
  );
}

function SortSelect<T extends string>({
  value,
  options,
  onChange,
}: {
  value: T;
  options: [T, string][];
  onChange: (value: T) => void;
}) {
  return (
    <label className="sort-select">
      <span className="secondary">Sort</span>
      <select value={value} onChange={(e) => onChange(e.target.value as T)}>
        {options.map(([v, label]) => (
          <option key={v} value={v}>
            {label}
          </option>
        ))}
      </select>
    </label>
  );
}
