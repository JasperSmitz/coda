// Library's List of albums (DESIGN.md "Library"): dense 52px rows (a
// small cover, title, artist, genre, year, or plays when sorted by Most
// played, length), virtualized against the page like the track tables,
// so it stays fast with tens of thousands of albums. A column head whose
// sort the source offers sorts by it; the current sort is marked. ↑/↓
// select, Enter or double-click opens, Shift+Enter plays, right-click, the
// context-menu key or "…" opens the album menu. `onEnd` asks for more
// (infinite scroll).

import {
  memo,
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type RefObject,
} from "react";
import { clearHeight, revealRow } from "../hooks/useLayout";
import { formatTime } from "../lib/format";
import { openRoute } from "../lib/play";
import { morphKey } from "../lib/pageTransition";
import type { Album, SourceKind } from "../lib/types";
import { AlbumMenu, openAlbum, playAlbum, usePlayable, type AlbumMenuState } from "./AlbumMenu";
import { Cover } from "./Cover";
import { MoreIcon } from "./icons";
import { NameLink } from "./NameLink";

const ROW = 52;
const OVERSCAN = 10;

/** The columns a list can sort by. */
export type ListColumn = "title" | "artist" | "year" | "plays";

interface Props<S extends string> {
  albums: Album[];
  source: SourceKind;
  scroller: RefObject<HTMLElement | null>;
  /** Show listens instead of the year (the Most played order). */
  plays?: boolean;
  /** The sort shown and the sort each column head stands for, if any. */
  sort?: S;
  sorts?: Partial<Record<ListColumn, S>>;
  onSort?: (sort: S) => void;
  onEnd?: () => void;
  endKey?: unknown;
  label: string;
}

const HEAD: [ListColumn | null, string][] = [
  ["title", "Title"],
  ["artist", "Artist"],
  [null, "Genre"],
];

export function AlbumList<S extends string>({
  albums,
  source,
  scroller,
  plays = false,
  sort,
  sorts = {},
  onSort,
  onEnd,
  endKey,
  label,
}: Props<S>) {
  const { disabled } = usePlayable();
  const list = useRef<HTMLDivElement>(null);
  const [view, setView] = useState({ top: 0, height: 800, offset: 0 });
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [menu, setMenu] = useState<AlbumMenuState | null>(null);
  const idPrefix = useId();
  const found = selectedId === null ? -1 : albums.findIndex((a) => a.id === selectedId);
  const selected = found >= 0 ? found : null;

  useLayoutEffect(() => {
    const el = scroller.current;
    const own = list.current;
    if (!el || !own) return;
    const update = () =>
      setView({ top: el.scrollTop, height: el.clientHeight, offset: own.offsetTop });
    update();
    el.addEventListener("scroll", update, { passive: true });
    const observer = new ResizeObserver(update);
    observer.observe(el);
    return () => {
      el.removeEventListener("scroll", update);
      observer.disconnect();
    };
  }, [scroller]);

  // Content above the list may load later.
  useLayoutEffect(() => {
    const offset = list.current?.offsetTop ?? 0;
    if (offset !== view.offset) setView((v) => ({ ...v, offset }));
  });

  const start = Math.max(0, Math.floor((view.top - view.offset) / ROW) - OVERSCAN);
  const end = Math.min(
    albums.length,
    Math.ceil((view.top - view.offset + view.height) / ROW) + OVERSCAN,
  );

  const nearEnd = albums.length > 0 && end >= albums.length;
  useEffect(() => {
    if (nearEnd) onEnd?.();
  }, [nearEnd, onEnd, albums.length, endKey]);

  const select = (index: number) => {
    setSelectedId(albums[index].id);
    const el = scroller.current;
    if (el) revealRow(el, view.offset + index * ROW, ROW);
  };

  // Stable, so the memoized rows do not re-render as the page scrolls.
  const openMenu = useCallback((album: Album, x: number, y: number) => {
    setSelectedId(album.id);
    setMenu({ x, y, key: album.id, album });
  }, []);

  const onKeyDown = (e: KeyboardEvent) => {
    if (albums.length === 0) return;
    const at = selected ?? -1;
    const room = scroller.current ? clearHeight(scroller.current) : view.height;
    const page = Math.max(1, Math.floor(room / ROW) - 1);
    const key = e.shiftKey && e.key === "F10" ? "ContextMenu" : e.key;
    let to: number | null = null;
    switch (key) {
      case "ArrowDown":
        to = at + 1;
        break;
      case "ArrowUp":
        to = at < 0 ? 0 : at - 1;
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
        to = albums.length - 1;
        break;
      // Nothing selected: Enter and the menu key do nothing (as in Artists).
      case "Enter": {
        if (selected === null) break;
        const album = albums[selected];
        if (e.shiftKey) {
          if (!disabled) playAlbum(source, album);
        } else {
          openAlbum(source, album);
        }
        break;
      }
      case "ContextMenu": {
        if (selected === null) break;
        const rect = list.current?.getBoundingClientRect();
        openMenu(
          albums[selected],
          (rect?.left ?? 0) + 40,
          (rect?.top ?? 0) + (selected + 1) * ROW,
        );
        break;
      }
      default:
        return;
    }
    e.preventDefault();
    e.stopPropagation();
    if (to !== null) select(Math.min(albums.length - 1, Math.max(0, to)));
  };

  const head: [ListColumn | null, string][] = [
    ...HEAD,
    [plays ? "plays" : "year", plays ? "Plays" : "Year"],
  ];

  return (
    <div className="album-list-wrap">
      <div className="album-list-head" role="presentation">
        <span />
        {head.map(([column, text]) => {
          const value = column ? sorts[column] : undefined;
          const active = value !== undefined && value === sort;
          return value !== undefined && onSort ? (
            <button
              key={text}
              className={`album-list-sort ${active ? "is-active" : ""}`}
              aria-pressed={active}
              title={`Sort by ${text.toLowerCase()}`}
              onClick={() => onSort(value)}
            >
              {text}
            </button>
          ) : (
            <span key={text}>{text}</span>
          );
        })}
        <span className="album-list-length">Length</span>
        <span />
      </div>
      <div
        ref={list}
        className="album-list"
        role="listbox"
        aria-label={label}
        tabIndex={0}
        aria-activedescendant={selected !== null ? `${idPrefix}-${selected}` : undefined}
        aria-keyshortcuts="Enter Shift+Enter"
        style={{ height: albums.length * ROW }}
        onKeyDown={onKeyDown}
      >
        {albums.slice(start, end).map((album, i) => {
          const index = start + i;
          return (
            <AlbumRow
              key={`${album.id}-${index}`}
              id={`${idPrefix}-${index}`}
              index={index}
              album={album}
              source={source}
              plays={plays}
              selected={selected === index}
              onSelect={setSelectedId}
              onMenu={openMenu}
            />
          );
        })}
      </div>
      {menu && (
        <AlbumMenu
          menu={menu}
          source={source}
          onClose={() => {
            setMenu(null);
            list.current?.focus();
          }}
        />
      )}
    </div>
  );
}

interface RowProps {
  id: string;
  index: number;
  album: Album;
  source: SourceKind;
  plays: boolean;
  selected: boolean;
  onSelect: (id: string) => void;
  onMenu: (album: Album, x: number, y: number) => void;
}

const AlbumRow = memo(function AlbumRow({
  id,
  index,
  album,
  source,
  plays,
  selected,
  onSelect,
  onMenu,
}: RowProps) {
  const artistId = album.artistId;
  const artist = album.artist;
  // Never heard (0): blank.
  const fourth = plays
    ? album.playCount
      ? album.playCount.toLocaleString()
      : ""
    : album.year
      ? String(album.year)
      : "";
  return (
    <div
      id={id}
      role="option"
      aria-selected={selected}
      className={`album-row ${selected ? "is-selected" : ""}`}
      style={{ top: index * ROW }}
      onClick={() => onSelect(album.id)}
      onDoubleClick={() => openAlbum(source, album)}
      onContextMenu={(e) => {
        e.preventDefault();
        onMenu(album, e.clientX, e.clientY);
      }}
    >
      <Cover
        url={album.coverUrl}
        name={album.name}
        size="small"
        className="album-row-cover"
        morph={morphKey("album", source, album.id)}
      />
      <span className="album-row-title" title={album.name}>
        {album.name}
      </span>
      <span className="album-row-text secondary">
        {artistId !== null && artist ? (
          <NameLink
            inRow
            label={`Go to artist ${artist}`}
            onGo={() => openRoute({ kind: "artist", source, id: artistId, name: artist })}
          >
            {artist}
          </NameLink>
        ) : (
          (artist ?? "Unknown artist")
        )}
      </span>
      <span className="album-row-text secondary" title={album.genre ?? undefined}>
        {album.genre ?? ""}
      </span>
      <span className="album-row-number secondary">{fourth}</span>
      <span className="album-row-number secondary">
        {album.durationMs !== null ? formatTime(album.durationMs) : ""}
      </span>
      <button
        className="icon-button album-row-more"
        tabIndex={-1}
        aria-label={`More for ${album.name}`}
        title="More"
        onClick={(e) => {
          e.stopPropagation();
          const rect = e.currentTarget.getBoundingClientRect();
          onMenu(album, rect.left, rect.bottom);
        }}
        onDoubleClick={(e) => e.stopPropagation()}
      >
        <MoreIcon size={16} />
      </button>
    </div>
  );
});
