// The queue: current entry highlighted, double-click or Enter plays an
// entry. Multi-select (click, Shift/Cmd/Ctrl-click, Shift+↑/↓, Cmd/Ctrl+A);
// Delete or Cmd+Backspace removes the selection (plain Backspace is Back), Alt+↑/↓ moves it, drag-and-drop moves it
// to the drop gap. The header shuffles the upcoming entries, saves the
// queue as a Navidrome playlist and clears it (after a confirmation, as
// in Coda). Rows are virtualized (fixed 36px), since a queue holds up to
// 10,000 entries.

import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type PointerEvent } from "react";
import { useDragReorder } from "../hooks/useDragReorder";
import { useSelection } from "../hooks/useSelection";
import { formatTime } from "../lib/format";
import { intent } from "../lib/ipc";
import { isRemoveKey } from "../lib/keys";
import { addToPlaylistFlow, canAddToPlaylist, saveQueueFlow, songIds } from "../lib/playlists";
import type { QueueItem, Status } from "../lib/types";
import { confirm } from "../state/dialogs";
import { BroomIcon, CloseIcon, PlaylistAddIcon, ShuffleIcon } from "./icons";
import { ContextMenu, type MenuItem, type MenuState } from "./ContextMenu";
import { TrackArtist } from "./NameLink";

const ROW = 36;
const OVERSCAN = 8;

interface Props {
  queue: QueueItem[];
  currentKey: string | null;
  playingKey: string | null;
  status: Status;
  /** Playing is not possible (starting, or playback unavailable). */
  disabled: boolean;
  /** Editing is not possible yet (still starting, or quitting). */
  locked: boolean;
  /** A Navidrome server is set up (Save as playlist, Add to playlist). */
  serverConfigured: boolean;
}

const plural = (n: number, noun: string) =>
  `${n.toLocaleString()} ${n === 1 ? noun : `${noun}s`}`;

export function QueuePanel({
  queue,
  currentKey,
  playingKey,
  status,
  disabled,
  locked,
  serverConfigured,
}: Props) {
  const list = useRef<HTMLDivElement>(null);
  const rowsEl = useRef<HTMLDivElement>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [height, setHeight] = useState(0);
  const [menu, setMenu] = useState<MenuState | null>(null);
  const keys = useMemo(() => queue.map((i) => i.key), [queue]);
  const indexByKey = useMemo(() => new Map(keys.map((k, i) => [k, i])), [keys]);
  const selection = useSelection(keys);

  useLayoutEffect(() => {
    const el = list.current;
    if (!el) return;
    const observer = new ResizeObserver(() => setHeight(el.clientHeight));
    observer.observe(el);
    setHeight(el.clientHeight);
    return () => observer.disconnect();
  }, []);

  const indexOf = useCallback((key: string | null) => (key === null ? -1 : (indexByKey.get(key) ?? -1)), [indexByKey]);

  const scrollToIndex = (index: number) => {
    const el = list.current;
    const rows = rowsEl.current;
    if (!el) return;
    const top = (rows?.offsetTop ?? 0) + index * ROW;
    if (top < el.scrollTop) el.scrollTop = top;
    else if (top + ROW > el.scrollTop + el.clientHeight) el.scrollTop = top + ROW - el.clientHeight;
  };

  // Bring the current entry into view when it changes.
  useEffect(() => {
    const index = indexOf(currentKey);
    if (index >= 0) scrollToIndex(index);
  }, [currentKey, indexOf]);

  const play = (key: string) => intent({ type: "playEntry", key });

  /** The keys an action applies to: the selection, or `key` if it is outside it. */
  const targetKeys = (key: string | null): string[] => {
    if (key !== null && !selection.has(key)) return [key];
    return selection.ordered();
  };

  const remove = (target: string[]) => {
    if (locked || target.length === 0) return;
    // The selection moves to the entry after the last removed one.
    const last = Math.max(...target.map(indexOf));
    const gone = new Set(target);
    const neighbour =
      queue.slice(last + 1).find((i) => !gone.has(i.key)) ??
      queue
        .slice(0, last)
        .reverse()
        .find((i) => !gone.has(i.key));
    intent({ type: "removeEntries", keys: target });
    selection.set(neighbour ? [neighbour.key] : []);
  };

  const playNext = (target: string[]) => {
    if (!locked && target.length > 0) intent({ type: "playNextEntries", keys: target });
  };

  const moveSelection = (delta: -1 | 1) => {
    const target = selection.ordered();
    if (locked || target.length === 0) return;
    const positions = target.map(indexOf);
    const first = Math.min(...positions);
    const last = Math.max(...positions);
    const to = delta < 0 ? Math.max(0, first - 1) : Math.min(queue.length, last + 2);
    intent({ type: "moveEntries", keys: target, before: queue[to]?.key ?? null });
    scrollToIndex(delta < 0 ? Math.max(0, first - 1) : Math.min(queue.length - 1, last + 1));
  };

  const { drag, start, onClickCapture } = useDragReorder({
    scroller: list,
    rows: rowsEl,
    rowHeight: ROW,
    count: queue.length,
    disabled: locked,
    // The gap as the entry it precedes, so a queue that changed since
    // this render still gets the block where it was dropped.
    onDrop: (gap, payload) =>
      intent({ type: "moveEntries", keys: payload, before: queue[gap]?.key ?? null }),
  });

  const onRowPointerDown = (e: PointerEvent, key: string) => {
    if (e.button !== 0) return;
    const modified = e.shiftKey || e.metaKey || e.ctrlKey;
    // Dragging an unselected row drags just that row.
    start(e, () => (selection.has(key) ? selection.ordered() : [key]));
    if (!modified && !selection.has(key)) selection.click(key, e);
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (queue.length === 0) return;
    const cursor = indexOf(selection.cursor);
    const index = cursor >= 0 ? cursor : Math.max(0, indexOf(currentKey));
    const fresh = cursor < 0;
    const move = (to: number, extend = e.shiftKey) => {
      const clamped = Math.min(queue.length - 1, Math.max(0, to));
      selection.moveTo(clamped, extend);
      scrollToIndex(clamped);
    };
    const page = Math.max(1, Math.floor(height / ROW) - 1);
    const mod = e.metaKey || e.ctrlKey;
    switch (e.key) {
      case "ArrowDown":
        if (e.altKey) moveSelection(1);
        else move(fresh ? index : index + 1);
        break;
      case "ArrowUp":
        if (e.altKey) moveSelection(-1);
        else move(fresh ? index : index - 1);
        break;
      case "PageDown":
        move(index + page);
        break;
      case "PageUp":
        move(index - page);
        break;
      case "Home":
        move(0);
        break;
      case "End":
        move(queue.length - 1);
        break;
      case "a":
      case "A":
        if (!mod) return;
        selection.selectAll();
        break;
      case "Enter":
        if (cursor >= 0 && !disabled) play(queue[cursor].key);
        break;
      case "Delete":
      case "Backspace":
        // Plain Backspace is Back (DESIGN.md); removing needs Delete
        // (fn+⌫ on Mac keyboards) or Cmd+Backspace.
        if (!isRemoveKey(e)) return;
        remove(selection.ordered());
        break;
      case "ContextMenu": {
        if (cursor < 0) return;
        const rect = list.current?.getBoundingClientRect();
        const y = (rect?.top ?? 0) + (rowsEl.current?.offsetTop ?? 0) + (cursor + 1) * ROW - scrollTop;
        setMenu({ x: (rect?.left ?? 0) + 40, y, key: queue[cursor].key });
        break;
      }
      default:
        return;
    }
    e.preventDefault();
    e.stopPropagation();
  };

  const clear = async () => {
    if (locked || queue.length === 0) return;
    const ok = await confirm({
      title: "Clear the queue?",
      message: `All ${plural(queue.length, "track")} are removed and playback stops.`,
      confirmLabel: "Clear",
      danger: true,
    });
    if (ok) intent({ type: "clearQueue" });
  };

  const remote = useMemo(() => songIds(queue.map((i) => i.track)).length, [queue]);
  const save = () => {
    if (!locked && remote > 0) void saveQueueFlow(remote, queue.length - remote);
  };

  const first = Math.max(0, Math.floor(scrollTop / ROW) - OVERSCAN);
  const last = Math.min(queue.length, Math.ceil((scrollTop + height) / ROW) + OVERSCAN);
  const rows = queue.slice(first, last);
  const currentIndex = indexOf(currentKey);
  const total = queue.reduce((sum, i) => sum + (i.track.durationMs ?? 0), 0);
  const dragging = drag.gap !== null ? new Set(drag.payload) : null;
  const selectedCount = selection.keys.size;

  const menuItems = (key: string): MenuItem[] => {
    const target = targetKeys(key);
    const many = target.length > 1;
    const targetTracks = target.flatMap((k) => {
      const i = indexOf(k);
      return i >= 0 ? [queue[i].track] : [];
    });
    return [
      { label: "Play", disabled: disabled || many, action: () => play(key) },
      {
        label: many ? `Play ${plural(target.length, "track")} next` : "Play next",
        disabled: locked,
        action: () => playNext(target),
      },
      {
        label: many ? "Add selection to playlist…" : "Add to playlist…",
        disabled: locked || !canAddToPlaylist(targetTracks, serverConfigured),
        action: () => void addToPlaylistFlow(targetTracks),
      },
      {
        label: many ? `Remove ${plural(target.length, "track")}` : "Remove from queue",
        disabled: locked,
        action: () => remove(target),
      },
    ];
  };

  return (
    <aside className="queue" aria-label="Queue">
      <header className="panel-header">
        <div>
          <h2>Queue</h2>
          <div className="secondary">
            {queue.length === 0
              ? "Empty"
              : selectedCount > 1
                ? `${selectedCount.toLocaleString()} of ${plural(queue.length, "track")} selected`
                : `${plural(queue.length, "track")} · ${formatTime(total)}`}
          </div>
        </div>
        <div className="panel-actions">
          <button
            className="icon-button"
            onClick={() => intent({ type: "shuffleUpcoming" })}
            disabled={locked || queue.length < 2}
            aria-label="Shuffle upcoming"
            title="Shuffle the upcoming tracks"
          >
            <ShuffleIcon />
          </button>
          {serverConfigured && (
            <button
              className="icon-button"
              onClick={save}
              disabled={locked || remote === 0}
              aria-label="Save queue as playlist…"
              title={
                remote === 0
                  ? "Save queue as playlist: the queue has no Navidrome tracks"
                  : "Save the queue's Navidrome tracks as a playlist…"
              }
            >
              <PlaylistAddIcon />
            </button>
          )}
          <button
            className="icon-button"
            onClick={() => void clear()}
            disabled={locked || queue.length === 0}
            aria-label="Clear queue…"
            title="Clear the queue…"
          >
            <BroomIcon />
          </button>
        </div>
      </header>
      <div
        className={`queue-list ${dragging ? "is-dragging" : ""}`}
        ref={list}
        tabIndex={0}
        role="listbox"
        aria-label="Queue entries"
        aria-multiselectable="true"
        aria-activedescendant={selection.cursor ? `queue-${selection.cursor}` : undefined}
        onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
        onKeyDown={onKeyDown}
      >
        {queue.length === 0 ? (
          <div className="queue-empty secondary">
            The queue is empty. Browse your library to add music.
          </div>
        ) : (
          <div
            ref={rowsEl}
            style={{ height: queue.length * ROW, position: "relative" }}
            onClickCapture={onClickCapture}
          >
            {rows.map((item, i) => {
              const index = first + i;
              return (
                <QueueRow
                  key={item.key}
                  item={item}
                  index={index}
                  top={index * ROW}
                  current={item.key === currentKey}
                  playing={item.key === playingKey && status === "playing"}
                  played={currentIndex >= 0 && index < currentIndex}
                  selected={selection.has(item.key)}
                  cursor={item.key === selection.cursor}
                  dragged={dragging?.has(item.key) ?? false}
                  onClick={selection.click}
                  onPointerDown={onRowPointerDown}
                  onPlay={disabled ? undefined : play}
                  onRemove={locked ? undefined : (key) => remove([key])}
                  onMenu={(key, x, y) => {
                    if (!selection.has(key)) selection.set([key]);
                    setMenu({ x, y, key });
                  }}
                />
              );
            })}
            {drag.gap !== null && (
              <div className="drop-line" style={{ top: drag.gap * ROW }} aria-hidden="true" />
            )}
          </div>
        )}
      </div>
      {menu && (
        <ContextMenu
          state={menu}
          onClose={() => {
            setMenu(null);
            list.current?.focus();
          }}
          items={menuItems(menu.key)}
        />
      )}
    </aside>
  );
}

interface RowProps {
  item: QueueItem;
  index: number;
  top: number;
  current: boolean;
  playing: boolean;
  played: boolean;
  selected: boolean;
  cursor: boolean;
  dragged: boolean;
  onClick: (key: string, e: { shiftKey: boolean; metaKey: boolean; ctrlKey: boolean }) => void;
  onPointerDown: (e: PointerEvent, key: string) => void;
  onPlay?: (key: string) => void;
  onRemove?: (key: string) => void;
  onMenu: (key: string, x: number, y: number) => void;
}

const QueueRow = memo(function QueueRow({
  item,
  index,
  top,
  current,
  playing,
  played,
  selected,
  cursor,
  dragged,
  onClick,
  onPointerDown,
  onPlay,
  onRemove,
  onMenu,
}: RowProps) {
  const { track } = item;
  const classes = ["queue-row"];
  if (current) classes.push("is-current");
  if (selected) classes.push("is-selected");
  if (cursor) classes.push("is-cursor");
  if (played) classes.push("is-played");
  if (dragged) classes.push("is-dragged");
  return (
    <div
      id={`queue-${item.key}`}
      className={classes.join(" ")}
      style={{ top }}
      role="option"
      aria-selected={selected}
      aria-current={current || undefined}
      onPointerDown={(e) => onPointerDown(e, item.key)}
      onClick={(e) => onClick(item.key, e)}
      onDoubleClick={() => onPlay?.(item.key)}
      onContextMenu={(e) => {
        e.preventDefault();
        onMenu(item.key, e.clientX, e.clientY);
      }}
    >
      <span className="queue-index">
        {current ? <Equalizer animated={playing} /> : index + 1}
      </span>
      <span className="queue-text">
        <span className="queue-title">{track.title}</span>
        <TrackArtist track={track} className="queue-artist secondary" />
      </span>
      <span className="queue-duration time secondary">{formatTime(track.durationMs)}</span>
      {onRemove && (
        <button
          className="icon-button queue-remove"
          tabIndex={-1}
          aria-label={`Remove ${track.title}`}
          title="Remove (Delete or ⌘⌫)"
          onClick={(e) => {
            e.stopPropagation();
            onRemove(item.key);
          }}
          onDoubleClick={(e) => e.stopPropagation()}
        >
          <CloseIcon size={14} />
        </button>
      )}
    </div>
  );
});

export function Equalizer({ animated }: { animated: boolean }) {
  return (
    <span className={`equalizer ${animated ? "is-animated" : ""}`} aria-label="Now playing">
      <i />
      <i />
      <i />
    </span>
  );
}
