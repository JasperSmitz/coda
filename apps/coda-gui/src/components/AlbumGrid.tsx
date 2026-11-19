// A virtualized album grid, Library's Covers (DESIGN.md "Library": tiles
// of at least 178px, the cover with a soft shadow, the title and
// "artist · year" below; hover lifts the cover and shows a round Play
// button in the album's own accent). Rows render only near the page's
// scroll position; the arrow keys move between tiles, Enter opens one,
// Shift+Enter plays it, right-click or "…" opens the album menu. `onEnd`
// asks for more (infinite scroll). Each tile in view takes its cover's
// accent (`useTileAccents`).

import {
  memo,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type RefObject,
} from "react";
import { openRoute } from "../lib/play";
import { morphKey } from "../lib/pageTransition";
import type { Album, Palette, SourceKind } from "../lib/types";
import { revealRow } from "../hooks/useLayout";
import { usePendingFocus } from "../hooks/usePendingFocus";
import { accentStyle, useTileAccents } from "../state/palettes";
import { AlbumMenu, openAlbum, playAlbum, usePlayable, type AlbumMenuState } from "./AlbumMenu";
import { Cover } from "./Cover";
import { MoreIcon, PlayIcon } from "./icons";
import { NameLink } from "./NameLink";

export { openAlbum } from "./AlbumMenu";

/** The smallest tile (DESIGN.md: ≥178px); tiles grow to fill a row. */
const TILE = 180;
const GAP = 22;
/** Cover plus two text lines. */
const TEXT = 46;
const OVERSCAN_ROWS = 2;

interface Props {
  albums: Album[];
  source: SourceKind;
  scroller: RefObject<HTMLElement | null>;
  /** Hide the artist line (an artist's own page): the year alone. */
  hideArtist?: boolean;
  /** Called when the last rows come into view. */
  onEnd?: () => void;
  /**
   * Changes whenever another page may be asked for (e.g. the next offset):
   * `onEnd` is asked again even if the last page added no new albums.
   */
  endKey?: unknown;
  label: string;
}

/**
 * A virtualized grid's geometry in the page: columns, row height and the
 * rows near the view, following the scroller and the grid's own size.
 */
export function useGridGeometry(
  scroller: RefObject<HTMLElement | null>,
  grid: RefObject<HTMLElement | null>,
  count: number,
  { tile: minTile, gap, text }: { tile: number; gap: number; text: number },
) {
  const [view, setView] = useState({ top: 0, height: 800, offset: 0, width: 800 });
  useLayoutEffect(() => {
    const el = scroller.current;
    const own = grid.current;
    if (!el || !own) return;
    const update = () =>
      setView({
        top: el.scrollTop,
        height: el.clientHeight,
        offset: own.offsetTop,
        width: own.clientWidth,
      });
    update();
    el.addEventListener("scroll", update, { passive: true });
    const observer = new ResizeObserver(update);
    observer.observe(el);
    observer.observe(own);
    return () => {
      el.removeEventListener("scroll", update);
      observer.disconnect();
    };
  }, [scroller, grid]);

  // Content above the grid may load later (another search group).
  useLayoutEffect(() => {
    const offset = grid.current?.offsetTop ?? 0;
    if (offset !== view.offset) setView((v) => ({ ...v, offset }));
  });

  const columns = Math.max(1, Math.floor((view.width + gap) / (minTile + gap)));
  const tile = Math.floor((view.width - gap * (columns - 1)) / columns);
  const cover = Math.min(tile, minTile + 40);
  const rowHeight = cover + text + gap;
  const rowCount = Math.ceil(count / columns);
  const rowAt = (y: number) => Math.floor((y - view.offset) / rowHeight);
  const first = Math.max(0, rowAt(view.top) - OVERSCAN_ROWS);
  const last = Math.min(rowCount, rowAt(view.top + view.height) + 1 + OVERSCAN_ROWS);
  // The rows actually on screen (no overscan): what per-tile colours ask for.
  const seenFirst = Math.max(0, rowAt(view.top));
  const seenLast = Math.min(rowCount, rowAt(view.top + view.height) + 1);
  return {
    columns,
    tile,
    cover,
    rowHeight,
    rowCount,
    first,
    last,
    seen: [seenFirst * columns, Math.min(count, seenLast * columns)] as const,
    offset: view.offset,
  };
}

/** Arrow keys, Home and End in a grid of `count` tiles: the index to go to, or `null`. */
export function gridKeyTarget(key: string, at: number, columns: number, count: number): number | null {
  let to: number;
  switch (key) {
    case "ArrowRight":
      to = at + 1;
      break;
    case "ArrowLeft":
      to = at - 1;
      break;
    case "ArrowDown":
      to = at + columns;
      break;
    case "ArrowUp":
      to = at - columns;
      break;
    case "Home":
      to = 0;
      break;
    case "End":
      to = count - 1;
      break;
    default:
      return null;
  }
  return Math.min(count - 1, Math.max(0, to));
}

export function AlbumGrid({
  albums,
  source,
  scroller,
  hideArtist = false,
  onEnd,
  endKey,
  label,
}: Props) {
  const { disabled } = usePlayable();
  const grid = useRef<HTMLDivElement>(null);
  const [menu, setMenu] = useState<AlbumMenuState | null>(null);
  const focusAt = usePendingFocus(grid);
  const geometry = useGridGeometry(scroller, grid, albums.length, {
    tile: TILE,
    gap: GAP,
    text: TEXT,
  });
  const { columns, tile, cover, rowHeight, rowCount, first, last } = geometry;
  const accentOf = useTileAccents(
    albums.slice(geometry.seen[0], geometry.seen[1]).map((a) => a.coverUrl),
  );

  // Infinite scroll: ask for more when the last rows are near.
  const nearEnd = rowCount > 0 && last >= rowCount;
  useEffect(() => {
    if (nearEnd) onEnd?.();
  }, [nearEnd, onEnd, albums.length, endKey]);

  const focusTile = (index: number) => {
    const el = scroller.current;
    if (!el) return;
    const row = Math.floor(index / columns);
    revealRow(el, geometry.offset + row * rowHeight, rowHeight);
    // The tile may only mount with the scroll's render.
    focusAt(index);
  };

  const onKeyDown = (e: KeyboardEvent) => {
    const target = e.target as HTMLElement;
    const at = Number(target.dataset.index ?? -1);
    if (at < 0) return;
    if (e.key === "Enter" && e.shiftKey) {
      e.preventDefault();
      e.stopPropagation();
      if (!disabled) playAlbum(source, albums[at]);
      return;
    }
    if (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) {
      const rect = target.getBoundingClientRect();
      setMenu({ x: rect.left + 20, y: rect.top + 40, key: albums[at].id, album: albums[at] });
      e.preventDefault();
      return;
    }
    const to = gridKeyTarget(e.key, at, columns, albums.length);
    if (to === null) return;
    e.preventDefault();
    e.stopPropagation();
    focusTile(to);
  };

  return (
    <div
      ref={grid}
      className="album-grid"
      role="list"
      aria-label={label}
      style={{ height: rowCount * rowHeight }}
      onKeyDown={onKeyDown}
    >
      {Array.from({ length: last - first }, (_, r) => {
        const row = first + r;
        return albums.slice(row * columns, row * columns + columns).map((album, c) => {
          const index = row * columns + c;
          return (
            <AlbumTile
              key={`${album.id}-${index}`}
              album={album}
              index={index}
              left={c * (tile + GAP)}
              top={row * rowHeight}
              size={tile}
              cover={cover}
              accent={accentOf(album.coverUrl)}
              hideArtist={hideArtist}
              source={source}
              canPlay={!disabled}
              onMenu={setMenu}
            />
          );
        });
      })}
      {menu && <AlbumMenu menu={menu} source={source} onClose={() => setMenu(null)} />}
    </div>
  );
}

interface TileProps {
  album: Album;
  index: number;
  left: number;
  top: number;
  size: number;
  cover: number;
  /** The album's own colours (its accent), once known. */
  accent: Palette | null;
  hideArtist: boolean;
  source: SourceKind;
  canPlay: boolean;
  onMenu: (menu: AlbumMenuState) => void;
}

const AlbumTile = memo(function AlbumTile({
  album,
  index,
  left,
  top,
  size,
  cover,
  accent,
  hideArtist,
  source,
  canPlay,
  onMenu,
}: TileProps) {
  const year = album.year ? String(album.year) : null;
  // A known artist is a link below the button (no button in a button).
  const artistId = hideArtist ? null : album.artistId;
  const artist = hideArtist ? null : album.artist;
  const menuAt = (x: number, y: number) => onMenu({ x, y, key: album.id, album });
  const sub = [hideArtist ? null : (artist ?? "Unknown artist"), year].filter(Boolean).join(" · ");
  return (
    <div
      className="album-tile"
      role="listitem"
      style={{ left, top, width: size, ...accentStyle(accent) }}
      onContextMenu={(e) => {
        e.preventDefault();
        menuAt(e.clientX, e.clientY);
      }}
    >
      <button
        className="album-open"
        data-index={index}
        onClick={() => openAlbum(source, album)}
        title={album.name}
        aria-keyshortcuts="Shift+Enter"
      >
        <Cover
          url={album.coverUrl}
          name={album.name}
          size="small"
          className="album-cover"
          morph={morphKey("album", source, album.id)}
        />
        <span className="album-title" style={{ maxWidth: cover }}>
          {album.name}
        </span>
        {(artistId === null || !artist) && (
          <span className="album-sub secondary" style={{ maxWidth: cover }}>
            {sub}
          </span>
        )}
      </button>
      {artistId !== null && artist && (
        <div className="album-sub card-sub-link secondary" style={{ maxWidth: cover }} title={album.name}>
          <NameLink
            inRow
            label={`Go to artist ${artist}`}
            onGo={() => openRoute({ kind: "artist", source, id: artistId, name: artist })}
          >
            {artist}
          </NameLink>
          {year && <span> · {year}</span>}
        </div>
      )}
      <button
        className="album-play"
        tabIndex={-1}
        aria-label={`Play ${album.name}`}
        title="Play (Shift+Enter)"
        disabled={!canPlay}
        style={{ left: cover - 52, top: cover - 52 }}
        onClick={() => playAlbum(source, album)}
      >
        <PlayIcon size={18} />
      </button>
      <button
        className="icon-button album-more"
        tabIndex={-1}
        aria-label={`More for ${album.name}`}
        style={{ left: cover - 36 }}
        onClick={(e) => {
          const rect = e.currentTarget.getBoundingClientRect();
          menuAt(rect.left, rect.bottom);
        }}
      >
        <MoreIcon size={16} />
      </button>
    </div>
  );
});
