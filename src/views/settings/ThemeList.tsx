// The searchable theme picker: a virtualized listbox (Ghostty ships
// ~800 themes), one row per theme with a swatch strip and its source.
// Moving through it (arrow keys, pointer) previews the theme; Enter or a
// click applies it; Escape (or the pointer leaving) goes back to the
// applied one.

import { useEffect, useId, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { filterThemes, SOURCE_LABEL } from "../../lib/theme";
import type { ThemeSummary } from "../../lib/types";

const ROW = 40;
const HEIGHT = 320;
const OVERSCAN = 6;

interface Props {
  themes: ThemeSummary[];
  /** The applied theme (for the slot being chosen). */
  selected: string;
  disabled: boolean;
  onPreview: (name: string | null) => void;
  onApply: (name: string) => void;
  /** The highlighted row changed (actions under the list act on it). */
  onActive: (theme: ThemeSummary | null) => void;
}

export function ThemeList({ themes, selected, disabled, onPreview, onApply, onActive }: Props) {
  const [query, setQuery] = useState("");
  const queryRef = useRef(query);
  queryRef.current = query;
  const shown = useMemo(() => filterThemes(themes, query), [themes, query]);
  const [active, setActive] = useState<string | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const list = useRef<HTMLDivElement>(null);
  const id = useId();

  const activeName = active ?? selected;
  const activeIndex = shown.findIndex((t) => t.name === activeName);

  useEffect(() => {
    onActive(shown[activeIndex] ?? null);
  }, [shown, activeIndex, onActive]);

  // The applied theme in view (of the rows shown) when the list opens, the
  // slot or the list changes; any preview ends.
  useEffect(() => {
    setActive(null);
    onPreview(null);
    const i = filterThemes(themes, queryRef.current).findIndex((t) => t.name === selected);
    if (i >= 0 && list.current) list.current.scrollTop = Math.max(0, i * ROW - HEIGHT / 2 + ROW / 2);
  }, [selected, themes, onPreview]);

  const reveal = (index: number) => {
    const el = list.current;
    if (!el) return;
    if (index * ROW < el.scrollTop) el.scrollTop = index * ROW;
    else if ((index + 1) * ROW > el.scrollTop + HEIGHT) el.scrollTop = (index + 1) * ROW - HEIGHT;
  };

  const moveTo = (index: number) => {
    if (shown.length === 0) return;
    const i = Math.max(0, Math.min(shown.length - 1, index));
    setActive(shown[i].name);
    reveal(i);
    onPreview(shown[i].name);
  };

  const cancel = () => {
    setActive(null);
    onPreview(null);
  };

  const onKeyDown = (e: KeyboardEvent) => {
    const page = Math.floor(HEIGHT / ROW) - 1;
    const handled: Record<string, () => void> = {
      ArrowDown: () => moveTo(activeIndex + 1),
      ArrowUp: () => moveTo(activeIndex - 1),
      PageDown: () => moveTo(activeIndex + page),
      PageUp: () => moveTo(activeIndex - page),
      // Only a highlighted theme other than the applied one is applied.
      Enter: () => {
        if (active !== null && activeIndex >= 0 && shown[activeIndex].name !== selected) {
          onApply(shown[activeIndex].name);
        }
        setActive(null);
      },
      Escape: cancel,
    };
    // Home/End belong to the search field while typing in it.
    if (e.target === list.current) {
      handled.Home = () => moveTo(0);
      handled.End = () => moveTo(shown.length - 1);
    }
    const action = handled[e.key];
    if (!action || disabled) return;
    // Escape with nothing previewed leaves the field (and Back) alone.
    if (e.key === "Escape" && active === null) return;
    e.preventDefault();
    e.stopPropagation();
    action();
  };

  const first = Math.max(0, Math.floor(scrollTop / ROW) - OVERSCAN);
  const last = Math.min(shown.length, Math.ceil((scrollTop + HEIGHT) / ROW) + OVERSCAN);
  const optionId = (i: number) => `${id}-${i}`;

  return (
    <div className="theme-picker">
      <input
        type="search"
        className="theme-search"
        placeholder={`Search ${themes.length.toLocaleString()} themes`}
        aria-label="Search themes"
        aria-controls={id}
        value={query}
        disabled={disabled}
        onChange={(e) => {
          setQuery(e.target.value);
          setActive(null);
          onPreview(null);
          if (list.current) list.current.scrollTop = 0;
        }}
        onKeyDown={onKeyDown}
      />
      <div
        ref={list}
        id={id}
        className="theme-list"
        role="listbox"
        aria-label="Themes"
        aria-activedescendant={activeIndex >= first && activeIndex < last ? optionId(activeIndex) : undefined}
        aria-disabled={disabled}
        tabIndex={disabled ? -1 : 0}
        style={{ height: HEIGHT }}
        onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
        onKeyDown={onKeyDown}
        onPointerLeave={() => {
          if (active !== null) cancel();
        }}
      >
        <div style={{ height: shown.length * ROW, position: "relative" }}>
          {shown.slice(first, last).map((t, offset) => {
            const i = first + offset;
            return (
              <div
                key={t.name}
                id={optionId(i)}
                role="option"
                aria-selected={t.name === selected}
                className={`theme-row ${i === activeIndex ? "is-active" : ""} ${t.name === selected ? "is-selected" : ""}`}
                style={{ top: i * ROW, height: ROW }}
                onPointerEnter={() => {
                  if (disabled || t.name === active) return;
                  setActive(t.name);
                  onPreview(t.name);
                }}
                onClick={() => {
                  if (disabled) return;
                  if (t.name !== selected) onApply(t.name);
                  else onPreview(null);
                  setActive(null);
                }}
              >
                <Swatches theme={t} />
                <span className="theme-name">{t.name}</span>
                <span className={`theme-badge theme-badge-${t.source}`}>{SOURCE_LABEL[t.source]}</span>
                <span className="theme-check" aria-hidden="true">
                  {t.name === selected ? "✓" : ""}
                </span>
              </div>
            );
          })}
        </div>
        {shown.length === 0 && <div className="theme-empty secondary">No theme matches “{query}”.</div>}
      </div>
    </div>
  );
}

function Swatches({ theme }: { theme: ThemeSummary }) {
  return (
    <span className="theme-swatches" aria-hidden="true" style={{ background: theme.bg }}>
      {[theme.fg, theme.accent, ...theme.swatches].map((color, i) => (
        <span key={i} className="theme-swatch" style={{ background: color }} />
      ))}
    </span>
  );
}
