// Where you left off (DESIGN.md "Home"): the recent contexts as a
// carousel. The focused cover is large and centred, its neighbours
// smaller and dimmed (lib/home.ts `slot`); they glide by transforms only
// (`--motion-carousel`, none with reduced motion). ←/→ (and Home/End)
// move the focus only while the carousel has the keyboard focus, and
// then never seek; anywhere else they seek as usual. A click on a cover
// or the ‹ › buttons gives the (new) focused cover the keyboard focus
// (macOS buttons take none on a click by themselves), so the arrows go
// on from there. A horizontal swipe (wheel `deltaX`, throttled) moves it
// too; a click on the focused cover opens it. One Tab stop: the focused
// cover.

import { useRef, type KeyboardEvent, type WheelEvent } from "react";
import { useFadeOnChange } from "../hooks/useFadeOnChange";
import { usePendingFocus } from "../hooks/usePendingFocus";
import { quietLine, slot } from "../lib/home";
import { morphKey } from "../lib/pageTransition";
import type { HomeItem } from "../lib/types";
import { Cover } from "./Cover";
import { BackIcon, ForwardIcon, PlayIcon } from "./icons";
import { NameLink } from "./NameLink";

/** A swipe moves one place at most this often. */
const SWIPE_MS = 450;
/** Horizontal wheel travel that makes a swipe. */
const SWIPE_PX = 40;
/** Wheel travel older than this (no wheel event since) is forgotten. */
const SWIPE_IDLE_MS = 180;

const OPEN_LABEL: Record<HomeItem["kind"], string> = {
  album: "Open album",
  artist: "Open artist",
  playlist: "Open playlist",
  folder: "Open folder",
  mix: "Open mix",
};

export function itemKey(item: HomeItem): string {
  return `${item.kind}:${item.id}`;
}

export function HomeCarousel({
  items,
  focus,
  onFocus,
  playing,
  canPlay,
  now,
  onOpen,
  onResume,
  onNowPlaying,
  onArtist,
}: {
  items: HomeItem[];
  focus: number;
  /** Move the carousel's focus to `index`. */
  onFocus: (index: number) => void;
  /** The key of the item whose context is playing. */
  playing: string | null;
  canPlay: boolean;
  now: Date;
  onOpen: (item: HomeItem) => void;
  onResume: (item: HomeItem) => void;
  onNowPlaying: () => void;
  onArtist: (item: HomeItem) => void;
}) {
  const track = useRef<HTMLDivElement>(null);
  const meta = useRef<HTMLDivElement>(null);
  const wheel = useRef({ dx: 0, last: 0, seen: 0 });
  const focusCover = usePendingFocus(track);
  const item = items[focus];
  const key = item ? itemKey(item) : null;
  useFadeOnChange(key, () => [meta.current], {
    duration: "--motion-slow",
    from: { opacity: 0, transform: "translateY(8px)" },
  });

  /** Moves the focus to `to`; `keyboard`: the cover takes the keyboard focus too. */
  const move = (to: number, keyboard: boolean) => {
    const next = Math.min(items.length - 1, Math.max(0, to));
    if (next === focus) return;
    onFocus(next);
    if (keyboard) focusCover(next);
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.altKey || e.ctrlKey || e.metaKey) return;
    let to: number;
    switch (e.key) {
      case "ArrowLeft":
        to = focus - 1;
        break;
      case "ArrowRight":
        to = focus + 1;
        break;
      case "Home":
        to = 0;
        break;
      case "End":
        to = items.length - 1;
        break;
      default:
        return;
    }
    // The carousel has the arrows: not the global seek.
    e.preventDefault();
    e.stopPropagation();
    move(to, true);
  };

  const onWheel = (e: WheelEvent) => {
    if (Math.abs(e.deltaX) <= Math.abs(e.deltaY)) return;
    const state = wheel.current;
    const t = e.timeStamp;
    // A pause in the travel: a new gesture starts from nothing.
    if (t - state.seen > SWIPE_IDLE_MS) state.dx = 0;
    state.seen = t;
    if (t - state.last < SWIPE_MS) return;
    state.dx += e.deltaX;
    if (Math.abs(state.dx) < SWIPE_PX) return;
    move(focus + Math.sign(state.dx), false);
    state.dx = 0;
    state.last = t;
  };

  const isPlaying = key !== null && key === playing;
  const openLabel = item ? OPEN_LABEL[item.kind] : "";
  return (
    <div className="carousel" onWheel={onWheel}>
      <div
        ref={track}
        className="carousel-track"
        role="group"
        aria-roledescription="carousel"
        aria-label="Where you left off"
        onKeyDown={onKeyDown}
      >
        {items.map((it, i) => {
          const s = slot(i - focus);
          const focused = i === focus;
          const hidden = s.opacity === 0;
          return (
            <button
              key={itemKey(it)}
              className={`carousel-item ${focused ? "is-focused" : ""} ${it.kind === "artist" ? "round" : ""}`}
              data-index={i}
              tabIndex={focused ? 0 : -1}
              aria-hidden={hidden || undefined}
              aria-label={focused ? `${openLabel}: ${it.name}` : `Show ${it.name}`}
              title={focused ? `${openLabel} (Enter)` : it.name}
              style={{
                transform: `translateX(calc(-50% + ${s.x} * var(--carousel-size))) scale(${s.scale})`,
                opacity: s.opacity,
                zIndex: 10 - Math.abs(i - focus),
                pointerEvents: hidden ? "none" : undefined,
              }}
              onClick={(e) => {
                if (focused) {
                  onOpen(it);
                } else {
                  move(i, true);
                  e.currentTarget.focus({ preventScroll: true });
                }
              }}
            >
              <Cover
                url={it.coverUrl}
                name={it.name}
                size="large"
                className="carousel-cover"
                morph={itemMorph(it)}
              />
            </button>
          );
        })}
        <button
          className="icon-button carousel-step prev"
          tabIndex={-1}
          aria-label="Previous"
          disabled={focus <= 0}
          onClick={() => move(focus - 1, true)}
        >
          <BackIcon size={18} />
        </button>
        <button
          className="icon-button carousel-step next"
          tabIndex={-1}
          aria-label="Next"
          disabled={focus >= items.length - 1}
          onClick={() => move(focus + 1, true)}
        >
          <ForwardIcon size={18} />
        </button>
      </div>
      <div className="carousel-meta" ref={meta}>
        {item && (
          <>
            <h2 className="carousel-title" title={item.name}>
              {item.name}
            </h2>
            <div className="carousel-artist">
              {item.subtitle && item.kind === "album" && item.artistId ? (
                <NameLink label={`Go to artist ${item.subtitle}`} onGo={() => onArtist(item)}>
                  {item.subtitle}
                </NameLink>
              ) : (
                (item.subtitle ?? KIND_SUBTITLE[item.kind])
              )}
            </div>
            <div className="carousel-quiet">{quietLine(item, isPlaying, now)}</div>
          </>
        )}
        <div className="carousel-dots" aria-hidden="true">
          {items.map((it, i) => (
            <span key={itemKey(it)} className={`carousel-dot ${i === focus ? "is-current" : ""}`} />
          ))}
        </div>
        {item && (
          <div className="carousel-actions">
            {isPlaying ? (
              <button className="button button-primary" onClick={onNowPlaying}>
                <PlayIcon size={14} /> Now Playing
              </button>
            ) : (
              <button
                className="button button-primary"
                disabled={!canPlay}
                onClick={() => onResume(item)}
              >
                <PlayIcon size={14} /> Resume
              </button>
            )}
            <button className="button" onClick={() => onOpen(item)}>
              {openLabel}
            </button>
          </div>
        )}
      </div>
    </div>
  );
}

/** The page an item's cover morphs into (albums, artists, playlists open one with a header cover). */
function itemMorph(item: HomeItem): string | undefined {
  return item.kind === "album" || item.kind === "artist" || item.kind === "playlist"
    ? morphKey(item.kind, item.source, item.id)
    : undefined;
}

const KIND_SUBTITLE: Record<HomeItem["kind"], string> = {
  album: "Album",
  artist: "Artist",
  playlist: "Playlist",
  folder: "Folder",
  mix: "Mix",
};
