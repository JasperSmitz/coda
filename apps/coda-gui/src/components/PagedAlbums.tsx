// Navidrome album pages with infinite scroll: the server's Albums orders
// and a genre's albums. `usePagedAlbums` loads pages as the grid nears its
// end (one at a time); `PagedAlbumsBody` draws the albums (Covers or
// List) with their loading, empty and error states.

import { useCallback, useEffect, useRef, useState, type ReactNode, type RefObject } from "react";
import { toBrowseError } from "../lib/browse";
import type { Album, AlbumPage, BrowseError, LibraryView } from "../lib/types";
import { AlbumCollection, type ListSorts } from "./AlbumCollection";
import { Empty, ErrorState, SkeletonGrid } from "./LoadState";

interface Pages {
  albums: Album[];
  next: number | null;
  loading: boolean;
  error: BrowseError | null;
}

export interface PagedAlbums {
  pages: Pages;
  /** The grid reached its end: the next page, unless one failed. */
  onEnd: () => void;
  /** Starts again from the first page. */
  restart: () => void;
  /** Retries the page that failed, keeping the albums shown. */
  retryMore: () => void;
}

/** `load` fetches the page at an offset; a new `key` starts over. */
export function usePagedAlbums(load: (offset: number) => Promise<AlbumPage>, key: string): PagedAlbums {
  const [pages, setPages] = useState<Pages>({ albums: [], next: 0, loading: false, error: null });
  const [attempt, setAttempt] = useState(0);
  const generation = useRef(0);
  // Set at once (state updates later): two `onEnd`s before a render must
  // not fetch the same page twice.
  const inFlight = useRef(false);
  const loadRef = useRef(load);
  loadRef.current = load;

  const loadMore = useCallback((from: Pages) => {
    if (inFlight.current || from.loading || from.next === null) return;
    const offset = from.next;
    const mine = generation.current;
    inFlight.current = true;
    setPages({ ...from, loading: true, error: null });
    loadRef.current(offset).then(
      (page) => {
        if (generation.current !== mine) return;
        inFlight.current = false;
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
        inFlight.current = false;
        setPages((p) => ({ ...p, loading: false, error: toBrowseError(error) }));
      },
    );
  }, []);

  useEffect(() => {
    generation.current += 1;
    inFlight.current = false;
    const start = { albums: [], next: 0, loading: false, error: null };
    setPages(start);
    loadMore(start);
  }, [key, attempt, loadMore]);

  const pagesRef = useRef(pages);
  pagesRef.current = pages;
  const onEnd = useCallback(() => {
    const p = pagesRef.current;
    if (!p.error) loadMore(p);
  }, [loadMore]);
  const restart = useCallback(() => setAttempt((n) => n + 1), []);
  const retryMore = useCallback(() => loadMore({ ...pagesRef.current, error: null }), [loadMore]);

  return { pages, onEnd, restart, retryMore };
}

export function PagedAlbumsBody<S extends string>({
  paged,
  scroller,
  label,
  empty,
  view = "covers",
  list,
}: {
  paged: PagedAlbums;
  scroller: RefObject<HTMLElement | null>;
  label: string;
  /** The empty state's text. */
  empty: ReactNode;
  /** `null`: not known yet (the preferences are loading): the loading state. */
  view?: LibraryView | null;
  list?: ListSorts<S>;
}) {
  const { pages } = paged;
  const first = pages.albums.length === 0;
  return (
    <>
      {((first && pages.loading) || (!first && view === null)) && <SkeletonGrid />}
      {first && pages.error && <ErrorState error={pages.error} onRetry={paged.restart} />}
      {first && !pages.loading && !pages.error && pages.next === null && (
        <Empty title="No albums">{empty}</Empty>
      )}
      {!first && view !== null && (
        <AlbumCollection
          view={view}
          albums={pages.albums}
          source="navidrome"
          scroller={scroller}
          onEnd={paged.onEnd}
          endKey={`${pages.next}:${pages.loading}`}
          label={label}
          list={list}
        />
      )}
      {!first && pages.loading && <div className="load-more secondary">Loading more…</div>}
      {!first && pages.error && (
        <div className="load-more">
          <span className="secondary">{pages.error.message}</span>
          <button className="button" onClick={paged.retryMore}>
            Retry
          </button>
        </div>
      )}
    </>
  );
}
