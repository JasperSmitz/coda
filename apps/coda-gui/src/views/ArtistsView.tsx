// All artists of a source, A–Z, as Covers (a virtualized grid of round
// images) or List (virtualized rows), with a letter index on the right.
// List: ↑/↓ select, Enter or click opens. Covers: the arrow keys move
// between tiles, Enter or click opens. A row's or tile's menu
// (right-click or the context-menu key) offers "Add to mix…".

import { memo, useCallback, useMemo, useRef, useState, type KeyboardEvent, type MutableRefObject, type RefObject } from "react";
import { gridKeyTarget, useGridGeometry } from "../components/AlbumGrid";
import { ContextMenu, type MenuState } from "../components/ContextMenu";
import { Cover } from "../components/Cover";
import { Empty, ErrorState, SkeletonRows } from "../components/LoadState";
import {
  clearHeight,
  revealRow,
  scrollToTop,
  useElementSize,
  useLibraryVersion,
  useOffsetTop,
  usePageScroller,
  useScroll,
} from "../hooks/useLayout";
import { useLoad } from "../hooks/useLoad";
import { usePendingFocus } from "../hooks/usePendingFocus";
import { browse } from "../lib/browse";
import { addToMixFlow } from "../lib/mixes";
import { artistIngredient } from "../lib/mixRecipe";
import { shuffleAll, useShuffleContext } from "../lib/play";
import type { Artist, SourceKind } from "../lib/types";
import { useLibraryView } from "../state/libraryView";
import { nav, type Entry } from "../state/nav";
import { useAppState } from "../state/store";
import { ListHeader, PlayButtons, ScanLine, SOURCE_LABEL, ViewToggle, plural } from "./common";

const ROW = 48;
const OVERSCAN = 8;

function letterOf(name: string): string {
  const first = [...name.trim()][0]?.toUpperCase() ?? "#";
  return /\p{L}/u.test(first) ? first.normalize("NFD")[0] : "#";
}

export function openArtist(source: SourceKind, artist: Artist) {
  nav.push({ kind: "artist", source, id: artist.id, name: artist.name });
}

export function ArtistsView({ entry, source }: { entry: Entry; source: SourceKind }) {
  const version = useLibraryVersion();
  const load = useLoad(() => browse.artists(source), source, source === "local" ? version : null);
  const scroller = usePageScroller();
  const artists = load.data ?? [];
  // `known`: the Covers / List to show (a List default never shows Covers first).
  const [view, setView, known] = useLibraryView(source, "artists");
  const scrollTop = useScroll(scroller, entry, artists.length > 0 && known);
  const { height } = useElementSize(scroller);
  const [selected, setSelected] = useState<number | null>(null);
  useShuffleContext(() => shuffleAll(source));
  const [menu, setMenu] = useState<(MenuState & { artist: Artist }) | null>(null);
  const list = useRef<HTMLDivElement>(null);
  // Where the list starts in the page (below the header and the tools).
  const offset = useOffsetTop(list);
  const { snapshot } = useAppState();
  const started = snapshot?.phase === "ready";
  // Covers: the grid scrolls a tile's row into view (the letter index).
  const jumpTo = useRef<((index: number) => void) | null>(null);
  const focusTile = useRef<((index: number) => void) | null>(null);
  const openTileMenu = useCallback(
    (artist: Artist, x: number, y: number) => setMenu({ x, y, key: artist.id, artist }),
    [],
  );

  const letters = useMemo(() => {
    const first = new Map<string, number>();
    artists.forEach((a, i) => {
      const letter = letterOf(a.name);
      if (!first.has(letter)) first.set(letter, i);
    });
    return [...first.entries()];
  }, [artists]);

  const scrollTo = (index: number) => {
    const el = scroller.current;
    if (!el) return;
    revealRow(el, offset + index * ROW, ROW);
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (artists.length === 0) return;
    const at = selected ?? -1;
    const room = scroller.current ? clearHeight(scroller.current) : height;
    const page = Math.max(1, Math.floor(room / ROW) - 1);
    let to: number | null = null;
    // Shift+F10 opens the row's menu like the context-menu key.
    const key = e.shiftKey && e.key === "F10" ? "ContextMenu" : e.key;
    switch (key) {
      case "ArrowDown":
        to = at + 1;
        break;
      case "ArrowUp":
        to = at - 1;
        break;
      case "PageDown":
        to = at + page;
        break;
      case "PageUp":
        to = at - page;
        break;
      case "Home":
        to = 0;
        break;
      case "End":
        to = artists.length - 1;
        break;
      case "Enter":
        if (selected !== null) openArtist(source, artists[selected]);
        break;
      case "ContextMenu":
        if (selected !== null) {
          const row = document.getElementById(`artist-${selected}`)?.getBoundingClientRect();
          const artist = artists[selected];
          setMenu({ x: (row?.left ?? 0) + 40, y: row?.bottom ?? 0, key: artist.id, artist });
        }
        break;
      default:
        // Letters stay global shortcuts; the index on the right jumps.
        return;
    }
    e.preventDefault();
    e.stopPropagation();
    if (to !== null) {
      const index = Math.min(artists.length - 1, Math.max(0, to));
      setSelected(index);
      scrollTo(index);
    }
  };

  const first = Math.max(0, Math.floor((scrollTop - offset) / ROW) - OVERSCAN);
  const last = Math.min(artists.length, Math.ceil((scrollTop - offset + height) / ROW) + OVERSCAN);

  return (
    <section className="view">
      <ListHeader
        eyebrow={SOURCE_LABEL[source]}
        title="Artists"
        meta={load.state === "ready" && plural(artists.length, "artist")}
        tools={
          <>
            <ViewToggle value={view} onChange={setView} />
            <PlayButtons onShuffle={() => shuffleAll(source)} />
          </>
        }
      />
      {source === "local" && <ScanLine />}
      <div className="artists-layout">
        <div className="view-body list-body">
          {(load.state === "loading" || (!known && artists.length > 0)) && <SkeletonRows />}
          {load.state === "error" && <ErrorState error={load.error} onRetry={load.retry} />}
          {load.state === "ready" && artists.length === 0 && (
            <Empty title="No artists">
              {source === "local"
                ? "Nothing has been scanned yet."
                : "The server lists no artists."}
            </Empty>
          )}
          {artists.length > 0 && known && view === "covers" && (
            <ArtistGrid
              artists={artists}
              source={source}
              scroller={scroller}
              jumpTo={jumpTo}
              focusTile={focusTile}
              onMenu={openTileMenu}
            />
          )}
          {artists.length > 0 && known && view === "list" && (
            <div
              className="artist-list"
              ref={list}
              role="listbox"
              aria-label="Artists"
              tabIndex={0}
              aria-activedescendant={selected !== null ? `artist-${selected}` : undefined}
              style={{ height: artists.length * ROW }}
              onKeyDown={onKeyDown}
            >
              {artists.slice(first, last).map((artist, i) => {
                const index = first + i;
                const count = artist.albumCount !== null ? plural(artist.albumCount, "album") : "";
                return (
                  <div
                    key={artist.id}
                    id={`artist-${index}`}
                    role="option"
                    aria-selected={selected === index}
                    className={`artist-row ${selected === index ? "is-selected" : ""}`}
                    style={{ top: index * ROW }}
                    onClick={() => {
                      setSelected(index);
                      openArtist(source, artist);
                    }}
                    onContextMenu={(e) => {
                      e.preventDefault();
                      setSelected(index);
                      setMenu({ x: e.clientX, y: e.clientY, key: artist.id, artist });
                    }}
                  >
                    <Cover
                      url={artist.coverUrl}
                      name={artist.name}
                      size="small"
                      className="artist-thumb"
                    />
                    <span className="artist-name">{artist.name}</span>
                    <span className="secondary">{count}</span>
                  </div>
                );
              })}
            </div>
          )}
        </div>
        {letters.length > 1 && (
          <nav className="letter-index" aria-label="Jump to letter">
            {letters.map(([letter, index]) => (
              <button
                key={letter}
                tabIndex={-1}
                onClick={() => {
                  if (view === "covers") {
                    jumpTo.current?.(index);
                    return;
                  }
                  setSelected(index);
                  const el = scroller.current;
                  if (el) scrollToTop(el, offset + index * ROW);
                }}
              >
                {letter}
              </button>
            ))}
          </nav>
        )}
      </div>
      {menu && (
        <ContextMenu
          state={menu}
          onClose={() => {
            setMenu(null);
            if (view === "list") {
              list.current?.focus();
            } else {
              const at = artists.findIndex((a) => a.id === menu.artist.id);
              if (at >= 0) focusTile.current?.(at);
            }
          }}
          items={[
            { label: "Open", action: () => openArtist(source, menu.artist) },
            {
              label: "Add to mix…",
              disabled: !started,
              action: () => void addToMixFlow(source, artistIngredient(source, menu.artist)),
            },
          ]}
        />
      )}
    </section>
  );
}

/** The smallest artist tile; tiles grow to fill a row. */
const ARTIST_TILE = 160;
const ARTIST_GAP = 22;
/** The name and the album count below the image. */
const ARTIST_TEXT = 48;

/**
 * Covers: artists as round images with their name and album count, a
 * virtualized grid like the albums'. `jumpTo` is set to scroll a tile's
 * row to the top and focus it (the letter index); `focusTile` to focus
 * one (after its menu closes).
 */
function ArtistGrid({
  artists,
  source,
  scroller,
  jumpTo,
  focusTile,
  onMenu,
}: {
  artists: Artist[];
  source: SourceKind;
  scroller: RefObject<HTMLElement | null>;
  jumpTo: MutableRefObject<((index: number) => void) | null>;
  /** Set to focus a tile (the menu gives the focus back to its tile). */
  focusTile: MutableRefObject<((index: number) => void) | null>;
  onMenu: (artist: Artist, x: number, y: number) => void;
}) {
  const grid = useRef<HTMLDivElement>(null);
  const geometry = useGridGeometry(scroller, grid, artists.length, {
    tile: ARTIST_TILE,
    gap: ARTIST_GAP,
    text: ARTIST_TEXT,
  });
  const { columns, tile, cover, rowHeight, rowCount, first, last, offset } = geometry;
  // A tile far away mounts only with the scroll's render: focus it then.
  const focusAt = usePendingFocus(grid);
  jumpTo.current = (index) => {
    const el = scroller.current;
    if (!el) return;
    scrollToTop(el, offset + Math.floor(index / columns) * rowHeight);
    focusAt(index);
  };
  focusTile.current = focusAt;

  const onKeyDown = (e: KeyboardEvent) => {
    const target = e.target as HTMLElement;
    const at = Number(target.dataset.index ?? -1);
    if (at < 0) return;
    if (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) {
      const rect = target.getBoundingClientRect();
      onMenu(artists[at], rect.left + 20, rect.top + 40);
      e.preventDefault();
      return;
    }
    const to = gridKeyTarget(e.key, at, columns, artists.length);
    if (to === null) return;
    e.preventDefault();
    e.stopPropagation();
    const el = scroller.current;
    if (el) revealRow(el, offset + Math.floor(to / columns) * rowHeight, rowHeight);
    focusAt(to);
  };

  return (
    <div
      ref={grid}
      className="artist-grid"
      role="list"
      aria-label="Artists"
      style={{ height: rowCount * rowHeight }}
      onKeyDown={onKeyDown}
    >
      {Array.from({ length: last - first }, (_, r) => {
        const row = first + r;
        return artists.slice(row * columns, row * columns + columns).map((artist, c) => {
          const index = row * columns + c;
          return (
            <ArtistTile
              key={artist.id}
              artist={artist}
              index={index}
              left={c * (tile + ARTIST_GAP)}
              top={row * rowHeight}
              size={tile}
              cover={cover}
              source={source}
              onMenu={onMenu}
            />
          );
        });
      })}
    </div>
  );
}

const ArtistTile = memo(function ArtistTile({
  artist,
  index,
  left,
  top,
  size,
  cover,
  source,
  onMenu,
}: {
  artist: Artist;
  index: number;
  left: number;
  top: number;
  size: number;
  cover: number;
  source: SourceKind;
  onMenu: (artist: Artist, x: number, y: number) => void;
}) {
  return (
    <div className="artist-tile" role="listitem" style={{ left, top, width: size }}>
      <button
        className="artist-open"
        data-index={index}
        title={artist.name}
        onClick={() => openArtist(source, artist)}
        onContextMenu={(e) => {
          e.preventDefault();
          onMenu(artist, e.clientX, e.clientY);
        }}
      >
        <Cover
          url={artist.coverUrl}
          name={artist.name}
          size="small"
          className="artist-cover"
        />
        <span className="artist-tile-name" style={{ maxWidth: cover }}>
          {artist.name}
        </span>
        <span className="artist-tile-sub secondary" style={{ maxWidth: cover }}>
          {artist.albumCount !== null ? plural(artist.albumCount, "album") : ""}
        </span>
      </button>
    </div>
  );
});
