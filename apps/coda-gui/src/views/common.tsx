// Pieces shared by the browse views: the header, Play/Shuffle buttons,
// the scan progress line and count formatting.

import { useEffect, useRef, type KeyboardEvent, type ReactNode } from "react";
import { BackIcon, PlayIcon, ShuffleIcon } from "../components/icons";
import { formatTime } from "../lib/format";
import { SECTION_LABEL, sectionOf, sectionsFor, type Section } from "../lib/navModel";
import { dragRegion } from "../lib/platform";
import { nav, useActiveRoot, useNav, useNavState } from "../state/nav";
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
  const navState = useNavState();
  const root = useActiveRoot();
  // Library's root (a section) shows "Library" and the section control
  // instead of the section's own title.
  const section =
    root === "library" && stack.length === 1
      ? sectionOf(stack[0].route)
      : null;
  return (
    <header className="view-header browse-header" {...dragRegion}>
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
          <h1>{section ? "Library" : title}</h1>
          {section && <SectionControl section={section} source={navState.source} />}
          {children}
        </div>
      </div>
      {actions && <div className="browse-header-actions">{actions}</div>}
    </header>
  );
}

/** Set by the section control's arrow keys: focus the new section's radio once it shows. */
let focusSectionOnMount = false;

/** Library's sections as a segmented control (a radio group). */
function SectionControl({ section, source }: { section: Section; source: "local" | "navidrome" }) {
  const group = useRef<HTMLDivElement>(null);
  const sections = sectionsFor(source);
  useEffect(() => {
    if (!focusSectionOnMount) return;
    focusSectionOnMount = false;
    group.current?.querySelector<HTMLElement>('[aria-checked="true"]')?.focus();
  }, []);
  const choose = (next: Section) => {
    if (next === section) return;
    // The new section's view replaces this one (and this control): its
    // radio takes the focus.
    focusSectionOnMount = true;
    nav.setSection(next);
  };
  const onKeyDown = (e: KeyboardEvent) => {
    const at = sections.indexOf(section);
    let to: number;
    if (e.key === "ArrowRight" || e.key === "ArrowDown") to = (at + 1) % sections.length;
    else if (e.key === "ArrowLeft" || e.key === "ArrowUp")
      to = (at - 1 + sections.length) % sections.length;
    else if (e.key === "Home") to = 0;
    else if (e.key === "End") to = sections.length - 1;
    else return;
    // Not a seek (the global ←/→).
    e.preventDefault();
    e.stopPropagation();
    choose(sections[to]);
  };
  return (
    <div
      ref={group}
      className="section-control"
      role="radiogroup"
      aria-label="Library section"
      onKeyDown={onKeyDown}
    >
      {sections.map((s) => (
        <button
          key={s}
          role="radio"
          aria-checked={s === section}
          tabIndex={s === section ? 0 : -1}
          className={`section-option ${s === section ? "is-active" : ""}`}
          onClick={() => choose(s)}
        >
          {SECTION_LABEL[s]}
        </button>
      ))}
    </div>
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
