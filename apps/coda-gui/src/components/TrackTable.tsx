// A track table (DESIGN.md): #, title (artist below when mixed), album,
// duration; the playing row shows the equaliser. Rows are virtualized
// (36px) inside the view's scroller, with disc headers when an album has
// several discs. ↑/↓ select, Enter or double-click plays the list from
// that track, right-click or "…" opens the track menu.

import {
  memo,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type RefObject,
} from "react";
import { formatTime } from "../lib/format";
import { playTracks, sameTrack } from "../lib/play";
import type { PlayContext, Track } from "../lib/types";
import { useAppState } from "../state/store";
import type { MenuState } from "./ContextMenu";
import { MoreIcon } from "./icons";
import { TrackAlbum, TrackArtist } from "./NameLink";
import { Equalizer } from "./QueuePanel";
import { TrackMenu } from "./TrackMenu";

const ROW = 36;
const OVERSCAN = 10;

type Row = { kind: "disc"; disc: number } | { kind: "track"; index: number };

/** A track's identity in a list (the same track twice shares it). */
function trackKey(track: Track | undefined): string | null {
  if (!track) return null;
  return track.source.kind === "local"
    ? `l:${track.source.id ?? track.title}`
    : `n:${track.source.songId}`;
}

/** A position held by track identity, so it survives a reload of the list. */
interface Anchor {
  index: number;
  track: string | null;
}

/**
 * Where `anchor` is in `tracks` now: the same index if it still holds the
 * track, else the nearest index that does, else `null`.
 */
function resolve(anchor: Anchor | null, tracks: Track[]): number | null {
  if (!anchor) return null;
  if (trackKey(tracks[anchor.index]) === anchor.track) return anchor.index;
  let best: number | null = null;
  tracks.forEach((t, i) => {
    if (
      trackKey(t) === anchor.track &&
      (best === null || Math.abs(i - anchor.index) < Math.abs(best - anchor.index))
    ) {
      best = i;
    }
  });
  return best;
}

interface Props {
  tracks: Track[];
  /** The view's scrolling element (positioned), for virtualization. */
  scroller: RefObject<HTMLElement | null>;
  /** Show the album column. */
  showAlbum?: boolean;
  /** Show the artist under each title (mixed artists). */
  showArtist?: boolean;
  /** Group by disc when there is more than one. */
  discs?: boolean;
  /** Plays the list from `index` (default: `playTracks(tracks, index)`). */
  onPlay?: (index: number) => void;
  /** What the list is (album, playlist, folder), for the play history. */
  context?: PlayContext | null;
  label: string;
}

function rowsFor(tracks: Track[], discs: boolean): Row[] {
  const numbers = new Set(tracks.map((t) => t.discNo ?? 1));
  if (!discs || numbers.size < 2) return tracks.map((_, index) => ({ kind: "track", index }));
  const rows: Row[] = [];
  let disc: number | null = null;
  tracks.forEach((t, index) => {
    const d = t.discNo ?? 1;
    if (d !== disc) {
      disc = d;
      rows.push({ kind: "disc", disc: d });
    }
    rows.push({ kind: "track", index });
  });
  return rows;
}

export function TrackTable({
  tracks,
  scroller,
  showAlbum = false,
  showArtist = false,
  discs = false,
  onPlay,
  context,
  label,
}: Props) {
  const { snapshot } = useAppState();
  // `locked`: no queue edits (starting or quitting); `disabled`: no playback either.
  const locked = snapshot?.phase !== "ready";
  const disabled = locked || snapshot.status === "unavailable";
  const playing = snapshot?.playing ?? null;
  const animated = snapshot?.status === "playing";
  const table = useRef<HTMLDivElement>(null);
  const [view, setView] = useState({ top: 0, height: 800, offset: 0 });
  // Selection and the open menu follow their track when the list reloads
  // (a scan inserts tracks while a view refreshes quietly).
  const [selection, setSelection] = useState<Anchor | null>(null);
  const [menu, setMenu] = useState<(MenuState & Anchor) | null>(null);
  const selected = resolve(selection, tracks);
  const menuIndex = resolve(menu, tracks);
  const setSelected = (index: number | null) =>
    setSelection(index === null ? null : { index, track: trackKey(tracks[index]) });
  const openMenu = (index: number, x: number, y: number) =>
    setMenu({ x, y, key: String(index), index, track: trackKey(tracks[index]) });
  // DOM ids unique per table (Search shows several).
  const idPrefix = useId();
  const rows = rowsFor(tracks, discs);

  // Follow the scroller to render only the rows in view.
  useLayoutEffect(() => {
    const el = scroller.current;
    const own = table.current;
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

  // Content above the table may load later (another search group).
  useLayoutEffect(() => {
    const offset = table.current?.offsetTop ?? 0;
    if (offset !== view.offset) setView((v) => ({ ...v, offset }));
  });

  // The menu's track left the list: close it.
  useEffect(() => {
    if (menu && menuIndex === null) setMenu(null);
  }, [menu, menuIndex]);

  const play = (index: number) => {
    if (disabled) return;
    if (onPlay) onPlay(index);
    else playTracks(tracks, index, false, context);
  };

  const rowOf = (index: number) => rows.findIndex((r) => r.kind === "track" && r.index === index);

  const reveal = (index: number) => {
    const el = scroller.current;
    const own = table.current;
    if (!el || !own) return;
    const top = own.offsetTop + rowOf(index) * ROW;
    if (top < el.scrollTop) el.scrollTop = top;
    else if (top + ROW > el.scrollTop + el.clientHeight) el.scrollTop = top + ROW - el.clientHeight;
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (tracks.length === 0) return;
    const at = selected ?? -1;
    const page = Math.max(1, Math.floor(view.height / ROW) - 1);
    const move = (to: number) => {
      const index = Math.min(tracks.length - 1, Math.max(0, to));
      setSelected(index);
      reveal(index);
    };
    switch (e.key) {
      case "ArrowDown":
        move(at + 1);
        break;
      case "ArrowUp":
        move(at < 0 ? 0 : at - 1);
        break;
      case "PageDown":
        move(at + page);
        break;
      case "PageUp":
        move(at - page);
        break;
      case "Home":
        move(0);
        break;
      case "End":
        move(tracks.length - 1);
        break;
      case "Enter":
        play(Math.max(0, at));
        break;
      case "ContextMenu": {
        const index = Math.max(0, at);
        const rect = table.current?.getBoundingClientRect();
        const y = (rect?.top ?? 0) + rowOf(index) * ROW + ROW - view.top + view.offset;
        openMenu(index, (rect?.left ?? 0) + 40, y);
        break;
      }
      default:
        return;
    }
    e.preventDefault();
    e.stopPropagation();
  };

  const start = Math.max(0, Math.floor((view.top - view.offset) / ROW) - OVERSCAN);
  const end = Math.min(
    rows.length,
    Math.ceil((view.top - view.offset + view.height) / ROW) + OVERSCAN,
  );

  return (
    <div className="track-table">
      <div className={`track-head ${showAlbum ? "with-album" : ""}`} aria-hidden="true">
        <span className="track-no">#</span>
        <span>Title</span>
        {showAlbum && <span>Album</span>}
        <span className="track-duration">Time</span>
        <span />
      </div>
      <div
        ref={table}
        className="track-rows"
        role="listbox"
        aria-label={label}
        tabIndex={0}
        aria-activedescendant={selected !== null ? `${idPrefix}-${selected}` : undefined}
        style={{ height: rows.length * ROW }}
        onKeyDown={onKeyDown}
      >
        {rows.slice(start, end).map((row, i) => {
          const top = (start + i) * ROW;
          if (row.kind === "disc") {
            return (
              <div key={`disc-${row.disc}`} className="disc-row" style={{ top }}>
                Disc {row.disc}
              </div>
            );
          }
          const track = tracks[row.index];
          return (
            <TrackRow
              key={row.index}
              id={`${idPrefix}-${row.index}`}
              index={row.index}
              track={track}
              top={top}
              showAlbum={showAlbum}
              showArtist={showArtist}
              playing={sameTrack(track, playing)}
              animated={animated}
              selected={selected === row.index}
              onSelect={setSelected}
              onPlay={play}
              onMenu={(index, x, y) => {
                setSelected(index);
                openMenu(index, x, y);
              }}
            />
          );
        })}
      </div>
      {menu && menuIndex !== null && (
        <TrackMenu
          state={menu}
          track={tracks[menuIndex]}
          disabled={disabled}
          locked={locked}
          onPlay={() => play(menuIndex)}
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
  id: string;
  index: number;
  track: Track;
  top: number;
  showAlbum: boolean;
  showArtist: boolean;
  playing: boolean;
  animated: boolean;
  selected: boolean;
  onSelect: (index: number) => void;
  onPlay: (index: number) => void;
  onMenu: (index: number, x: number, y: number) => void;
}

const TrackRow = memo(function TrackRow({
  id,
  index,
  track,
  top,
  showAlbum,
  showArtist,
  playing,
  animated,
  selected,
  onSelect,
  onPlay,
  onMenu,
}: RowProps) {
  const classes = ["track-row"];
  if (showAlbum) classes.push("with-album");
  if (playing) classes.push("is-playing");
  if (selected) classes.push("is-selected");
  return (
    <div
      id={id}
      className={classes.join(" ")}
      style={{ top }}
      role="option"
      aria-selected={selected}
      onClick={() => onSelect(index)}
      onDoubleClick={() => onPlay(index)}
      onContextMenu={(e) => {
        e.preventDefault();
        onMenu(index, e.clientX, e.clientY);
      }}
    >
      <span className="track-no">
        {playing ? (
          <Equalizer animated={animated} />
        ) : showAlbum ? (
          index + 1
        ) : (
          (track.trackNo ?? index + 1)
        )}
      </span>
      <span className="track-text">
        <span className="track-title">{track.title}</span>
        {showArtist && <TrackArtist track={track} className="track-artist secondary" />}
      </span>
      {showAlbum && <TrackAlbum track={track} className="track-album secondary" />}
      <span className="track-duration time secondary">{formatTime(track.durationMs)}</span>
      <button
        className="icon-button track-more"
        tabIndex={-1}
        aria-label={`More for ${track.title}`}
        title="More"
        onClick={(e) => {
          e.stopPropagation();
          const rect = e.currentTarget.getBoundingClientRect();
          onMenu(index, rect.left, rect.bottom);
        }}
        onDoubleClick={(e) => e.stopPropagation()}
      >
        <MoreIcon size={16} />
      </button>
    </div>
  );
});
