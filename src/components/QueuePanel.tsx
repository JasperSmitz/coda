// The queue: current entry highlighted, double-click or Enter plays an
// entry, Delete (or the hover button / context menu) removes it. Rows are
// virtualized (fixed 36px), since a queue holds up to 10,000 entries.

import { memo, useCallback, useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent } from "react";
import { intent } from "../lib/ipc";
import { formatTime } from "../lib/format";
import type { QueueItem, Status } from "../lib/types";
import { CloseIcon } from "./icons";
import { ContextMenu, type MenuState } from "./ContextMenu";

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
}

export function QueuePanel({ queue, currentKey, playingKey, status, disabled, locked }: Props) {
  const list = useRef<HTMLDivElement>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [height, setHeight] = useState(0);
  const [selected, setSelected] = useState<string | null>(null);
  const [menu, setMenu] = useState<MenuState | null>(null);

  useLayoutEffect(() => {
    const el = list.current;
    if (!el) return;
    const observer = new ResizeObserver(() => setHeight(el.clientHeight));
    observer.observe(el);
    setHeight(el.clientHeight);
    return () => observer.disconnect();
  }, []);

  const indexOf = useCallback((key: string | null) => queue.findIndex((i) => i.key === key), [queue]);

  // A removed selection moves to the entry now in its place.
  const selectedIndex = indexOf(selected);
  useEffect(() => {
    if (selected !== null && selectedIndex < 0) setSelected(null);
  }, [selected, selectedIndex]);

  const scrollToIndex = (index: number) => {
    const el = list.current;
    if (!el) return;
    const top = index * ROW;
    if (top < el.scrollTop) el.scrollTop = top;
    else if (top + ROW > el.scrollTop + el.clientHeight) el.scrollTop = top + ROW - el.clientHeight;
  };

  // Bring the current entry into view when it changes.
  useEffect(() => {
    const index = indexOf(currentKey);
    if (index >= 0) scrollToIndex(index);
  }, [currentKey, indexOf]);

  const play = (key: string) => intent({ type: "playEntry", key });
  const remove = (key: string) => {
    if (locked) return;
    const index = indexOf(key);
    intent({ type: "removeEntry", key });
    if (key === selected) {
      const neighbour = queue[index + 1] ?? queue[index - 1];
      setSelected(neighbour ? neighbour.key : null);
    }
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (queue.length === 0) return;
    const index = selectedIndex >= 0 ? selectedIndex : Math.max(0, indexOf(currentKey));
    const move = (to: number) => {
      const clamped = Math.min(queue.length - 1, Math.max(0, to));
      setSelected(queue[clamped].key);
      scrollToIndex(clamped);
    };
    const page = Math.max(1, Math.floor(height / ROW) - 1);
    switch (e.key) {
      case "ArrowDown":
        move(selectedIndex < 0 ? index : index + 1);
        break;
      case "ArrowUp":
        move(selectedIndex < 0 ? index : index - 1);
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
      case "Enter":
        if (selectedIndex >= 0 && !disabled) play(queue[selectedIndex].key);
        break;
      case "Delete":
      // The ⌫ key on Mac keyboards (Delete needs fn+⌫).
      case "Backspace":
        if (selectedIndex >= 0) remove(queue[selectedIndex].key);
        break;
      default:
        return;
    }
    e.preventDefault();
    e.stopPropagation();
  };

  const first = Math.max(0, Math.floor(scrollTop / ROW) - OVERSCAN);
  const last = Math.min(queue.length, Math.ceil((scrollTop + height) / ROW) + OVERSCAN);
  const rows = queue.slice(first, last);
  const currentIndex = indexOf(currentKey);
  const total = queue.reduce((sum, i) => sum + (i.track.durationMs ?? 0), 0);

  return (
    <aside className="queue" aria-label="Queue">
      <header className="panel-header">
        <div>
          <h2>Queue</h2>
          <div className="secondary">
            {queue.length === 0
              ? "Empty"
              : `${queue.length.toLocaleString()} ${queue.length === 1 ? "track" : "tracks"} · ${formatTime(total)}`}
          </div>
        </div>
        <button
          className="text-button"
          onClick={() => intent({ type: "clearQueue" })}
          disabled={locked || queue.length === 0}
          title="Clear the queue"
        >
          Clear
        </button>
      </header>
      <div
        className="queue-list"
        ref={list}
        tabIndex={0}
        role="listbox"
        aria-label="Queue entries"
        aria-activedescendant={selected ? `queue-${selected}` : undefined}
        onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
        onKeyDown={onKeyDown}
      >
        {queue.length === 0 ? (
          <div className="queue-empty secondary">
            The queue is empty. Browse your library to add music.
          </div>
        ) : (
          <div style={{ height: queue.length * ROW, position: "relative" }}>
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
                  selected={item.key === selected}
                  onSelect={setSelected}
                  onPlay={disabled ? undefined : play}
                  onRemove={locked ? undefined : remove}
                  onMenu={(x, y) => {
                    setSelected(item.key);
                    setMenu({ x, y, key: item.key });
                  }}
                />
              );
            })}
          </div>
        )}
      </div>
      {menu && (
        <ContextMenu
          state={menu}
          onClose={() => setMenu(null)}
          items={[
            { label: "Play", disabled, action: () => play(menu.key) },
            { label: "Remove from queue", disabled: locked, action: () => remove(menu.key) },
          ]}
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
  onSelect: (key: string) => void;
  onPlay?: (key: string) => void;
  onRemove?: (key: string) => void;
  onMenu: (x: number, y: number) => void;
}

const QueueRow = memo(function QueueRow({
  item,
  index,
  top,
  current,
  playing,
  played,
  selected,
  onSelect,
  onPlay,
  onRemove,
  onMenu,
}: RowProps) {
  const { track } = item;
  const classes = ["queue-row"];
  if (current) classes.push("is-current");
  if (selected) classes.push("is-selected");
  if (played) classes.push("is-played");
  return (
    <div
      id={`queue-${item.key}`}
      className={classes.join(" ")}
      style={{ top }}
      role="option"
      aria-selected={selected}
      aria-current={current || undefined}
      onClick={() => onSelect(item.key)}
      onDoubleClick={() => onPlay?.(item.key)}
      onContextMenu={(e) => {
        e.preventDefault();
        onMenu(e.clientX, e.clientY);
      }}
    >
      <span className="queue-index">
        {current ? <Equalizer animated={playing} /> : index + 1}
      </span>
      <span className="queue-text">
        <span className="queue-title">{track.title}</span>
        <span className="queue-artist secondary">{track.artist ?? "Unknown artist"}</span>
      </span>
      <span className="queue-duration time secondary">{formatTime(track.durationMs)}</span>
      {onRemove && (
      <button
        className="icon-button queue-remove"
        tabIndex={-1}
        aria-label={`Remove ${track.title}`}
        title="Remove (Delete)"
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
