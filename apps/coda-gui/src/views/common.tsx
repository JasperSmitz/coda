// Pieces shared by the browse views: the page headers (a page about one
// thing: album, artist, genre, playlist, folder; and a list: a Library
// section and other lists), the sticky tool row, Play/Shuffle and the
// header's "…" menu, the scan progress line and count formatting.
//
// Every view scrolls in the page (App's `.main.page`, `usePageScroller`):
// its header scrolls away under the top bar; a list's tools (the section
// control, sorts, …) stay reachable in a tool row that sticks under the
// top bar.

import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type ReactNode,
} from "react";
import { ContextMenu, type MenuItem, type MenuState } from "../components/ContextMenu";
import { Cover } from "../components/Cover";
import { BackIcon, GridIcon, ListIcon, MoreIcon, PlayIcon, ShuffleIcon } from "../components/icons";
import { usePageScroller } from "../hooks/useLayout";
import { formatTime } from "../lib/format";
import type { LibraryView } from "../lib/types";
import { SECTION_LABEL, sectionOf, sectionsFor, type Section } from "../lib/navModel";
import { nav, useActiveRoot, useNav, useNavState } from "../state/nav";
import { useAppState } from "../state/store";

/** `3 albums`; nouns that do not just take an "s" pass their plural (`plural(n, "mix", "mixes")`). */
export function plural(n: number, noun: string, nouns = `${noun}s`): string {
  return `${n.toLocaleString()} ${n === 1 ? noun : nouns}`;
}

/** `12 tracks · 48:10` from what is known. */
export function summary(tracks: number | null, durationMs: number | null): string {
  const parts: string[] = [];
  if (tracks !== null) parts.push(plural(tracks, "track"));
  if (durationMs !== null) parts.push(formatTime(durationMs));
  return parts.join(" · ");
}

/** A length in words: `48 min`, `1 h 12 min`, `2 h`. */
export function formatLength(ms: number): string {
  const minutes = Math.max(1, Math.round(ms / 60000));
  const h = Math.floor(minutes / 60);
  const m = minutes % 60;
  if (h === 0) return `${m} min`;
  return m === 0 ? `${h} h` : `${h} h ${m} min`;
}

/** A page header's meta line: `9 songs · 48 min` from what is known. */
export function songsSummary(songs: number | null, durationMs: number | null): string {
  const parts: string[] = [];
  if (songs !== null) parts.push(plural(songs, "song"));
  if (durationMs !== null && durationMs > 0) parts.push(formatLength(durationMs));
  return parts.join(" · ");
}

export function useCanPlay(): boolean {
  const { snapshot } = useAppState();
  return snapshot?.phase === "ready" && snapshot.status !== "unavailable";
}

/** The page's ‹ Back (Backspace), shown above a header below a root. */
function BackButton() {
  return (
    <button
      className="icon-button back-button"
      onClick={() => nav.back()}
      title="Back (Backspace)"
      aria-label="Back"
    >
      <BackIcon />
    </button>
  );
}

/**
 * The header of a page about one thing (DESIGN.md "Library"): a large
 * cover (`#page-cover`, where the cover morph lands), the eyebrow, the
 * title in large display type (up to three lines), the byline, the meta
 * line and the actions. Narrow windows stack the cover above the text.
 */
export function PageHeader({
  cover,
  round = false,
  eyebrow,
  title,
  byline,
  meta,
  actions,
}: {
  /** The cover (or a mosaic); none: the text alone. */
  cover?: ReactNode;
  /** A round cover (artists). */
  round?: boolean;
  eyebrow?: string;
  title: string;
  byline?: ReactNode;
  meta?: ReactNode;
  actions?: ReactNode;
}) {
  const stack = useNav();
  return (
    <header className="page-header">
      {stack.length > 1 && <BackButton />}
      <div className={`page-header-main ${cover ? "" : "no-cover"}`}>
        {cover && (
          <div id="page-cover" className={`page-header-cover ${round ? "round" : ""}`}>
            {cover}
          </div>
        )}
        <div className="page-header-text">
          {eyebrow && <div className="eyebrow">{eyebrow}</div>}
          <h1 className="page-title" title={title}>
            {title}
          </h1>
          {byline && <div className="page-byline">{byline}</div>}
          {meta && <div className="page-meta secondary">{meta}</div>}
          {actions}
        </div>
      </div>
    </header>
  );
}

/**
 * The header of a list (a Library section, or a list opened from
 * elsewhere): at Library's root "Library" and the section control,
 * else the list's title; its tools in the sticky tool row.
 */
export function ListHeader({
  eyebrow,
  title,
  meta,
  tools,
}: {
  eyebrow?: string;
  title: string;
  meta?: ReactNode;
  /** Sorts, Shuffle, New…: in the tool row, right of the section control. */
  tools?: ReactNode;
}) {
  const stack = useNav();
  const navState = useNavState();
  const root = useActiveRoot();
  // Library's root (a section) shows "Library" and the section control
  // instead of the section's own title.
  const section =
    root === "library" && stack.length === 1 ? sectionOf(stack[0].route) : null;
  return (
    <>
      <header className="list-header">
        {stack.length > 1 && <BackButton />}
        {eyebrow && <div className="eyebrow">{eyebrow}</div>}
        <h1 className="list-title">{section ? "Library" : title}</h1>
        {meta && <div className="page-meta secondary">{meta}</div>}
      </header>
      {(section || tools) && (
        <ToolRow>
          {section && <SectionControl section={section} source={navState.source} />}
          {tools && <div className="tool-row-tools">{tools}</div>}
        </ToolRow>
      )}
    </>
  );
}

/**
 * A list's tools, sticking under the top bar once the header has
 * scrolled away (as glass then). While shown, the page keeps rows clear
 * of it when it scrolls one into view (`.main.page.has-tool-row`).
 */
export function ToolRow({ children }: { children: ReactNode }) {
  const scroller = usePageScroller();
  const mark = useRef<HTMLDivElement>(null);
  const row = useRef<HTMLDivElement>(null);
  const [stuck, setStuck] = useState(false);
  useLayoutEffect(() => {
    const root = scroller.current;
    const at = mark.current;
    const el = row.current;
    if (!root || !at || !el) return;
    root.classList.add("has-tool-row");
    // Where the row sticks (its `top`, below the top bar). The mark sits
    // just above the row: once it has passed that line, the row is stuck.
    const line = parseFloat(getComputedStyle(el).top) || 0;
    const observer = new IntersectionObserver(
      ([e]) => {
        const top = e.rootBounds?.top ?? root.getBoundingClientRect().top + line;
        setStuck(!e.isIntersecting && e.boundingClientRect.top < top);
      },
      { root, rootMargin: `-${line}px 0px 0px 0px`, threshold: 0 },
    );
    observer.observe(at);
    return () => {
      observer.disconnect();
      root.classList.remove("has-tool-row");
    };
  }, [scroller]);
  return (
    <>
      <div ref={mark} className="tool-row-mark" aria-hidden="true" />
      <div ref={row} className={`tool-row ${stuck ? "is-stuck" : ""}`}>
        {children}
      </div>
    </>
  );
}

/**
 * Up to four different covers as a 2×2 mosaic (a playlist without its
 * own cover, a genre); fewer: the first one alone.
 */
export function CoverMosaic({ urls, name }: { urls: (string | null)[]; name: string }) {
  const distinct = [...new Set(urls.filter((u): u is string => u !== null))].slice(0, 4);
  if (distinct.length < 4) {
    return <Cover url={distinct[0] ?? null} name={name} size="large" className="page-cover" />;
  }
  return (
    <div className="page-cover cover-mosaic" aria-hidden="true">
      {distinct.map((url) => (
        <Cover key={url} url={url} name={name} size="small" className="cover-mosaic-tile" />
      ))}
    </div>
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

/**
 * A page header's actions: Play (the accent pill), Shuffle (glass), any
 * other buttons, and "…" with the page's menu (Add to playlist…, Add to
 * mix…, …; hidden without items).
 */
export function PageActions({
  onPlay,
  onShuffle,
  disabled,
  menu,
  children,
}: {
  onPlay?: () => void;
  onShuffle?: () => void;
  disabled?: boolean;
  menu?: MenuItem[];
  children?: ReactNode;
}) {
  const [open, setOpen] = useState<MenuState | null>(null);
  const more = useRef<HTMLButtonElement>(null);
  const hasMenu = menu !== undefined && menu.length > 0;
  return (
    <div className="page-actions">
      <PlayButtons onPlay={onPlay} onShuffle={onShuffle} disabled={disabled} />
      {children}
      {hasMenu && (
        <button
          ref={more}
          className="icon-button page-more"
          aria-label="More"
          aria-haspopup="menu"
          aria-expanded={open !== null}
          title="More"
          onClick={(e) => {
            const rect = e.currentTarget.getBoundingClientRect();
            setOpen({ x: rect.left, y: rect.bottom + 6, key: "page" });
          }}
        >
          <MoreIcon size={18} />
        </button>
      )}
      {open && hasMenu && (
        <ContextMenu
          state={open}
          items={menu}
          onClose={() => {
            setOpen(null);
            more.current?.focus();
          }}
        />
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

/**
 * Covers / List (a section's tool row): an icon segmented control, a
 * radio group (arrow keys switch; not the global seek).
 */
export function ViewToggle({
  value,
  onChange,
}: {
  value: LibraryView;
  onChange: (view: LibraryView) => void;
}) {
  const group = useRef<HTMLDivElement>(null);
  const option = (view: LibraryView, label: string, icon: ReactNode) => (
    <button
      role="radio"
      aria-checked={value === view}
      aria-label={label}
      title={label}
      tabIndex={value === view ? 0 : -1}
      className={`view-option ${value === view ? "is-active" : ""}`}
      onClick={() => onChange(view)}
    >
      {icon}
    </button>
  );
  const onKeyDown = (e: KeyboardEvent) => {
    if (!["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"].includes(e.key)) return;
    e.preventDefault();
    e.stopPropagation();
    onChange(value === "covers" ? "list" : "covers");
    // The control re-renders in place: keep the focus on the chosen one.
    requestAnimationFrame(() =>
      group.current?.querySelector<HTMLElement>('[aria-checked="true"]')?.focus(),
    );
  };
  return (
    <div
      ref={group}
      className="view-toggle"
      role="radiogroup"
      aria-label="Show as"
      onKeyDown={onKeyDown}
    >
      {option("covers", "Covers", <GridIcon size={16} />)}
      {option("list", "List", <ListIcon size={16} />)}
    </div>
  );
}

/** A compact sort choice (a native select) for a tool row. */
export function SortSelect<T extends string>({
  value,
  options,
  onChange,
}: {
  value: T;
  options: [T, string][];
  onChange: (value: T) => void;
}) {
  return (
    <label className="sort-select">
      <span className="secondary">Sort</span>
      <select value={value} onChange={(e) => onChange(e.target.value as T)}>
        {options.map(([v, label]) => (
          <option key={v} value={v}>
            {label}
          </option>
        ))}
      </select>
    </label>
  );
}
