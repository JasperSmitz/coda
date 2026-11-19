// All albums of a source as Covers or List (DESIGN.md "Library"). Local:
// A–Z, by artist, by year or most played (our listens). Navidrome: the
// server's orders, paged with infinite scroll. The sort and Covers / List
// are remembered per source (`state/libraryView.ts`).

import { AlbumCollection, type ListSorts } from "../components/AlbumCollection";
import { Empty, ErrorState, SkeletonGrid, SkeletonRows } from "../components/LoadState";
import { PagedAlbumsBody, usePagedAlbums } from "../components/PagedAlbums";
import { useLibraryVersion, usePageScroller, useScroll } from "../hooks/useLayout";
import { useLoad } from "../hooks/useLoad";
import { browse } from "../lib/browse";
import { shuffleAll, useShuffleContext } from "../lib/play";
import type { AlbumSort, LocalAlbumOrder } from "../lib/types";
import {
  localAlbumOrder,
  rememberAlbumSort,
  serverAlbumSort,
  useLibraryView,
} from "../state/libraryView";
import { nav, type Entry, type Route } from "../state/nav";
import {
  ListHeader,
  PlayButtons,
  ScanLine,
  SOURCE_LABEL,
  SortSelect,
  ViewToggle,
  plural,
} from "./common";

const SORTS: [AlbumSort, string][] = [
  ["name", "A–Z"],
  ["artist", "Artist"],
  ["newest", "Recently added"],
  ["frequent", "Most played"],
  ["recent", "Recently played"],
  ["random", "Random"],
];

const ORDERS: [LocalAlbumOrder, string][] = [
  ["name", "A–Z"],
  ["artist", "Artist"],
  ["year", "Year"],
  ["mostPlayed", "Most played"],
];

type AlbumsRoute = Extract<Route, { kind: "albums" }>;

export function AlbumsView({ entry, route }: { entry: Entry; route: AlbumsRoute }) {
  return route.source === "local" ? (
    <LocalAlbums entry={entry} order={localAlbumOrder(route.order)} />
  ) : (
    <NavidromeAlbums entry={entry} sort={serverAlbumSort(route.sort)} />
  );
}

function setLocalOrder(order: LocalAlbumOrder) {
  rememberAlbumSort("local", order);
  nav.replace({ kind: "albums", source: "local", order });
}

function setServerSort(sort: AlbumSort) {
  rememberAlbumSort("navidrome", sort);
  nav.replace({ kind: "albums", source: "navidrome", sort });
}

function LocalAlbums({ entry, order }: { entry: Entry; order: LocalAlbumOrder }) {
  const version = useLibraryVersion();
  const load = useLoad(() => browse.localAlbums(order), order, version);
  const scroller = usePageScroller();
  // `known`: the Covers / List to show (a List default never shows Covers first).
  const [view, setView, known] = useLibraryView("local", "albums");
  const albums = load.data ?? [];
  useScroll(scroller, entry, albums.length > 0 && known);
  useShuffleContext(() => shuffleAll("local"));
  const list: ListSorts<LocalAlbumOrder> = {
    sort: order,
    plays: order === "mostPlayed",
    sorts: { title: "name", artist: "artist", year: "year", plays: "mostPlayed" },
    onSort: setLocalOrder,
  };
  return (
    <section className="view">
      <ListHeader
        eyebrow={SOURCE_LABEL.local}
        title="Albums"
        meta={load.state === "ready" && plural(albums.length, "album")}
        tools={
          <>
            <SortSelect value={order} options={ORDERS} onChange={setLocalOrder} />
            <ViewToggle value={view} onChange={setView} />
            <PlayButtons onShuffle={() => shuffleAll("local")} />
          </>
        }
      />
      <ScanLine />
      <div className="view-body">
        {(load.state === "loading" || (!known && albums.length > 0)) &&
          (view === "list" ? <SkeletonRows /> : <SkeletonGrid />)}
        {load.state === "error" && <ErrorState error={load.error} onRetry={load.retry} />}
        {load.state === "ready" && albums.length === 0 && (
          <Empty title="No albums">Nothing has been scanned yet.</Empty>
        )}
        {albums.length > 0 && known && (
          <AlbumCollection
            view={view}
            albums={albums}
            source="local"
            scroller={scroller}
            label="Albums"
            list={list}
          />
        )}
      </div>
    </section>
  );
}

function NavidromeAlbums({ entry, sort }: { entry: Entry; sort: AlbumSort }) {
  const scroller = usePageScroller();
  const paged = usePagedAlbums((offset) => browse.navidromeAlbums(sort, offset), sort);
  const { pages } = paged;
  const [view, setView, known] = useLibraryView("navidrome", "albums");
  useScroll(scroller, entry, pages.albums.length > 0 && known);
  useShuffleContext(() => shuffleAll("navidrome"));
  // The server's play counts, when its albums carry them.
  const plays = sort === "frequent" && pages.albums.some((a) => a.playCount !== null);
  const list: ListSorts<AlbumSort> = {
    sort,
    plays,
    sorts: plays
      ? { title: "name", artist: "artist", plays: "frequent" }
      : { title: "name", artist: "artist" },
    onSort: setServerSort,
  };

  return (
    <section className="view">
      <ListHeader
        eyebrow={SOURCE_LABEL.navidrome}
        title="Albums"
        meta={
          pages.albums.length > 0 &&
          `${plural(pages.albums.length, "album")}${pages.next !== null ? "+" : ""}`
        }
        tools={
          <>
            <SortSelect value={sort} options={SORTS} onChange={setServerSort} />
            <ViewToggle value={view} onChange={setView} />
            <PlayButtons onShuffle={() => shuffleAll("navidrome")} />
          </>
        }
      />
      <div className="view-body">
        <PagedAlbumsBody
          paged={paged}
          scroller={scroller}
          label="Albums"
          empty="The server lists no albums."
          view={known ? view : null}
          list={list}
        />
      </div>
    </section>
  );
}
