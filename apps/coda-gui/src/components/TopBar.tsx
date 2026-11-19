// The top bar (DESIGN.md "Layout"): the coda sign and wordmark, the main
// navigation (Home, Search, Library) in a pill whose active entry is
// marked by one indicator that glides between entries, and Preferences.
// It floats over the page. On macOS it is the window's drag region and the
// native window buttons sit in its row (`titlebar.rs`, `--topbar-h`).

import { useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { dragRegion } from "../lib/platform";
import { nav, useActiveRoot, type Root } from "../state/nav";
import { CodaSignIcon, HomeIcon, LibraryIcon, SearchIcon, SlidersIcon } from "./icons";

const ENTRIES: { root: Root; label: string; icon: ReactNode; shortcut?: string }[] = [
  { root: "home", label: "Home", icon: <HomeIcon size={16} /> },
  { root: "search", label: "Search", icon: <SearchIcon size={16} />, shortcut: "/" },
  { root: "library", label: "Library", icon: <LibraryIcon size={16} /> },
];

/** The indicator's place under the active entry (`null`: none active). */
type Mark = { left: number; width: number } | null;

export function TopBar() {
  const active = useActiveRoot();
  const pill = useRef<HTMLDivElement>(null);
  const [mark, setMark] = useState<Mark>(null);
  // No glide for the first placement (or back from none active).
  const [glide, setGlide] = useState(false);

  useLayoutEffect(() => {
    const element = pill.current;
    if (!element) return;
    const measure = () => {
      const button = element.querySelector<HTMLElement>('[aria-current="page"]');
      setMark(button ? { left: button.offsetLeft, width: button.offsetWidth } : null);
    };
    measure();
    // Labels hide in narrow windows (CSS): the entries change size.
    const observer = new ResizeObserver(measure);
    element.querySelectorAll("button").forEach((button) => observer.observe(button));
    return () => observer.disconnect();
  }, [active]);

  const hasMark = mark !== null;
  useLayoutEffect(() => {
    if (!hasMark) {
      setGlide(false);
      return;
    }
    const frame = requestAnimationFrame(() => setGlide(true));
    return () => cancelAnimationFrame(frame);
  }, [hasMark]);

  const select = (root: Root) => {
    if (root === "search" && active === "search") nav.search();
    else nav.selectRoot(root);
  };

  return (
    <header className="topbar" {...dragRegion}>
      <div className="topbar-brand" aria-hidden="true">
        <CodaSignIcon size={18} className="topbar-sign" />
        <span className="topbar-wordmark">coda</span>
      </div>
      <nav className="topnav" aria-label="Main">
        <div className="topnav-pill" ref={pill}>
          <span
            className={`topnav-mark ${glide ? "is-gliding" : ""}`}
            aria-hidden="true"
            style={
              mark
                ? { transform: `translateX(${mark.left}px)`, width: mark.width }
                : { opacity: 0 }
            }
          />
          {ENTRIES.map(({ root, label, icon, shortcut }) => (
            <button
              key={root}
              className={`topnav-item ${active === root ? "is-active" : ""}`}
              aria-current={active === root ? "page" : undefined}
              aria-label={label}
              title={shortcut ? `${label} (${shortcut})` : label}
              onClick={() => select(root)}
            >
              {icon}
              <span className="topnav-label">{label}</span>
            </button>
          ))}
        </div>
      </nav>
      <div className="topbar-end">
        <button
          className={`icon-button topbar-prefs ${active === "prefs" ? "is-active" : ""}`}
          aria-current={active === "prefs" ? "page" : undefined}
          aria-label="Preferences"
          title="Preferences"
          onClick={() => nav.selectRoot("prefs")}
        >
          <SlidersIcon size={18} />
        </button>
      </div>
    </header>
  );
}
