// Pieces shared by the browse views: the header, Play/Shuffle buttons,
// the scan progress line and count formatting.

import type { ReactNode } from "react";
import { BackIcon, PlayIcon, ShuffleIcon } from "../components/icons";
import { formatTime } from "../lib/format";
import { nav, useNav } from "../state/nav";
import { useAppState } from "../state/store";

export function plural(n: number, noun: string): string {
  return `${n.toLocaleString()} ${noun}${n === 1 ? "" : "s"}`;
}

/** `12 tracks · 48:10` from what is known. */
export function summary(tracks: number | null, durationMs: number | null): string {
  const parts: string[] = [];
  if (tracks !== null) parts.push(plural(tracks, "track"));
  if (durationMs !== null) parts.push(formatTime(durationMs));
  return parts.join(" · ");
}

export function useCanPlay(): boolean {
  const { snapshot } = useAppState();
  return snapshot?.phase === "ready" && snapshot.status !== "unavailable";
}

export function ViewHeader({
  eyebrow,
  title,
  children,
  actions,
  cover,
}: {
  eyebrow?: string;
  title: string;
  children?: ReactNode;
  actions?: ReactNode;
  /** A cover left of the title (artist, album, playlist pages). */
  cover?: ReactNode;
}) {
  const stack = useNav();
  return (
    <header className="view-header browse-header">
      <div className="browse-header-main">
        {stack.length > 1 && (
          <button
            className="icon-button back-button"
            onClick={() => nav.back()}
            title="Back (Backspace)"
            aria-label="Back"
          >
            <BackIcon />
          </button>
        )}
        {cover}
        <div className="browse-header-text">
          {eyebrow && <div className="eyebrow">{eyebrow}</div>}
          <h1>{title}</h1>
          {children}
        </div>
      </div>
      {actions && <div className="browse-header-actions">{actions}</div>}
    </header>
  );
}

export function PlayButtons({
  onPlay,
  onShuffle,
  disabled,
}: {
  onPlay?: () => void;
  onShuffle?: () => void;
  disabled?: boolean;
}) {
  const canPlay = useCanPlay();
  const off = disabled || !canPlay;
  return (
    <div className="play-buttons">
      {onPlay && (
        <button className="button button-primary" onClick={onPlay} disabled={off}>
          <PlayIcon size={14} /> Play
        </button>
      )}
      {onShuffle && (
        <button className="button" onClick={onShuffle} disabled={off} title="Shuffle (s)">
          <ShuffleIcon size={14} /> Shuffle
        </button>
      )}
    </div>
  );
}

/** "Scanning… 1,234 tracks so far" while the local scan runs. */
export function ScanLine() {
  const { snapshot } = useAppState();
  if (!snapshot || snapshot.scan.state !== "scanning") return null;
  return (
    <div className="scan-line" role="status">
      <span className="scan-dot" />
      Scanning the library… {plural(snapshot.scan.tracks, "track")} so far
    </div>
  );
}

export const SOURCE_LABEL = { local: "Local library", navidrome: "Navidrome" } as const;
