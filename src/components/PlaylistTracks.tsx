// The tracks of a Navidrome playlist the user may change: TrackTable's
// look plus multi-select (click, Shift/Cmd/Ctrl-click, Shift+↑/↓,
// Cmd/Ctrl+A), removal (Delete) and reordering (drag-and-drop or
// Alt+↑/↓). Every change is saved at once as Coda does it: a replace of
// the whole list, refused by the backend if the playlist changed on the
// server; a playlist with hidden entries asks first. While an edit runs
// the list shows the new order and takes no further edits.

import { memo, useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type PointerEvent, type RefObject } from "react";
import { useDragReorder } from "../hooks/useDragReorder";
import { useSelection } from "../hooks/useSelection";
import { formatTime } from "../lib/format";
import { canGoToAlbum, canGoToArtist, enqueue, goToAlbum, goToArtist, playTracks, sameTrack } from "../lib/play";
import { addToPlaylistFlow, canAddToPlaylist, confirmRewrite, playlistEdits, songIds, useEditsRunning } from "../lib/playlists";
import type { Hidden, Playlist, Track } from "../lib/types";
import { useAppState } from "../state/store";
import { ContextMenu, type MenuItem, type MenuState } from "./ContextMenu";
import { MoreIcon } from "./icons";
import { Equalizer } from "./QueuePanel";

const ROW = 36;
const OVERSCAN = 10;

interface Props {
  playlist: Playlist;
  /** The tracks as loaded (the server's order). */
  tracks: Track[];
  hidden: Hidden;
  scroller: RefObject<HTMLElement | null>;
  label: string;
}

const plural = (n: number, noun: string) => `${n.toLocaleString()} ${n === 1 ? noun : `${noun}s`}`;

/** `order` with the rows `moved` (keys = original indices) put at `gap`. */
export function moveOrder(order: number[], moved: Set<number>, gap: number): number[] {
  const before = order.slice(0, gap).filter((i) => !moved.has(i));
  const after = order.slice(gap).filter((i) => !moved.has(i));
  const block = order.filter((i) => moved.has(i));
  return [...before, ...block, ...after];
}

export function PlaylistTracks({ playlist, tracks, hidden, scroller, label }: Props) {
  const { snapshot } = useAppState();
  const locked = snapshot?.phase !== "ready";
  const disabled = locked || snapshot.status === "unavailable";
  const server = snapshot !== null && snapshot.navidrome.state !== "notConfigured";
  const playing = snapshot?.playing ?? null;
  const animated = snapshot?.status === "playing";
  const busy = useEditsRunning() > 0;

  // The shown order (indices into `tracks`): the loaded one, or a new one
  // being saved.
  const [order, setOrder] = useState<number[]>(() => tracks.map((_, i) => i));
  const keys = useMemo(() => order.map(String), [order]);
  const selection = useSelection(keys);
  const select = selection.set;
  // A reload (after an edit) starts from the server's order; the old
  // selection's indices would name other songs.
  useEffect(() => {
    setOrder(tracks.map((_, i) => i));
    select([]);
  }, [tracks, select]);
  const shown = useMemo(() => order.map((i) => tracks[i]).filter(Boolean), [order, tracks]);
  const expected = useMemo(() => songIds(tracks), [tracks]);

  const table = useRef<HTMLDivElement>(null);
  const [view, setView] = useState({ top: 0, height: 800, offset: 0 });
  const [menu, setMenu] = useState<(MenuState & { at: number }) | null>(null);

  useLayoutEffect(() => {
    const el = scroller.current;
    const own = table.current;
    if (!el || !own) return;
    const update = () => setView({ top: el.scrollTop, height: el.clientHeight, offset: own.offsetTop });
    update();
    el.addEventListener("scroll", update, { passive: true });
    const observer = new ResizeObserver(update);
    observer.observe(el);
    return () => {
      el.removeEventListener("scroll", update);
      observer.disconnect();
    };
  }, [scroller]);

  useLayoutEffect(() => {
    const offset = table.current?.offsetTop ?? 0;
    if (offset !== view.offset) setView((v) => ({ ...v, offset }));
  });

  const canEdit = !busy && !locked && expected.length === tracks.length;

  const reveal = (at: number) => {
    const el = scroller.current;
    const own = table.current;
    if (!el || !own) return;
    const top = own.offsetTop + at * ROW;
    if (top < el.scrollTop) el.scrollTop = top;
    else if (top + ROW > el.scrollTop + el.clientHeight) el.scrollTop = top + ROW - el.clientHeight;
  };

  const saveOrder = async (next: number[]) => {
    if (!canEdit || next.every((v, i) => v === order[i])) return;
    if (!(await confirmRewrite(hidden, "Save the new order"))) return;
    setOrder(next);
    await playlistEdits.reorder(playlist, expected, next, hidden);
  };

  /** Selected original indices (or `at`'s row when it is outside the selection). */
  const targets = (key: string | null): number[] => {
    if (key !== null && !selection.has(key)) return [Number(key)];
    return selection.ordered().map(Number);
  };

  const remove = async (indices: number[]) => {
    if (!canEdit || indices.length === 0) return;
    const what = indices.length === 1 ? "Remove the song" : `Remove ${plural(indices.length, "song")}`;
    if (!(await confirmRewrite(hidden, what))) return;
    await playlistEdits.remove(playlist, expected, indices, hidden);
  };

  const moveSelection = (delta: -1 | 1) => {
    const moved = new Set(selection.ordered().map(Number));
    if (moved.size === 0) return;
    const positions = order.flatMap((v, at) => (moved.has(v) ? [at] : []));
    const first = Math.min(...positions);
    const last = Math.max(...positions);
    const gap = delta < 0 ? Math.max(0, first - 1) : Math.min(order.length, last + 2);
    void saveOrder(moveOrder(order, moved, gap));
  };

  const { drag, start, onClickCapture } = useDragReorder({
    scroller,
    rows: table,
    rowHeight: ROW,
    count: order.length,
    disabled: !canEdit,
    onDrop: (gap, payload) => void saveOrder(moveOrder(order, new Set(payload.map(Number)), gap)),
  });

  const onRowPointerDown = (e: PointerEvent, key: string) => {
    if (e.button !== 0) return;
    start(e, () => (selection.has(key) ? selection.ordered() : [key]));
    if (!(e.shiftKey || e.metaKey || e.ctrlKey) && !selection.has(key)) selection.click(key, e);
  };

  const play = (at: number) => {
    if (!disabled) playTracks(shown, at);
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (order.length === 0) return;
    const cursor = selection.cursor === null ? -1 : keys.indexOf(selection.cursor);
    const page = Math.max(1, Math.floor(view.height / ROW) - 1);
    const move = (to: number) => {
      const at = Math.min(order.length - 1, Math.max(0, to));
      selection.moveTo(at, e.shiftKey);
      reveal(at);
    };
    switch (e.key) {
      case "ArrowDown":
        if (e.altKey) moveSelection(1);
        else move(cursor + 1);
        break;
      case "ArrowUp":
        if (e.altKey) moveSelection(-1);
        else move(cursor < 0 ? 0 : cursor - 1);
        break;
      case "PageDown":
        move(cursor + page);
        break;
      case "PageUp":
        move(cursor - page);
        break;
      case "Home":
        move(0);
        break;
      case "End":
        move(order.length - 1);
        break;
      case "a":
      case "A":
        if (!(e.metaKey || e.ctrlKey)) return;
        selection.selectAll();
        break;
      case "Enter":
        play(Math.max(0, cursor));
        break;
      case "Delete":
      case "Backspace":
        void remove(selection.ordered().map(Number));
        break;
      case "ContextMenu": {
        const at = Math.max(0, cursor);
        const rect = table.current?.getBoundingClientRect();
        setMenu({ x: (rect?.left ?? 0) + 40, y: (rect?.top ?? 0) + (at + 1) * ROW, key: keys[at], at });
        break;
      }
      default:
        return;
    }
    e.preventDefault();
    e.stopPropagation();
  };

  const menuItems = (key: string, at: number): MenuItem[] => {
    const indices = targets(key);
    const many = indices.length > 1;
    const picked = indices.map((i) => tracks[i]).filter(Boolean);
    const track = tracks[Number(key)];
    return [
      { label: "Play", disabled: disabled || many, action: () => play(at) },
      { label: "Play next", disabled: locked, action: () => enqueue(picked, true) },
      { label: "Add to queue", disabled: locked, action: () => enqueue(picked, false) },
      {
        label: "Add to playlist…",
        disabled: locked || !canAddToPlaylist(picked, server),
        action: () => void addToPlaylistFlow(picked),
      },
      {
        label: many ? `Remove ${plural(indices.length, "song")} from playlist` : "Remove from playlist",
        disabled: !canEdit,
        action: () => void remove(indices),
      },
      { label: "Go to album", disabled: many || !track || !canGoToAlbum(track), action: () => goToAlbum(track) },
      { label: "Go to artist", disabled: many || !track || !canGoToArtist(track), action: () => goToArtist(track) },
    ];
  };

  const first = Math.max(0, Math.floor((view.top - view.offset) / ROW) - OVERSCAN);
  const last = Math.min(order.length, Math.ceil((view.top - view.offset + view.height) / ROW) + OVERSCAN);
  const dragging = drag.gap !== null ? new Set(drag.payload) : null;

  return (
    <div className={`track-table ${busy ? "is-saving" : ""}`}>
      <div className="track-head with-album" aria-hidden="true">
        <span className="track-no">#</span>
        <span>Title</span>
        <span>Album</span>
        <span className="track-duration">Time</span>
        <span />
      </div>
      <div
        ref={table}
        className={`track-rows ${dragging ? "is-dragging" : ""}`}
        role="listbox"
        aria-label={label}
        aria-multiselectable="true"
        aria-busy={busy || undefined}
        tabIndex={0}
        aria-activedescendant={selection.cursor !== null ? `ptrack-${selection.cursor}` : undefined}
        style={{ height: order.length * ROW }}
        onKeyDown={onKeyDown}
        onClickCapture={onClickCapture}
      >
        {keys.slice(first, last).map((key, i) => {
          const at = first + i;
          const track = tracks[Number(key)];
          if (!track) return null;
          return (
            <Row
              key={key}
              rowKey={key}
              at={at}
              track={track}
              playing={sameTrack(track, playing)}
              animated={animated}
              selected={selection.has(key)}
              cursor={selection.cursor === key}
              dragged={dragging?.has(key) ?? false}
              onClick={selection.click}
              onPointerDown={onRowPointerDown}
              onPlay={play}
              onMenu={(x, y) => {
                if (!selection.has(key)) selection.set([key]);
                setMenu({ x, y, key, at });
              }}
            />
          );
        })}
        {drag.gap !== null && <div className="drop-line" style={{ top: drag.gap * ROW }} aria-hidden="true" />}
      </div>
      {menu && (
        <ContextMenu
          state={menu}
          items={menuItems(menu.key, menu.at)}
          onClose={() => {
            setMenu(null);
            table.current?.focus();
          }}
        />
      )}
    </div>
  );
}

interface RowProps {
  rowKey: string;
  at: number;
  track: Track;
  playing: boolean;
  animated: boolean;
  selected: boolean;
  cursor: boolean;
  dragged: boolean;
  onClick: (key: string, e: { shiftKey: boolean; metaKey: boolean; ctrlKey: boolean }) => void;
  onPointerDown: (e: PointerEvent, key: string) => void;
  onPlay: (at: number) => void;
  onMenu: (x: number, y: number) => void;
}

const Row = memo(function Row({
  rowKey,
  at,
  track,
  playing,
  animated,
  selected,
  cursor,
  dragged,
  onClick,
  onPointerDown,
  onPlay,
  onMenu,
}: RowProps) {
  const classes = ["track-row", "with-album"];
  if (playing) classes.push("is-playing");
  if (selected) classes.push("is-selected");
  if (cursor) classes.push("is-cursor");
  if (dragged) classes.push("is-dragged");
  return (
    <div
      id={`ptrack-${rowKey}`}
      className={classes.join(" ")}
      style={{ top: at * ROW }}
      role="option"
      aria-selected={selected}
      onPointerDown={(e) => onPointerDown(e, rowKey)}
      onClick={(e) => onClick(rowKey, e)}
      onDoubleClick={() => onPlay(at)}
      onContextMenu={(e) => {
        e.preventDefault();
        onMenu(e.clientX, e.clientY);
      }}
    >
      <span className="track-no">{playing ? <Equalizer animated={animated} /> : at + 1}</span>
      <span className="track-text">
        <span className="track-title">{track.title}</span>
        <span className="track-artist secondary">{track.artist ?? "Unknown artist"}</span>
      </span>
      <span className="track-album secondary">{track.album ?? ""}</span>
      <span className="track-duration time secondary">{formatTime(track.durationMs)}</span>
      <button
        className="icon-button track-more"
        tabIndex={-1}
        aria-label={`More for ${track.title}`}
        title="More"
        onClick={(e) => {
          e.stopPropagation();
          const rect = e.currentTarget.getBoundingClientRect();
          onMenu(rect.left, rect.bottom);
        }}
        onDoubleClick={(e) => e.stopPropagation()}
      >
        <MoreIcon size={16} />
      </button>
    </div>
  );
});
