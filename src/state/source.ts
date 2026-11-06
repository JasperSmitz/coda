// The browsed source (owner feedback: one source at a time). The saved
// choice lives in `settings.toml` (`ui_source_get/set`); the source shown
// is that choice when it is configured, else the other configured one
// (default: Navidrome if configured, else local). Switching shows that
// source's own navigation stack (`nav.setSource`).

import { useSyncExternalStore } from "react";
import { errorText, sourceApi } from "../lib/ipc";
import type { SourceKind } from "../lib/types";
import { nav } from "./nav";
import { showUiNotice } from "./uiNotices";

export interface Sources {
  /** The source shown. */
  active: SourceKind;
  /** Configured sources, once known (`null` while starting). */
  configured: Record<SourceKind, boolean> | null;
  /** The saved choice has been read. */
  loaded: boolean;
}

let saved: SourceKind | null = null;
let state: Sources = { active: nav.source(), configured: null, loaded: false };
const listeners = new Set<() => void>();

/** The source to show for a saved choice and the configured sources. */
export function effectiveSource(
  choice: SourceKind | null,
  configured: Record<SourceKind, boolean> | null,
): SourceKind {
  if (!configured) return choice ?? "local";
  if (choice && configured[choice]) return choice;
  if (configured.navidrome) return "navidrome";
  if (configured.local) return "local";
  return choice ?? "local";
}

/** `fallback`: the shown source went away (Settings stays open then). */
function update(next: Partial<Sources>, fallback = false) {
  const merged = { ...state, ...next };
  merged.active = effectiveSource(saved, merged.configured);
  nav.setSource(merged.active, fallback);
  if (
    merged.active !== state.active ||
    merged.loaded !== state.loaded ||
    merged.configured !== state.configured
  ) {
    state = merged;
    listeners.forEach((l) => l());
  }
}

/** Reads the saved choice once (call at startup). */
export function startSources() {
  sourceApi.get().then(
    (choice) => {
      saved = saved ?? choice;
      update({ loaded: true });
    },
    () => update({ loaded: true }),
  );
}

/** The snapshot says which sources are configured. */
export function setConfigured(local: boolean, navidrome: boolean) {
  const c = state.configured;
  if (c && c.local === local && c.navidrome === navidrome) return;
  update({ configured: { local, navidrome } }, true);
}

/** The user picks a source (sidebar, Ctrl/Cmd+1/2, Go to album…). */
export function switchSource(source: SourceKind) {
  if (state.configured && !state.configured[source]) return;
  if (saved === source && state.active === source) return;
  saved = source;
  update({});
  sourceApi.set(source).catch((error: unknown) => {
    showUiNotice(`The source was not saved: ${errorText(error)}`);
  });
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function useSources(): Sources {
  return useSyncExternalStore(subscribe, () => state);
}

/** Both sources are set up (the sidebar shows the switch). */
export function bothConfigured(sources: Sources): boolean {
  return !!sources.configured?.local && !!sources.configured.navidrome;
}
