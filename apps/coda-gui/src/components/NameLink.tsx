// A clickable artist or album name (DESIGN.md: no link styling; the
// accent colour on hover, the default cursor). A span with role="button"
// rather than a <button>: it stays inline text, so a line of names keeps
// its ellipsis, and it can sit inside a card's button-free text line or a
// row without nesting buttons. Enter activates it (Space stays play/pause,
// as on every button).
//
// In a row or a card (`inRow`) it is no Tab stop (rows and grids move by
// arrow keys; "Go to album/artist" is in their menus) and a press on it
// is the row's no business: it neither selects, plays nor starts a drag,
// and the focus stays where it was. Right-click still opens the row's menu.

import type { KeyboardEvent, MouseEvent, PointerEvent, ReactNode } from "react";
import { canGoToAlbum, canLinkArtist, goToAlbum, goToArtist } from "../lib/play";
import type { Track } from "../lib/types";

interface Props {
  children: ReactNode;
  /** The accessible name, e.g. "Go to artist X". */
  label: string;
  onGo: () => void;
  inRow?: boolean;
  className?: string;
}

/** Shift/Ctrl/Cmd: a row's multi-select click, left to the row. */
const modified = (e: { shiftKey: boolean; ctrlKey: boolean; metaKey: boolean }) =>
  e.shiftKey || e.ctrlKey || e.metaKey;

export function NameLink({ children, label, onGo, inRow = false, className }: Props) {
  const stop = (e: PointerEvent | MouseEvent) => {
    if (inRow && modified(e)) return;
    e.stopPropagation();
  };
  return (
    <span
      role="button"
      tabIndex={inRow ? -1 : 0}
      aria-label={label}
      className={`name-link ${inRow ? "in-row" : ""} ${className ?? ""}`}
      onPointerDown={stop}
      onMouseDown={(e) => {
        if (inRow && modified(e)) return;
        e.stopPropagation();
        // A row keeps the focus (its keyboard model).
        if (inRow) e.preventDefault();
      }}
      onDoubleClick={stop}
      onClick={(e) => {
        if (inRow && modified(e)) return;
        e.stopPropagation();
        onGo();
      }}
      onKeyDown={(e: KeyboardEvent) => {
        if (e.key !== "Enter" || e.repeat) return;
        e.preventDefault();
        e.stopPropagation();
        onGo();
      }}
    >
      {children}
    </span>
  );
}

/** A row's artist: a link when the artist is known, else plain text as before. */
export function TrackArtist({ track, className }: { track: Track; className: string }) {
  const name = track.artist ?? "Unknown artist";
  if (!track.artist || !canLinkArtist(track)) return <span className={className}>{name}</span>;
  return (
    <NameLink inRow className={className} label={`Go to artist ${name}`} onGo={() => goToArtist(track)}>
      {name}
    </NameLink>
  );
}

/** A row's album: a link when the album is known, else plain text as before. */
export function TrackAlbum({ track, className }: { track: Track; className: string }) {
  if (!track.album || !canGoToAlbum(track)) return <span className={className}>{track.album ?? ""}</span>;
  return (
    <NameLink inRow className={className} label={`Go to album ${track.album}`} onGo={() => goToAlbum(track)}>
      {track.album}
    </NameLink>
  );
}

/**
 * "Artist — Album" (the now-playing bar and view): each a link when known,
 * the separator plain text. Both are Tab stops. `onGo`: after navigating
 * (Now Playing closes, even when the page is already the top one).
 */
export function ArtistAlbumLine({ track, onGo }: { track: Track; onGo?: () => void }) {
  const parts: ReactNode[] = [];
  if (track.artist) {
    parts.push(
      canLinkArtist(track) ? (
        <NameLink
          key="artist"
          label={`Go to artist ${track.artist}`}
          onGo={() => {
            goToArtist(track);
            onGo?.();
          }}
        >
          {track.artist}
        </NameLink>
      ) : (
        track.artist
      ),
    );
  }
  if (track.album) {
    if (parts.length > 0) parts.push(" — ");
    parts.push(
      canGoToAlbum(track) ? (
        <NameLink
          key="album"
          label={`Go to album ${track.album}`}
          onGo={() => {
            goToAlbum(track);
            onGo?.();
          }}
        >
          {track.album}
        </NameLink>
      ) : (
        track.album
      ),
    );
  }
  return <>{parts}</>;
}
