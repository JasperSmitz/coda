// Development builds only (`import.meta.env.DEV`, i.e. `pnpm tauri dev`):
// trace lines for the app's log, to see what the webview really delivers
// (2.0 R3: the root swipe's wheel events, hooks/useRootSwipe.ts). Lines
// are gathered and sent in batches (at most one call a second, at most
// `MAX_BATCH` lines each) through the debug-only `debug_trace` command
// (src-tauri `debug_trace.rs`, compiled only with `debug_assertions`),
// which writes them to `coda-app.log` (target `coda_app::trace`). Never
// anything private: only numbers and states. In a release build this is
// a no-op and the command does not exist.

import { invoke } from "@tauri-apps/api/core";

const MAX_BATCH = 500;
/** Lines kept while the log is not reachable (the oldest dropped). */
const MAX_PENDING = 5000;
const FLUSH_MS = 1000;

const pending: string[] = [];
let timer: number | null = null;

function flush() {
  timer = null;
  const lines = pending.splice(0, MAX_BATCH);
  if (lines.length === 0) return;
  invoke("debug_trace", { lines }).catch(() => {});
  if (pending.length > 0) timer = window.setTimeout(flush, FLUSH_MS);
}

/** Adds `line` to the log (development builds; else nothing). */
export function devTrace(line: string) {
  if (!import.meta.env.DEV) return;
  pending.push(`${performance.now().toFixed(1)} ${line}`);
  if (pending.length > MAX_PENDING) pending.splice(0, pending.length - MAX_PENDING);
  timer ??= window.setTimeout(flush, FLUSH_MS);
}
