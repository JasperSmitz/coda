// All albums of a source as a grid. Local: A–Z or by artist. Navidrome:
// the six server orders, paged with infinite scroll.

import { useCallback, useEffect, useRef, useState } from "react";
import { AlbumGrid } from "../components/AlbumGrid";
import { Empty, ErrorState, SkeletonGrid } from "../components/LoadState";
import { useLibraryVersion, useScroll } from "../hooks/useLayout";
import { useLoad } from "../hooks/useLoad";
import { browse, toBrowseError } from "../lib/browse";
import { shuffleAll, useShuffleContext } from "../lib/play";
import type { Album, AlbumSort, BrowseError, LocalAlbumOrder } from "../lib/types";
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
  useShuffleContext(() => void shuffleAll("local"));
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
            <PlayButtons onShuffle={() => void shuffleAll("local")} />
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

interface Pages {
  albums: Album[];
  next: number | null;
  loading: boolean;
  error: BrowseError | null;
}

function NavidromeAlbums({ entry, sort }: { entry: Entry; sort: AlbumSort }) {
  const scroller = useRef<HTMLDivElement>(null);
  const [pages, setPages] = useState<Pages>({ albums: [], next: 0, loading: false, error: null });
  const [attempt, setAttempt] = useState(0);
  const generation = useRef(0);
  useScroll(scroller, entry, pages.albums.length > 0);
  useShuffleContext(() => void shuffleAll("navidrome"));

  const loadMore = useCallback(
    (from: Pages) => {
      if (from.loading || from.next === null) return;
      const offset = from.next;
      const mine = generation.current;
      setPages({ ...from, loading: true, error: null });
      browse.navidromeAlbums(sort, offset).then(
        (page) => {
          if (generation.current !== mine) return;
          setPages((p) => {
            // A random order may repeat albums across pages.
            const seen = new Set(p.albums.map((a) => a.id));
            const fresh = page.albums.filter((a) => !seen.has(a.id));
            return {
              albums: [...p.albums, ...fresh],
              next: page.nextOffset,
              loading: false,
              error: null,
            };
          });
        },
        (error: unknown) => {
          if (generation.current !== mine) return;
          setPages((p) => ({ ...p, loading: false, error: toBrowseError(error) }));
        },
      );
    },
    [sort],
  );

  useEffect(() => {
    generation.current += 1;
    const start = { albums: [], next: 0, loading: false, error: null };
    setPages(start);
    loadMore(start);
  }, [sort, attempt, loadMore]);

  const pagesRef = useRef(pages);
  pagesRef.current = pages;
  const onEnd = useCallback(() => {
    const p = pagesRef.current;
    if (!p.error) loadMore(p);
  }, [loadMore]);

  const first = pages.albums.length === 0;
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
            <PlayButtons onShuffle={() => void shuffleAll("navidrome")} />
          </>
        }
      >
        {!first && (
          <div className="secondary">
            {plural(pages.albums.length, "album")}
            {pages.next !== null ? "+" : ""}
          </div>
        )}
      </ViewHeader>
      <div className="view-body" ref={scroller}>
        {first && pages.loading && <SkeletonGrid />}
        {first && pages.error && (
          <ErrorState
            error={pages.error}
            onRetry={() => {
              setAttempt((n) => n + 1);
            }}
          />
        )}
        {first && !pages.loading && !pages.error && pages.next === null && (
          <Empty title="No albums">The server lists no albums.</Empty>
        )}
        {!first && (
          <AlbumGrid
            albums={pages.albums}
            source="navidrome"
            scroller={scroller}
            onEnd={onEnd}
            label="Albums"
          />
        )}
        {!first && pages.loading && <div className="load-more secondary">Loading more…</div>}
        {!first && pages.error && (
          <div className="load-more">
            <span className="secondary">{pages.error.message}</span>
            <button className="button" onClick={() => loadMore({ ...pages, error: null })}>
              Retry
            </button>
          </div>
        )}
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
