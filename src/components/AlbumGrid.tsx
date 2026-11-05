// A virtualized album grid (DESIGN.md: ~180px tiles, cover, title and
// artist below). Rows render only near the view's scroll position; the
// arrow keys move between tiles, Enter opens one, right-click or "…"
// opens the album menu. `onEnd` asks for more (infinite scroll).

import {
  memo,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type RefObject,
} from "react";
import { browse } from "../lib/browse";
import { enqueue, playTracks } from "../lib/play";
import type { Album, SourceKind } from "../lib/types";
import { nav } from "../state/nav";
import { useAppState } from "../state/store";
import { ContextMenu, type MenuState } from "./ContextMenu";
import { Cover } from "./Cover";
import { MoreIcon } from "./icons";

const TILE = 180;
const GAP = 22;
/** Cover plus two text lines. */
const TEXT = 46;
const OVERSCAN_ROWS = 2;

interface Props {
  albums: Album[];
  source: SourceKind;
  scroller: RefObject<HTMLElement | null>;
  /** Hide the artist line (an artist's own page). */
  hideArtist?: boolean;
  /** Called when the last rows come into view. */
  onEnd?: () => void;
  label: string;
}

export function openAlbum(source: SourceKind, album: Album) {
  nav.push({ kind: "album", source, id: album.id, name: album.name, hint: album });
}

export function AlbumGrid({ albums, source, scroller, hideArtist = false, onEnd, label }: Props) {
  const { snapshot } = useAppState();
  // `locked`: no queue edits (starting or quitting); `disabled`: no playback either.
  const locked = snapshot?.phase !== "ready";
  const disabled = locked || snapshot.status === "unavailable";
  const grid = useRef<HTMLDivElement>(null);
  const [view, setView] = useState({ top: 0, height: 800, offset: 0, width: 800 });
  const [menu, setMenu] = useState<(MenuState & { album: Album }) | null>(null);
  const focusAfterRender = useRef<number | null>(null);

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
  }, [scroller]);

  // Content above the grid may load later (another search group).
  useLayoutEffect(() => {
    const offset = grid.current?.offsetTop ?? 0;
    if (offset !== view.offset) setView((v) => ({ ...v, offset }));
  });

  const columns = Math.max(1, Math.floor((view.width + GAP) / (TILE + GAP)));
  const tile = Math.floor((view.width - GAP * (columns - 1)) / columns);
  const rowHeight = Math.min(tile, TILE + 40) + TEXT + GAP;
  const rowCount = Math.ceil(albums.length / columns);
  const first = Math.max(0, Math.floor((view.top - view.offset) / rowHeight) - OVERSCAN_ROWS);
  const last = Math.min(
    rowCount,
    Math.ceil((view.top - view.offset + view.height) / rowHeight) + OVERSCAN_ROWS,
  );

  // Infinite scroll: ask for more when the last rows are near.
  const nearEnd = rowCount > 0 && last >= rowCount;
  useEffect(() => {
    if (nearEnd) onEnd?.();
  }, [nearEnd, onEnd, albums.length]);

  useEffect(() => {
    const index = focusAfterRender.current;
    if (index === null) return;
    const el = grid.current?.querySelector<HTMLElement>(`[data-index="${index}"]`);
    if (el) {
      el.focus({ preventScroll: true });
      focusAfterRender.current = null;
    }
  });

  const focusTile = (index: number) => {
    const el = scroller.current;
    if (!el) return;
    const row = Math.floor(index / columns);
    const top = view.offset + row * rowHeight;
    if (top < el.scrollTop) el.scrollTop = top;
    else if (top + rowHeight > el.scrollTop + el.clientHeight)
      el.scrollTop = top + rowHeight - el.clientHeight;
    focusAfterRender.current = index;
    const tileEl = grid.current?.querySelector<HTMLElement>(`[data-index="${index}"]`);
    if (tileEl) {
      tileEl.focus({ preventScroll: true });
      focusAfterRender.current = null;
    }
  };

  const onKeyDown = (e: KeyboardEvent) => {
    const target = e.target as HTMLElement;
    const at = Number(target.dataset.index ?? -1);
    if (at < 0) return;
    let to: number;
    switch (e.key) {
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
        to = albums.length - 1;
        break;
      case "ContextMenu": {
        const rect = target.getBoundingClientRect();
        setMenu({ x: rect.left + 20, y: rect.top + 40, key: albums[at].id, album: albums[at] });
        e.preventDefault();
        return;
      }
      default:
        return;
    }
    e.preventDefault();
    e.stopPropagation();
    focusTile(Math.min(albums.length - 1, Math.max(0, to)));
  };

  const tracksOf = (album: Album) => browse.album(source, album.id).then((d) => d.tracks);

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
              hideArtist={hideArtist}
              onOpen={() => openAlbum(source, album)}
              onMenu={(x, y) => setMenu({ x, y, key: album.id, album })}
            />
          );
        });
      })}
      {menu && (
        <ContextMenu
          state={menu}
          onClose={() => setMenu(null)}
          items={[
            {
              label: "Play",
              disabled,
              action: () => void tracksOf(menu.album).then((t) => playTracks(t)),
            },
            {
              label: "Shuffle",
              disabled,
              action: () => void tracksOf(menu.album).then((t) => playTracks(t, undefined, true)),
            },
            {
              label: "Play next",
              disabled: locked,
              action: () => void tracksOf(menu.album).then((t) => enqueue(t, true)),
            },
            {
              label: "Add to queue",
              disabled: locked,
              action: () => void tracksOf(menu.album).then((t) => enqueue(t, false)),
            },
            // ADD TO PLAYLIST: "Add to playlist…" goes here (later slice).
            { label: "Open album", action: () => openAlbum(source, menu.album) },
            {
              label: "Go to artist",
              disabled: menu.album.artistId === null,
              action: () =>
                menu.album.artistId !== null &&
                nav.push({
                  kind: "artist",
                  source,
                  id: menu.album.artistId,
                  name: menu.album.artist ?? "Artist",
                }),
            },
          ]}
        />
      )}
    </div>
  );
}

interface TileProps {
  album: Album;
  index: number;
  left: number;
  top: number;
  size: number;
  hideArtist: boolean;
  onOpen: () => void;
  onMenu: (x: number, y: number) => void;
}

const AlbumTile = memo(function AlbumTile({
  album,
  index,
  left,
  top,
  size,
  hideArtist,
  onOpen,
  onMenu,
}: TileProps) {
  const cover = Math.min(size, TILE + 40);
  const sub = hideArtist
    ? album.year
      ? String(album.year)
      : ""
    : (album.artist ?? "Unknown artist");
  return (
    <div className="album-tile" role="listitem" style={{ left, top, width: size }}>
      <button
        className="album-open"
        data-index={index}
        onClick={onOpen}
        onContextMenu={(e) => {
          e.preventDefault();
          onMenu(e.clientX, e.clientY);
        }}
        title={album.name}
      >
        <Cover url={album.coverUrl} name={album.name} size="small" className="album-cover" />
        <span className="album-title" style={{ maxWidth: cover }}>
          {album.name}
        </span>
        <span className="album-sub secondary" style={{ maxWidth: cover }}>
          {sub}
        </span>
      </button>
      <button
        className="icon-button album-more"
        tabIndex={-1}
        aria-label={`More for ${album.name}`}
        style={{ left: cover - 36 }}
        onClick={(e) => {
          const rect = e.currentTarget.getBoundingClientRect();
          onMenu(rect.left, rect.bottom);
        }}
      >
        <MoreIcon size={16} />
      </button>
    </div>
  );
});
