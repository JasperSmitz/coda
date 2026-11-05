// Navigation keys (DESIGN.md): Back with Backspace, Alt+←, Escape or the
// mouse back button; `/` or Ctrl+F search; `s` shuffles the shown
// context. Ignored while typing in a text field (Escape there leaves the
// field first).

import { useEffect } from "react";
import { shuffleContext } from "../lib/play";
import { nav } from "../state/nav";
import { isTyping } from "./useShortcuts";

export function useBrowseKeys(enabled: boolean) {
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.defaultPrevented) return;
      const typing = isTyping(e.target);
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "f" && !e.altKey) {
        nav.search();
      } else if (e.altKey && e.key === "ArrowLeft" && !typing) {
        nav.back();
      } else if (typing) {
        if (e.key === "Escape" && e.target instanceof HTMLElement) e.target.blur();
        else return;
      } else if (e.ctrlKey || e.metaKey || e.altKey) {
        return;
      } else if (e.key === "Backspace" || e.key === "Escape") {
        if (!nav.back()) return;
      } else if (e.key === "/") {
        nav.search();
      } else if (e.key === "s") {
        if (enabled) shuffleContext();
      } else {
        return;
      }
      e.preventDefault();
    };
    // Mouse "back" button (button 3).
    const onMouseUp = (e: MouseEvent) => {
      if (e.button === 3) {
        e.preventDefault();
        nav.back();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("mouseup", onMouseUp);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("mouseup", onMouseUp);
    };
  }, [enabled]);
}
