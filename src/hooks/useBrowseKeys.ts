// Navigation keys (DESIGN.md): Back with Backspace, Alt+←, Escape or the
// mouse back button; `/` or Ctrl+F search; `s` shuffles the shown
// context. Ignored while typing in a text field (Escape there leaves the
// field first).

import { useEffect, useRef } from "react";
import { shuffleContext } from "../lib/play";
import { nav } from "../state/nav";
import { isTyping } from "./useShortcuts";

interface Options {
  /** `s` may play (playback available). */
  enabled: boolean;
  /** Navigation keys work at all (not during onboarding). */
  active: boolean;
  /** Back goes here instead of the stack (an overlay such as Now Playing). */
  onBack?: () => void;
}

export function useBrowseKeys({ enabled, active, onBack }: Options) {
  const state = useRef({ enabled, active, onBack });
  state.current = { enabled, active, onBack };
  useEffect(() => {
    const back = (): boolean => {
      const { onBack } = state.current;
      if (onBack) {
        onBack();
        return true;
      }
      return nav.back();
    };
    // Search replaces an overlay too.
    const search = () => {
      state.current.onBack?.();
      nav.search();
    };
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.defaultPrevented || !state.current.active) return;
      const { enabled } = state.current;
      const typing = isTyping(e.target);
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "f" && !e.altKey) {
        search();
      } else if (e.altKey && e.key === "ArrowLeft" && !typing) {
        back();
      } else if (typing) {
        if (e.key === "Escape" && e.target instanceof HTMLElement) e.target.blur();
        else return;
      } else if (e.ctrlKey || e.metaKey || e.altKey) {
        return;
      } else if (e.key === "Backspace" || e.key === "Escape") {
        if (!back()) return;
      } else if (e.key === "/") {
        search();
      } else if (e.key === "s") {
        if (enabled) shuffleContext();
      } else {
        return;
      }
      e.preventDefault();
    };
    // Mouse "back" button (button 3).
    const onMouseUp = (e: MouseEvent) => {
      if (e.button === 3 && state.current.active) {
        e.preventDefault();
        back();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("mouseup", onMouseUp);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("mouseup", onMouseUp);
    };
  }, []);
}
