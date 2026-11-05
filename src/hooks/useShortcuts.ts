// Global keyboard shortcuts (DESIGN.md), unless typing in a text field.
// Space play/pause, n/p or Ctrl+→/← next/previous, ←/→ seek ∓5 s,
// +/- volume, r repeat, q queue, f Now Playing.

import { useEffect, useRef } from "react";
import { intent } from "../lib/ipc";

const SEEK_STEP = 5;
const VOLUME_STEP = 5;

function isTyping(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.isContentEditable) return true;
  if (target instanceof HTMLTextAreaElement || target instanceof HTMLSelectElement) return true;
  if (target instanceof HTMLInputElement) {
    return !["button", "checkbox", "radio", "range", "submit", "reset"].includes(target.type);
  }
  return false;
}

interface Options {
  volume: number;
  enabled: boolean;
  onToggleQueue: () => void;
  onToggleNowPlaying?: () => void;
}

export function useShortcuts({ volume, enabled, onToggleQueue, onToggleNowPlaying }: Options) {
  // Read through a ref so the listener is installed once.
  const state = useRef({ volume, enabled, onToggleQueue, onToggleNowPlaying });
  state.current = { volume, enabled, onToggleQueue, onToggleNowPlaying };

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.defaultPrevented || isTyping(e.target) || e.metaKey || e.altKey) return;
      const { volume, enabled, onToggleQueue, onToggleNowPlaying } = state.current;
      const ctrl = e.ctrlKey;
      let handled = true;
      if (ctrl) {
        if (e.key === "ArrowRight") enabled && intent({ type: "next" });
        else if (e.key === "ArrowLeft") enabled && intent({ type: "previous" });
        else handled = false;
      } else {
        switch (e.key) {
          case " ":
            if (enabled) intent({ type: "togglePause" });
            break;
          case "n":
            if (enabled) intent({ type: "next" });
            break;
          case "p":
            if (enabled) intent({ type: "previous" });
            break;
          case "ArrowRight":
            if (enabled) intent({ type: "seekBy", seconds: SEEK_STEP });
            break;
          case "ArrowLeft":
            if (enabled) intent({ type: "seekBy", seconds: -SEEK_STEP });
            break;
          case "+":
          case "=":
            if (enabled) intent({ type: "setVolume", volume: Math.min(100, volume + VOLUME_STEP) });
            break;
          case "-":
            if (enabled) intent({ type: "setVolume", volume: Math.max(0, volume - VOLUME_STEP) });
            break;
          case "r":
            intent({ type: "cycleRepeat" });
            break;
          case "q":
            onToggleQueue();
            break;
          case "f":
            if (onToggleNowPlaying) onToggleNowPlaying();
            else handled = false;
            break;
          default:
            handled = false;
        }
      }
      // Also stops Space from clicking a focused button.
      if (handled) e.preventDefault();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);
}
